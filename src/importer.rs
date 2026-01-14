use crate::library::{
    clean_source_label, normalize_times, path_times, resolve_times, InstallTarget, ModEntry,
    ModSource, PakInfo, TargetKind,
};
use crate::metadata;
use crate::sigillink::{SigilLinkEntry, SigilLinkIndex, SIGILLINK_VERSION};
use anyhow::{Context, Result};
use blake3::Hasher;
use filetime::{set_file_mtime, FileTime};
use larian_formats::lspk;
use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use time::{Date, Month, PrimitiveDateTime, Time as TimeOfDay};
use walkdir::WalkDir;

pub struct ImportResult {
    pub batches: Vec<ImportBatch>,
    pub unrecognized: bool,
    pub failures: Vec<ImportFailure>,
}

#[derive(Clone, Copy)]
struct SourceTimes {
    created_at: Option<i64>,
    modified_at: Option<i64>,
}

struct DirImportResult {
    mods: Vec<ImportMod>,
    unrecognized: bool,
}

const NESTED_ARCHIVE_SCAN_DEPTH: usize = 4;

#[derive(Clone, Copy)]
enum CandidateKind {
    Directory,
    PakFile,
    ArchiveFile,
}

struct ImportCandidate {
    path: PathBuf,
    label: String,
    kind: CandidateKind,
}

struct ProgressReporter {
    label: String,
    unit_index: usize,
    unit_count: usize,
    stage_count: usize,
    callback: Option<ProgressCallback>,
}

impl ProgressReporter {
    fn report(
        &self,
        stage: ImportStage,
        stage_current: usize,
        stage_total: usize,
        detail: Option<String>,
    ) {
        let Some(callback) = &self.callback else {
            return;
        };
        let stage_total = stage_total.max(1);
        let stage_current = stage_current.min(stage_total);
        let stage_fraction = (stage_current as f32) / (stage_total as f32);
        let stage_index = stage.index() as f32;
        let stage_count = self.stage_count as f32;
        let overall_progress = (stage_index + stage_fraction) / stage_count;
        callback(ImportProgress {
            label: self.label.clone(),
            unit_index: self.unit_index + 1,
            unit_count: self.unit_count,
            stage,
            stage_current,
            stage_total,
            overall_progress: overall_progress.clamp(0.0, 1.0),
            detail,
        });
    }
}

struct CopyProgress<'a> {
    reporter: Option<&'a ProgressReporter>,
    copied: usize,
    total: usize,
    stage: ImportStage,
    offset: usize,
    last_report: Instant,
}

impl<'a> CopyProgress<'a> {
    fn new(reporter: Option<&'a ProgressReporter>, total: usize, stage: ImportStage) -> Self {
        Self {
            reporter,
            copied: 0,
            total: total.max(1),
            stage,
            offset: 0,
            last_report: Instant::now(),
        }
    }

    fn new_with_offset(
        reporter: Option<&'a ProgressReporter>,
        total: usize,
        stage: ImportStage,
        offset: usize,
    ) -> Self {
        Self {
            reporter,
            copied: 0,
            total: total.max(1),
            stage,
            offset,
            last_report: Instant::now(),
        }
    }

    fn bump(&mut self, detail: Option<String>, force: bool) {
        self.copied = self.copied.saturating_add(1);
        let should_report =
            force || self.copied % 50 == 0 || self.last_report.elapsed().as_millis() >= 120;
        if should_report {
            if let Some(reporter) = self.reporter {
                let current = self.copied.saturating_add(self.offset).min(self.total);
                reporter.report(self.stage, current, self.total, detail);
            }
            self.last_report = Instant::now();
        }
    }

    fn advance(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        self.copied = self.copied.saturating_add(count);
        if let Some(reporter) = self.reporter {
            let current = self.copied.saturating_add(self.offset).min(self.total);
            reporter.report(self.stage, current, self.total, None);
        }
        self.last_report = Instant::now();
    }

    fn finish(&mut self) {
        if let Some(reporter) = self.reporter {
            reporter.report(self.stage, self.total, self.total, None);
        }
    }
}

struct StagingGuard {
    path: PathBuf,
    armed: bool,
}

