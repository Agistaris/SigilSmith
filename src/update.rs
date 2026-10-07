//! Finds newer SigilSmith releases on GitHub and installs them. Checking
//! never downloads a package. Installing downloads the package for this kind
//! of install, refuses it unless it matches a published SHA-256, and puts it
//! in place when SigilSmith can (an AppImage, or a .tar.gz install in a
//! folder it can write); otherwise it gives the command that installs it.

use anyhow::{bail, Context, Result};
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
const RELEASES_PAGE: &str = "https://github.com/Agistaris/SigilSmith/releases";
const USER_AGENT: &str = "SigilSmith";
/// Far bigger than any SigilSmith package; anything larger is refused.
const MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;

/// How SigilSmith was installed, which decides how it updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateKind {
    AppImage,
    Deb,
    Rpm,
    Tarball,
    /// Installed with pacman (for example from the AUR): pacman updates it.
    Pacman,
    /// Run from a cargo build folder: never replaced.
    SourceBuild,
}

impl UpdateKind {
    /// Whether SigilSmith downloads this kind of update itself.
    pub fn downloads(self) -> bool {
        !matches!(self, UpdateKind::Pacman | UpdateKind::SourceBuild)
    }
}

/// A release newer than the running version.
#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    /// What's New: the headline of each bullet in the release notes.
    pub notes: Vec<String>,
    pub page_url: String,
    pub kind: UpdateKind,
    /// The AppImage, or the binary of a .tar.gz install, that gets replaced.
    install_path: Option<PathBuf>,
    /// The package for this kind of install and machine, when there is one.
    asset: Option<Asset>,
    checksums_url: Option<String>,
}

impl Release {
    /// Whether "Update now" can download a package for this install.
    pub fn can_install(&self) -> bool {
        self.kind.downloads() && self.asset.is_some()
    }

    /// Whether installing puts the update in place without a command.
    pub fn installs_in_place(&self) -> bool {
        self.can_install()
            && matches!(self.kind, UpdateKind::AppImage | UpdateKind::Tarball)
            && self.install_path.is_some()
    }
}

#[derive(Debug, Clone)]
pub enum CheckResult {
    UpToDate,
    Available(Box<Release>),
}

#[derive(Debug, Clone)]
pub enum InstallOutcome {
    /// In place; starting `restart` runs the new version.
    Installed { restart: PathBuf },
    /// Downloaded and checked; `command` installs it (it needs sudo).
    Manual { path: PathBuf, command: String },
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: Option<u64>,
    /// "sha256:<hex>", published by GitHub for each release asset.
    #[serde(default)]
    digest: Option<String>,
}

#[derive(Debug)]
struct UpdateTarget {
    kind: UpdateKind,
    install_path: Option<PathBuf>,
}

/// Asks GitHub for the latest release. Downloads nothing.
pub fn check(current_version: &str) -> Result<CheckResult> {
    let release: GithubRelease = agent(Duration::from_secs(10))
        .get(&releases_url())
        .set("User-Agent", USER_AGENT)
        .call()
        .context("look up the latest release")?
        .into_json()
        .context("read the latest release")?;
    if release.prerelease || release.draft {
        return Ok(CheckResult::UpToDate);
    }
    let version = normalize_version(&release.tag_name);
    if !is_newer_version(&version, current_version) {
        return Ok(CheckResult::UpToDate);
    }

    let target = detect_update_target();
    let asset = if target.kind.downloads() {
        select_asset(&release.assets, target.kind, env::consts::ARCH)
    } else {
        None
    };
    let checksums_url = release
        .assets
        .iter()
        .find(|asset| asset.name == "SHA256SUMS.txt")
        .map(|asset| asset.browser_download_url.clone());
    Ok(CheckResult::Available(Box::new(Release {
        version,
        notes: release_notes(release.body.as_deref().unwrap_or_default()),
        page_url: release
            .html_url
            .unwrap_or_else(|| RELEASES_PAGE.to_string()),
        kind: target.kind,
        install_path: target.install_path,
        asset,
        checksums_url,
    })))
}

