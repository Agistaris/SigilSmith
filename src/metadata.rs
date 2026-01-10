use flate2::read::ZlibDecoder;
use larian_formats::lspk;
use lz4_flex::block::decompress;
use quick_xml::{events::Event, Reader};
use serde_json::Value;
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};
use time::{format_description::well_known::Rfc3339, Date, OffsetDateTime, PrimitiveDateTime};
use walkdir::WalkDir;
use zstd::bulk::decompress as zstd_decompress;

#[derive(Debug, Default, Clone)]
pub struct ModMeta {
    pub dependencies: Vec<String>,
    pub tags: Vec<String>,
    pub created_at: Option<i64>,
    pub uuid: Option<String>,
    pub folder: Option<String>,
    pub name: Option<String>,
    pub version: Option<u64>,
    pub md5: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub publish_handle: Option<u64>,
    pub module_type: Option<String>,
}

#[derive(Debug, Clone)]
pub struct JsonModInfo {
    pub uuid: Option<String>,
    pub folder: Option<String>,
    pub name: Option<String>,
    pub created_at: Option<i64>,
    pub dependencies: Vec<String>,
}

pub fn parse_meta_lsx(bytes: &[u8]) -> ModMeta {
    let mut reader = Reader::from_reader(bytes);
    reader.trim_text(true);
    let mut buf = Vec::new();
    let mut node_stack: Vec<String> = Vec::new();
    let mut deps = Vec::new();
    let mut tags = Vec::new();
    let mut created_at: Option<i64> = None;
    let mut uuid = None;
    let mut folder = None;
    let mut name = None;
    let mut version = None;
    let mut md5 = None;
    let mut author = None;
    let mut description = None;
    let mut publish_handle = None;
    let mut module_type = None;
    let mut in_dependencies = false;
    let mut in_dependency = false;
    let mut in_module_info = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if e.name().as_ref() == b"node" {
                    if let Some(id) = attr_value(&e, b"id") {
                        node_stack.push(id);
                        in_dependencies = node_stack.iter().any(|node| node == "Dependencies");
                        in_dependency = node_stack.iter().any(|node| node == "Dependency");
                        in_module_info = node_stack.iter().any(|node| node == "ModuleInfo");
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                if e.name().as_ref() == b"attribute" {
                    if in_dependencies && in_dependency {
                        if let (Some(id), Some(value)) =
                            (attr_value(&e, b"id"), attr_value(&e, b"value"))
                        {
                            if id == "UUID" {
                                deps.push(value);
                            }
                        }
                    }
                    if in_module_info {
                        if let (Some(id), Some(value)) =
                            (attr_value(&e, b"id"), attr_value(&e, b"value"))
                        {
                            let value_str = value.as_str();
                            if id == "Tags" && !value_str.trim().is_empty() {
                                tags.extend(split_tags(value_str));
                            }
                            if id == "Created" {
                                if let Some(parsed) = parse_created_at(value_str) {
                                    created_at = Some(match created_at {
                                        Some(existing) => existing.min(parsed),
                                        None => parsed,
                                    });
                                }
                            }
                            match id.as_str() {
                                "UUID" => uuid = Some(value.clone()),
                                "Folder" => folder = Some(value.clone()),
                                "Name" => name = Some(value.clone()),
                                "Version64" | "Version" => {
                                    if let Ok(parsed) = value_str.parse::<u64>() {
                                        version = Some(parsed);
                                    }
                                }
                                "MD5" => md5 = Some(value.clone()),
                                "Author" => author = Some(value.clone()),
                                "Description" => description = Some(value.clone()),
                                "PublishHandle" => {
                                    if let Ok(parsed) = value_str.parse::<u64>() {
                                        publish_handle = Some(parsed);
                                    }
                                }
                                "Type" | "ModuleType" => module_type = Some(value.clone()),
                                _ => {}
                            }
                        }
                    } else if created_at.is_none() {
                        if let (Some(id), Some(value)) =
                            (attr_value(&e, b"id"), attr_value(&e, b"value"))
                        {
                            if id == "Created" {
                                if let Some(parsed) = parse_created_at(&value) {
                                    created_at = Some(parsed);
                                }
                            }
                        }
                    }
                }
            }
            Ok(Event::End(e)) => {
                if e.name().as_ref() == b"node" {
                    node_stack.pop();
                    in_dependencies = node_stack.iter().any(|node| node == "Dependencies");
                    in_dependency = node_stack.iter().any(|node| node == "Dependency");
                    in_module_info = node_stack.iter().any(|node| node == "ModuleInfo");
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    ModMeta {
        dependencies: deps,
        tags,
        created_at,
        uuid,
        folder,
        name,
        version,
        md5,
        author,
        description,
        publish_handle,
        module_type,
    }
}

pub fn read_meta_lsx(path: &Path) -> Option<ModMeta> {
    let bytes = fs::read(path).ok()?;
    Some(parse_meta_lsx(&bytes))
}

pub fn read_meta_lsx_from_pak(path: &Path) -> Option<ModMeta> {
    if let Some(mut meta) = read_meta_lsx_from_pak_custom(path) {
        fill_dependency_fallback(&mut meta, path);
        return Some(meta);
    }
    if let Ok(file) = fs::File::open(path) {
        if let Ok(lspk) = lspk::Reader::new(file).and_then(|mut reader| reader.read()) {
            if let Ok(meta) = lspk.extract_meta_lsx() {
                let mut parsed = parse_meta_lsx(&meta.decompressed_bytes);
                fill_dependency_fallback(&mut parsed, path);
                return Some(parsed);
            }
        }
    }
    read_meta_lsx_from_pak_fuzzy(path).map(|mut meta| {
        fill_dependency_fallback(&mut meta, path);
        meta
    })
}

fn read_meta_lsx_from_pak_fuzzy(path: &Path) -> Option<ModMeta> {
    let bytes = fs::read(path).ok()?;
    let xml_start = find_bytes(&bytes, b"<?xml")?;
    let xml_end = find_bytes(&bytes[xml_start..], b"</save>")?;
    let end = xml_start + xml_end + b"</save>".len();
    let slice = &bytes[xml_start..end];
    let meta = parse_meta_lsx(slice);
    if !meta.dependencies.is_empty()
        || meta.uuid.is_some()
        || meta.folder.is_some()
        || meta.name.is_some()
    {
        return Some(meta);
    }
    read_meta_lsx_from_pak_raw(&bytes)
}

fn read_meta_lsx_from_pak_raw(bytes: &[u8]) -> Option<ModMeta> {
    let deps = scan_dependency_uuids(bytes);
    if deps.is_empty() {
        return None;
    }
    Some(ModMeta {
        dependencies: deps,
        ..ModMeta::default()
    })
}

fn scan_dependency_uuids(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let needle = b"Dependencies";
    let window_size = 4096usize;
    let mut offset = 0usize;
    while let Some(index) = find_bytes(&bytes[offset..], needle) {
        let start = offset + index;
        let end = (start + window_size).min(bytes.len());
        let window = &bytes[start..end];
        let mut found = extract_uuid_suffix_strings(window);
        if found.is_empty() {
            found = extract_uuid_strings(window);
        }
        out.extend(found);
        offset = start + needle.len();
    }
    out.sort();
    out.dedup();
    out
}

fn extract_uuid_strings(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let len = bytes.len();
    let uuid_len = 36;
    let mut i = 0usize;
    while i + uuid_len <= len {
        let slice = &bytes[i..i + uuid_len];
        if is_uuid_bytes(slice) {
            if let Ok(value) = std::str::from_utf8(slice) {
                out.push(value.to_ascii_lowercase());
            }
            i += uuid_len;
        } else {
            i += 1;
        }
    }
    out
}

fn extract_uuid_suffix_strings(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let len = bytes.len();
    let uuid_len = 36;
    let mut i = 0usize;
    while i + uuid_len + 1 <= len {
        if bytes[i] == b'_' && is_uuid_bytes(&bytes[i + 1..i + 1 + uuid_len]) {
            if let Ok(value) = std::str::from_utf8(&bytes[i + 1..i + 1 + uuid_len]) {
                out.push(value.to_ascii_lowercase());
            }
            i += uuid_len + 1;
        } else {
            i += 1;
        }
    }
    out
}

fn is_uuid_bytes(bytes: &[u8]) -> bool {
    const DASHES: [usize; 4] = [8, 13, 18, 23];
    if bytes.len() != 36 {
        return false;
    }
    for (i, &byte) in bytes.iter().enumerate() {
        if DASHES.contains(&i) {
            if byte != b'-' {
                return false;
            }
        } else if !byte.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

fn fill_dependency_fallback(meta: &mut ModMeta, path: &Path) {
    if !meta.dependencies.is_empty() {
        return;
    }
    if let Ok(bytes) = fs::read(path) {
        let deps = scan_dependency_uuids(&bytes);
        if !deps.is_empty() {
            meta.dependencies = deps;
        }
    }
}

pub fn find_meta_lsx(root: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<(bool, usize, PathBuf)> = Vec::new();
    for entry in WalkDir::new(root).max_depth(6) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if !name.eq_ignore_ascii_case("meta.lsx") {
            continue;
        }
        let has_mods = entry
            .path()
            .ancestors()
            .filter_map(|ancestor| ancestor.file_name())
            .any(|name| name.to_string_lossy().eq_ignore_ascii_case("Mods"));
        candidates.push((has_mods, entry.depth(), entry.path().to_path_buf()));
    }
    candidates.sort_by_key(|(has_mods, depth, _)| (!*has_mods, *depth));
    candidates.first().map(|(_, _, path)| path.clone())
}

pub fn find_info_json(root: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<(usize, usize, PathBuf)> = Vec::new();
    for entry in WalkDir::new(root).max_depth(6) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let priority = match name.as_str() {
            "info.json" => 0,
            "mod.json" => 1,
            "modinfo.json" => 2,
            _ => continue,
        };
        candidates.push((priority, entry.depth(), entry.path().to_path_buf()));
    }
    candidates.sort_by_key(|(priority, depth, _)| (*priority, *depth));
    candidates.first().map(|(_, _, path)| path.clone())
}

pub fn read_json_mods(path: &Path) -> Vec<JsonModInfo> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return Vec::new(),
    };
    let value: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    parse_json_mods(&value)
}

fn parse_json_mods(value: &Value) -> Vec<JsonModInfo> {
    let mut out = Vec::new();
    if let Some(mods) = value.get("Mods").and_then(|v| v.as_array()) {
        for entry in mods {
            if let Some(info) = parse_json_mod(entry) {
                out.push(info);
            }
        }
        return out;
    }
    if let Some(info) = parse_json_mod(value) {
        out.push(info);
    }
    out
}

fn parse_json_mod(value: &Value) -> Option<JsonModInfo> {
    let obj = value.as_object()?;
    let created_at = obj
        .get("Created")
        .or_else(|| obj.get("created"))
        .and_then(|v| v.as_str())
        .and_then(parse_created_at);
    let dependencies = parse_json_dependencies(obj);
    Some(JsonModInfo {
        uuid: obj
            .get("UUID")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        folder: obj
            .get("Folder")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        name: obj
            .get("Name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        created_at,
        dependencies,
    })
}

fn parse_json_dependencies(obj: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["Dependencies", "dependencies", "RequiredMods", "requiredMods"] {
        if let Some(value) = obj.get(key) {
            out.extend(parse_json_dependency_list(value));
        }
    }
    out.sort();
    out.dedup();
    out
}

fn parse_json_dependency_list(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let Some(entries) = value.as_array() else {
        return out;
    };
    for entry in entries {
        if let Some(value) = entry.as_str() {
            if !value.trim().is_empty() {
                out.push(value.trim().to_string());
            }
            continue;
        }
        let Some(obj) = entry.as_object() else {
            continue;
        };
        for key in ["UUID", "Uuid", "ModUUID", "mod_uuid", "uuid"] {
            if let Some(value) = obj.get(key).and_then(|value| value.as_str()) {
                if !value.trim().is_empty() {
                    out.push(value.trim().to_string());
                }
            }
        }
    }
    out
}

pub fn parse_created_at_value(value: &str) -> Option<i64> {
    parse_created_at(value)
}

fn parse_created_at(value: &str) -> Option<i64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(dt) = OffsetDateTime::parse(trimmed, &Rfc3339) {
        return Some(dt.unix_timestamp());
    }
    let naive_format =
        time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]");
    if let Ok(dt) = PrimitiveDateTime::parse(trimmed, &naive_format) {
        return Some(dt.assume_utc().unix_timestamp());
    }
    let spaced_format =
        time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    if let Ok(dt) = PrimitiveDateTime::parse(trimmed, &spaced_format) {
        return Some(dt.assume_utc().unix_timestamp());
    }
    let date_format = time::macros::format_description!("[year]-[month]-[day]");
    if let Ok(date) = Date::parse(trimmed, &date_format) {
        return date
            .with_hms(0, 0, 0)
            .ok()
            .map(|dt| dt.assume_utc().unix_timestamp());
    }
    None
}

