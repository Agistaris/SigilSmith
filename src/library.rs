use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const SIGILLINK_RANKING_PROFILE: &str = "__sigillink_ranking__";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SigilLinkRankMeta {
    #[serde(default)]
    pub last_ranked_at: Option<i64>,
    #[serde(default)]
    pub last_moves: usize,
    #[serde(default)]
    pub last_pins: usize,
    #[serde(default)]
    pub last_inputs_hash: Option<String>,
}

pub fn is_sigillink_ranking_profile(name: &str) -> bool {
    name == SIGILLINK_RANKING_PROFILE
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    pub mods: Vec<ModEntry>,
    pub profiles: Vec<Profile>,
    pub active_profile: String,
    #[serde(default)]
    pub dependency_blocks: HashSet<String>,
    #[serde(default)]
    pub metadata_cache_version: u32,
    #[serde(default)]
    pub metadata_cache_key: Option<String>,
    #[serde(default)]
    pub modsettings_hash: Option<String>,
    #[serde(default = "default_true")]
    pub modsettings_sync_enabled: bool,
}

impl Library {
    pub fn load_or_create(data_dir: &Path) -> Result<Self> {
        let library_path = data_dir.join("library.json");
        if library_path.exists() {
            let raw = fs::read_to_string(&library_path).context("read library.json")?;
            let mut library: Library = serde_json::from_str(&raw).context("parse library.json")?;
            if library.profiles.is_empty() {
                library.profiles.push(Profile::new("Default"));
            }
            if library.active_profile.is_empty() {
                library.active_profile = library.profiles[0].name.clone();
            } else if !library
                .profiles
                .iter()
                .any(|profile| profile.name == library.active_profile)
            {
                library.active_profile = library.profiles[0].name.clone();
            }
            return Ok(library);
        }

        let library = Library {
            mods: Vec::new(),
            profiles: vec![Profile::new("Default")],
            active_profile: "Default".to_string(),
            dependency_blocks: HashSet::new(),
            metadata_cache_version: 0,
            metadata_cache_key: None,
            modsettings_hash: None,
            modsettings_sync_enabled: true,
        };
        library.save(data_dir)?;
        Ok(library)
    }

    pub fn save(&self, data_dir: &Path) -> Result<()> {
        let library_path = data_dir.join("library.json");
        let raw = serde_json::to_string_pretty(self).context("serialize library.json")?;
        fs::write(library_path, raw).context("write library.json")?;
        Ok(())
    }

    pub fn active_profile_mut(&mut self) -> Option<&mut Profile> {
        self.profiles
            .iter_mut()
            .find(|profile| profile.name == self.active_profile)
    }

    pub fn active_profile(&self) -> Option<&Profile> {
        self.profiles
            .iter()
            .find(|profile| profile.name == self.active_profile)
    }

    pub fn ensure_mods_in_profiles(&mut self) {
        let mod_ids: Vec<String> = self.mods.iter().map(|m| m.id.clone()).collect();
        let mod_set: HashSet<&str> = mod_ids.iter().map(|id| id.as_str()).collect();
        for profile in &mut self.profiles {
            if is_sigillink_ranking_profile(&profile.name) {
                continue;
            }
            profile.ensure_mods(&mod_ids);
        }
        self.dependency_blocks
            .retain(|id| mod_set.contains(id.as_str()));
    }

