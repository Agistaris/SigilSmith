use anyhow::{bail, Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub const GAME_NAME: &str = "Baldur's Gate 3";
const STEAM_APP_ID: &str = "1086940";
// When Steam runs BG3 through Proton, the Windows build keeps user data in the
// prefix under the steamapps/ of the library the game is installed in.
const PROTON_LARIAN_SUBDIR: &str = "pfx/drive_c/users/steamuser/AppData/Local/Larian Studios";

#[derive(Debug, Clone)]
pub struct GamePaths {
    pub game_root: PathBuf,
    pub data_dir: PathBuf,
    pub larian_dir: PathBuf,
    pub larian_mods_dir: PathBuf,
    pub modsettings_path: PathBuf,
    #[allow(dead_code)]
    pub profiles_dir: PathBuf,
}

pub fn detect_paths(
    game_root_override: Option<&Path>,
    larian_dir_override: Option<&Path>,
) -> Result<GamePaths> {
    let game_root = match game_root_override {
        Some(path) => path.to_path_buf(),
        None => find_game_root().context("locate BG3 game directory")?,
    };

    let larian_dir = match larian_dir_override {
        Some(path) => path.to_path_buf(),
        None => find_larian_dir(&game_root).context("locate BG3 Larian data directory")?,
    };

    let data_dir = game_root.join("Data");
    let larian_mods_dir = larian_dir.join("Mods");
    let profiles_dir = larian_dir.join("PlayerProfiles");
    let modsettings_path = profiles_dir.join("Public").join("modsettings.lsx");

    if !looks_like_game_root(&game_root) {
        bail!(
            "invalid game root: expected Data/ and bin/ in {}",
            game_root.display()
        );
    }

    if !looks_like_larian_dir(&larian_dir) {
        bail!(
            "invalid Larian data dir: expected PlayerProfiles/ in {}",
            larian_dir.display()
        );
    }

    Ok(GamePaths {
        game_root,
        data_dir,
        larian_dir,
        larian_mods_dir,
        modsettings_path,
        profiles_dir,
    })
}

/// Larian data dir candidates, most likely first. Steam runs the native Linux
/// build unless a compatibility tool is forced for BG3, so the native path
/// leads unless Proton is forced; Proton prefixes start with the game's library.
pub fn larian_dir_candidates(game_root: Option<&Path>) -> Vec<PathBuf> {
    match dirs_home() {
        Some(home) => larian_dir_candidates_in(&home, game_root),
        None => Vec::new(),
    }
}

/// Larian folders to offer when picking one, each with where it comes from: Steam's native
/// build and Proton prefixes first, then other launchers' Wine prefixes (read-only).
pub fn larian_dir_suggestions(game_root: Option<&Path>) -> Vec<(String, PathBuf)> {
    match dirs_home() {
        Some(home) => larian_dir_suggestions_in(&home, game_root),
        None => Vec::new(),
    }
}

pub(crate) fn larian_dir_suggestions_in(
    home: &Path,
    game_root: Option<&Path>,
) -> Vec<(String, PathBuf)> {
    let native = native_larian_dir(home);
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    for dir in larian_dir_candidates_in(home, game_root) {
        if !looks_like_larian_dir(&dir) {
            continue;
        }
        let label = if same_path(&dir, &native) {
            "Steam, native build"
        } else {
            "Steam, Proton"
        };
        out.push((label.to_string(), dir));
    }
    for item in crate::launchers::larian_dirs(home) {
        if !out.iter().any(|(_, dir)| same_path(dir, &item.larian_dir)) {
            out.push((item.launcher.to_string(), item.larian_dir));
        }
    }
    out
}

/// The Larian data dir Steam's launch settings say BG3 reads, when it exists
/// and differs from `larian_dir`.
pub fn larian_dir_mismatch(game_root: &Path, larian_dir: &Path) -> Option<PathBuf> {
    larian_dir_mismatch_in(&dirs_home()?, game_root, larian_dir)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SteamRuntime {
    Native,
    Proton,
    Unknown,
}

fn find_game_root() -> Option<PathBuf> {
    find_game_root_in(&dirs_home()?)
}

fn find_game_root_in(home: &Path) -> Option<PathBuf> {
    for lib in steam_libraries(home) {
        for folder in ["Baldurs Gate 3", "Baldur's Gate 3"] {
            let candidate = lib.join("steamapps/common").join(folder);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    None
}

fn find_larian_dir(game_root: &Path) -> Option<PathBuf> {
    let candidates = larian_dir_candidates(Some(game_root));
    candidates
        .iter()
        .find(|path| looks_like_larian_dir(path))
        .or_else(|| candidates.iter().find(|path| path.exists()))
        .cloned()
}

fn larian_dir_candidates_in(home: &Path, game_root: Option<&Path>) -> Vec<PathBuf> {
    let native = native_larian_dir(home);
    let mut proton = Vec::new();
    if let Some(dir) = game_root.and_then(proton_larian_dir_for_game_root) {
        proton.push(dir);
    }
    for lib in steam_libraries(home) {
        proton.push(proton_larian_dir(&lib.join("steamapps")));
    }

    let mut candidates = Vec::new();
    if steam_runtime(home) == SteamRuntime::Proton {
        candidates.extend(proton);
        candidates.push(native);
    } else {
        candidates.push(native);
        candidates.extend(proton);
    }

    dedup_paths(candidates)
}

fn larian_dir_mismatch_in(home: &Path, game_root: &Path, larian_dir: &Path) -> Option<PathBuf> {
    let proton = proton_larian_dir_for_game_root(game_root)?;
    let expected = match steam_runtime(home) {
        SteamRuntime::Proton => proton,
        SteamRuntime::Native => native_larian_dir(home),
        SteamRuntime::Unknown => return None,
    };
    if !looks_like_larian_dir(&expected) || same_path(&expected, larian_dir) {
        return None;
    }
    Some(expected)
}

fn native_larian_dir(home: &Path) -> PathBuf {
    home.join(".local/share/Larian Studios").join(GAME_NAME)
}

fn proton_larian_dir(steamapps: &Path) -> PathBuf {
    steamapps
        .join("compatdata")
        .join(STEAM_APP_ID)
        .join(PROTON_LARIAN_SUBDIR)
        .join(GAME_NAME)
}

fn proton_larian_dir_for_game_root(game_root: &Path) -> Option<PathBuf> {
    Some(proton_larian_dir(&steamapps_for_game_root(game_root)?))
}

/// The steamapps/ folder of a Steam install; None for other launchers.
fn steamapps_for_game_root(game_root: &Path) -> Option<PathBuf> {
    let common = game_root.parent()?;
    let steamapps = common.parent()?;
    if common.file_name()? != "common" || steamapps.file_name()? != "steamapps" {
        return None;
    }
    Some(steamapps.to_path_buf())
}

fn steam_roots(home: &Path) -> Vec<PathBuf> {
    dedup_paths(vec![
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        // Flatpak and Snap installs keep Steam inside their sandboxed home.
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        home.join("snap/steam/common/.local/share/Steam"),
    ])
}

fn steam_libraries(home: &Path) -> Vec<PathBuf> {
    let mut libraries = Vec::new();
    for root in steam_roots(home) {
        let vdf = root.join("steamapps/libraryfolders.vdf");
        if vdf.exists() {
            if let Ok(paths) = parse_steam_library_paths(&vdf) {
                libraries.extend(paths);
            }
        }
        libraries.push(root);
    }

    dedup_paths(libraries)
}

// A per-game CompatToolMapping entry forces Proton; the global "0" entry only
// covers titles without a Linux build, which no longer includes BG3.
fn steam_runtime(home: &Path) -> SteamRuntime {
    let mut runtime = SteamRuntime::Unknown;
    for root in steam_roots(home) {
        let Ok(raw) = fs::read_to_string(root.join("config/config.vdf")) else {
            continue;
        };
        if forced_compat_tool(&raw, STEAM_APP_ID).is_some() {
            return SteamRuntime::Proton;
        }
        runtime = SteamRuntime::Native;
    }
    runtime
}

fn forced_compat_tool(config_vdf: &str, app_id: &str) -> Option<String> {
    let mut lines = config_vdf.lines().map(str::trim);
    lines.find(|line| *line == "\"CompatToolMapping\"")?;

    let app_key = format!("\"{app_id}\"");
    let mut depth = 0;
    let mut in_app = false;
    for line in lines {
        if line == "{" {
            depth += 1;
        } else if line == "}" {
            depth -= 1;
            if depth <= 0 {
                return None;
            }
            in_app = false;
        } else if depth == 1 {
            in_app = line == app_key;
        } else if depth == 2 && in_app {
            let parts: Vec<&str> = line.split('"').collect();
            if parts.len() >= 4 && parts[1] == "name" && !parts[3].is_empty() {
                return Some(parts[3].to_string());
            }
        }
    }

    None
}

fn parse_steam_library_paths(path: &Path) -> Result<Vec<PathBuf>> {
    let raw = fs::read_to_string(path).context("read libraryfolders.vdf")?;
    let mut paths = Vec::new();

    for line in raw.lines() {
        let line = line.trim();
        if !line.contains("\"path\"") {
            continue;
        }

        let parts: Vec<&str> = line.split('"').collect();
        if parts.len() >= 4 {
            let path = parts[3].replace("\\\\", "\\");
            paths.push(PathBuf::from(path));
        }
    }

    Ok(paths)
}

// Keeps the first spelling of each path; ~/.steam/steam is usually a symlink
// to ~/.local/share/Steam.
fn dedup_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = Vec::new();
    let mut unique = Vec::new();
    for path in paths {
        let key = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !seen.contains(&key) {
            seen.push(key);
            unique.push(path);
        }
    }
    unique
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub fn looks_like_game_root(path: &Path) -> bool {
    path.join("Data").is_dir() && path.join("bin").is_dir()
}

pub fn looks_like_larian_dir(path: &Path) -> bool {
    path.join("PlayerProfiles").is_dir()
}

/// Steam launch option that makes Proton load the Script Extender's DWrite.dll
/// instead of Wine's own.
pub const SCRIPT_EXTENDER_LAUNCH_OPTION: &str = "WINEDLLOVERRIDES=\"DWrite.dll=n,b\" %command%";
const SCRIPT_EXTENDER_DLL_OVERRIDE: &str = "DWrite.dll=n,b";
// SteamID64 of account id 0; userdata/ folders are named by account id.
const STEAM_ID64_BASE: u64 = 76_561_197_960_265_728;
// The same entry winecfg writes for "dwrite (native, builtin)".
const PREFIX_DLL_OVERRIDE_ENTRY: &str = "\"dwrite\"=\"native,builtin\"";
// Steam's reaper carries the app id; the rest are the game and its launcher.
const GAME_PROCESS_MARKERS: [&str; 4] = [
    "appid=1086940",
    "bg3.exe",
    "bg3_dx11.exe",
    "larilauncher.exe",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupCheck {
    Ok,
    Missing,
    Unknown,
}

/// What the Script Extender needs on Linux. `None` means SigilSmith can't
/// tell (not a Steam install, or Steam's config is unreadable); unknowns never
/// count as problems.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScriptExtenderSetup {
    pub uses_proton: Option<bool>,
    pub installed: bool,
    pub launch_options: Option<String>,
    /// DWrite set to native in the Proton prefix (winecfg/protontricks), which
    /// works without the launch option.
    pub prefix_override: bool,
    /// Steam has created BG3's Proton prefix, so SigilSmith can set the
    /// override there.
    pub prefix_exists: bool,
    /// The game isn't in a Steam library (Lutris, Heroic, Bottles, GOG...):
    /// SigilSmith can't read that launcher's Wine settings.
    pub other_launcher: bool,
    /// BG3LE, the Script Extender for the native Linux build, when installed.
    pub native_extender: Option<NativeExtender>,
}

/// BG3LE (github.com/lenonk/bg3le, Nexus mod 25431): its installer puts a
/// launch wrapper in `~/.local/share/bg3le` and in front of `%command%` in
/// BG3's Steam launch options. SigilSmith only reads both.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeExtender {
    /// From its `version` file; a build from source has none.
    pub version: Option<String>,
    pub wrapper: PathBuf,
}

pub const BG3LE_WRAPPER_NAME: &str = "bg3le-launch";
/// The variable another launcher has to set for the Script Extender's DLL.
pub const LAUNCHER_DLL_OVERRIDE: &str = "WINEDLLOVERRIDES=\"DWrite.dll=n,b\"";

impl ScriptExtenderSetup {
    /// Steam runs the native build and BG3LE is installed: the Script Extender
    /// comes from BG3LE, so Proton, DWrite.dll and its override don't apply.
    pub fn uses_bg3le(&self) -> bool {
        self.uses_proton == Some(false) && self.native_extender.is_some()
    }

    pub fn proton_check(&self) -> SetupCheck {
        match self.uses_proton {
            Some(true) => SetupCheck::Ok,
            Some(false) if self.uses_bg3le() => SetupCheck::Unknown,
            Some(false) => SetupCheck::Missing,
            None => SetupCheck::Unknown,
        }
    }

    pub fn installed_check(&self) -> SetupCheck {
        if self.installed || self.uses_bg3le() {
            SetupCheck::Ok
        } else {
            SetupCheck::Missing
        }
    }

    pub fn launch_option_check(&self) -> SetupCheck {
        if self.uses_bg3le() {
            return match &self.launch_options {
                Some(options) if options.contains(BG3LE_WRAPPER_NAME) => SetupCheck::Ok,
                Some(_) => SetupCheck::Missing,
                None => SetupCheck::Unknown,
            };
        }
        if self.prefix_override {
            return SetupCheck::Ok;
        }
        match &self.launch_options {
            Some(options) if launch_options_have_dll_override(options) => SetupCheck::Ok,
            Some(_) => SetupCheck::Missing,
            None => SetupCheck::Unknown,
        }
    }

    /// The DLL is in place for a launcher whose Wine settings SigilSmith can't
    /// read: the player has to check the override there.
    pub fn needs_launcher_check(&self) -> bool {
        self.other_launcher && self.installed
    }

    pub fn is_ready(&self) -> bool {
        self.problem().is_none()
    }

    /// The first missing piece, short enough for the Details panel.
    pub fn problem(&self) -> Option<&'static str> {
        if self.proton_check() == SetupCheck::Missing {
            Some("needs Proton or BG3LE")
        } else if self.installed_check() == SetupCheck::Missing {
            Some("not installed")
        } else if self.launch_option_check() == SetupCheck::Missing {
            Some(if self.uses_bg3le() {
                "BG3LE launch option missing"
            } else {
                "launch option missing"
            })
        } else {
            None
        }
    }

    /// SigilSmith can set the override in the Proton prefix itself.
    pub fn can_set_override(&self) -> bool {
        !self.uses_bg3le()
            && self.launch_option_check() == SetupCheck::Missing
            && self.prefix_exists
    }

    /// Something "Set up for me" can fix: the DLL or the prefix override.
    pub fn can_set_up(&self) -> bool {
        self.installed_check() == SetupCheck::Missing || self.can_set_override()
    }

    /// The current launch options plus what the Script Extender needs, ready
    /// to paste: BG3LE's wrapper on the native build, else the DLL override.
    pub fn suggested_launch_options(&self) -> String {
        let current = self.launch_options.as_deref().unwrap_or("");
        match &self.native_extender {
            Some(extender) if self.uses_bg3le() => {
                launch_options_with_wrapper(current, &extender.wrapper)
            }
            _ => launch_options_with_dll_override(current),
        }
    }
}

pub fn script_extender_setup(game_root: &Path) -> ScriptExtenderSetup {
    let home = dirs_home();
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.as_ref().map(|home| home.join(".local/share")));
    script_extender_setup_with(home.as_deref(), data_home.as_deref(), game_root)
}

#[cfg(test)]
fn script_extender_setup_in(home: Option<&Path>, game_root: &Path) -> ScriptExtenderSetup {
    let data_home = home.map(|home| home.join(".local/share"));
    script_extender_setup_with(home, data_home.as_deref(), game_root)
}

pub(crate) fn script_extender_setup_with(
    home: Option<&Path>,
    data_home: Option<&Path>,
    game_root: &Path,
) -> ScriptExtenderSetup {
    let mut setup = ScriptExtenderSetup {
        installed: dir_has_file(&game_root.join("bin"), "dwrite.dll"),
        ..ScriptExtenderSetup::default()
    };
    // Proton and launch options only apply to Steam installs.
    if steamapps_for_game_root(game_root).is_none() {
        setup.other_launcher = true;
        return setup;
    }
    setup.native_extender = data_home.and_then(bg3le_install);
    if let Some(user_reg) = proton_prefix_user_reg(game_root) {
        setup.prefix_exists = true;
        setup.prefix_override = fs::read(user_reg)
            .map(|raw| prefix_has_dll_override(&String::from_utf8_lossy(&raw)))
            .unwrap_or(false);
    }
    let Some(home) = home else {
        return setup;
    };
    setup.uses_proton = match steam_runtime(home) {
        SteamRuntime::Proton => Some(true),
        SteamRuntime::Native => Some(false),
        SteamRuntime::Unknown => None,
    };
    setup.launch_options = steam_roots(home)
        .iter()
        .find_map(|root| steam_launch_options(root, STEAM_APP_ID));
    setup
}

/// BG3LE as its installer leaves it: the library and the launch wrapper in
/// `<data home>/bg3le`.
fn bg3le_install(data_home: &Path) -> Option<NativeExtender> {
    let root = data_home.join("bg3le");
    let wrapper = root.join("bin").join(BG3LE_WRAPPER_NAME);
    if !root.join("lib/libbg3le.so").is_file() || !wrapper.is_file() {
        return None;
    }
    let version = fs::read_to_string(root.join("version"))
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|version| !version.is_empty());
    Some(NativeExtender { version, wrapper })
}