impl StagingGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImportSource {
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct ImportBatch {
    pub source: ImportSource,
    pub mods: Vec<ImportMod>,
}

#[derive(Debug, Clone)]
pub struct ImportFailure {
    pub source: ImportSource,
    pub error: String,
}

#[derive(Debug, Clone)]
pub struct ImportMod {
    pub entry: ModEntry,
    pub staging_root: Option<PathBuf>,
    pub sigillink: Option<SigilLinkIndex>,
}

impl ImportMod {
    pub fn cleanup_staging(&self) {
        let Some(staging_root) = &self.staging_root else {
            return;
        };
        let _ = fs::remove_dir_all(staging_root);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportStage {
    Extracting,
    Indexing,
    Installing,
    Linking,
    Finalizing,
}

impl ImportStage {
    fn index(self) -> usize {
        match self {
            ImportStage::Extracting => 0,
            ImportStage::Indexing => 1,
            ImportStage::Installing => 2,
            ImportStage::Linking => 3,
            ImportStage::Finalizing => 4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ImportStage::Extracting => "Extracting",
            ImportStage::Indexing => "Indexing",
            ImportStage::Installing => "Installing",
            ImportStage::Linking => "Linking (SigiLink Cache)",
            ImportStage::Finalizing => "Finalizing",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImportProgress {
    pub label: String,
    pub unit_index: usize,
    pub unit_count: usize,
    pub stage: ImportStage,
    pub stage_current: usize,
    pub stage_total: usize,
    pub overall_progress: f32,
    pub detail: Option<String>,
}

pub type ProgressCallback = Arc<dyn Fn(ImportProgress) + Send + Sync>;

fn source_times_for(path: &Path) -> SourceTimes {
    let (created_at, modified_at) = path_times(path);
    SourceTimes {
        created_at,
        modified_at,
    }
}

fn scan_dir_times(path: &Path) -> SourceTimes {
    let mut created_at: Option<i64> = None;
    let mut modified_at: Option<i64> = None;
    for entry in WalkDir::new(path) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        let created_value = meta
            .created()
            .ok()
            .and_then(|created| created.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs() as i64);
        let modified_value = meta
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs() as i64);
        if let Some(value) = created_value.or(modified_value) {
            created_at = Some(created_at.map_or(value, |current| current.min(value)));
        }
        if let Some(value) = modified_value {
            modified_at = Some(modified_at.map_or(value, |current| current.max(value)));
        }
    }
    let (created_at, modified_at) = normalize_times(created_at, modified_at);
    SourceTimes {
        created_at,
        modified_at,
    }
}

fn scan_payload_times(scan: &PayloadScan) -> SourceTimes {
    let mut created_at: Option<i64> = None;
    let mut modified_at: Option<i64> = None;
    let mut merge = |times: SourceTimes| {
        if let Some(value) = times.created_at {
            created_at = Some(created_at.map_or(value, |current| current.min(value)));
        }
        if let Some(value) = times.modified_at {
            modified_at = Some(modified_at.map_or(value, |current| current.max(value)));
        }
    };
    if let Some(dir) = &scan.data_dir {
        merge(scan_dir_times(dir));
    }
    if let Some(dir) = &scan.generated_dir {
        merge(scan_dir_times(dir));
    }
    if let Some(dir) = &scan.public_dir {
        merge(scan_dir_times(dir));
    }
    if let Some(dir) = &scan.bin_dir {
        merge(scan_dir_times(dir));
    }
    let (created_at, modified_at) = normalize_times(created_at, modified_at);
    SourceTimes {
        created_at,
        modified_at,
    }
}

pub fn import_path_with_progress(
    path: &Path,
    data_dir: &Path,
    progress: Option<ProgressCallback>,
) -> Result<ImportResult> {
    if !path.exists() {
        return Ok(ImportResult {
            batches: Vec::new(),
            unrecognized: false,
            failures: Vec::new(),
        });
    }

    let result = if path.is_dir() {
        import_batch_from_dir(path, data_dir, None, false, None, progress)?
    } else {
        let source_label = source_label_for_archive(path);
        match path.extension().and_then(|ext| ext.to_str()).unwrap_or("") {
            "pak" | "PAK" => {
                let label = source_label
                    .clone()
                    .unwrap_or_else(|| display_path_label(path));
                let reporter = ProgressReporter {
                    label: label.clone(),
                    unit_index: 0,
                    unit_count: 1,
                    stage_count: 5,
                    callback: progress.clone(),
                };
                let mods =
                    import_pak_file(path, data_dir, source_label.as_deref(), Some(&reporter))?;
                ImportResult {
                    batches: vec![ImportBatch {
                        source: ImportSource { label },
                        mods,
                    }],
                    unrecognized: false,
                    failures: Vec::new(),
                }
            }
            "zip" | "ZIP" => import_archive_zip(path, data_dir, source_label.as_deref(), progress)?,
            "7z" | "7Z" | "rar" | "RAR" => {
                import_archive_7z(path, data_dir, source_label.as_deref(), progress)?
            }
            _ => ImportResult {
                batches: Vec::new(),
                unrecognized: true,
                failures: Vec::new(),
            },
        }
    };

    Ok(result)
}

fn import_archive_zip(
    path: &Path,
    data_dir: &Path,
    source_label: Option<&str>,
    progress: Option<ProgressCallback>,
) -> Result<ImportResult> {
    let temp_dir = make_temp_dir(data_dir, "zip")?;
    let source_times = source_times_for(path);
    let label = source_label
        .map(|label| label.to_string())
        .unwrap_or_else(|| display_path_label(path));
    let reporter = ProgressReporter {
        label,
        unit_index: 0,
        unit_count: 1,
        stage_count: 5,
        callback: progress.clone(),
    };
    reporter.report(ImportStage::Extracting, 0, 1, None);
    if let Err(err) = extract_zip(path, &temp_dir) {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(err);
    }
    reporter.report(ImportStage::Extracting, 1, 1, None);
    let result = import_batch_from_dir(
        &temp_dir,
        data_dir,
        source_label,
        true,
        Some(source_times),
        progress,
    );
    let _ = fs::remove_dir_all(&temp_dir);
    result
}

fn import_archive_7z(
    path: &Path,
    data_dir: &Path,
    source_label: Option<&str>,
    progress: Option<ProgressCallback>,
) -> Result<ImportResult> {
    let temp_dir = make_temp_dir(data_dir, "7z")?;
    let source_times = source_times_for(path);
    let label = source_label
        .map(|label| label.to_string())
        .unwrap_or_else(|| display_path_label(path));
    let reporter = ProgressReporter {
        label,
        unit_index: 0,
        unit_count: 1,
        stage_count: 5,
        callback: progress.clone(),
    };
    reporter.report(ImportStage::Extracting, 0, 1, None);
    if let Err(err) = extract_7z(path, &temp_dir) {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(err);
    }
    reporter.report(ImportStage::Extracting, 1, 1, None);
    let result = import_batch_from_dir(
        &temp_dir,
        data_dir,
        source_label,
        true,
        Some(source_times),
        progress,
    );
    let _ = fs::remove_dir_all(&temp_dir);
    result
}

fn import_batch_from_dir(
    path: &Path,
    data_dir: &Path,
    source_label: Option<&str>,
    allow_move: bool,
    source_times: Option<SourceTimes>,
    progress: Option<ProgressCallback>,
) -> Result<ImportResult> {
    let mut candidates = collect_import_candidates(path)?;
    if candidates.is_empty() {
        candidates.push(ImportCandidate {
            path: path.to_path_buf(),
            label: display_path_label(path),
            kind: CandidateKind::Directory,
        });
    }

    let unit_count = candidates.len();
    let multi = unit_count > 1;
    let mut batches = Vec::new();
    let mut failures = Vec::new();
    let mut unrecognized = false;

    for (index, candidate) in candidates.into_iter().enumerate() {
        let candidate_label = candidate.label.clone();
        let display_label = match source_label {
            Some(root_label) if multi => format!("{root_label} -> {candidate_label}"),
            Some(root_label) => root_label.to_string(),
            None => candidate_label.clone(),
        };
        let candidate_source_label = if multi {
            Some(candidate_label.as_str())
        } else {
            source_label
        };
        let reporter = ProgressReporter {
            label: display_label.clone(),
            unit_index: index,
            unit_count,
            stage_count: 5,
            callback: progress.clone(),
        };

        match candidate.kind {
            CandidateKind::PakFile => {
                let mods = match import_pak_file(
                    &candidate.path,
                    data_dir,
                    candidate_source_label,
                    Some(&reporter),
                ) {
                    Ok(mods) => mods,
                    Err(err) => {
                        failures.push(ImportFailure {
                            source: ImportSource {
                                label: display_label,
                            },
                            error: err.to_string(),
                        });
                        continue;
                    }
                };
                if mods.is_empty() {
                    failures.push(ImportFailure {
                        source: ImportSource {
                            label: display_label,
                        },
                        error: "No mods found".to_string(),
                    });
                    continue;
                }
                batches.push(ImportBatch {
                    source: ImportSource {
                        label: display_label,
                    },
                    mods,
                });
            }
            CandidateKind::ArchiveFile => {
                let source_label = candidate_source_label;
                let result = match candidate
                    .path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .unwrap_or("")
                {
                    "zip" | "ZIP" => import_archive_zip(
                        &candidate.path,
                        data_dir,
                        source_label,
                        progress.clone(),
                    ),
                    "7z" | "7Z" | "rar" | "RAR" => {
                        import_archive_7z(&candidate.path, data_dir, source_label, progress.clone())
                    }
                    _ => Ok(ImportResult {
                        batches: Vec::new(),
                        unrecognized: true,
                        failures: Vec::new(),
                    }),
                };

                match result {
                    Ok(mut result) => {
                        if result.unrecognized && result.batches.is_empty() {
                            failures.push(ImportFailure {
                                source: ImportSource {
                                    label: display_label,
                                },
                                error: "Unrecognized archive layout".to_string(),
                            });
                            continue;
                        }
                        failures.append(&mut result.failures);
                        batches.append(&mut result.batches);
                    }
                    Err(err) => {
                        failures.push(ImportFailure {
                            source: ImportSource {
                                label: display_label,
                            },
                            error: err.to_string(),
                        });
                    }
                }
            }
            CandidateKind::Directory => {
                let result = match import_from_dir(
                    &candidate.path,
                    data_dir,
                    candidate_source_label,
                    allow_move,
                    source_times,
                    Some(&reporter),
                ) {
                    Ok(result) => result,
                    Err(err) => {
                        failures.push(ImportFailure {
                            source: ImportSource {
                                label: display_label,
                            },
                            error: err.to_string(),
                        });
                        continue;
                    }
                };
                if result.unrecognized && unit_count == 1 {
                    unrecognized = true;
                }
                if result.mods.is_empty() {
                    if !unrecognized {
                        failures.push(ImportFailure {
                            source: ImportSource {
                                label: display_label,
                            },
                            error: "No mods found".to_string(),
                        });
                    }
                    continue;
                }
                batches.push(ImportBatch {
                    source: ImportSource {
                        label: display_label,
                    },
                    mods: result.mods,
                });
            }
        }
    }

    Ok(ImportResult {
        batches,
        unrecognized,
        failures,
    })
}

fn import_from_dir(
    path: &Path,
    data_dir: &Path,
    source_label: Option<&str>,
    allow_move: bool,
    source_times: Option<SourceTimes>,
    reporter: Option<&ProgressReporter>,
) -> Result<DirImportResult> {
    let scan = scan_payload(path)?;
    let unrecognized = scan.pak_files.is_empty() && !scan.has_loose_targets();
    let allow_move = allow_move && !scan.has_overlap();
    let mut mods = Vec::new();
    let mut last_error: Option<anyhow::Error> = None;
    let loose_file_count = if scan.has_loose_targets() {
        count_loose_files(&scan)
    } else {
        0
    };

    if let Some(reporter) = reporter {
        let detail = if scan.has_loose_targets() {
            Some(format!("{loose_file_count} files"))
        } else {
            Some("Scanning layout".to_string())
        };
        reporter.report(ImportStage::Indexing, 1, 1, detail);
    }

    let use_archive_label = scan.pak_files.len() == 1;
    let meta_created = scan
        .meta_file
        .as_ref()
        .and_then(|path| metadata::read_meta_lsx(path))
        .and_then(|meta| meta.created_at);
    let json_mods = scan
        .info_json
        .as_ref()
        .map(|path| metadata::read_json_mods(path))
        .unwrap_or_default();

    let pak_total = scan.pak_files.len();
    let install_total = pak_total.saturating_add(loose_file_count).max(1);
    for (index, pak_path) in scan.pak_files.iter().enumerate() {
        let label = if use_archive_label {
            source_label
        } else {
            pak_path.file_stem().and_then(|stem| stem.to_str())
        };
        if let Some(reporter) = reporter {
            reporter.report(
                ImportStage::Installing,
                index + 1,
                install_total,
                label.map(|label| format!("Importing {label}")),
            );
        }
        match import_single_pak(pak_path, data_dir, label, source_times, &json_mods, None) {
            Ok(entry) => mods.push(entry),
            Err(err) => {
                last_error = Some(err.context(format!("import pak {:?}", pak_path)));
            }
        }
    }

    if scan.has_loose_targets() {
        match import_loose(
            path,
            data_dir,
            &scan,
            source_label,
            allow_move,
            source_times,
            meta_created.or_else(|| json_mods.iter().filter_map(|info| info.created_at).min()),
            loose_file_count,
            pak_total,
            reporter,
        ) {
            Ok(entry) => mods.push(entry),
            Err(err) => {
                last_error = Some(err.context("import loose files"));
            }
        }
    } else if let Some(reporter) = reporter {
        reporter.report(
            ImportStage::Linking,
            1,
            1,
            Some("No loose files".to_string()),
        );
    }

    if let Some(reporter) = reporter {
        reporter.report(ImportStage::Finalizing, 1, 1, None);
    }

    if mods.is_empty() {
        if let Some(err) = last_error {
            return Err(err);
        }
    }

    Ok(DirImportResult { mods, unrecognized })
}

fn import_pak_file(
    path: &Path,
    data_dir: &Path,
    source_label: Option<&str>,
    reporter: Option<&ProgressReporter>,
) -> Result<Vec<ImportMod>> {
    if let Some(reporter) = reporter {
        reporter.report(
            ImportStage::Indexing,
            1,
            1,
            Some("Reading metadata".to_string()),
        );
        reporter.report(
            ImportStage::Installing,
            1,
            1,
            Some(display_path_label(path)),
        );
    }
    let mod_entry = import_single_pak(path, data_dir, source_label, None, &[], None)?;
    if let Some(reporter) = reporter {
        reporter.report(
            ImportStage::Linking,
            1,
            1,
            Some("No loose files".to_string()),
        );
        reporter.report(ImportStage::Finalizing, 1, 1, None);
    }
    Ok(vec![mod_entry])
}

fn import_single_pak(
    path: &Path,
    data_dir: &Path,
    source_label: Option<&str>,
    source_times: Option<SourceTimes>,
    json_mods: &[metadata::JsonModInfo],
    _reporter: Option<&ProgressReporter>,
) -> Result<ImportMod> {
    let file = fs::File::open(path).context("open .pak")?;
    let lspk = lspk::Reader::new(file)
        .ok()
        .and_then(|mut reader| reader.read().ok());
    let mut meta_info = None;
    let mut module_info = None;
    if let Some(lspk) = lspk {
        if let Ok(meta) = lspk.extract_meta_lsx() {
            meta_info = Some(metadata::parse_meta_lsx(&meta.decompressed_bytes));
            if let Ok(parsed) = meta.deserialize_as_mod_pak() {
                module_info = Some(parsed.module_info);
            }
        }
    }
    if meta_info.is_none() {
        meta_info = metadata::read_meta_lsx_from_pak(path);
    }
    let meta_info = meta_info.unwrap_or_default();
    let pak_info = module_info
        .map(PakInfo::from_module_info)
        .or_else(|| pak_info_from_meta(&meta_info));
    let Some(pak_info) = pak_info else {
        return import_override_pak(path, data_dir, source_label, source_times);
    };
    let json_matches: Vec<&metadata::JsonModInfo> = json_mods
        .iter()
        .filter(|entry| {
            entry
                .uuid
                .as_ref()
                .map(|uuid| uuid == &pak_info.uuid)
                .unwrap_or(false)
                || entry
                    .folder
                    .as_ref()
                    .map(|folder| folder == &pak_info.folder)
                    .unwrap_or(false)
                || entry
                    .name
                    .as_ref()
                    .map(|name| name == &pak_info.name)
                    .unwrap_or(false)
        })
        .collect();
    let json_created = json_matches.iter().find_map(|entry| entry.created_at);

    let mod_id = pak_info.uuid.clone();
    let mut dependencies = meta_info.dependencies.clone();
    for info in json_matches {
        if !info.dependencies.is_empty() {
            dependencies.extend(info.dependencies.clone());
        }
    }
    dependencies.sort();
    dependencies.dedup();
    dependencies.retain(|dep| !dep.eq_ignore_ascii_case(&mod_id));
    let staging_root = make_stage_dir(data_dir, &mod_id)?;
    let mut guard = StagingGuard::new(staging_root.clone());

    let filename = format!("{}.pak", pak_info.folder);
    let dest = staging_root.join(&filename);
    fs::copy(path, &dest).context("copy .pak")?;

    let mut times = source_times_for(path);
    if times.created_at.is_none() && times.modified_at.is_none() {
        if let Some(fallback) = source_times {
            times = fallback;
        }
    }
    let primary_created = json_created.or(meta_info.created_at);
    let (created_at, modified_at) =
        resolve_times(primary_created, times.created_at, times.modified_at);
    let entry = ModEntry {
        id: mod_id,
        name: pak_info.name.clone(),
        created_at,
        modified_at,
        added_at: now_timestamp(),
        targets: vec![InstallTarget::Pak {
            file: filename,
            info: pak_info,
        }],
        target_overrides: Vec::new(),
        source_label: source_label.map(|label| label.to_string()),
        source: ModSource::Managed,
        dependencies,
    };
    guard.disarm();
    Ok(ImportMod {
        entry,
        staging_root: Some(staging_root),
        sigillink: None,
    })
}

fn pak_info_from_meta(meta: &metadata::ModMeta) -> Option<PakInfo> {
    let uuid = meta.uuid.clone()?;
    let folder = meta
        .folder
        .clone()
        .or_else(|| meta.name.clone())
        .unwrap_or_else(|| uuid.clone());
    let name = meta.name.clone().unwrap_or_else(|| folder.clone());
    Some(PakInfo {
        uuid,
        name,
        folder,
        version: meta.version.unwrap_or(0),
        md5: meta.md5.clone(),
        publish_handle: meta.publish_handle,
        author: meta.author.clone(),
        description: meta.description.clone(),
        module_type: meta.module_type.clone(),
    })
}

fn import_override_pak(
    path: &Path,
    data_dir: &Path,
    source_label: Option<&str>,
    source_times: Option<SourceTimes>,
) -> Result<ImportMod> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("override.pak");
    let mod_id = hash_path_with_prefix(path, "pak");
    let staging_root = make_stage_dir(data_dir, &mod_id)?;
    let mut guard = StagingGuard::new(staging_root.clone());
    let data_root = staging_root.join("Data");
    fs::create_dir_all(&data_root).context("create override pak storage")?;
    let dest = data_root.join(filename);
    fs::copy(path, &dest).context("copy override .pak")?;

    let mut times = source_times_for(path);
    if times.created_at.is_none() && times.modified_at.is_none() {
        if let Some(fallback) = source_times {
            times = fallback;
        }
    }
    let (created_at, modified_at) = resolve_times(None, times.created_at, times.modified_at);
    let name = if let Some(label) = source_label {
        format!("Override Pak: {label}")
    } else {
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("Override Pak");
        format!("Override Pak: {stem}")
    };

    let entry = ModEntry {
        id: mod_id,
        name,
        created_at,
        modified_at,
        added_at: now_timestamp(),
        targets: vec![InstallTarget::Data {
            dir: "Data".to_string(),
        }],
        target_overrides: Vec::new(),
        source_label: source_label.map(|label| label.to_string()),
        source: ModSource::Managed,
        dependencies: Vec::new(),
    };
    guard.disarm();
    Ok(ImportMod {
        entry,
        staging_root: Some(staging_root),
        sigillink: None,
    })
}

fn count_loose_files(scan: &PayloadScan) -> usize {
    let mut total_files = 0usize;
    if let Some(data_dir) = &scan.data_dir {
        total_files = total_files.saturating_add(count_copy_files(data_dir));
    }
    if let Some(generated_dir) = &scan.generated_dir {
        total_files = total_files.saturating_add(count_copy_files(generated_dir));
    } else if let Some(public_dir) = &scan.public_dir {
        total_files = total_files.saturating_add(count_copy_files(public_dir));
    }
    if let Some(bin_dir) = &scan.bin_dir {
        total_files = total_files.saturating_add(count_copy_files(bin_dir));
    }
    total_files
}

fn import_loose(
    path: &Path,
    data_dir: &Path,
    scan: &PayloadScan,
    source_label: Option<&str>,
    allow_move: bool,
    source_times: Option<SourceTimes>,
    meta_created: Option<i64>,
    total_files: usize,
    install_offset: usize,
    reporter: Option<&ProgressReporter>,
) -> Result<ImportMod> {
    let mod_id = hash_path(path);
    let staging_root = make_stage_dir(data_dir, &mod_id)?;
    let mut guard = StagingGuard::new(staging_root.clone());

    let mut targets = Vec::new();
    let install_total = install_offset.saturating_add(total_files).max(1);
    if let Some(reporter) = reporter {
        reporter.report(
            ImportStage::Installing,
            install_offset.min(install_total),
            install_total,
            Some("Copying files".to_string()),
        );
    }
    let mut progress = CopyProgress::new_with_offset(
        reporter,
        install_total,
        ImportStage::Installing,
        install_offset,
    );

    if let Some(data_dir) = &scan.data_dir {
        let dest = staging_root.join("Data");
        if allow_move {
            move_or_copy_dir_with_progress(data_dir, &dest, &mut progress)?;
        } else {
            copy_dir_with_progress(data_dir, &dest, &mut progress)?;
        }
        targets.push(InstallTarget::Data {
            dir: "Data".to_string(),
        });
    }

    if let Some(generated_dir) = &scan.generated_dir {
        let dest = staging_root.join("Generated");
        if allow_move {
            move_or_copy_dir_with_progress(generated_dir, &dest, &mut progress)?;
        } else {
            copy_dir_with_progress(generated_dir, &dest, &mut progress)?;
        }
        targets.push(InstallTarget::Generated {
            dir: "Generated".to_string(),
        });
    } else if let Some(public_dir) = &scan.public_dir {
        let dest = staging_root.join("Generated").join("Public");
        if allow_move {
            move_or_copy_dir_with_progress(public_dir, &dest, &mut progress)?;
        } else {
            copy_dir_with_progress(public_dir, &dest, &mut progress)?;
        }
        targets.push(InstallTarget::Generated {
            dir: "Generated".to_string(),
        });
    }

    if let Some(bin_dir) = &scan.bin_dir {
        let dest = staging_root.join("bin");
        if allow_move {
            move_or_copy_dir_with_progress(bin_dir, &dest, &mut progress)?;
        } else {
            copy_dir_with_progress(bin_dir, &dest, &mut progress)?;
        }
        targets.push(InstallTarget::Bin {
            dir: "bin".to_string(),
        });
    }
    progress.finish();

    persist_payload_metadata(scan, &staging_root);
    let sigillink = build_sigillink_index(&staging_root, &targets, total_files, reporter)?;

    let raw_label = source_label
        .map(|label| label.to_string())
        .or_else(|| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.to_string())
        })
        .unwrap_or_else(|| "Loose Files".to_string());
    let cleaned = clean_source_label(&raw_label);
    let name = if cleaned.is_empty() {
        raw_label
    } else {
        cleaned
    };
    let mut times = scan_payload_times(scan);
    if times.created_at.is_none() && times.modified_at.is_none() {
        times = source_times.unwrap_or_else(|| source_times_for(path));
    }
    let (created_at, modified_at) =
        resolve_times(meta_created, times.created_at, times.modified_at);

