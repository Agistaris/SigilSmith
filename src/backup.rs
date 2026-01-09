use crate::{bg3::GamePaths, config::GameConfig, library::Library};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Serialize, Deserialize)]
pub struct BackupMeta {
    pub timestamp: u64,
    pub reason: Option<String>,
    pub game: String,
    pub profile: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct LastBackup {
    path: PathBuf,
    timestamp: u64,
}

pub fn create_backup(
    config: &GameConfig,
    library: &Library,
    paths: &GamePaths,
    reason: Option<&str>,
) -> Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let backup_root = config.data_dir.join("backups");
    fs::create_dir_all(&backup_root).context("create backups dir")?;
    let backup_dir = backup_root.join(format!("backup-{stamp}"));
    fs::create_dir_all(&backup_dir).context("create backup dir")?;

    let library_json = serde_json::to_string_pretty(library).context("serialize library")?;
    fs::write(backup_dir.join("library.json"), library_json).context("write library backup")?;

    let manifest_path = config.data_dir.join("deploy_manifest.json");
    if manifest_path.exists() {
        let _ = fs::copy(&manifest_path, backup_dir.join("deploy_manifest.json"));
    }

    if paths.modsettings_path.exists() {
        let _ = fs::copy(&paths.modsettings_path, backup_dir.join("modsettings.lsx"));
    }

    let meta = BackupMeta {
        timestamp: stamp,
        reason: reason.map(|value| value.to_string()),
        game: config.game_name.clone(),
        profile: library.active_profile.clone(),
    };
    let meta_json = serde_json::to_string_pretty(&meta).context("serialize backup meta")?;
    fs::write(backup_dir.join("meta.json"), meta_json).context("write backup meta")?;

    let last = LastBackup {
        path: backup_dir.clone(),
        timestamp: stamp,
    };
    let last_json = serde_json::to_string_pretty(&last).context("serialize last backup")?;
    fs::write(backup_root.join("last.json"), last_json).context("write last backup")?;

    Ok(backup_dir)
}

pub fn load_last_backup(data_dir: &Path) -> Result<Option<PathBuf>> {
    let path = data_dir.join("backups").join("last.json");
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).context("read last backup")?;
    let last: LastBackup = serde_json::from_str(&raw).context("parse last backup")?;
    if last.path.exists() {
        Ok(Some(last.path))
    } else {
        Ok(None)
    }
}

pub fn load_backup_library(backup_dir: &Path) -> Result<Library> {
    let raw = fs::read_to_string(backup_dir.join("library.json")).context("read backup library")?;
    let library = serde_json::from_str(&raw).context("parse backup library")?;
    Ok(library)
}