/// Downloads the release's package, checks it against its published SHA-256
/// and installs it where SigilSmith can.
pub fn install(release: &Release) -> Result<InstallOutcome> {
    if !release.kind.downloads() {
        bail!("this install updates through its package manager");
    }
    let asset = release
        .asset
        .as_ref()
        .context("the release has no package for this system")?;
    // Without SHA256SUMS.txt (missing or unreachable), GitHub's digest for
    // the asset still lets the download be checked.
    let sums = release
        .checksums_url
        .as_deref()
        .and_then(|url| fetch_text(url).ok());
    let expected = expected_sha256(&asset.name, sums.as_deref(), asset.digest.as_deref())?;
    let path = download_verified(asset, &expected, &update_cache_dir()?)?;
    let outcome = install_download(release, &path)?;
    if matches!(outcome, InstallOutcome::Installed { .. }) {
        // In place now; a manual install still needs the file.
        let _ = fs::remove_file(&path);
    }
    Ok(outcome)
}

fn install_download(release: &Release, path: &Path) -> Result<InstallOutcome> {
    match release.kind {
        UpdateKind::AppImage => {
            let Some(target) = &release.install_path else {
                return Ok(manual(path, format!("chmod +x {}", shell_quote(path))));
            };
            match apply_appimage_update(path, target) {
                Ok(()) => Ok(InstallOutcome::Installed {
                    restart: target.clone(),
                }),
                // Most likely a folder only root can write to, like /opt.
                Err(_) => Ok(manual(
                    path,
                    format!(
                        "sudo install -m 755 {} {}",
                        shell_quote(path),
                        shell_quote(target)
                    ),
                )),
            }
        }
        UpdateKind::Tarball => {
            let Some(target) = &release.install_path else {
                bail!("can't tell where SigilSmith is installed");
            };
            apply_tarball_update(path, target)
        }
        UpdateKind::Deb => Ok(manual(
            path,
            format!("sudo apt install {}", shell_quote(path)),
        )),
        UpdateKind::Rpm => Ok(manual(path, format!("sudo rpm -Uvh {}", shell_quote(path)))),
        UpdateKind::Pacman | UpdateKind::SourceBuild => unreachable!("checked above"),
    }
}

fn manual(path: &Path, command: String) -> InstallOutcome {
    InstallOutcome::Manual {
        path: path.to_path_buf(),
        command,
    }
}

fn agent(read_timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(read_timeout)
        .timeout_write(Duration::from_secs(10))
        .build()
}

fn releases_url() -> String {
    // Debug builds can point at a local test server.
    #[cfg(debug_assertions)]
    if let Ok(url) = env::var("SIGILSMITH_RELEASES_URL") {
        return url;
    }
    RELEASES_URL.to_string()
}

