use crate::{
    config::GameConfig,
    game,
    library::{InstallTarget, Library, ModEntry, ProfileEntry, TargetKind},
    metadata, native_pak,
};
use anyhow::{Context, Result};
use lz4_flex::block::decompress;
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    time::Instant,
};
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub struct SmartRankReport {
    pub moved: usize,
    pub missing: usize,
    pub conflicts: usize,
    pub total: usize,
    pub missing_loose: usize,
    pub missing_pak: usize,
    pub scanned_loose: usize,
    pub scanned_pak: usize,
    pub enabled_loose: usize,
    pub enabled_pak: usize,
    pub conflicts_loose: usize,
    pub conflicts_pak: usize,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmartRankGroup {
    Loose,
    Pak,
}

impl SmartRankGroup {
    pub fn label(self) -> &'static str {
        match self {
            SmartRankGroup::Loose => "Loose",
            SmartRankGroup::Pak => "Pak",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SmartRankProgress {
    pub group: SmartRankGroup,
    pub scanned: usize,
    pub total: usize,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct SmartRankResult {
    pub order: Vec<ProfileEntry>,
    pub report: SmartRankReport,
    pub warnings: Vec<String>,
    pub explain: SmartRankExplain,
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
    conflict_partners: usize,
    original_index: usize,
    has_data: bool,
    patch_score: u8,
    patch_reasons: Vec<String>,
    dependencies: Vec<String>,
    date_hint: i64,
}

#[derive(Debug, Clone)]
pub struct SmartRankExplain {
    pub lines: Vec<SmartRankExplainLine>,
}

#[derive(Debug, Clone)]
pub struct SmartRankExplainLine {
    pub kind: ExplainLineKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy)]
pub enum ExplainLineKind {
    Header,
    Item,
    Muted,
}

#[allow(dead_code)]
pub fn smart_rank_profile(config: &GameConfig, library: &Library) -> Result<SmartRankResult> {
    smart_rank_profile_with_progress(config, library, |_| {})
}

pub fn smart_rank_profile_with_progress<F>(
    config: &GameConfig,
    library: &Library,
    mut progress: F,
) -> Result<SmartRankResult>
where
    F: FnMut(SmartRankProgress),
{
    let started = Instant::now();
    let paths = game::detect_paths(
        config.game_id,
        Some(&config.game_root),
        Some(&config.larian_dir),
    )?;
    let profile = library.active_profile().context("active profile not set")?;
    let mod_map = library.index_by_id();
    let mut warnings = Vec::new();
    let native_pak_index = native_pak::build_native_pak_index(&paths.larian_mods_dir);
    let mut items = Vec::new();
    let mut missing = 0usize;
    let mut missing_loose = 0usize;
    let mut missing_pak = 0usize;
    let mut scanned_loose = 0usize;
    let mut scanned_pak = 0usize;
    let mut progress_loose = 0usize;
    let mut progress_pak = 0usize;
    let mut enabled_loose = 0usize;
    let mut enabled_pak = 0usize;

    let mut group_by_id: HashMap<String, RankGroup> = HashMap::new();
    for entry in &profile.order {
        let Some(mod_entry) = mod_map.get(&entry.id) else {
            continue;
        };
        let has_loose = mod_entry
            .targets
            .iter()
            .any(|target| !matches!(target, InstallTarget::Pak { .. }));
        let group = if has_loose {
            RankGroup::Loose
        } else {
            RankGroup::Pak
        };
        group_by_id.insert(entry.id.clone(), group);
        if entry.enabled {
            match group {
                RankGroup::Loose => enabled_loose += 1,
                RankGroup::Pak => enabled_pak += 1,
            }
        }
    }

    for (index, entry) in profile.order.iter().enumerate() {
        let Some(mod_entry) = mod_map.get(&entry.id) else {
            continue;
        };
        let group = *group_by_id.get(&entry.id).unwrap_or(&RankGroup::Pak);

        let mut file_paths = HashSet::new();
        let mut total_bytes = 0u64;
        let mut has_data = false;
        let mut dependencies = Vec::new();
        let mut tags = Vec::new();
        let mut meta_created = None;

        if entry.enabled {
            match group {
                RankGroup::Loose => {}
                RankGroup::Pak => {}
            }
            match scan_mod_files(
                mod_entry,
                config,
                &paths.larian_mods_dir,
                group,
                &native_pak_index,
            ) {
                Ok(files) => {
                    for file in files {
                        total_bytes = total_bytes.saturating_add(file.size);
                        file_paths.insert(file.key);
                    }
                    if file_paths.is_empty() {
                        missing += 1;
                        match group {
                            RankGroup::Loose => missing_loose += 1,
                            RankGroup::Pak => missing_pak += 1,
                        }
                        warnings.push(format!(
                            "Smart rank scan empty for {}",
                            mod_entry.display_name()
                        ));
                    } else {
                        has_data = true;
                        match group {
                            RankGroup::Loose => scanned_loose += 1,
                            RankGroup::Pak => scanned_pak += 1,
                        }
                    }
                }
                Err(err) => {
                    missing += 1;
                    match group {
                        RankGroup::Loose => missing_loose += 1,
                        RankGroup::Pak => missing_pak += 1,
                    }
                    warnings.push(format!(
                        "Smart rank scan failed for {}: {err}",
                        mod_entry.display_name()
                    ));
                }
            }

            if matches!(group, RankGroup::Pak) {
                if let Ok(meta) =
                    read_mod_metadata(mod_entry, config, &paths.larian_mods_dir, &native_pak_index)
                {
                    dependencies = meta.dependencies;
                    tags = meta.tags;
                    meta_created = meta.created_at;
                }
            }
        }

        let (patch_score, patch_notes) = patch_score(mod_entry, &tags);

        if entry.enabled {
            let (progress_scanned, total) = match group {
                RankGroup::Loose => {
                    progress_loose = progress_loose.saturating_add(1);
                    (progress_loose, enabled_loose)
                }
                RankGroup::Pak => {
                    progress_pak = progress_pak.saturating_add(1);
                    (progress_pak, enabled_pak)
                }
            };
            progress(SmartRankProgress {
                group: if matches!(group, RankGroup::Loose) {
                    SmartRankGroup::Loose
                } else {
                    SmartRankGroup::Pak
                },
                scanned: progress_scanned,
                total,
                name: mod_entry.display_name(),
            });
        }

        let file_count = file_paths.len();
        let date_hint = mod_entry
            .created_at
            .or(meta_created)
            .or(mod_entry.modified_at)
            .unwrap_or(mod_entry.added_at);
        items.push(RankItem {
            id: entry.id.clone(),
            enabled: entry.enabled,
            group,
            file_paths,
            file_count,
            total_bytes,
            conflict_files: 0,
            conflict_partners: 0,
            original_index: index,
            has_data,
            patch_score,
            patch_reasons: patch_notes,
            dependencies,
            date_hint,
        });
    }

    let mut conflicts = 0usize;
    let mut conflicts_loose = 0usize;
    let mut conflicts_pak = 0usize;
    let mut top_paths: Vec<ConflictPathInfo> = Vec::new();
    for group in [RankGroup::Loose, RankGroup::Pak] {
        let mut path_counts: HashMap<String, usize> = HashMap::new();
        let mut path_mods: HashMap<String, Vec<String>> = HashMap::new();
        for item in items
            .iter()
            .filter(|item| item.group == group && item.enabled && item.has_data)
        {
            for path in &item.file_paths {
                *path_counts.entry(path.clone()).or_insert(0) += 1;
                path_mods
                    .entry(path.clone())
                    .or_default()
                    .push(item.id.clone());
            }
        }
        let group_conflicts = path_counts.values().filter(|count| **count > 1).count();
        conflicts += group_conflicts;
        match group {
            RankGroup::Loose => conflicts_loose = group_conflicts,
            RankGroup::Pak => conflicts_pak = group_conflicts,
        }

        let mut path_entries: Vec<(String, Vec<String>)> = path_mods
            .iter()
            .filter(|(_, mods)| mods.len() > 1)
            .map(|(path, mods)| (path.clone(), mods.clone()))
            .collect();
        path_entries.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
        for (path, mods) in path_entries.into_iter().take(4) {
            top_paths.push(ConflictPathInfo { group, path, mods });
        }

        for item in items.iter_mut().filter(|item| item.group == group) {
            if !item.enabled || !item.has_data {
                item.conflict_files = 0;
                item.conflict_partners = 0;
                continue;
            }
            let mut conflict_files = 0usize;
            let mut partners = HashSet::new();
            for path in &item.file_paths {
                if path_counts.get(path).copied().unwrap_or(0) > 1 {
                    conflict_files += 1;
                    if let Some(mods) = path_mods.get(path) {
                        for id in mods {
                            if id != &item.id {
                                partners.insert(id.clone());
                            }
                        }
                    }
                }
            }
            item.conflict_files = conflict_files;
            item.conflict_partners = partners.len();
        }
    }

    let loose_order = rank_group_order(&items, RankGroup::Loose, &mod_map, &mut warnings);
    let pak_order = rank_group_order(&items, RankGroup::Pak, &mod_map, &mut warnings);
    let mut new_ids = Vec::new();
    new_ids.extend(loose_order);
    new_ids.extend(pak_order);

    let entry_map: HashMap<String, ProfileEntry> = profile
        .order
        .iter()
        .cloned()
        .map(|entry| (entry.id.clone(), entry))
        .collect();
    let mut new_order = Vec::new();
    for id in &new_ids {
        if let Some(entry) = entry_map.get(id) {
            new_order.push(entry.clone());
        }
    }

    let moved = profile
        .order
        .iter()
        .zip(new_order.iter())
        .filter(|(a, b)| a.id != b.id)
        .count();

    let explain = build_explain_lines(&items, &top_paths, &mod_map, profile);

    Ok(SmartRankResult {
        order: new_order,
        report: SmartRankReport {
            moved,
            missing,
            conflicts,
            total: profile.order.len(),
            missing_loose,
            missing_pak,
            scanned_loose,
            scanned_pak,
            enabled_loose,
            enabled_pak,
            conflicts_loose,
            conflicts_pak,
            elapsed_ms: started.elapsed().as_millis() as u64,
        },
        warnings,
        explain,
    })
}

fn rank_group_order(
    items: &[RankItem],
    group: RankGroup,
    mod_map: &HashMap<String, ModEntry>,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    let mut group_items: Vec<&RankItem> = items.iter().filter(|item| item.group == group).collect();
    group_items.sort_by_key(|item| item.original_index);

    let group_ids: HashSet<String> = group_items.iter().map(|item| item.id.clone()).collect();
    let mut reorder_set: HashSet<String> = group_items
        .iter()
        .filter(|item| {
            (item.enabled && item.has_data && item.conflict_files > 0)
                || !item.dependencies.is_empty()
        })
        .map(|item| item.id.clone())
        .collect();
    for item in &group_items {
        for dep in &item.dependencies {
            if group_ids.contains(dep) {
                reorder_set.insert(dep.clone());
            }
        }
    }

    let mut ranked = topological_rank(group_items.as_slice(), &reorder_set, mod_map, warnings);
    let mut ranked_iter = ranked.drain(..);
    let mut out = Vec::new();
    for item in &group_items {
        if reorder_set.contains(&item.id) {
            if let Some(next) = ranked_iter.next() {
                out.push(next.id.clone());
            } else {
                out.push(item.id.clone());
            }
        } else {
            out.push(item.id.clone());
        }
    }
    out
}

fn topological_rank<'a>(
    items: &'a [&'a RankItem],
    reorder_set: &HashSet<String>,
    mod_map: &HashMap<String, ModEntry>,
    warnings: &mut Vec<String>,
) -> Vec<&'a RankItem> {
    let mut indegree: HashMap<String, usize> = HashMap::new();
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();

    for item in items {
        if !reorder_set.contains(&item.id) {
            continue;
        }
        indegree.entry(item.id.clone()).or_insert(0);
    }

    for item in items.iter().filter(|item| reorder_set.contains(&item.id)) {
        for dep in &item.dependencies {
            if !reorder_set.contains(dep) {
                if mod_map.contains_key(dep) {
                    warnings.push(format!(
                        "Smart rank dependency skipped for {}: {} in different group",
                        display_mod_name(&item.id, mod_map),
                        display_mod_name(dep, mod_map)
                    ));
                }
                continue;
            }
            edges.entry(dep.clone()).or_default().push(item.id.clone());
            *indegree.entry(item.id.clone()).or_insert(0) += 1;
        }
    }

    let mut available: Vec<&RankItem> = items
        .iter()
        .filter(|item| reorder_set.contains(&item.id))
        .filter(|item| indegree.get(&item.id).copied().unwrap_or(0) == 0)
        .copied()
        .collect();

    let mut result = Vec::new();
    let mut remaining = reorder_set.len();
    while !available.is_empty() {
        available.sort_by(|a, b| compare_rank_items(a, b));
        let next = available.remove(0);
        result.push(next);
        remaining = remaining.saturating_sub(1);

        if let Some(children) = edges.get(&next.id) {
            for child in children {
                let entry = indegree.entry(child.clone()).or_insert(0);
                if *entry > 0 {
                    *entry -= 1;
                }
                if *entry == 0 {
                    if let Some(item) = items.iter().find(|item| item.id == *child) {
                        available.push(*item);
                    }
                }
            }
        }
    }

    if remaining > 0 {
        warnings
            .push("Smart rank dependency cycle detected; falling back to score order".to_string());
        let mut fallback: Vec<&RankItem> = items
            .iter()
            .filter(|item| reorder_set.contains(&item.id))
            .copied()
            .collect();
        fallback.sort_by(|a, b| compare_rank_items(a, b));
        return fallback;
    }

    result
}

