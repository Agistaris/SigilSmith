use crate::{
    config::GameConfig,
    game,
    library::{InstallTarget, Library, ModEntry, ProfileEntry, TargetKind},
};
use anyhow::{Context, Result};
use lz4_flex::block::decompress;
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub struct SmartRankReport {
    pub moved: usize,
    pub missing: usize,
    pub conflicts: usize,
    pub total: usize,
}

#[derive(Debug, Clone)]
pub struct SmartRankResult {
    pub order: Vec<ProfileEntry>,
    pub report: SmartRankReport,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RankGroup {
    Loose,
    Pak,
}

#[derive(Debug, Clone)]
struct FileEntry {
    key: String,
    size: u64,
}

#[derive(Debug, Clone)]
struct RankItem {
    id: String,
    enabled: bool,
    group: RankGroup,
    file_paths: HashSet<String>,
    file_count: usize,
    total_bytes: u64,
    conflict_files: usize,
    original_index: usize,
    has_data: bool,
}

pub fn smart_rank_profile(config: &GameConfig, library: &Library) -> Result<SmartRankResult> {
    let paths = game::detect_paths(
        config.game_id,
        Some(&config.game_root),
        Some(&config.larian_dir),
    )?;
    let profile = library
        .active_profile()
        .context("active profile not set")?;
    let mod_map = library.index_by_id();
    let mut warnings = Vec::new();
    let mut items = Vec::new();
    let mut missing = 0usize;

    for (index, entry) in profile.order.iter().enumerate() {
        let Some(mod_entry) = mod_map.get(&entry.id) else {
            continue;
        };
        let has_loose = mod_entry
            .targets
            .iter()
            .any(|target| !matches!(target, InstallTarget::Pak { .. }));
        let group = if has_loose { RankGroup::Loose } else { RankGroup::Pak };

        let mut file_paths = HashSet::new();
        let mut total_bytes = 0u64;
        let mut has_data = false;

        if entry.enabled {
            match scan_mod_files(
                mod_entry,
                config,
                &paths.larian_mods_dir,
                group,
            ) {
                Ok(files) => {
                    for file in files {
                        total_bytes = total_bytes.saturating_add(file.size);
                        file_paths.insert(file.key);
                    }
                    if file_paths.is_empty() {
                        missing += 1;
                        warnings.push(format!(
                            "Smart rank scan empty for {}",
                            mod_entry.display_name()
                        ));
                    } else {
                        has_data = true;
                    }
                }
                Err(err) => {
                    missing += 1;
                    warnings.push(format!(
                        "Smart rank scan failed for {}: {err}",
                        mod_entry.display_name()
                    ));
                }
            }
        }

        let file_count = file_paths.len();
        items.push(RankItem {
            id: entry.id.clone(),
            enabled: entry.enabled,
            group,
            file_paths,
            file_count,
            total_bytes,
            conflict_files: 0,
            original_index: index,
            has_data,
        });
    }

    let mut conflicts = 0usize;
    for group in [RankGroup::Loose, RankGroup::Pak] {
        let mut path_counts: HashMap<String, usize> = HashMap::new();
        for item in items.iter().filter(|item| {
            item.group == group && item.enabled && item.has_data
        }) {
            for path in &item.file_paths {
                *path_counts.entry(path.clone()).or_insert(0) += 1;
            }
        }
        conflicts += path_counts.values().filter(|count| **count > 1).count();

        for item in items.iter_mut().filter(|item| item.group == group) {
            if !item.enabled || !item.has_data {
                item.conflict_files = 0;
                continue;
            }
            let mut conflict_files = 0usize;
            for path in &item.file_paths {
                if path_counts.get(path).copied().unwrap_or(0) > 1 {
                    conflict_files += 1;
                }
            }
            item.conflict_files = conflict_files;
        }
    }

    items.sort_by(|a, b| {
        let group = (a.group as u8).cmp(&(b.group as u8));
        if group != std::cmp::Ordering::Equal {
            return group;
        }
        let enabled = b.enabled.cmp(&a.enabled);
        if enabled != std::cmp::Ordering::Equal {
            return enabled;
        }
        let data = b.has_data.cmp(&a.has_data);
        if data != std::cmp::Ordering::Equal {
            return data;
        }
        let a_conflicts = a.conflict_files > 0;
        let b_conflicts = b.conflict_files > 0;
        let conflict_group = b_conflicts.cmp(&a_conflicts);
        if conflict_group != std::cmp::Ordering::Equal {
            return conflict_group;
        }
        let size = b.total_bytes.cmp(&a.total_bytes);
        if size != std::cmp::Ordering::Equal {
            return size;
        }
        let count = b.file_count.cmp(&a.file_count);
        if count != std::cmp::Ordering::Equal {
            return count;
        }
        a.original_index.cmp(&b.original_index)
    });

    let entry_map: HashMap<String, ProfileEntry> = profile
        .order
        .iter()
        .cloned()
        .map(|entry| (entry.id.clone(), entry))
        .collect();
    let mut new_order = Vec::new();
    for item in items {
        if let Some(entry) = entry_map.get(&item.id) {
            new_order.push(entry.clone());
        }
    }

    let moved = profile
        .order
        .iter()
        .zip(new_order.iter())
        .filter(|(a, b)| a.id != b.id)
        .count();

    Ok(SmartRankResult {
        order: new_order,
        report: SmartRankReport {
            moved,
            missing,
            conflicts,
            total: profile.order.len(),
        },
        warnings,
    })
}

fn scan_mod_files(
    mod_entry: &ModEntry,
    config: &GameConfig,
    larian_mods_dir: &Path,
    group: RankGroup,
) -> Result<Vec<FileEntry>> {
    let data_dir = &config.data_dir;
    match group {
        RankGroup::Pak => scan_pak_files(mod_entry, data_dir, larian_mods_dir),
        RankGroup::Loose => scan_loose_files(mod_entry, data_dir),
    }
}

fn scan_loose_files(mod_entry: &ModEntry, data_dir: &Path) -> Result<Vec<FileEntry>> {
    let mod_root = data_dir.join("mods").join(&mod_entry.id);
    let mut files = Vec::new();

    for target in &mod_entry.targets {
        let (dir, kind) = match target {
            InstallTarget::Data { dir } => (dir.as_str(), TargetKind::Data),
            InstallTarget::Generated { dir } => (dir.as_str(), TargetKind::Generated),
            InstallTarget::Bin { dir } => (dir.as_str(), TargetKind::Bin),
            InstallTarget::Pak { .. } => continue,
        };
        let root = mod_root.join(dir);
        if !root.exists() {
            continue;
        }
        let prefix = format!("{}:", target_kind_label(kind));
        for entry in WalkDir::new(&root).into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let rel = path.strip_prefix(&root).unwrap_or(path);
            let rel = normalize_path(&rel.to_string_lossy());
            let key = format!("{prefix}{rel}");
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            files.push(FileEntry { key, size });
        }
    }