fn fetch_text(url: &str) -> Result<String> {
    agent(Duration::from_secs(10))
        .get(url)
        .set("User-Agent", USER_AGENT)
        .call()
        .with_context(|| format!("download {url}"))?
        .into_string()
        .context("read the download")
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

/// The bullets of a release's notes as plain text ("- **Bold.** `code`"
/// reads "Bold. code"), or its first lines when it has no bullets.
fn release_notes(body: &str) -> Vec<String> {
    let lines = body.lines().map(str::trim);
    let bullets: Vec<String> = lines
        .clone()
        .filter_map(|line| line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")))
        .map(|bullet| plain_text(headline(bullet)))
        .filter(|line| !line.is_empty())
        .collect();
    if !bullets.is_empty() {
        return bullets;
    }
    lines
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .take(3)
        .map(plain_text)
        .collect()
}

/// Markdown emphasis, code marks and links reduced to their text.
/// "**Headline.** More text" keeps only "Headline."; the release page has
/// the rest. Bullets without a bold start stay whole.
fn headline(bullet: &str) -> &str {
    for mark in ["**", "__"] {
        let Some(rest) = bullet.strip_prefix(mark) else {
            continue;
        };
        if let Some(end) = rest.find(mark).filter(|&end| end > 0) {
            return &rest[..end];
        }
    }
    bullet
}

fn plain_text(markdown: &str) -> String {
    let mut out = String::new();
    let mut rest = markdown;
    // "[text](url)" keeps only the text.
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find("](").map(|at| open + at) else {
            break;
        };
        let Some(end) = rest[close..].find(')').map(|at| close + at) else {
            break;
        };
        out.push_str(&rest[..open]);
        out.push_str(&rest[open + 1..close]);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out.replace("**", "")
        .replace("__", "")
        .replace('`', "")
        .trim()
        .to_string()
}

fn detect_update_target() -> UpdateTarget {
    let exe = env::current_exe().ok();
    let appimage = env::var_os("APPIMAGE").map(PathBuf::from);
    let appdir = env::var_os("APPDIR").map(PathBuf::from);
    if let Some(appimage) = running_appimage(exe.as_deref(), appimage, appdir.as_deref()) {
        return UpdateTarget {
            kind: UpdateKind::AppImage,
            install_path: appimage,
        };
    }
    let Some(exe) = exe else {
        return UpdateTarget {
            kind: UpdateKind::Tarball,
            install_path: None,
        };
    };
    let kind = if is_source_build(&exe) {
        UpdateKind::SourceBuild
    } else if pacman_owns(&exe) {
        UpdateKind::Pacman
    } else if dpkg_owns(&exe) {
        UpdateKind::Deb
    } else if rpm_owns(&exe) {
        UpdateKind::Rpm
    } else {
        UpdateKind::Tarball
    };
    UpdateTarget {
        kind,
        install_path: Some(exe),
    }
}

/// Whether this process runs from an AppImage: `Some(Some(file))` with the
/// AppImage file to replace, `Some(None)` from an AppImage's folder with no
/// file to replace (an extracted AppImage), `None` otherwise. The exe must
/// sit inside `APPDIR`: programs started from another AppImage inherit its
/// `APPIMAGE` and `APPDIR`, and replacing that file would break the other app.
fn running_appimage(
    exe: Option<&Path>,
    appimage: Option<PathBuf>,
    appdir: Option<&Path>,
) -> Option<Option<PathBuf>> {
    let exe = exe?;
    if exe.extension().is_some_and(|ext| ext == "AppImage") {
        return Some(Some(exe.to_path_buf()));
    }
    let appdir = appdir.filter(|dir| !dir.as_os_str().is_empty())?;
    exe.starts_with(appdir)
        .then(|| appimage.filter(|file| file.is_absolute()))
}

/// A binary in a cargo `target/release` or `target/debug` folder.
fn is_source_build(exe: &Path) -> bool {
    let mut dirs = exe.ancestors().skip(1);
    let profile = dirs.next().and_then(Path::file_name);
    let target = dirs.next().and_then(Path::file_name);
    target.is_some_and(|name| name == "target")
        && profile.is_some_and(|name| name == "release" || name == "debug")
}

/// Whether a pacman package lists this binary among its files.
fn pacman_owns(exe: &Path) -> bool {
    pacman_owns_in(Path::new("/var/lib/pacman/local"), exe)
}

fn pacman_owns_in(db: &Path, exe: &Path) -> bool {
    let Ok(relative) = exe.strip_prefix("/") else {
        return false;
    };
    let relative = relative.to_string_lossy();
    let Ok(packages) = fs::read_dir(db) else {
        return false;
    };
    packages.filter_map(Result::ok).any(|package| {
        fs::read_to_string(package.path().join("files"))
            .is_ok_and(|files| files.lines().any(|line| line == relative))
    })
}

/// Whether the sigilsmith .deb installed `exe` (its file list names it).
fn dpkg_owns(exe: &Path) -> bool {
    dpkg_owns_in(Path::new("/var/lib/dpkg/info"), exe)
}

fn dpkg_owns_in(info: &Path, exe: &Path) -> bool {
    let exe = exe.to_string_lossy();
    let Ok(entries) = fs::read_dir(info) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let name = entry.file_name().to_string_lossy().into_owned();
        // "sigilsmith.list", or "sigilsmith:amd64.list" on multiarch systems.
        let package = name.strip_suffix(".list").unwrap_or_default();
        (package == "sigilsmith" || package.starts_with("sigilsmith:"))
            && fs::read_to_string(entry.path())
                .is_ok_and(|files| files.lines().any(|line| line == exe))
    })
}