    let entry = ModEntry {
        id: mod_id,
        name,
        created_at,
        modified_at,
        added_at: now_timestamp(),
        targets,
        target_overrides: Vec::new(),
        source_label: source_label.map(|label| label.to_string()),
        source: ModSource::Managed,
        dependencies: Vec::new(),
    };
    guard.disarm();
    Ok(ImportMod {
        entry,
        staging_root: Some(staging_root),
        sigillink: Some(sigillink),
    })
}

fn persist_payload_metadata(scan: &PayloadScan, mod_root: &Path) {
    let mut copied_any = false;
    if scan.meta_file.is_some() || scan.info_json.is_some() {
        let meta_root = mod_root.join("_meta");
        if fs::create_dir_all(&meta_root).is_ok() {
            if let Some(meta_path) = &scan.meta_file {
                let dest = meta_root.join("meta.lsx");
                if fs::copy(meta_path, &dest).is_ok() {
                    copied_any = true;
                }
            }
            if let Some(info_path) = &scan.info_json {
                let name = info_path
                    .file_name()
                    .map(|name| name.to_os_string())
                    .unwrap_or_else(|| "info.json".into());
                let dest = meta_root.join(name);
                if fs::copy(info_path, &dest).is_ok() {
                    copied_any = true;
                }
            }
        }
    }
    if !copied_any {
        let _ = fs::remove_dir_all(mod_root.join("_meta"));
    }
}

fn build_sigillink_index(
    mod_root: &Path,
    targets: &[InstallTarget],
    total_files: usize,
    reporter: Option<&ProgressReporter>,
) -> Result<SigilLinkIndex> {
    let mut entries = Vec::new();
    let mut total_bytes = 0u64;
    let mut progress = CopyProgress::new(reporter, total_files, ImportStage::Linking);
    if let Some(reporter) = reporter {
        reporter.report(
            ImportStage::Linking,
            0,
            total_files.max(1),
            Some("Building SigiLink cache".to_string()),
        );
    }

    for target in targets {
        let (kind, dir) = match target {
            InstallTarget::Data { dir } => (TargetKind::Data, dir.as_str()),
            InstallTarget::Generated { dir } => (TargetKind::Generated, dir.as_str()),
            InstallTarget::Bin { dir } => (TargetKind::Bin, dir.as_str()),
            InstallTarget::Pak { .. } => continue,
        };
        let root = mod_root.join(dir);
        if !root.exists() {
            continue;
        }
        collect_sigillink_entries(&root, kind, &mut entries, &mut total_bytes, &mut progress)?;
    }

    progress.finish();
    Ok(SigilLinkIndex {
        version: SIGILLINK_VERSION,
        entries,
        total_bytes,
    })
}

fn collect_sigillink_entries(
    root: &Path,
    kind: TargetKind,
    entries: &mut Vec<SigilLinkEntry>,
    total_bytes: &mut u64,
    progress: &mut CopyProgress<'_>,
) -> Result<()> {
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !is_ignored_path(entry.path()))
    {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry.path().strip_prefix(root).context("rel path")?;
        let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
        *total_bytes = total_bytes.saturating_add(size);
        entries.push(SigilLinkEntry {
            kind,
            relative_path: rel.to_string_lossy().to_string(),
            size,
        });
        progress.bump(None, false);
    }

