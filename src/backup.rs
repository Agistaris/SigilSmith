use crate::{
    config::GameConfig,
    library::{is_sigillink_ranking_profile, Library, Profile},
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Each deploy writes one backup; older ones beyond this are deleted.
pub const KEEP_BACKUPS: usize = 50;

/// Reason recorded for the backup a restore saves first, so it can be undone.
pub const BEFORE_RESTORE: &str = "before restore";

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
    modsettings_path: Option<&Path>,
    reason: Option<&str>,
) -> Result<PathBuf> {
    let mut stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let backup_root = config.data_dir.join("backups");
    fs::create_dir_all(&backup_root).context("create backups dir")?;
    // Two backups in the same second must not share a folder.
    while backup_root.join(format!("backup-{stamp}")).exists() {
        stamp += 1;
    }
    let backup_dir = backup_root.join(format!("backup-{stamp}"));
    fs::create_dir_all(&backup_dir).context("create backup dir")?;

    let library_json = serde_json::to_string_pretty(library).context("serialize library")?;
    fs::write(backup_dir.join("library.json"), library_json).context("write library backup")?;

    let manifest_path = config.data_dir.join("deploy_manifest.json");
    if manifest_path.exists() {
        let _ = fs::copy(&manifest_path, backup_dir.join("deploy_manifest.json"));
    }

    if let Some(modsettings_path) = modsettings_path.filter(|path| path.exists()) {
        let _ = fs::copy(modsettings_path, backup_dir.join("modsettings.lsx"));
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

pub fn load_backup_library(backup_dir: &Path) -> Result<Library> {
    let raw = fs::read_to_string(backup_dir.join("library.json")).context("read backup library")?;
    let library = serde_json::from_str(&raw).context("parse backup library")?;
    Ok(library)
}

#[derive(Debug, Clone)]
pub struct BackupEntry {
    pub dir: PathBuf,
    pub timestamp: u64,
    /// What started the deploy, e.g. "auto: order changed" or "manual deploy".
    pub reason: Option<String>,
}

/// Every backup folder, newest first.
pub fn list_backups(data_dir: &Path) -> Vec<BackupEntry> {
    let Ok(entries) = fs::read_dir(data_dir.join("backups")) else {
        return Vec::new();
    };
    let mut backups: Vec<BackupEntry> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let dir = entry.path();
            let timestamp = backup_timestamp(&dir)?;
            let reason = fs::read_to_string(dir.join("meta.json"))
                .ok()
                .and_then(|raw| serde_json::from_str::<BackupMeta>(&raw).ok())
                .and_then(|meta| meta.reason);
            Some(BackupEntry {
                dir,
                timestamp,
                reason,
            })
        })
        .collect();
    backups.sort_unstable_by(|a, b| b.timestamp.cmp(&a.timestamp));
    backups
}

/// A deploy reason as a short label: "auto: order changed" -> "Order changed".
pub fn reason_label(reason: Option<&str>) -> String {
    let Some(reason) = reason.map(str::trim).filter(|reason| !reason.is_empty()) else {
        return "Unknown".to_string();
    };
    let reason = reason.strip_prefix("auto: ").unwrap_or(reason);
    let reason = reason.replace("sigillink", "SigiLink");
    let mut chars = reason.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "Unknown".to_string(),
    }
}

/// A kind of change a restore makes, shown as a colored label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Profile,
    Moved,
    On,
    Off,
    ReAdded,
    Removed,
    Targets,
    Overrides,
    Profiles,
    Deploy,
}

impl ChangeKind {
    pub fn label(self) -> &'static str {
        match self {
            ChangeKind::Profile => "Switch to",
            ChangeKind::Moved => "Moved",
            ChangeKind::On => "On",
            ChangeKind::Off => "Off",
            ChangeKind::ReAdded => "Re-added",
            ChangeKind::Removed => "Removed",
            ChangeKind::Targets => "Targets",
            ChangeKind::Overrides => "Overrides",
            ChangeKind::Profiles => "Profiles",
            ChangeKind::Deploy => "Deploy",
        }
    }
}

