//! One-time fixes for libraries saved by older SigilSmith versions.

use crate::{
    importer,
    library::{InstallTarget, Library, ModSource},
    sigillink,
};
use std::{collections::HashMap, fs, path::Path};

/// Bumped when a new one-time repair is added.
pub const REPAIR_VERSION: u32 = 1;

/// Runs the repairs this library hasn't had yet. The caller saves the library.
pub fn run_pending_repairs(library: &mut Library, cache_root: &Path) -> Vec<IdRepair> {
    if library.repair_version >= REPAIR_VERSION {
        return Vec::new();
    }
    let repairs = repair_pak_ids(library, cache_root);
    library.repair_version = REPAIR_VERSION;
    repairs
}

/// Follows ID changes from `renamed` in a library restored from an older backup.
pub fn apply_renamed_ids(library: &mut Library, renamed: &HashMap<String, String>) {
    for (old_id, new_id) in renamed {
        if !library.mods.iter().any(|entry| &entry.id == old_id) {
            continue;
        }
        let duplicate = library.mods.iter().position(|entry| &entry.id == new_id);
        if duplicate.is_some_and(|position| library.mods[position].source != ModSource::Native) {
            continue;
        }
        if let Some(position) = duplicate {
            library.mods.remove(position);
        }
        if let Some(mod_entry) = library.mods.iter_mut().find(|entry| &entry.id == old_id) {
            mod_entry.id = new_id.clone();
        }
        rekey_references(library, old_id, new_id, duplicate.is_some());
    }
}

/// A managed mod whose stored UUID didn't match its pak.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdRepair {
    pub name: String,
    pub old_id: String,
    pub new_id: String,
    /// The in-game mod manager entry the wrong ID had made, now merged into this mod.
    pub merged_duplicate: bool,
    /// Set when the mod kept its old ID; its load-order entry still uses the right UUID.
    pub kept_id: Option<String>,
}

/// Before 0.9.11, SigilSmith could read a pak's UUID from a Script node nested in meta.lsx's
/// ModuleInfo (KaiLime UI is one), so BG3 dropped the mod from the load order and the next
/// sync could add the deployed file again as a second, "in-game" copy. This re-reads every
/// managed pak, fixes the stored module info, moves the mod to its real UUID and merges that
/// duplicate.
pub fn repair_pak_ids(library: &mut Library, cache_root: &Path) -> Vec<IdRepair> {
    let mods_root = cache_root.join("mods");
    let mut repairs = Vec::new();
    let managed: Vec<String> = library
        .mods
        .iter()
        .filter(|entry| entry.source == ModSource::Managed)
        .map(|entry| entry.id.clone())
        .collect();
    for id in managed {
        // Merging a duplicate removes an entry, so look each mod up again.
        let Some(index) = library.mods.iter().position(|entry| entry.id == id) else {
            continue;
        };
        let mod_entry = &library.mods[index];
        let paks = mod_entry
            .targets
            .iter()
            .filter(|target| matches!(target, InstallTarget::Pak { .. }))
            .count();
        let mut fixed: Option<(String, String)> = None;
        let mod_root = mods_root.join(&mod_entry.id);
        let mut targets = mod_entry.targets.clone();
        for target in &mut targets {
            let InstallTarget::Pak { file, info } = target else {
                continue;
            };
            let (Some(read), _) = importer::read_pak_info(&mod_root.join(&*file)) else {
                continue;
            };
            if read.uuid == info.uuid {
                continue;
            }
            fixed = Some((info.uuid.clone(), read.uuid.clone()));
            info.uuid = read.uuid;
            info.module_type = read.module_type;
        }
        let Some((old_uuid, new_uuid)) = fixed else {
            continue;
        };
        let name = mod_entry.display_name();
        let old_id = mod_entry.id.clone();
        library.mods[index].targets = targets;

        let mut repair = IdRepair {
            name,
            old_id: old_id.clone(),
            new_id: new_uuid.clone(),
            merged_duplicate: false,
            kept_id: None,
        };
        // Only a single-pak mod is keyed by its pak's UUID.
        if paks != 1 || old_id != old_uuid {
            repair.kept_id = Some(old_id);
            repairs.push(repair);
            continue;
        }
        let duplicate = library.mods.iter().position(|entry| entry.id == new_uuid);
        let blocked = match duplicate {
            Some(position) => library.mods[position].source != ModSource::Native,
            None => false,
        };
        let new_root = mods_root.join(&new_uuid);
        if blocked || fs::symlink_metadata(&new_root).is_ok() {
            repair.kept_id = Some(old_id);
            repairs.push(repair);
            continue;
        }
        if fs::symlink_metadata(&mod_root).is_ok() && fs::rename(&mod_root, &new_root).is_err() {
            repair.kept_id = Some(old_id);
            repairs.push(repair);
            continue;
        }
        let old_index = sigillink::sigillink_index_path(cache_root, &old_id);
        if old_index.exists() {
            let _ = fs::rename(
                &old_index,
                sigillink::sigillink_index_path(cache_root, &new_uuid),
            );
        }

        if let Some(position) = duplicate {
            library.mods.remove(position);
            repair.merged_duplicate = true;
        }
        if let Some(mod_entry) = library.mods.iter_mut().find(|entry| entry.id == old_id) {
            mod_entry.id = new_uuid.clone();
        }
        rekey_references(library, &old_id, &new_uuid, repair.merged_duplicate);
        library.renamed_ids.insert(old_id, new_uuid);
        repairs.push(repair);
    }
    repairs
}