/// Puts BG3LE's wrapper in front of `%command%`, the way its installer does.
pub fn launch_options_with_wrapper(existing: &str, wrapper: &Path) -> String {
    let existing = existing.trim();
    if existing.contains(BG3LE_WRAPPER_NAME) {
        return existing.to_string();
    }
    let token = format!("\"{}\"", wrapper.display());
    match existing.find("%command%") {
        Some(index) => format!("{}{token} {}", &existing[..index], &existing[index..]),
        // Options without %command% are arguments Steam passes to the game.
        None => format!("{token} %command% {existing}").trim().to_string(),
    }
}

/// The registry file of BG3's Proton prefix, once Steam has created it.
fn proton_prefix_user_reg(game_root: &Path) -> Option<PathBuf> {
    let path = steamapps_for_game_root(game_root)?
        .join("compatdata")
        .join(STEAM_APP_ID)
        .join("pfx/user.reg");
    path.is_file().then_some(path)
}

/// BG3 keeps an empty `ModCrashSanityCheck` folder in its Larian data folder while it runs and
/// removes it on a clean exit. When it finds one at startup it turns every mod off, so a crash (or a
/// killed game) resets the load order. Script Extender removes it at each launch; this does
/// the same while the game isn't running. Returns the folder when one was removed.
pub fn clear_crash_marker(larian_dir: &Path) -> Option<PathBuf> {
    clear_crash_marker_in(larian_dir, game_running())
}

