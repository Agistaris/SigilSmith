use anyhow::{Context, Result};
use directories::BaseDirs;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    env,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    time::Duration,
};

const RELEASES_URL: &str = "https://api.github.com/repos/Agistaris/SigilSmith/releases/latest";
const USER_AGENT: &str = "SigilSmith";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateKind {
    AppImage,
    Deb,
    Rpm,
    Tarball,
}

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub version: String,
    pub kind: UpdateKind,
    pub asset_name: String,
}

#[derive(Debug, Clone)]
pub enum UpdateResult {
    UpToDate,
    Applied(UpdateInfo),
    Ready {
        info: UpdateInfo,
        path: PathBuf,
        instructions: String,
    },
    Skipped {
        version: String,
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub enum ApplyOutcome {
    Applied,
    Manual { instructions: String },
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: Option<u64>,
}

#[derive(Debug)]
struct UpdateTarget {
    kind: UpdateKind,
    appimage_path: Option<PathBuf>,
    current_exe: Option<PathBuf>,
}

pub fn check_for_updates(current_version: &str) -> Result<UpdateResult> {
    let release = fetch_latest_release()?;
    if release.prerelease {
        return Ok(UpdateResult::UpToDate);
    }

    let latest_version = normalize_version(&release.tag_name);
    if !is_newer_version(&latest_version, current_version) {
        return Ok(UpdateResult::UpToDate);
    }

    let target = detect_update_target();
    let arch = env::consts::ARCH;
    let asset = match select_asset(&release.assets, target.kind, arch) {
        Some(asset) => asset,
        None => {
            return Ok(UpdateResult::Skipped {
                version: latest_version,
                reason: format!("No {:?} asset found for {arch}", target.kind),
            });
        }
    };

    let checksums = fetch_checksums(&release.assets).unwrap_or_default();
    let update_dir = update_cache_dir()?;
    let asset_path = ensure_asset(&asset, &update_dir)?;
    if let Some(expected) = checksums.get(&asset.name) {
        verify_sha256(&asset_path, expected)?;
    }

    let info = UpdateInfo {
        version: latest_version.clone(),
        kind: target.kind,
        asset_name: asset.name.clone(),
    };

    match target.kind {
        UpdateKind::AppImage => {
            if let Some(appimage_path) = target.appimage_path {
                match apply_appimage_update(&asset_path, &appimage_path) {
                    Ok(()) => Ok(UpdateResult::Applied(info)),
                    Err(_) => Ok(UpdateResult::Ready {
                        info,
                        path: asset_path.clone(),
                        instructions: format!(
                            "Move update into place: mv '{}' '{}'",
                            asset_path.display(),
                            appimage_path.display()
                        ),
                    }),
                }
            } else {
                Ok(UpdateResult::Ready {
                    info,
                    path: asset_path.clone(),
                    instructions: format!(
                        "Move update into place: mv '{}' '<path-to-AppImage>'",
                        asset_path.display()
                    ),
                })
            }
        }
        UpdateKind::Deb => Ok(UpdateResult::Ready {
            info,
            path: asset_path.clone(),
            instructions: format!("Install update: sudo dpkg -i '{}'", asset_path.display()),
        }),
        UpdateKind::Rpm => Ok(UpdateResult::Ready {
            info,
            path: asset_path.clone(),
            instructions: format!("Install update: sudo rpm -Uvh '{}'", asset_path.display()),
        }),
        UpdateKind::Tarball => {
            let hint = target
                .current_exe
                .and_then(|path| path.parent().map(|parent| parent.display().to_string()))
                .unwrap_or_else(|| "<install-dir>".to_string());
            Ok(UpdateResult::Ready {
                info,
                path: asset_path.clone(),
                instructions: format!(
                    "Extract and replace: tar -xzf '{}' -C '{}'",
                    asset_path.display(),
                    hint
                ),
            })
        }
    }
}

pub fn apply_downloaded_update(info: &UpdateInfo, path: &Path) -> Result<ApplyOutcome> {
    match info.kind {
        UpdateKind::AppImage => {
            if let Some(appimage_path) = detect_update_target().appimage_path {
                match apply_appimage_update(path, &appimage_path) {
                    Ok(()) => Ok(ApplyOutcome::Applied),
                    Err(_) => Ok(ApplyOutcome::Manual {
                        instructions: format!(
                            "Move update into place: mv '{}' '{}'",
                            path.display(),
                            appimage_path.display()
                        ),
                    }),
                }
            } else {
                Ok(ApplyOutcome::Manual {
                    instructions: format!(
                        "Move update into place: mv '{}' '<path-to-AppImage>'",
                        path.display()
                    ),
                })
            }
        }
        UpdateKind::Deb => Ok(ApplyOutcome::Manual {
            instructions: format!("Install update: sudo dpkg -i '{}'", path.display()),
        }),
        UpdateKind::Rpm => Ok(ApplyOutcome::Manual {
            instructions: format!("Install update: sudo rpm -Uvh '{}'", path.display()),
        }),
        UpdateKind::Tarball => apply_tarball_update(path),
    }
}

fn fetch_latest_release() -> Result<Release> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(10))
        .timeout_write(Duration::from_secs(10))
        .build();
    let response = agent
        .get(RELEASES_URL)
        .set("User-Agent", USER_AGENT)
        .call()
        .context("fetch latest release")?;
    let release: Release = response.into_json().context("decode release")?;
    Ok(release)
}

