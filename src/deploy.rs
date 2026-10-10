use crate::{
    backup,
    bg3::GamePaths,
    config::GameConfig,
    game,
    library::{FileOverride, InstallTarget, Library, ModEntry, PakInfo, TargetKind},
    metadata, native_pak, sigillink,
};
use anyhow::{Context, Result};
use larian_formats::bg3::raw::{
    ModuleInfoAttribute, ModulesChildren, ModulesShortDescriptionNode, Save, Version,
};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs, io,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

pub struct DeployReport {
    pub pak_count: usize,
    pub loose_count: usize,
    pub file_count: usize,
    pub removed_count: usize,
    pub overridden_files: usize,
    pub link_mode_summary: String,
    pub warnings: Vec<String>,
    /// Mods left out of modsettings because their file is missing.
    pub missing_mods: Vec<String>,
    /// Fingerprint of the modsettings.lsx this deploy wrote.
    pub modsettings_hash: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ConflictCandidate {
    pub mod_id: String,
    pub mod_name: String,
}

#[derive(Debug, Clone)]
pub struct ConflictEntry {
    pub target: TargetKind,
    pub relative_path: PathBuf,
    pub candidates: Vec<ConflictCandidate>,
    pub winner_id: String,
    pub winner_name: String,
    pub default_winner_id: String,
    pub overridden: bool,
}

#[derive(Debug, Clone)]
pub struct DeployOptions {
    pub backup: bool,
    pub reason: Option<String>,
}

impl Default for DeployOptions {
    fn default() -> Self {
        Self {
            backup: true,
            reason: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SigilLinkMode {
    Hardlink,
    Symlink,
}

impl SigilLinkMode {
    pub fn label(self) -> &'static str {
        match self {
            SigilLinkMode::Hardlink => "hardlink",
            SigilLinkMode::Symlink => "symlink",
        }
    }
}

#[derive(Debug)]
pub struct SigilLinkRelocationError {
    pub target_root: PathBuf,
    pub source: PathBuf,
    pub dest: PathBuf,
    pub err: io::Error,
}

impl std::fmt::Display for SigilLinkRelocationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "sigillink symlink failed: {:?} -> {:?} ({})",
            self.source, self.dest, self.err
        )
    }
}

impl std::error::Error for SigilLinkRelocationError {}

struct LinkModeCache {
    cache_dev: u64,
    modes: HashMap<PathBuf, SigilLinkMode>,
    used: HashSet<SigilLinkMode>,
}

impl LinkModeCache {
    fn new(cache_root: &Path) -> Result<Self> {
        fs::create_dir_all(cache_root).context("create sigillink cache root")?;
        let cache_dev = filesystem_id(cache_root)?;
        Ok(Self {
            cache_dev,
            modes: HashMap::new(),
            used: HashSet::new(),
        })
    }

    fn mode_for(&mut self, target_root: &Path) -> Result<SigilLinkMode> {
        if let Some(mode) = self.modes.get(target_root) {
            self.used.insert(*mode);
            return Ok(*mode);
        }
        let target_dev = filesystem_id(target_root)?;
        let mode = if target_dev == self.cache_dev {
            SigilLinkMode::Hardlink
        } else {
            SigilLinkMode::Symlink
        };
        self.modes.insert(target_root.to_path_buf(), mode);
        self.used.insert(mode);
        Ok(mode)
    }

    fn summary(&self) -> String {
        if self.used.is_empty() {
            return "none".to_string();
        }
        if self.used.len() == 1 {
            return self
                .used
                .iter()
                .next()
                .copied()
                .unwrap()
                .label()
                .to_string();
        }
        "mixed".to_string()
    }
}

pub fn resolve_sigillink_mode(cache_root: &Path, target_root: &Path) -> Result<SigilLinkMode> {
    fs::create_dir_all(cache_root).context("create sigillink cache root")?;
    let cache_dev = filesystem_id(cache_root)?;
    let target_dev = filesystem_id(target_root)?;
    Ok(if target_dev == cache_dev {
        SigilLinkMode::Hardlink
    } else {
        SigilLinkMode::Symlink
    })
}

pub fn summarize_sigillink_modes(cache_root: &Path, targets: &[PathBuf]) -> Result<String> {
    let mut used = HashSet::new();
    for target in targets {
        if target.as_os_str().is_empty() {
            continue;
        }
        let mode = resolve_sigillink_mode(cache_root, target)?;
        used.insert(mode);
    }
    if used.is_empty() {
        return Ok("none".to_string());
    }
    if used.len() == 1 {
        return Ok(used.iter().next().copied().unwrap().label().to_string());
    }
    Ok("mixed".to_string())
}

#[cfg(unix)]
fn filesystem_id(path: &Path) -> Result<u64> {
    Ok(fs::metadata(path)
        .with_context(|| format!("stat {:?}", path))?
        .dev())
}

#[cfg(not(unix))]
fn filesystem_id(path: &Path) -> Result<u64> {
    let _ = path;
    Ok(0)
}

#[cfg(unix)]
fn create_symlink(source: &Path, dest: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(source, dest)
}

#[cfg(not(unix))]
fn create_symlink(_source: &Path, _dest: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Other,
        "symlink unavailable on this platform",
    ))
}

fn link_with_mode(
    source: &Path,
    dest: &Path,
    target_root: &Path,
    mode: SigilLinkMode,
) -> Result<()> {
    if let Ok(meta) = fs::symlink_metadata(dest) {
        if meta.file_type().is_dir() {
            return Err(anyhow::anyhow!(
                "destination exists as directory: {:?}",
                dest
            ));
        }
        fs::remove_file(dest).with_context(|| format!("remove existing file {:?}", dest))?;
    }
    match mode {
        SigilLinkMode::Hardlink => {
            fs::hard_link(source, dest)
                .with_context(|| format!("hardlink {:?} -> {:?}", source, dest))?;
        }
        SigilLinkMode::Symlink => match create_symlink(source, dest) {
            Ok(()) => {}
            Err(err) => {
                if err.kind() == io::ErrorKind::AlreadyExists {
                    let _ = fs::remove_file(dest);
                    if create_symlink(source, dest).is_ok() {
                        return Ok(());
                    }
                }
                if dest.exists() {
                    let _ = fs::remove_file(dest);
                }
                return Err(SigilLinkRelocationError {
                    target_root: target_root.to_path_buf(),
                    source: source.to_path_buf(),
                    dest: dest.to_path_buf(),
                    err,
                }
                .into());
            }
        },
    }
    Ok(())
}

#[derive(Default, Debug, Serialize, Deserialize)]
struct DeployManifest {
    files: Vec<DeployedFile>,
    pak_files: Vec<String>,
    /// How each path in `files` and `pak_files` was linked, so a later deploy removes a
    /// file only while it is still SigilSmith's link. Manifests from before 0.9.11 have none.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    links: HashMap<String, LinkRecord>,
    /// Files that were already at a deploy path, moved aside for SigilSmith's link and put
    /// back when that link goes away.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    set_aside: Vec<SetAsideFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LinkRecord {
    source: String,
    mode: SigilLinkMode,
    /// Device, inode and size of a hardlink when it was made.
    #[serde(default)]
    dev: u64,
    #[serde(default)]
    ino: u64,
    #[serde(default)]
    size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SetAsideFile {
    original: String,
    saved: String,
}

/// File identities in SigilSmith's mod store, read only when an old manifest without link
/// records has to be checked.
struct StoreIndex {
    root: PathBuf,
    identities: Option<HashSet<(u64, u64)>>,
}

impl StoreIndex {
    fn new(cache_root: &Path) -> Self {
        Self {
            root: cache_root.join("mods"),
            identities: None,
        }
    }