    pub fn index_by_id(&self) -> HashMap<String, ModEntry> {
        self.mods
            .iter()
            .cloned()
            .map(|mod_entry| (mod_entry.id.clone(), mod_entry))
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub order: Vec<ProfileEntry>,
    #[serde(default)]
    pub file_overrides: Vec<FileOverride>,
    #[serde(default)]
    pub sigillink_pins: HashMap<String, usize>,
    #[serde(default)]
    pub sigillink_meta: SigilLinkRankMeta,
}

impl Profile {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            order: Vec::new(),
            file_overrides: Vec::new(),
            sigillink_pins: HashMap::new(),
            sigillink_meta: SigilLinkRankMeta::default(),
        }
    }

    pub fn ensure_mods(&mut self, mod_ids: &[String]) {
        let mod_set: std::collections::HashSet<&String> = mod_ids.iter().collect();
        for id in mod_ids {
            if !self.order.iter().any(|entry| entry.id == *id) {
                self.order.push(ProfileEntry {
                    id: id.clone(),
                    enabled: false,
                    missing_label: None,
                });
            }
        }
        self.file_overrides
            .retain(|override_entry| mod_set.contains(&override_entry.mod_id));
        self.sigillink_pins
            .retain(|mod_id, _| mod_set.contains(&mod_id));
    }

    pub fn move_up(&mut self, index: usize) {
        if index == 0 || index >= self.order.len() {
            return;
        }
        self.order.swap(index, index - 1);
    }

    pub fn move_down(&mut self, index: usize) {
        if index + 1 >= self.order.len() {
            return;
        }
        self.order.swap(index, index + 1);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileEntry {
    pub id: String,
    pub enabled: bool,
    #[serde(default)]
    pub missing_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOverride {
    pub kind: TargetKind,
    pub relative_path: String,
    pub mod_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub created_at: Option<i64>,
    #[serde(default)]
    pub modified_at: Option<i64>,
    pub added_at: i64,
    pub targets: Vec<InstallTarget>,
    #[serde(default)]
    pub target_overrides: Vec<TargetOverride>,
    #[serde(default)]
    pub source_label: Option<String>,
    #[serde(default)]
    pub source: ModSource,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default, skip_serializing_if = "ModScripts::is_empty")]
    pub scripts: ModScripts,
}

/// Scripting a mod ships, found from the same files BG3 Mod Manager checks for its
/// Script Extender and Osiris icons.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModScripts {
    /// Set when the mod has a Mods/<Folder>/ScriptExtender/Config.json.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_extender: Option<ScriptExtenderUse>,
    /// The mod has Osiris goal scripts under Mods/<Folder>/Story/RawFiles/Goals.
    #[serde(default, skip_serializing_if = "is_false")]
    pub osiris: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptExtenderUse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
}

impl ModScripts {
    pub fn is_empty(&self) -> bool {
        self.script_extender.is_none() && !self.osiris
    }