fn clear_crash_marker_in(larian_dir: &Path, running: bool) -> Option<PathBuf> {
    let path = larian_dir.join("ModCrashSanityCheck");
    let is_dir = fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir());
    if !is_dir || running {
        return None;
    }
    // Only an empty folder, as the game leaves it.
    fs::remove_dir(&path).ok().map(|()| path)
}

/// BG3 or its launcher running, or Wine still holding the game's prefix.
/// Wine writes its registry back when it exits, which would undo an edit made
/// in the meantime.
pub fn game_running() -> bool {
    let Ok(entries) = fs::read_dir("/proc") else {
        return false;
    };
    entries.flatten().any(|entry| {
        let Ok(raw) = fs::read(entry.path().join("cmdline")) else {
            return false;
        };
        let cmdline = String::from_utf8_lossy(&raw).to_ascii_lowercase();
        if GAME_PROCESS_MARKERS
            .iter()
            .any(|marker| cmdline.contains(marker))
        {
            return true;
        }
        if !cmdline.contains("wineserver") {
            return false;
        }
        fs::read(entry.path().join("environ"))
            .map(|env| {
                String::from_utf8_lossy(&env).contains(&format!("compatdata/{STEAM_APP_ID}/pfx"))
            })
            .unwrap_or(false)
    })
}

