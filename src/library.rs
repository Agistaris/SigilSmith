use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    pub mods: Vec<ModEntry>,
    pub profiles: Vec<Profile>,
    pub active_profile: String,
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
        for profile in &mut self.profiles {
            profile.ensure_mods(&mod_ids);
        }
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
}

impl Profile {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            order: Vec::new(),
            file_overrides: Vec::new(),
        }
    }

    pub fn ensure_mods(&mut self, mod_ids: &[String]) {
        let mod_set: std::collections::HashSet<&String> = mod_ids.iter().collect();
        for id in mod_ids {
            if !self.order.iter().any(|entry| entry.id == *id) {
                self.order.push(ProfileEntry {
                    id: id.clone(),
                    enabled: false,
                });
            }
        }
        self.file_overrides
            .retain(|override_entry| mod_set.contains(&override_entry.mod_id));
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
}

impl ModEntry {
    pub fn display_name(&self) -> String {
        if let Some(label) = &self.source_label {
            let cleaned = clean_source_label(label);
            if !cleaned.is_empty() {
                return cleaned;
            }
        }
        self.name.clone()
    }

    pub fn source_label(&self) -> Option<&str> {
        self.source_label.as_deref()
    }

    pub fn is_native(&self) -> bool {
        matches!(self.source, ModSource::Native)
    }

    pub fn display_type(&self) -> String {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

pub fn library_mod_root(data_dir: &Path) -> PathBuf {
    data_dir.join("mods")
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
        (Some(created), Some(modified)) => (
            Some(created.min(modified)),
            Some(created.max(modified)),
        ),
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

pub fn clean_source_label(label: &str) -> String {
    let raw = label.trim().replace('_', " ");
    if raw.is_empty() {
        return String::new();
    }

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

    if !numeric_segments.is_empty() {
        let last_len = numeric_segments[0].len();
        if !(last_len >= 6 || numeric_segments.len() >= 2) {
            idx = parts.len();
        }
    }

    let mut cleaned_parts = Vec::new();
    for part in parts.iter().take(idx) {
        let trimmed = part.trim();
        if !trimmed.is_empty() {
            cleaned_parts.push(trimmed);
        }
    }

    let mut base = cleaned_parts.join(joiner);
    base = base.split_whitespace().collect::<Vec<_>>().join(" ");
    base.trim().to_string()
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