    fn contains(&mut self, identity: (u64, u64)) -> bool {
        // No identity to compare (not a Unix filesystem): nothing counts as ours.
        if identity == (0, 0) {
            return false;
        }
        let root = &self.root;
        self.identities
            .get_or_insert_with(|| {
                WalkDir::new(root)
                    .follow_links(false)
                    .into_iter()
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_type().is_file())
                    .filter_map(|entry| entry.metadata().ok())
                    .map(|meta| file_identity(&meta))
                    .collect()
            })
            .contains(&identity)
    }
}

#[cfg(unix)]
fn file_identity(meta: &fs::Metadata) -> (u64, u64) {
    (meta.dev(), meta.ino())
}

#[cfg(not(unix))]
fn file_identity(_meta: &fs::Metadata) -> (u64, u64) {
    (0, 0)
}

fn record_link(source: &Path, dest: &Path, mode: SigilLinkMode) -> LinkRecord {
    let meta = fs::symlink_metadata(dest).ok();
    let (dev, ino) = match (&meta, mode) {
        (Some(meta), SigilLinkMode::Hardlink) => file_identity(meta),
        _ => (0, 0),
    };
    LinkRecord {
        source: source.to_string_lossy().to_string(),
        mode,
        dev,
        ino,
        size: meta.map(|meta| meta.len()).unwrap_or(0),
    }
}

/// Whether `path` still holds the link SigilSmith made there: a symlink into the store, or
/// the same file (device and inode) it hardlinked. Anything else was put there or changed by
/// someone else and is left alone.
fn is_our_link(path: &Path, record: Option<&LinkRecord>, store: &mut StoreIndex) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if path.starts_with(&store.root) {
        return false;
    }
    if meta.file_type().is_symlink() {
        let Ok(target) = fs::read_link(path) else {
            return false;
        };
        return target.starts_with(&store.root)
            || record.is_some_and(|record| target == Path::new(&record.source));
    }
    if !meta.is_file() {
        return false;
    }
    match record {
        Some(record) => {
            record.mode == SigilLinkMode::Hardlink
                && record.ino != 0
                && file_identity(&meta) == (record.dev, record.ino)
                && meta.len() == record.size
        }
        None => store.contains(file_identity(&meta)),
    }
}

/// SigilSmith's own deployed files, for code that must not mistake them for files the game
/// or the player put in the game's folders.
pub struct DeployedFiles {
    manifest: DeployManifest,
    store: std::cell::RefCell<StoreIndex>,
}

impl DeployedFiles {
    pub fn load(data_dir: &Path, cache_root: &Path) -> Self {
        Self {
            manifest: load_manifest(data_dir).unwrap_or_default(),
            store: std::cell::RefCell::new(StoreIndex::new(cache_root)),
        }
    }

    /// True for a link into SigilSmith's store at `path`, whether or not the last deploy
    /// recorded it.
    pub fn is_ours(&self, path: &Path) -> bool {
        let key = path.to_string_lossy();
        let record = self.manifest.links.get(key.as_ref());
        is_our_link(path, record, &mut self.store.borrow_mut())
    }

    fn recorded(&self) -> impl Iterator<Item = &String> {
        self.manifest
            .files
            .iter()
            .map(|file| &file.path)
            .chain(self.manifest.pak_files.iter())
    }

    /// How the last deploy's links compare with what is on disk now, for a Mods folder in
    /// `larian_dir`.
    pub fn drift(&self, larian_dir: &Path) -> LinkDrift {
        let mut drift = LinkDrift::default();
        let mut store = self.store.borrow_mut();
        let paks: HashSet<&String> = self.manifest.pak_files.iter().collect();
        for path_text in self.recorded() {
            let path = Path::new(path_text);
            if paks.contains(path_text) && !path.starts_with(larian_dir) {
                if fs::symlink_metadata(path).is_ok() {
                    drift.elsewhere += 1;
                }
                continue;
            }
            if fs::symlink_metadata(path).is_err() {
                drift.missing += 1;
            } else if !is_our_link(path, self.manifest.links.get(path_text), &mut store) {
                drift.changed += 1;
            }
        }
        drift
    }

