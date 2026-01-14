use crate::{
    backup,
    bg3::GamePaths,
    config::GameConfig,
    game,
    library::{FileOverride, InstallTarget, Library, ModEntry, PakInfo, TargetKind},
    metadata, sigillink,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
                InstallTarget::Pak { info, .. } => enabled_paks.push(info.clone()),
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
        backup::create_backup(config, library, &paths, options.reason.as_deref())?;
    }

    let mut manifest = load_manifest(&config.data_dir)?;
    let removed_count = remove_previous_deploy(&paths, &mut manifest)?;
    let warnings = Vec::new();
    let mut link_modes = LinkModeCache::new(&cache_root)?;

    let mut pak_files = Vec::new();
    for mod_entry in &all_mods {
        if mod_entry.is_native() {
            continue;
        }
        for target in &mod_entry.targets {
            let kind = target.kind();
            if !mod_entry.is_target_enabled(kind) {
                continue;
            }
            if let InstallTarget::Pak { file, info } = target {
                let source = library_mod_path(&cache_root, &mod_entry.id).join(file);
                let dest = paths.larian_mods_dir.join(format!("{}.pak", info.folder));
                fs::create_dir_all(&paths.larian_mods_dir).context("create mods dir")?;
                let mode = link_modes.mode_for(&paths.larian_mods_dir)?;
                link_with_mode(&source, &dest, &paths.larian_mods_dir, mode)
                    .with_context(|| format!("deploy pak {:?}", source))?;
                pak_files.push(dest.to_string_lossy().to_string());
            }
        }
    }

    let overridden_files = deploy_loose_files(
        &paths,
        &loose_targets,
        &cache_root,
        &mut manifest,
        &file_overrides,
        &mut link_modes,
    )?;
    update_modsettings(&paths, &installed_paks, &enabled_paks)?;

    manifest.pak_files = pak_files;
    save_manifest(&config.data_dir, &manifest)?;

    let file_count = manifest.files.len() + manifest.pak_files.len();
    let link_mode_summary = link_modes.summary();

    Ok(DeployReport {
        pak_count: installed_paks.len(),
        loose_count: loose_targets.len(),
        file_count,
        removed_count,
        overridden_files,
        link_mode_summary,
        warnings,
    })
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

    let order = save
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

    Ok(ModSettingsSnapshot { modules, order })
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
    installed_paks: &[PakInfo],
    enabled_paks: &[PakInfo],
) -> Save {
    let existing_order_uuids: Vec<String> = save
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
                .collect()
        })
        .unwrap_or_default();

    let existing_nodes: VecDeque<ModulesShortDescriptionNode> = save
        .find_node_by_id("Mods")
        .ok()
        .and_then(|node| node.children.get(0))
        .map(|child| child.node.clone())
        .unwrap_or_default();

    let existing_by_uuid: HashMap<String, ModulesShortDescriptionNode> = existing_nodes
        .iter()
        .cloned()
        .filter_map(|node| {
            let uuid = node
                .attribute
                .iter()
                .find(|attr| attr.id == "UUID")
                .map(|attr| attr.value.clone())?;
            Some((uuid, node))
        })
        .collect();

    let mut base_nodes = Vec::new();
    let mut base_uuid_order = Vec::new();
    let mut base_uuids = HashSet::new();

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
                base_uuids.insert(uuid);
            }
            base_nodes.push(node.clone());
        }
    }

    let mut mods_list = VecDeque::new();
    for node in &base_nodes {
        mods_list.push_back(node.clone());
    }

    let installed_uuid_set: HashSet<String> = installed_paks
        .iter()
        .map(|info| info.uuid.clone())
        .collect();

    for (uuid, node) in existing_by_uuid.iter() {
        if base_uuids.contains(uuid) || installed_uuid_set.contains(uuid) {
            continue;
        }
        mods_list.push_back(node.clone());
    }

    for info in installed_paks {
        mods_list.push_back(module_short_desc_from_info(info));
    }

    let mods_node = save.get_or_insert_node_mut_by_id("Mods");
    mods_node.children = vec![ModulesChildren { node: mods_list }];

    let mut order_list = VecDeque::new();
    for uuid in base_uuid_order.iter() {
        order_list.push_back(module_order_node(uuid));
    }

    let enabled_uuid_set: HashSet<String> =
        enabled_paks.iter().map(|info| info.uuid.clone()).collect();

    for info in enabled_paks {
        order_list.push_back(module_order_node(&info.uuid));
    }

    for uuid in existing_order_uuids {
        if base_uuids.contains(&uuid) || enabled_uuid_set.contains(&uuid) {
            continue;
        }
        order_list.push_back(module_order_node(&uuid));
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
        if let Err(err) = link_with_mode(&plan.source, &plan.dest, &plan.dest_root, mode) {
            for path in created.iter().rev() {
                let _ = fs::remove_file(path);
            }
            return Err(err).context("deploy loose file");
        }
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

fn remove_previous_deploy(paths: &GamePaths, manifest: &mut DeployManifest) -> Result<usize> {
    let mut removed = 0;

    for file in &manifest.files {
        let path = PathBuf::from(&file.path);
        if !path.exists() {
            continue;
        }

        let allowed = path.starts_with(&paths.data_dir) || path.starts_with(&paths.game_root);
        if !allowed {
            continue;
        }

        if fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }

    for pak_path in &manifest.pak_files {
        let path = PathBuf::from(pak_path);
        if path.starts_with(&paths.larian_mods_dir) && path.exists() {
            if fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }

    Ok(removed)
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

fn save_manifest(data_dir: &Path, manifest: &DeployManifest) -> Result<()> {
    let path = data_dir.join("deploy_manifest.json");
    let raw = serde_json::to_string_pretty(manifest).context("serialize manifest")?;
    fs::write(path, raw).context("write manifest")?;
    Ok(())
}

fn library_mod_path(cache_root: &Path, id: &str) -> PathBuf {
    cache_root.join("mods").join(id)
}