/// Points profiles and blocks at `new_id`. When a duplicate under `new_id` was merged, the mod
/// is enabled if either entry was, and keeps the enabled entry's place (the managed one's when
/// both were): an older SigilSmith moved the player's choice onto the duplicate.
fn rekey_references(library: &mut Library, old_id: &str, new_id: &str, merged: bool) {
    for profile in &mut library.profiles {
        let own = profile.order.iter().position(|entry| entry.id == old_id);
        let duplicate = profile.order.iter().position(|entry| entry.id == new_id);
        if let (true, Some(own), Some(duplicate)) = (merged, own, duplicate) {
            let own_enabled = profile.order[own].enabled;
            let duplicate_enabled = profile.order[duplicate].enabled;
            let drop = if duplicate_enabled && !own_enabled {
                profile.order[duplicate].id = old_id.to_string();
                own
            } else {
                duplicate
            };
            profile.order[if drop == own { duplicate } else { own }].missing_label = None;
            profile.order.remove(drop);
        }
        for entry in &mut profile.order {
            if entry.id == old_id {
                entry.id = new_id.to_string();
            }
        }
        // Keep the first entry if a profile somehow lists the mod twice.
        let mut seen = false;
        profile.order.retain(|entry| {
            if entry.id != new_id {
                return true;
            }
            !std::mem::replace(&mut seen, true)
        });
        if merged {
            profile
                .file_overrides
                .retain(|override_entry| override_entry.mod_id != new_id);
        }
        for override_entry in &mut profile.file_overrides {
            if override_entry.mod_id == old_id {
                override_entry.mod_id = new_id.to_string();
            }
        }
        if let Some(pin) = profile.sigillink_pins.remove(old_id) {
            profile.sigillink_pins.insert(new_id.to_string(), pin);
        }
    }
    if library.dependency_blocks.remove(old_id) {
        library.dependency_blocks.insert(new_id.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        library::{ModEntry, ModScripts, PakInfo, Profile, ProfileEntry},
        metadata::{write_test_pak_version, TEST_NESTED_META},
    };
    use std::{collections::HashSet, path::PathBuf};

    const WRONG: &str = "0d6510f5-50a3-4ecd-83d8-134c9a640324";
    const RIGHT: &str = "baf029fa-af6e-4080-bd6a-abacdea9e684";

    fn pak_entry(id: &str, uuid: &str, source: ModSource) -> ModEntry {
        ModEntry {
            id: id.to_string(),
            name: "KaiLime UI".to_string(),
            created_at: None,
            modified_at: None,
            added_at: 0,
            targets: vec![InstallTarget::Pak {
                file: "KaiLimeUI.pak".to_string(),
                info: PakInfo {
                    uuid: uuid.to_string(),
                    name: "KaiLime UI".to_string(),
                    folder: "KaiLimeUI".to_string(),
                    version: 1,
                    md5: None,
                    publish_handle: None,
                    author: None,
                    description: None,
                    module_type: Some("1".to_string()),
                },
            }],
            target_overrides: Vec::new(),
            source_label: None,
            source,
            dependencies: Vec::new(),
            scripts: ModScripts::default(),
        }
    }

    fn setup(name: &str) -> (PathBuf, Library) {
        let root =
            std::env::temp_dir().join(format!("sigilsmith-repair-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let store = root.join("mods").join(WRONG);
        fs::create_dir_all(&store).unwrap();
        write_test_pak_version(
            &store.join("KaiLimeUI.pak"),
            16,
            &[("Mods/KaiLimeUI/meta.lsx", TEST_NESTED_META.as_bytes(), true)],
        );
        let mut profile = Profile::new("Default");
        profile.order = vec![
            ProfileEntry {
                id: "other".to_string(),
                enabled: true,
                missing_label: None,
            },
            ProfileEntry {
                id: WRONG.to_string(),
                enabled: true,
                missing_label: None,
            },
        ];
        profile.sigillink_pins.insert(WRONG.to_string(), 1);
        let library = Library {
            mods: vec![pak_entry(WRONG, WRONG, ModSource::Managed)],
            profiles: vec![profile],
            active_profile: "Default".to_string(),
            dependency_blocks: HashSet::from([WRONG.to_string()]),
            metadata_cache_version: 3,
            metadata_cache_key: None,
            modsettings_hash: None,
            modsettings_sync_enabled: true,
            repair_version: 0,
            renamed_ids: Default::default(),
        };
        (root, library)
    }

    #[test]
    fn moves_a_mod_to_its_real_uuid() {
        let (root, mut library) = setup("rekey");
        let repairs = repair_pak_ids(&mut library, &root);
        let store_moved = root.join("mods").join(RIGHT).join("KaiLimeUI.pak").exists();
        let old_left = root.join("mods").join(WRONG).exists();
        let _ = fs::remove_dir_all(&root);

        assert_eq!(repairs.len(), 1);
        assert_eq!(repairs[0].new_id, RIGHT);
        assert!(repairs[0].kept_id.is_none());
        assert!(store_moved && !old_left);
        let mod_entry = &library.mods[0];
        assert_eq!(mod_entry.id, RIGHT);
        let InstallTarget::Pak { info, .. } = &mod_entry.targets[0] else {
            panic!("not a pak");
        };
        assert_eq!(info.uuid, RIGHT);
        assert_eq!(info.module_type, None);
        let profile = &library.profiles[0];
        assert_eq!(profile.order[1].id, RIGHT);
        assert!(profile.order[1].enabled);
        assert_eq!(profile.sigillink_pins.get(RIGHT), Some(&1));
        assert!(library.dependency_blocks.contains(RIGHT));

        // A second run finds nothing to fix.
        assert!(repair_pak_ids(&mut library, &root).is_empty());
    }

    #[test]
    fn merges_the_duplicate_the_wrong_id_created() {
        let (root, mut library) = setup("merge");
        library
            .mods
            .push(pak_entry(RIGHT, RIGHT, ModSource::Native));
        library.profiles[0].order.insert(
            0,
            ProfileEntry {
                id: RIGHT.to_string(),
                enabled: false,
                missing_label: None,
            },
        );
        let repairs = repair_pak_ids(&mut library, &root);
        let _ = fs::remove_dir_all(&root);

        assert!(repairs[0].merged_duplicate);
        assert_eq!(library.mods.len(), 1);
        assert_eq!(library.mods[0].source, ModSource::Managed);
        assert_eq!(order(&library), [("other", true), (RIGHT, true)]);
    }

    #[test]
    fn a_merge_keeps_the_mod_enabled_where_the_duplicate_was() {
        // 0.9.10's modsettings sync enabled the duplicate and turned the real entry off.
        let (root, mut library) = setup("merge-enabled");
        library
            .mods
            .push(pak_entry(RIGHT, RIGHT, ModSource::Native));
        library.profiles[0].order[1].enabled = false;
        library.profiles[0].order.insert(
            0,
            ProfileEntry {
                id: RIGHT.to_string(),
                enabled: true,
                missing_label: None,
            },
        );
        repair_pak_ids(&mut library, &root);
        let _ = fs::remove_dir_all(&root);

        assert_eq!(order(&library), [(RIGHT, true), ("other", true)]);
    }

    fn order(library: &Library) -> Vec<(&str, bool)> {
        library.profiles[0]
            .order
            .iter()
            .map(|entry| (entry.id.as_str(), entry.enabled))
            .collect()
    }

    #[test]
    fn keeps_the_id_when_another_managed_mod_has_the_uuid() {
        let (root, mut library) = setup("blocked");
        let mut other = pak_entry(RIGHT, RIGHT, ModSource::Managed);
        other.name = "Another import".to_string();
        library.mods.push(other);
        let repairs = repair_pak_ids(&mut library, &root);
        let old_store = root.join("mods").join(WRONG).exists();
        let _ = fs::remove_dir_all(&root);

        assert_eq!(repairs[0].kept_id.as_deref(), Some(WRONG));
        assert!(old_store);
        assert_eq!(library.mods[0].id, WRONG);
        // Its load-order entry is still written with the right UUID.
        let InstallTarget::Pak { info, .. } = &library.mods[0].targets[0] else {
            panic!("not a pak");
        };
        assert_eq!(info.uuid, RIGHT);
    }
}