/// Puts the Script Extender's DWrite.dll in the game's bin folder, replacing
/// any copy already there.
pub fn install_script_extender_dll(game_root: &Path, dll: &[u8]) -> Result<PathBuf> {
    let bin = game_root.join("bin");
    if !bin.is_dir() {
        bail!("no bin folder in {}", game_root.display());
    }
    let target = bin.join("DWrite.dll");
    let temp = bin.join(".DWrite.dll.sigilsmith-tmp");
    fs::write(&temp, dll).context("write DWrite.dll")?;
    fs::rename(&temp, &target).context("replace DWrite.dll")?;
    // Linux is case-sensitive: don't leave an old dwrite.dll next to the new one.
    for entry in fs::read_dir(&bin).context("read bin folder")?.flatten() {
        let name = entry.file_name();
        if name != "DWrite.dll" && name.to_string_lossy().eq_ignore_ascii_case("dwrite.dll") {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(target)
}

/// Sets DWrite to native in BG3's Proton prefix. Returns false when Steam
/// hasn't created the prefix yet. Keeps the previous file as
/// user.reg.sigilsmith-backup.
pub fn set_prefix_dll_override(game_root: &Path) -> Result<bool> {
    let Some(user_reg) = proton_prefix_user_reg(game_root) else {
        return Ok(false);
    };
    let raw = fs::read(&user_reg).context("read the Proton prefix registry")?;
    let text = String::from_utf8(raw).context("the Proton prefix registry isn't text")?;
    if prefix_has_dll_override(&text) {
        return Ok(true);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let updated = with_prefix_dll_override(&text, now);
    fs::copy(
        &user_reg,
        user_reg.with_file_name("user.reg.sigilsmith-backup"),
    )
    .context("back up the Proton prefix registry")?;
    let temp = user_reg.with_file_name(".user.reg.sigilsmith-tmp");
    fs::write(&temp, updated).context("write the Proton prefix registry")?;
    fs::rename(&temp, &user_reg).context("replace the Proton prefix registry")?;
    Ok(true)
}

/// Adds the DWrite override to a Wine user.reg, replacing any other DWrite
/// entry, and creates the DllOverrides key when it's missing.
fn with_prefix_dll_override(user_reg: &str, now_unix: u64) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_overrides = false;
    let mut pending = false;
    let mut added = false;
    for line in user_reg.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if pending {
                out.push(PREFIX_DLL_OVERRIDE_ENTRY.to_string());
                pending = false;
            }
            in_overrides = is_dll_overrides_header(trimmed);
            if in_overrides && !added {
                pending = true;
                added = true;
            }
            out.push(line.to_string());
            continue;
        }
        if in_overrides {
            // Key metadata (#time=...) stays right under the header.
            if pending && !trimmed.starts_with('#') {
                out.push(PREFIX_DLL_OVERRIDE_ENTRY.to_string());
                pending = false;
            }
            if let Some((name, _)) = trimmed.split_once('=') {
                if is_dwrite(name.trim_matches('"')) {
                    continue;
                }
            }
        }
        out.push(line.to_string());
    }
    if pending {
        out.push(PREFIX_DLL_OVERRIDE_ENTRY.to_string());
    }
    if !added {
        // Windows FILETIME: 100 ns steps since 1601.
        let filetime = (now_unix + 11_644_473_600) * 10_000_000;
        if out.last().is_some_and(|line| !line.trim().is_empty()) {
            out.push(String::new());
        }
        out.push(format!("[Software\\\\Wine\\\\DllOverrides] {now_unix}"));
        out.push(format!("#time={filetime:x}"));
        out.push(PREFIX_DLL_OVERRIDE_ENTRY.to_string());
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

fn is_dll_overrides_header(line: &str) -> bool {
    line.replace('\\', "")
        .to_ascii_lowercase()
        .starts_with("[softwarewinedlloverrides]")
}

fn dir_has_file(dir: &Path, name: &str) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry
            .file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case(name)
            && entry.path().is_file()
    })
}

/// Launch options Steam keeps for an app; Some("") when there are none.
fn steam_launch_options(steam_root: &Path, app_id: &str) -> Option<String> {
    let raw = fs::read(steam_user_localconfig(steam_root)?).ok()?;
    let root = parse_vdf(&String::from_utf8_lossy(&raw));
    let app = vdf_section(
        &root,
        &[
            "UserLocalConfigStore",
            "Software",
            "Valve",
            "Steam",
            "apps",
            app_id,
        ],
    );
    Some(
        app.and_then(|entries| vdf_text(entries, "LaunchOptions"))
            .unwrap_or_default()
            .to_string(),
    )
}

/// localconfig.vdf of the account that last signed in, else the one Steam
/// saved most recently.
fn steam_user_localconfig(steam_root: &Path) -> Option<PathBuf> {
    let userdata = steam_root.join("userdata");
    if let Some(account) = most_recent_steam_account(steam_root) {
        let path = userdata
            .join(account.to_string())
            .join("config/localconfig.vdf");
        if path.is_file() {
            return Some(path);
        }
    }
    fs::read_dir(&userdata)
        .ok()?
        .flatten()
        .map(|entry| entry.path().join("config/localconfig.vdf"))
        .filter(|path| path.is_file())
        .max_by_key(|path| path.metadata().and_then(|meta| meta.modified()).ok())
}

fn most_recent_steam_account(steam_root: &Path) -> Option<u64> {
    let raw = fs::read_to_string(steam_root.join("config/loginusers.vdf")).ok()?;
    let root = parse_vdf(&raw);
    let users = vdf_section(&root, &["users"])?;
    users.iter().find_map(|(steam_id, user)| {
        let Vdf::Section(fields) = user else {
            return None;
        };
        if vdf_text(fields, "MostRecent") != Some("1") {
            return None;
        }
        steam_id.parse::<u64>().ok()?.checked_sub(STEAM_ID64_BASE)
    })
}

pub fn launch_options_have_dll_override(options: &str) -> bool {
    wine_dll_overrides(options)
        .iter()
        .any(|(_, _, value)| overrides_native_dwrite(value))
}

/// Adds the Script Extender's DLL override to existing Steam launch options,
/// keeping everything the user already has.
pub fn launch_options_with_dll_override(existing: &str) -> String {
    let existing = existing.trim();
    if launch_options_have_dll_override(existing) {
        return existing.to_string();
    }
    if let Some((start, end, value)) = wine_dll_overrides(existing).into_iter().next() {
        let mut entries = vec![SCRIPT_EXTENDER_DLL_OVERRIDE.to_string()];
        entries.extend(without_dwrite(&value));
        return format!(
            "{}WINEDLLOVERRIDES=\"{}\"{}",
            &existing[..start],
            entries.join(";"),
            &existing[end..]
        );
    }
    if existing.is_empty() {
        SCRIPT_EXTENDER_LAUNCH_OPTION.to_string()
    } else if existing.contains("%command%") {
        format!("WINEDLLOVERRIDES=\"{SCRIPT_EXTENDER_DLL_OVERRIDE}\" {existing}")
    } else {
        // Without %command%, Steam passes the options to the game as arguments.
        format!("{SCRIPT_EXTENDER_LAUNCH_OPTION} {existing}")
    }
}