    Ok(files)
}

fn scan_pak_files(
    mod_entry: &ModEntry,
    data_dir: &Path,
    larian_mods_dir: &Path,
) -> Result<Vec<FileEntry>> {
    let mut pak_paths = Vec::new();

    for target in &mod_entry.targets {
        if let InstallTarget::Pak { file, info } = target {
            if mod_entry.is_native() {
                let filename = format!("{}.pak", info.folder);
                pak_paths.push(larian_mods_dir.join(filename));
            } else {
                pak_paths.push(data_dir.join("mods").join(&mod_entry.id).join(file));
            }
        }
    }

    let mut files = Vec::new();
    let mut found = false;
    for pak_path in pak_paths {
        if !pak_path.exists() {
            continue;
        }
        found = true;
        let mut pak_files = scan_pak_index(&pak_path)?;
        files.append(&mut pak_files);
    }
    if !found {
        anyhow::bail!("pak file missing");
    }

    Ok(files)
}

fn scan_pak_index(path: &Path) -> Result<Vec<FileEntry>> {
    const ENTRY_LEN: usize = 272;
    const PATH_LEN: usize = 256;
    const MIN_VERSION: u32 = 18;

    let mut file = File::open(path).with_context(|| format!("open pak {:?}", path))?;
    let mut id = [0u8; 4];
    file.read_exact(&mut id)?;
    if &id != b"LSPK" {
        anyhow::bail!("invalid pak header");
    }
    let version = read_u32(&mut file)?;
    if version < MIN_VERSION {
        anyhow::bail!("unsupported pak version {version}");
    }
    let footer_offset = read_u64(&mut file)?;
    let footer_offset = i64::try_from(footer_offset)?;
    file.seek(SeekFrom::Current(footer_offset))?;

    let file_count = read_u32(&mut file)? as usize;
    let compressed_len = read_u32(&mut file)? as usize;
    let decompressed_len = file_count.saturating_mul(ENTRY_LEN);

    let mut compressed = vec![0u8; compressed_len];
    file.read_exact(&mut compressed)?;
    let table = decompress(&compressed, decompressed_len)?;

    let mut out = Vec::new();
    for index in 0..file_count {
        let start = index * ENTRY_LEN;
        let end = start + ENTRY_LEN;
        if end > table.len() {
            break;
        }
        let entry = &table[start..end];
        let path_end = entry[..PATH_LEN]
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(PATH_LEN);
        let raw_path = String::from_utf8_lossy(&entry[..path_end]);
        let path = normalize_path(&raw_path);
        let size = u32::from_le_bytes(entry[268..272].try_into().unwrap_or([0; 4])) as u64;
        out.push(FileEntry { key: path, size });
    }

    Ok(out)
}

fn read_u32(file: &mut File) -> Result<u32> {
    let mut bytes = [0u8; 4];
    file.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(file: &mut File) -> Result<u64> {
    let mut bytes = [0u8; 8];
    file.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches('/')
        .to_ascii_lowercase()
}

fn target_kind_label(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Pak => "pak",
        TargetKind::Generated => "generated",
        TargetKind::Data => "data",
        TargetKind::Bin => "bin",
    }
}
