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
    resolve_native_pak_path(info, native_pak_index).and_then(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(|s| s.to_string())
    })
}

pub fn resolve_native_pak_path(
    info: &PakInfo,
    native_pak_index: &[NativePakEntry],
) -> Option<PathBuf> {
    if native_pak_index.is_empty() {
        return None;
    }

    let folder_key = normalize_pak_key(&info.folder);
    let name_key = normalize_pak_key(&info.name);
    let uuid_key = normalize_pak_key(&info.uuid);
    let uuid_prefix = uuid_key
        .get(0..16)
        .or_else(|| uuid_key.get(0..12))
        .unwrap_or("")
        .to_string();

    let mut best: Option<&NativePakEntry> = None;
    let mut best_score = 0i32;
    let mut best_len_diff = usize::MAX;

    for entry in native_pak_index {
        let mut score = 0i32;
        let mut len_diff = usize::MAX;

        if let Some(detail) = match_detail(&entry.normalized, &uuid_key, 120, 110, 80) {
            score = score.saturating_add(detail.score);
            len_diff = len_diff.min(detail.len_diff);
        }
        if !uuid_prefix.is_empty() {
            if let Some(detail) = match_detail(&entry.normalized, &uuid_prefix, 70, 55, 35) {
                score = score.saturating_add(detail.score);
                len_diff = len_diff.min(detail.len_diff);
            }
        }
        if let Some(detail) = match_detail(&entry.normalized, &folder_key, 90, 70, 50) {
            score = score.saturating_add(detail.score);
            len_diff = len_diff.min(detail.len_diff);
        }
        if let Some(detail) = match_detail(&entry.normalized, &name_key, 85, 65, 45) {
            score = score.saturating_add(detail.score);
            len_diff = len_diff.min(detail.len_diff);
        }

        if score == 0 {
            continue;
        }

        if score > best_score || (score == best_score && len_diff < best_len_diff) {
            best = Some(entry);
            best_score = score;
            best_len_diff = len_diff;
        }
    }

    best.map(|entry| entry.path.clone())
}

struct MatchDetail {
    score: i32,
    len_diff: usize,
}

fn match_detail(
    haystack: &str,
    needle: &str,
    exact: i32,
    prefix: i32,
    contains: i32,
) -> Option<MatchDetail> {
    if needle.is_empty() {
        return None;
    }
    if haystack == needle {
        return Some(MatchDetail {
            score: exact,
            len_diff: 0,
        });
    }
    if haystack.starts_with(needle) {
        return Some(MatchDetail {
            score: prefix,
            len_diff: haystack.len().saturating_sub(needle.len()),
        });
    }
    if haystack.contains(needle) {
        return Some(MatchDetail {
            score: contains,
            len_diff: haystack.len().saturating_sub(needle.len()),
        });
    }
    None
}

fn normalize_pak_key(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}