/// Whether the sigilsmith .rpm installed `exe`.
fn rpm_owns(exe: &Path) -> bool {
    if !(Path::new("/usr/bin/rpm").exists() || Path::new("/bin/rpm").exists()) {
        return false;
    }
    std::process::Command::new("rpm")
        .args(["-qf", "--queryformat", "%{NAME}"])
        .arg(exe)
        .stderr(std::process::Stdio::null())
        .output()
        .is_ok_and(|output| output.status.success() && output.stdout == b"sigilsmith")
}

fn select_asset(assets: &[Asset], kind: UpdateKind, arch: &str) -> Option<Asset> {
    let aliases = arch_aliases(&arch.to_lowercase());
    let for_arch = |name: &str| aliases.iter().any(|alias| name.contains(alias.as_str()));
    assets
        .iter()
        .find(|asset| {
            let name = asset.name.to_lowercase();
            match kind {
                UpdateKind::AppImage => asset.name.ends_with(".AppImage") && for_arch(&name),
                UpdateKind::Deb => name.ends_with(".deb") && for_arch(&name),
                UpdateKind::Rpm => name.ends_with(".rpm") && for_arch(&name),
                UpdateKind::Tarball => {
                    name.ends_with(".tar.gz") && name.contains("linux") && for_arch(&name)
                }
                UpdateKind::Pacman | UpdateKind::SourceBuild => false,
            }
        })
        .cloned()
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

/// The SHA-256 a download must match: its line in SHA256SUMS.txt, or else
/// GitHub's digest for the asset. With neither, the download can't be
/// checked, so it's refused; when both exist they must agree.
fn expected_sha256(name: &str, sums: Option<&str>, digest: Option<&str>) -> Result<String> {
    let from_sums = sums.and_then(|sums| parse_sha256sums(sums).remove(name));
    let from_digest = digest
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .map(str::to_lowercase)
        .filter(|hash| is_sha256(hash));
    match (from_sums, from_digest) {
        (Some(sums), Some(digest)) if sums != digest => {
            bail!("the published checksums for {name} disagree, so it wasn't installed")
        }
        (Some(hash), _) | (None, Some(hash)) => Ok(hash),
        (None, None) => {
            bail!("{name} has no published checksum to check it against, so it wasn't installed")
        }
    }
}

fn parse_sha256sums(body: &str) -> HashMap<String, String> {
    body.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let hash = parts.next()?.to_lowercase();
            // "*name" marks binary mode in sha256sum output.
            let name = parts.next()?.trim_start_matches('*');
            is_sha256(&hash).then(|| (name.to_string(), hash))
        })
        .collect()
}

fn is_sha256(hash: &str) -> bool {
    hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit())
}

fn update_cache_dir() -> Result<PathBuf> {
    let base = BaseDirs::new().context("find the cache folder")?;
    let dir = base.cache_dir().join("sigilsmith").join("updates");
    fs::create_dir_all(&dir).context("create the update folder")?;
    Ok(dir)
}

/// Downloads `asset` into `dir` (or reuses an earlier download) and returns
/// its path, but only once its SHA-256 matches `expected`.
fn download_verified(asset: &Asset, expected: &str, dir: &Path) -> Result<PathBuf> {
    // The name comes from the release: never let it point outside `dir`.
    let name = Path::new(&asset.name);
    if name.file_name() != Some(name.as_os_str()) || asset.name.starts_with('.') {
        bail!("unexpected package name {}", asset.name);
    }
    let path = dir.join(name);
    if path.exists() && sha256_file(&path).is_ok_and(|hash| hash == expected) {
        return Ok(path);
    }
    let partial = dir.join(format!(".{}.part", asset.name));
    let result = download_to(asset, &partial).and_then(|()| {
        if sha256_file(&partial)? != expected {
            bail!(
                "the {} download doesn't match its published checksum, so it wasn't installed",
                asset.name
            );
        }
        fs::rename(&partial, &path).context("save the download")
    });
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result.map(|()| path)
}