fn normalize_version(tag: &str) -> String {
    tag.trim_start_matches('v').to_string()
}

fn is_newer_version(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

fn parse_version(raw: &str) -> Option<(u64, u64, u64)> {
    let raw = raw
        .trim_start_matches('v')
        .split('-')
        .next()?
        .split('+')
        .next()?;
    let mut parts = raw.split('.').map(|part| part.parse::<u64>().ok());
    let major = parts.next().flatten()?;
    let minor = parts.next().flatten()?;
    let patch = parts.next().flatten()?;
    Some((major, minor, patch))
}

fn detect_update_target() -> UpdateTarget {
    if let Ok(appimage) = env::var("APPIMAGE") {
        return UpdateTarget {
            kind: UpdateKind::AppImage,
            appimage_path: Some(PathBuf::from(appimage)),
            current_exe: env::current_exe().ok(),
        };
    }

    if let Some(appimage_path) = detect_appimage_path() {
        return UpdateTarget {
            kind: UpdateKind::AppImage,
            appimage_path: Some(appimage_path),
            current_exe: env::current_exe().ok(),
        };
    }

    if dpkg_installed() {
        return UpdateTarget {
            kind: UpdateKind::Deb,
            appimage_path: None,
            current_exe: env::current_exe().ok(),
        };
    }

    if rpm_installed() {
        return UpdateTarget {
            kind: UpdateKind::Rpm,
            appimage_path: None,
            current_exe: env::current_exe().ok(),
        };
    }

    UpdateTarget {
        kind: UpdateKind::Tarball,
        appimage_path: None,
        current_exe: env::current_exe().ok(),
    }
}

fn select_asset(assets: &[Asset], kind: UpdateKind, arch: &str) -> Option<Asset> {
    let arch = arch.to_lowercase();
    let aliases = arch_aliases(&arch);
    let match_asset = |asset: &Asset| match kind {
        UpdateKind::AppImage => {
            let name = asset.name.to_lowercase();
            asset.name.ends_with(".AppImage") && aliases.iter().any(|alias| name.contains(alias))
        }
        UpdateKind::Deb => {
            asset.name.ends_with(".deb") && {
                let name = asset.name.to_lowercase();
                name.contains("amd64") || aliases.iter().any(|alias| name.contains(alias))
            }
        }
        UpdateKind::Rpm => {
            asset.name.ends_with(".rpm") && {
                let name = asset.name.to_lowercase();
                aliases.iter().any(|alias| name.contains(alias))
            }
        }
        UpdateKind::Tarball => {
            asset.name.ends_with(".tar.gz") && asset.name.contains("linux") && {
                let name = asset.name.to_lowercase();
                aliases.iter().any(|alias| name.contains(alias))
            }
        }
    };

    assets.iter().find(|asset| match_asset(asset)).cloned()
}

fn arch_aliases(arch: &str) -> Vec<String> {
    match arch {
        "x86_64" => vec!["x86_64".to_string(), "amd64".to_string()],
        "aarch64" => vec!["aarch64".to_string(), "arm64".to_string()],
        "arm" | "armv7" | "armhf" => {
            vec!["arm".to_string(), "armv7".to_string(), "armhf".to_string()]
        }
        other => vec![other.to_string()],
    }
}

fn detect_appimage_path() -> Option<PathBuf> {
    let current = env::current_exe().ok()?;
    let file_name = current.file_name()?.to_string_lossy();
    if file_name.ends_with(".AppImage") {
        return Some(current);
    }
    None
}

fn dpkg_installed() -> bool {
    if Path::new("/var/lib/dpkg/info/sigilsmith.list").exists() {
        return true;
    }
    let Ok(entries) = fs::read_dir("/var/lib/dpkg/info") else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        entry
            .file_name()
            .to_string_lossy()
            .starts_with("sigilsmith.")
    })
}