    Ok(())
}

fn source_label_for_archive(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
}

fn display_path_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_string())
        .unwrap_or_else(|| path.display().to_string())
}

fn scan_payload(root: &Path) -> Result<PayloadScan> {
    let mut pak_files = Vec::new();
    let mut data_candidates = Vec::new();
    let mut generated_candidates = Vec::new();
    let mut bin_candidates = Vec::new();
    let mut public_candidates = Vec::new();
    let mut meta_candidates = Vec::new();
    let mut json_candidates = Vec::new();
    let mut root_bin_marker = false;

    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        let path = entry.path();

        if is_ignored_path(path) {
            continue;
        }

        if entry.file_type().is_file() {
            if path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("pak"))
                .unwrap_or(false)
            {
                pak_files.push(path.to_path_buf());
            }
            let name = entry.file_name().to_string_lossy();
            let depth = relative_depth(root, path);
            if depth == 1 && is_bin_root_file(&name) {
                root_bin_marker = true;
            }
            if name.eq_ignore_ascii_case("meta.lsx") {
                meta_candidates.push((path.to_path_buf(), depth));
            }
            if name.to_ascii_lowercase().ends_with(".json") {
                let lower = name.to_ascii_lowercase();
                let priority = match lower.as_str() {
                    "info.json" => 0,
                    "mod.json" => 1,
                    "modinfo.json" => 2,
                    _ => 3,
                };
                if priority < 3 {
                    json_candidates.push((path.to_path_buf(), depth, priority));
                }
            }
        }

        if entry.file_type().is_dir() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            let depth = relative_depth(root, path);
            if name == "data" {
                data_candidates.push((path.to_path_buf(), depth));
            } else if name == "generated" {
                generated_candidates.push((path.to_path_buf(), depth));
            } else if name == "bin" {
                bin_candidates.push((path.to_path_buf(), depth));
            } else if name == "public" {
                if has_parent_named(path, "generated") || has_parent_named(path, "data") {
                    continue;
                }
                public_candidates.push((path.to_path_buf(), depth));
            }
        }
    }

    let data_dir = pick_shallowest(data_candidates);
    let generated_dir = pick_shallowest(generated_candidates);
    let mut bin_dir = pick_shallowest(bin_candidates);
    let public_dir = pick_shallowest(public_candidates);
    let meta_file = pick_meta_lsx(meta_candidates);
    let info_json = pick_info_json(json_candidates);
    if bin_dir.is_none()
        && root_bin_marker
        && pak_files.is_empty()
        && data_dir.is_none()
        && generated_dir.is_none()
        && public_dir.is_none()
    {
        bin_dir = Some(root.to_path_buf());
    }

    Ok(PayloadScan {
        pak_files,
        data_dir,
        generated_dir,
        bin_dir,
        public_dir,
        meta_file,
        info_json,
    })
}

