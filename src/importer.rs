use crate::library::{
    library_mod_root, normalize_times, path_times, resolve_times, InstallTarget, ModEntry,
    ModSource, PakInfo,
};
use crate::metadata;
use anyhow::{Context, Result};
use blake3::Hasher;
use filetime::{set_file_mtime, FileTime};
use larian_formats::lspk;
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};
use time::{Date, Month, PrimitiveDateTime, Time as TimeOfDay};
use walkdir::WalkDir;

pub struct ImportResult {
    pub mods: Vec<ModEntry>,
    pub unrecognized: bool,
}

#[derive(Clone, Copy)]
struct SourceTimes {
    created_at: Option<i64>,
    modified_at: Option<i64>,
}

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

pub fn import_path(path: &Path, data_dir: &Path) -> Result<ImportResult> {
    if !path.exists() {
        return Ok(ImportResult {
            mods: Vec::new(),
            unrecognized: false,
        });
    }

    fs::create_dir_all(library_mod_root(data_dir)).context("create mod library root")?;

    let result = if path.is_dir() {
        import_from_dir(path, data_dir, None, false, None)?
    } else {
        let source_label = source_label_for_archive(path);
        match path.extension().and_then(|ext| ext.to_str()).unwrap_or("") {
            "pak" | "PAK" => ImportResult {
                mods: import_pak_file(path, data_dir)?,
                unrecognized: false,
            },
            "zip" | "ZIP" => import_archive_zip(path, data_dir, source_label.as_deref())?,
            "7z" | "7Z" => import_archive_7z(path, data_dir, source_label.as_deref())?,
            _ => ImportResult {
                mods: Vec::new(),
                unrecognized: true,
            },
        }
    };

    Ok(result)
}

fn import_archive_zip(
    path: &Path,
    data_dir: &Path,
    source_label: Option<&str>,
) -> Result<ImportResult> {
    let temp_dir = make_temp_dir(data_dir, "zip")?;
    let source_times = source_times_for(path);
    if let Err(err) = extract_zip(path, &temp_dir) {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(err);
    }
    let result = import_from_dir(&temp_dir, data_dir, source_label, true, Some(source_times));
    let _ = fs::remove_dir_all(&temp_dir);
    result
}

fn import_archive_7z(
    path: &Path,
    data_dir: &Path,
    source_label: Option<&str>,
) -> Result<ImportResult> {
    let temp_dir = make_temp_dir(data_dir, "7z")?;
    let source_times = source_times_for(path);
    if let Err(err) = extract_7z(path, &temp_dir) {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(err);
    }
    let result = import_from_dir(&temp_dir, data_dir, source_label, true, Some(source_times));
    let _ = fs::remove_dir_all(&temp_dir);
    result
}

fn import_from_dir(
    path: &Path,
    data_dir: &Path,
    source_label: Option<&str>,
    allow_move: bool,
    source_times: Option<SourceTimes>,
) -> Result<ImportResult> {
    let scan = scan_payload(path)?;
    let unrecognized = scan.pak_files.is_empty() && !scan.has_loose_targets();
    let allow_move = allow_move && !scan.has_overlap();
    let mut mods = Vec::new();
    let mut last_error: Option<anyhow::Error> = None;

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

    for pak_path in &scan.pak_files {
        let label = if use_archive_label {
            source_label
        } else {
            pak_path.file_stem().and_then(|stem| stem.to_str())
        };
        match import_single_pak(pak_path, data_dir, label, source_times, &json_mods) {
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
        ) {
            Ok(entry) => mods.push(entry),
            Err(err) => {
                last_error = Some(err.context("import loose files"));
            }
        }
    }

    if mods.is_empty() {
        if let Some(err) = last_error {
            return Err(err);
        }
    }

    Ok(ImportResult { mods, unrecognized })
}

fn import_pak_file(path: &Path, data_dir: &Path) -> Result<Vec<ModEntry>> {
    let label = source_label_for_archive(path);
    Ok(vec![import_single_pak(
        path,
        data_dir,
        label.as_deref(),
        None,
        &[],
    )?])
}