fn rpm_installed() -> bool {
    if !(Path::new("/usr/bin/rpm").exists() || Path::new("/bin/rpm").exists()) {
        return false;
    }
    std::process::Command::new("rpm")
        .arg("-q")
        .arg("sigilsmith")
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn update_cache_dir() -> Result<PathBuf> {
    let base = BaseDirs::new().context("resolve cache dir")?;
    let dir = base.cache_dir().join("sigilsmith").join("updates");
    fs::create_dir_all(&dir).context("create update cache dir")?;
    Ok(dir)
}

fn ensure_asset(asset: &Asset, dir: &Path) -> Result<PathBuf> {
    let path = dir.join(&asset.name);
    if path.exists() {
        if let Some(expected) = asset.size {
            if let Ok(metadata) = fs::metadata(&path) {
                if metadata.len() == expected {
                    return Ok(path);
                }
            }
        } else {
            return Ok(path);
        }
    }

    download_asset(asset, &path)?;
    Ok(path)
}

fn download_asset(asset: &Asset, path: &Path) -> Result<()> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(60))
        .timeout_write(Duration::from_secs(60))
        .build();
    let response = agent
        .get(&asset.browser_download_url)
        .set("User-Agent", USER_AGENT)
        .call()
        .context("download asset")?;
    let mut reader = response.into_reader();
    let mut file = File::create(path).context("create asset file")?;
    io::copy(&mut reader, &mut file).context("write asset file")?;
    Ok(())
}

fn fetch_checksums(assets: &[Asset]) -> Result<HashMap<String, String>> {
    let checksum_asset = assets
        .iter()
        .find(|asset| asset.name == "SHA256SUMS.txt")
        .cloned()
        .context("missing SHA256SUMS")?;
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(10))
        .timeout_write(Duration::from_secs(10))
        .build();
    let response = agent
        .get(&checksum_asset.browser_download_url)
        .set("User-Agent", USER_AGENT)
        .call()
        .context("download SHA256SUMS")?;
    let body = response.into_string().context("read SHA256SUMS")?;
    let mut map = HashMap::new();
    for line in body.lines() {
        let mut parts = line.split_whitespace();
        let hash = match parts.next() {
            Some(value) => value.trim(),
            None => continue,
        };
        let name = match parts.next() {
            Some(value) => value.trim(),
            None => continue,
        };
        map.insert(name.to_string(), hash.to_lowercase());
    }
    Ok(map)
}

fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let mut file = File::open(path).context("open asset for checksum")?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected.to_lowercase() {
        return Err(anyhow::anyhow!("Checksum mismatch for {}", path.display()));
    }
    Ok(())
}

fn apply_appimage_update(asset_path: &Path, target: &Path) -> Result<()> {
    let parent = target.parent().context("resolve AppImage directory")?;
    let temp_path = parent.join(".sigilsmith-update");

    fs::copy(asset_path, &temp_path).context("stage AppImage update")?;
    set_executable(&temp_path)?;
    fs::rename(&temp_path, target).or_else(|_| {
        fs::copy(&temp_path, target).and_then(|_| {
            let _ = set_executable(target);
            fs::remove_file(&temp_path)
        })
    })?;
    Ok(())
}

fn set_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}

fn apply_tarball_update(path: &Path) -> Result<ApplyOutcome> {
    let current = env::current_exe().context("resolve current executable")?;
    let target_dir = match current.parent() {
        Some(dir) => dir.to_path_buf(),
        None => {
            return Ok(ApplyOutcome::Manual {
                instructions: format!(
                    "Extract and replace: tar -xzf '{}' -C '<install-dir>'",
                    path.display()
                ),
            })
        }
    };

    if !dir_writable(&target_dir) {
        return Ok(ApplyOutcome::Manual {
            instructions: format!(
                "Extract and replace: tar -xzf '{}' -C '{}'",
                path.display(),
                target_dir.display()
            ),
        });
    }

    let status = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(path)
        .arg("-C")
        .arg(&target_dir)
        .status()
        .context("run tar")?;

    if status.success() {
        Ok(ApplyOutcome::Applied)
    } else {
        Ok(ApplyOutcome::Manual {
            instructions: format!(
                "Extract and replace: tar -xzf '{}' -C '{}'",
                path.display(),
                target_dir.display()
            ),
        })
    }
}

fn dir_writable(dir: &Path) -> bool {
    let test_path = dir.join(".sigilsmith-write-test");
    match File::create(&test_path) {
        Ok(_) => {
            let _ = fs::remove_file(&test_path);
            true
        }
        Err(_) => false,
    }
}