/// What restoring a backup would change, compared with the library now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestoreChanges {
    /// The profile that is active after restoring.
    pub profile: String,
    /// The backup makes a different profile active than the one in use now.
    pub switches_profile: bool,
    /// Mods in that profile that change places: (id, position now, position
    /// after), counting from 1. Mods only pushed along by others aren't listed.
    pub moved: Vec<(String, usize, usize)>,
    pub turned_on: Vec<String>,
    pub turned_off: Vec<String>,
    /// Mods that come back to the list, and mods that leave it.
    pub returning: Vec<String>,
    pub leaving: Vec<String>,
    /// Mods whose target choices (Auto/Mods/Gen/Data/Bin) change.
    pub targets: Vec<String>,
    /// Overrides-panel winner choices in that profile that change.
    pub overrides: usize,
    pub profiles_returning: Vec<String>,
    pub profiles_leaving: Vec<String>,
    /// Other profiles whose order, on/off or overrides change.
    pub other_profiles: Vec<String>,
}

impl RestoreChanges {
    pub fn is_empty(&self) -> bool {
        *self
            == RestoreChanges {
                profile: self.profile.clone(),
                ..RestoreChanges::default()
            }
    }

    /// Short counts for the backup list, each with its kind for coloring:
    /// "1 moved", "2 on", "1 removed".
    pub fn short_parts(&self) -> Vec<(ChangeKind, String)> {
        let count =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut parts = Vec::new();
        if self.switches_profile {
            parts.push((ChangeKind::Profile, format!("switch to {}", self.profile)));
        }
        let mut push = |kind: ChangeKind, n: usize, one: &str, many: &str| {
            if n > 0 {
                parts.push((kind, count(n, one, many)));
            }
        };
        push(ChangeKind::Moved, self.moved.len(), "moved", "moved");
        push(ChangeKind::On, self.turned_on.len(), "on", "on");
        push(ChangeKind::Off, self.turned_off.len(), "off", "off");
        push(
            ChangeKind::ReAdded,
            self.returning.len(),
            "re-added",
            "re-added",
        );
        push(
            ChangeKind::Removed,
            self.leaving.len(),
            "removed",
            "removed",
        );
        push(ChangeKind::Targets, self.targets.len(), "target", "targets");
        push(
            ChangeKind::Overrides,
            self.overrides,
            "override",
            "overrides",
        );
        let profiles =
            self.profiles_returning.len() + self.profiles_leaving.len() + self.other_profiles.len();
        push(ChangeKind::Profiles, profiles, "profile", "profiles");
        parts
    }
}

/// Compares the library now with the one restoring `then` would produce.
pub fn restore_changes(now: &Library, then: &Library) -> RestoreChanges {
    let profile = restored_active_profile(then);
    let mut changes = RestoreChanges {
        switches_profile: profile != now.active_profile,
        profile: profile.clone(),
        ..RestoreChanges::default()
    };

    let now_ids: HashSet<&str> = now.mods.iter().map(|entry| entry.id.as_str()).collect();
    let then_ids: HashSet<&str> = then.mods.iter().map(|entry| entry.id.as_str()).collect();
    changes.returning = then
        .mods
        .iter()
        .filter(|entry| !now_ids.contains(entry.id.as_str()))
        .map(|entry| entry.id.clone())
        .collect();
    changes.leaving = now
        .mods
        .iter()
        .filter(|entry| !then_ids.contains(entry.id.as_str()))
        .map(|entry| entry.id.clone())
        .collect();
    let now_mods: HashMap<&str, _> = now
        .mods
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect();
    for entry in &then.mods {
        if let Some(current) = now_mods.get(entry.id.as_str()) {
            let mut before: Vec<_> = current
                .target_overrides
                .iter()
                .map(|value| (value.kind as u8, value.enabled))
                .collect();
            let mut after: Vec<_> = entry
                .target_overrides
                .iter()
                .map(|value| (value.kind as u8, value.enabled))
                .collect();
            before.sort_unstable();
            after.sort_unstable();
            if before != after {
                changes.targets.push(entry.id.clone());
            }
        }
    }

    let find = |library: &'_ Library, name: &str| -> Option<Profile> {
        library
            .profiles
            .iter()
            .find(|candidate| candidate.name == name)
            .cloned()
    };
    if let (Some(before), Some(after)) = (find(now, &profile), find(then, &profile)) {
        changes.moved = moved_mods(&before, &after);
        let before_enabled: HashMap<&str, bool> = before
            .order
            .iter()
            .map(|entry| (entry.id.as_str(), entry.enabled))
            .collect();
        for entry in &after.order {
            match before_enabled.get(entry.id.as_str()) {
                Some(false) if entry.enabled => changes.turned_on.push(entry.id.clone()),
                Some(true) if !entry.enabled => changes.turned_off.push(entry.id.clone()),
                _ => {}
            }
        }
        changes.overrides = changed_overrides(&before, &after);
    }

    let visible = |library: &Library| -> Vec<String> {
        library
            .profiles
            .iter()
            .filter(|candidate| !is_sigillink_ranking_profile(&candidate.name))
            .map(|candidate| candidate.name.clone())
            .collect()
    };
    let now_names = visible(now);
    let then_names = visible(then);
    changes.profiles_returning = then_names
        .iter()
        .filter(|name| !now_names.contains(name))
        .cloned()
        .collect();
    changes.profiles_leaving = now_names
        .iter()
        .filter(|name| !then_names.contains(name))
        .cloned()
        .collect();
    for name in then_names.iter().filter(|name| **name != profile) {
        if let (Some(before), Some(after)) = (find(now, name), find(then, name)) {
            let order = |candidate: &Profile| -> Vec<(String, bool)> {
                candidate
                    .order
                    .iter()
                    .map(|entry| (entry.id.clone(), entry.enabled))
                    .collect()
            };
            if order(&before) != order(&after) || changed_overrides(&before, &after) > 0 {
                changes.other_profiles.push(name.clone());
            }
        }
    }
    changes
}