#[derive(Debug, Clone)]
struct PakIndexEntry {
    path: String,
    offset: u64,
    compressed_size: u32,
    decompressed_size: u32,
    compression: CompressionType,
}

#[derive(Debug, Clone, Copy)]
enum CompressionType {
    None,
    Zlib,
    Lz4,
    Zstd,
}

fn read_meta_lsx_from_pak_custom(path: &Path) -> Option<ModMeta> {
    let entries = read_pak_index_entries(path)?;
    let mut meta_entry = entries.iter().find(|entry| {
        let lower = entry.path.to_ascii_lowercase();
        lower.ends_with("/meta.lsx") && lower.contains("/mods/")
    });
    if meta_entry.is_none() {
        meta_entry = entries.iter().find(|entry| {
            let lower = entry.path.to_ascii_lowercase();
            lower.ends_with("/meta.lsx") || lower == "meta.lsx"
        });
    }
    let entry = meta_entry?;

    let mut file = fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(entry.offset)).ok()?;
    let mut compressed = vec![0u8; entry.compressed_size as usize];
    file.read_exact(&mut compressed).ok()?;
    let bytes = match entry.compression {
        CompressionType::None => compressed,
        CompressionType::Lz4 => decompress(&compressed, entry.decompressed_size as usize).ok()?,
        CompressionType::Zlib => {
            let mut decoder = ZlibDecoder::new(compressed.as_slice());
            let mut out = vec![0u8; entry.decompressed_size as usize];
            decoder.read_exact(&mut out).ok()?;
            out
        }
        CompressionType::Zstd => {
            zstd_decompress(&compressed, entry.decompressed_size as usize).ok()?
        }
    };

    Some(parse_meta_lsx(&bytes))
}