fn collect_import_candidates(root: &Path) -> Result<Vec<ImportCandidate>> {
    let mut candidates = Vec::new();
    let mut top_level_dirs = Vec::new();
    let mut mods_dir: Option<PathBuf> = None;
    let mut candidate_dirs: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut root_has_loose_dir = false;
    let mut root_has_pak = false;

    for entry in fs::read_dir(root).context("read import dir")? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();
        if is_ignored_path(&path) {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if file_type.is_file() {
            if is_archive_file(&path) {
                let label = display_path_label(&path);
                push_candidate(
                    &mut candidates,
                    &mut candidate_dirs,
                    &mut seen,
                    path,
                    label,
                    CandidateKind::ArchiveFile,
                );
            } else if is_pak_file(&path) {
                root_has_pak = true;
                let label = display_pak_label(&path);
                push_candidate(
                    &mut candidates,
                    &mut candidate_dirs,
                    &mut seen,
                    path,
                    label,
                    CandidateKind::PakFile,
                );
            }
        } else if file_type.is_dir() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if matches!(name.as_str(), "data" | "generated" | "bin" | "public") {
                root_has_loose_dir = true;
            }
            if name == "mods" {
                mods_dir = Some(path);
            } else {
                top_level_dirs.push(path);
            }
        }
    }

    if root_has_loose_dir || root_has_pak {
        return Ok(Vec::new());
    }

    if let Some(mods_dir) = mods_dir {
        for entry in fs::read_dir(mods_dir).context("read Mods dir")? {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let path = entry.path();
            if is_ignored_path(&path) {
                continue;
            }
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if file_type.is_file() && is_pak_file(&path) {
                let label = display_pak_label(&path);
                push_candidate(
                    &mut candidates,
                    &mut candidate_dirs,
                    &mut seen,
                    path,
                    label,
                    CandidateKind::PakFile,
                );
            } else if file_type.is_dir() {
                if let Ok(true) = is_mod_candidate_dir(&path) {
                    let label = display_path_label(&path);
                    push_candidate(
                        &mut candidates,
                        &mut candidate_dirs,
                        &mut seen,
                        path,
                        label,
                        CandidateKind::Directory,
                    );
                }
            }
        }
    }

    for dir in top_level_dirs {
        if let Ok(true) = is_mod_candidate_dir(&dir) {
            let label = display_path_label(&dir);
            push_candidate(
                &mut candidates,
                &mut candidate_dirs,
                &mut seen,
                dir,
                label,
                CandidateKind::Directory,
            );
        }
    }

    if root.is_dir() {
        let walker = WalkDir::new(root)
            .follow_links(false)
            .min_depth(2)
            .max_depth(NESTED_ARCHIVE_SCAN_DEPTH);
        for entry in walker.into_iter().filter_map(Result::ok) {
            let path = entry.path();
            if is_ignored_path(path) {
                continue;
            }
            if !entry.file_type().is_file() {
                continue;
            }
            if is_archive_file(path) {
                let label = display_path_label(path);
                push_candidate(
                    &mut candidates,
                    &mut candidate_dirs,
                    &mut seen,
                    path.to_path_buf(),
                    label,
                    CandidateKind::ArchiveFile,
                );
                continue;
            }
            if is_pak_file(path) && !has_candidate_dir_ancestor(path, &candidate_dirs) {
                let label = display_pak_label(path);
                push_candidate(
                    &mut candidates,
                    &mut candidate_dirs,
                    &mut seen,
                    path.to_path_buf(),
                    label,
                    CandidateKind::PakFile,
                );
            }
        }
    }

    candidates.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(candidates)
}

