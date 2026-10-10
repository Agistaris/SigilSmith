//! Moving a setup between Larian data folders, as when Steam switches BG3 between the native
//! build and Proton. SigilSmith keeps one copy of each mod it manages in its store and only
//! links it into the active Mods folder: a move takes those links out of the old folder and
//! makes them in the new one. Mods from the in-game mod manager stay where the game put them;
//! the new folder's own copies are matched by UUID and keep their place and on/off state.
//! In-game mods turned on only in the new folder join the library, still on, so the move
//! doesn't drop them from that folder's load order.

use crate::{
    backup, bg3,
    config::GameConfig,
    deploy::{self, DeployOptions, DeployReport},
    library::{InstallTarget, Library, ModEntry, ModScripts, ModSource, PakInfo},
    native_pak,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const JOURNAL_FILE: &str = "move-journal.json";

/// What a move would do, for the confirmation dialog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MovePlan {
    pub from: PathBuf,
    pub to: PathBuf,
    /// Paks from SigilSmith's store that get linked into the new folder.
    pub managed: usize,
    /// In-game mods the new folder already has.
    pub native_found: usize,
    /// In-game mods the new folder doesn't have yet. They stay in the library, left out of
    /// the load order until the game downloads them there.
    pub native_missing: Vec<String>,
    /// In-game mods turned on only in the new folder; the library takes them in.
    pub new_folder_only: Vec<String>,
    /// Save folders only the old folder has.
    pub saves_only_in_from: Vec<String>,
}

/// Marks a move in progress, so one cut short is offered again at the next start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveJournal {
    pub from: PathBuf,
    pub to: PathBuf,
    pub started_at: u64,
    /// The backup taken before anything changed.
    #[serde(default)]
    pub backup: Option<PathBuf>,
}

/// What going back from an unfinished move undid.
pub struct AbandonReport {
    pub removed_links: usize,
    pub restored_modsettings: bool,
}

pub struct MoveReport {
    pub deploy: DeployReport,
    pub backup: PathBuf,
    /// In-game mods from the new folder that joined the library, turned on.
    pub adopted: Vec<ModEntry>,
}

pub fn plan_move(config: &GameConfig, library: &Library, to: &Path) -> MovePlan {
    let mut plan = MovePlan {
        from: config.larian_dir.clone(),
        to: to.to_path_buf(),
        ..MovePlan::default()
    };
    let index = native_pak::build_native_pak_index_cached(&to.join("Mods"));
    let enabled: HashSet<&str> = library
        .active_profile()
        .map(|profile| {
            profile
                .order
                .iter()
                .filter(|entry| entry.enabled)
                .map(|entry| entry.id.as_str())
                .collect()
        })
        .unwrap_or_default();
    for mod_entry in &library.mods {
        let paks: Vec<_> = mod_entry
            .targets
            .iter()
            .filter_map(|target| match target {
                InstallTarget::Pak { info, .. } => Some(info),
                _ => None,
            })
            .collect();
        if paks.is_empty() {
            continue;
        }
        if mod_entry.source == ModSource::Managed {
            plan.managed += paks.len();
            continue;
        }
        let found = paks
            .iter()
            .all(|info| native_pak::resolve_native_pak_path(info, &index).is_some());
        if found {
            plan.native_found += 1;
        } else if enabled.contains(mod_entry.id.as_str()) {
            plan.native_missing.push(mod_entry.display_name());
        }
    }
    plan.new_folder_only = new_folder_mods(config, library, to)
        .into_iter()
        .map(|(_, info)| info.name)
        .collect();
    plan.saves_only_in_from = saves_only_in(&config.larian_dir, to);
    plan
}

