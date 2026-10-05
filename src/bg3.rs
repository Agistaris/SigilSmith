use anyhow::{bail, Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub const GAME_NAME: &str = "Baldur's Gate 3";
const STEAM_APP_ID: &str = "1086940";
// When Steam runs BG3 through Proton, the Windows build keeps user data in the
// prefix under the steamapps/ of the library the game is installed in.
const PROTON_LARIAN_SUBDIR: &str = "pfx/drive_c/users/steamuser/AppData/Local/Larian Studios";

#[derive(Debug, Clone)]
pub struct GamePaths {
    pub game_root: PathBuf,
    pub data_dir: PathBuf,
    pub larian_dir: PathBuf,
    pub larian_mods_dir: PathBuf,
    pub modsettings_path: PathBuf,
    #[allow(dead_code)]
    pub profiles_dir: PathBuf,
}

pub fn detect_paths(
    game_root_override: Option<&Path>,
    larian_dir_override: Option<&Path>,
) -> Result<GamePaths> {
    let game_root = match game_root_override {
        Some(path) => path.to_path_buf(),
        None => find_game_root().context("locate BG3 game directory")?,
    };

    let larian_dir = match larian_dir_override {
        Some(path) => path.to_path_buf(),
        None => find_larian_dir(&game_root).context("locate BG3 Larian data directory")?,
    };

    let data_dir = game_root.join("Data");
    let larian_mods_dir = larian_dir.join("Mods");
    let profiles_dir = larian_dir.join("PlayerProfiles");
    let modsettings_path = profiles_dir.join("Public").join("modsettings.lsx");

    if !looks_like_game_root(&game_root) {
        bail!(
            "invalid game root: expected Data/ and bin/ in {}",
            game_root.display()
        );
    }

    if !looks_like_larian_dir(&larian_dir) {
        bail!(
            "invalid Larian data dir: expected PlayerProfiles/ in {}",
            larian_dir.display()
        );
    }

    Ok(GamePaths {
        game_root,
        data_dir,
        larian_dir,
        larian_mods_dir,
        modsettings_path,
        profiles_dir,
    })
}

/// Larian data dir candidates, most likely first. Steam runs the native Linux
/// build unless a compatibility tool is forced for BG3, so the native path
/// leads unless Proton is forced; Proton prefixes start with the game's library.
pub fn larian_dir_candidates(game_root: Option<&Path>) -> Vec<PathBuf> {
    match dirs_home() {
        Some(home) => larian_dir_candidates_in(&home, game_root),
        None => Vec::new(),
    }
}

/// The Larian data dir Steam's launch settings say BG3 reads, when it exists
/// and differs from `larian_dir`.
pub fn larian_dir_mismatch(game_root: &Path, larian_dir: &Path) -> Option<PathBuf> {
    larian_dir_mismatch_in(&dirs_home()?, game_root, larian_dir)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SteamRuntime {
    Native,
    Proton,
    Unknown,
}

fn find_game_root() -> Option<PathBuf> {
    find_game_root_in(&dirs_home()?)
}

fn find_game_root_in(home: &Path) -> Option<PathBuf> {
    for lib in steam_libraries(home) {
        for folder in ["Baldurs Gate 3", "Baldur's Gate 3"] {
            let candidate = lib.join("steamapps/common").join(folder);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    None
}

fn find_larian_dir(game_root: &Path) -> Option<PathBuf> {
    let candidates = larian_dir_candidates(Some(game_root));
    candidates
        .iter()
        .find(|path| looks_like_larian_dir(path))
        .or_else(|| candidates.iter().find(|path| path.exists()))
        .cloned()
}

fn larian_dir_candidates_in(home: &Path, game_root: Option<&Path>) -> Vec<PathBuf> {
    let native = native_larian_dir(home);
    let mut proton = Vec::new();
    if let Some(dir) = game_root.and_then(proton_larian_dir_for_game_root) {
        proton.push(dir);
    }
    for lib in steam_libraries(home) {
        proton.push(proton_larian_dir(&lib.join("steamapps")));
    }

    let mut candidates = Vec::new();
    if steam_runtime(home) == SteamRuntime::Proton {
        candidates.extend(proton);
        candidates.push(native);
    } else {
        candidates.push(native);
        candidates.extend(proton);
    }

    dedup_paths(candidates)
}

fn larian_dir_mismatch_in(home: &Path, game_root: &Path, larian_dir: &Path) -> Option<PathBuf> {
    let proton = proton_larian_dir_for_game_root(game_root)?;
    let expected = match steam_runtime(home) {
        SteamRuntime::Proton => proton,
        SteamRuntime::Native => native_larian_dir(home),
        SteamRuntime::Unknown => return None,
    };
    if !looks_like_larian_dir(&expected) || same_path(&expected, larian_dir) {
        return None;
    }
    Some(expected)
}

fn native_larian_dir(home: &Path) -> PathBuf {
    home.join(".local/share/Larian Studios").join(GAME_NAME)
}

fn proton_larian_dir(steamapps: &Path) -> PathBuf {
    steamapps
        .join("compatdata")
        .join(STEAM_APP_ID)
        .join(PROTON_LARIAN_SUBDIR)
        .join(GAME_NAME)
}

fn proton_larian_dir_for_game_root(game_root: &Path) -> Option<PathBuf> {
    let common = game_root.parent()?;
    let steamapps = common.parent()?;
    if common.file_name()? != "common" || steamapps.file_name()? != "steamapps" {
        return None;
    }
    Some(proton_larian_dir(steamapps))
}

fn steam_roots(home: &Path) -> Vec<PathBuf> {
    dedup_paths(vec![
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        // Flatpak and Snap installs keep Steam inside their sandboxed home.
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        home.join("snap/steam/common/.local/share/Steam"),
    ])
}

fn steam_libraries(home: &Path) -> Vec<PathBuf> {
    let mut libraries = Vec::new();
    for root in steam_roots(home) {
        let vdf = root.join("steamapps/libraryfolders.vdf");
        if vdf.exists() {
            if let Ok(paths) = parse_steam_library_paths(&vdf) {
                libraries.extend(paths);
            }
        }
        libraries.push(root);
    }

    dedup_paths(libraries)
}

// A per-game CompatToolMapping entry forces Proton; the global "0" entry only
// covers titles without a Linux build, which no longer includes BG3.
fn steam_runtime(home: &Path) -> SteamRuntime {
    let mut runtime = SteamRuntime::Unknown;
    for root in steam_roots(home) {
        let Ok(raw) = fs::read_to_string(root.join("config/config.vdf")) else {
            continue;
        };
        if forced_compat_tool(&raw, STEAM_APP_ID).is_some() {
            return SteamRuntime::Proton;
        }
        runtime = SteamRuntime::Native;
    }
    runtime
}

fn forced_compat_tool(config_vdf: &str, app_id: &str) -> Option<String> {
    let mut lines = config_vdf.lines().map(str::trim);
    lines.find(|line| *line == "\"CompatToolMapping\"")?;

    let app_key = format!("\"{app_id}\"");
    let mut depth = 0;
    let mut in_app = false;
    for line in lines {
        if line == "{" {
            depth += 1;
        } else if line == "}" {
            depth -= 1;
            if depth <= 0 {
                return None;
            }
            in_app = false;
        } else if depth == 1 {
            in_app = line == app_key;
        } else if depth == 2 && in_app {
            let parts: Vec<&str> = line.split('"').collect();
            if parts.len() >= 4 && parts[1] == "name" && !parts[3].is_empty() {
                return Some(parts[3].to_string());
            }
        }
    }

    None
}

fn parse_steam_library_paths(path: &Path) -> Result<Vec<PathBuf>> {
    let raw = fs::read_to_string(path).context("read libraryfolders.vdf")?;
    let mut paths = Vec::new();

    for line in raw.lines() {
        let line = line.trim();
        if !line.contains("\"path\"") {
            continue;
        }

        let parts: Vec<&str> = line.split('"').collect();
        if parts.len() >= 4 {
            let path = parts[3].replace("\\\\", "\\");
            paths.push(PathBuf::from(path));
        }
    }

    Ok(paths)
}

// Keeps the first spelling of each path; ~/.steam/steam is usually a symlink
// to ~/.local/share/Steam.
fn dedup_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = Vec::new();
    let mut unique = Vec::new();
    for path in paths {
        let key = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !seen.contains(&key) {
            seen.push(key);
            unique.push(path);
        }
    }
    unique
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub fn looks_like_game_root(path: &Path) -> bool {
    path.join("Data").is_dir() && path.join("bin").is_dir()
}

pub fn looks_like_larian_dir(path: &Path) -> bool {
    path.join("PlayerProfiles").is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    const GLOBAL_PROTON_ONLY: &str = "\"InstallConfigStore\"\n{\n\t\"CompatToolMapping\"\n\t{\n\t\t\"0\"\n\t\t{\n\t\t\t\"name\"\t\t\"proton-cachyos-slr\"\n\t\t\t\"config\"\t\t\"\"\n\t\t}\n\t\t\"294100\"\n\t\t{\n\t\t\t\"name\"\t\t\"proton_experimental\"\n\t\t}\n\t}\n\t\"1086940\"\n\t{\n\t\t\"name\"\t\t\"not-a-mapping\"\n\t}\n}\n";
    const BG3_FORCED_PROTON: &str = "\"InstallConfigStore\"\n{\n\t\"CompatToolMapping\"\n\t{\n\t\t\"0\"\n\t\t{\n\t\t\t\"name\"\t\t\"proton_experimental\"\n\t\t}\n\t\t\"1086940\"\n\t\t{\n\t\t\t\"name\"\t\t\"proton-cachyos\"\n\t\t\t\"config\"\t\t\"\"\n\t\t\t\"priority\"\t\t\"250\"\n\t\t}\n\t}\n}\n";

    struct TempHome(PathBuf);

    impl TempHome {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("sigilsmith-bg3-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            TempHome(path)
        }

        fn steam(&self) -> PathBuf {
            self.0.join(".local/share/Steam")
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_steam_config(steam_root: &Path, config_vdf: &str) {
        fs::create_dir_all(steam_root.join("config")).unwrap();
        fs::write(steam_root.join("config/config.vdf"), config_vdf).unwrap();
    }

    fn make_game(library: &Path) -> PathBuf {
        let game_root = library.join("steamapps/common/Baldurs Gate 3");
        fs::create_dir_all(game_root.join("Data")).unwrap();
        fs::create_dir_all(game_root.join("bin")).unwrap();
        game_root
    }

    fn make_larian_dir(path: &Path) -> PathBuf {
        fs::create_dir_all(path.join("PlayerProfiles")).unwrap();
        path.to_path_buf()
    }

    fn pick(home: &Path, game_root: &Path) -> Option<PathBuf> {
        larian_dir_candidates_in(home, Some(game_root))
            .into_iter()
            .find(|path| looks_like_larian_dir(path))
    }

    #[test]
    fn reads_only_per_game_compat_tool_mappings() {
        assert_eq!(forced_compat_tool(GLOBAL_PROTON_ONLY, STEAM_APP_ID), None);
        assert_eq!(
            forced_compat_tool(BG3_FORCED_PROTON, STEAM_APP_ID).as_deref(),
            Some("proton-cachyos")
        );
    }

    #[test]
    fn prefers_native_dir_when_steam_runs_native_build() {
        let home = TempHome::new("native-run");
        write_steam_config(&home.steam(), GLOBAL_PROTON_ONLY);
        let game_root = make_game(&home.steam());
        let native = make_larian_dir(&native_larian_dir(&home.0));
        make_larian_dir(&proton_larian_dir(&home.steam().join("steamapps")));

        assert_eq!(pick(&home.0, &game_root), Some(native));
    }

    #[test]
    fn prefers_proton_prefix_when_steam_forces_proton() {
        let home = TempHome::new("forced-proton");
        write_steam_config(&home.steam(), BG3_FORCED_PROTON);
        let game_root = make_game(&home.steam());
        make_larian_dir(&native_larian_dir(&home.0));
        let proton = make_larian_dir(&proton_larian_dir(&home.steam().join("steamapps")));

        assert_eq!(pick(&home.0, &game_root), Some(proton));
    }

    #[test]
    fn uses_compatdata_from_the_games_library() {
        let home = TempHome::new("library");
        let library = home.0.join("games/SteamLibrary");
        write_steam_config(&home.steam(), BG3_FORCED_PROTON);
        fs::create_dir_all(home.steam().join("steamapps")).unwrap();
        fs::write(
            home.steam().join("steamapps/libraryfolders.vdf"),
            format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
                home.steam().display(),
                library.display()
            ),
        )
        .unwrap();
        let game_root = make_game(&library);
        let proton = make_larian_dir(&proton_larian_dir(&library.join("steamapps")));

        assert_eq!(find_game_root_in(&home.0), Some(game_root.clone()));
        assert_eq!(pick(&home.0, &game_root), Some(proton));
    }

    #[test]
    fn finds_flatpak_steam() {
        let home = TempHome::new("flatpak");
        let steam = home
            .0
            .join(".var/app/com.valvesoftware.Steam/.local/share/Steam");
        write_steam_config(&steam, BG3_FORCED_PROTON);
        let game_root = make_game(&steam);
        let proton = make_larian_dir(&proton_larian_dir(&steam.join("steamapps")));

        assert_eq!(find_game_root_in(&home.0), Some(game_root.clone()));
        assert_eq!(pick(&home.0, &game_root), Some(proton));
    }

    #[test]
    fn keeps_native_first_without_steam_config() {
        let home = TempHome::new("no-config");
        let game_root = make_game(&home.steam());
        let native = make_larian_dir(&native_larian_dir(&home.0));
        make_larian_dir(&proton_larian_dir(&home.steam().join("steamapps")));

        assert_eq!(pick(&home.0, &game_root), Some(native));
    }

    #[test]
    fn reports_mismatch_against_the_dir_steam_launches_with() {
        let home = TempHome::new("mismatch");
        let game_root = make_game(&home.steam());
        let proton = make_larian_dir(&proton_larian_dir(&home.steam().join("steamapps")));
        let native = make_larian_dir(&native_larian_dir(&home.0));
        let linked = home.0.join("linked");
        symlink(&proton, &linked).unwrap();

        // Unknown runtime: no warning.
        assert_eq!(larian_dir_mismatch_in(&home.0, &game_root, &native), None);

        write_steam_config(&home.steam(), BG3_FORCED_PROTON);
        assert_eq!(
            larian_dir_mismatch_in(&home.0, &game_root, &native),
            Some(proton.clone())
        );
        assert_eq!(larian_dir_mismatch_in(&home.0, &game_root, &proton), None);
        assert_eq!(larian_dir_mismatch_in(&home.0, &game_root, &linked), None);
        assert_eq!(
            larian_dir_mismatch_in(&home.0, &home.0.join("Games/BG3"), &native),
            None
        );

        write_steam_config(&home.steam(), GLOBAL_PROTON_ONLY);
        assert_eq!(
            larian_dir_mismatch_in(&home.0, &game_root, &proton),
            Some(native.clone())
        );
        assert_eq!(larian_dir_mismatch_in(&home.0, &game_root, &native), None);
    }
}