/// Byte range and unquoted value of each WINEDLLOVERRIDES= assignment.
fn wine_dll_overrides(options: &str) -> Vec<(usize, usize, String)> {
    const KEY: &str = "WINEDLLOVERRIDES=";
    let mut found = Vec::new();
    let mut search = 0;
    while let Some(offset) = options[search..].find(KEY) {
        let start = search + offset;
        let value_start = start + KEY.len();
        search = value_start;
        if !options[..start].is_empty() && !options[..start].ends_with(char::is_whitespace) {
            continue;
        }
        let rest = &options[value_start..];
        let (value, len) = match rest.chars().next() {
            Some(quote @ ('"' | '\'')) => match rest[1..].find(quote) {
                Some(close) => (&rest[1..1 + close], close + 2),
                None => (&rest[1..], rest.len()),
            },
            _ => {
                let len = rest.find(char::is_whitespace).unwrap_or(rest.len());
                (&rest[..len], len)
            }
        };
        found.push((start, value_start + len, value.to_string()));
        search = value_start + len;
    }
    found
}

fn is_dwrite(name: &str) -> bool {
    let name = name.trim().trim_start_matches('*').to_ascii_lowercase();
    name == "dwrite" || name == "dwrite.dll"
}

/// Wine override lists look like "dwrite,d3d11=n,b;winhttp=n".
fn overrides_native_dwrite(value: &str) -> bool {
    value.split(';').any(|entry| {
        let Some((names, mode)) = entry.split_once('=') else {
            return false;
        };
        names.split(',').any(is_dwrite) && mode.trim().to_ascii_lowercase().starts_with('n')
    })
}

fn without_dwrite(value: &str) -> Vec<String> {
    value
        .split(';')
        .filter_map(|entry| {
            let entry = entry.trim();
            let Some((names, mode)) = entry.split_once('=') else {
                return (!entry.is_empty()).then(|| entry.to_string());
            };
            let names: Vec<&str> = names.split(',').filter(|name| !is_dwrite(name)).collect();
            (!names.is_empty()).then(|| format!("{}={mode}", names.join(",")))
        })
        .collect()
}

fn prefix_has_dll_override(user_reg: &str) -> bool {
    let mut in_overrides = false;
    for line in user_reg.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_overrides = is_dll_overrides_header(line);
            continue;
        }
        if !in_overrides {
            continue;
        }
        let Some((name, mode)) = line.split_once('=') else {
            continue;
        };
        if is_dwrite(name.trim_matches('"'))
            && mode.trim_matches('"').to_ascii_lowercase().starts_with('n')
        {
            return true;
        }
    }
    false
}

#[derive(Debug)]
enum Vdf {
    Text(String),
    Section(Vec<(String, Vdf)>),
}

enum VdfToken {
    Text(String),
    Open,
    Close,
}

/// Lenient reader for Steam's text KeyValues files; keys compare
/// case-insensitively like Steam does.
fn parse_vdf(raw: &str) -> Vec<(String, Vdf)> {
    let mut stack: Vec<(String, Vec<(String, Vdf)>)> = vec![(String::new(), Vec::new())];
    let mut key: Option<String> = None;
    for token in vdf_tokens(raw) {
        match token {
            VdfToken::Open => stack.push((key.take().unwrap_or_default(), Vec::new())),
            VdfToken::Close => {
                key = None;
                if stack.len() > 1 {
                    let (name, entries) = stack.pop().unwrap_or_default();
                    if let Some((_, parent)) = stack.last_mut() {
                        parent.push((name, Vdf::Section(entries)));
                    }
                }
            }
            VdfToken::Text(text) => match key.take() {
                None => key = Some(text),
                Some(name) => {
                    if let Some((_, entries)) = stack.last_mut() {
                        entries.push((name, Vdf::Text(text)));
                    }
                }
            },
        }
    }
    while stack.len() > 1 {
        let (name, entries) = stack.pop().unwrap_or_default();
        if let Some((_, parent)) = stack.last_mut() {
            parent.push((name, Vdf::Section(entries)));
        }
    }
    stack.pop().map(|(_, entries)| entries).unwrap_or_default()
}

fn vdf_tokens(raw: &str) -> Vec<VdfToken> {
    let mut tokens = Vec::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => tokens.push(VdfToken::Open),
            '}' => tokens.push(VdfToken::Close),
            '"' => {
                let mut text = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some('n') => text.push('\n'),
                            Some('t') => text.push('\t'),
                            Some(other) => text.push(other),
                            None => break,
                        },
                        _ => text.push(c),
                    }
                }
                tokens.push(VdfToken::Text(text));
            }
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            c if c.is_whitespace() => {}
            _ => {
                let mut text = String::from(c);
                while let Some(&next) = chars.peek() {
                    if next.is_whitespace() || matches!(next, '{' | '}' | '"') {
                        break;
                    }
                    text.push(next);
                    chars.next();
                }
                // Platform conditionals like [$WIN32] aren't keys or values.
                if !(text.starts_with('[') && text.ends_with(']')) {
                    tokens.push(VdfToken::Text(text));
                }
            }
        }
    }
    tokens
}

fn vdf_section<'a>(entries: &'a [(String, Vdf)], path: &[&str]) -> Option<&'a [(String, Vdf)]> {
    let Some((first, rest)) = path.split_first() else {
        return Some(entries);
    };
    entries
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case(first))
        .find_map(|(_, value)| match value {
            Vdf::Section(children) => vdf_section(children, rest),
            Vdf::Text(_) => None,
        })
}