fn compare_rank_items(a: &RankItem, b: &RankItem) -> std::cmp::Ordering {
    let partners = b.conflict_partners.cmp(&a.conflict_partners);
    if partners != std::cmp::Ordering::Equal {
        return partners;
    }
    let conflicts = b.conflict_files.cmp(&a.conflict_files);
    if conflicts != std::cmp::Ordering::Equal {
        return conflicts;
    }
    let patch = a.patch_score.cmp(&b.patch_score);
    if patch != std::cmp::Ordering::Equal {
        return patch;
    }
    let size = b.total_bytes.cmp(&a.total_bytes);
    if size != std::cmp::Ordering::Equal {
        return size;
    }
    let count = b.file_count.cmp(&a.file_count);
    if count != std::cmp::Ordering::Equal {
        return count;
    }
    let date = a.date_hint.cmp(&b.date_hint);
    if date != std::cmp::Ordering::Equal {
        return date;
    }
    a.original_index.cmp(&b.original_index)
}

fn scan_mod_files(
    mod_entry: &ModEntry,
    config: &GameConfig,
    larian_mods_dir: &Path,
    group: RankGroup,
    native_pak_index: &[native_pak::NativePakEntry],
) -> Result<Vec<FileEntry>> {
    let data_dir = &config.data_dir;
    match group {
        RankGroup::Pak => scan_pak_files(mod_entry, data_dir, larian_mods_dir, native_pak_index),
        RankGroup::Loose => scan_loose_files(mod_entry, data_dir),
    }
}

