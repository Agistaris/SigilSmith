//! Sets up Norbyte's Script Extender for the user: downloads the latest
//! release's DWrite.dll into the game's bin folder and turns on the DLL
//! override in BG3's Proton prefix. The DLL updates the extender itself when
//! the game starts, so this only has to run once.

use crate::bg3;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    io::{Cursor, Read},
    path::Path,
    time::Duration,
};

const RELEASES_URL: &str = "https://api.github.com/repos/Norbyte/bg3se/releases/latest";
const USER_AGENT: &str = "SigilSmith";
// The updater zip is about 5 MB; refuse anything far bigger.
const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct SetupReport {
    /// Release tag of the DLL that was installed, e.g. "v32".
    pub installed: Option<String>,
    pub override_set: bool,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    /// "sha256:<hex>", published by GitHub for each release asset.
    digest: Option<String>,
}

/// Fixes what `setup` reports as missing and SigilSmith can fix itself.
pub fn set_up(game_root: &Path, setup: &bg3::ScriptExtenderSetup) -> Result<SetupReport> {
    if bg3::game_running() {
        bail!("close Baldur's Gate 3 first");
    }
    let mut report = SetupReport::default();
    if setup.installed_check() == bg3::SetupCheck::Missing {
        let (tag, dll) = download_dll()?;
        bg3::install_script_extender_dll(game_root, &dll)?;
        report.installed = Some(tag);
    }
    if setup.can_set_override() {
        report.override_set = bg3::set_prefix_dll_override(game_root)?;
    }
    Ok(report)
}

fn download_dll() -> Result<(String, Vec<u8>)> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(60))
        .timeout_write(Duration::from_secs(10))
        .build();
    let release: Release = agent
        .get(RELEASES_URL)
        .set("User-Agent", USER_AGENT)
        .call()
        .context("look up the latest Script Extender release")?
        .into_json()
        .context("read the Script Extender release")?;
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name.starts_with("BG3SE-Updater") && asset.name.ends_with(".zip"))
        .context("the latest Script Extender release has no updater zip")?;
    let mut zip = Vec::new();
    agent
        .get(&asset.browser_download_url)
        .set("User-Agent", USER_AGENT)
        .call()
        .context("download the Script Extender")?
        .into_reader()
        .take(MAX_DOWNLOAD_BYTES + 1)
        .read_to_end(&mut zip)
        .context("download the Script Extender")?;
    if zip.len() as u64 > MAX_DOWNLOAD_BYTES {
        bail!("the Script Extender download is unexpectedly large");
    }
    if let Some(expected) = asset.digest.as_deref() {
        verify_digest(&zip, expected)?;
    }
    Ok((release.tag_name, extract_dll(&zip)?))
}

fn verify_digest(bytes: &[u8], expected: &str) -> Result<()> {
    let Some(expected) = expected.strip_prefix("sha256:") else {
        // Another algorithm: nothing to compare against.
        return Ok(());
    };
    let actual = format!("{:x}", Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("the Script Extender download doesn't match its published checksum");
    }
    Ok(())
}

fn extract_dll(zip: &[u8]) -> Result<Vec<u8>> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(zip)).context("open the Script Extender zip")?;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .context("read the Script Extender zip")?;
        let is_dll = file
            .enclosed_name()
            .and_then(|path| path.file_name().map(|name| name.to_owned()))
            .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("DWrite.dll"));
        if !is_dll {
            continue;
        }
        let mut dll = Vec::new();
        file.by_ref()
            .take(MAX_DOWNLOAD_BYTES)
            .read_to_end(&mut dll)
            .context("unpack DWrite.dll")?;
        // Every Windows DLL starts with the "MZ" header.
        if !dll.starts_with(b"MZ") {
            bail!("DWrite.dll in the Script Extender zip isn't a Windows DLL");
        }
        return Ok(dll);
    }
    bail!("the Script Extender zip has no DWrite.dll")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_with(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in files {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn extracts_the_dll_and_rejects_anything_else() {
        let zip = zip_with(&[("readme.txt", b"hi"), ("DWrite.dll", b"MZ\x90\x00dll")]);
        assert_eq!(extract_dll(&zip).unwrap(), b"MZ\x90\x00dll");

        let zip = zip_with(&[("bin/dwrite.dll", b"MZrest")]);
        assert_eq!(extract_dll(&zip).unwrap(), b"MZrest");

        let not_a_dll = zip_with(&[("DWrite.dll", b"<html>")]);
        assert!(extract_dll(&not_a_dll).is_err());
        let no_dll = zip_with(&[("other.dll", b"MZ")]);
        assert!(extract_dll(&no_dll).is_err());
        assert!(extract_dll(b"not a zip").is_err());
    }

    #[test]
    fn checks_the_published_sha256() {
        let data = b"abc";
        let good = "sha256:BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD";
        assert!(verify_digest(data, good).is_ok());
        assert!(verify_digest(b"abd", good).is_err());
        assert!(verify_digest(data, "sha512:whatever").is_ok());
    }
}
