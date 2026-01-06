use crate::library::PakInfo;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct NativePakEntry {
    pub path: PathBuf,
    normalized: String,
}

pub fn build_native_pak_index(larian_mods_dir: &Path) -> Vec<NativePakEntry> {
    let mut entries = Vec::new();
    let Ok(dir_entries) = fs::read_dir(larian_mods_dir) else {
        return entries;
    };

    for entry in dir_entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("pak"))
            != Some(true)
        {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let normalized = normalize_pak_key(stem);
        entries.push(NativePakEntry { path, normalized });
    }
    entries
}

pub fn resolve_native_pak_filename(
    info: &PakInfo,
    native_pak_index: &[NativePakEntry],
) -> Option<String> {
    resolve_native_pak_path(info, native_pak_index)
        .and_then(|path| path.file_name().and_then(|name| name.to_str()).map(|s| s.to_string()))
}

pub fn resolve_native_pak_path(
    info: &PakInfo,
    native_pak_index: &[NativePakEntry],
) -> Option<PathBuf> {
    if native_pak_index.is_empty() {
        return None;
    }

    let folder_key = normalize_pak_key(&info.folder);
    if let Some(entry) = find_single_match(native_pak_index, &folder_key) {
        return Some(entry.path.clone());
    }

    let uuid_key = normalize_pak_key(&info.uuid);
    if let Some(entry) = find_single_match(native_pak_index, &uuid_key) {
        return Some(entry.path.clone());
    }

    for prefix_len in [16usize, 12, 8] {
        if uuid_key.len() >= prefix_len {
            if let Some(entry) = find_single_match(native_pak_index, &uuid_key[..prefix_len]) {
                return Some(entry.path.clone());
            }
        }
    }

    for prefix_len in [32usize, 24, 16] {
        if folder_key.len() >= prefix_len {
            if let Some(entry) = find_single_match(native_pak_index, &folder_key[..prefix_len]) {
                return Some(entry.path.clone());
            }
        }
    }

    None
}

fn find_single_match<'a>(
    native_pak_index: &'a [NativePakEntry],
    needle: &str,
) -> Option<&'a NativePakEntry> {
    if needle.is_empty() {
        return None;
    }
    let mut matches = native_pak_index
        .iter()
        .filter(|entry| entry.normalized.contains(needle));
    let first = matches.next()?;
    if matches.next().is_some() {
        None
    } else {
        Some(first)
    }
}

fn normalize_pak_key(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}