fn has_candidate_dir_ancestor(path: &Path, candidates: &[PathBuf]) -> bool {
    candidates.iter().any(|dir| path.starts_with(dir))
}

fn push_candidate(
    candidates: &mut Vec<ImportCandidate>,
    candidate_dirs: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
    path: PathBuf,
    label: String,
    kind: CandidateKind,
) {
    if seen.insert(path.clone()) {
        if matches!(kind, CandidateKind::Directory) {
            candidate_dirs.push(path.clone());
        }
        candidates.push(ImportCandidate { label, path, kind });
    }
}

fn is_mod_candidate_dir(path: &Path) -> Result<bool> {
    let scan = scan_payload(path)?;
    Ok(!scan.pak_files.is_empty() || scan.has_loose_targets())
}

fn is_archive_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()).unwrap_or(""),
        "zip" | "ZIP" | "7z" | "7Z" | "rar" | "RAR"
    )
}

fn is_pak_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()).unwrap_or(""),
        "pak" | "PAK"
    )
}

fn display_pak_label(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| display_path_label(path))
}

fn is_bin_root_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "dwrite.dll" | "bink2w64.dll" | "scriptendersettings.json"
    ) {
        return true;
    }
    lower.contains("scriptextender") || lower.contains("bg3se")
}

fn pick_shallowest(mut candidates: Vec<(PathBuf, usize)>) -> Option<PathBuf> {
    candidates.sort_by_key(|(_, depth)| *depth);
    candidates.into_iter().map(|(path, _)| path).next()
}

