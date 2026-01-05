use anyhow::{bail, Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub const GAME_NAME: &str = "Baldur's Gate 3";
const STEAM_APP_ID: &str = "1086940";

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
        None => find_larian_dir().context("locate BG3 Larian data directory")?,
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

fn find_game_root() -> Option<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(home) = dirs_home() {
        candidates.push(home.join(".local/share/Steam"));
        candidates.push(home.join(".steam/steam"));
    }

    let mut libraries = Vec::new();
    for base in candidates {
        let vdf = base.join("steamapps/libraryfolders.vdf");
        if vdf.exists() {
            if let Ok(paths) = parse_steam_library_paths(&vdf) {
                libraries.extend(paths);
            }
        }
        libraries.push(base);
    }

    for lib in libraries {
        for folder in ["Baldurs Gate 3", "Baldur's Gate 3"] {
            let candidate = lib.join("steamapps/common").join(folder);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    None
}

fn find_larian_dir() -> Option<PathBuf> {
    let home = dirs_home()?;
    let native = home
        .join(".local/share/Larian Studios")
        .join(GAME_NAME);
    if native.exists() {
        return Some(native);
    }

    let proton = home
        .join(".local/share/Steam/steamapps/compatdata")
        .join(STEAM_APP_ID)
        .join("pfx/drive_c/users/steamuser/AppData/Local/Larian Studios")
        .join(GAME_NAME);
    if proton.exists() {
        return Some(proton);
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

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub fn looks_like_game_root(path: &Path) -> bool {
    path.join("Data").is_dir() && path.join("bin").is_dir()
}

pub fn looks_like_larian_dir(path: &Path) -> bool {
    path.join("PlayerProfiles").is_dir()
}