fn read_mod_metadata(
    mod_entry: &ModEntry,
    config: &GameConfig,
    larian_mods_dir: &Path,
    native_pak_index: &[native_pak::NativePakEntry],
) -> Result<metadata::ModMeta> {
    let data_dir = &config.data_dir;
    let pak_paths = collect_pak_paths(mod_entry, data_dir, larian_mods_dir, native_pak_index);
    let mut merged = metadata::ModMeta::default();
    for pak_path in pak_paths {
        if !pak_path.exists() {
            continue;
        }
        if let Some(meta) = metadata::read_meta_lsx_from_pak(&pak_path) {
            merged.dependencies.extend(meta.dependencies);
            merged.tags.extend(meta.tags);
            if let Some(created_at) = meta.created_at {
                merged.created_at = Some(match merged.created_at {
                    Some(existing) => existing.min(created_at),
                    None => created_at,
                });
            }
        }
    }
    Ok(merged)
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
        for entry in WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| !is_ignored_mod_path(entry.path()))
        {
            let entry = entry?;
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
    native_pak_index: &[native_pak::NativePakEntry],
) -> Result<Vec<FileEntry>> {
    let pak_paths = collect_pak_paths(mod_entry, data_dir, larian_mods_dir, native_pak_index);

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

fn collect_pak_paths(
    mod_entry: &ModEntry,
    data_dir: &Path,
    larian_mods_dir: &Path,
    native_pak_index: &[native_pak::NativePakEntry],
) -> Vec<std::path::PathBuf> {
    let mut pak_paths = Vec::new();

    for target in &mod_entry.targets {
        if let InstallTarget::Pak { file, info } = target {
            if mod_entry.is_native() {
                let by_folder = larian_mods_dir.join(format!("{}.pak", info.folder));
                if by_folder.exists() {
                    pak_paths.push(by_folder);
                }
                let by_file = larian_mods_dir.join(file);
                if by_file.exists() && !pak_paths.iter().any(|p| p == &by_file) {
                    pak_paths.push(by_file);
                }
            } else {
                pak_paths.push(data_dir.join("mods").join(&mod_entry.id).join(file));
            }
        }
    }

    if pak_paths.is_empty() && mod_entry.is_native() {
        if let Some(info) = mod_entry.targets.iter().find_map(|target| match target {
            InstallTarget::Pak { info, .. } => Some(info),
            _ => None,
        }) {
            if let Some(path) = native_pak::resolve_native_pak_path(info, native_pak_index) {
                pak_paths.push(path);
            }
        }
    }

    pak_paths
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
    file.seek(SeekFrom::Start(0))?;
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

fn patch_score(mod_entry: &ModEntry, tags: &[String]) -> (u8, Vec<String>) {
    let mut score = 0u8;
    let mut reasons = Vec::new();
    let label = mod_entry.display_name().to_ascii_lowercase();
    for (keyword, weight) in patch_keywords() {
        if label.contains(keyword) {
            score = score.saturating_add(weight);
            reasons.push(format!("name:{keyword}"));
        }
    }
    for tag in tags {
        let tag_lower = tag.to_ascii_lowercase();
        for (keyword, weight) in patch_keywords() {
            if tag_lower.contains(keyword) {
                score = score.saturating_add(weight);
                reasons.push(format!("tag:{tag}"));
                break;
            }
        }
    }
    (score, reasons)
}

fn patch_keywords() -> Vec<(&'static str, u8)> {
    vec![
        ("patch", 2),
        ("hotfix", 2),
        ("fix", 1),
        ("compat", 2),
        ("compatibility", 2),
        ("override", 1),
        ("addon", 1),
        ("add-on", 1),
        ("addon", 1),
    ]
}

struct ConflictPathInfo {
    group: RankGroup,
    path: String,
    mods: Vec<String>,
}

fn build_explain_lines(
    items: &[RankItem],
    paths: &[ConflictPathInfo],
    mod_map: &HashMap<String, ModEntry>,
    profile: &crate::library::Profile,
) -> SmartRankExplain {
    let mut lines = Vec::new();
    let mut ordered_ids = HashMap::new();
    for (index, entry) in profile.order.iter().enumerate() {
        ordered_ids.insert(entry.id.clone(), index);
    }

    lines.push(SmartRankExplainLine {
        kind: ExplainLineKind::Header,
        text: "Top conflicts".to_string(),
    });
    let mut conflict_items: Vec<&RankItem> = items
        .iter()
        .filter(|item| item.conflict_files > 0)
        .collect();
    conflict_items.sort_by(|a, b| b.conflict_files.cmp(&a.conflict_files));
    if conflict_items.is_empty() {
        lines.push(SmartRankExplainLine {
            kind: ExplainLineKind::Muted,
            text: "No overlapping files detected.".to_string(),
        });
    } else {
        for item in conflict_items.into_iter().take(6) {
            lines.push(SmartRankExplainLine {
                kind: ExplainLineKind::Item,
                text: format!(
                    "{} — {} files, {} mods",
                    display_mod_name(&item.id, mod_map),
                    item.conflict_files,
                    item.conflict_partners
                ),
            });
        }
    }

    lines.push(SmartRankExplainLine {
        kind: ExplainLineKind::Header,
        text: "Top conflict paths".to_string(),
    });
    if paths.is_empty() {
        lines.push(SmartRankExplainLine {
            kind: ExplainLineKind::Muted,
            text: "No conflict paths recorded.".to_string(),
        });
    } else {
        for info in paths.iter().take(6) {
            let winner = info
                .mods
                .iter()
                .max_by_key(|id| ordered_ids.get(*id).copied().unwrap_or(0))
                .map(|id| display_mod_name(id, mod_map))
                .unwrap_or_else(|| "Unknown".to_string());
            let group_label = match info.group {
                RankGroup::Loose => "Loose",
                RankGroup::Pak => "Pak",
            };
            let mods_label = info
                .mods
                .iter()
                .map(|id| display_mod_name(id, mod_map))
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(SmartRankExplainLine {
                kind: ExplainLineKind::Item,
                text: format!(
                    "[{group_label}] {} — {} (winner: {winner})",
                    info.path, mods_label
                ),
            });
        }
    }

    lines.push(SmartRankExplainLine {
        kind: ExplainLineKind::Header,
        text: "Dependencies".to_string(),
    });
    let mut dep_lines = Vec::new();
    for item in items.iter().filter(|item| !item.dependencies.is_empty()) {
        for dep in &item.dependencies {
            dep_lines.push(format!(
                "{} → {}",
                display_mod_name(&item.id, mod_map),
                display_mod_name(dep, mod_map)
            ));
        }
    }
    if dep_lines.is_empty() {
        lines.push(SmartRankExplainLine {
            kind: ExplainLineKind::Muted,
            text: "No dependencies detected.".to_string(),
        });
    } else {
        for line in dep_lines.into_iter().take(6) {
            lines.push(SmartRankExplainLine {
                kind: ExplainLineKind::Item,
                text: line,
            });
        }
    }

    lines.push(SmartRankExplainLine {
        kind: ExplainLineKind::Header,
        text: "Patch heuristic".to_string(),
    });
    let mut patch_items: Vec<&RankItem> =
        items.iter().filter(|item| item.patch_score > 0).collect();
    patch_items.sort_by(|a, b| b.patch_score.cmp(&a.patch_score));
    if patch_items.is_empty() {
        lines.push(SmartRankExplainLine {
            kind: ExplainLineKind::Muted,
            text: "No patch/compat hints found.".to_string(),
        });
    } else {
        for item in patch_items.into_iter().take(6) {
            let reasons = if item.patch_reasons.is_empty() {
                String::new()
            } else {
                format!(" ({})", item.patch_reasons.join(", "))
            };
            lines.push(SmartRankExplainLine {
                kind: ExplainLineKind::Item,
                text: format!(
                    "{} — score {}{}",
                    display_mod_name(&item.id, mod_map),
                    item.patch_score,
                    reasons
                ),
            });
        }
    }

    SmartRankExplain { lines }
}

fn display_mod_name(id: &str, mod_map: &HashMap<String, ModEntry>) -> String {
    mod_map
        .get(id)
        .map(|entry| entry.display_name())
        .unwrap_or_else(|| id.to_string())
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

fn is_ignored_mod_path(path: &Path) -> bool {
    path.components().any(|component| {
        let part = component.as_os_str().to_string_lossy();
        part.eq_ignore_ascii_case("__MACOSX")
            || part == ".git"
            || part == ".svn"
            || part == ".vscode"
    })
}

fn target_kind_label(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Pak => "pak",
        TargetKind::Generated => "generated",
        TargetKind::Data => "data",
        TargetKind::Bin => "bin",
    }
}