/// Mods the new folder's load order has turned on that the library doesn't know, with their
/// files in that Mods folder. SigilSmith's own links don't count.
fn new_folder_mods(config: &GameConfig, library: &Library, to: &Path) -> Vec<(String, PakInfo)> {
    let Ok(snapshot) =
        deploy::read_modsettings_snapshot(&to.join("PlayerProfiles/Public/modsettings.lsx"))
    else {
        return Vec::new();
    };
    let enabled: HashSet<&String> = if snapshot.enabled.is_empty() {
        snapshot.order.iter().collect()
    } else {
        snapshot.enabled.iter().collect()
    };
    let known: HashSet<&str> = library.mods.iter().map(|entry| entry.id.as_str()).collect();
    let mods_dir = to.join("Mods");
    let index = native_pak::build_native_pak_index_cached(&mods_dir);
    let deployed = deploy::DeployedFiles::load(&config.data_dir, &config.sigillink_cache_root());
    let mut found = Vec::new();
    for module in snapshot.modules {
        let info = module.info;
        if !enabled.contains(&info.uuid) || known.contains(info.uuid.as_str()) {
            continue;
        }
        let Some(path) = native_pak::resolve_native_pak_path(&info, &index) else {
            continue;
        };
        if deployed.is_ours(&path) {
            continue;
        }
        let Some(file) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        found.push((file.to_string(), info));
    }
    found
}

/// Adds in-game mods to the library, turned on in the active profile. Mods it already has are
/// left as they are.
pub fn adopt_mods(library: &mut Library, adopted: &[ModEntry]) {
    let mut added = Vec::new();
    for entry in adopted {
        if library.mods.iter().any(|existing| existing.id == entry.id) {
            continue;
        }
        library.mods.push(entry.clone());
        added.push(entry.id.as_str());
    }
    if added.is_empty() {
        return;
    }
    library.ensure_mods_in_profiles();
    if let Some(profile) = library.active_profile_mut() {
        for item in &mut profile.order {
            if added.contains(&item.id.as_str()) {
                item.enabled = true;
            }
        }
    }
}

/// Whether there's a setup to move: mods in the library, or files from an earlier deploy.
/// Without one, a new Larian folder is only a setting.
pub fn has_setup(config: &GameConfig, library: &Library) -> bool {
    !library.mods.is_empty() || config.data_dir.join("deploy_manifest.json").exists()
}