/// The profile a restored library makes active: the backup's own, unless it
/// is missing or the hidden SigiLink ranking profile.
fn restored_active_profile(then: &Library) -> String {
    let usable = |name: &str| {
        !is_sigillink_ranking_profile(name)
            && then.profiles.iter().any(|profile| profile.name == name)
    };
    if usable(&then.active_profile) {
        return then.active_profile.clone();
    }
    then.profiles
        .iter()
        .map(|profile| profile.name.clone())
        .find(|name| !is_sigillink_ranking_profile(name))
        .unwrap_or_else(|| "Default".to_string())
}

/// Mods that change places between two orders, leaving out the ones that
/// only shift because others moved: everything outside the longest run that
/// keeps its relative order.
fn moved_mods(before: &Profile, after: &Profile) -> Vec<(String, usize, usize)> {
    let after_pos: HashMap<&str, usize> = after
        .order
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.id.as_str(), index))
        .collect();
    // Mods in both orders, in today's order: (id, position now, position after).
    let common: Vec<(&str, usize, usize)> = before
        .order
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            after_pos
                .get(entry.id.as_str())
                .map(|after| (entry.id.as_str(), index, *after))
        })
        .collect();
    let mut stays = vec![false; common.len()];
    for index in longest_increasing(
        &common
            .iter()
            .map(|(_, _, after)| *after)
            .collect::<Vec<_>>(),
    ) {
        stays[index] = true;
    }
    let mut moved: Vec<(String, usize, usize)> = common
        .iter()
        .zip(stays)
        .filter(|(_, stays)| !stays)
        .map(|((id, before, after), _)| (id.to_string(), before + 1, after + 1))
        .collect();
    moved.sort_by_key(|(_, _, after)| *after);
    moved
}

/// Indices of one longest strictly increasing subsequence.
fn longest_increasing(values: &[usize]) -> Vec<usize> {
    // tails[k]: index of the smallest last value of a run of length k + 1.
    let mut tails: Vec<usize> = Vec::new();
    let mut previous: Vec<Option<usize>> = vec![None; values.len()];
    for (index, value) in values.iter().enumerate() {
        let slot = tails.partition_point(|&tail| values[tail] < *value);
        if slot > 0 {
            previous[index] = Some(tails[slot - 1]);
        }
        if slot == tails.len() {
            tails.push(index);
        } else {
            tails[slot] = index;
        }
    }
    let mut run = Vec::with_capacity(tails.len());
    let mut cursor = tails.last().copied();
    while let Some(index) = cursor {
        run.push(index);
        cursor = previous[index];
    }
    run.reverse();
    run
}

/// Overrides-panel choices that differ: files whose chosen mod changes, or
/// that gain or lose a choice.
fn changed_overrides(before: &Profile, after: &Profile) -> usize {
    let choices = |profile: &Profile| -> HashMap<(u8, String), String> {
        profile
            .file_overrides
            .iter()
            .map(|entry| {
                (
                    (entry.kind as u8, entry.relative_path.clone()),
                    entry.mod_id.clone(),
                )
            })
            .collect()
    };
    let before = choices(before);
    let after = choices(after);
    let keys: HashSet<&(u8, String)> = before.keys().chain(after.keys()).collect();
    keys.into_iter()
        .filter(|key| before.get(*key) != after.get(*key))
        .count()
}

