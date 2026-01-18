use crate::{library::PakInfo, metadata};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct NativePakEntry {
    pub path: PathBuf,
    normalized: String,
}

#[derive(Debug, Clone)]
struct NativePakMetaEntry {
    path: PathBuf,
    size: u64,
    modified: Option<i64>,
    uuid_key: Option<String>,
    folder_key: Option<String>,
    name_key: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct NativePakMetaIndex {
    entries: Vec<NativePakMetaEntry>,
}

#[derive(Debug, Clone)]
struct NativePakIndexCacheEntry {
    modified: Option<i64>,
    entries: Vec<NativePakEntry>,
}

#[derive(Debug, Clone)]
struct NativePakMetaIndexCacheEntry {
    modified: Option<i64>,
    entries: HashMap<PathBuf, NativePakMetaEntry>,
}

static NATIVE_PAK_INDEX_CACHE: OnceLock<Mutex<HashMap<PathBuf, NativePakIndexCacheEntry>>> =
    OnceLock::new();
static NATIVE_PAK_META_INDEX_CACHE: OnceLock<
    Mutex<HashMap<PathBuf, NativePakMetaIndexCacheEntry>>,
> = OnceLock::new();

fn dir_modified_timestamp(path: &Path) -> Option<i64> {
    let meta = fs::metadata(path).ok()?;
    meta.modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
}

fn pak_signature(path: &Path) -> Option<(u64, Option<i64>)> {
    let meta = fs::metadata(path).ok()?;
    let size = meta.len();
    let modified = meta
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64);
    Some((size, modified))
}

