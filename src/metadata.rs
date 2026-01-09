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
}

#[derive(Debug, Clone)]
pub struct JsonModInfo {
    pub uuid: Option<String>,
    pub folder: Option<String>,
    pub name: Option<String>,
    pub created_at: Option<i64>,
}

pub fn parse_meta_lsx(bytes: &[u8]) -> ModMeta {
    let mut reader = Reader::from_reader(bytes);
    reader.trim_text(true);
    let mut buf = Vec::new();
    let mut node_stack: Vec<String> = Vec::new();
    let mut deps = Vec::new();
    let mut tags = Vec::new();
    let mut created_at: Option<i64> = None;
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
                            if id == "Tags" && !value.trim().is_empty() {
                                tags.extend(split_tags(&value));
                            }
                            if id == "Created" {
                                if let Some(parsed) = parse_created_at(&value) {
                                    created_at = Some(match created_at {
                                        Some(existing) => existing.min(parsed),
                                        None => parsed,
                                    });
                                }
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
    }
}

pub fn read_meta_lsx(path: &Path) -> Option<ModMeta> {
    let bytes = fs::read(path).ok()?;
    Some(parse_meta_lsx(&bytes))
}

pub fn read_meta_lsx_from_pak(path: &Path) -> Option<ModMeta> {
    if let Some(meta) = read_meta_lsx_from_pak_custom(path) {
        return Some(meta);
    }
    let file = fs::File::open(path).ok()?;
    let lspk = lspk::Reader::new(file).ok()?.read().ok()?;
    let meta = lspk.extract_meta_lsx().ok()?;
    Some(parse_meta_lsx(&meta.decompressed_bytes))
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
    if created_at.is_none() {
        return None;
    }
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
    })
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