/// The library a restore produces: the backup's mod list, order, on/off,
/// target and override choices, pins and profiles, with today's info (names,
/// files, scripts) for mods that are still in the library. App settings and
/// caches stay as they are now.
pub fn restored_library(now: &Library, mut then: Library) -> Library {
    let current: HashMap<&str, _> = now
        .mods
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect();
    for entry in &mut then.mods {
        if let Some(existing) = current.get(entry.id.as_str()) {
            let target_overrides = std::mem::take(&mut entry.target_overrides);
            *entry = (*existing).clone();
            entry.target_overrides = target_overrides;
        }
    }
    then.dependency_blocks = now.dependency_blocks.clone();
    then.metadata_cache_version = now.metadata_cache_version;
    then.metadata_cache_key = now.metadata_cache_key.clone();
    then.modsettings_hash = now.modsettings_hash.clone();
    then.modsettings_sync_enabled = now.modsettings_sync_enabled;
    if !then
        .profiles
        .iter()
        .any(|profile| !is_sigillink_ranking_profile(&profile.name))
    {
        then.profiles.push(Profile::new("Default"));
    }
    then.active_profile = restored_active_profile(&then);
    then.ensure_mods_in_profiles();
    then
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{
        FileOverride, ModEntry, ModScripts, ModSource, ProfileEntry, TargetKind, TargetOverride,
    };

    fn mod_entry(id: &str) -> ModEntry {
        ModEntry {
            id: id.to_string(),
            name: format!("{id} mod"),
            created_at: None,
            modified_at: None,
            added_at: 0,
            targets: Vec::new(),
            target_overrides: Vec::new(),
            source_label: None,
            source: ModSource::Managed,
            dependencies: Vec::new(),
            scripts: ModScripts::default(),
        }
    }

    /// A library with one profile per `(name, order)`; an order item starting
    /// with '-' is a disabled mod. The first profile is active.
    fn library(profiles: &[(&str, &[&str])]) -> Library {
        let mut mods: Vec<ModEntry> = Vec::new();
        let profiles = profiles
            .iter()
            .map(|(name, order)| {
                let mut profile = Profile::new(name);
                for item in order.iter() {
                    let id = item.trim_start_matches('-');
                    if !mods.iter().any(|entry| entry.id == id) {
                        mods.push(mod_entry(id));
                    }
                    profile.order.push(ProfileEntry {
                        id: id.to_string(),
                        enabled: !item.starts_with('-'),
                        missing_label: None,
                    });
                }
                profile
            })
            .collect::<Vec<_>>();
        Library {
            mods,
            active_profile: profiles[0].name.clone(),
            profiles,
            dependency_blocks: HashSet::new(),
            metadata_cache_version: 0,
            metadata_cache_key: None,
            modsettings_hash: None,
            modsettings_sync_enabled: true,
        }
    }

    fn file_override(path: &str, mod_id: &str) -> FileOverride {
        FileOverride {
            kind: TargetKind::Data,
            relative_path: path.to_string(),
            mod_id: mod_id.to_string(),
        }
    }

    #[test]
    fn only_mods_that_really_moved_count() {
        let now = library(&[("Default", &["a", "b", "c", "d", "e"])]);
        // d moved up two places; b and c only shifted down because of it.
        let then = library(&[("Default", &["a", "d", "b", "c", "e"])]);
        let changes = restore_changes(&now, &then);
        assert_eq!(changes.moved, vec![("d".to_string(), 4, 2)]);
        assert_eq!(
            changes.short_parts(),
            [(ChangeKind::Moved, "1 moved".to_string())]
        );

        let reversed = library(&[("Default", &["e", "d", "c", "b", "a"])]);
        assert_eq!(restore_changes(&now, &reversed).moved.len(), 4);
        assert!(restore_changes(&now, &now.clone()).is_empty());
    }

    #[test]
    fn lists_every_kind_of_change() {
        let mut now = library(&[
            ("Default", &["a", "b", "-c", "new"]),
            ("Coop", &["a", "b", "c", "new"]),
            ("Made since", &["a"]),
        ]);
        now.profiles[0].file_overrides = vec![file_override("x.dds", "a")];
        now.mods[1].target_overrides = vec![TargetOverride {
            kind: TargetKind::Data,
            enabled: true,
        }];
        let mut then = library(&[
            ("Default", &["a", "-b", "c", "gone"]),
            ("Coop", &["b", "a", "c"]),
            ("Old", &["a"]),
        ]);
        then.profiles[0].file_overrides =
            vec![file_override("x.dds", "c"), file_override("y.dds", "a")];

        let changes = restore_changes(&now, &then);
        assert_eq!(changes.profile, "Default");
        assert!(!changes.switches_profile);
        assert_eq!(changes.turned_on, vec!["c".to_string()]);
        assert_eq!(changes.turned_off, vec!["b".to_string()]);
        assert_eq!(changes.returning, vec!["gone".to_string()]);
        assert_eq!(changes.leaving, vec!["new".to_string()]);
        assert_eq!(changes.targets, vec!["b".to_string()]);
        assert_eq!(changes.overrides, 2);
        assert_eq!(changes.profiles_returning, vec!["Old".to_string()]);
        assert_eq!(changes.profiles_leaving, vec!["Made since".to_string()]);
        assert_eq!(changes.other_profiles, vec!["Coop".to_string()]);
        let words: Vec<String> = changes
            .short_parts()
            .into_iter()
            .map(|(_, text)| text)
            .collect();
        assert_eq!(
            words,
            [
                "1 on",
                "1 off",
                "1 re-added",
                "1 removed",
                "1 target",
                "2 overrides",
                "3 profiles"
            ]
        );

        then.active_profile = "Coop".to_string();
        let changes = restore_changes(&now, &then);
        assert!(changes.switches_profile);
        assert_eq!(
            changes.short_parts()[0],
            (ChangeKind::Profile, "switch to Coop".to_string())
        );
    }

    #[test]
    fn restoring_keeps_todays_mod_info() {
        let mut now = library(&[("Default", &["a", "b"])]);
        now.mods[0].name = "Renamed since".to_string();
        now.mods[0].scripts.osiris = true;
        now.metadata_cache_version = 7;
        let mut then = library(&[("Default", &["b", "-a", "old"])]);
        then.mods[1].target_overrides = vec![TargetOverride {
            kind: TargetKind::Data,
            enabled: false,
        }];
        then.mods[0].name = "Old name".to_string();

        let restored = restored_library(&now, then);
        let ids: Vec<&str> = restored.profiles[0]
            .order
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(ids, ["b", "a", "old"]);
        assert!(!restored.profiles[0].order[1].enabled);
        let a = restored.mods.iter().find(|entry| entry.id == "a").unwrap();
        assert_eq!(a.name, "Renamed since");
        assert!(a.scripts.osiris);
        assert_eq!(a.target_overrides.len(), 1);
        assert!(restored.mods.iter().any(|entry| entry.id == "old"));
        assert_eq!(restored.metadata_cache_version, 7);
    }

    #[test]
    fn restored_profile_is_never_the_hidden_ranking_one() {
        let now = library(&[("Default", &["a"])]);
        let mut then = library(&[(crate::library::SIGILLINK_RANKING_PROFILE, &["a"])]);
        then.active_profile = "Missing".to_string();
        let restored = restored_library(&now, then);
        assert_eq!(restored.active_profile, "Default");
        assert!(restored
            .profiles
            .iter()
            .any(|profile| profile.name == "Default"));
    }

    #[test]
    fn reasons_read_as_labels() {
        assert_eq!(reason_label(Some("auto: order changed")), "Order changed");
        assert_eq!(reason_label(Some("manual deploy")), "Manual deploy");
        assert_eq!(
            reason_label(Some("auto: sigillink ranking")),
            "SigiLink ranking"
        );
        assert_eq!(reason_label(Some(BEFORE_RESTORE)), "Before restore");
        assert_eq!(reason_label(None), "Unknown");
    }

    #[test]
    fn lists_backups_newest_first_and_never_reuses_a_folder() {
        let root = std::env::temp_dir().join(format!("sigilsmith-list-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let config = GameConfig {
            game_id: Default::default(),
            game_name: "Baldur's Gate 3".to_string(),
            data_dir: root.clone(),
            sigillink_cache_dir: None,
            game_root: root.join("game"),
            larian_dir: root.join("larian"),
            active_profile: "Default".to_string(),
        };
        let first =
            create_backup(&config, &library(&[("Default", &["a"])]), None, Some("one")).unwrap();
        let second = create_backup(&config, &library(&[("Default", &["a"])]), None, None).unwrap();
        fs::create_dir_all(root.join("backups").join("backup-5")).unwrap();

        let listed = list_backups(&root);
        let _ = fs::remove_dir_all(&root);
        assert_ne!(first, second);
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].dir, second);
        assert_eq!(listed[1].dir, first);
        assert_eq!(listed[1].reason.as_deref(), Some("one"));
        assert_eq!(listed[2].timestamp, 5);
        assert_eq!(listed[2].reason, None);
    }

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