fn read_pak_index_entries(path: &Path) -> Option<Vec<PakIndexEntry>> {
    const ENTRY_LEN: usize = 272;
    const PATH_LEN: usize = 256;
    const MIN_VERSION: u32 = 18;

    let mut file = fs::File::open(path).ok()?;
    let mut id = [0u8; 4];
    file.read_exact(&mut id).ok()?;
    if &id != b"LSPK" {
        return None;
    }
    let version = read_u32(&mut file)?;
    if version < MIN_VERSION {
        return None;
    }
    let footer_offset = read_u64(&mut file)?;
    let footer_offset = i64::try_from(footer_offset).ok()?;
    file.seek(SeekFrom::Start(0)).ok()?;
    file.seek(SeekFrom::Current(footer_offset)).ok()?;

    let file_count = read_u32(&mut file)? as usize;
    let compressed_len = read_u32(&mut file)? as usize;
    let decompressed_len = file_count.saturating_mul(ENTRY_LEN);

    let mut compressed = vec![0u8; compressed_len];
    file.read_exact(&mut compressed).ok()?;
    let table = match decompress(&compressed, decompressed_len) {
        Ok(table) => table,
        Err(_) => zstd_decompress(&compressed, decompressed_len).ok()?,
    };

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

        let offset_upper = u32::from_le_bytes(entry[256..260].try_into().unwrap_or([0; 4]));
        let offset_lower = u16::from_le_bytes(entry[260..262].try_into().unwrap_or([0; 2]));
        let offset = u64::from(offset_upper) | (u64::from(offset_lower) << 32);
        let offset = offset & 0x000f_ffff_ffff_ffff;

        let compression = match entry[263] & 0x0F {
            0 => CompressionType::None,
            1 => CompressionType::Zlib,
            2 => CompressionType::Lz4,
            _ => CompressionType::Zstd,
        };
        let compressed_size = u32::from_le_bytes(entry[264..268].try_into().unwrap_or([0; 4]));
        let decompressed_size = u32::from_le_bytes(entry[268..272].try_into().unwrap_or([0; 4]));

        out.push(PakIndexEntry {
            path,
            offset,
            compressed_size,
            decompressed_size,
            compression,
        });
    }

    Some(out)
}

fn read_u32(file: &mut fs::File) -> Option<u32> {
    let mut bytes = [0u8; 4];
    file.read_exact(&mut bytes).ok()?;
    Some(u32::from_le_bytes(bytes))
}

fn read_u64(file: &mut fs::File) -> Option<u64> {
    let mut bytes = [0u8; 8];
    file.read_exact(&mut bytes).ok()?;
    Some(u64::from_le_bytes(bytes))
}

fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches('/')
        .to_ascii_lowercase()
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn attr_value(e: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> Option<String> {
    for attr in e.attributes().flatten() {
        if attr.key.as_ref() == key {
            if let Ok(value) = attr.unescape_value() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn split_tags(value: &str) -> Vec<String> {
    value
        .split(|c| c == ';' || c == ',' || c == '|')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}