pub fn pending_move(data_dir: &Path) -> Option<MoveJournal> {
    let raw = fs::read_to_string(data_dir.join(JOURNAL_FILE)).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_journal(data_dir: &Path, journal: &MoveJournal) -> Result<()> {
    let raw = serde_json::to_string_pretty(journal).context("serialize move journal")?;
    let path = data_dir.join(JOURNAL_FILE);
    let tmp = data_dir.join(format!("{JOURNAL_FILE}.tmp"));
    fs::write(&tmp, raw).context("write move journal")?;
    fs::rename(&tmp, &path).context("write move journal")?;
    Ok(())
}

pub fn clear_journal(data_dir: &Path) {
    let _ = fs::remove_file(data_dir.join(JOURNAL_FILE));
}

/// Undoes what an unfinished move did to its new folder: links to the store that no deploy
/// recorded come out, and the folder's own load order comes back from the backup taken before
/// the move (the one there now is kept in that backup first). Links the move did record leave
/// with the next deploy, which goes to the folder SigilSmith is still set to.
pub fn abandon_move(config: &GameConfig, journal: &MoveJournal) -> Result<AbandonReport> {
    if bg3::game_running() {
        bail!("close Baldur's Gate 3 first");
    }
    abandon_move_unchecked(config, journal)
}

fn abandon_move_unchecked(config: &GameConfig, journal: &MoveJournal) -> Result<AbandonReport> {
    let deployed = deploy::DeployedFiles::load(&config.data_dir, &config.sigillink_cache_root());
    let strays = deployed.stray_links_in(&journal.to.join("Mods"));
    let removed_links = deployed.remove_links(&strays);

    let mut restored_modsettings = false;
    let saved = journal
        .backup
        .as_ref()
        .map(|dir| dir.join("modsettings-new-folder.lsx"))
        .filter(|path| path.exists());
    if let Some(saved) = saved {
        let current = journal.to.join("PlayerProfiles/Public/modsettings.lsx");
        if fs::read(&current).ok() != fs::read(&saved).ok() {
            if current.exists() {
                fs::copy(
                    &current,
                    saved.with_file_name("modsettings-new-folder-before-undo.lsx"),
                )
                .context("keep the new folder's load order")?;
            }
            fs::copy(&saved, &current).context("restore the new folder's load order")?;
            restored_modsettings = true;
        }
    }
    clear_journal(&config.data_dir);
    Ok(AbandonReport {
        removed_links,
        restored_modsettings,
    })
}

/// Moves the setup to the Larian folder `to`: backs up (library, deploy record, both load
/// orders, config), then deploys there. The deploy removes SigilSmith's links from every
/// folder an earlier deploy recorded, so the old Mods folder keeps only the game's own files.
/// Steam's settings and other launchers' files are never touched.
pub fn move_setup(config: &mut GameConfig, library: &mut Library, to: &Path) -> Result<MoveReport> {
    if bg3::game_running() {
        bail!("close Baldur's Gate 3 first");
    }
    move_setup_unchecked(config, library, to)
}

fn move_setup_unchecked(
    config: &mut GameConfig,
    library: &mut Library,
    to: &Path,
) -> Result<MoveReport> {
    if !bg3::looks_like_larian_dir(to) {
        bail!(
            "{} isn't a Larian data folder yet: start BG3 there once first",
            to.display()
        );
    }
    let from = config.larian_dir.clone();
    let reason = format!("before moving to {}", to.display());
    let backup_dir = backup::create_backup(
        config,
        library,
        Some(&from.join("PlayerProfiles/Public/modsettings.lsx")),
        Some(&reason),
    )?;
    let to_modsettings = to.join("PlayerProfiles/Public/modsettings.lsx");
    if to_modsettings.exists() {
        let _ = fs::copy(
            &to_modsettings,
            backup_dir.join("modsettings-new-folder.lsx"),
        );
    }
    write_journal(
        &config.data_dir,
        &MoveJournal {
            from: from.clone(),
            to: to.to_path_buf(),
            started_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            backup: Some(backup_dir.clone()),
        },
    )?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let adopted: Vec<ModEntry> = new_folder_mods(config, library, to)
        .into_iter()
        .map(|(file, info)| ModEntry {
            id: info.uuid.clone(),
            name: info.name.clone(),
            created_at: None,
            modified_at: None,
            added_at: now,
            targets: vec![InstallTarget::Pak { file, info }],
            target_overrides: Vec::new(),
            source_label: None,
            source: ModSource::Native,
            dependencies: Vec::new(),
            scripts: ModScripts::default(),
        })
        .collect();
    adopt_mods(library, &adopted);

    let mut moved = config.clone();
    moved.larian_dir = to.to_path_buf();
    let report = deploy::deploy_with_options(
        &moved,
        library,
        DeployOptions {
            backup: false,
            reason: Some(format!("move to {}", to.display())),
        },
    )?;
    config.larian_dir = to.to_path_buf();
    config.save()?;
    clear_journal(&config.data_dir);
    Ok(MoveReport {
        deploy: report,
        backup: backup_dir,
        adopted,
    })
}

fn save_dir(larian_dir: &Path) -> PathBuf {
    larian_dir.join("PlayerProfiles/Public/Savegames/Story")
}

fn save_names(larian_dir: &Path) -> HashSet<String> {
    fs::read_dir(save_dir(larian_dir))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Save folders in `from` that `to` doesn't have, sorted.
pub fn saves_only_in(from: &Path, to: &Path) -> Vec<String> {
    let there = save_names(to);
    let mut only: Vec<String> = save_names(from)
        .into_iter()
        .filter(|name| !there.contains(name))
        .collect();
    only.sort();
    only
}

/// Copies the save folders only `from` has into `to`. Never overwrites: a save `to` already
/// has is left as it is. Returns how many were copied.
pub fn copy_missing_saves(from: &Path, to: &Path) -> Result<usize> {
    let names = saves_only_in(from, to);
    let dest_root = save_dir(to);
    fs::create_dir_all(&dest_root).context("create the save folder")?;
    let mut copied = 0;
    for name in names {
        let source = save_dir(from).join(&name);
        let dest = dest_root.join(&name);
        // Copied beside the target and renamed, so a cut-short copy never looks like a save.
        let partial = dest_root.join(format!(".{name}.sigilsmith-partial"));
        let _ = fs::remove_dir_all(&partial);
        fs::create_dir_all(&partial).context("create a save copy")?;
        for entry in fs::read_dir(&source).with_context(|| format!("read {}", source.display()))? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                fs::copy(entry.path(), partial.join(entry.file_name()))
                    .with_context(|| format!("copy {}", entry.path().display()))?;
            }
        }
        if dest.exists() {
            let _ = fs::remove_dir_all(&partial);
            continue;
        }
        fs::rename(&partial, &dest).with_context(|| format!("copy save {name}"))?;
        copied += 1;
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Profile, ProfileEntry};
    use std::collections::HashMap;

    struct Fixture {
        root: PathBuf,
        config: GameConfig,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("sigilsmith-move-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            for dir in [
                "game/Data",
                "game/bin",
                "native/PlayerProfiles/Public",
                "native/Mods",
                "proton/PlayerProfiles/Public",
                "proton/Mods",
                "data",
            ] {
                fs::create_dir_all(root.join(dir)).unwrap();
            }
            let config = GameConfig {
                game_id: Default::default(),
                game_name: "Baldur's Gate 3".to_string(),
                data_dir: root.join("data"),
                sigillink_cache_dir: None,
                game_root: root.join("game"),
                larian_dir: root.join("native"),
                active_profile: "Default".to_string(),
                declined_move: None,
                keep_links_in: Vec::new(),
            };
            config.save().unwrap();
            Self { root, config }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn entry(id: &str, folder: &str, source: ModSource) -> ModEntry {
        ModEntry {
            id: id.to_string(),
            name: folder.to_string(),
            created_at: None,
            modified_at: None,
            added_at: 0,
            targets: vec![InstallTarget::Pak {
                file: format!("{folder}.pak"),
                info: PakInfo {
                    uuid: id.to_string(),
                    name: folder.to_string(),
                    folder: folder.to_string(),
                    version: 1,
                    md5: None,
                    publish_handle: None,
                    author: None,
                    description: None,
                    module_type: None,
                },
            }],
            target_overrides: Vec::new(),
            source_label: None,
            source,
            dependencies: Vec::new(),
            scripts: ModScripts::default(),
        }
    }

    const MANAGED: &str = "baf029fa-af6e-4080-bd6a-abacdea9e684";
    const GAME_MOD: &str = "6e84559a-1b7a-4441-bfc6-c2bae5538491";
    const NOT_DOWNLOADED: &str = "11111111-2222-3333-4444-555555555555";

    fn library(fixture: &Fixture) -> Library {
        let store = fixture.config.sigillink_mods_root().join(MANAGED);
        fs::create_dir_all(&store).unwrap();
        fs::write(store.join("KaiLimeUI.pak"), b"kai").unwrap();
        let mut profile = Profile::new("Default");
        for id in [GAME_MOD, MANAGED, NOT_DOWNLOADED] {
            profile.order.push(ProfileEntry {
                id: id.to_string(),
                enabled: true,
                missing_label: None,
            });
        }
        Library {
            mods: vec![
                entry(MANAGED, "KaiLimeUI", ModSource::Managed),
                entry(GAME_MOD, "Camera", ModSource::Native),
                entry(NOT_DOWNLOADED, "Later", ModSource::Native),
            ],
            profiles: vec![profile],
            active_profile: "Default".to_string(),
            dependency_blocks: HashSet::new(),
            metadata_cache_version: 0,
            metadata_cache_key: None,
            modsettings_hash: None,
            modsettings_sync_enabled: true,
            repair_version: 0,
            renamed_ids: HashMap::new(),
        }
    }

    fn uuids(larian_dir: &Path) -> Vec<String> {
        deploy::read_modsettings_snapshot(&larian_dir.join("PlayerProfiles/Public/modsettings.lsx"))
            .unwrap()
            .order
    }

    #[test]
    fn a_move_leaves_one_copy_and_keeps_the_load_order() {
        let mut fixture = Fixture::new("roundtrip");
        let mut library = library(&fixture);
        let native = fixture.root.join("native");
        let proton = fixture.root.join("proton");
        // Both folders have the game's own copy of the in-game mod.
        fs::write(native.join("Mods/Camera.pak"), b"camera").unwrap();
        fs::write(proton.join("Mods/Camera.pak"), b"camera").unwrap();
        deploy::deploy_with_options(
            &fixture.config,
            &mut library,
            DeployOptions {
                backup: false,
                reason: None,
            },
        )
        .unwrap();
        assert!(native.join("Mods/KaiLimeUI.pak").exists());

        let plan = plan_move(&fixture.config, &library, &proton);
        assert_eq!(plan.managed, 1);
        assert_eq!(plan.native_found, 1);
        assert_eq!(plan.native_missing, ["Later"]);

        let mut config = fixture.config.clone();
        let report = move_setup_unchecked(&mut config, &mut library, &proton).unwrap();
        assert_eq!(config.larian_dir, proton);
        assert!(!native.join("Mods/KaiLimeUI.pak").exists());
        assert!(proton.join("Mods/KaiLimeUI.pak").exists());
        // The game's own files stay where they are.
        assert_eq!(fs::read(native.join("Mods/Camera.pak")).unwrap(), b"camera");
        assert_eq!(uuids(&proton), [GAME_MOD, MANAGED]);
        assert_eq!(report.deploy.missing_mods, ["Later"]);
        assert!(report.backup.join("config.json").exists());
        assert!(pending_move(&config.data_dir).is_none());
        // The library still has the mod that isn't downloaded yet, enabled.
        let profile = library.active_profile().unwrap();
        assert!(profile
            .order
            .iter()
            .any(|entry| entry.id == NOT_DOWNLOADED && entry.enabled));

        // And back again.
        let report = move_setup_unchecked(&mut config, &mut library, &native).unwrap();
        assert!(native.join("Mods/KaiLimeUI.pak").exists());
        assert!(!proton.join("Mods/KaiLimeUI.pak").exists());
        assert_eq!(fs::read(proton.join("Mods/Camera.pak")).unwrap(), b"camera");
        assert_eq!(report.deploy.removed_count, 1);
        fixture.config = config;
    }

    #[test]
    fn a_move_keeps_mods_turned_on_only_in_the_new_folder() {
        const PROTON_ONLY: &str = "99999999-8888-7777-6666-555555555555";
        let fixture = Fixture::new("adopt");
        let mut library = library(&fixture);
        let proton = fixture.root.join("proton");
        fs::write(proton.join("Mods/Camera.pak"), b"camera").unwrap();
        fs::write(proton.join("Mods/ProtonOnly.pak"), b"proton only").unwrap();
        // The game turned ProtonOnly on in the Proton folder; the library has never seen it.
        let mut game_side = library.clone();
        game_side
            .mods
            .push(entry(PROTON_ONLY, "ProtonOnly", ModSource::Native));
        game_side.ensure_mods_in_profiles();
        for item in &mut game_side.active_profile_mut().unwrap().order {
            item.enabled = item.id == PROTON_ONLY || item.id == GAME_MOD;
        }
        let mut game_config = fixture.config.clone();
        game_config.larian_dir = proton.clone();
        game_config.data_dir = fixture.root.join("game-side");
        fs::create_dir_all(&game_config.data_dir).unwrap();
        deploy::deploy_with_options(
            &game_config,
            &mut game_side,
            DeployOptions {
                backup: false,
                reason: None,
            },
        )
        .unwrap();
        assert_eq!(uuids(&proton), [GAME_MOD, PROTON_ONLY]);

        let plan = plan_move(&fixture.config, &library, &proton);
        assert_eq!(plan.new_folder_only, ["ProtonOnly"]);
        let mut config = fixture.config.clone();
        let report = move_setup_unchecked(&mut config, &mut library, &proton).unwrap();
        assert_eq!(report.adopted.len(), 1);
        assert_eq!(uuids(&proton), [GAME_MOD, MANAGED, PROTON_ONLY]);
        let profile = library.active_profile().unwrap();
        assert!(profile
            .order
            .iter()
            .any(|item| item.id == PROTON_ONLY && item.enabled));
        // Adopting again changes nothing.
        let before = library.mods.len();
        adopt_mods(&mut library, &report.adopted);
        assert_eq!(library.mods.len(), before);
    }

    #[test]
    fn a_move_needs_a_real_larian_folder() {
        let fixture = Fixture::new("invalid");
        let mut library = library(&fixture);
        let mut config = fixture.config.clone();
        let missing = fixture.root.join("nowhere");
        assert!(move_setup_unchecked(&mut config, &mut library, &missing).is_err());
        assert_eq!(config.larian_dir, fixture.config.larian_dir);
        assert!(pending_move(&config.data_dir).is_none());
    }

    fn snapshot(root: &Path, skip: &[&Path], out: &mut Vec<(PathBuf, Vec<u8>)>) {
        let mut entries: Vec<_> = fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            if skip.iter().any(|skip| path.starts_with(skip)) {
                continue;
            }
            let meta = fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                out.push((path.clone(), Vec::new()));
                snapshot(&path, skip, out);
            } else {
                out.push((path.clone(), fs::read(&path).unwrap_or_default()));
            }
        }
    }

    /// Moving a setup, and the checks around it, only change the two Larian folders and
    /// SigilSmith's own data: Steam's settings, Proton's Wine registry and other launchers'
    /// settings and prefixes stay byte for byte the same.
    #[test]
    fn a_move_leaves_steam_and_other_launchers_untouched() {
        let home = std::env::temp_dir().join(format!("sigilsmith-guard-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let steam = home.join(".local/share/Steam");
        let game_root = steam.join("steamapps/common/Baldurs Gate 3");
        let prefix = steam.join("steamapps/compatdata/1086940/pfx");
        let native = home.join(".local/share/Larian Studios/Baldur's Gate 3");
        let proton =
            prefix.join("drive_c/users/steamuser/AppData/Local/Larian Studios/Baldur's Gate 3");
        let data_dir = home.join(".local/share/sigilsmith/bg3");
        for dir in [
            game_root.join("Data"),
            game_root.join("bin"),
            steam.join("config"),
            steam.join("userdata/12345/config"),
            native.join("PlayerProfiles/Public"),
            native.join("Mods"),
            proton.join("PlayerProfiles/Public"),
            proton.join("Mods"),
            data_dir.clone(),
        ] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(game_root.join("bin/bg3.exe"), b"exe").unwrap();
        fs::write(steam.join("config/config.vdf"), "\"InstallConfigStore\"\n{\n\t\"Software\"\n\t{\n\t\t\"Valve\"\n\t\t{\n\t\t\t\"Steam\"\n\t\t\t{\n\t\t\t\t\"CompatToolMapping\"\n\t\t\t\t{\n\t\t\t\t\t\"1086940\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"name\"\t\t\"proton_experimental\"\n\t\t\t\t\t}\n\t\t\t\t}\n\t\t\t}\n\t\t}\n\t}\n}\n").unwrap();
        fs::write(
            steam.join("userdata/12345/config/localconfig.vdf"),
            "\"UserLocalConfigStore\"\n{\n\t\"Software\"\n\t{\n\t\t\"Valve\"\n\t\t{\n\t\t\t\"Steam\"\n\t\t\t{\n\t\t\t\t\"apps\"\n\t\t\t\t{\n\t\t\t\t\t\"1086940\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"LaunchOptions\"\t\t\"WINEDLLOVERRIDES=\\\"DWrite.dll=n,b\\\" %command%\"\n\t\t\t\t\t}\n\t\t\t\t}\n\t\t\t}\n\t\t}\n\t}\n}\n",
        )
        .unwrap();
        fs::write(prefix.join("user.reg"), "WINE REGISTRY Version 2\n").unwrap();
        fs::write(prefix.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
        let launcher_dirs = crate::launchers::tests::write_launcher_configs(&home);

        let config = GameConfig {
            game_id: Default::default(),
            game_name: "Baldur's Gate 3".to_string(),
            data_dir: data_dir.clone(),
            sigillink_cache_dir: None,
            game_root: game_root.clone(),
            larian_dir: native.clone(),
            active_profile: "Default".to_string(),
            declined_move: None,
            keep_links_in: Vec::new(),
        };
        config.save().unwrap();
        let fixture_like = Fixture {
            root: home.join("unused"),
            config: config.clone(),
        };
        let mut library = library(&fixture_like);
        fs::write(native.join("Mods/Camera.pak"), b"camera").unwrap();
        fs::write(proton.join("Mods/Camera.pak"), b"camera").unwrap();
        deploy::deploy_with_options(
            &config,
            &mut library,
            DeployOptions {
                backup: false,
                reason: None,
            },
        )
        .unwrap();

        let skip = [native.as_path(), proton.as_path(), data_dir.as_path()];
        let mut before = Vec::new();
        snapshot(&home, &skip, &mut before);
        for guarded in [
            prefix.join("user.reg"),
            steam.join("userdata/12345/config/localconfig.vdf"),
            steam.join("config/config.vdf"),
            home.join(".config/heroic/GamesConfig/1456460669.json"),
            home.join(".config/faugus-launcher/games.json"),
            launcher_dirs[0].join("PlayerProfiles"),
        ] {
            assert!(
                before.iter().any(|(path, _)| *path == guarded),
                "{}",
                guarded.display()
            );
        }

        let suggestions = bg3::larian_dir_suggestions_in(&home, Some(&game_root));
        assert_eq!(
            suggestions.len(),
            2 + launcher_dirs.len(),
            "{suggestions:?}"
        );
        let _ = bg3::script_extender_setup_with(
            Some(&home),
            Some(&home.join(".local/share")),
            &game_root,
        );
        let mut config = config;
        plan_move(&config, &library, &proton);
        move_setup_unchecked(&mut config, &mut library, &proton).unwrap();
        let deployed =
            deploy::DeployedFiles::load(&config.data_dir, &config.sigillink_cache_root());
        assert_eq!(deployed.drift(&proton), deploy::LinkDrift::default());
        for (_, dir) in &suggestions {
            assert!(deployed.stray_links_in(&dir.join("Mods")).is_empty());
        }
        move_setup_unchecked(&mut config, &mut library, &native).unwrap();
        copy_missing_saves(&proton, &native).unwrap();

        let mut after = Vec::new();
        snapshot(&home, &skip, &mut after);
        assert_eq!(before.len(), after.len());
        for (old, new) in before.iter().zip(&after) {
            assert_eq!(old.0, new.0);
            assert!(old.1 == new.1, "{} changed", old.0.display());
        }
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn going_back_from_an_unfinished_move_restores_the_new_folder() {
        let fixture = Fixture::new("abandon");
        let mut library = library(&fixture);
        let proton = fixture.root.join("proton");
        fs::write(proton.join("Mods/Camera.pak"), b"camera").unwrap();
        // The Proton folder's own load order has only the game's mod on.
        let mut game_side = library.clone();
        for item in &mut game_side.active_profile_mut().unwrap().order {
            item.enabled = item.id == GAME_MOD;
        }
        let mut game_config = fixture.config.clone();
        game_config.larian_dir = proton.clone();
        game_config.data_dir = fixture.root.join("game-side");
        fs::create_dir_all(&game_config.data_dir).unwrap();
        let options = || DeployOptions {
            backup: false,
            reason: None,
        };
        deploy::deploy_with_options(&game_config, &mut game_side, options()).unwrap();
        let modsettings = proton.join("PlayerProfiles/Public/modsettings.lsx");
        let original = fs::read(&modsettings).unwrap();

        // The move deployed, then stopped before switching folders; a link it made before
        // anything recorded it is still there.
        let mut moved = fixture.config.clone();
        let report = move_setup_unchecked(&mut moved, &mut library.clone(), &proton).unwrap();
        assert_ne!(fs::read(&modsettings).unwrap(), original);
        fixture.config.save().unwrap();
        let journal = MoveJournal {
            from: fixture.config.larian_dir.clone(),
            to: proton.clone(),
            started_at: 0,
            backup: Some(report.backup.clone()),
        };
        write_journal(&fixture.config.data_dir, &journal).unwrap();
        let store_pak = fixture
            .config
            .sigillink_mods_root()
            .join(MANAGED)
            .join("KaiLimeUI.pak");
        fs::hard_link(&store_pak, proton.join("Mods/Stray.pak")).unwrap();

        let undone = abandon_move_unchecked(&fixture.config, &journal).unwrap();
        assert_eq!(undone.removed_links, 1);
        assert!(undone.restored_modsettings);
        assert_eq!(fs::read(&modsettings).unwrap(), original);
        assert!(report
            .backup
            .join("modsettings-new-folder-before-undo.lsx")
            .exists());
        assert!(pending_move(&fixture.config.data_dir).is_none());

        // The next deploy goes to the native folder and takes the recorded link out of Proton.
        deploy::deploy_with_options(&fixture.config, &mut library, options()).unwrap();
        assert!(fixture.root.join("native/Mods/KaiLimeUI.pak").exists());
        assert!(!proton.join("Mods/KaiLimeUI.pak").exists());
        assert!(!proton.join("Mods/Stray.pak").exists());
        assert_eq!(fs::read(proton.join("Mods/Camera.pak")).unwrap(), b"camera");
        assert_eq!(fs::read(&modsettings).unwrap(), original);
    }

    #[test]
    fn missing_saves_are_copied_and_nothing_is_overwritten() {
        let fixture = Fixture::new("saves");
        let native = fixture.root.join("native");
        let proton = fixture.root.join("proton");
        for (dir, name, body) in [
            (&native, "Tav-1__QuickSave_1", "native one"),
            (&native, "Shared__AutoSave_2", "native copy"),
            (&proton, "Shared__AutoSave_2", "proton copy"),
            (&proton, "Proton-only__QuickSave_3", "proton only"),
        ] {
            let save = save_dir(dir).join(name);
            fs::create_dir_all(&save).unwrap();
            fs::write(save.join("save.lsv"), body).unwrap();
        }
        assert_eq!(saves_only_in(&native, &proton), ["Tav-1__QuickSave_1"]);
        assert_eq!(copy_missing_saves(&native, &proton).unwrap(), 1);
        let copied = save_dir(&proton).join("Tav-1__QuickSave_1/save.lsv");
        assert_eq!(fs::read_to_string(copied).unwrap(), "native one");
        let shared = save_dir(&proton).join("Shared__AutoSave_2/save.lsv");
        assert_eq!(fs::read_to_string(shared).unwrap(), "proton copy");
        assert!(saves_only_in(&native, &proton).is_empty());
        // Nothing left behind from the copy.
        let names: Vec<String> = fs::read_dir(save_dir(&proton))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().all(|name| !name.starts_with('.')), "{names:?}");
    }
}