fn import_single_pak(
    path: &Path,
    data_dir: &Path,
    source_label: Option<&str>,
    source_times: Option<SourceTimes>,
    json_mods: &[metadata::JsonModInfo],
) -> Result<ModEntry> {
    let file = fs::File::open(path).context("open .pak")?;
    let lspk = lspk::Reader::new(file)
        .context("read .pak header")?
        .read()
        .context("read .pak index")?;
    let meta = match lspk.extract_meta_lsx() {
        Ok(meta) => meta,
        Err(_) => {
            return import_override_pak(path, data_dir, source_label, source_times);
        }
    };
    let meta_info = metadata::parse_meta_lsx(&meta.decompressed_bytes);
    let module_info = meta
        .deserialize_as_mod_pak()
        .context("parse meta.lsx")?
        .module_info;
    let pak_info = PakInfo::from_module_info(module_info.clone());
    let json_created = json_mods
        .iter()
        .find(|entry| {
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
        .and_then(|entry| entry.created_at);

    let mod_id = pak_info.uuid.clone();
    let mod_root = library_mod_root(data_dir).join(&mod_id);
    fs::create_dir_all(&mod_root).context("create mod storage")?;

    let filename = format!("{}.pak", pak_info.folder);
    let dest = mod_root.join(&filename);
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
    Ok(ModEntry {
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
    })
}

fn import_override_pak(
    path: &Path,
    data_dir: &Path,
    source_label: Option<&str>,
    source_times: Option<SourceTimes>,
) -> Result<ModEntry> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("override.pak");
    let mod_id = hash_path_with_prefix(path, "pak");
    let mod_root = library_mod_root(data_dir).join(&mod_id);
    let data_root = mod_root.join("Data");
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

    Ok(ModEntry {
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
    })
}

fn import_loose(
    path: &Path,
    data_dir: &Path,
    scan: &PayloadScan,
    source_label: Option<&str>,
    allow_move: bool,
    source_times: Option<SourceTimes>,
    meta_created: Option<i64>,
) -> Result<ModEntry> {
    let mod_id = hash_path(path);
    let mod_root = library_mod_root(data_dir).join(&mod_id);
    fs::create_dir_all(&mod_root).context("create loose mod storage")?;

    let mut targets = Vec::new();

    if let Some(data_dir) = &scan.data_dir {
        let dest = mod_root.join("Data");
        if allow_move {
            move_or_copy_dir(data_dir, &dest)?;
        } else {
            copy_dir(data_dir, &dest)?;
        }
        targets.push(InstallTarget::Data {
            dir: "Data".to_string(),
        });
    }

    if let Some(generated_dir) = &scan.generated_dir {
        let dest = mod_root.join("Generated");
        if allow_move {
            move_or_copy_dir(generated_dir, &dest)?;
        } else {
            copy_dir(generated_dir, &dest)?;
        }
        targets.push(InstallTarget::Generated {
            dir: "Generated".to_string(),
        });
    } else if let Some(public_dir) = &scan.public_dir {
        let dest = mod_root.join("Generated").join("Public");
        if allow_move {
            move_or_copy_dir(public_dir, &dest)?;
        } else {
            copy_dir(public_dir, &dest)?;
        }
        targets.push(InstallTarget::Generated {
            dir: "Generated".to_string(),
        });
    }

    if let Some(bin_dir) = &scan.bin_dir {
        let dest = mod_root.join("bin");
        if allow_move {
            move_or_copy_dir(bin_dir, &dest)?;
        } else {
            copy_dir(bin_dir, &dest)?;
        }
        targets.push(InstallTarget::Bin {
            dir: "bin".to_string(),
        });
    }

    persist_payload_metadata(scan, &mod_root);

    let name = if let Some(label) = source_label {
        format!("Loose Files: {label}")
    } else {
        path.file_name()
            .and_then(|s| s.to_str())
            .map(|s| format!("Loose Files: {s}"))
            .unwrap_or_else(|| "Loose Files".to_string())
    };
    let mut times = scan_payload_times(scan);
    if times.created_at.is_none() && times.modified_at.is_none() {
        times = source_times.unwrap_or_else(|| source_times_for(path));
    }
    let (created_at, modified_at) =
        resolve_times(meta_created, times.created_at, times.modified_at);

    Ok(ModEntry {
        id: mod_id,
        name,
        created_at,
        modified_at,
        added_at: now_timestamp(),
        targets,
        target_overrides: Vec::new(),
        source_label: source_label.map(|label| label.to_string()),
        source: ModSource::Managed,
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

fn source_label_for_archive(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
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

fn copy_dir(source: &Path, dest: &Path) -> Result<()> {
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

fn move_or_copy_dir(source: &Path, dest: &Path) -> Result<()> {
    if dest.exists() {
        fs::remove_dir_all(dest).context("remove existing target")?;
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).context("create target parent")?;
    }
    if contains_ignored_path(source) {
        return copy_dir(source, dest);
    }
    match fs::rename(source, dest) {
        Ok(_) => Ok(()),
        Err(_) => copy_dir(source, dest),
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

fn make_temp_dir(data_dir: &Path, suffix: &str) -> Result<PathBuf> {
    let temp_root = data_dir.join("tmp");
    fs::create_dir_all(&temp_root).context("create temp root")?;

    let name = format!("import-{}-{}", now_timestamp(), suffix);
    let temp_dir = temp_root.join(name);
    fs::create_dir_all(&temp_dir).context("create temp dir")?;
    Ok(temp_dir)
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