fn vdf_text<'a>(entries: &'a [(String, Vdf)], key: &str) -> Option<&'a str> {
    entries.iter().find_map(|(name, value)| match value {
        Vdf::Text(text) if name.eq_ignore_ascii_case(key) => Some(text.as_str()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    const GLOBAL_PROTON_ONLY: &str = "\"InstallConfigStore\"\n{\n\t\"CompatToolMapping\"\n\t{\n\t\t\"0\"\n\t\t{\n\t\t\t\"name\"\t\t\"proton-cachyos-slr\"\n\t\t\t\"config\"\t\t\"\"\n\t\t}\n\t\t\"294100\"\n\t\t{\n\t\t\t\"name\"\t\t\"proton_experimental\"\n\t\t}\n\t}\n\t\"1086940\"\n\t{\n\t\t\"name\"\t\t\"not-a-mapping\"\n\t}\n}\n";
    const BG3_FORCED_PROTON: &str = "\"InstallConfigStore\"\n{\n\t\"CompatToolMapping\"\n\t{\n\t\t\"0\"\n\t\t{\n\t\t\t\"name\"\t\t\"proton_experimental\"\n\t\t}\n\t\t\"1086940\"\n\t\t{\n\t\t\t\"name\"\t\t\"proton-cachyos\"\n\t\t\t\"config\"\t\t\"\"\n\t\t\t\"priority\"\t\t\"250\"\n\t\t}\n\t}\n}\n";

    struct TempHome(PathBuf);

    impl TempHome {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("sigilsmith-bg3-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            TempHome(path)
        }

        fn steam(&self) -> PathBuf {
            self.0.join(".local/share/Steam")
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_steam_config(steam_root: &Path, config_vdf: &str) {
        fs::create_dir_all(steam_root.join("config")).unwrap();
        fs::write(steam_root.join("config/config.vdf"), config_vdf).unwrap();
    }

    fn make_game(library: &Path) -> PathBuf {
        let game_root = library.join("steamapps/common/Baldurs Gate 3");
        fs::create_dir_all(game_root.join("Data")).unwrap();
        fs::create_dir_all(game_root.join("bin")).unwrap();
        game_root
    }

    fn make_larian_dir(path: &Path) -> PathBuf {
        fs::create_dir_all(path.join("PlayerProfiles")).unwrap();
        path.to_path_buf()
    }

    fn pick(home: &Path, game_root: &Path) -> Option<PathBuf> {
        larian_dir_candidates_in(home, Some(game_root))
            .into_iter()
            .find(|path| looks_like_larian_dir(path))
    }

    #[test]
    fn reads_only_per_game_compat_tool_mappings() {
        assert_eq!(forced_compat_tool(GLOBAL_PROTON_ONLY, STEAM_APP_ID), None);
        assert_eq!(
            forced_compat_tool(BG3_FORCED_PROTON, STEAM_APP_ID).as_deref(),
            Some("proton-cachyos")
        );
    }

    #[test]
    fn prefers_native_dir_when_steam_runs_native_build() {
        let home = TempHome::new("native-run");
        write_steam_config(&home.steam(), GLOBAL_PROTON_ONLY);
        let game_root = make_game(&home.steam());
        let native = make_larian_dir(&native_larian_dir(&home.0));
        make_larian_dir(&proton_larian_dir(&home.steam().join("steamapps")));

        assert_eq!(pick(&home.0, &game_root), Some(native));
    }

    #[test]
    fn prefers_proton_prefix_when_steam_forces_proton() {
        let home = TempHome::new("forced-proton");
        write_steam_config(&home.steam(), BG3_FORCED_PROTON);
        let game_root = make_game(&home.steam());
        make_larian_dir(&native_larian_dir(&home.0));
        let proton = make_larian_dir(&proton_larian_dir(&home.steam().join("steamapps")));

        assert_eq!(pick(&home.0, &game_root), Some(proton));
    }

    #[test]
    fn uses_compatdata_from_the_games_library() {
        let home = TempHome::new("library");
        let library = home.0.join("games/SteamLibrary");
        write_steam_config(&home.steam(), BG3_FORCED_PROTON);
        fs::create_dir_all(home.steam().join("steamapps")).unwrap();
        fs::write(
            home.steam().join("steamapps/libraryfolders.vdf"),
            format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
                home.steam().display(),
                library.display()
            ),
        )
        .unwrap();
        let game_root = make_game(&library);
        let proton = make_larian_dir(&proton_larian_dir(&library.join("steamapps")));

        assert_eq!(find_game_root_in(&home.0), Some(game_root.clone()));
        assert_eq!(pick(&home.0, &game_root), Some(proton));
    }

    #[test]
    fn finds_flatpak_steam() {
        let home = TempHome::new("flatpak");
        let steam = home
            .0
            .join(".var/app/com.valvesoftware.Steam/.local/share/Steam");
        write_steam_config(&steam, BG3_FORCED_PROTON);
        let game_root = make_game(&steam);
        let proton = make_larian_dir(&proton_larian_dir(&steam.join("steamapps")));

        assert_eq!(find_game_root_in(&home.0), Some(game_root.clone()));
        assert_eq!(pick(&home.0, &game_root), Some(proton));
    }

    #[test]
    fn keeps_native_first_without_steam_config() {
        let home = TempHome::new("no-config");
        let game_root = make_game(&home.steam());
        let native = make_larian_dir(&native_larian_dir(&home.0));
        make_larian_dir(&proton_larian_dir(&home.steam().join("steamapps")));

        assert_eq!(pick(&home.0, &game_root), Some(native));
    }

    #[test]
    fn reports_mismatch_against_the_dir_steam_launches_with() {
        let home = TempHome::new("mismatch");
        let game_root = make_game(&home.steam());
        let proton = make_larian_dir(&proton_larian_dir(&home.steam().join("steamapps")));
        let native = make_larian_dir(&native_larian_dir(&home.0));
        let linked = home.0.join("linked");
        symlink(&proton, &linked).unwrap();

        // Unknown runtime: no warning.
        assert_eq!(larian_dir_mismatch_in(&home.0, &game_root, &native), None);

        write_steam_config(&home.steam(), BG3_FORCED_PROTON);
        assert_eq!(
            larian_dir_mismatch_in(&home.0, &game_root, &native),
            Some(proton.clone())
        );
        assert_eq!(larian_dir_mismatch_in(&home.0, &game_root, &proton), None);
        assert_eq!(larian_dir_mismatch_in(&home.0, &game_root, &linked), None);
        assert_eq!(
            larian_dir_mismatch_in(&home.0, &home.0.join("Games/BG3"), &native),
            None
        );

        write_steam_config(&home.steam(), GLOBAL_PROTON_ONLY);
        assert_eq!(
            larian_dir_mismatch_in(&home.0, &game_root, &proton),
            Some(native.clone())
        );
        assert_eq!(larian_dir_mismatch_in(&home.0, &game_root, &native), None);
    }

    fn write_launch_options(steam_root: &Path, account: u64, options: &str) {
        let config = steam_root.join(format!("userdata/{account}/config"));
        fs::create_dir_all(&config).unwrap();
        let escaped = options.replace('\\', "\\\\").replace('"', "\\\"");
        let vdf = format!(
            "\"UserLocalConfigStore\"\n{{\n\t\"apps\"\n\t{{\n\t\t\"1086940\"\n\t\t{{\n\t\t\t\"LaunchOptions\"\t\t\"wrong level\"\n\t\t}}\n\t}}\n\t\"Software\"\n\t{{\n\t\t\"Valve\"\n\t\t{{\n\t\t\t\"Steam\"\n\t\t\t{{\n\t\t\t\t\"Apps\"\n\t\t\t\t{{\n\t\t\t\t\t\"1086940\"\n\t\t\t\t\t{{\n\t\t\t\t\t\t\"LastPlayed\"\t\t\"1784871616\"\n\t\t\t\t\t\t\"cloud\"\n\t\t\t\t\t\t{{\n\t\t\t\t\t\t\t\"last_sync_state\"\t\t\"synchronized\"\n\t\t\t\t\t\t}}\n\t\t\t\t\t\t\"launchoptions\"\t\t\"{escaped}\"\n\t\t\t\t\t}}\n\t\t\t\t}}\n\t\t\t}}\n\t\t}}\n\t}}\n}}\n"
        );
        fs::write(config.join("localconfig.vdf"), vdf).unwrap();
    }

    fn write_login_users(steam_root: &Path, most_recent: u64, other: u64) {
        let vdf = format!(
            "\"users\"\n{{\n\t\"{}\"\n\t{{\n\t\t\"AccountName\"\t\t\"other\"\n\t\t\"MostRecent\"\t\t\"0\"\n\t}}\n\t\"{}\"\n\t{{\n\t\t\"AccountName\"\t\t\"me\"\n\t\t\"MostRecent\"\t\t\"1\"\n\t}}\n}}\n",
            STEAM_ID64_BASE + other,
            STEAM_ID64_BASE + most_recent
        );
        fs::create_dir_all(steam_root.join("config")).unwrap();
        fs::write(steam_root.join("config/loginusers.vdf"), vdf).unwrap();
    }

    #[test]
    fn script_extender_setup_reads_steam_proton_dll_and_launch_options() {
        let home = TempHome::new("se-setup");
        let game_root = make_game(&home.steam());

        let setup = script_extender_setup_in(Some(&home.0), &game_root);
        assert_eq!(setup.proton_check(), SetupCheck::Unknown);
        assert_eq!(setup.launch_option_check(), SetupCheck::Unknown);
        assert_eq!(setup.problem(), Some("not installed"));

        write_steam_config(&home.steam(), GLOBAL_PROTON_ONLY);
        write_login_users(&home.steam(), 22, 11);
        write_launch_options(&home.steam(), 11, SCRIPT_EXTENDER_LAUNCH_OPTION);
        write_launch_options(&home.steam(), 22, "game-performance %command% --vulkan");
        let setup = script_extender_setup_in(Some(&home.0), &game_root);
        assert_eq!(setup.uses_proton, Some(false));
        assert_eq!(
            setup.launch_options.as_deref(),
            Some("game-performance %command% --vulkan")
        );
        assert_eq!(setup.problem(), Some("needs Proton or BG3LE"));

        write_steam_config(&home.steam(), BG3_FORCED_PROTON);
        fs::write(game_root.join("bin/DWrite.dll"), b"dll").unwrap();
        let setup = script_extender_setup_in(Some(&home.0), &game_root);
        assert!(setup.installed);
        assert_eq!(setup.problem(), Some("launch option missing"));
        assert_eq!(
            setup.suggested_launch_options(),
            "WINEDLLOVERRIDES=\"DWrite.dll=n,b\" game-performance %command% --vulkan"
        );

        write_launch_options(&home.steam(), 22, &setup.suggested_launch_options());
        let setup = script_extender_setup_in(Some(&home.0), &game_root);
        assert!(setup.is_ready(), "{setup:?}");
    }

    #[test]
    fn script_extender_setup_accepts_a_prefix_dll_override() {
        let home = TempHome::new("se-prefix");
        let game_root = make_game(&home.steam());
        fs::write(game_root.join("bin/dwrite.dll"), b"dll").unwrap();
        write_steam_config(&home.steam(), BG3_FORCED_PROTON);
        write_launch_options(&home.steam(), 5, "");
        let setup = script_extender_setup_in(Some(&home.0), &game_root);
        assert_eq!(setup.launch_options.as_deref(), Some(""));
        assert_eq!(setup.problem(), Some("launch option missing"));

        let pfx = home.steam().join("steamapps/compatdata/1086940/pfx");
        fs::create_dir_all(&pfx).unwrap();
        fs::write(
            pfx.join("user.reg"),
            "WINE REGISTRY Version 2\n\n[Software\\\\Wine\\\\DllOverrides] 1700000000\n#time=1da\n\"*d3d11\"=\"native\"\n\"dwrite\"=\"native,builtin\"\n\n[Software\\\\Wine\\\\Fonts] 1700000000\n",
        )
        .unwrap();
        let setup = script_extender_setup_in(Some(&home.0), &game_root);
        assert!(setup.prefix_override);
        assert!(setup.is_ready());
    }

    #[test]
    fn script_extender_setup_skips_steam_checks_for_other_launchers() {
        let home = TempHome::new("se-other");
        write_steam_config(&home.steam(), GLOBAL_PROTON_ONLY);
        let game_root = home.0.join("Games/BG3");
        fs::create_dir_all(game_root.join("bin")).unwrap();
        fs::write(game_root.join("bin/DWrite.dll"), b"dll").unwrap();
        let setup = script_extender_setup_in(Some(&home.0), &game_root);
        assert_eq!(setup.uses_proton, None);
        assert_eq!(setup.launch_options, None);
        assert!(setup.is_ready());
        // Ready as far as SigilSmith can tell, but the override is the launcher's.
        assert!(setup.other_launcher);
        assert!(setup.needs_launcher_check());
    }

    fn install_bg3le(home: &Path, version: Option<&str>) -> PathBuf {
        let root = home.join(".local/share/bg3le");
        fs::create_dir_all(root.join("lib")).unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(root.join("lib/libbg3le.so"), b"so").unwrap();
        let wrapper = root.join("bin").join(BG3LE_WRAPPER_NAME);
        fs::write(&wrapper, b"#!/bin/sh").unwrap();
        if let Some(version) = version {
            fs::write(root.join("version"), format!("{version}\n")).unwrap();
        }
        wrapper
    }

    #[test]
    fn script_extender_setup_counts_bg3le_on_the_native_build() {
        let home = TempHome::new("se-bg3le");
        let game_root = make_game(&home.steam());
        write_steam_config(&home.steam(), GLOBAL_PROTON_ONLY);
        write_launch_options(&home.steam(), 7, "gamemoderun %command%");
        let wrapper = install_bg3le(&home.0, Some("v0.3.4"));

        let setup = script_extender_setup_in(Some(&home.0), &game_root);
        assert!(setup.uses_bg3le(), "{setup:?}");
        assert_eq!(
            setup
                .native_extender
                .as_ref()
                .and_then(|e| e.version.as_deref()),
            Some("v0.3.4")
        );
        assert_eq!(setup.proton_check(), SetupCheck::Unknown);
        assert_eq!(setup.installed_check(), SetupCheck::Ok);
        assert_eq!(setup.problem(), Some("BG3LE launch option missing"));
        // Nothing for "Set up for me": no DLL download, no prefix edit.
        assert!(!setup.can_set_up());
        assert_eq!(
            setup.suggested_launch_options(),
            format!("gamemoderun \"{}\" %command%", wrapper.display())
        );

        write_launch_options(&home.steam(), 7, &setup.suggested_launch_options());
        let setup = script_extender_setup_in(Some(&home.0), &game_root);
        assert!(setup.is_ready(), "{setup:?}");

        // On Proton, BG3LE's wrapper leaves the game alone: the DLL route applies.
        write_steam_config(&home.steam(), BG3_FORCED_PROTON);
        let setup = script_extender_setup_in(Some(&home.0), &game_root);
        assert!(!setup.uses_bg3le());
        assert_eq!(setup.problem(), Some("not installed"));
    }

    #[test]
    fn bg3le_wrapper_goes_in_front_of_the_command() {
        let wrapper = Path::new("/home/me/.local/share/bg3le/bin/bg3le-launch");
        let quoted = format!("\"{}\"", wrapper.display());
        for (existing, expected) in [
            ("", format!("{quoted} %command%")),
            ("%command%", format!("{quoted} %command%")),
            (
                "--skip-launcher",
                format!("{quoted} %command% --skip-launcher"),
            ),
            (
                "PROTON_LOG=1 %command% -x",
                format!("PROTON_LOG=1 {quoted} %command% -x"),
            ),
        ] {
            assert_eq!(launch_options_with_wrapper(existing, wrapper), expected);
        }
        let installed = format!("{quoted} %command%");
        assert_eq!(launch_options_with_wrapper(&installed, wrapper), installed);
    }

    #[test]
    fn launch_options_gain_the_dll_override_without_losing_anything() {
        let cases = [
            ("", SCRIPT_EXTENDER_LAUNCH_OPTION),
            (
                "gamemoderun %command%",
                "WINEDLLOVERRIDES=\"DWrite.dll=n,b\" gamemoderun %command%",
            ),
            (
                "--skip-launcher",
                "WINEDLLOVERRIDES=\"DWrite.dll=n,b\" %command% --skip-launcher",
            ),
            (
                "WINEDLLOVERRIDES=\"winhttp=n,b\" %command%",
                "WINEDLLOVERRIDES=\"DWrite.dll=n,b;winhttp=n,b\" %command%",
            ),
            (
                "PROTON_LOG=1 WINEDLLOVERRIDES=d3d11,dwrite=b %command%",
                "PROTON_LOG=1 WINEDLLOVERRIDES=\"DWrite.dll=n,b;d3d11=b\" %command%",
            ),
            (
                "WINEDLLOVERRIDES='dwrite=n,b' %command%",
                "WINEDLLOVERRIDES='dwrite=n,b' %command%",
            ),
        ];
        for (existing, expected) in cases {
            let merged = launch_options_with_dll_override(existing);
            assert_eq!(merged, expected, "from {existing:?}");
            assert!(launch_options_have_dll_override(&merged), "{merged}");
        }
        assert!(!launch_options_have_dll_override(
            "WINEDLLOVERRIDES=\"dwrite=b,n\" %command%"
        ));
        assert!(!launch_options_have_dll_override(
            "MY_WINEDLLOVERRIDES=dwrite=n %command%"
        ));
    }

    const USER_REG: &str = "WINE REGISTRY Version 2\n;; All keys relative to \\\\User\\\\S-1-5-21-0-0-0-1000\n\n#arch=win64\n\n[Software\\\\Wine\\\\DllOverrides] 1700000000\n#time=1da1b2c3d4e5f60\n\"*d3d11\"=\"native\"\n\"dwrite\"=\"builtin\"\n\n[Software\\\\Wine\\\\Fonts] 1700000000\n#time=1da1b2c3d4e5f60\n\"LogPixels\"=dword:00000060\n";

    #[test]
    fn prefix_override_replaces_an_existing_dwrite_entry() {
        assert!(!prefix_has_dll_override(USER_REG));
        let updated = with_prefix_dll_override(USER_REG, 1_800_000_000);
        assert!(prefix_has_dll_override(&updated));
        assert_eq!(updated.matches("dwrite").count(), 1);
        // Inserted under the key's metadata; everything else is unchanged.
        assert!(updated.contains(
            "[Software\\\\Wine\\\\DllOverrides] 1700000000\n#time=1da1b2c3d4e5f60\n\"dwrite\"=\"native,builtin\"\n\"*d3d11\"=\"native\"\n"
        ));
        assert!(updated.contains("\"LogPixels\"=dword:00000060\n"));
        assert_eq!(
            updated.lines().count(),
            USER_REG.lines().count(),
            "{updated}"
        );
        assert_eq!(with_prefix_dll_override(&updated, 1_800_000_000), updated);
    }

    #[test]
    fn prefix_override_creates_the_key_when_missing() {
        let reg = "WINE REGISTRY Version 2\n\n[Software\\\\Wine\\\\Fonts] 1700000000\n\"LogPixels\"=dword:00000060\n";
        let updated = with_prefix_dll_override(reg, 1_700_000_000);
        assert!(prefix_has_dll_override(&updated));
        assert!(updated.starts_with(reg));
        assert!(updated.ends_with(
            "\n\n[Software\\\\Wine\\\\DllOverrides] 1700000000\n#time=1da1747c66d0000\n\"dwrite\"=\"native,builtin\"\n"
        ));
    }

    #[test]
    fn set_prefix_dll_override_writes_the_prefix_and_keeps_a_backup() {
        let home = TempHome::new("se-write-prefix");
        let game_root = make_game(&home.steam());
        assert!(!set_prefix_dll_override(&game_root).unwrap());

        let pfx = home.steam().join("steamapps/compatdata/1086940/pfx");
        fs::create_dir_all(&pfx).unwrap();
        fs::write(pfx.join("user.reg"), USER_REG).unwrap();
        write_steam_config(&home.steam(), BG3_FORCED_PROTON);
        write_launch_options(&home.steam(), 7, "");
        let before = script_extender_setup_in(Some(&home.0), &game_root);
        assert!(before.can_set_override());

        assert!(set_prefix_dll_override(&game_root).unwrap());
        assert_eq!(
            fs::read_to_string(pfx.join("user.reg.sigilsmith-backup")).unwrap(),
            USER_REG
        );
        let after = script_extender_setup_in(Some(&home.0), &game_root);
        assert!(after.prefix_override);
        assert_eq!(after.launch_option_check(), SetupCheck::Ok);
        assert!(!after.can_set_up() || after.installed_check() == SetupCheck::Missing);
    }

    #[test]
    fn installs_the_dll_and_drops_other_spellings() {
        let home = TempHome::new("se-install-dll");
        let game_root = make_game(&home.steam());
        fs::write(game_root.join("bin/dwrite.dll"), b"old").unwrap();
        let path = install_script_extender_dll(&game_root, b"MZnew").unwrap();
        assert_eq!(path, game_root.join("bin/DWrite.dll"));
        assert_eq!(fs::read(&path).unwrap(), b"MZnew");
        assert!(!game_root.join("bin/dwrite.dll").exists());
        assert!(!game_root.join("bin/.DWrite.dll.sigilsmith-tmp").exists());
        assert!(install_script_extender_dll(&home.0.join("missing"), b"MZ").is_err());
    }

    #[test]
    fn crash_marker_is_cleared_only_when_safe() {
        let root = std::env::temp_dir().join(format!("sigilsmith-marker-{}", std::process::id()));
        let marker = root.join("ModCrashSanityCheck");
        fs::create_dir_all(&marker).unwrap();
        assert_eq!(clear_crash_marker_in(&root, true), None);
        assert!(marker.exists());
        assert_eq!(clear_crash_marker_in(&root, false), Some(marker.clone()));
        assert!(!marker.exists());
        assert_eq!(clear_crash_marker_in(&root, false), None);
        // A folder with something in it isn't the game's marker.
        fs::create_dir_all(&marker).unwrap();
        fs::write(marker.join("note"), b"x").unwrap();
        assert_eq!(clear_crash_marker_in(&root, false), None);
        let _ = fs::remove_dir_all(&root);
    }
}