    /// Combines what several paks or folders of one mod ship.
    pub fn merge(&mut self, other: ModScripts) {
        self.osiris |= other.osiris;
        let Some(found) = other.script_extender else {
            return;
        };
        if let Some(existing) = self.script_extender.as_mut() {
            existing.required_version = existing.required_version.max(found.required_version);
            for feature in found.features {
                if !existing.features.contains(&feature) {
                    existing.features.push(feature);
                }
            }
        } else {
            self.script_extender = Some(found);
        }
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn default_true() -> bool {
    true
}

impl ModEntry {
    pub fn display_name(&self) -> String {
        let name = strip_trailing_id(&self.name);
        if let Some(label) = &self.source_label {
            let stripped = strip_trailing_id(label);
            if stripped.len() < label.trim_end().len() && !name.trim().is_empty() {
                // A generated pak file name; the mod's own name reads better.
                return name.to_string();
            }
            let cleaned = clean_source_label(stripped);
            if !cleaned.is_empty() {
                return cleaned;
            }
        }
        name.to_string()
    }

    pub fn source_label(&self) -> Option<&str> {
        self.source_label.as_deref()
    }

    pub fn is_native(&self) -> bool {
        matches!(self.source, ModSource::Native)
    }

    /// A .pak without a readable meta.lsx: imported whole into the game's Data
    /// folder, where BG3 always loads it, with no load-order entry.
    pub fn is_override_pak(&self) -> bool {
        self.id.starts_with("pak-")
            && matches!(self.targets.as_slice(), [InstallTarget::Data { dir }] if dir == "Data")
    }

    /// The mod list's Kind column.
    pub fn kind_label(&self) -> &'static str {
        if self.is_override_pak() {
            return "Override";
        }
        let has_pak = self
            .targets
            .iter()
            .any(|target| matches!(target, InstallTarget::Pak { .. }));
        let has_loose = self
            .targets
            .iter()
            .any(|target| !matches!(target, InstallTarget::Pak { .. }));
        match (has_pak, has_loose) {
            (true, true) => "Mixed",
            (true, false) => "Pak",
            (false, true) => "Loose",
            _ => "Unknown",
        }
    }

    pub fn display_type(&self) -> String {
        if self.is_override_pak() {
            return "Override Pak".to_string();
        }
        let mut kinds = Vec::new();
        let mut has_pak = false;
        let mut has_generated = false;
        let mut has_data = false;
        let mut has_bin = false;

        for target in &self.targets {
            match target {
                InstallTarget::Pak { .. } => has_pak = true,
                InstallTarget::Generated { .. } => has_generated = true,
                InstallTarget::Data { .. } => has_data = true,
                InstallTarget::Bin { .. } => has_bin = true,
            }
        }

        if has_pak {
            kinds.push("Pak");
        }
        if has_generated {
            kinds.push("Generated");
        }
        if has_data {
            kinds.push("Data");
        }
        if has_bin {
            kinds.push("Bin");
        }

        if kinds.is_empty() {
            "Unknown".to_string()
        } else {
            kinds.join("+")
        }
    }

    pub fn has_target_kind(&self, kind: TargetKind) -> bool {
        self.targets.iter().any(|target| target.kind() == kind)
    }

    pub fn is_target_enabled(&self, kind: TargetKind) -> bool {
        if !self.has_target_kind(kind) {
            return false;
        }

        self.target_overrides
            .iter()
            .find(|override_entry| override_entry.kind == kind)
            .map(|override_entry| override_entry.enabled)
            .unwrap_or(true)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModSource {
    Managed,
    Native,
}

impl Default for ModSource {
    fn default() -> Self {
        Self::Managed
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstallTarget {
    Pak { file: String, info: PakInfo },
    Generated { dir: String },
    Data { dir: String },
    Bin { dir: String },
}

impl InstallTarget {
    pub fn kind(&self) -> TargetKind {
        match self {
            InstallTarget::Pak { .. } => TargetKind::Pak,
            InstallTarget::Generated { .. } => TargetKind::Generated,
            InstallTarget::Data { .. } => TargetKind::Data,
            InstallTarget::Bin { .. } => TargetKind::Bin,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Pak,
    Generated,
    Data,
    Bin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetOverride {
    pub kind: TargetKind,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PakInfo {
    pub uuid: String,
    pub name: String,
    pub folder: String,
    pub version: u64,
    pub md5: Option<String>,
    pub publish_handle: Option<u64>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub module_type: Option<String>,
}

impl PakInfo {
    pub fn from_module_info(info: larian_formats::bg3::ModuleInfo) -> Self {
        Self {
            uuid: info.uuid,
            name: info.name,
            folder: info.folder,
            version: info.version,
            md5: info.md5,
            publish_handle: None,
            author: info.author,
            description: info.description,
            module_type: info.module_type,
        }
    }
}

pub fn library_mod_root(cache_root: &Path) -> PathBuf {
    cache_root.join("mods")
}

pub fn path_times(path: &Path) -> (Option<i64>, Option<i64>) {
    let meta = fs::metadata(path).ok();
    let created_at = meta
        .as_ref()
        .and_then(|m| m.created().ok())
        .and_then(system_time_to_epoch);
    let modified_at = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(system_time_to_epoch);
    (created_at, modified_at)
}

pub fn normalize_times(created: Option<i64>, modified: Option<i64>) -> (Option<i64>, Option<i64>) {
    match (created, modified) {
        (Some(created), Some(modified)) => {
            (Some(created.min(modified)), Some(created.max(modified)))
        }
        (Some(created), None) => (Some(created), Some(created)),
        (None, Some(modified)) => (Some(modified), Some(modified)),
        (None, None) => (None, None),
    }
}

pub fn resolve_times(
    primary_created: Option<i64>,
    file_created: Option<i64>,
    file_modified: Option<i64>,
) -> (Option<i64>, Option<i64>) {
    if let Some(primary) = primary_created {
        let modified = file_modified
            .or(file_created)
            .map(|value| value.max(primary))
            .or(Some(primary));
        return (Some(primary), modified);
    }

    normalize_times(file_created, file_modified)
}

fn system_time_to_epoch(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs() as i64)
}

/// Drops an ID that some mod names and pak file names carry after the name:
/// a full UUID ("AlfiraJoinsTheParty_3539eba9-6d77-c53d-1009-b3c77c9cd04c")
/// or the shortened form the in-game mod manager uses
/// ("bettercontainers_cb42bc3a-f1d2-afwl").
fn strip_trailing_id(label: &str) -> &str {
    let trimmed = label.trim_end();
    let Some(head) = strip_uuid_suffix(trimmed).or_else(|| strip_mod_manager_suffix(trimmed))
    else {
        return trimmed;
    };
    let head =
        head.trim_end_matches(|ch: char| ch.is_whitespace() || ch == '_' || ch == '-' || ch == '.');
    if head.is_empty() {
        trimmed
    } else {
        head
    }
}

fn strip_uuid_suffix(label: &str) -> Option<&str> {
    let split = label.len().checked_sub(36)?;
    (label.is_char_boundary(split) && is_uuid(&label[split..])).then(|| &label[..split])
}

/// The in-game mod manager names paks "<name>_<start of the UUID>-<4 random
/// characters>", e.g. "tashascauldronhairstyles_1af5b-i3zl".
fn strip_mod_manager_suffix(label: &str) -> Option<&str> {
    let (head, tail) = label.rsplit_once('_')?;
    let (uuid_start, random) = tail.rsplit_once('-')?;
    let random_ok = random.len() == 4
        && random
            .chars()
            .all(|ch| ch.is_ascii_digit() || ch.is_ascii_lowercase());
    let uuid_start_ok = (4..36).contains(&uuid_start.len())
        && uuid_start
            .chars()
            .enumerate()
            .all(|(index, ch)| match index {
                8 | 13 | 18 | 23 => ch == '-',
                _ => ch.is_ascii_hexdigit(),
            });
    (random_ok && uuid_start_ok).then_some(head)
}

fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value.chars().enumerate().all(|(index, ch)| match index {
            8 | 13 | 18 | 23 => ch == '-',
            _ => ch.is_ascii_hexdigit(),
        })
}

pub fn clean_source_label(label: &str) -> String {
    let raw = label.trim().replace('_', " ");
    if raw.is_empty() {
        return String::new();
    }
    let raw = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let joiner = if raw.contains(" - ") { " - " } else { "-" };
    let parts: Vec<&str> = raw.split('-').collect();
    let mut idx = parts.len();
    let mut numeric_segments: Vec<&str> = Vec::new();

    while idx > 0 {
        let seg = parts[idx - 1].trim();
        if seg.is_empty() {
            idx -= 1;
            continue;
        }
        if seg.chars().all(|c| c.is_ascii_digit()) {
            numeric_segments.push(seg);
            idx -= 1;
        } else {
            break;
        }
    }

    let mut keep_len = parts.len();
    if !numeric_segments.is_empty() {
        let last_len = numeric_segments[0].len();
        let has_timestamp = last_len >= 10;
        let has_nexus_chain = numeric_segments.len() >= 4;
        if has_timestamp || has_nexus_chain {
            keep_len = idx;
        }
    }

    let mut cleaned_parts = Vec::new();
    for part in parts.iter().take(keep_len) {
        let trimmed = part.trim();
        if !trimmed.is_empty() {
            cleaned_parts.push(trimmed);
        }
    }

    let mut base = cleaned_parts.join(joiner);
    base = base.split_whitespace().collect::<Vec<_>>().join(" ");
    let base = base.trim().to_string();
    if base.is_empty() {
        raw
    } else {
        base
    }
}

pub fn normalize_label(label: &str) -> String {
    let cleaned = clean_source_label(label);
    let mut out = String::new();
    for ch in cleaned.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_label_drops_trailing_uuid() {
        let label = "AlfiraJoinsTheParty_3539eba9-6d77-c53d-1009-b3c77c9cd04c";
        assert_eq!(
            clean_source_label(strip_trailing_id(label)),
            "AlfiraJoinsTheParty"
        );
        let label = "Aardi_KnightsandDames_194a7429-5b34-7d68-06ee-494607e165e4";
        assert_eq!(
            clean_source_label(strip_trailing_id(label)),
            "Aardi KnightsandDames"
        );
        // Native mod names can carry the UUID after a space.
        assert_eq!(
            strip_trailing_id("AlfiraJoinsTheParty 3539eba9-6d77-c53d-1009-b3c77c9cd04c"),
            "AlfiraJoinsTheParty"
        );
    }

    fn entry(name: &str, source_label: Option<&str>) -> ModEntry {
        ModEntry {
            id: "id".to_string(),
            name: name.to_string(),
            created_at: None,
            modified_at: None,
            added_at: 0,
            targets: Vec::new(),
            target_overrides: Vec::new(),
            source_label: source_label.map(str::to_string),
            source: ModSource::Managed,
            dependencies: Vec::new(),
            scripts: ModScripts::default(),
        }
    }

    #[test]
    fn override_paks_get_their_own_kind() {
        let mut mod_entry = entry("Override Pak: Tweaks", None);
        mod_entry.id = "pak-1a2b".to_string();
        mod_entry.targets = vec![InstallTarget::Data {
            dir: "Data".to_string(),
        }];
        assert!(mod_entry.is_override_pak());
        assert_eq!(mod_entry.kind_label(), "Override");
        assert_eq!(mod_entry.display_type(), "Override Pak");

        // Loose Data files are not an override pak.
        mod_entry.id = "loose-1a2b".to_string();
        assert!(!mod_entry.is_override_pak());
        assert_eq!(mod_entry.kind_label(), "Loose");
        assert_eq!(mod_entry.display_type(), "Data");
    }

    #[test]
    fn scripts_merge_keeps_highest_version_and_all_features() {
        let mut scripts = ModScripts {
            script_extender: Some(ScriptExtenderUse {
                required_version: Some(18),
                features: vec!["Lua".to_string()],
            }),
            osiris: false,
        };
        scripts.merge(ModScripts {
            script_extender: Some(ScriptExtenderUse {
                required_version: Some(29),
                features: vec!["Lua".to_string(), "Preprocessor".to_string()],
            }),
            osiris: true,
        });
        let script_extender = scripts.script_extender.as_ref().unwrap();
        assert_eq!(script_extender.required_version, Some(29));
        assert_eq!(script_extender.features, ["Lua", "Preprocessor"]);
        assert!(scripts.osiris);
    }

    #[test]
    fn mods_without_scripts_save_and_load_as_before() {
        let plain = entry("Plain", None);
        let json = serde_json::to_string(&plain).unwrap();
        assert!(!json.contains("scripts"), "{json}");
        let loaded: ModEntry = serde_json::from_str(&json).unwrap();
        assert!(loaded.scripts.is_empty());

        let mut scripted = entry("Scripted", None);
        scripted.scripts.osiris = true;
        let json = serde_json::to_string(&scripted).unwrap();
        let loaded: ModEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.scripts, scripted.scripts);
    }

    #[test]
    fn display_name_prefers_mod_name_over_generated_file_name() {
        let generated = entry(
            "Better Healing Potions",
            Some("betterhealingpotions_a909a143-gw7g"),
        );
        assert_eq!(generated.display_name(), "Better Healing Potions");
        // A named archive keeps its label (often with a version).
        let archive = entry("Party Limit Begone", Some("Party Limit Begone SE v3.5"));
        assert_eq!(archive.display_name(), "Party Limit Begone SE v3.5");
        let native = entry(
            "AlfiraJoinsTheParty 3539eba9-6d77-c53d-1009-b3c77c9cd04c",
            None,
        );
        assert_eq!(native.display_name(), "AlfiraJoinsTheParty");
    }

    #[test]
    fn display_label_drops_mod_manager_suffix() {
        for (label, expected) in [
            ("bettercontainers_cb42bc3a-f1d2-afwl", "bettercontainers"),
            ("betterinventoryui_6b585be8-ed7-bplo", "betterinventoryui"),
            (
                "addonbetterinventoryui_8c7d340-3bzc",
                "addonbetterinventoryui",
            ),
            ("beards_aff0c400-ef4e-bc46-c255-69dx", "beards"),
            ("weightlessgold_81117bd5-de2f-a-du9y", "weightlessgold"),
            (
                "tashascauldronhairstyles_1af5b-i3zl",
                "tashascauldronhairstyles",
            ),
            (
                "aza_npcre_theemeraldgrove_74cb-bl78",
                "aza_npcre_theemeraldgrove",
            ),
            ("ImpUI_26922ba9-6018-5252-075d-7ff2ba6ed879", "ImpUI"),
        ] {
            assert_eq!(strip_trailing_id(label), expected);
        }
    }

    #[test]
    fn display_label_keeps_names_without_uuid() {
        assert_eq!(
            strip_trailing_id("Party Limit Begone SE v3.5"),
            "Party Limit Begone SE v3.5"
        );
        // A bare UUID stays as-is rather than becoming an empty name.
        let bare = "3539eba9-6d77-c53d-1009-b3c77c9cd04c";
        assert_eq!(strip_trailing_id(bare), bare);
        assert_eq!(strip_trailing_id("Ünïcödé name"), "Ünïcödé name");
        // Ordinary names with dashes or versions are left alone.
        for label in [
            "Party_Limit_Begone-SE-v3.5",
            "Mod_Configuration_Menu-9162-1-41-0-1727000000",
            "Some_Mod_final-v2",
            "Cool_Armor_1234567-abc",
            "Better_Dyes_v2-beta",
            "Camp_Events_2024-final",
        ] {
            assert_eq!(strip_trailing_id(label), label);
        }
    }
}