fn download_to(asset: &Asset, path: &Path) -> Result<()> {
    let limit = asset
        .size
        .map_or(MAX_DOWNLOAD_BYTES, |size| size.min(MAX_DOWNLOAD_BYTES));
    let reader = agent(Duration::from_secs(60))
        .get(&asset.browser_download_url)
        .set("User-Agent", USER_AGENT)
        .call()
        .with_context(|| format!("download {}", asset.name))?
        .into_reader();
    let mut file = File::create(path).context("create the download file")?;
    let written = io::copy(&mut reader.take(limit + 1), &mut file)
        .with_context(|| format!("download {}", asset.name))?;
    if written > limit {
        bail!("{} is bigger than expected", asset.name);
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).context("open the download")?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher).context("read the download")?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Copies the new AppImage next to the old one and renames it over it, so
/// the old file is never left half-written.
fn apply_appimage_update(asset_path: &Path, target: &Path) -> Result<()> {
    let parent = target.parent().context("find the AppImage's folder")?;
    let staged = parent.join(".sigilsmith-update");
    let result = fs::copy(asset_path, &staged)
        .context("stage the AppImage")
        .and_then(|_| set_executable(&staged))
        .and_then(|()| fs::rename(&staged, target).context("replace the AppImage"));
    if result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    result
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

/// Unpacks the release tarball next to the binary and renames the new
/// binary over the old one. A folder SigilSmith can't write gets a command.
fn apply_tarball_update(archive: &Path, exe: &Path) -> Result<InstallOutcome> {
    let dir = exe.parent().context("find SigilSmith's folder")?;
    let command = format!(
        "sudo tar -xzf {} -C {}",
        shell_quote(archive),
        shell_quote(dir)
    );
    let staging = dir.join(".sigilsmith-update");
    let _ = fs::remove_dir_all(&staging);
    if fs::create_dir(&staging).is_err() {
        return Ok(manual(archive, command));
    }
    let result = (|| {
        let status = std::process::Command::new("tar")
            .arg("-xzf")
            .arg(archive)
            .arg("-C")
            .arg(&staging)
            .status()
            .context("run tar")?;
        if !status.success() {
            bail!("tar couldn't unpack {}", archive.display());
        }
        let new_binary = staging.join("sigilsmith");
        if !fs::symlink_metadata(&new_binary).is_ok_and(|meta| meta.is_file()) {
            bail!("the archive has no sigilsmith binary");
        }
        set_executable(&new_binary)?;
        fs::rename(&new_binary, exe).context("replace the binary")
    })();
    let _ = fs::remove_dir_all(&staging);
    result.map(|()| InstallOutcome::Installed {
        restart: exe.to_path_buf(),
    })
}

/// Quotes a path for a shell command the user copies.
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH_A: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const HASH_B: &str = "6893b802f40a123990328e97f6f4ebbe9e3e1b9176e1e8068124c6833adea0f6";

    #[test]
    fn needs_a_published_checksum() {
        let sums = format!("{HASH_A}  app.AppImage\n{HASH_B} *other.deb\n");
        let digest_a = format!("sha256:{HASH_A}");
        let digest_b = format!("sha256:{}", HASH_B.to_uppercase());

        assert_eq!(
            expected_sha256("app.AppImage", Some(&sums), None).unwrap(),
            HASH_A
        );
        assert_eq!(
            expected_sha256("other.deb", Some(&sums), None).unwrap(),
            HASH_B
        );
        // No SHA256SUMS.txt, or no line for the file: GitHub's digest.
        assert_eq!(
            expected_sha256("app.AppImage", None, Some(&digest_b)).unwrap(),
            HASH_B
        );
        assert_eq!(
            expected_sha256("new.rpm", Some(&sums), Some(&digest_a)).unwrap(),
            HASH_A
        );
        // Both, agreeing.
        assert!(expected_sha256("app.AppImage", Some(&sums), Some(&digest_a)).is_ok());
        // Refused: they disagree, or nothing to check against.
        assert!(expected_sha256("app.AppImage", Some(&sums), Some(&digest_b)).is_err());
        assert!(expected_sha256("app.AppImage", None, None).is_err());
        assert!(expected_sha256("app.AppImage", Some("garbage"), None).is_err());
        assert!(expected_sha256("app.AppImage", None, Some("sha512:abc")).is_err());
        assert!(expected_sha256("app.AppImage", None, Some("sha256:short")).is_err());
    }

    #[test]
    fn downloads_only_keep_files_that_match() {
        let dir =
            std::env::temp_dir().join(format!("sigilsmith-update-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let asset = |name: &str| Asset {
            name: name.to_string(),
            // Nothing listens here, so only a cached file can satisfy it.
            browser_download_url: "http://127.0.0.1:9/none".to_string(),
            size: Some(3),
            digest: None,
        };
        // An earlier download with the right hash is reused.
        fs::write(dir.join("app.AppImage"), b"abc").unwrap();
        assert_eq!(
            download_verified(&asset("app.AppImage"), HASH_A, &dir).unwrap(),
            dir.join("app.AppImage")
        );
        // One with the wrong hash isn't, and a failed download leaves nothing.
        fs::write(dir.join("bad.AppImage"), b"abd").unwrap();
        assert!(download_verified(&asset("bad.AppImage"), HASH_A, &dir).is_err());
        assert!(!dir.join(".bad.AppImage.part").exists());
        // Names that would leave the folder are refused.
        assert!(download_verified(&asset("../escape"), HASH_A, &dir).is_err());
        assert!(download_verified(&asset(".hidden"), HASH_A, &dir).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn release_notes_read_as_plain_text() {
        let body = "## What's new in 0.9.8\n\n- **\"Only this mod\" in prompts.** Press `O`.\n- **Mods show their own names**, like BG3 Mod Manager.\n* See [the docs](https://example.com) for `O`.\n- **** Empty bold.\n\n**Install:** the AppImage.";
        assert_eq!(
            release_notes(body),
            [
                "\"Only this mod\" in prompts.",
                "Mods show their own names",
                "See the docs for O.",
                "Empty bold."
            ]
        );
        assert_eq!(
            release_notes("# Title\n\nJust a paragraph.\n"),
            ["Just a paragraph."]
        );
        assert!(release_notes("").is_empty());
    }

    #[test]
    fn spots_source_builds_and_pacman_installs() {
        assert!(is_source_build(Path::new(
            "/home/me/SigilSmith/target/release/sigilsmith"
        )));
        assert!(is_source_build(Path::new("/x/target/debug/sigilsmith")));
        assert!(!is_source_build(Path::new("/usr/bin/sigilsmith")));
        assert!(!is_source_build(Path::new("/opt/release/sigilsmith")));

        let db =
            std::env::temp_dir().join(format!("sigilsmith-pacman-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&db);
        fs::create_dir_all(db.join("sigilsmith-bin-0.9.8-1")).unwrap();
        fs::create_dir_all(db.join("other-1.0-1")).unwrap();
        fs::write(
            db.join("sigilsmith-bin-0.9.8-1/files"),
            "%FILES%\nusr/\nusr/bin/\nusr/bin/sigilsmith\n",
        )
        .unwrap();
        fs::write(db.join("other-1.0-1/files"), "%FILES%\nusr/bin/other\n").unwrap();
        assert!(pacman_owns_in(&db, Path::new("/usr/bin/sigilsmith")));
        assert!(!pacman_owns_in(&db, Path::new("/usr/local/bin/sigilsmith")));
        assert!(!pacman_owns_in(
            &db.join("missing"),
            Path::new("/usr/bin/sigilsmith")
        ));
        let _ = fs::remove_dir_all(&db);

        let info =
            std::env::temp_dir().join(format!("sigilsmith-dpkg-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&info);
        fs::create_dir_all(&info).unwrap();
        fs::write(
            info.join("sigilsmith:amd64.list"),
            "/.\n/usr\n/usr/bin/sigilsmith\n",
        )
        .unwrap();
        fs::write(info.join("sigilsmith-extra.list"), "/opt/sigilsmith\n").unwrap();
        fs::write(
            info.join("sigilsmith.md5sums"),
            "/usr/local/bin/sigilsmith\n",
        )
        .unwrap();
        assert!(dpkg_owns_in(&info, Path::new("/usr/bin/sigilsmith")));
        // A .deb installed elsewhere doesn't make a copy in ~/bin a .deb install.
        assert!(!dpkg_owns_in(&info, Path::new("/home/me/bin/sigilsmith")));
        assert!(!dpkg_owns_in(&info, Path::new("/opt/sigilsmith")));
        assert!(!dpkg_owns_in(&info, Path::new("/usr/local/bin/sigilsmith")));
        let _ = fs::remove_dir_all(&info);
    }

    #[test]
    fn spots_appimages_but_not_inherited_ones() {
        let file = || Some(PathBuf::from("/home/me/Apps/mods.AppImage"));
        let mount = Path::new("/tmp/.mount_modsAb12");
        // Started from the AppImage, whatever the file is called.
        assert_eq!(
            running_appimage(Some(&mount.join("usr/bin/sigilsmith")), file(), Some(mount)),
            Some(file())
        );
        // Started by another AppImage: its variables are inherited.
        assert_eq!(
            running_appimage(
                Some(Path::new("/usr/bin/sigilsmith")),
                Some(PathBuf::from("/home/me/Apps/Kitty.AppImage")),
                Some(Path::new("/tmp/.mount_kittyX")),
            ),
            None
        );
        // An extracted AppImage: nothing to replace.
        let extracted = Path::new("/home/me/squashfs-root");
        assert_eq!(
            running_appimage(
                Some(&extracted.join("usr/bin/sigilsmith")),
                None,
                Some(extracted)
            ),
            Some(None)
        );
        let named = Path::new("/home/me/sigilsmith-0.9.8-x86_64.AppImage");
        assert_eq!(
            running_appimage(Some(named), None, None),
            Some(Some(named.to_path_buf()))
        );
        assert_eq!(
            running_appimage(Some(Path::new("/usr/bin/sigilsmith")), None, None),
            None
        );
        assert_eq!(
            running_appimage(
                Some(Path::new("/usr/bin/sigilsmith")),
                file(),
                Some(Path::new(""))
            ),
            None
        );
    }

    #[test]
    fn picks_the_package_for_this_install() {
        let assets: Vec<Asset> = [
            "SHA256SUMS.txt",
            "sigilsmith-0.9.9-1.x86_64.rpm",
            "sigilsmith-0.9.9-linux-x86_64.tar.gz",
            "sigilsmith-0.9.9-x86_64.AppImage",
            "sigilsmith_0.9.9-1_amd64.deb",
        ]
        .iter()
        .map(|name| Asset {
            name: name.to_string(),
            browser_download_url: String::new(),
            size: None,
            digest: None,
        })
        .collect();
        let pick = |kind, arch| select_asset(&assets, kind, arch).map(|asset| asset.name);
        assert_eq!(
            pick(UpdateKind::AppImage, "x86_64").as_deref(),
            Some("sigilsmith-0.9.9-x86_64.AppImage")
        );
        assert_eq!(
            pick(UpdateKind::Deb, "x86_64").as_deref(),
            Some("sigilsmith_0.9.9-1_amd64.deb")
        );
        assert_eq!(
            pick(UpdateKind::Rpm, "x86_64").as_deref(),
            Some("sigilsmith-0.9.9-1.x86_64.rpm")
        );
        assert_eq!(
            pick(UpdateKind::Tarball, "x86_64").as_deref(),
            Some("sigilsmith-0.9.9-linux-x86_64.tar.gz")
        );
        assert_eq!(pick(UpdateKind::AppImage, "aarch64"), None);
        assert_eq!(pick(UpdateKind::Pacman, "x86_64"), None);
    }

    #[test]
    fn compares_versions() {
        assert!(is_newer_version("0.9.10", "0.9.9"));
        assert!(!is_newer_version("0.9.9", "0.9.9"));
        assert!(!is_newer_version("0.9.8", "0.9.9"));
        assert!(is_newer_version("1.0.0", "0.9.9"));
        assert!(!is_newer_version("garbage", "0.9.9"));
    }

    #[test]
    fn quotes_paths_for_the_shell() {
        assert_eq!(shell_quote(Path::new("/a b/c")), "'/a b/c'");
        assert_eq!(shell_quote(Path::new("/it's")), r"'/it'\''s'");
    }
}