fn pick_meta_lsx(mut candidates: Vec<(PathBuf, usize)>) -> Option<PathBuf> {
    candidates.sort_by_key(|(path, depth)| (!has_mods_parent(path), *depth));
    candidates.into_iter().map(|(path, _)| path).next()
}

fn pick_info_json(mut candidates: Vec<(PathBuf, usize, usize)>) -> Option<PathBuf> {
    candidates.sort_by_key(|(_, depth, priority)| (*priority, *depth));
    candidates.into_iter().map(|(path, _, _)| path).next()
}

fn has_mods_parent(path: &Path) -> bool {
    path.ancestors()
        .skip(1)
        .filter_map(|ancestor| ancestor.file_name())
        .any(|name| name.to_string_lossy().eq_ignore_ascii_case("Mods"))
}

fn relative_depth(root: &Path, path: &Path) -> usize {
    path.strip_prefix(root)
        .map(|p| p.components().count())
        .unwrap_or(usize::MAX)
}

fn is_ignored_path(path: &Path) -> bool {
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

fn extract_zip(path: &Path, dest: &Path) -> Result<()> {
    match extract_with_7z(path, dest) {
        Ok(Some(())) => return Ok(()),
        Ok(None) => {}
        Err(err) => return Err(err),
    }

    let file = fs::File::open(path).context("open zip")?;
    let mut archive = zip::ZipArchive::new(file).context("read zip")?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).context("zip entry")?;
        let Some(out_path) = file.enclosed_name() else {
            continue;
        };

        let out_path = dest.join(out_path);
        if file.is_dir() {
            fs::create_dir_all(&out_path).context("create zip dir")?;
            continue;
        }

        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).context("create zip dir")?;
        }

        let mut out_file = fs::File::create(&out_path).context("write zip entry")?;
        std::io::copy(&mut file, &mut out_file).context("extract zip entry")?;
        if let Some(dt) = file.last_modified() {
            if let Some(mtime) = zip_time_to_unix(dt) {
                let mtime = FileTime::from_unix_time(mtime, 0);
                let _ = set_file_mtime(&out_path, mtime);
            }
        }
    }

    Ok(())
}