    /// Links to SigilSmith's store in `mods_dir` that the last deploy didn't record, as an
    /// older version could leave behind when it changed folders.
    pub fn stray_links_in(&self, mods_dir: &Path) -> Vec<PathBuf> {
        let recorded: HashSet<&str> = self.recorded().map(String::as_str).collect();
        let Ok(entries) = fs::read_dir(mods_dir) else {
            return Vec::new();
        };
        let mut store = self.store.borrow_mut();
        let mut found: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| !recorded.contains(path.to_string_lossy().as_ref()))
            .filter(|path| is_our_link(path, None, &mut store))
            .collect();
        found.sort();
        found
    }

    /// Removes the paths that are still links to SigilSmith's store and returns how many.
    pub fn remove_links(&self, paths: &[PathBuf]) -> usize {
        let mut store = self.store.borrow_mut();
        paths
            .iter()
            .filter(|path| is_our_link(path, None, &mut store) && fs::remove_file(path).is_ok())
            .count()
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LinkDrift {
    /// Recorded links that are gone.
    pub missing: usize,
    /// Recorded paths that hold something else now.
    pub changed: usize,
    /// Recorded paks still in another Larian folder's Mods.
    pub elsewhere: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct DeployedFile {
    target: String,
    path: String,
    #[serde(default)]
    source_mod: Option<String>,
    #[serde(default)]
    source_id: Option<String>,
    #[serde(default)]
    source_kind: Option<String>,
}

struct LooseFilePlan {
    source: PathBuf,
    dest: PathBuf,
    dest_root: PathBuf,
    mod_id: String,
    mod_name: String,
    kind_label: String,
    order: usize,
}

struct LooseFileCandidate {
    source: PathBuf,
    dest_root: PathBuf,
    mod_id: String,
    mod_name: String,
    kind_label: String,
    order: usize,
    kind: TargetKind,
    relative_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ModSettingsModule {
    pub info: PakInfo,
    pub created_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ModSettingsSnapshot {
    pub modules: Vec<ModSettingsModule>,
    pub order: Vec<String>,
    pub enabled: HashSet<String>,
}

pub fn deploy_with_options(
    config: &GameConfig,
    library: &mut Library,
    options: DeployOptions,
) -> Result<DeployReport> {
    let paths = game::detect_paths(
        config.game_id,
        Some(&config.game_root),
        Some(&config.larian_dir),
    )?;
    let cache_root = config.sigillink_cache_root();

    let active_profile = library.active_profile().context("active profile not set")?;
    let mod_map = library.index_by_id();
    let file_overrides = active_profile.file_overrides.clone();

    let ordered_mods: Vec<ModEntry> = active_profile
        .order
        .iter()
        .filter_map(|entry| mod_map.get(&entry.id).cloned().map(|m| (entry, m)))
        .filter(|(entry, _)| entry.enabled)
        .map(|(_, m)| m)
        .collect();

    let all_mods: Vec<ModEntry> = active_profile
        .order
        .iter()
        .filter_map(|entry| mod_map.get(&entry.id).cloned())
        .collect();

    // A mod whose file is gone is left out of modsettings (BG3 drops such entries itself)
    // and listed, so the rest of the load order still deploys.
    let native_pak_index = native_pak::build_native_pak_index_cached(&paths.larian_mods_dir);
    let mut missing_mods = Vec::new();
    let mut present: HashSet<String> = HashSet::new();
    for mod_entry in &all_mods {
        let has_file = mod_entry.targets.iter().all(|target| match target {
            InstallTarget::Pak { file, info } => {
                if mod_entry.is_native() {
                    native_pak::resolve_native_pak_path(info, &native_pak_index).is_some()
                        || paths.larian_mods_dir.join(file).exists()
                } else {
                    library_mod_path(&cache_root, &mod_entry.id)
                        .join(file)
                        .exists()
                }
            }
            _ => true,
        });
        if has_file {
            present.insert(mod_entry.id.clone());
        } else if ordered_mods
            .iter()
            .any(|enabled| enabled.id == mod_entry.id)
        {
            missing_mods.push(mod_entry.display_name());
        }
    }

    let mut enabled_paks = Vec::new();
    let mut installed_paks = Vec::new();
    let mut loose_targets = Vec::new();

    for mod_entry in &ordered_mods {
        let mut has_loose = false;
        for target in &mod_entry.targets {
            let kind = target.kind();
            if !mod_entry.is_target_enabled(kind) {
                continue;
            }
            match target {
                InstallTarget::Pak { info, .. } => {
                    if present.contains(&mod_entry.id) {
                        enabled_paks.push(info.clone());
                    }
                }
                InstallTarget::Generated { .. }
                | InstallTarget::Data { .. }
                | InstallTarget::Bin { .. } => has_loose = true,
            }
        }
        if has_loose && !mod_entry.is_native() {
            loose_targets.push(mod_entry.clone());
        }
    }

    for mod_entry in &all_mods {
        if !present.contains(&mod_entry.id) {
            continue;
        }
        for target in &mod_entry.targets {
            let kind = target.kind();
            if !mod_entry.is_target_enabled(kind) {
                continue;
            }
            if let InstallTarget::Pak { info, .. } = target {
                installed_paks.push(info.clone());
            }
        }
    }

    if options.backup {
        backup::create_backup(
            config,
            library,
            Some(&paths.modsettings_path),
            options.reason.as_deref(),
        )?;
    }

    let mut manifest = load_manifest(&config.data_dir)?;
    merge_set_aside_journal(&config.data_dir, &mut manifest);
    let mut warnings = Vec::new();
    let mut store = StoreIndex::new(&cache_root);
    let (removed_count, restored) =
        remove_previous_deploy(&mut manifest, &mut store, &mut warnings);
    let mut deploy_state = DeployState {
        data_dir: config.data_dir.clone(),
        store,
        links: HashMap::new(),
        pak_files: Vec::new(),
        set_aside: std::mem::take(&mut manifest.set_aside),
        restored,
        warnings,
    };

    let linked = link_mods(
        &paths,
        &cache_root,
        &all_mods,
        &present,
        &loose_targets,
        &file_overrides,
        &mut manifest,
        &mut deploy_state,
    );
    // Record what was linked even when a link failed part way, so the next deploy can
    // clean it up and put back anything moved aside.
    manifest.pak_files = std::mem::take(&mut deploy_state.pak_files);
    manifest.links = std::mem::take(&mut deploy_state.links);
    manifest.set_aside = std::mem::take(&mut deploy_state.set_aside);
    save_manifest(&config.data_dir, &manifest)?;
    let _ = fs::remove_file(config.data_dir.join(SET_ASIDE_JOURNAL));
    let (overridden_files, link_mode_summary) = linked?;
    update_modsettings(&paths, &installed_paks, &enabled_paks)?;
    let modsettings_hash = read_modsettings_snapshot(&paths.modsettings_path)
        .ok()
        .map(|snapshot| modsettings_fingerprint(&snapshot));
    if modsettings_hash.is_some() {
        library.modsettings_hash = modsettings_hash.clone();
    }
    if let Some(marker) = crate::bg3::clear_crash_marker(&paths.larian_dir) {
        deploy_state.warnings.push(format!(
            "Removed {}: BG3 left it after a crash and would have turned every mod off on its next launch",
            marker.display()
        ));
    }

    let file_count = manifest.files.len() + manifest.pak_files.len();
    let mut warnings = deploy_state.warnings;
    if !missing_mods.is_empty() {
        warnings.push(format!(
            "Left out of the load order until their files are back: {}",
            missing_mods.join(", ")
        ));
    }

    Ok(DeployReport {
        pak_count: installed_paks.len(),
        loose_count: loose_targets.len(),
        file_count,
        removed_count,
        overridden_files,
        link_mode_summary,
        warnings,
        missing_mods,
        modsettings_hash,
    })
}

/// Links every managed pak into the Mods folder and loose files into the game folder.
/// Returns the overridden loose file count and the link mode summary.
#[allow(clippy::too_many_arguments)]
fn link_mods(
    paths: &GamePaths,
    cache_root: &Path,
    all_mods: &[ModEntry],
    present: &HashSet<String>,
    loose_targets: &[ModEntry],
    file_overrides: &[FileOverride],
    manifest: &mut DeployManifest,
    deploy_state: &mut DeployState,
) -> Result<(usize, String)> {
    let mut link_modes = LinkModeCache::new(cache_root)?;
    for mod_entry in all_mods {
        if mod_entry.is_native() || !present.contains(&mod_entry.id) {
            continue;
        }
        for target in &mod_entry.targets {
            let kind = target.kind();
            if !mod_entry.is_target_enabled(kind) {
                continue;
            }
            if let InstallTarget::Pak { file, info } = target {
                let source = library_mod_path(cache_root, &mod_entry.id).join(file);
                let dest = paths.larian_mods_dir.join(format!("{}.pak", info.folder));
                fs::create_dir_all(&paths.larian_mods_dir).context("create mods dir")?;
                let mode = link_modes.mode_for(&paths.larian_mods_dir)?;
                deploy_state.make_room(&dest)?;
                link_with_mode(&source, &dest, &paths.larian_mods_dir, mode)
                    .with_context(|| format!("deploy pak {:?}", source))?;
                deploy_state.record(&source, &dest, mode);
                deploy_state
                    .pak_files
                    .push(dest.to_string_lossy().to_string());
            }
        }
    }

    let overridden_files = deploy_loose_files(
        paths,
        loose_targets,
        cache_root,
        manifest,
        file_overrides,
        &mut link_modes,
        deploy_state,
    )?;
    Ok((overridden_files, link_modes.summary()))
}

pub fn scan_conflicts(config: &GameConfig, library: &Library) -> Result<Vec<ConflictEntry>> {
    let paths = game::detect_paths(
        config.game_id,
        Some(&config.game_root),
        Some(&config.larian_dir),
    )?;

    let active_profile = library.active_profile().context("active profile not set")?;
    let mod_map = library.index_by_id();
    let ordered_mods: Vec<ModEntry> = active_profile
        .order
        .iter()
        .filter_map(|entry| mod_map.get(&entry.id).cloned().map(|m| (entry, m)))
        .filter(|(entry, _)| entry.enabled)
        .map(|(_, m)| m)
        .collect();

    let file_overrides = active_profile.file_overrides.clone();
    let (_plans, conflicts, _overridden_files) = build_loose_plan(
        &paths,
        &ordered_mods,
        &config.sigillink_cache_root(),
        &file_overrides,
    )?;
    Ok(conflicts)
}

/// What the native sync compares to notice modsettings.lsx changing outside SigilSmith.
pub fn modsettings_fingerprint(snapshot: &ModSettingsSnapshot) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"modsettings-v2");
    let mut module_ids: Vec<&str> = snapshot
        .modules
        .iter()
        .map(|module| module.info.uuid.as_str())
        .collect();
    module_ids.sort();
    for id in module_ids {
        hasher.update(id.as_bytes());
    }
    let mut enabled_ids: Vec<&str> = snapshot.enabled.iter().map(|id| id.as_str()).collect();
    enabled_ids.sort();
    hasher.update(b"|enabled|");
    for id in enabled_ids {
        hasher.update(id.as_bytes());
    }
    hasher.update(b"|order|");
    for id in &snapshot.order {
        hasher.update(id.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

pub fn read_modsettings_snapshot(path: &Path) -> Result<ModSettingsSnapshot> {
    let save = read_modsettings(path)?;
    let nodes: VecDeque<ModulesShortDescriptionNode> = save
        .find_node_by_id("Mods")
        .ok()
        .and_then(|node| node.children.get(0))
        .map(|child| child.node.clone())
        .unwrap_or_default();

    let mut base_uuids = HashSet::new();
    let mut modules = Vec::new();
    let mut enabled = HashSet::new();
    let mut saw_enabled_attr = false;
    let mut mods_order = Vec::new();
    for node in nodes {
        let uuid = match module_attr(&node, "UUID") {
            Some(uuid) => uuid,
            None => continue,
        };
        let name = module_attr(&node, "Name").unwrap_or_else(|| "Unknown".to_string());
        let folder = module_attr(&node, "Folder").unwrap_or_else(|| uuid.clone());
        if is_base_module(&name, &folder) {
            base_uuids.insert(uuid);
            continue;
        }
        let version = module_attr(&node, "Version64")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let publish_handle =
            module_attr(&node, "PublishHandle").and_then(|value| value.parse::<u64>().ok());
        let md5 = module_attr(&node, "MD5");

        let created_at = module_attr(&node, "Created")
            .or_else(|| module_attr(&node, "CreatedOn"))
            .and_then(|value| metadata::parse_created_at_value(&value));

        let enabled_attr = module_attr(&node, "Enabled");
        let is_enabled = match enabled_attr {
            Some(value) => {
                saw_enabled_attr = true;
                value == "1" || value.eq_ignore_ascii_case("true")
            }
            None => false,
        };
        if is_enabled {
            enabled.insert(uuid.clone());
        }
        mods_order.push(uuid.clone());
        modules.push(ModSettingsModule {
            info: PakInfo {
                uuid,
                name,
                folder,
                version,
                md5,
                publish_handle,
                author: None,
                description: None,
                module_type: None,
            },
            created_at,
        });
    }

    let mod_order = save
        .find_node_by_id("ModOrder")
        .ok()
        .and_then(|node| node.children.get(0))
        .map(|child| {
            child
                .node
                .iter()
                .filter_map(|node| {
                    node.attribute
                        .iter()
                        .find(|attr| attr.id == "UUID")
                        .map(|attr| attr.value.clone())
                })
                .filter(|uuid| !base_uuids.contains(uuid))
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();

    let mut order = if !mods_order.is_empty() {
        mods_order
    } else {
        mod_order
    };

    if order.is_empty() {
        order = modules
            .iter()
            .map(|module| module.info.uuid.clone())
            .collect();
    }

    if !saw_enabled_attr {
        enabled = order.iter().cloned().collect();
    }

    Ok(ModSettingsSnapshot {
        modules,
        order,
        enabled,
    })
}

fn update_modsettings(
    paths: &GamePaths,
    installed_paks: &[PakInfo],
    enabled_paks: &[PakInfo],
) -> Result<()> {
    let save = read_modsettings(&paths.modsettings_path)?;
    let save = build_modsettings_save(save, installed_paks, enabled_paks);
    write_modsettings(&paths.modsettings_path, &save)
}

pub(crate) fn build_modsettings_export(
    modsettings_path: &Path,
    installed_paks: &[PakInfo],
    enabled_paks: &[PakInfo],
) -> Result<Save> {
    let save = read_modsettings(modsettings_path)?;
    Ok(build_modsettings_save(save, installed_paks, enabled_paks))
}

fn build_modsettings_save(
    mut save: Save,
    _installed_paks: &[PakInfo],
    enabled_paks: &[PakInfo],
) -> Save {
    let existing_nodes: VecDeque<ModulesShortDescriptionNode> = save
        .find_node_by_id("Mods")
        .ok()
        .and_then(|node| node.children.get(0))
        .map(|child| child.node.clone())
        .unwrap_or_default();

    let mut base_nodes = Vec::new();
    let mut base_uuid_order = Vec::new();

    for node in &existing_nodes {
        let name = node
            .attribute
            .iter()
            .find(|attr| attr.id == "Name")
            .map(|attr| attr.value.clone())
            .unwrap_or_default();
        let folder = node
            .attribute
            .iter()
            .find(|attr| attr.id == "Folder")
            .map(|attr| attr.value.clone())
            .unwrap_or_default();
        if is_base_module(&name, &folder) {
            if let Some(uuid) = node
                .attribute
                .iter()
                .find(|attr| attr.id == "UUID")
                .map(|attr| attr.value.clone())
            {
                base_uuid_order.push(uuid.clone());
            }
            base_nodes.push(node.clone());
        }
    }

    let mut mods_list = VecDeque::new();
    for node in &base_nodes {
        mods_list.push_back(node.clone());
    }

    for info in enabled_paks {
        mods_list.push_back(module_short_desc_from_info(info));
    }

    let mods_node = save.get_or_insert_node_mut_by_id("Mods");
    mods_node.children = vec![ModulesChildren { node: mods_list }];

    let mut order_list = VecDeque::new();
    for uuid in base_uuid_order.iter() {
        order_list.push_back(module_order_node(uuid));
    }

    for info in enabled_paks {
        order_list.push_back(module_order_node(&info.uuid));
    }

    let mod_order_node = save.get_or_insert_node_mut_by_id("ModOrder");
    mod_order_node.children = vec![ModulesChildren { node: order_list }];

    save
}

fn module_attr(node: &ModulesShortDescriptionNode, key: &str) -> Option<String> {
    node.attribute
        .iter()
        .find(|attr| attr.id == key)
        .map(|attr| attr.value.clone())
}

fn is_base_module(name: &str, folder: &str) -> bool {
    matches!(
        name,
        "Gustav" | "GustavX" | "GustavDev" | "Honour" | "HonourX"
    ) || matches!(
        folder,
        "Gustav" | "GustavX" | "GustavDev" | "Honour" | "HonourX"
    )
}

fn module_short_desc_from_info(info: &PakInfo) -> ModulesShortDescriptionNode {
    ModulesShortDescriptionNode {
        id: "ModuleShortDesc".to_string(),
        attribute: vec![
            ModuleInfoAttribute::new("Folder", &info.folder, "LSString"),
            ModuleInfoAttribute::new("MD5", info.md5.clone().unwrap_or_default(), "LSString"),
            ModuleInfoAttribute::new("Name", &info.name, "LSString"),
            ModuleInfoAttribute::new(
                "PublishHandle",
                info.publish_handle.unwrap_or(0).to_string(),
                "uint64",
            ),
            ModuleInfoAttribute::new("UUID", &info.uuid, "guid"),
            ModuleInfoAttribute::new("Version64", info.version.to_string(), "int64"),
        ],
    }
}

fn module_order_node(uuid: &str) -> ModulesShortDescriptionNode {
    ModulesShortDescriptionNode {
        id: "Module".to_string(),
        attribute: vec![ModuleInfoAttribute::new("UUID", uuid, "FixedString")],
    }
}

fn read_modsettings(path: &Path) -> Result<Save> {
    if !path.exists() {
        return Ok(default_modsettings());
    }
    let raw = fs::read_to_string(path).context("read modsettings.lsx")?;
    let parsed = quick_xml::de::from_str(&raw).context("parse modsettings.lsx")?;
    Ok(parsed)
}

pub(crate) fn write_modsettings_export(path: &Path, save: &Save) -> Result<()> {
    let xml = modsettings_xml(save)?;
    write_atomic_text(path, &xml).context("write modsettings export")
}

fn write_modsettings(path: &Path, save: &Save) -> Result<()> {
    let xml = modsettings_xml(save)?;
    fs::create_dir_all(path.parent().context("modsettings parent")?)
        .context("create modsettings dir")?;
    fs::write(path, xml).context("write modsettings")?;
    Ok(())
}

fn modsettings_xml(save: &Save) -> Result<String> {
    let mut xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n".to_string();
    let mut ser = quick_xml::se::Serializer::new(&mut xml);
    ser.indent(' ', 4);
    save.serialize(ser).context("serialize modsettings")?;
    xml.push('\n');
    Ok(xml.replace("/>\n", " />\n"))
}

fn write_atomic_text(path: &Path, contents: &str) -> Result<()> {
    let parent = path.parent().context("modsettings export parent")?;
    fs::create_dir_all(parent).context("create modsettings export dir")?;
    let file_name = path.file_name().context("modsettings export filename")?;
    let mut temp_name = std::ffi::OsString::from(file_name);
    temp_name.push(".tmp");
    let mut temp_path = parent.join(temp_name);
    if temp_path.exists() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let mut temp_name = std::ffi::OsString::from(file_name);
        temp_name.push(format!(".{stamp}.tmp"));
        temp_path = parent.join(temp_name);
    }
    fs::write(&temp_path, contents).context("write modsettings export temp")?;
    fs::rename(&temp_path, path).context("finalize modsettings export")?;
    Ok(())
}

fn default_modsettings() -> Save {
    Save {
        version: Version {
            major: 4,
            minor: 8,
            revision: 0,
            build: 500,
        },
        region: larian_formats::bg3::raw::Region {
            id: "ModuleSettings".to_string(),
            node: larian_formats::bg3::raw::ConfigNode {
                id: "root".to_string(),
                children: larian_formats::bg3::raw::ConfigChildren { node: Vec::new() },
            },
        },
    }
}

fn deploy_loose_files(
    paths: &GamePaths,
    mods: &[ModEntry],
    cache_root: &Path,
    manifest: &mut DeployManifest,
    file_overrides: &[FileOverride],
    link_modes: &mut LinkModeCache,
    deploy_state: &mut DeployState,
) -> Result<usize> {
    let (plans, _conflicts, overridden_files) =
        build_loose_plan(paths, mods, cache_root, file_overrides)?;
    let mut deployed = Vec::with_capacity(plans.len());
    let mut created = Vec::with_capacity(plans.len());

    for plan in plans {
        if let Some(parent) = plan.dest.parent() {
            fs::create_dir_all(parent).context("create dir")?;
        }
        let mode = link_modes.mode_for(&plan.dest_root)?;
        let linked = deploy_state
            .make_room(&plan.dest)
            .and_then(|()| link_with_mode(&plan.source, &plan.dest, &plan.dest_root, mode));
        if let Err(err) = linked {
            for path in created.iter().rev() {
                let _ = fs::remove_file(path);
            }
            return Err(err).context("deploy loose file");
        }
        deploy_state.record(&plan.source, &plan.dest, mode);
        created.push(plan.dest.clone());
        deployed.push(DeployedFile {
            target: plan.dest_root.to_string_lossy().to_string(),
            path: plan.dest.to_string_lossy().to_string(),
            source_mod: Some(plan.mod_name.clone()),
            source_id: Some(plan.mod_id.clone()),
            source_kind: Some(plan.kind_label.clone()),
        });
    }

    manifest.files = deployed;
    Ok(overridden_files)
}

fn build_loose_plan(
    paths: &GamePaths,
    mods: &[ModEntry],
    cache_root: &Path,
    file_overrides: &[FileOverride],
) -> Result<(Vec<LooseFilePlan>, Vec<ConflictEntry>, usize)> {
    let mut map: HashMap<PathBuf, Vec<LooseFileCandidate>> = HashMap::new();

    for (order, mod_entry) in mods.iter().enumerate() {
        let mod_root = library_mod_path(cache_root, &mod_entry.id);
        let sigillink_index = sigillink::load_sigillink_index(cache_root, &mod_entry.id);
        for target in &mod_entry.targets {
            let kind = target.kind();
            if !mod_entry.is_target_enabled(kind) {
                continue;
            }
            let (source_root, dest_root, kind_label, kind) = match target {
                InstallTarget::Generated { dir } => (
                    mod_root.join(dir),
                    paths.data_dir.join("Generated"),
                    "Generated",
                    TargetKind::Generated,
                ),
                InstallTarget::Data { dir } => (
                    mod_root.join(dir),
                    paths.data_dir.clone(),
                    "Data",
                    TargetKind::Data,
                ),
                InstallTarget::Bin { dir } => (
                    mod_root.join(dir),
                    paths.game_root.join("bin"),
                    "Bin",
                    TargetKind::Bin,
                ),
                InstallTarget::Pak { .. } => continue,
            };
            if !source_root.exists() {
                continue;
            }
            if let Some(index) = sigillink_index.as_ref() {
                collect_target_files_from_index(
                    &source_root,
                    &dest_root,
                    mod_entry,
                    kind_label,
                    kind,
                    order,
                    index,
                    &mut map,
                )?;
            } else {
                collect_target_files(
                    &source_root,
                    &dest_root,
                    mod_entry,
                    kind_label,
                    kind,
                    order,
                    &mut map,
                )?;
            }
        }
    }

    let override_map = build_override_map(file_overrides);
    let mut plans = Vec::new();
    let mut conflicts = Vec::new();
    let mut overridden = 0usize;

    for (dest, mut candidates) in map {
        candidates.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.mod_id.cmp(&b.mod_id)));
        let default = candidates.last().context("loose plan candidate missing")?;
        let key = (default.kind, default.relative_path.clone());
        let mut winner = default;
        let mut overridden_flag = false;

        if let Some(override_mod_id) = override_map.get(&key) {
            if let Some(candidate) = candidates
                .iter()
                .find(|candidate| &candidate.mod_id == override_mod_id)
            {
                winner = candidate;
                overridden_flag = candidate.mod_id != default.mod_id;
            }
        }

        if candidates.len() > 1 {
            overridden = overridden.saturating_add(candidates.len() - 1);
            conflicts.push(ConflictEntry {
                target: winner.kind,
                relative_path: winner.relative_path.clone(),
                candidates: candidates
                    .iter()
                    .map(|candidate| ConflictCandidate {
                        mod_id: candidate.mod_id.clone(),
                        mod_name: candidate.mod_name.clone(),
                    })
                    .collect(),
                winner_id: winner.mod_id.clone(),
                winner_name: winner.mod_name.clone(),
                default_winner_id: default.mod_id.clone(),
                overridden: overridden_flag,
            });
        }

        plans.push(LooseFilePlan {
            source: winner.source.clone(),
            dest: dest.clone(),
            dest_root: winner.dest_root.clone(),
            mod_id: winner.mod_id.clone(),
            mod_name: winner.mod_name.clone(),
            kind_label: winner.kind_label.clone(),
            order: winner.order,
        });
    }

    plans.sort_by(|a, b| {
        a.order
            .cmp(&b.order)
            .then_with(|| a.dest.to_string_lossy().cmp(&b.dest.to_string_lossy()))
    });
    conflicts.sort_by(|a, b| {
        a.relative_path
            .to_string_lossy()
            .cmp(&b.relative_path.to_string_lossy())
    });
    Ok((plans, conflicts, overridden))
}

fn collect_target_files(
    source_root: &Path,
    dest_root: &Path,
    mod_entry: &ModEntry,
    kind_label: &str,
    kind: TargetKind,
    order: usize,
    map: &mut HashMap<PathBuf, Vec<LooseFileCandidate>>,
) -> Result<()> {
    for entry in WalkDir::new(source_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !is_ignored_deploy_path(entry.path()))
    {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry.path().strip_prefix(source_root).context("rel path")?;
        let dest = dest_root.join(rel);
        map.entry(dest.clone())
            .or_default()
            .push(LooseFileCandidate {
                source: entry.path().to_path_buf(),
                dest_root: dest_root.to_path_buf(),
                mod_id: mod_entry.id.clone(),
                mod_name: mod_entry.name.clone(),
                kind_label: kind_label.to_string(),
                order,
                kind,
                relative_path: rel.to_path_buf(),
            });
    }

    Ok(())
}

fn collect_target_files_from_index(
    source_root: &Path,
    dest_root: &Path,
    mod_entry: &ModEntry,
    kind_label: &str,
    kind: TargetKind,
    order: usize,
    index: &sigillink::SigilLinkIndex,
    map: &mut HashMap<PathBuf, Vec<LooseFileCandidate>>,
) -> Result<()> {
    for entry in &index.entries {
        if entry.kind != kind {
            continue;
        }
        let rel = PathBuf::from(&entry.relative_path);
        if is_ignored_deploy_path(&rel) {
            continue;
        }
        let source = source_root.join(&rel);
        if !source.exists() {
            continue;
        }
        let dest = dest_root.join(&rel);
        map.entry(dest.clone())
            .or_default()
            .push(LooseFileCandidate {
                source,
                dest_root: dest_root.to_path_buf(),
                mod_id: mod_entry.id.clone(),
                mod_name: mod_entry.name.clone(),
                kind_label: kind_label.to_string(),
                order,
                kind,
                relative_path: rel,
            });
    }

    Ok(())
}

fn build_override_map(file_overrides: &[FileOverride]) -> HashMap<(TargetKind, PathBuf), String> {
    let mut map = HashMap::new();
    for override_entry in file_overrides {
        map.insert(
            (
                override_entry.kind,
                PathBuf::from(&override_entry.relative_path),
            ),
            override_entry.mod_id.clone(),
        );
    }
    map
}

fn is_ignored_deploy_path(path: &Path) -> bool {
    path.components().any(|component| {
        let part = component.as_os_str().to_string_lossy();
        part.eq_ignore_ascii_case("__MACOSX")
            || part.eq_ignore_ascii_case(".ds_store")
            || part.eq_ignore_ascii_case("thumbs.db")
            || part == ".git"
            || part == ".svn"
            || part == ".vscode"
    })
}

/// Removes the last deploy's links wherever they were made (also in a Mods folder SigilSmith
/// no longer points at), but only while each is still SigilSmith's link, then puts back files
/// that were moved aside for them. Returns the count removed and the paths put back.
fn remove_previous_deploy(
    manifest: &mut DeployManifest,
    store: &mut StoreIndex,
    warnings: &mut Vec<String>,
) -> (usize, HashSet<PathBuf>) {
    let mut removed = 0;
    let recorded: Vec<String> = manifest
        .files
        .iter()
        .map(|file| file.path.clone())
        .chain(manifest.pak_files.iter().cloned())
        .collect();
    for path_text in recorded {
        let path = PathBuf::from(&path_text);
        if fs::symlink_metadata(&path).is_err() {
            continue;
        }
        if !is_our_link(&path, manifest.links.get(&path_text), store) {
            warnings.push(format!(
                "Left {} in place: it changed after SigilSmith deployed it",
                path.display()
            ));
            continue;
        }
        if fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    manifest.files.clear();
    manifest.pak_files.clear();
    manifest.links.clear();

    let mut restored = HashSet::new();
    let mut kept = Vec::new();
    for item in manifest.set_aside.drain(..) {
        let original = PathBuf::from(&item.original);
        let saved = PathBuf::from(&item.saved);
        if fs::symlink_metadata(&saved).is_err() {
            continue;
        }
        if fs::symlink_metadata(&original).is_ok() {
            // A link an interrupted deploy made there is SigilSmith's own and makes way.
            let ours = is_our_link(&original, None, store) && fs::remove_file(&original).is_ok();
            if !ours {
                warnings.push(format!(
                    "Kept {} aside: something else is at {} now",
                    saved.display(),
                    original.display()
                ));
                kept.push(item);
                continue;
            }
        }
        let moved = original
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| move_file(&saved, &original));
        match moved {
            Ok(()) => {
                remove_empty_parents(&saved);
                restored.insert(original);
            }
            Err(err) => {
                warnings.push(format!(
                    "Could not put {} back at {}: {err}",
                    saved.display(),
                    original.display()
                ));
                kept.push(item);
            }
        }
    }
    manifest.set_aside = kept;
    (removed, restored)
}

struct DeployState {
    data_dir: PathBuf,
    store: StoreIndex,
    links: HashMap<String, LinkRecord>,
    pak_files: Vec<String>,
    set_aside: Vec<SetAsideFile>,
    /// Files put back at the start of this deploy; moving them aside again isn't news.
    restored: HashSet<PathBuf>,
    warnings: Vec<String>,
}

impl DeployState {
    /// Clears `dest` for a link. SigilSmith's own link (one this deploy made, or one an
    /// interrupted deploy left behind) is replaced; any other file is moved aside, never
    /// deleted.
    fn make_room(&mut self, dest: &Path) -> Result<()> {
        let Ok(meta) = fs::symlink_metadata(dest) else {
            return Ok(());
        };
        if meta.file_type().is_dir() {
            return Err(anyhow::anyhow!(
                "destination exists as directory: {:?}",
                dest
            ));
        }
        if self.links.contains_key(dest.to_string_lossy().as_ref())
            || is_our_link(dest, None, &mut self.store)
        {
            return Ok(());
        }
        let name = dest
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_else(|| "file".into());
        let set_aside_root = self.data_dir.join("set-aside");
        let mut index = self.set_aside.len();
        let saved = loop {
            let candidate = set_aside_root.join(index.to_string()).join(&name);
            if fs::symlink_metadata(&candidate).is_err() {
                break candidate;
            }
            index += 1;
        };
        if let Some(parent) = saved.parent() {
            fs::create_dir_all(parent).context("create set-aside dir")?;
        }
        move_file(dest, &saved).with_context(|| format!("move {:?} aside", dest))?;
        if !self.restored.contains(dest) {
            self.warnings.push(format!(
                "Moved {} to {} so SigilSmith's copy could go there; it goes back when that mod is no longer deployed",
                dest.display(),
                saved.display()
            ));
        }
        self.set_aside.push(SetAsideFile {
            original: dest.to_string_lossy().to_string(),
            saved: saved.to_string_lossy().to_string(),
        });
        // Recorded at once: if the deploy stops before the manifest is saved, the next one
        // still knows to put the file back.
        let raw = serde_json::to_string_pretty(&self.set_aside).context("serialize set-aside")?;
        fs::write(self.data_dir.join(SET_ASIDE_JOURNAL), raw).context("record set-aside file")?;
        Ok(())
    }

    fn record(&mut self, source: &Path, dest: &Path, mode: SigilLinkMode) {
        self.links.insert(
            dest.to_string_lossy().to_string(),
            record_link(source, dest, mode),
        );
    }
}

/// Renames `from` to `to`, copying across filesystems. A symlink stays a symlink.
fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    let meta = fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        create_symlink(&fs::read_link(from)?, to)?;
    } else {
        fs::copy(from, to)?;
    }
    fs::remove_file(from)
}

/// Removes the numbered folder a set-aside file sat in once it is empty.
fn remove_empty_parents(saved: &Path) {
    if let Some(parent) = saved.parent() {
        let _ = fs::remove_dir(parent);
    }
}

fn load_manifest(data_dir: &Path) -> Result<DeployManifest> {
    let path = data_dir.join("deploy_manifest.json");
    if !path.exists() {
        return Ok(DeployManifest::default());
    }

    let raw = fs::read_to_string(path).context("read manifest")?;
    let manifest = serde_json::from_str(&raw).context("parse manifest")?;
    Ok(manifest)
}

/// Files moved aside by a deploy that stopped before saving its manifest.
const SET_ASIDE_JOURNAL: &str = "set-aside/journal.json";

fn merge_set_aside_journal(data_dir: &Path, manifest: &mut DeployManifest) {
    let Some(entries) = fs::read_to_string(data_dir.join(SET_ASIDE_JOURNAL))
        .ok()
        .and_then(|raw| serde_json::from_str::<Vec<SetAsideFile>>(&raw).ok())
    else {
        return;
    };
    for entry in entries {
        if !manifest
            .set_aside
            .iter()
            .any(|known| known.saved == entry.saved)
        {
            manifest.set_aside.push(entry);
        }
    }
}

fn save_manifest(data_dir: &Path, manifest: &DeployManifest) -> Result<()> {
    let path = data_dir.join("deploy_manifest.json");
    let raw = serde_json::to_string_pretty(manifest).context("serialize manifest")?;
    fs::write(path, raw).context("write manifest")?;
    Ok(())
}

fn library_mod_path(cache_root: &Path, id: &str) -> PathBuf {
    cache_root.join("mods").join(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{ModScripts, ModSource, Profile, ProfileEntry};

    struct Fixture {
        root: PathBuf,
        config: GameConfig,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("sigilsmith-deploy-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            for dir in ["game/Data", "game/bin", "native/PlayerProfiles/Public"] {
                fs::create_dir_all(root.join(dir)).unwrap();
            }
            fs::create_dir_all(root.join("proton/PlayerProfiles/Public")).unwrap();
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
            fs::create_dir_all(&config.data_dir).unwrap();
            Self { root, config }
        }

        fn mods_dir(&self, larian: &str) -> PathBuf {
            self.root.join(larian).join("Mods")
        }

        /// A managed pak mod in the store, enabled in the Default profile.
        fn add_managed(&self, library: &mut Library, id: &str, folder: &str) {
            let store = self.config.sigillink_mods_root().join(id);
            fs::create_dir_all(&store).unwrap();
            fs::write(store.join(format!("{folder}.pak")), folder.as_bytes()).unwrap();
            add_entry(library, id, folder, ModSource::Managed);
        }

        fn deploy(&self, library: &mut Library) -> DeployReport {
            deploy_to(&self.config, library)
        }

        fn modsettings_uuids(&self, larian: &str) -> Vec<String> {
            let path = self
                .root
                .join(larian)
                .join("PlayerProfiles/Public/modsettings.lsx");
            read_modsettings_snapshot(&path)
                .unwrap()
                .modules
                .into_iter()
                .map(|module| module.info.uuid)
                .collect()
        }
    }

    fn deploy_to(config: &GameConfig, library: &mut Library) -> DeployReport {
        deploy_with_options(
            config,
            library,
            DeployOptions {
                backup: false,
                reason: None,
            },
        )
        .unwrap()
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn add_entry(library: &mut Library, id: &str, folder: &str, source: ModSource) {
        library.mods.push(ModEntry {
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
        });
        library.profiles[0].order.push(ProfileEntry {
            id: id.to_string(),
            enabled: true,
            missing_label: None,
        });
    }

    fn library() -> Library {
        Library {
            mods: Vec::new(),
            profiles: vec![Profile::new("Default")],
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

    const KAI: &str = "baf029fa-af6e-4080-bd6a-abacdea9e684";
    const CAMERA: &str = "6e84559a-1b7a-4441-bfc6-c2bae5538491";

    #[test]
    fn switching_folders_leaves_links_only_in_the_new_one() {
        let fixture = Fixture::new("switch");
        let mut library = library();
        fixture.add_managed(&mut library, KAI, "KaiLimeUI");
        fixture.deploy(&mut library);
        let old_link = fixture.mods_dir("native").join("KaiLimeUI.pak");
        assert!(old_link.exists());

        let mut proton = fixture.config.clone();
        proton.larian_dir = fixture.root.join("proton");
        let report = deploy_to(&proton, &mut library);

        assert!(!old_link.exists(), "old folder still has the link");
        assert!(fixture.mods_dir("proton").join("KaiLimeUI.pak").exists());
        assert_eq!(report.removed_count, 1);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        // The store copy is untouched.
        let store = fixture
            .config
            .sigillink_mods_root()
            .join(KAI)
            .join("KaiLimeUI.pak");
        assert_eq!(fs::read(store).unwrap(), b"KaiLimeUI");
    }

    #[test]
    fn a_file_replaced_after_deploy_is_left_alone() {
        let fixture = Fixture::new("replaced");
        let mut library = library();
        fixture.add_managed(&mut library, KAI, "KaiLimeUI");
        fixture.deploy(&mut library);
        let link = fixture.mods_dir("native").join("KaiLimeUI.pak");
        fs::remove_file(&link).unwrap();
        fs::write(&link, b"someone else's copy").unwrap();

        library.mods.clear();
        library.profiles[0].order.clear();
        let report = fixture.deploy(&mut library);

        assert_eq!(fs::read(&link).unwrap(), b"someone else's copy");
        assert_eq!(report.removed_count, 0);
        assert!(
            report.warnings.iter().any(|w| w.contains("Left")),
            "{:?}",
            report.warnings
        );
    }

    #[test]
    fn a_file_in_the_way_is_set_aside_and_put_back() {
        let fixture = Fixture::new("set-aside");
        let mods = fixture.mods_dir("native");
        fs::create_dir_all(&mods).unwrap();
        fs::write(mods.join("KaiLimeUI.pak"), b"player's own copy").unwrap();
        let mut library = library();
        fixture.add_managed(&mut library, KAI, "KaiLimeUI");

        let report = fixture.deploy(&mut library);
        assert_eq!(fs::read(mods.join("KaiLimeUI.pak")).unwrap(), b"KaiLimeUI");
        assert!(
            report.warnings.iter().any(|w| w.contains("Moved")),
            "{:?}",
            report.warnings
        );

        // A second deploy keeps it aside without repeating the warning.
        let report = fixture.deploy(&mut library);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(fs::read(mods.join("KaiLimeUI.pak")).unwrap(), b"KaiLimeUI");

        library.profiles[0].order.clear();
        library.mods.clear();
        fixture.deploy(&mut library);
        assert_eq!(
            fs::read(mods.join("KaiLimeUI.pak")).unwrap(),
            b"player's own copy"
        );
        let manifest = load_manifest(&fixture.config.data_dir).unwrap();
        assert!(manifest.set_aside.is_empty());
    }

    #[test]
    fn missing_mods_are_left_out_of_modsettings_and_listed() {
        let fixture = Fixture::new("missing");
        let mut library = library();
        fixture.add_managed(&mut library, KAI, "KaiLimeUI");
        add_entry(
            &mut library,
            CAMERA,
            "TrueThirdPersonCamera",
            ModSource::Native,
        );

        let report = fixture.deploy(&mut library);
        assert_eq!(fixture.modsettings_uuids("native"), [KAI]);
        assert_eq!(report.missing_mods, ["TrueThirdPersonCamera"]);

        // Once the game puts the file back, the mod is listed again.
        let mods = fixture.mods_dir("native");
        fs::write(mods.join("TrueThirdPersonCamera.pak"), b"x").unwrap();
        let report = fixture.deploy(&mut library);
        assert!(report.missing_mods.is_empty());
        let uuids = fixture.modsettings_uuids("native");
        assert!(uuids.contains(&CAMERA.to_string()), "{uuids:?}");
        // The game's own file is never touched.
        assert_eq!(
            fs::read(mods.join("TrueThirdPersonCamera.pak")).unwrap(),
            b"x"
        );
    }

    #[test]
    fn old_manifests_without_link_records_still_clean_up_safely() {
        let fixture = Fixture::new("legacy");
        let mut library = library();
        fixture.add_managed(&mut library, KAI, "KaiLimeUI");
        fixture.deploy(&mut library);
        let mods = fixture.mods_dir("native");
        let ours = mods.join("KaiLimeUI.pak");
        let foreign = mods.join("Other.pak");
        fs::write(&foreign, b"not ours").unwrap();

        // What 0.9.10 wrote: paths only.
        let mut manifest = load_manifest(&fixture.config.data_dir).unwrap();
        manifest.links.clear();
        manifest
            .pak_files
            .push(foreign.to_string_lossy().to_string());
        save_manifest(&fixture.config.data_dir, &manifest).unwrap();

        library.mods.clear();
        library.profiles[0].order.clear();
        let report = fixture.deploy(&mut library);
        assert!(!ours.exists());
        assert_eq!(fs::read(&foreign).unwrap(), b"not ours");
        assert_eq!(report.removed_count, 1);
    }

    #[test]
    fn a_file_set_aside_by_an_interrupted_deploy_is_put_back() {
        let fixture = Fixture::new("interrupted");
        let mut library = library();
        fixture.add_managed(&mut library, KAI, "KaiLimeUI");
        let mods = fixture.mods_dir("native");
        fs::create_dir_all(&mods).unwrap();
        fs::write(mods.join("KaiLimeUI.pak"), b"mine").unwrap();
        fixture.deploy(&mut library);
        // As if the deploy stopped after moving the file aside and linking, before it saved
        // its manifest: only the set-aside journal knows.
        let manifest = load_manifest(&fixture.config.data_dir).unwrap();
        let journal = fixture.config.data_dir.join(SET_ASIDE_JOURNAL);
        fs::write(
            &journal,
            serde_json::to_string(&manifest.set_aside).unwrap(),
        )
        .unwrap();
        save_manifest(&fixture.config.data_dir, &DeployManifest::default()).unwrap();

        library.profiles[0].order.clear();
        library.mods.clear();
        fixture.deploy(&mut library);
        assert_eq!(fs::read(mods.join("KaiLimeUI.pak")).unwrap(), b"mine");
        assert!(!journal.exists());
        assert!(load_manifest(&fixture.config.data_dir)
            .unwrap()
            .set_aside
            .is_empty());
    }

    #[test]
    fn drift_and_stray_links_are_found_and_only_links_are_removed() {
        let fixture = Fixture::new("drift");
        let mut library = library();
        fixture.add_managed(&mut library, "a", "Alpha");
        fixture.add_managed(&mut library, "b", "Beta");
        fixture.deploy(&mut library);
        let native = fixture.root.join("native");
        let proton = fixture.root.join("proton");
        let deployed = DeployedFiles::load(
            &fixture.config.data_dir,
            &fixture.config.sigillink_cache_root(),
        );
        assert_eq!(deployed.drift(&native), LinkDrift::default());

        // One link deleted, one replaced by the player.
        fs::remove_file(fixture.mods_dir("native").join("Alpha.pak")).unwrap();
        fs::remove_file(fixture.mods_dir("native").join("Beta.pak")).unwrap();
        fs::write(fixture.mods_dir("native").join("Beta.pak"), b"mine").unwrap();
        let drift = deployed.drift(&native);
        assert_eq!((drift.missing, drift.changed, drift.elsewhere), (1, 1, 0));
        // Seen from the Proton folder, the native links are left elsewhere.
        assert_eq!(deployed.drift(&proton).elsewhere, 1);

        // An unrecorded link in the other folder, next to the game's own file.
        let proton_mods = fixture.mods_dir("proton");
        fs::create_dir_all(&proton_mods).unwrap();
        let store_pak = fixture.config.sigillink_mods_root().join("a/Alpha.pak");
        fs::hard_link(&store_pak, proton_mods.join("Alpha.pak")).unwrap();
        std::os::unix::fs::symlink(&store_pak, proton_mods.join("AlphaLink.pak")).unwrap();
        fs::write(proton_mods.join("Game.pak"), b"game").unwrap();
        let strays = deployed.stray_links_in(&proton_mods);
        assert_eq!(
            strays,
            [
                proton_mods.join("Alpha.pak"),
                proton_mods.join("AlphaLink.pak")
            ]
        );
        let mut with_game_file = strays.clone();
        with_game_file.push(proton_mods.join("Game.pak"));
        assert_eq!(deployed.remove_links(&with_game_file), 2);
        assert!(proton_mods.join("Game.pak").exists());
        assert!(store_pak.exists());
    }

    #[test]
    fn deployed_files_recognizes_its_links_only() {
        let fixture = Fixture::new("ours");
        let mut library = library();
        fixture.add_managed(&mut library, KAI, "KaiLimeUI");
        fixture.deploy(&mut library);
        let mods = fixture.mods_dir("native");
        fs::write(mods.join("game_download.pak"), b"x").unwrap();

        let deployed = DeployedFiles::load(
            &fixture.config.data_dir,
            &fixture.config.sigillink_cache_root(),
        );
        assert!(deployed.is_ours(&mods.join("KaiLimeUI.pak")));
        assert!(!deployed.is_ours(&mods.join("game_download.pak")));
        assert!(!deployed.is_ours(&mods.join("absent.pak")));
    }
}
