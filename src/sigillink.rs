use crate::library::TargetKind;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub const SIGILLINK_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SigilLinkIndex {
    pub version: u32,
    pub entries: Vec<SigilLinkEntry>,
    #[serde(default)]
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SigilLinkEntry {
    pub kind: TargetKind,
    pub relative_path: String,
    pub size: u64,
}

pub fn sigillink_root(cache_root: &Path) -> PathBuf {
    cache_root.join("sigillink")
}

pub fn sigillink_index_path(cache_root: &Path, mod_id: &str) -> PathBuf {
    sigillink_root(cache_root).join(format!("{mod_id}.json"))
}

pub fn write_sigillink_index(
    cache_root: &Path,
    mod_id: &str,
    index: &SigilLinkIndex,
) -> Result<()> {
    let path = sigillink_index_path(cache_root, mod_id);
    let parent = path.parent().context("sigillink parent")?;
    fs::create_dir_all(parent).context("create sigillink dir")?;

    let raw = serde_json::to_string_pretty(index).context("serialize sigillink index")?;
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, raw).context("write sigillink temp")?;
    if path.exists() {
        let _ = fs::remove_file(&path);
    }
    fs::rename(&temp, &path).context("finalize sigillink index")?;
    Ok(())
}

pub fn load_sigillink_index(cache_root: &Path, mod_id: &str) -> Option<SigilLinkIndex> {
    let path = sigillink_index_path(cache_root, mod_id);
    let raw = fs::read_to_string(path).ok()?;
    let index: SigilLinkIndex = serde_json::from_str(&raw).ok()?;
    if index.version != SIGILLINK_VERSION {
        return None;
    }
    Some(index)
}

pub fn remove_sigillink_index(cache_root: &Path, mod_id: &str) {
    let path = sigillink_index_path(cache_root, mod_id);
    let _ = fs::remove_file(path);
}