fn zip_time_to_unix(dt: zip::DateTime) -> Option<i64> {
    let month = Month::try_from(dt.month()).ok()?;
    let date = Date::from_calendar_date(dt.year() as i32, month, dt.day()).ok()?;
    let time = TimeOfDay::from_hms(dt.hour(), dt.minute(), dt.second()).ok()?;
    let datetime = PrimitiveDateTime::new(date, time).assume_utc();
    Some(datetime.unix_timestamp())
}

fn extract_7z(path: &Path, dest: &Path) -> Result<()> {
    match extract_with_7z(path, dest) {
        Ok(Some(())) => Ok(()),
        Ok(None) => sevenz_rust::decompress_file(path, dest)
            .with_context(|| format!("extract 7z archive {path:?}")),
        Err(err) => Err(err),
    }
}

fn extract_with_7z(path: &Path, dest: &Path) -> Result<Option<()>> {
    let mut command = Command::new("7z");
    let output = command
        .arg("x")
        .arg("-y")
        .arg("-mmt=on")
        .arg(format!("-o{}", dest.display()))
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();

    let output = match output {
        Ok(output) => output,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).context("launch 7z");
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("7z extraction failed: {}", stderr.trim()));
    }

    Ok(Some(()))
}

fn count_copy_files(source: &Path) -> usize {
    WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !is_ignored_path(entry.path()))
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .count()
}

fn copy_dir_with_progress(
    source: &Path,
    dest: &Path,
    progress: &mut CopyProgress<'_>,
) -> Result<()> {
    for entry in WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !is_ignored_path(entry.path()))
    {
        let entry = entry?;
        let rel = entry.path().strip_prefix(source).context("rel path")?;
        let target = dest.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target).context("create dir")?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).context("create file dir")?;
            }
            fs::copy(entry.path(), &target).context("copy file")?;
            preserve_mtime(entry.path(), &target);
            progress.bump(None, false);
        }
    }
    Ok(())
}

fn preserve_mtime(source: &Path, dest: &Path) {
    let Ok(meta) = fs::metadata(source) else {
        return;
    };
    let Ok(modified) = meta.modified() else {
        return;
    };
    let Ok(duration) = modified.duration_since(UNIX_EPOCH) else {
        return;
    };
    let mtime = FileTime::from_unix_time(duration.as_secs() as i64, 0);
    let _ = set_file_mtime(dest, mtime);
}

fn move_or_copy_dir_with_progress(
    source: &Path,
    dest: &Path,
    progress: &mut CopyProgress<'_>,
) -> Result<()> {
    if dest.exists() {
        fs::remove_dir_all(dest).context("remove existing target")?;
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).context("create target parent")?;
    }
    let count = count_copy_files(source);
    if contains_ignored_path(source) {
        copy_dir_with_progress(source, dest, progress)?;
        return Ok(());
    }
    match fs::rename(source, dest) {
        Ok(_) => {
            progress.advance(count);
            Ok(())
        }
        Err(_) => copy_dir_with_progress(source, dest, progress),
    }
}

fn contains_ignored_path(source: &Path) -> bool {
    for entry in WalkDir::new(source).follow_links(false).into_iter() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if is_ignored_path(entry.path()) {
            return true;
        }
    }
    false
}

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn make_temp_dir(data_dir: &Path, suffix: &str) -> Result<PathBuf> {
    let temp_root = data_dir.join("tmp");
    fs::create_dir_all(&temp_root).context("create temp root")?;

    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = format!("import-{nanos}-{counter}-{suffix}");
    let temp_dir = temp_root.join(name);
    fs::create_dir_all(&temp_dir).context("create temp dir")?;
    Ok(temp_dir)
}

fn make_stage_dir(data_dir: &Path, mod_id: &str) -> Result<PathBuf> {
    let label = sanitize_stage_label(mod_id);
    make_temp_dir(data_dir, &format!("stage-{label}"))
}

fn sanitize_stage_label(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn hash_path(path: &Path) -> String {
    hash_path_with_prefix(path, "loose")
}

fn hash_path_with_prefix(path: &Path, prefix: &str) -> String {
    let mut hasher = Hasher::new();
    hasher.update(path.to_string_lossy().as_bytes());
    if let Ok(meta) = fs::metadata(path) {
        if let Ok(modified) = meta.modified() {
            if let Ok(duration) = modified.duration_since(UNIX_EPOCH) {
                hasher.update(&duration.as_secs().to_le_bytes());
            }
        }
    }

    let hash = hasher.finalize();
    format!("{prefix}-{}", hash.to_hex())
}

fn now_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[derive(Debug)]
struct PayloadScan {
    pak_files: Vec<PathBuf>,
    data_dir: Option<PathBuf>,
    generated_dir: Option<PathBuf>,
    bin_dir: Option<PathBuf>,
    public_dir: Option<PathBuf>,
    meta_file: Option<PathBuf>,
    info_json: Option<PathBuf>,
}

impl PayloadScan {
    fn has_loose_targets(&self) -> bool {
        self.data_dir.is_some()
            || self.generated_dir.is_some()
            || self.bin_dir.is_some()
            || self.public_dir.is_some()
    }

    fn has_overlap(&self) -> bool {
        let mut paths = Vec::new();
        if let Some(path) = &self.data_dir {
            paths.push(path);
        }
        if let Some(path) = &self.generated_dir {
            paths.push(path);
        }
        if let Some(path) = &self.bin_dir {
            paths.push(path);
        }
        if let Some(path) = &self.public_dir {
            paths.push(path);
        }

        for (idx, path) in paths.iter().enumerate() {
            for other in paths.iter().skip(idx + 1) {
                if is_parent_path(path, other) || is_parent_path(other, path) {
                    return true;
                }
            }
        }

        false
    }
}

fn is_parent_path(parent: &Path, child: &Path) -> bool {
    child.starts_with(parent) && child != parent
}

fn has_parent_named(path: &Path, needle: &str) -> bool {
    path.ancestors()
        .skip(1)
        .filter_map(|ancestor| ancestor.file_name())
        .any(|name| name.to_string_lossy().eq_ignore_ascii_case(needle))
}