fn normalize_key_opt(value: Option<&str>) -> Option<String> {
    let value = value?;
    let normalized = normalize_pak_key(value);
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn meta_keys(meta: Option<&metadata::ModMeta>) -> (Option<String>, Option<String>, Option<String>) {
    let Some(meta) = meta else {
        return (None, None, None);
    };
    (
        normalize_key_opt(meta.uuid.as_deref()),
        normalize_key_opt(meta.folder.as_deref()),
        normalize_key_opt(meta.name.as_deref()),
    )
}

fn entry_signature_matches(entry: &NativePakMetaEntry) -> bool {
    let Some((size, modified)) = pak_signature(&entry.path) else {
        return false;
    };
    entry.size == size && entry.modified == modified
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

fn build_native_pak_meta_entries(
    larian_mods_dir: &Path,
    previous: Option<&HashMap<PathBuf, NativePakMetaEntry>>,
) -> HashMap<PathBuf, NativePakMetaEntry> {
    let mut entries = HashMap::new();
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

        let Some((size, modified)) = pak_signature(&path) else {
            continue;
        };

        if let Some(prev) = previous.and_then(|prev| prev.get(&path)) {
            if prev.size == size && prev.modified == modified {
                entries.insert(path.clone(), prev.clone());
                continue;
            }
        }

        let meta = metadata::read_meta_lsx_from_pak(&path);
        let (uuid_key, folder_key, name_key) = meta_keys(meta.as_ref());
        entries.insert(
            path.clone(),
            NativePakMetaEntry {
                path,
                size,
                modified,
                uuid_key,
                folder_key,
                name_key,
            },
        );
    }

    entries
}

fn build_native_pak_meta_index_cached(
    larian_mods_dir: &Path,
    force_refresh: bool,
) -> NativePakMetaIndex {
    let stamp = dir_modified_timestamp(larian_mods_dir);
    let cache = NATIVE_PAK_META_INDEX_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cached_entry: Option<NativePakMetaIndexCacheEntry> = None;

    if let Ok(cache) = cache.lock() {
        cached_entry = cache.get(larian_mods_dir).cloned();
    }

    if !force_refresh {
        if let Some(entry) = cached_entry.as_ref() {
            if entry.modified == stamp {
                return NativePakMetaIndex {
                    entries: entry.entries.values().cloned().collect(),
                };
            }
        }
    }

    let previous = cached_entry.as_ref().map(|entry| &entry.entries);
    let entries = build_native_pak_meta_entries(larian_mods_dir, previous);
    if let Ok(mut cache) = cache.lock() {
        cache.insert(
            larian_mods_dir.to_path_buf(),
            NativePakMetaIndexCacheEntry {
                modified: stamp,
                entries: entries.clone(),
            },
        );
    }
    NativePakMetaIndex {
        entries: entries.into_values().collect(),
    }
}

pub fn build_native_pak_index_cached(larian_mods_dir: &Path) -> Vec<NativePakEntry> {
    let stamp = dir_modified_timestamp(larian_mods_dir);
    let cache = NATIVE_PAK_INDEX_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut cache) = cache.lock() {
        if let Some(entry) = cache.get(larian_mods_dir) {
            if entry.modified == stamp {
                return entry.entries.clone();
            }
        }
        let entries = build_native_pak_index(larian_mods_dir);
        cache.insert(
            larian_mods_dir.to_path_buf(),
            NativePakIndexCacheEntry {
                modified: stamp,
                entries: entries.clone(),
            },
        );
        return entries;
    }
    build_native_pak_index(larian_mods_dir)
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

pub fn resolve_native_pak_path_by_uuid(
    uuid: &str,
    native_pak_index: &[NativePakEntry],
) -> Option<PathBuf> {
    if native_pak_index.is_empty() {
        return None;
    }
    let Some(dir) = native_pak_index.first().and_then(|entry| entry.path.parent()) else {
        return None;
    };
    let uuid_key = normalize_pak_key(uuid);
    if uuid_key.is_empty() {
        return None;
    }

    let mut meta_index = build_native_pak_meta_index_cached(dir, false);
    let mut candidates: Vec<&NativePakMetaEntry> = meta_index
        .entries
        .iter()
        .filter(|entry| entry.uuid_key.as_deref() == Some(uuid_key.as_str()))
        .collect();

    if candidates.is_empty() {
        meta_index = build_native_pak_meta_index_cached(dir, true);
        candidates = meta_index
            .entries
            .iter()
            .filter(|entry| entry.uuid_key.as_deref() == Some(uuid_key.as_str()))
            .collect();
    }

    if candidates.is_empty() {
        return None;
    }

    pick_best_meta_match(&mut candidates, "", "")
}

pub fn resolve_native_pak_path(
    info: &PakInfo,
    native_pak_index: &[NativePakEntry],
) -> Option<PathBuf> {
    if native_pak_index.is_empty() {
        return None;
    }

    if let Some(dir) = native_pak_index
        .first()
        .and_then(|entry| entry.path.parent())
    {
        let meta_index = build_native_pak_meta_index_cached(dir, false);
        if let Some(path) = resolve_native_pak_path_by_meta(info, &meta_index) {
            return Some(path);
        }

        let refreshed = build_native_pak_meta_index_cached(dir, true);
        if let Some(path) = resolve_native_pak_path_by_meta(info, &refreshed) {
            return Some(path);
        }
    }

    resolve_native_pak_path_by_filename(info, native_pak_index)
}

fn resolve_native_pak_path_by_meta(
    info: &PakInfo,
    meta_index: &NativePakMetaIndex,
) -> Option<PathBuf> {
    if meta_index.entries.is_empty() {
        return None;
    }

    let uuid_key = normalize_pak_key(&info.uuid);
    let folder_key = normalize_pak_key(&info.folder);
    let name_key = normalize_pak_key(&info.name);

    if !uuid_key.is_empty() {
        let mut candidates: Vec<&NativePakMetaEntry> = meta_index
            .entries
            .iter()
            .filter(|entry| entry.uuid_key.as_deref() == Some(uuid_key.as_str()))
            .collect();
        if !candidates.is_empty() {
            return pick_best_meta_match(&mut candidates, &folder_key, &name_key);
        }
    }

    if folder_key.is_empty() && name_key.is_empty() {
        return None;
    }

    let mut candidates: Vec<&NativePakMetaEntry> = meta_index
        .entries
        .iter()
        .filter(|entry| {
            entry.folder_key.as_deref() == Some(folder_key.as_str())
                || entry.name_key.as_deref() == Some(name_key.as_str())
        })
        .collect();

    if candidates.is_empty() {
        return None;
    }

    pick_best_meta_match(&mut candidates, &folder_key, &name_key)
}

fn pick_best_meta_match(
    candidates: &mut [&NativePakMetaEntry],
    folder_key: &str,
    name_key: &str,
) -> Option<PathBuf> {
    let mut best: Option<&NativePakMetaEntry> = None;
    let mut best_score = -1i32;
    let mut best_modified = i64::MIN;

    for entry in candidates.iter() {
        if !entry_signature_matches(entry) {
            continue;
        }
        let mut score = 0i32;
        if !folder_key.is_empty() && entry.folder_key.as_deref() == Some(folder_key) {
            score += 2;
        }
        if !name_key.is_empty() && entry.name_key.as_deref() == Some(name_key) {
            score += 1;
        }

        let modified = entry.modified.unwrap_or(0);
        let replace = score > best_score
            || (score == best_score && modified > best_modified)
            || (score == best_score
                && modified == best_modified
                && best
                    .as_ref()
                    .map(|best| entry.path < best.path)
                    .unwrap_or(true));
        if replace {
            best = Some(entry);
            best_score = score;
            best_modified = modified;
        }
    }

    best.map(|entry| entry.path.clone())
}

fn resolve_native_pak_path_by_filename(
    info: &PakInfo,
    native_pak_index: &[NativePakEntry],
) -> Option<PathBuf> {
    let folder_key = normalize_pak_key(&info.folder);
    let folder_base = info.folder.split('_').next().unwrap_or(&info.folder);
    let folder_base_key = normalize_pak_key(folder_base);
    let name_key = normalize_pak_key(&info.name);
    let uuid_key = normalize_pak_key(&info.uuid);
    let uuid_prefix = uuid_key
        .get(0..16)
        .or_else(|| uuid_key.get(0..12))
        .unwrap_or("")
        .to_string();
    let uuid_short = uuid_key.get(0..8).unwrap_or("").to_string();

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
        if !uuid_short.is_empty() {
            if let Some(detail) = match_detail(&entry.normalized, &uuid_short, 55, 40, 25) {
                score = score.saturating_add(detail.score);
                len_diff = len_diff.min(detail.len_diff);
            }
        }
        if let Some(detail) = match_detail(&entry.normalized, &folder_key, 90, 70, 50) {
            score = score.saturating_add(detail.score);
            len_diff = len_diff.min(detail.len_diff);
        }
        if !folder_base_key.is_empty() && folder_base_key != folder_key {
            if let Some(detail) = match_detail(&entry.normalized, &folder_base_key, 75, 55, 35) {
                score = score.saturating_add(detail.score);
                len_diff = len_diff.min(detail.len_diff);
            }
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
