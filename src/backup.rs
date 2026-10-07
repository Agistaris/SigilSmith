use crate::{bg3::GamePaths, config::GameConfig, library::Library};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Each deploy writes one backup; older ones beyond this are deleted.
const KEEP_BACKUPS: usize = 50;

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

    prune_backups(&backup_root, KEEP_BACKUPS, &backup_dir);
    Ok(backup_dir)
}

/// The Unix time in a `backup-<time>` folder name.
pub fn backup_timestamp(backup_dir: &Path) -> Option<u64> {
    backup_dir
        .file_name()?
        .to_str()?
        .strip_prefix("backup-")?
        .parse()
        .ok()
}

/// Deletes all but the newest `keep` backup folders. Touches only
/// `backup-<time>` folders (not symlinks) directly inside `backup_root`, and
/// never `current`, even if a wrong clock made it look older than the rest.
fn prune_backups(backup_root: &Path, keep: usize, current: &Path) {
    let Ok(entries) = fs::read_dir(backup_root) else {
        return;
    };
    let mut backups: Vec<(u64, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let path = entry.path();
            backup_timestamp(&path).map(|stamp| (stamp, path))
        })
        .collect();
    if backups.len() <= keep {
        return;
    }
    backups.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in backups.into_iter().skip(keep) {
        if path != current {
            let _ = fs::remove_dir_all(path);
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_the_newest_backups() {
        let root = std::env::temp_dir().join(format!("sigilsmith-prune-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        for stamp in 1000..1060u64 {
            let dir = root.join(format!("backup-{stamp}"));
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("library.json"), "{}").unwrap();
        }
        // Anything that isn't a backup folder stays.
        fs::write(root.join("last.json"), "{}").unwrap();
        fs::create_dir_all(root.join("backup-notes")).unwrap();
        fs::write(root.join("backup-999"), "a file, not a folder").unwrap();

        // The newest backup has an older time, as after a clock change.
        let current = root.join("backup-500");
        fs::create_dir_all(&current).unwrap();
        prune_backups(&root, 50, &current);
        let mut left: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        let _ = fs::remove_dir_all(&root);

        let backups: Vec<&String> = left
            .iter()
            .filter(|name| backup_timestamp(Path::new(name)).is_some())
            .collect();
        // 50 folders, the current one and the file.
        assert_eq!(backups.len(), 52, "{left:?}");
        assert!(left.contains(&"backup-1010".to_string()));
        assert!(!left.contains(&"backup-1009".to_string()));
        assert!(left.contains(&"backup-1059".to_string()));
        assert!(left.contains(&"backup-999".to_string()));
        assert!(left.contains(&"backup-500".to_string()));
        assert!(left.contains(&"backup-notes".to_string()));
        assert!(left.contains(&"last.json".to_string()));
    }
}
