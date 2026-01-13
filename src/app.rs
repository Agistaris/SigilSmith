use crate::{
    backup,
    config::{AppConfig, GameConfig},
    deploy,
    game::{self, GameId},
    importer,
    library::{
        library_mod_root, normalize_label, normalize_times, path_times, resolve_times,
        FileOverride, InstallTarget, Library, ModEntry, ModSource, Profile, ProfileEntry,
        TargetKind, TargetOverride,
    },
    metadata, native_pak, smart_rank, update,
};
use anyhow::{Context, Result};
use arboard::Clipboard;
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet, VecDeque},
    fs,
    io::{self, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        mpsc::{self, Receiver, Sender, TryRecvError},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use walkdir::WalkDir;

use blake3::Hasher;

const SEARCH_DEBOUNCE_MS: u64 = 250;
const HOTKEY_DEBOUNCE_MS: u64 = 200;
const HOTKEY_FADE_MS: u64 = 200;
const METADATA_CACHE_VERSION: u32 = 2;
const SMART_RANK_DEBOUNCE_MS: u64 = 600;
const SMART_RANK_CACHE_SAVE_DEBOUNCE_MS: u64 = 400;
const SMART_RANK_CACHE_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputPurpose {
    ImportPath,
    CreateProfile,
    RenameProfile { original: String },
    DuplicateProfile { source: String },
    ExportProfile { profile: String },
    ImportProfile,
    FilterMods,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Editing {
        prompt: String,
        buffer: String,
        purpose: InputPurpose,
        auto_submit: bool,
        last_edit_at: Instant,
    },
    Browsing(PathBrowser),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupStep {
    GameRoot,
    LarianDir,
    DownloadsDir,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathBrowserEntryKind {
    Select,
    Parent,
    Dir,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathBrowserFocus {
    List,
    PathInput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathBrowserEntry {
    pub label: String,
    pub path: PathBuf,
    pub kind: PathBrowserEntryKind,
    pub selectable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathBrowser {
    pub step: SetupStep,
    pub current: PathBuf,
    pub entries: Vec<PathBrowserEntry>,
    pub selected: usize,
    pub path_input: String,
    pub focus: PathBrowserFocus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogChoice {
    Yes,
    No,
}

#[derive(Debug, Clone)]
pub struct DialogToggle {
    pub label: String,
    pub checked: bool,
}

#[derive(Debug, Clone)]
pub enum DialogKind {
    Overwrite,
    Similar,
    Unrecognized {
        path: PathBuf,
        label: String,
    },
    DeleteProfile {
        name: String,
    },
    DeleteMod {
        id: String,
        name: String,
        native: bool,
        dependents: Vec<DependentMod>,
    },
    MoveBlocked {
        resume_move_mode: bool,
        clear_filter: bool,
    },
    CancelImport,
    OverrideDependencies,
    ImportSummary,
    CopyDependencySearchLink {
        link: String,
    },
    StartupDependencyNotice,
    EnableAllVisible,
    DisableAllVisible,
    InvertVisible,
}

#[derive(Debug, Clone)]
pub struct Dialog {
    pub title: String,
    pub message: String,
    pub yes_label: String,
    pub no_label: String,
    pub choice: DialogChoice,
    pub kind: DialogKind,
    pub toggle: Option<DialogToggle>,
    pub scroll: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum DependencyStatus {
    Missing,
    Waiting,
    Downloaded,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyItemKind {
    Missing,
    OverrideAction,
}

#[derive(Debug, Clone)]
pub struct DependencyItem {
    pub label: String,
    pub display_label: String,
    pub uuid: Option<String>,
    pub required_by: Vec<String>,
    pub status: DependencyStatus,
    pub link: Option<String>,
    pub search_link: Option<String>,
    pub search_label: String,
    pub kind: DependencyItemKind,
}

#[derive(Debug, Clone)]
pub struct DependencyQueue {
    pub items: Vec<DependencyItem>,
    pub selected: usize,
}

impl DependencyItem {
    pub fn is_override_action(&self) -> bool {
        matches!(self.kind, DependencyItemKind::OverrideAction)
    }
}

#[derive(Debug, Clone)]
pub struct DependencyLookup {
    id_map: HashMap<String, String>,
    key_map: HashMap<String, Vec<String>>,
}

impl DependencyLookup {
    pub fn new(mods: &[ModEntry]) -> Self {
        let mut id_map = HashMap::new();
        let mut key_map: HashMap<String, Vec<String>> = HashMap::new();
        for mod_entry in mods {
            let id_key = normalize_label(&mod_entry.id);
            if !id_key.is_empty() {
                id_map.insert(id_key, mod_entry.id.clone());
            }
            for key in mod_dependency_keys(mod_entry) {
                key_map.entry(key).or_default().push(mod_entry.id.clone());
            }
        }
        for ids in key_map.values_mut() {
            ids.sort();
            ids.dedup();
        }
        Self { id_map, key_map }
    }

    pub fn resolve_ids(&self, dependency: &str) -> Vec<String> {
        let mut out = Vec::new();
        for key in dependency_match_keys(dependency) {
            if let Some(id) = self.id_map.get(&key) {
                out.push(id.clone());
            }
            if let Some(ids) = self.key_map.get(&key) {
                out.extend(ids.iter().cloned());
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

#[derive(Debug, Clone)]
pub struct DependentMod {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub level: LogLevel,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub enum UpdateStatus {
    Idle,
    Checking,
    UpToDate {
        version: String,
    },
    Available {
        info: update::UpdateInfo,
        path: PathBuf,
        instructions: String,
    },
    Applied {
        info: update::UpdateInfo,
    },
    Skipped {
        version: String,
        reason: String,
    },
    Failed {
        error: String,
    },
}

enum ImportMessage {
    Progress(importer::ImportProgress),
    Completed {
        path: PathBuf,
        result: importer::ImportResult,
    },
    Failed {
        path: PathBuf,
        error: String,
    },
}

enum DeployMessage {
    Completed { report: deploy::DeployReport },
    Failed { error: String },
}

#[derive(Debug, Clone)]
struct MetadataUpdate {
    id: String,
    created_at: Option<i64>,
    modified_at: Option<i64>,
    dependencies: Vec<String>,
}

enum MetadataMessage {
    Progress {
        update: MetadataUpdate,
        current: usize,
        total: usize,
    },
    Completed,
    Failed {
        error: String,
    },
}

enum ConflictMessage {
    Completed {
        conflicts: Vec<deploy::ConflictEntry>,
    },
    Failed {
        error: String,
    },
}

enum UpdateMessage {
    Completed(update::UpdateResult),
    Failed { error: String },
}

#[derive(Debug, Clone)]
enum DuplicateKind {
    Exact,
    Similar {
        new_label: String,
        existing_label: String,
        new_stamp: Option<u64>,
        existing_stamp: Option<u64>,
        similarity: f32,
    },
}

struct DuplicateDecision {
    mod_entry: ModEntry,
    existing_id: String,
    existing_label: String,
    kind: DuplicateKind,
    default_overwrite: Option<bool>,
}

#[derive(Debug, Clone)]
struct SimilarMatch {
    existing_id: String,
    existing_label: String,
    new_label: String,
    new_stamp: Option<u64>,
    existing_stamp: Option<u64>,
    similarity: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProfileExport {
    game_id: String,
    game_name: String,
    profile_name: String,
    entries: Vec<ProfileExportEntry>,
    #[serde(default)]
    file_overrides: Vec<FileOverride>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProfileExportEntry {
    id: String,
    name: String,
    enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub message: String,
    pub level: ToastLevel,
    pub expires_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Explorer,
    Mods,
    Conflicts,
    Log,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModSortColumn {
    Order,
    Name,
    Enabled,
    Native,
    Kind,
    Target,
    Created,
    Added,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModSort {
    pub column: ModSortColumn,
    pub direction: SortDirection,
}

impl Default for ModSort {
    fn default() -> Self {
        Self {
            column: ModSortColumn::Order,
            direction: SortDirection::Asc,
        }
    }
}

impl ModSort {
    pub fn column_label(&self) -> &'static str {
        match self.column {
            ModSortColumn::Order => "Order",
            ModSortColumn::Name => "Mod",
            ModSortColumn::Enabled => "Enabled",
            ModSortColumn::Native => "Native",
            ModSortColumn::Kind => "Kind",
            ModSortColumn::Target => "Target",
            ModSortColumn::Created => "Created",
            ModSortColumn::Added => "Added",
        }
    }

    pub fn direction_arrow(&self) -> &'static str {
        match self.direction {
            SortDirection::Asc => "↑",
            SortDirection::Desc => "↓",
        }
    }

    pub fn direction_label(&self) -> &'static str {
        match self.direction {
            SortDirection::Asc => "asc",
            SortDirection::Desc => "desc",
        }
    }

    pub fn is_order_default(&self) -> bool {
        matches!(self.column, ModSortColumn::Order) && matches!(self.direction, SortDirection::Asc)
    }
}

const MOD_SORT_COLUMNS: [ModSortColumn; 8] = [
    ModSortColumn::Enabled,
    ModSortColumn::Native,
    ModSortColumn::Order,
    ModSortColumn::Kind,
    ModSortColumn::Target,
    ModSortColumn::Created,
    ModSortColumn::Added,
    ModSortColumn::Name,
];

#[derive(Debug, Clone)]
pub enum ExplorerItemKind {
    Game(GameId),
    ProfilesHeader(GameId),
    Profile { name: String },
    NewProfile(GameId),
    Info(GameId),
}

#[derive(Debug, Clone)]
pub struct ExplorerItem {
    pub kind: ExplorerItemKind,
    pub label: String,
    pub depth: usize,
    pub active: bool,
    pub expanded: bool,
    pub disabled: bool,
    pub renaming: bool,
}

pub struct App {
    pub app_config: AppConfig,
    pub game_id: GameId,
    pub config: GameConfig,
    pub library: Library,
    pub status: String,
    pub selected: usize,
    pub input_mode: InputMode,
    pub help_open: bool,
    pub help_scroll: usize,
    pub should_quit: bool,
    pub move_mode: bool,
    pub dialog: Option<Dialog>,
    pub logs: Vec<LogEntry>,
    pub log_scroll: usize,
    pub move_dirty: bool,
    pub focus: Focus,
    pub hotkey_focus: Focus,
    pub explorer_selected: usize,
    pub toast: Option<Toast>,
    clipboard: Option<Clipboard>,
    pub mod_filter: String,
    mod_filter_snapshot: Option<String>,
    pub mod_sort: ModSort,
    pub settings_menu: Option<SettingsMenu>,
    pub update_status: UpdateStatus,
    pub smart_rank_preview: Option<SmartRankPreview>,
    pub smart_rank_scroll: usize,
    pub smart_rank_view: SmartRankView,
    pub smart_rank_progress: Option<smart_rank::SmartRankProgress>,
    smart_rank_cache: Option<SmartRankCache>,
    smart_rank_active: bool,
    smart_rank_mode: Option<SmartRankMode>,
    smart_rank_interrupt: bool,
    smart_rank_refresh_pending: Option<smart_rank::SmartRankRefreshMode>,
    smart_rank_refresh_kind: Option<smart_rank::SmartRankRefreshMode>,
    smart_rank_refresh_at: Option<Instant>,
    smart_rank_cache_last_saved: Option<Instant>,
    smart_rank_scan_id: u64,
    smart_rank_scan_active: Option<u64>,
    smart_rank_scan_profile_key: Option<String>,
    #[cfg(debug_assertions)]
    debug_suppress_persistence: bool,
    startup_dependency_check_pending: bool,
    smart_rank_tx: Sender<SmartRankMessage>,
    smart_rank_rx: Receiver<SmartRankMessage>,
    native_sync_tx: Sender<NativeSyncMessage>,
    native_sync_rx: Receiver<NativeSyncMessage>,
    native_sync_active: bool,
    native_sync_progress: Option<NativeSyncProgress>,
    metadata_tx: Sender<MetadataMessage>,
    metadata_rx: Receiver<MetadataMessage>,
    metadata_active: bool,
    metadata_processed: usize,
    metadata_total: usize,
    metadata_processed_ids: HashSet<String>,
    metadata_dirty: bool,
    update_tx: Sender<UpdateMessage>,
    update_rx: Receiver<UpdateMessage>,
    update_active: bool,
    update_started_at: Option<Instant>,
    startup_pending: bool,
    startup_mode: StartupMode,
    startup_post_sync_pending: bool,
    hotkey_pending_focus: Option<Focus>,
    hotkey_transition_at: Option<Instant>,
    hotkey_fade_until: Option<Instant>,
    import_queue: VecDeque<PathBuf>,
    import_active: Option<PathBuf>,
    import_tx: Sender<ImportMessage>,
    import_rx: Receiver<ImportMessage>,
    import_batches: VecDeque<importer::ImportBatch>,
    pending_import_batch: Option<importer::ImportBatch>,
    dependency_queue: Option<DependencyQueue>,
    pending_dependency_enable: Option<Vec<String>>,
    dependency_queue_view: usize,
    dependency_cache: HashMap<String, Vec<String>>,
    dependency_cache_ready: bool,
    pending_delete_mod: Option<(String, String)>,
    import_failures: Vec<importer::ImportFailure>,
    import_progress: Option<importer::ImportProgress>,
    import_summary_pending: bool,
    deploy_active: bool,
    deploy_pending: bool,
    deploy_reason: Option<String>,
    deploy_backup: bool,
    deploy_tx: Sender<DeployMessage>,
    deploy_rx: Receiver<DeployMessage>,
    conflict_active: bool,
    conflict_pending: bool,
    conflict_tx: Sender<ConflictMessage>,
    conflict_rx: Receiver<ConflictMessage>,
    log_path: PathBuf,
    duplicate_queue: VecDeque<DuplicateDecision>,
    pending_duplicate: Option<DuplicateDecision>,
    duplicate_apply_all: Option<bool>,
    approved_imports: Vec<ModEntry>,
    pub conflicts: Vec<deploy::ConflictEntry>,
    pub conflict_selected: usize,
    pub override_swap: Option<OverrideSwap>,
    pub pending_override: Option<PendingOverride>,
    pub mods_view_height: usize,
    explorer_game_expanded: HashSet<GameId>,
    explorer_profiles_expanded: HashSet<GameId>,
}

#[derive(Debug, Clone)]
pub struct SettingsMenu {
    pub selected: usize,
}

#[derive(Debug, Clone)]
pub struct OverrideSwap {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone)]
pub struct PendingOverride {
    pub conflict_index: usize,
    pub winner_id: String,
    pub from: String,
    pub to: String,
    pub last_input: Instant,
}

#[derive(Debug, Clone)]
pub struct OverrideSwapInfo {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone)]
pub struct SmartRankMove {
    pub name: String,
    pub from: usize,
    pub to: usize,
    pub created_at: Option<i64>,
    pub added_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmartRankView {
    Changes,
    Explain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmartRankMode {
    Preview,
    Warmup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupMode {
    Ui,
    Cli,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliVerbosity {
    Quiet,
    Normal,
    Verbose,
    Debug,
}

#[derive(Debug, Clone)]
pub struct CliImportOptions {
    pub deploy: bool,
    pub verbosity: CliVerbosity,
}

#[derive(Debug, Clone)]
pub struct SmartRankPreview {
    pub proposed: Vec<ProfileEntry>,
    pub report: smart_rank::SmartRankReport,
    pub moves: Vec<SmartRankMove>,
    pub warnings: Vec<String>,
    pub explain: smart_rank::SmartRankExplain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SmartRankCache {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    profile_key: String,
    #[serde(default)]
    mod_cache: smart_rank::SmartRankCacheData,
    #[serde(default)]
    result: Option<smart_rank::SmartRankResult>,
}

#[derive(Debug, Clone)]
pub enum SmartRankMessage {
    Progress {
        scan_id: u64,
        progress: smart_rank::SmartRankProgress,
    },
    Finished {
        scan_id: u64,
        computed: smart_rank::SmartRankComputed,
    },
    Failed {
        scan_id: u64,
        error: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeSyncStage {
    NativeFiles,
    AdoptNative,
    AddMissing,
}

impl NativeSyncStage {
    fn label(self) -> &'static str {
        match self {
            NativeSyncStage::NativeFiles => "Native mods prepass",
            NativeSyncStage::AdoptNative => "Native mods adopt",
            NativeSyncStage::AddMissing => "Native mods add",
        }
    }
}

#[derive(Debug, Clone)]
pub struct NativeSyncProgress {
    pub stage: NativeSyncStage,
    pub current: usize,
    pub total: usize,
}

#[derive(Debug, Clone)]
pub struct NativeModUpdate {
    pub id: String,
    pub source: ModSource,
    pub name: String,
    pub source_label: Option<String>,
    pub targets: Vec<InstallTarget>,
    pub created_at: Option<i64>,
    pub modified_at: Option<i64>,
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct NativeSyncDelta {
    pub updates: Vec<NativeModUpdate>,
    pub added: Vec<ModEntry>,
    pub updated_native_files: usize,
    pub adopted_native: usize,
    pub modsettings_exists: bool,
    pub modsettings_hash: Option<String>,
    pub enabled_set: HashSet<String>,
    pub order: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum NativeSyncMessage {
    Progress(NativeSyncProgress),
    Completed(NativeSyncDelta),
    Skipped(String),
}

impl App {
    pub fn initialize(mode: StartupMode) -> Result<Self> {
        let mut setup_error = None;
        let mut app_config = AppConfig::load_or_create()?;
        if app_config.downloads_dir.as_os_str().is_empty() {
            if let Some(user_dirs) = directories::UserDirs::new() {
                if let Some(path) = user_dirs.download_dir() {
                    app_config.downloads_dir = path.to_path_buf();
                }
            }
            if app_config.downloads_dir.as_os_str().is_empty() {
                app_config.downloads_dir = BaseDirs::new()
                    .map(|base| base.home_dir().to_path_buf())
                    .unwrap_or_else(|| PathBuf::from("/"));
            }
            let _ = app_config.save();
        }
        let game_id = app_config.active_game;
        let mut config = GameConfig::load_or_create(game_id)?;
        if let Err(err) =
            game::detect_paths(game_id, Some(&config.game_root), Some(&config.larian_dir))
        {
            // Retry auto-detect when stored paths are missing or stale.
            if let Ok(paths) = game::detect_paths(game_id, None, None) {
                config.game_root = paths.game_root;
                config.larian_dir = paths.larian_dir;
                let _ = config.save();
            } else {
                setup_error = Some(err.to_string());
            }
        }

        let mut library = Library::load_or_create(&config.data_dir)?;
        library.ensure_mods_in_profiles();
        if !config.active_profile.is_empty()
            && library
                .profiles
                .iter()
                .any(|profile| profile.name == config.active_profile)
        {
            library.active_profile = config.active_profile.clone();
        } else {
            config.active_profile = library.active_profile.clone();
        }
        library.save(&config.data_dir)?;
        config.save()?;

        let (import_tx, import_rx) = mpsc::channel();
        let (deploy_tx, deploy_rx) = mpsc::channel();
        let (conflict_tx, conflict_rx) = mpsc::channel();
        let (smart_rank_tx, smart_rank_rx) = mpsc::channel();
        let (native_sync_tx, native_sync_rx) = mpsc::channel();
        let (metadata_tx, metadata_rx) = mpsc::channel();
        let (update_tx, update_rx) = mpsc::channel();
        let log_path = config.data_dir.join("sigilsmith.log");

        let mut app = Self {
            app_config,
            game_id,
            config,
            library,
            status: "Detecting game paths...".to_string(),
            selected: 0,
            input_mode: InputMode::Normal,
            help_open: false,
            help_scroll: 0,
            should_quit: false,
            move_mode: false,
            dialog: None,
            logs: Vec::new(),
            log_scroll: 0,
            move_dirty: false,
            focus: Focus::Mods,
            hotkey_focus: Focus::Mods,
            explorer_selected: 0,
            toast: None,
            clipboard: Clipboard::new().ok(),
            mod_filter: String::new(),
            mod_filter_snapshot: None,
            mod_sort: ModSort::default(),
            settings_menu: None,
            update_status: UpdateStatus::Idle,
            smart_rank_preview: None,
            smart_rank_scroll: 0,
            smart_rank_view: SmartRankView::Changes,
            smart_rank_progress: None,
            smart_rank_active: false,
            smart_rank_mode: None,
            smart_rank_cache: None,
            smart_rank_interrupt: false,
            smart_rank_refresh_pending: None,
            smart_rank_refresh_kind: None,
            smart_rank_refresh_at: None,
            smart_rank_cache_last_saved: None,
            smart_rank_scan_id: 0,
            smart_rank_scan_active: None,
            smart_rank_scan_profile_key: None,
            #[cfg(debug_assertions)]
            debug_suppress_persistence: false,
            startup_dependency_check_pending: matches!(mode, StartupMode::Ui),
            smart_rank_tx,
            smart_rank_rx,
            native_sync_tx,
            native_sync_rx,
            native_sync_active: false,
            native_sync_progress: None,
            metadata_tx,
            metadata_rx,
            metadata_active: false,
            metadata_processed: 0,
            metadata_total: 0,
            metadata_processed_ids: HashSet::new(),
            metadata_dirty: false,
            update_tx,
            update_rx,
            update_active: false,
            update_started_at: None,
            startup_pending: true,
            startup_mode: mode,
            startup_post_sync_pending: false,
            hotkey_pending_focus: None,
            hotkey_transition_at: None,
            hotkey_fade_until: None,
            import_queue: VecDeque::new(),
            import_active: None,
            import_tx,
            import_rx,
            import_batches: VecDeque::new(),
            pending_import_batch: None,
            dependency_queue: None,
            pending_dependency_enable: None,
            dependency_queue_view: 1,
            dependency_cache: HashMap::new(),
            dependency_cache_ready: false,
            pending_delete_mod: None,
            import_failures: Vec::new(),
            import_progress: None,
            import_summary_pending: false,
            deploy_active: false,
            deploy_pending: false,
            deploy_reason: None,
            deploy_backup: true,
            deploy_tx,
            deploy_rx,
            conflict_active: false,
            conflict_pending: false,
            conflict_tx,
            conflict_rx,
            log_path,
            duplicate_queue: VecDeque::new(),
            pending_duplicate: None,
            duplicate_apply_all: None,
            approved_imports: Vec::new(),
            conflicts: Vec::new(),
            conflict_selected: 0,
            override_swap: None,
            pending_override: None,
            mods_view_height: 0,
            explorer_game_expanded: {
                let mut expanded = HashSet::new();
                expanded.insert(game_id);
                expanded
            },
            explorer_profiles_expanded: {
                let mut expanded = HashSet::new();
                expanded.insert(game_id);
                expanded
            },
        };

        app.load_smart_rank_cache();
        let mod_count = app.library.mods.len();
        app.log_info(format!("Library loaded: {mod_count} mod(s)"));
        app.log_info("Detecting game paths...".to_string());
        if let Some(error) = setup_error {
            app.log_warn(format!("Path auto-detect failed: {error}"));
            app.status = "Setup required: open Menu (Esc) to configure paths".to_string();
        } else if app.paths_ready() {
            app.status = "Paths ready (Esc → Menu to change)".to_string();
            app.log_info(format!(
                "Paths ready: root={} user={}",
                app.config.game_root.display(),
                app.config.larian_dir.display()
            ));
            app.set_toast(
                "Paths detected: BG3 + Larian data",
                ToastLevel::Info,
                Duration::from_secs(3),
            );
        }
        app.ensure_setup();
        if matches!(mode, StartupMode::Cli) {
            app.finish_startup();
        }
        Ok(app)
    }

    pub fn finish_startup(&mut self) {
        if !self.startup_pending {
            return;
        }
        self.startup_pending = false;
        if matches!(self.startup_mode, StartupMode::Ui) {
            self.startup_post_sync_pending = true;
            self.start_native_sync();
        } else {
            self.run_native_sync_inline();
            self.run_post_sync_tasks();
        }
    }

    fn run_post_sync_tasks(&mut self) {
        if self.normalize_mod_sources() {
            let _ = self.library.save(&self.config.data_dir);
        }
        self.maybe_start_metadata_refresh();
        self.queue_conflict_scan("startup");
        self.start_update_check();
    }

    pub fn profile_counts(&self) -> (usize, usize) {
        let Some(profile) = self.library.active_profile() else {
            return (0, 0);
        };
        let total = profile.order.len();
        let enabled = profile.order.iter().filter(|entry| entry.enabled).count();
        (total, enabled)
    }

    pub fn visible_profile_indices(&self) -> Vec<usize> {
        let Some(profile) = self.library.active_profile() else {
            return Vec::new();
        };
        let mod_map = self.library.index_by_id();
        let filter = self.mod_filter_normalized();
        let mut indices: Vec<usize> = profile
            .order
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let mod_entry = mod_map.get(&entry.id)?;
                if let Some(filter) = filter.as_deref() {
                    if !mod_matches_filter(mod_entry, filter) {
                        return None;
                    }
                }
                Some(index)
            })
            .collect();
        if !self.mod_list_loading() {
            sort_mod_indices(&mut indices, profile, &mod_map, self.mod_sort);
        }
        indices
    }

    pub fn visible_profile_entries(&self) -> Vec<(usize, ProfileEntry)> {
        let Some(profile) = self.library.active_profile() else {
            return Vec::new();
        };
        let indices = self.visible_profile_indices();
        indices
            .into_iter()
            .filter_map(|index| {
                profile
                    .order
                    .get(index)
                    .cloned()
                    .map(|entry| (index, entry))
            })
            .collect()
    }

    pub fn selected_profile_index(&self) -> Option<usize> {
        let indices = self.visible_profile_indices();
        indices.get(self.selected).copied()
    }

    fn selected_profile_id(&self) -> Option<String> {
        let Some(profile) = self.library.active_profile() else {
            return None;
        };
        let index = self.selected_profile_index()?;
        profile.order.get(index).map(|entry| entry.id.clone())
    }

    pub fn mod_filter_active(&self) -> bool {
        !self.mod_filter.trim().is_empty()
    }

    fn mod_filter_normalized(&self) -> Option<String> {
        let trimmed = self.mod_filter.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_lowercase())
        }
    }

    pub fn cycle_mod_sort_column(&mut self, direction: i32) {
        let current_id = self.selected_profile_id();
        let next_column = mod_sort_next_column(self.mod_sort.column, direction);
        if next_column == self.mod_sort.column {
            return;
        }
        self.mod_sort.column = next_column;
        self.move_mode = false;
        self.reselect_mod_by_id(current_id);
        self.status = format!(
            "Sort: {} ({})",
            self.mod_sort.column_label(),
            self.mod_sort.direction_label()
        );
    }

    pub fn toggle_mod_sort_direction(&mut self) {
        let current_id = self.selected_profile_id();
        self.mod_sort.direction = match self.mod_sort.direction {
            SortDirection::Asc => SortDirection::Desc,
            SortDirection::Desc => SortDirection::Asc,
        };
        self.move_mode = false;
        self.reselect_mod_by_id(current_id);
        self.status = format!(
            "Sort: {} ({})",
            self.mod_sort.column_label(),
            self.mod_sort.direction_label()
        );
    }

    fn reselect_mod_by_id(&mut self, id: Option<String>) {
        self.selected = 0;
        if let Some(id) = id {
            if let Some(profile) = self.library.active_profile() {
                let indices = self.visible_profile_indices();
                if let Some(pos) = indices.iter().position(|index| {
                    profile
                        .order
                        .get(*index)
                        .map(|entry| entry.id == id)
                        .unwrap_or(false)
                }) {
                    self.selected = pos;
                }
            }
        }
        self.clamp_selection();
    }

    pub fn rename_preview(&self) -> Option<(String, String)> {
        match &self.input_mode {
            InputMode::Editing {
                buffer,
                purpose: InputPurpose::RenameProfile { original },
                ..
            } => Some((original.clone(), buffer.clone())),
            _ => None,
        }
    }

    pub fn active_profile_label(&self) -> String {
        let name = self.library.active_profile.clone();
        if let Some((original, buffer)) = self.rename_preview() {
            if original == name {
                let trimmed = buffer.trim();
                if trimmed.is_empty() {
                    return "<new name>".to_string();
                }
                return buffer;
            }
        }
        name
    }

    pub fn is_renaming_profile(&self, name: &str) -> bool {
        self.rename_preview()
            .map(|(original, _)| original == name)
            .unwrap_or(false)
    }

    pub fn is_renaming_active_profile(&self) -> bool {
        let active = self.library.active_profile.clone();
        self.is_renaming_profile(&active)
    }

    pub fn set_toast(&mut self, message: &str, level: ToastLevel, duration: Duration) {
        self.toast = Some(Toast {
            message: message.to_string(),
            level,
            expires_at: Instant::now() + duration,
        });
    }

    pub fn open_settings_menu(&mut self) {
        self.settings_menu = Some(SettingsMenu { selected: 0 });
        self.start_update_check();
    }

    pub fn close_settings_menu(&mut self) {
        self.settings_menu = None;
    }

    pub fn toggle_settings_menu(&mut self) {
        if self.settings_menu.is_some() {
            self.close_settings_menu();
        } else {
            self.open_settings_menu();
        }
    }

    pub fn toggle_help(&mut self) {
        if self.help_open {
            self.help_open = false;
        } else {
            self.help_open = true;
            self.help_scroll = 0;
        }
    }

    pub fn close_help(&mut self) {
        self.help_open = false;
    }

    pub fn toggle_confirm_profile_delete(&mut self) -> Result<()> {
        self.app_config.confirm_profile_delete = !self.app_config.confirm_profile_delete;
        self.app_config.save()?;
        let state = if self.app_config.confirm_profile_delete {
            "enabled"
        } else {
            "disabled"
        };
        self.status = format!("Confirm profile delete {state}");
        Ok(())
    }

    pub fn toggle_confirm_mod_delete(&mut self) -> Result<()> {
        self.app_config.confirm_mod_delete = !self.app_config.confirm_mod_delete;
        self.app_config.save()?;
        let state = if self.app_config.confirm_mod_delete {
            "enabled"
        } else {
            "disabled"
        };
        self.status = format!("Confirm mod delete {state}");
        Ok(())
    }

    pub fn toggle_enable_mods_after_import(&mut self) -> Result<()> {
        self.app_config.enable_mods_after_import = !self.app_config.enable_mods_after_import;
        self.app_config.save()?;
        let state = if self.app_config.enable_mods_after_import {
            "enabled"
        } else {
            "disabled"
        };
        self.status = format!("Enable mods after import {state}");
        Ok(())
    }

    pub fn toggle_delete_mod_files_on_remove(&mut self) -> Result<()> {
        self.app_config.delete_mod_files_on_remove = !self.app_config.delete_mod_files_on_remove;
        self.app_config.save()?;
        let state = if self.app_config.delete_mod_files_on_remove {
            "enabled"
        } else {
            "disabled"
        };
        self.status = format!("Delete mod files on remove {state}");
        Ok(())
    }

    pub fn toggle_dependency_downloads(&mut self) -> Result<()> {
        self.app_config.offer_dependency_downloads = !self.app_config.offer_dependency_downloads;
        self.app_config.save()?;
        let state = if self.app_config.offer_dependency_downloads {
            "enabled"
        } else {
            "disabled"
        };
        self.status = format!("Dependency downloads {state}");
        Ok(())
    }

    pub fn toggle_dependency_warnings(&mut self) -> Result<()> {
        self.app_config.warn_missing_dependencies = !self.app_config.warn_missing_dependencies;
        self.app_config.save()?;
        let state = if self.app_config.warn_missing_dependencies {
            "enabled"
        } else {
            "disabled"
        };
        self.status = format!("Missing dependency warnings {state}");
        Ok(())
    }

    pub fn toggle_startup_dependency_notice(&mut self) -> Result<()> {
        self.app_config.show_startup_dependency_notice =
            !self.app_config.show_startup_dependency_notice;
        self.app_config.save()?;
        let state = if self.app_config.show_startup_dependency_notice {
            "enabled"
        } else {
            "disabled"
        };
        self.status = format!("Startup dependency notice {state}");
        Ok(())
    }

    pub fn clear_system_caches(&mut self) {
        self.dependency_cache.clear();
        self.dependency_cache_ready = false;
        self.library.metadata_cache_version = 0;
        self.library.metadata_cache_key = None;
        self.smart_rank_cache = None;
        self.smart_rank_cache_last_saved = None;
        self.clear_smart_rank_cache_file();
        if let Err(err) = self.library.save(&self.config.data_dir) {
            self.log_warn(format!("System cache clear save failed: {err}"));
        }
        if !self.metadata_active {
            self.maybe_start_metadata_refresh();
        }
        self.status = "System caches cleared".to_string();
        self.set_toast(
            "System caches cleared",
            ToastLevel::Info,
            Duration::from_secs(2),
        );
    }

    pub fn open_smart_rank_preview(&mut self) {
        if self.smart_rank_active {
            self.status = "Smart ranking already running".to_string();
            return;
        }
        if self.is_busy() {
            self.status = "Smart ranking blocked: busy".to_string();
            self.log_warn("Smart ranking blocked: busy".to_string());
            return;
        }
        if !self.paths_ready() {
            self.status = "Smart ranking blocked: paths not set".to_string();
            self.log_warn("Smart ranking blocked: paths not set".to_string());
            return;
        }

        self.start_smart_rank_scan(
            SmartRankMode::Preview,
            smart_rank::SmartRankRefreshMode::Incremental,
        );
    }

    pub fn clear_smart_rank_cache(&mut self) {
        self.smart_rank_cache = None;
        self.clear_smart_rank_cache_file();
        self.smart_rank_refresh_at = None;
        if self.smart_rank_active {
            self.status = "Smart rank cache cleared; warming up after scan".to_string();
            self.schedule_smart_rank_refresh(
                smart_rank::SmartRankRefreshMode::Full,
                "cache cleared",
                false,
            );
            return;
        }
        if !self.paths_ready() {
            self.status = "Smart rank cache cleared".to_string();
            return;
        }
        self.status = "Smart rank cache cleared; warming up".to_string();
        self.start_smart_rank_scan(
            SmartRankMode::Warmup,
            smart_rank::SmartRankRefreshMode::Full,
        );
    }

    fn compute_missing_dependency_blocks(&self) -> HashSet<String> {
        if !self.dependency_cache_ready {
            return HashSet::new();
        }
        let lookup = DependencyLookup::new(&self.library.mods);
        let mut missing = HashSet::new();
        for mod_entry in &self.library.mods {
            if self.missing_dependency_count_for_mod(mod_entry, &lookup) > 0 {
                missing.insert(mod_entry.id.clone());
            }
        }
        missing
    }

    fn refresh_dependency_blocks(&mut self) -> HashSet<String> {
        let missing = self.compute_missing_dependency_blocks();
        if missing != self.library.dependency_blocks {
            self.library.dependency_blocks = missing.clone();
            let _ = self.library.save(&self.config.data_dir);
        }
        missing
    }

    fn run_startup_dependency_check(&mut self) {
        if !self.startup_dependency_check_pending {
            return;
        }
        if !self.dependency_cache_ready {
            return;
        }
        self.startup_dependency_check_pending = false;
        let missing_blocks = self.refresh_dependency_blocks();
        let Some(profile) = self.library.active_profile() else {
            return;
        };
        let mut to_disable = Vec::new();
        let mut disabled_names = Vec::new();
        for entry in profile.order.iter().filter(|entry| entry.enabled) {
            if !entry.enabled {
                continue;
            }
            let Some(mod_entry) = self
                .library
                .mods
                .iter()
                .find(|mod_entry| mod_entry.id == entry.id)
            else {
                continue;
            };
            if missing_blocks.contains(&mod_entry.id) {
                to_disable.push(entry.id.clone());
                disabled_names.push(mod_entry.display_name());
            }
        }
        if to_disable.is_empty() {
            return;
        }
        let changed = self.set_mods_enabled_in_active(&to_disable, false);
        if changed == 0 {
            return;
        }
        self.status = format!("Startup: disabled {changed} mod(s) missing dependencies");
        self.log_warn(format!(
            "Startup: disabled {changed} mod(s) missing dependencies"
        ));
        self.schedule_smart_rank_refresh(
            smart_rank::SmartRankRefreshMode::Incremental,
            "startup dependency disable",
            true,
        );
        self.queue_auto_deploy("startup dependency disable");
        if self.app_config.show_startup_dependency_notice {
            self.prompt_startup_dependency_notice(disabled_names);
        }
    }

    fn maybe_start_metadata_refresh(&mut self) {
        if self.metadata_cache_valid() {
            self.log_info("Metadata cache valid; skipping refresh".to_string());
            self.prime_dependency_cache_from_library();
            self.run_startup_dependency_check();
            self.schedule_smart_rank_warmup();
            return;
        }
        self.start_metadata_refresh();
    }

    fn metadata_cache_valid(&self) -> bool {
        if self.library.metadata_cache_version != METADATA_CACHE_VERSION {
            return false;
        }
        let Some(expected) = self.library.metadata_cache_key.as_deref() else {
            return false;
        };
        expected == self.metadata_cache_key()
    }

    fn metadata_cache_key(&self) -> String {
        let mut hasher = Hasher::new();
        hasher.update(b"metadata-cache-v1");
        let mut mods: Vec<&ModEntry> = self.library.mods.iter().collect();
        mods.sort_by(|a, b| a.id.cmp(&b.id));
        for mod_entry in mods {
            hasher.update(mod_entry.id.as_bytes());
            hasher.update(mod_entry.name.as_bytes());
            if let Some(label) = mod_entry.source_label.as_deref() {
                hasher.update(label.as_bytes());
            }
            let source_tag = match mod_entry.source {
                ModSource::Managed => 0u8,
                ModSource::Native => 1u8,
            };
            hasher.update(&[source_tag]);
            let mut targets: Vec<String> = Vec::new();
            for target in &mod_entry.targets {
                let key = match target {
                    InstallTarget::Pak { file, info } => {
                        format!("pak|{}|{}|{}", file, info.uuid, info.folder)
                    }
                    InstallTarget::Generated { dir } => format!("gen|{dir}"),
                    InstallTarget::Data { dir } => format!("data|{dir}"),
                    InstallTarget::Bin { dir } => format!("bin|{dir}"),
                };
                targets.push(key);
            }
            targets.sort();
            for target in targets {
                hasher.update(target.as_bytes());
            }
        }
        hasher.finalize().to_hex().to_string()
    }

    fn prime_dependency_cache_from_library(&mut self) {
        self.dependency_cache.clear();
        for mod_entry in &self.library.mods {
            if !mod_entry.dependencies.is_empty() {
                self.dependency_cache
                    .insert(mod_entry.id.clone(), mod_entry.dependencies.clone());
            } else {
                self.dependency_cache
                    .insert(mod_entry.id.clone(), Vec::new());
            }
        }
        self.dependency_cache_ready = true;
        self.refresh_dependency_blocks();
    }

    fn schedule_smart_rank_warmup(&mut self) {
        if !self.paths_ready() {
            return;
        }
        if self.smart_rank_cache.is_none() {
            self.load_smart_rank_cache();
        }
        let profile_key = self.smart_rank_profile_key();
        let mut desired = smart_rank::SmartRankRefreshMode::Full;
        if let Some(cache) = &self.smart_rank_cache {
            if cache.version == SMART_RANK_CACHE_VERSION {
                if cache.result.is_some()
                    && cache.profile_key == profile_key
                    && self.smart_rank_cache_ready(cache)
                {
                    self.smart_rank_refresh_pending = None;
                    self.smart_rank_refresh_at = None;
                    self.status = "Smart ranking warmup cached".to_string();
                    self.log_info("Smart rank warmup: cache hit".to_string());
                    return;
                }
                if cache.profile_key == profile_key {
                    desired = smart_rank::SmartRankRefreshMode::Incremental;
                } else if cache.result.is_some() && self.smart_rank_cache_ready(cache) {
                    desired = smart_rank::SmartRankRefreshMode::ReorderOnly;
                } else {
                    desired = smart_rank::SmartRankRefreshMode::Incremental;
                }
            }
        }
        self.smart_rank_refresh_pending = Some(desired);
        self.smart_rank_refresh_at = None;
    }

    fn smart_rank_cache_ready(&self, cache: &SmartRankCache) -> bool {
        Self::smart_rank_cache_ready_for(&self.library, cache)
    }

    fn smart_rank_cache_ready_for(library: &Library, cache: &SmartRankCache) -> bool {
        if cache.version != SMART_RANK_CACHE_VERSION {
            return false;
        }
        if cache.result.is_none() {
            return false;
        }
        let Some(profile) = library.active_profile() else {
            return false;
        };
        for entry in profile.order.iter().filter(|entry| entry.enabled) {
            let Some(mod_entry) = library
                .mods
                .iter()
                .find(|mod_entry| mod_entry.id == entry.id)
            else {
                continue;
            };
            let key = smart_rank::mod_cache_key(mod_entry);
            match cache.mod_cache.mods.get(&entry.id) {
                Some(entry_cache) if entry_cache.key == key && entry_cache.has_data => {}
                _ => return false,
            }
        }
        true
    }

    fn smart_rank_cache_missing_ids(&self, cache: &SmartRankCache) -> Vec<String> {
        Self::smart_rank_cache_missing_ids_for(&self.library, cache)
    }

    fn smart_rank_cache_missing_ids_for(library: &Library, cache: &SmartRankCache) -> Vec<String> {
        let mut missing = Vec::new();
        let Some(profile) = library.active_profile() else {
            return missing;
        };
        for entry in &profile.order {
            let Some(mod_entry) = library
                .mods
                .iter()
                .find(|mod_entry| mod_entry.id == entry.id)
            else {
                continue;
            };
            let key = smart_rank::mod_cache_key(mod_entry);
            match cache.mod_cache.mods.get(&entry.id) {
                Some(entry_cache) if entry_cache.key == key && entry_cache.has_data => {}
                _ => missing.push(entry.id.clone()),
            }
        }
        missing
    }

    fn merge_smart_rank_refresh_kind(
        &self,
        current: smart_rank::SmartRankRefreshMode,
        next: smart_rank::SmartRankRefreshMode,
    ) -> smart_rank::SmartRankRefreshMode {
        use smart_rank::SmartRankRefreshMode;
        match (current, next) {
            (SmartRankRefreshMode::Full, _) | (_, SmartRankRefreshMode::Full) => {
                SmartRankRefreshMode::Full
            }
            (SmartRankRefreshMode::Incremental, _) | (_, SmartRankRefreshMode::Incremental) => {
                SmartRankRefreshMode::Incremental
            }
            _ => SmartRankRefreshMode::ReorderOnly,
        }
    }

    fn schedule_smart_rank_refresh(
        &mut self,
        kind: smart_rank::SmartRankRefreshMode,
        reason: &str,
        debounce: bool,
    ) {
        if matches!(kind, smart_rank::SmartRankRefreshMode::Full) {
            self.smart_rank_cache = None;
            self.clear_smart_rank_cache_file();
        }
        self.interrupt_smart_rank(reason);
        let next = match self.smart_rank_refresh_pending.take() {
            Some(current) => self.merge_smart_rank_refresh_kind(current, kind),
            None => kind,
        };
        self.smart_rank_refresh_pending = Some(next);
        if debounce {
            self.smart_rank_refresh_at =
                Some(Instant::now() + Duration::from_millis(SMART_RANK_DEBOUNCE_MS));
        } else {
            self.smart_rank_refresh_at = None;
        }
    }

    fn resolve_smart_rank_refresh_kind(
        &self,
        requested: smart_rank::SmartRankRefreshMode,
    ) -> smart_rank::SmartRankRefreshMode {
        if matches!(requested, smart_rank::SmartRankRefreshMode::Full) {
            return requested;
        }
        let Some(cache) = &self.smart_rank_cache else {
            return smart_rank::SmartRankRefreshMode::Full;
        };
        if cache.version != SMART_RANK_CACHE_VERSION {
            return smart_rank::SmartRankRefreshMode::Full;
        }
        if matches!(requested, smart_rank::SmartRankRefreshMode::ReorderOnly)
            && !self.smart_rank_cache_ready(cache)
        {
            return smart_rank::SmartRankRefreshMode::Incremental;
        }
        requested
    }

    fn prompt_startup_dependency_notice(&mut self, disabled: Vec<String>) {
        if disabled.is_empty() || self.dialog.is_some() {
            return;
        }
        let total = disabled.len();
        let mut lines = Vec::new();
        lines.push(format!("Disabled {total} mod(s) missing dependencies."));
        lines.push(String::new());
        for (index, name) in disabled.iter().enumerate() {
            lines.push(format!("{}. {name}", index + 1));
        }
        self.open_dialog(Dialog {
            title: "Dependencies missing".to_string(),
            message: lines.join("\n"),
            yes_label: "OK".to_string(),
            no_label: "Hide next time".to_string(),
            choice: DialogChoice::Yes,
            kind: DialogKind::StartupDependencyNotice,
            toggle: None,
            scroll: 0,
        });
    }

    fn interrupt_smart_rank(&mut self, reason: &str) {
        if !self.smart_rank_active {
            return;
        }
        self.smart_rank_interrupt = true;
        self.smart_rank_progress = None;
        self.status = "Smart ranking paused for changes".to_string();
        self.log_info(format!("Smart rank interrupted: {reason}"));
    }

    fn clear_smart_rank_scan_state(&mut self) {
        self.smart_rank_active = false;
        self.smart_rank_progress = None;
        self.smart_rank_mode = None;
        self.smart_rank_refresh_kind = None;
        self.smart_rank_interrupt = false;
        self.smart_rank_scan_active = None;
        self.smart_rank_scan_profile_key = None;
    }

    fn allow_persistence(&self) -> bool {
        #[cfg(debug_assertions)]
        {
            return !self.debug_suppress_persistence;
        }
        #[cfg(not(debug_assertions))]
        {
            return true;
        }
    }

    fn smart_rank_scan_matches(&self, scan_id: u64) -> bool {
        self.smart_rank_scan_active == Some(scan_id)
    }

    fn smart_rank_scan_profile_matches(&self, profile_key: &str) -> bool {
        self.smart_rank_scan_profile_key.as_deref() == Some(profile_key)
    }

    fn maybe_restart_smart_rank(&mut self) {
        let Some(pending) = self.smart_rank_refresh_pending else {
            return;
        };
        if let Some(ready_at) = self.smart_rank_refresh_at {
            if Instant::now() < ready_at {
                return;
            }
        }
        if self.smart_rank_active || self.is_busy() {
            return;
        }
        if !self.paths_ready() {
            return;
        }
        self.smart_rank_refresh_pending = None;
        self.smart_rank_refresh_at = None;
        let refresh = self.resolve_smart_rank_refresh_kind(pending);
        self.start_smart_rank_scan(SmartRankMode::Warmup, refresh);
    }

    fn start_smart_rank_scan(
        &mut self,
        mode: SmartRankMode,
        refresh: smart_rank::SmartRankRefreshMode,
    ) {
        if self.smart_rank_active {
            return;
        }
        let profile_key = self.smart_rank_profile_key();
        if let Some(cache) = &self.smart_rank_cache {
            if cache.profile_key == profile_key && self.smart_rank_cache_ready(cache) {
                if let Some(result) = cache.result.clone() {
                    self.clear_smart_rank_scan_state();
                    match mode {
                        SmartRankMode::Preview => {
                            self.finalize_smart_rank_preview(result);
                        }
                        SmartRankMode::Warmup => {
                            self.log_info("Smart rank warmup: cache hit".to_string());
                            self.status = "Smart ranking warmup cached".to_string();
                        }
                    }
                    return;
                }
            }
            self.log_info("Smart rank cache miss (profile change)".to_string());
        }
        self.smart_rank_scan_id = self.smart_rank_scan_id.wrapping_add(1);
        let scan_id = self.smart_rank_scan_id;
        self.smart_rank_scan_active = Some(scan_id);
        self.smart_rank_scan_profile_key = Some(profile_key.clone());
        self.smart_rank_mode = Some(mode);
        self.smart_rank_active = true;
        self.smart_rank_refresh_kind = Some(refresh);
        self.smart_rank_progress = None;
        self.smart_rank_view = SmartRankView::Changes;
        self.smart_rank_scroll = 0;
        self.status = match mode {
            SmartRankMode::Preview => "Smart ranking: scanning...".to_string(),
            SmartRankMode::Warmup => "Smart ranking: warmup scan...".to_string(),
        };
        self.log_info("Smart ranking scan started".to_string());

        let config = self.config.clone();
        let library = self.library.clone();
        let cache_data = self
            .smart_rank_cache
            .as_ref()
            .map(|cache| cache.mod_cache.clone());
        let tx = self.smart_rank_tx.clone();
        thread::spawn(move || {
            let result = smart_rank::smart_rank_profile_cached_with_progress(
                &config,
                &library,
                cache_data.as_ref(),
                refresh,
                |progress| {
                    let _ = tx.send(SmartRankMessage::Progress { scan_id, progress });
                },
            );
            match result {
                Ok(result) => {
                    let _ = tx.send(SmartRankMessage::Finished {
                        scan_id,
                        computed: result,
                    });
                }
                Err(err) => {
                    let _ = tx.send(SmartRankMessage::Failed {
                        scan_id,
                        error: err.to_string(),
                    });
                }
            }
        });
    }

    fn finalize_smart_rank_preview(&mut self, result: smart_rank::SmartRankResult) {
        self.smart_rank_active = false;
        self.smart_rank_progress = None;
        self.smart_rank_mode = None;
        self.smart_rank_refresh_kind = None;
        self.smart_rank_interrupt = false;
        self.smart_rank_scan_active = None;
        self.smart_rank_scan_profile_key = None;

        self.log_info(format!(
            "Smart rank scan: loose {}/{} pak {}/{} in {}ms (missing loose {}, pak {})",
            result.report.scanned_loose,
            result.report.enabled_loose,
            result.report.scanned_pak,
            result.report.enabled_pak,
            result.report.elapsed_ms,
            result.report.missing_loose,
            result.report.missing_pak,
        ));

        let Some(profile) = self.library.active_profile() else {
            self.status = "Smart ranking skipped: no profile".to_string();
            return;
        };
        if result.order.len() != profile.order.len() {
            self.status = "Smart ranking skipped: incomplete order".to_string();
            self.log_warn("Smart ranking skipped: incomplete order".to_string());
            return;
        }

        let mod_map = self.library.index_by_id();
        let mut current_index = HashMap::new();
        for (index, entry) in profile.order.iter().enumerate() {
            current_index.insert(entry.id.clone(), index);
        }
        let mut proposed_index = HashMap::new();
        for (index, entry) in result.order.iter().enumerate() {
            proposed_index.insert(entry.id.clone(), index);
        }
        let mut moves = Vec::new();
        for (id, from) in current_index {
            let Some(to) = proposed_index.get(&id).copied() else {
                continue;
            };
            if from == to {
                continue;
            }
            let (name, created_at, added_at) = if let Some(mod_entry) = mod_map.get(&id) {
                (
                    mod_entry.display_name(),
                    mod_entry.created_at,
                    mod_entry.added_at,
                )
            } else {
                (id.clone(), None, 0)
            };
            moves.push(SmartRankMove {
                name,
                from,
                to,
                created_at,
                added_at,
            });
        }
        moves.sort_by_key(|entry| entry.from);

        let missing = result.report.missing;
        if moves.is_empty() && missing == 0 && result.warnings.is_empty() {
            self.status = "Smart ranking: no changes".to_string();
            return;
        }

        self.smart_rank_preview = Some(SmartRankPreview {
            proposed: result.order,
            report: result.report,
            moves,
            warnings: result.warnings,
            explain: result.explain,
        });
        self.smart_rank_scroll = 0;
        self.smart_rank_view = SmartRankView::Changes;
        self.status = "Smart ranking preview ready".to_string();
    }

    pub fn apply_smart_rank_preview(&mut self) {
        let Some(preview) = self.smart_rank_preview.take() else {
            return;
        };
        self.smart_rank_scroll = 0;
        self.smart_rank_view = SmartRankView::Changes;
        let Some(profile) = self.library.active_profile_mut() else {
            self.status = "Smart ranking skipped: no profile".to_string();
            return;
        };
        if preview.proposed.len() != profile.order.len() {
            self.status = "Smart ranking skipped: incomplete order".to_string();
            return;
        }
        profile.order = preview.proposed;
        if let Err(err) = self.library.save(&self.config.data_dir) {
            self.status = format!("Smart ranking save failed: {err}");
            self.log_error(format!("Smart ranking save failed: {err}"));
            return;
        }
        let moved = preview.report.moved;
        let total = preview.report.total;
        let missing = preview.report.missing;
        if missing > 0 {
            self.status = format!("Smart ranking applied: {moved}/{total} (missing {missing})");
        } else {
            self.status = format!("Smart ranking applied: {moved}/{total} mod(s)");
        }
        if preview.report.conflicts > 0 {
            self.log_info(format!(
                "Smart ranking analyzed {} conflict set(s)",
                preview.report.conflicts
            ));
        }
        for warning in preview.warnings {
            self.log_warn(warning);
        }
        self.queue_auto_deploy("smart ranking");
    }

    pub fn cancel_smart_rank_preview(&mut self) {
        if self.smart_rank_preview.take().is_some() {
            self.status = "Smart ranking canceled".to_string();
        }
        self.smart_rank_scroll = 0;
        self.smart_rank_view = SmartRankView::Changes;
    }

    pub fn conflicts_scanning(&self) -> bool {
        self.conflict_active
    }

    pub fn conflicts_pending(&self) -> bool {
        self.conflict_pending
    }

    pub fn is_busy(&self) -> bool {
        self.startup_pending
            || self.native_sync_active
            || self.import_active.is_some()
            || self.deploy_active
            || self.deploy_pending
            || self.conflict_active
            || self.conflict_pending
            || self.smart_rank_active
            || self.metadata_active
    }

    pub fn startup_pending(&self) -> bool {
        self.startup_pending
    }

    pub fn override_swap_info(&self) -> Option<OverrideSwapInfo> {
        if self.focus != Focus::Conflicts {
            return None;
        }
        if let Some(pending) = &self.pending_override {
            return Some(OverrideSwapInfo {
                from: pending.from.clone(),
                to: pending.to.clone(),
            });
        }
        let Some(info) = self.override_swap.as_ref() else {
            return None;
        };
        Some(OverrideSwapInfo {
            from: info.from.clone(),
            to: info.to.clone(),
        })
    }

    pub fn explorer_items(&self) -> Vec<ExplorerItem> {
        let mut items = Vec::new();

        for game_id in game::supported_games() {
            let expanded = self.explorer_game_expanded.contains(&game_id);
            let active = self.game_id == game_id;
            items.push(ExplorerItem {
                kind: ExplorerItemKind::Game(game_id),
                label: game_id.display_name().to_string(),
                depth: 0,
                active,
                expanded,
                disabled: false,
                renaming: false,
            });

            if !expanded {
                continue;
            }

            let profiles_expanded = self.explorer_profiles_expanded.contains(&game_id);
            items.push(ExplorerItem {
                kind: ExplorerItemKind::ProfilesHeader(game_id),
                label: "Profiles".to_string(),
                depth: 1,
                active: false,
                expanded: profiles_expanded,
                disabled: !active,
                renaming: false,
            });

            if !profiles_expanded {
                continue;
            }

            if active {
                for profile in &self.library.profiles {
                    let mut label = profile.name.clone();
                    let mut renaming = false;
                    if let Some((original, buffer)) = self.rename_preview() {
                        if original == profile.name {
                            renaming = true;
                            let trimmed = buffer.trim();
                            label = if trimmed.is_empty() {
                                "<new name>".to_string()
                            } else {
                                buffer
                            };
                        }
                    }
                    items.push(ExplorerItem {
                        kind: ExplorerItemKind::Profile {
                            name: profile.name.clone(),
                        },
                        label,
                        depth: 2,
                        active: profile.name == self.library.active_profile,
                        expanded: false,
                        disabled: false,
                        renaming,
                    });
                }
                items.push(ExplorerItem {
                    kind: ExplorerItemKind::NewProfile(game_id),
                    label: "New Profile...".to_string(),
                    depth: 2,
                    active: false,
                    expanded: false,
                    disabled: false,
                    renaming: false,
                });
            } else {
                items.push(ExplorerItem {
                    kind: ExplorerItemKind::Info(game_id),
                    label: "Select game to view profiles".to_string(),
                    depth: 2,
                    active: false,
                    expanded: false,
                    disabled: true,
                    renaming: false,
                });
            }
        }

        items
    }

    pub fn explorer_selected_item(&self) -> Option<ExplorerItem> {
        let items = self.explorer_items();
        items.get(self.explorer_selected).cloned()
    }

    pub fn explorer_move_up(&mut self) {
        if self.explorer_selected == 0 {
            return;
        }
        self.explorer_selected -= 1;
    }

    pub fn explorer_move_down(&mut self) {
        let len = self.explorer_items().len();
        if self.explorer_selected + 1 >= len {
            return;
        }
        self.explorer_selected += 1;
    }

    pub fn explorer_activate(&mut self) -> Result<()> {
        let Some(item) = self.explorer_selected_item() else {
            return Ok(());
        };

        match item.kind {
            ExplorerItemKind::Game(game_id) => {
                if game_id == self.game_id {
                    self.toggle_game_expanded(game_id);
                } else {
                    self.set_active_game(game_id)?;
                }
            }
            ExplorerItemKind::ProfilesHeader(game_id) => {
                self.toggle_profiles_expanded(game_id);
            }
            ExplorerItemKind::Profile { name, .. } => {
                self.set_active_profile(&name)?;
            }
            ExplorerItemKind::NewProfile(_) => {
                self.enter_create_profile();
            }
            ExplorerItemKind::Info(_) => {}
        }

        Ok(())
    }

    pub fn explorer_toggle_collapse(&mut self) {
        let Some(item) = self.explorer_selected_item() else {
            return;
        };
        match item.kind {
            ExplorerItemKind::Game(game_id) => {
                self.explorer_game_expanded.remove(&game_id);
            }
            ExplorerItemKind::ProfilesHeader(game_id) => {
                self.explorer_profiles_expanded.remove(&game_id);
            }
            _ => {}
        }
    }

    pub fn explorer_toggle_expand(&mut self) {
        let Some(item) = self.explorer_selected_item() else {
            return;
        };
        match item.kind {
            ExplorerItemKind::Game(game_id) => {
                self.explorer_game_expanded.insert(game_id);
            }
            ExplorerItemKind::ProfilesHeader(game_id) => {
                self.explorer_profiles_expanded.insert(game_id);
            }
            _ => {}
        }
    }

    fn toggle_game_expanded(&mut self, game_id: GameId) {
        if !self.explorer_game_expanded.insert(game_id) {
            self.explorer_game_expanded.remove(&game_id);
        }
    }

    fn toggle_profiles_expanded(&mut self, game_id: GameId) {
        if !self.explorer_profiles_expanded.insert(game_id) {
            self.explorer_profiles_expanded.remove(&game_id);
        }
    }

    pub fn cycle_focus(&mut self) {
        let next_focus = match self.focus {
            Focus::Explorer => Focus::Mods,
            Focus::Mods => Focus::Conflicts,
            Focus::Conflicts => Focus::Log,
            Focus::Log => Focus::Explorer,
        };
        self.set_focus(next_focus);
        self.status = match self.focus {
            Focus::Explorer => "Focus: explorer".to_string(),
            Focus::Mods => "Focus: mod stack".to_string(),
            Focus::Conflicts => "Focus: overrides".to_string(),
            Focus::Log => "Focus: log".to_string(),
        };
    }

    pub fn focus_mods(&mut self) {
        if self.focus != Focus::Mods {
            self.set_focus(Focus::Mods);
        }
    }

    fn set_focus(&mut self, focus: Focus) {
        if self.focus != focus {
            self.hotkey_pending_focus = Some(focus);
            self.hotkey_transition_at = Some(Instant::now());
            self.hotkey_fade_until = None;
        }
        self.focus = focus;
        self.move_mode = false;
    }

    pub fn set_active_game(&mut self, game_id: GameId) -> Result<()> {
        if game_id == self.game_id {
            return Ok(());
        }

        if self.import_active.is_some() || self.deploy_active || self.deploy_pending {
            self.status = "Game switch blocked: active tasks".to_string();
            self.log_warn("Game switch blocked: active tasks".to_string());
            return Ok(());
        }
        if self.conflict_active || self.conflict_pending {
            self.status = "Game switch blocked: override scan running".to_string();
            self.log_warn("Game switch blocked: override scan running".to_string());
            return Ok(());
        }

        self.library.save(&self.config.data_dir)?;
        self.config.save()?;

        let mut config = GameConfig::load_or_create(game_id)?;
        let mut library = Library::load_or_create(&config.data_dir)?;
        library.ensure_mods_in_profiles();
        if !config.active_profile.is_empty()
            && library
                .profiles
                .iter()
                .any(|profile| profile.name == config.active_profile)
        {
            library.active_profile = config.active_profile.clone();
        } else {
            config.active_profile = library.active_profile.clone();
        }
        library.save(&config.data_dir)?;
        config.save()?;

        self.game_id = game_id;
        self.config = config;
        self.library = library;
        self.app_config.active_game = game_id;
        self.app_config.save()?;
        self.log_path = self.config.data_dir.join("sigilsmith.log");
        self.explorer_game_expanded.insert(game_id);
        self.explorer_profiles_expanded.insert(game_id);
        self.explorer_selected = 0;
        self.conflicts.clear();
        self.conflict_selected = 0;

        self.selected = 0;
        self.set_focus(Focus::Mods);
        self.status = format!("Active game: {}", game_id.display_name());
        self.log_info(format!("Active game: {}", game_id.display_name()));
        self.ensure_setup();
        self.run_native_sync_inline();
        self.queue_conflict_scan("game changed");
        Ok(())
    }

    pub fn enter_create_profile(&mut self) {
        self.move_mode = false;
        self.input_mode = InputMode::Editing {
            prompt: "New profile name".to_string(),
            buffer: String::new(),
            purpose: InputPurpose::CreateProfile,
            auto_submit: false,
            last_edit_at: Instant::now(),
        };
        self.status = "Profile: enter new profile name".to_string();
    }

    pub fn enter_rename_profile(&mut self, original: &str) {
        self.move_mode = false;
        self.input_mode = InputMode::Editing {
            prompt: "Rename profile".to_string(),
            buffer: original.to_string(),
            purpose: InputPurpose::RenameProfile {
                original: original.to_string(),
            },
            auto_submit: false,
            last_edit_at: Instant::now(),
        };
        self.status = "Profile: enter new name".to_string();
    }

    pub fn enter_duplicate_profile(&mut self, source: &str) {
        let suggested = self.unique_profile_name(&format!("{source} Copy"));
        self.move_mode = false;
        self.input_mode = InputMode::Editing {
            prompt: "Duplicate profile".to_string(),
            buffer: suggested,
            purpose: InputPurpose::DuplicateProfile {
                source: source.to_string(),
            },
            auto_submit: false,
            last_edit_at: Instant::now(),
        };
        self.status = "Profile: enter duplicated profile name".to_string();
    }

    pub fn enter_export_profile(&mut self, profile: &str) {
        self.move_mode = false;
        self.input_mode = InputMode::Editing {
            prompt: "Export path".to_string(),
            buffer: String::new(),
            purpose: InputPurpose::ExportProfile {
                profile: profile.to_string(),
            },
            auto_submit: false,
            last_edit_at: Instant::now(),
        };
        self.status = format!("Export profile: {profile}");
    }

    pub fn enter_import_profile(&mut self) {
        self.move_mode = false;
        self.input_mode = InputMode::Editing {
            prompt: "Import mod list".to_string(),
            buffer: String::new(),
            purpose: InputPurpose::ImportProfile,
            auto_submit: false,
            last_edit_at: Instant::now(),
        };
        self.status = "Import mod list: enter path".to_string();
    }

    fn normalize_profile_name(name: &str) -> String {
        name.trim().to_string()
    }

    fn profile_exists(&self, name: &str) -> bool {
        self.library
            .profiles
            .iter()
            .any(|profile| profile.name.eq_ignore_ascii_case(name))
    }

    fn unique_profile_name(&self, base: &str) -> String {
        let base = base.trim();
        if base.is_empty() {
            return "Profile".to_string();
        }
        if !self.profile_exists(base) {
            return base.to_string();
        }
        for idx in 2..1000 {
            let candidate = format!("{base} ({idx})");
            if !self.profile_exists(&candidate) {
                return candidate;
            }
        }
        format!("{base} (copy)")
    }

    pub fn create_profile(&mut self, name: String) -> Result<()> {
        let name = Self::normalize_profile_name(&name);
        if name.is_empty() {
            self.status = "Profile name is required".to_string();
            self.set_toast(
                "Profile name required",
                ToastLevel::Warn,
                Duration::from_secs(3),
            );
            return Ok(());
        }
        if self.profile_exists(&name) {
            self.status = format!("Profile already exists: {name}");
            self.set_toast(
                "Profile already exists",
                ToastLevel::Warn,
                Duration::from_secs(3),
            );
            return Ok(());
        }

        let mut profile = crate::library::Profile::new(&name);
        let mod_ids: Vec<String> = self.library.mods.iter().map(|m| m.id.clone()).collect();
        profile.ensure_mods(&mod_ids);
        self.library.profiles.push(profile);
        self.set_active_profile(&name)?;
        self.log_info(format!("Profile created: {name}"));
        self.set_toast(
            &format!("Profile created: {name}"),
            ToastLevel::Info,
            Duration::from_secs(3),
        );
        Ok(())
    }

    pub fn rename_profile(&mut self, original: String, name: String) -> Result<()> {
        let name = Self::normalize_profile_name(&name);
        if name.is_empty() {
            self.status = "Profile name is required".to_string();
            self.set_toast(
                "Profile name required",
                ToastLevel::Warn,
                Duration::from_secs(3),
            );
            return Ok(());
        }
        if original.eq_ignore_ascii_case(&name) {
            self.status = "Profile name unchanged".to_string();
            self.set_toast("Rename cancelled", ToastLevel::Warn, Duration::from_secs(3));
            return Ok(());
        }
        if self.profile_exists(&name) {
            self.status = format!("Profile already exists: {name}");
            self.set_toast(
                "Profile already exists",
                ToastLevel::Warn,
                Duration::from_secs(3),
            );
            return Ok(());
        }

        let mut renamed = false;
        for profile in &mut self.library.profiles {
            if profile.name == original {
                profile.name = name.clone();
                renamed = true;
                break;
            }
        }
        if !renamed {
            self.status = "Profile not found".to_string();
            self.set_toast(
                "Profile not found",
                ToastLevel::Warn,
                Duration::from_secs(3),
            );
            return Ok(());
        }

        if self.library.active_profile == original {
            self.library.active_profile = name.clone();
        }
        self.config.active_profile = self.library.active_profile.clone();
        self.library.save(&self.config.data_dir)?;
        self.config.save()?;
        self.status = format!("Profile renamed: {name}");
        self.log_info(format!("Profile renamed: {original} -> {name}"));
        self.set_toast(
            &format!("Profile renamed to {name}"),
            ToastLevel::Info,
            Duration::from_secs(3),
        );
        Ok(())
    }

    pub fn duplicate_profile(&mut self, source: String, name: String) -> Result<()> {
        let name = Self::normalize_profile_name(&name);
        if name.is_empty() {
            self.status = "Profile name is required".to_string();
            self.set_toast(
                "Profile name required",
                ToastLevel::Warn,
                Duration::from_secs(3),
            );
            return Ok(());
        }
        if self.profile_exists(&name) {
            self.status = format!("Profile already exists: {name}");
            self.set_toast(
                "Profile already exists",
                ToastLevel::Warn,
                Duration::from_secs(3),
            );
            return Ok(());
        }

        let Some(source_profile) = self
            .library
            .profiles
            .iter()
            .find(|profile| profile.name == source)
            .cloned()
        else {
            self.status = "Profile not found".to_string();
            self.set_toast(
                "Profile not found",
                ToastLevel::Warn,
                Duration::from_secs(3),
            );
            return Ok(());
        };

        let mut copy = source_profile.clone();
        copy.name = name.clone();
        self.library.profiles.push(copy);
        self.set_active_profile(&name)?;
        self.log_info(format!("Profile duplicated: {source} -> {name}"));
        self.set_toast(
            &format!("Profile duplicated: {name}"),
            ToastLevel::Info,
            Duration::from_secs(3),
        );
        Ok(())
    }

    pub fn prompt_delete_profile(&mut self, name: String) {
        if self.dialog.is_some() {
            return;
        }

        let message = String::new();
        self.open_dialog(Dialog {
            title: "Delete Profile".to_string(),
            message,
            yes_label: "Delete".to_string(),
            no_label: "Cancel".to_string(),
            choice: DialogChoice::No,
            kind: DialogKind::DeleteProfile { name },
            toggle: Some(DialogToggle {
                label: "Don't ask again for this action?".to_string(),
                checked: false,
            }),
            scroll: 0,
        });
    }

    pub fn prompt_delete_mod(&mut self, id: String, name: String) {
        if self.dialog.is_some() {
            return;
        }

        let dependents = self.find_dependents(&id);
        let is_native = self
            .library
            .mods
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.is_native())
            .unwrap_or(false);
        let mut message = build_dependent_message(&dependents);
        let (title, yes_label, toggle) = if is_native {
            if !message.is_empty() {
                message.push('\n');
                message.push('\n');
            }
            message.push_str("This will delete the local mod file from the Larian Mods folder.");
            ("Remove Native Mod".to_string(), "Remove".to_string(), None)
        } else {
            (
                "Remove Mod".to_string(),
                "Remove".to_string(),
                Some(DialogToggle {
                    label: "Don't ask again for this action?".to_string(),
                    checked: false,
                }),
            )
        };
        self.open_dialog(Dialog {
            title,
            message,
            yes_label,
            no_label: "Cancel".to_string(),
            choice: DialogChoice::No,
            kind: DialogKind::DeleteMod {
                id,
                name,
                native: is_native,
                dependents,
            },
            toggle,
            scroll: 0,
        });
    }

    fn queue_pending_delete(&mut self, id: String, name: String) {
        self.pending_delete_mod = Some((id, name));
        if !self.metadata_active && !self.dependency_cache_ready {
            self.start_metadata_refresh();
        }
        self.status = "Checking dependencies...".to_string();
        self.set_toast(
            "Checking dependencies...",
            ToastLevel::Info,
            Duration::from_secs(2),
        );
    }

    fn maybe_prompt_pending_delete(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        let Some((id, name)) = self.pending_delete_mod.take() else {
            return;
        };
        if self.library.mods.iter().any(|entry| entry.id == id) {
            self.prompt_delete_mod(id, name);
        }
    }

    fn find_dependents(&self, target_id: &str) -> Vec<DependentMod> {
        let target_keys: HashSet<String> = self
            .library
            .mods
            .iter()
            .find(|entry| entry.id == target_id)
            .map(mod_dependency_keys)
            .unwrap_or_else(|| vec![normalize_label(target_id)])
            .into_iter()
            .collect();
        let mut out = Vec::new();
        for mod_entry in &self.library.mods {
            if mod_entry.id == target_id {
                continue;
            }
            let deps = self.cached_mod_dependencies(mod_entry);
            if deps.iter().any(|dep| {
                dependency_match_keys(dep)
                    .iter()
                    .any(|key| target_keys.contains(key))
            }) {
                out.push(DependentMod {
                    id: mod_entry.id.clone(),
                    name: mod_entry.display_name(),
                });
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    pub fn prompt_move_blocked(&mut self, resume_move_mode: bool) {
        if self.dialog.is_some() {
            return;
        }
        let mod_name = self
            .selected_profile_id()
            .and_then(|id| {
                self.library
                    .mods
                    .iter()
                    .find(|entry| entry.id == id)
                    .map(|entry| entry.display_name())
            })
            .unwrap_or_else(|| "mod".to_string());
        let mut message = String::new();
        if !self.mod_sort.is_order_default() {
            message.push_str(&format!(
                "Can't move while sorting by {} ({}).\n",
                self.mod_sort.column_label(),
                self.mod_sort.direction_label()
            ));
        }
        if self.mod_filter_active() {
            message.push_str("Can't move while search is active.\n");
        }
        let clear_filter = self.mod_filter_active();
        let suffix = if clear_filter {
            "Switch to Order view and clear search"
        } else {
            "Switch to Order view"
        };
        message.push_str(&format!("{suffix} to move \"{mod_name}\"?"));

        self.open_dialog(Dialog {
            title: "Move requires Order".to_string(),
            message,
            yes_label: "Switch".to_string(),
            no_label: "Cancel".to_string(),
            choice: DialogChoice::Yes,
            kind: DialogKind::MoveBlocked {
                resume_move_mode,
                clear_filter,
            },
            toggle: None,
            scroll: 0,
        });
    }

    pub fn prompt_cancel_import(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        self.open_dialog(Dialog {
            title: "Cancel Import".to_string(),
            message: "Cancel this import and return to the main view?".to_string(),
            yes_label: "Continue import".to_string(),
            no_label: "Cancel import".to_string(),
            choice: DialogChoice::No,
            kind: DialogKind::CancelImport,
            toggle: Some(DialogToggle {
                label: "Remember import choice".to_string(),
                checked: false,
            }),
            scroll: 0,
        });
    }

    pub fn delete_profile(&mut self, name: String) -> Result<()> {
        if self.library.profiles.len() <= 1 {
            self.status = "Cannot delete the last profile".to_string();
            self.set_toast(
                "At least one profile is required",
                ToastLevel::Warn,
                Duration::from_secs(3),
            );
            return Ok(());
        }
        if !self
            .library
            .profiles
            .iter()
            .any(|profile| profile.name == name)
        {
            self.status = "Profile not found".to_string();
            self.set_toast(
                "Profile not found",
                ToastLevel::Warn,
                Duration::from_secs(3),
            );
            return Ok(());
        }

        let was_active = self.library.active_profile == name;
        self.library.profiles.retain(|profile| profile.name != name);

        if self.library.profiles.is_empty() {
            self.library
                .profiles
                .push(crate::library::Profile::new("Default"));
        }

        if was_active {
            self.library.active_profile = self.library.profiles[0].name.clone();
            self.config.active_profile = self.library.active_profile.clone();
            self.selected = 0;
            self.move_mode = false;
            self.schedule_smart_rank_warmup();
            self.queue_auto_deploy("profile deleted");
        }

        self.library.save(&self.config.data_dir)?;
        self.config.save()?;
        self.status = format!("Profile deleted: {name}");
        self.log_info(format!("Profile deleted: {name}"));
        self.set_toast(
            &format!("Profile deleted: {name}"),
            ToastLevel::Info,
            Duration::from_secs(3),
        );
        Ok(())
    }

    pub fn set_active_profile(&mut self, name: &str) -> Result<()> {
        if !self.library.profiles.iter().any(|p| p.name == name) {
            self.status = "Profile not found".to_string();
            return Ok(());
        }
        self.library.active_profile = name.to_string();
        self.config.active_profile = name.to_string();
        self.library.save(&self.config.data_dir)?;
        self.config.save()?;
        self.selected = 0;
        self.move_mode = false;
        self.status = format!("Profile loaded: {name}");
        self.log_info(format!("Profile loaded: {name}"));
        self.schedule_smart_rank_warmup();
        self.queue_auto_deploy("profile changed");
        Ok(())
    }

    pub fn conflict_move_up(&mut self) {
        if self.conflict_selected == 0 {
            return;
        }
        self.conflict_selected -= 1;
        self.pending_override = None;
    }

    pub fn conflict_move_down(&mut self) {
        if self.conflict_selected + 1 >= self.conflicts.len() {
            return;
        }
        self.conflict_selected += 1;
        self.pending_override = None;
    }

    pub fn cycle_conflict_winner(&mut self, delta: i32) {
        let Some(conflict) = self.conflicts.get(self.conflict_selected).cloned() else {
            return;
        };
        if conflict.candidates.is_empty() {
            return;
        }
        let current_index = conflict
            .candidates
            .iter()
            .position(|candidate| candidate.mod_id == conflict.winner_id)
            .unwrap_or(0);
        let len = conflict.candidates.len() as i32;
        let next_index = (current_index as i32 + delta).rem_euclid(len) as usize;
        let winner_id = conflict.candidates[next_index].mod_id.clone();
        self.schedule_conflict_winner(winner_id);
    }

    pub fn clear_conflict_override(&mut self) {
        let Some(conflict) = self.conflicts.get(self.conflict_selected).cloned() else {
            return;
        };
        self.schedule_conflict_winner(conflict.default_winner_id);
    }

    fn schedule_conflict_winner(&mut self, winner_id: String) {
        let Some(conflict) = self.conflicts.get(self.conflict_selected) else {
            return;
        };
        if winner_id == conflict.winner_id {
            self.pending_override = None;
            return;
        }
        let to_name = conflict
            .candidates
            .iter()
            .find(|candidate| candidate.mod_id == winner_id)
            .map(|candidate| candidate.mod_name.clone())
            .unwrap_or_else(|| winner_id.clone());
        self.pending_override = Some(PendingOverride {
            conflict_index: self.conflict_selected,
            winner_id,
            from: conflict.winner_name.clone(),
            to: to_name,
            last_input: Instant::now(),
        });
    }

    fn set_conflict_winner(&mut self, index: usize, winner_id: String) -> Result<()> {
        let Some(conflict) = self.conflicts.get(index).cloned() else {
            return Ok(());
        };
        let Some(profile) = self.library.active_profile_mut() else {
            return Ok(());
        };

        let rel_path = conflict.relative_path.to_string_lossy().to_string();
        if winner_id == conflict.default_winner_id {
            profile.file_overrides.retain(|override_entry| {
                override_entry.kind != conflict.target || override_entry.relative_path != rel_path
            });
        } else if let Some(existing) = profile.file_overrides.iter_mut().find(|override_entry| {
            override_entry.kind == conflict.target && override_entry.relative_path == rel_path
        }) {
            existing.mod_id = winner_id.clone();
        } else {
            profile.file_overrides.push(FileOverride {
                kind: conflict.target,
                relative_path: rel_path.clone(),
                mod_id: winner_id.clone(),
            });
        }

        self.library.save(&self.config.data_dir)?;
        let previous_name = conflict.winner_name.clone();
        let mut updated = conflict.clone();
        updated.winner_id = winner_id.clone();
        let updated_name = updated
            .candidates
            .iter()
            .find(|candidate| candidate.mod_id == winner_id)
            .map(|candidate| candidate.mod_name.clone())
            .unwrap_or_else(|| winner_id.clone());
        updated.winner_name = updated_name.clone();
        updated.overridden = updated.winner_id != updated.default_winner_id;
        self.conflicts[index] = updated;
        if previous_name != updated_name {
            self.override_swap = Some(OverrideSwap {
                from: previous_name,
                to: updated_name,
            });
        } else {
            self.override_swap = None;
        }

        self.status = "Override updated".to_string();
        self.log_info("Override updated".to_string());
        self.queue_auto_deploy("conflict override");
        Ok(())
    }

    pub fn export_profile(&mut self, profile: String, path: String) -> Result<()> {
        let path = expand_tilde(path.trim());
        if path.as_os_str().is_empty() {
            self.status = "Export path is required".to_string();
            self.set_toast(
                "Export path required",
                ToastLevel::Warn,
                Duration::from_secs(3),
            );
            return Ok(());
        }

        let Some(profile_data) = self
            .library
            .profiles
            .iter()
            .find(|entry| entry.name == profile)
        else {
            self.status = "Profile not found".to_string();
            self.set_toast(
                "Profile not found",
                ToastLevel::Warn,
                Duration::from_secs(3),
            );
            return Ok(());
        };

        let mod_map = self.library.index_by_id();
        let entries = profile_data
            .order
            .iter()
            .filter_map(|entry| mod_map.get(&entry.id).map(|mod_entry| (entry, mod_entry)))
            .map(|(entry, mod_entry)| ProfileExportEntry {
                id: entry.id.clone(),
                name: mod_entry.display_name(),
                enabled: entry.enabled,
            })
            .collect();

        let export = ProfileExport {
            game_id: self.game_id.as_str().to_string(),
            game_name: self.game_id.display_name().to_string(),
            profile_name: profile_data.name.clone(),
            entries,
            file_overrides: profile_data.file_overrides.clone(),
        };

        let raw = serde_json::to_string_pretty(&export).context("serialize profile export")?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).context("create export dir")?;
            }
        }
        fs::write(&path, raw).context("write profile export")?;
        self.status = format!("Profile exported: {}", path.display());
        self.log_info(format!("Profile exported: {}", path.display()));
        self.set_toast(
            &format!("Profile exported: {}", path.display()),
            ToastLevel::Info,
            Duration::from_secs(3),
        );
        Ok(())
    }

    pub fn import_profile(&mut self, path: String) -> Result<()> {
        let path = expand_tilde(path.trim());
        if !path.exists() {
            self.status = format!("Path not found: {}", path.display());
            self.set_toast(
                "Import path not found",
                ToastLevel::Warn,
                Duration::from_secs(3),
            );
            return Ok(());
        }

        let raw = fs::read_to_string(&path).context("read profile export")?;
        let export: ProfileExport = serde_json::from_str(&raw).context("parse profile export")?;
        let mut mismatch = false;
        if export.game_id != self.game_id.as_str() {
            mismatch = true;
            self.log_warn(format!(
                "Profile export game mismatch: expected {}, got {}",
                self.game_id.as_str(),
                export.game_id
            ));
        }

        let base_name = Self::normalize_profile_name(&export.profile_name);
        let mut name = if base_name.is_empty() {
            "Imported Profile".to_string()
        } else {
            base_name
        };
        name = self.unique_profile_name(&name);

        let mut profile = crate::library::Profile::new(&name);
        let mut missing = Vec::new();
        let mut seen = HashSet::new();

        for entry in export.entries {
            let mut found_id = None;
            if self
                .library
                .mods
                .iter()
                .any(|mod_entry| mod_entry.id == entry.id)
            {
                found_id = Some(entry.id);
            } else {
                for mod_entry in &self.library.mods {
                    if mod_entry
                        .display_name()
                        .eq_ignore_ascii_case(entry.name.trim())
                        || mod_entry.name.eq_ignore_ascii_case(entry.name.trim())
                    {
                        found_id = Some(mod_entry.id.clone());
                        break;
                    }
                }
            }

            let Some(id) = found_id else {
                missing.push(entry.name);
                continue;
            };
            if !seen.insert(id.clone()) {
                continue;
            }
            profile.order.push(ProfileEntry {
                id,
                enabled: entry.enabled,
            });
        }

        let mut overrides = export.file_overrides;
        overrides.retain(|override_entry| {
            self.library
                .mods
                .iter()
                .any(|mod_entry| mod_entry.id == override_entry.mod_id)
        });
        profile.file_overrides = overrides;

        let mod_ids: Vec<String> = self.library.mods.iter().map(|m| m.id.clone()).collect();
        profile.ensure_mods(&mod_ids);
        self.library.profiles.push(profile);
        self.set_active_profile(&name)?;

        let mut toast_level = ToastLevel::Info;
        let mut toast_message = format!("Profile imported: {name}");
        if !missing.is_empty() {
            self.log_warn(format!(
                "Profile import skipped {} missing mod(s)",
                missing.len()
            ));
            toast_level = ToastLevel::Warn;
            toast_message = format!("Profile imported: {name} (missing {} mods)", missing.len());
        } else if mismatch {
            toast_level = ToastLevel::Warn;
            toast_message = format!("Profile imported: {name} (game mismatch)");
        }
        self.status = format!("Profile imported: {name}");
        self.log_info(format!("Profile imported: {name}"));
        self.set_toast(&toast_message, toast_level, Duration::from_secs(3));
        Ok(())
    }

    pub fn tick(&mut self) {
        if let Some(toast) = &self.toast {
            if toast.expires_at <= Instant::now() {
                self.toast = None;
            }
        }

        if let Some(pending) = &self.pending_override {
            if pending.last_input.elapsed() >= Duration::from_millis(250) {
                let pending = self.pending_override.take().unwrap();
                if pending.conflict_index >= self.conflicts.len() {
                    return;
                }
                if let Err(err) =
                    self.set_conflict_winner(pending.conflict_index, pending.winner_id)
                {
                    self.status = format!("Override failed: {err}");
                    self.log_error(format!("Override failed: {err}"));
                }
            }
        }

        self.maybe_debounce_mod_filter();
        self.update_hotkey_transition();

        if self.update_active {
            if let Some(started_at) = self.update_started_at {
                if started_at.elapsed() >= Duration::from_secs(15) {
                    self.update_active = false;
                    self.update_started_at = None;
                    self.update_status = UpdateStatus::Failed {
                        error: "timeout".to_string(),
                    };
                    self.log_warn("Update check timed out".to_string());
                }
            }
        }
    }

    fn maybe_debounce_mod_filter(&mut self) {
        let (buffer, last_edit_at) = match &self.input_mode {
            InputMode::Editing {
                purpose: InputPurpose::FilterMods,
                buffer,
                last_edit_at,
                ..
            } => (buffer.clone(), *last_edit_at),
            _ => return,
        };

        if last_edit_at.elapsed() < Duration::from_millis(SEARCH_DEBOUNCE_MS) {
            return;
        }

        let value = buffer.trim().to_string();
        if value == self.mod_filter {
            return;
        }
        self.apply_mod_filter(value, false);
    }

    fn update_hotkey_transition(&mut self) {
        let Some(pending) = self.hotkey_pending_focus else {
            return;
        };
        let Some(started_at) = self.hotkey_transition_at else {
            return;
        };
        if started_at.elapsed() < Duration::from_millis(HOTKEY_DEBOUNCE_MS) {
            return;
        }
        self.hotkey_focus = pending;
        self.hotkey_pending_focus = None;
        self.hotkey_transition_at = None;
        self.hotkey_fade_until = Some(Instant::now() + Duration::from_millis(HOTKEY_FADE_MS));
    }

    pub fn paths_ready(&self) -> bool {
        game::looks_like_game_root(self.game_id, &self.config.game_root)
            && game::looks_like_user_dir(self.game_id, &self.config.larian_dir)
    }

    pub fn import_overlay_active(&self) -> bool {
        self.import_active.is_some() || self.import_progress.is_some()
    }

    pub fn import_progress(&self) -> Option<&importer::ImportProgress> {
        self.import_progress.as_ref()
    }

    pub fn import_summary_pending(&self) -> bool {
        self.import_summary_pending
    }

    pub fn hotkey_fade_active(&self) -> bool {
        self.hotkey_fade_until
            .map(|until| until > Instant::now())
            .unwrap_or(false)
    }

    fn ensure_setup(&mut self) {
        if self.paths_ready() {
            return;
        }

        let step = if game::looks_like_game_root(self.game_id, &self.config.game_root) {
            SetupStep::LarianDir
        } else {
            SetupStep::GameRoot
        };
        self.start_setup(step);
    }

    fn start_setup(&mut self, step: SetupStep) {
        match step {
            SetupStep::GameRoot => self.enter_setup_game_root(),
            SetupStep::LarianDir => self.enter_setup_larian_dir(),
            SetupStep::DownloadsDir => self.enter_setup_downloads_dir(),
        }
    }

    pub fn enter_setup_game_root(&mut self) {
        self.move_mode = false;
        self.open_path_browser(SetupStep::GameRoot);
    }

    pub fn enter_setup_larian_dir(&mut self) {
        self.move_mode = false;
        self.open_path_browser(SetupStep::LarianDir);
    }

    pub fn enter_setup_downloads_dir(&mut self) {
        self.move_mode = false;
        self.open_path_browser(SetupStep::DownloadsDir);
    }

    fn open_path_browser(&mut self, step: SetupStep) {
        let current = self.path_browser_start(step);
        let entries = self.build_path_browser_entries(step, &current);
        let title = match step {
            SetupStep::GameRoot => "Select BG3 install root (Data/ + bin/)",
            SetupStep::LarianDir => "Select Larian data dir (PlayerProfiles/)",
            SetupStep::DownloadsDir => "Select downloads folder",
        };
        let input_seed = current.display().to_string();
        self.input_mode = InputMode::Browsing(PathBrowser {
            step,
            current,
            entries,
            selected: 0,
            path_input: input_seed,
            focus: PathBrowserFocus::List,
        });
        self.status = title.to_string();
    }

    fn path_browser_start(&self, step: SetupStep) -> PathBuf {
        let home = BaseDirs::new()
            .map(|base| base.home_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("/"));
        let mut candidates = Vec::new();
        match step {
            SetupStep::GameRoot => {
                if !self.config.game_root.as_os_str().is_empty() {
                    candidates.push(self.config.game_root.clone());
                }
                candidates.push(home.join(".steam/steam/steamapps/common"));
                candidates.push(home.join(".local/share/Steam/steamapps/common"));
            }
            SetupStep::LarianDir => {
                if !self.config.larian_dir.as_os_str().is_empty() {
                    candidates.push(self.config.larian_dir.clone());
                }
                candidates.push(home.join(".local/share/Larian Studios"));
                candidates.push(home.join(
                    ".local/share/Steam/steamapps/compatdata/1086940/pfx/drive_c/users/steamuser/AppData/Local/Larian Studios",
                ));
            }
            SetupStep::DownloadsDir => {
                if !self.app_config.downloads_dir.as_os_str().is_empty() {
                    candidates.push(self.app_config.downloads_dir.clone());
                }
                candidates.push(home.join("Downloads"));
            }
        }
        candidates
            .into_iter()
            .find(|path| path.is_dir())
            .unwrap_or(home)
    }

    fn path_browser_selectable(&self, step: SetupStep, path: &PathBuf) -> bool {
        match step {
            SetupStep::GameRoot => game::looks_like_game_root(self.game_id, path),
            SetupStep::LarianDir => game::looks_like_user_dir(self.game_id, path),
            SetupStep::DownloadsDir => path.is_dir(),
        }
    }

    pub(crate) fn build_path_browser_entries(
        &self,
        step: SetupStep,
        current: &PathBuf,
    ) -> Vec<PathBrowserEntry> {
        let mut entries = Vec::new();
        let selectable = self.path_browser_selectable(step, current);
        entries.push(PathBrowserEntry {
            label: "[ Select this folder ]".to_string(),
            path: current.clone(),
            kind: PathBrowserEntryKind::Select,
            selectable,
        });
        if let Some(parent) = current.parent() {
            entries.push(PathBrowserEntry {
                label: "..".to_string(),
                path: parent.to_path_buf(),
                kind: PathBrowserEntryKind::Parent,
                selectable: false,
            });
        }
        let mut dirs = Vec::new();
        if let Ok(read_dir) = fs::read_dir(current) {
            for entry in read_dir.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(|name| format!("{name}/"))
                        .unwrap_or_else(|| path.display().to_string());
                    dirs.push(PathBrowserEntry {
                        label: name,
                        path,
                        kind: PathBrowserEntryKind::Dir,
                        selectable: false,
                    });
                }
            }
        }
        dirs.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
        entries.extend(dirs);
        entries
    }

    pub(crate) fn apply_path_browser_selection(
        &mut self,
        step: SetupStep,
        path: PathBuf,
    ) -> Result<()> {
        self.input_mode = InputMode::Normal;
        match step {
            SetupStep::GameRoot => self.submit_game_root_path(path),
            SetupStep::LarianDir => self.submit_larian_dir_path(path),
            SetupStep::DownloadsDir => self.submit_downloads_dir_path(path),
        }
    }

    pub fn enter_import_mode(&mut self) {
        if self.block_mod_changes("import") {
            return;
        }
        self.move_mode = false;
        self.input_mode = InputMode::Editing {
            prompt: "Import path".to_string(),
            buffer: String::new(),
            purpose: InputPurpose::ImportPath,
            auto_submit: false,
            last_edit_at: Instant::now(),
        };
        self.status = "Import: paste a file or folder path, then press Enter".to_string();
    }

    pub fn enter_mod_filter(&mut self) {
        self.move_mode = false;
        self.mod_filter_snapshot = Some(self.mod_filter.clone());
        self.input_mode = InputMode::Editing {
            prompt: "Search mods".to_string(),
            buffer: self.mod_filter.clone(),
            purpose: InputPurpose::FilterMods,
            auto_submit: false,
            last_edit_at: Instant::now(),
        };
        self.status = "Search mods: type (updates after pause, Enter to apply)".to_string();
    }

    pub fn clear_mod_filter(&mut self) {
        if self.mod_filter.trim().is_empty() {
            self.status = "Search already cleared".to_string();
            return;
        }
        self.mod_filter_snapshot = None;
        self.apply_mod_filter(String::new(), true);
    }

    pub fn dependency_queue_active(&self) -> bool {
        self.dependency_queue.is_some()
    }

    pub fn dependency_queue_enable_pending(&self) -> bool {
        self.pending_dependency_enable.is_some()
    }

    pub fn set_dependency_queue_view(&mut self, view_items: usize) {
        self.dependency_queue_view = view_items.max(1);
    }

    pub fn dependency_queue_page_step(&self) -> isize {
        let step = self.dependency_queue_view.saturating_sub(1).max(1);
        step as isize
    }

    pub fn dependency_queue_move(&mut self, delta: isize) {
        let Some(queue) = &mut self.dependency_queue else {
            return;
        };
        if queue.items.is_empty() {
            queue.selected = 0;
            return;
        }
        let len = queue.items.len() as isize;
        let mut next = queue.selected as isize + delta;
        if next < 0 {
            next = 0;
        }
        if next >= len {
            next = len - 1;
        }
        queue.selected = next as usize;
    }

    pub fn dependency_queue_home(&mut self) {
        if let Some(queue) = &mut self.dependency_queue {
            queue.selected = 0;
        }
    }

    pub fn dependency_queue_end(&mut self) {
        if let Some(queue) = &mut self.dependency_queue {
            if !queue.items.is_empty() {
                queue.selected = queue.items.len() - 1;
            }
        }
    }

    pub fn dependency_queue_continue(&mut self) {
        self.finish_dependency_queue(true);
    }

    pub fn dependency_queue_cancel(&mut self) {
        if self.pending_import_batch.is_some() {
            self.prompt_cancel_import();
            return;
        }
        self.dependency_queue = None;
        self.pending_dependency_enable = None;
        self.status = "Dependency check canceled".to_string();
    }

    pub fn dependency_queue(&self) -> Option<&DependencyQueue> {
        self.dependency_queue.as_ref()
    }

    pub fn dependency_queue_open_selected(&mut self) {
        let is_override = self
            .dependency_queue_selected()
            .map(|item| item.is_override_action())
            .unwrap_or(false);
        if is_override {
            self.prompt_dependency_override();
            return;
        }
        let Some((link, search, label)) = self.dependency_queue_selected().map(|item| {
            (
                item.link.clone(),
                item.search_link.clone(),
                item.search_label.clone(),
            )
        }) else {
            return;
        };
        if let Some(link) = link {
            self.open_link(&link);
            return;
        }
        if let Some(search) = search {
            self.open_link(&search);
            if label == "Unknown dependency" {
                self.maybe_prompt_copy_search_link(&search, &label);
            }
            return;
        }
        self.status = "No links available".to_string();
        self.set_toast(
            "No links available",
            ToastLevel::Warn,
            Duration::from_secs(2),
        );
    }

    pub fn dependency_queue_copy_link(&mut self) {
        let Some((is_override, link, search)) = self.dependency_queue_selected().map(|item| {
            (
                item.is_override_action(),
                item.link.clone(),
                item.search_link.clone(),
            )
        }) else {
            return;
        };
        if is_override {
            return;
        }
        if let Some(link) = link {
            if self.copy_to_clipboard(&link) {
                self.status = "Link copied".to_string();
            }
            return;
        }
        if let Some(search) = search {
            if self.copy_to_clipboard(&search) {
                self.status = "Search link copied".to_string();
            }
            return;
        }
        self.status = "No link available".to_string();
        self.set_toast(
            "No link available",
            ToastLevel::Warn,
            Duration::from_secs(2),
        );
    }

    pub fn dependency_queue_copy_uuid(&mut self) {
        let Some((is_override, uuid, label)) = self.dependency_queue_selected().map(|item| {
            (
                item.is_override_action(),
                item.uuid.clone(),
                item.label.clone(),
            )
        }) else {
            return;
        };
        if is_override {
            return;
        }
        if let Some(uuid) = uuid {
            if self.copy_to_clipboard(&uuid) {
                self.status = "Dependency UUID copied".to_string();
            }
            return;
        }
        if self.copy_to_clipboard(&label) {
            self.status = "Dependency id copied".to_string();
        }
    }

    fn prompt_dependency_override(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        let (title, message) = if self.dependency_queue_enable_pending() {
            (
                "Override Dependencies".to_string(),
                "Enable mods without resolving dependencies?\nThis can break your load order."
                    .to_string(),
            )
        } else {
            (
                "Override Dependencies".to_string(),
                "Continue import without resolving dependencies?\nThis can break your load order."
                    .to_string(),
            )
        };
        self.open_dialog(Dialog {
            title,
            message,
            yes_label: "Override".to_string(),
            no_label: "Cancel".to_string(),
            choice: DialogChoice::No,
            kind: DialogKind::OverrideDependencies,
            toggle: None,
            scroll: 0,
        });
    }

    fn maybe_prompt_copy_search_link(&mut self, link: &str, label: &str) {
        match self.app_config.dependency_search_copy_preference {
            Some(true) => {
                if self.copy_to_clipboard(link) {
                    self.status = "Search link copied".to_string();
                }
            }
            Some(false) => {
                self.status = "Search link available".to_string();
            }
            None => {
                if self.dialog.is_some() {
                    return;
                }
                let display_label = if label.trim().is_empty() {
                    "dependency"
                } else {
                    label
                };
                let message = format!(
                    "No direct link found for \"{display_label}\".\nCopy Nexus search link to clipboard?"
                );
                self.open_dialog(Dialog {
                    title: "Copy Search Link".to_string(),
                    message,
                    yes_label: "Copy".to_string(),
                    no_label: "Skip".to_string(),
                    choice: DialogChoice::No,
                    kind: DialogKind::CopyDependencySearchLink {
                        link: link.to_string(),
                    },
                    toggle: Some(DialogToggle {
                        label: "Remember my choice".to_string(),
                        checked: false,
                    }),
                    scroll: 0,
                });
            }
        }
    }

    fn open_external(&mut self, target: &str, label: &str) {
        let mut errors = Vec::new();
        let candidates = [
            ("xdg-open", vec![target]),
            ("gio", vec!["open", target]),
            ("kde-open5", vec![target]),
            ("kioclient5", vec!["exec", target]),
        ];
        for (command, args) in candidates {
            match Command::new(command)
                .args(&args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
            {
                Ok(status) if status.success() => {
                    self.status = format!("Opened {label}");
                    return;
                }
                Ok(status) => {
                    errors.push(format!("{command} exited {status}"));
                }
                Err(err) => {
                    errors.push(format!("{command} failed: {err}"));
                }
            }
        }
        self.status = format!("Failed to open {label}");
        if errors.is_empty() {
            self.log_warn(format!("Failed to open {label}"));
        } else {
            self.log_warn(format!("Failed to open {label}: {}", errors.join("; ")));
        }
    }

    fn copy_to_clipboard(&mut self, text: &str) -> bool {
        let result = match self.clipboard_mut() {
            Some(clipboard) => clipboard.set_text(text.to_string()),
            None => return false,
        };
        if let Err(err) = result {
            self.status = format!("Clipboard copy failed: {err}");
            self.log_warn(format!("Clipboard copy failed: {err}"));
            return false;
        }
        true
    }

    fn clipboard_mut(&mut self) -> Option<&mut Clipboard> {
        if self.clipboard.is_none() {
            match Clipboard::new() {
                Ok(clipboard) => {
                    self.clipboard = Some(clipboard);
                }
                Err(err) => {
                    self.status = format!("Clipboard unavailable: {err}");
                    self.log_warn(format!("Clipboard unavailable: {err}"));
                    return None;
                }
            }
        }
        self.clipboard.as_mut()
    }

    pub fn open_link(&mut self, link: &str) {
        if link.trim().is_empty() {
            return;
        }
        self.open_external(link, "link");
    }

    fn block_mod_changes(&mut self, action: &str) -> bool {
        if self.metadata_active {
            self.status = format!("Metadata scan running: {action} blocked");
            self.set_toast(
                "Metadata scan in progress - please wait",
                ToastLevel::Warn,
                Duration::from_secs(2),
            );
            return true;
        }
        if !self.native_sync_active {
            if self.smart_rank_active {
                let allow_during_warmup =
                    matches!(self.smart_rank_mode, Some(SmartRankMode::Warmup))
                        && matches!(
                            action,
                            "toggle" | "enable" | "disable" | "reorder" | "remove"
                        );
                if !allow_during_warmup {
                    let label = match self.smart_rank_mode {
                        Some(SmartRankMode::Warmup) => "Smart rank warmup",
                        Some(SmartRankMode::Preview) => "Smart rank preview",
                        None => "Smart ranking",
                    };
                    self.status = format!("{label} running: {action} blocked");
                    self.set_toast(
                        "Smart ranking in progress - please wait",
                        ToastLevel::Warn,
                        Duration::from_secs(2),
                    );
                    return true;
                }
            }
            if self.deploy_active {
                self.status = format!("Deploy running: {action} blocked");
                self.set_toast(
                    "Deploy in progress - please wait",
                    ToastLevel::Warn,
                    Duration::from_secs(2),
                );
                return true;
            }
            if self.import_active.is_some() {
                self.status = format!("Import running: {action} blocked");
                self.set_toast(
                    "Import in progress - please wait",
                    ToastLevel::Warn,
                    Duration::from_secs(2),
                );
                return true;
            }
            return false;
        }
        self.status = format!("Startup sync running: {action} blocked");
        self.set_toast(
            "Startup sync in progress - please wait",
            ToastLevel::Warn,
            Duration::from_secs(2),
        );
        true
    }

    #[cfg(debug_assertions)]
    fn block_mod_changes_warmup(&self, action: &str) -> bool {
        if self.metadata_active {
            return true;
        }
        if !self.native_sync_active {
            if self.smart_rank_active {
                let allow_during_warmup =
                    matches!(self.smart_rank_mode, Some(SmartRankMode::Warmup))
                        && matches!(
                            action,
                            "toggle" | "enable" | "disable" | "reorder" | "remove"
                        );
                if !allow_during_warmup {
                    return true;
                }
            }
            if self.deploy_active {
                return true;
            }
            if self.import_active.is_some() {
                return true;
            }
            return false;
        }
        true
    }

    fn dependency_queue_selected(&self) -> Option<&DependencyItem> {
        let queue = self.dependency_queue.as_ref()?;
        queue.items.get(queue.selected)
    }

    pub fn handle_submit(&mut self, purpose: InputPurpose, value: String) -> Result<()> {
        match purpose {
            InputPurpose::ImportPath => self.import_mod(value),
            InputPurpose::CreateProfile => self.create_profile(value),
            InputPurpose::RenameProfile { original } => self.rename_profile(original, value),
            InputPurpose::DuplicateProfile { source } => self.duplicate_profile(source, value),
            InputPurpose::ExportProfile { profile } => self.export_profile(profile, value),
            InputPurpose::ImportProfile => self.import_profile(value),
            InputPurpose::FilterMods => {
                self.apply_mod_filter(value, true);
                self.mod_filter_snapshot = None;
                Ok(())
            }
        }
    }

    fn apply_mod_filter(&mut self, value: String, announce: bool) {
        let trimmed = value.trim();
        let previous = self.selected_profile_id();
        self.mod_filter = trimmed.to_string();
        self.selected = 0;
        if let Some(previous_id) = previous {
            if let Some(profile) = self.library.active_profile() {
                let indices = self.visible_profile_indices();
                if let Some(pos) = indices.iter().position(|index| {
                    profile
                        .order
                        .get(*index)
                        .map(|entry| entry.id == previous_id)
                        .unwrap_or(false)
                }) {
                    self.selected = pos;
                }
            }
        }
        if announce {
            if self.mod_filter.is_empty() {
                self.status = "Search cleared".to_string();
                self.log_info("Search cleared".to_string());
            } else {
                self.status = format!("Search set: \"{}\"", self.mod_filter);
                self.log_info(format!("Search set: \"{}\"", self.mod_filter));
            }
        }
        self.clamp_selection();
    }

    pub fn cancel_mod_filter(&mut self) {
        if let Some(snapshot) = self.mod_filter_snapshot.take() {
            if snapshot != self.mod_filter {
                self.apply_mod_filter(snapshot, false);
            }
        }
    }

    pub fn import_mod(&mut self, raw_path: String) -> Result<()> {
        if self.block_mod_changes("import") {
            return Ok(());
        }
        let path = expand_tilde(raw_path.trim());
        if !path.exists() {
            let display = display_path(&path);
            self.status = format!("Import failed: {display} (not found)");
            self.log_warn(format!("Import path not found: {}", path.display()));
            self.set_toast(
                &format!("Import failed: {display} (not found)"),
                ToastLevel::Warn,
                Duration::from_secs(3),
            );
            return Ok(());
        }

        self.import_queue.push_back(path.clone());
        self.log_info(format!("Queued import: {}", path.display()));
        if let Some(active) = &self.import_active {
            let queued = self.import_queue.len();
            self.status = format!("Importing {} (queued {})", display_path(active), queued);
        } else {
            self.status = format!("Queued import: {}", display_path(&path));
        }
        self.start_next_import();

        Ok(())
    }

    fn submit_game_root_path(&mut self, path: PathBuf) -> Result<()> {
        if !path.exists() {
            self.status = format!("Path not found: {}", path.display());
            self.log_warn(format!("Game root not found: {}", path.display()));
            self.open_path_browser(SetupStep::GameRoot);
            return Ok(());
        }

        if !game::looks_like_game_root(self.game_id, &path) {
            self.status = "Invalid game root: expected Data/ and bin/".to_string();
            self.log_warn(format!("Invalid game root: {}", path.display()));
            self.open_path_browser(SetupStep::GameRoot);
            return Ok(());
        }

        self.config.game_root = path.clone();
        match game::detect_paths(self.game_id, Some(&path), None) {
            Ok(paths) => {
                self.config.larian_dir = paths.larian_dir;
                self.config.save()?;
                self.status = "Game paths set".to_string();
                self.log_info(format!("Game root set: {}", path.display()));
                self.set_toast("Paths updated", ToastLevel::Info, Duration::from_secs(2));
            }
            Err(err) => {
                self.status =
                    "Game root set. Larian data dir not found; please select it.".to_string();
                self.log_warn(format!("Larian dir auto-detect failed: {err}"));
                self.start_setup(SetupStep::LarianDir);
            }
        }

        Ok(())
    }

    fn submit_larian_dir_path(&mut self, path: PathBuf) -> Result<()> {
        if !path.exists() {
            self.status = format!("Path not found: {}", path.display());
            self.log_warn(format!("Larian dir not found: {}", path.display()));
            self.open_path_browser(SetupStep::LarianDir);
            return Ok(());
        }

        if !game::looks_like_user_dir(self.game_id, &path) {
            self.status = "Invalid Larian dir: expected PlayerProfiles/".to_string();
            self.log_warn(format!("Invalid Larian dir: {}", path.display()));
            self.open_path_browser(SetupStep::LarianDir);
            return Ok(());
        }

        if !game::looks_like_game_root(self.game_id, &self.config.game_root) {
            self.status = "Game root missing: select BG3 install root".to_string();
            self.log_warn("Game root missing while setting Larian dir".to_string());
            self.start_setup(SetupStep::GameRoot);
            return Ok(());
        }

        self.config.larian_dir = path.clone();
        self.config.save()?;
        self.status = "Game paths set".to_string();
        self.log_info(format!("Larian dir set: {}", path.display()));
        self.set_toast("Paths updated", ToastLevel::Info, Duration::from_secs(2));
        Ok(())
    }

    fn submit_downloads_dir_path(&mut self, path: PathBuf) -> Result<()> {
        if !path.exists() || !path.is_dir() {
            self.status = format!("Path not found: {}", path.display());
            self.log_warn(format!("Downloads dir not found: {}", path.display()));
            self.open_path_browser(SetupStep::DownloadsDir);
            return Ok(());
        }

        self.app_config.downloads_dir = path.clone();
        self.app_config.save()?;
        self.status = "Downloads folder set".to_string();
        self.log_info(format!("Downloads dir set: {}", path.display()));
        self.set_toast(
            "Downloads folder updated",
            ToastLevel::Info,
            Duration::from_secs(2),
        );
        Ok(())
    }

    pub fn import_mods_cli(&mut self, paths: Vec<String>, options: CliImportOptions) -> Result<()> {
        let mut total_imported = 0usize;
        let mut failures: Vec<importer::ImportFailure> = Vec::new();

        for raw_path in paths {
            let mut apply_all: Option<bool> = None;
            let path = expand_tilde(raw_path.trim());
            if !path.exists() {
                let label = path.display().to_string();
                if options.verbosity != CliVerbosity::Quiet {
                    eprintln!("Import path not found: {label}");
                }
                failures.push(importer::ImportFailure {
                    source: importer::ImportSource { label },
                    error: "path not found".to_string(),
                });
                continue;
            }

            if options.verbosity != CliVerbosity::Quiet {
                println!("Importing: {}", path.display());
            }

            let printer = if matches!(
                options.verbosity,
                CliVerbosity::Verbose | CliVerbosity::Debug
            ) {
                Some(Arc::new(Mutex::new(CliProgressPrinter::new(
                    options.verbosity,
                ))))
            } else {
                None
            };
            let progress: Option<importer::ProgressCallback> = printer.as_ref().map(|printer| {
                let printer = Arc::clone(printer);
                let callback: importer::ProgressCallback =
                    Arc::new(move |progress: importer::ImportProgress| {
                        if let Ok(mut printer) = printer.lock() {
                            printer.handle(&progress);
                        }
                    });
                callback
            });

            let start = Instant::now();
            let imports =
                match importer::import_path_with_progress(&path, &self.config.data_dir, progress)
                    .with_context(|| format!("import {path:?}"))
                {
                    Ok(imports) => imports,
                    Err(err) => {
                        let label = path.display().to_string();
                        if options.verbosity != CliVerbosity::Quiet {
                            eprintln!(
                                "Import failed: {label} ({})",
                                summarize_error(&err.to_string())
                            );
                        }
                        failures.push(importer::ImportFailure {
                            source: importer::ImportSource { label },
                            error: err.to_string(),
                        });
                        continue;
                    }
                };

            if imports.unrecognized && imports.batches.is_empty() {
                let label = path.display().to_string();
                if options.verbosity != CliVerbosity::Quiet {
                    eprintln!("Unrecognized mod layout for {label} (skipped)");
                }
                failures.push(importer::ImportFailure {
                    source: importer::ImportSource { label },
                    error: "unrecognized layout".to_string(),
                });
                continue;
            }

            for failure in &imports.failures {
                failures.push(failure.clone());
                if matches!(
                    options.verbosity,
                    CliVerbosity::Verbose | CliVerbosity::Debug
                ) {
                    eprintln!(
                        "Import failed: {} ({})",
                        failure.source.label,
                        summarize_error(&failure.error)
                    );
                }
            }

            let mut path_imported = 0usize;
            for batch in imports.batches {
                let source_label = batch.source.label.clone();
                if matches!(
                    options.verbosity,
                    CliVerbosity::Verbose | CliVerbosity::Debug
                ) {
                    println!("  Source: {}", source_label);
                }
                let mut approved = Vec::new();
                for mod_entry in batch.mods {
                    if let Some(existing) = self.find_duplicate_by_name(&mod_entry.name).cloned() {
                        let default_overwrite = duplicate_default_overwrite(&mod_entry, &existing);
                        let overwrite = if let Some(choice) = apply_all {
                            choice
                        } else {
                            let resolution = prompt_duplicate_cli(
                                &mod_entry,
                                &existing,
                                default_overwrite,
                                None,
                            )?;
                            match resolution {
                                CliDuplicateAction::Overwrite => true,
                                CliDuplicateAction::Skip => false,
                                CliDuplicateAction::OverwriteAll => {
                                    apply_all = Some(true);
                                    true
                                }
                                CliDuplicateAction::SkipAll => {
                                    apply_all = Some(false);
                                    false
                                }
                            }
                        };
                        if overwrite {
                            if existing.id != mod_entry.id {
                                let _ = self.remove_mod_by_id(&existing.id);
                            }
                            approved.push(mod_entry);
                        } else {
                            self.remove_mod_root(&mod_entry.id);
                        }
                        continue;
                    }

                    if let Some(similar) = self.find_similar_by_label(&mod_entry) {
                        let default_overwrite = similar
                            .new_stamp
                            .zip(similar.existing_stamp)
                            .map(|(new_stamp, existing_stamp)| new_stamp > existing_stamp);
                        let existing = match self
                            .library
                            .mods
                            .iter()
                            .find(|entry| entry.id == similar.existing_id)
                            .cloned()
                        {
                            Some(existing) => existing,
                            None => {
                                approved.push(mod_entry);
                                continue;
                            }
                        };
                        let overwrite = if let Some(choice) = apply_all {
                            choice
                        } else {
                            let resolution = prompt_duplicate_cli(
                                &mod_entry,
                                &existing,
                                default_overwrite,
                                Some(similar.similarity),
                            )?;
                            match resolution {
                                CliDuplicateAction::Overwrite => true,
                                CliDuplicateAction::Skip => false,
                                CliDuplicateAction::OverwriteAll => {
                                    apply_all = Some(true);
                                    true
                                }
                                CliDuplicateAction::SkipAll => {
                                    apply_all = Some(false);
                                    false
                                }
                            }
                        };
                        if overwrite {
                            if similar.existing_id != mod_entry.id {
                                let _ = self.remove_mod_by_id(&similar.existing_id);
                            }
                            approved.push(mod_entry);
                        } else {
                            self.remove_mod_root(&mod_entry.id);
                        }
                        continue;
                    }

                    approved.push(mod_entry);
                }

                if !approved.is_empty() {
                    match self.apply_mod_entries(approved) {
                        Ok(count) => {
                            path_imported = path_imported.saturating_add(count);
                        }
                        Err(err) => {
                            failures.push(importer::ImportFailure {
                                source: batch.source.clone(),
                                error: err.to_string(),
                            });
                            if options.verbosity != CliVerbosity::Quiet {
                                eprintln!(
                                    "Import apply failed: {} ({})",
                                    batch.source.label,
                                    summarize_error(&err.to_string())
                                );
                            }
                        }
                    }
                }
            }

            total_imported = total_imported.saturating_add(path_imported);
            if options.verbosity != CliVerbosity::Quiet {
                let elapsed = start.elapsed().as_millis();
                println!(
                    "Imported {} mod(s) from {} in {}ms",
                    path_imported,
                    path.display(),
                    elapsed
                );
            }
        }

        if options.verbosity != CliVerbosity::Quiet {
            if failures.is_empty() {
                println!("Import complete: {} mod(s) imported", total_imported);
            } else {
                println!(
                    "Import complete: {} mod(s) imported, {} failure(s)",
                    total_imported,
                    failures.len()
                );
                for failure in failures.iter().take(8) {
                    println!(
                        "  - {}: {}",
                        failure.source.label,
                        summarize_error(&failure.error)
                    );
                }
                if failures.len() > 8 {
                    println!("  ...and {} more (see log)", failures.len() - 8);
                }
            }
        }

        if options.deploy {
            if !self.paths_ready() {
                if options.verbosity != CliVerbosity::Quiet {
                    eprintln!("Deploy skipped: game paths not set");
                }
                return Ok(());
            }
            if total_imported == 0 {
                if options.verbosity != CliVerbosity::Quiet {
                    println!("No imports to deploy");
                }
                return Ok(());
            }

            if options.verbosity != CliVerbosity::Quiet {
                println!("Deploying imported mods...");
            }
            let mut library = self.library.clone();
            match deploy::deploy_with_options(
                &self.config,
                &mut library,
                deploy::DeployOptions {
                    backup: true,
                    reason: Some("cli import".to_string()),
                },
            ) {
                Ok(report) => {
                    if options.verbosity != CliVerbosity::Quiet {
                        println!(
                            "Deploy complete: {} pak, {} loose ({} files)",
                            report.pak_count, report.loose_count, report.file_count
                        );
                    }
                    self.library = library;
                }
                Err(err) => {
                    if options.verbosity != CliVerbosity::Quiet {
                        eprintln!("Deploy failed: {}", summarize_error(&err.to_string()));
                    }
                    return Err(err);
                }
            }
        }

        Ok(())
    }

    pub fn poll_imports(&mut self) {
        self.poll_native_sync();
        loop {
            match self.import_rx.try_recv() {
                Ok(message) => self.handle_import_message(message),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        // Manual dependency handling: no background download watching.

        if self.import_active.is_none() {
            self.process_next_import_batch();
            self.start_next_import();
            self.resume_pending_import_batch();
        }

        self.poll_deploys();
        self.maybe_start_deploy();
        self.poll_conflicts();
        self.maybe_start_conflict_scan();

        if self.dependency_queue.is_none()
            && self.import_active.is_none()
            && self.import_queue.is_empty()
            && self.import_batches.is_empty()
            && self.pending_duplicate.is_none()
            && self.dialog.is_none()
        {
            self.apply_pending_dependency_enable();
        }
    }

    pub fn poll_smart_rank(&mut self) {
        loop {
            match self.smart_rank_rx.try_recv() {
                Ok(message) => match message {
                    SmartRankMessage::Progress { scan_id, progress } => {
                        if !self.smart_rank_scan_matches(scan_id) {
                            continue;
                        }
                        let current_profile_key = self.smart_rank_profile_key();
                        if !self.smart_rank_scan_profile_matches(current_profile_key.as_str()) {
                            self.log_warn("Smart rank scan ignored (profile changed)".to_string());
                            self.clear_smart_rank_scan_state();
                            continue;
                        }
                        if let Some(cache) = progress.cache.clone() {
                            let profile_key = self
                                .smart_rank_cache
                                .as_ref()
                                .map(|cache| cache.profile_key.clone())
                                .unwrap_or_else(|| current_profile_key.clone());
                            let mut mod_cache = if let Some(existing) = &self.smart_rank_cache {
                                existing.mod_cache.clone()
                            } else {
                                smart_rank::SmartRankCacheData::default()
                            };
                            mod_cache.mods.insert(progress.mod_id.clone(), cache);
                            self.smart_rank_cache = Some(SmartRankCache {
                                version: SMART_RANK_CACHE_VERSION,
                                profile_key,
                                mod_cache,
                                result: None,
                            });
                            self.maybe_save_smart_rank_cache(false);
                        }
                        if self.smart_rank_interrupt {
                            continue;
                        }
                        self.smart_rank_progress = Some(progress.clone());
                        let label = progress.group.label();
                        if progress.total > 0 {
                            self.status = format!(
                                "Smart ranking: {label} {}/{} ({})",
                                progress.scanned, progress.total, progress.name
                            );
                        } else {
                            self.status = format!("Smart ranking: {label} ({})", progress.name);
                        }
                    }
                    SmartRankMessage::Finished { scan_id, computed } => {
                        if !self.smart_rank_scan_matches(scan_id) {
                            continue;
                        }
                        let current_profile_key = self.smart_rank_profile_key();
                        if !self.smart_rank_scan_profile_matches(current_profile_key.as_str()) {
                            self.log_warn("Smart rank scan ignored (profile changed)".to_string());
                            self.clear_smart_rank_scan_state();
                            continue;
                        }
                        if self.smart_rank_interrupt {
                            self.clear_smart_rank_scan_state();
                            self.log_info("Smart rank result ignored (stale)".to_string());
                            continue;
                        }
                        let profile_key = self
                            .smart_rank_scan_profile_key
                            .clone()
                            .unwrap_or(current_profile_key);
                        let result = computed.result.clone();
                        self.smart_rank_cache = Some(SmartRankCache {
                            version: SMART_RANK_CACHE_VERSION,
                            profile_key,
                            mod_cache: computed.cache.clone(),
                            result: Some(result.clone()),
                        });
                        self.maybe_save_smart_rank_cache(true);
                        match self.smart_rank_mode.unwrap_or(SmartRankMode::Preview) {
                            SmartRankMode::Preview => {
                                self.finalize_smart_rank_preview(result);
                            }
                            SmartRankMode::Warmup => {
                                self.clear_smart_rank_scan_state();
                                self.log_info(format!(
                                    "Smart rank warmup: scanned {} mod(s), loose {}/{} pak {}/{} in {}ms",
                                    computed.scanned_mods,
                                    result.report.scanned_loose,
                                    result.report.enabled_loose,
                                    result.report.scanned_pak,
                                    result.report.enabled_pak,
                                    result.report.elapsed_ms,
                                ));
                                self.status = "Smart ranking warmup complete".to_string();
                            }
                        }
                    }
                    SmartRankMessage::Failed { scan_id, error } => {
                        if !self.smart_rank_scan_matches(scan_id) {
                            continue;
                        }
                        if self.smart_rank_interrupt {
                            self.clear_smart_rank_scan_state();
                            self.log_warn("Smart rank failed after interrupt".to_string());
                            continue;
                        }
                        if let Some(cache) = &mut self.smart_rank_cache {
                            cache.result = None;
                        }
                        self.clear_smart_rank_scan_state();
                        self.status = format!("Smart ranking failed: {error}");
                        self.log_error(format!("Smart ranking failed: {error}"));
                    }
                },
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.clear_smart_rank_scan_state();
                    break;
                }
            }
        }
        self.maybe_restart_smart_rank();
    }

    fn start_metadata_refresh(&mut self) {
        if self.metadata_active {
            return;
        }
        self.metadata_active = true;
        self.metadata_processed = 0;
        self.metadata_total = self.library.mods.len();
        self.metadata_processed_ids.clear();
        self.metadata_dirty = false;
        self.dependency_cache_ready = false;
        let tx = self.metadata_tx.clone();
        let config = self.config.clone();
        let library = self.library.clone();
        let game_id = self.game_id;
        thread::spawn(move || {
            let result = collect_metadata_updates(game_id, &config, &library, Some(&tx));
            let message = match result {
                Ok(_) => MetadataMessage::Completed,
                Err(err) => MetadataMessage::Failed {
                    error: err.to_string(),
                },
            };
            let _ = tx.send(message);
        });
    }

    fn smart_rank_profile_key(&self) -> String {
        Self::smart_rank_profile_key_for(&self.library)
    }

    fn smart_rank_profile_key_for(library: &Library) -> String {
        let mut hasher = Hasher::new();
        if let Some(profile) = library.active_profile() {
            hasher.update(profile.name.as_bytes());
            for entry in &profile.order {
                hasher.update(entry.id.as_bytes());
                hasher.update(&[entry.enabled as u8]);
            }
        }
        hasher.finalize().to_hex().to_string()
    }

    fn smart_rank_cache_path(&self) -> PathBuf {
        self.config.data_dir.join("smart_rank_cache.json")
    }

    fn load_smart_rank_cache(&mut self) {
        let path = self.smart_rank_cache_path();
        if !path.exists() {
            self.log_info("Smart rank cache not found".to_string());
            return;
        }
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(_) => return,
        };
        match serde_json::from_str::<SmartRankCache>(&raw) {
            Ok(cache) => {
                if cache.version != SMART_RANK_CACHE_VERSION {
                    self.log_warn(format!(
                        "Smart rank cache version mismatch: {}",
                        cache.version
                    ));
                    return;
                }
                if cache.result.is_none() {
                    if cache.mod_cache.mods.is_empty() {
                        self.log_warn("Smart rank cache empty".to_string());
                        return;
                    }
                    self.log_warn(
                        "Smart rank cache missing result; using cached mod data".to_string(),
                    );
                }
                self.smart_rank_cache = Some(cache);
                self.log_info("Smart rank cache loaded".to_string());
            }
            Err(err) => {
                self.log_warn(format!("Smart rank cache load failed: {err}"));
            }
        }
    }

    fn save_smart_rank_cache(&mut self) {
        let Some(cache) = &self.smart_rank_cache else {
            return;
        };
        if cache.mod_cache.mods.is_empty() {
            return;
        }
        let raw = match serde_json::to_string_pretty(cache) {
            Ok(raw) => raw,
            Err(err) => {
                self.log_warn(format!("Smart rank cache serialize failed: {err}"));
                return;
            }
        };
        let path = self.smart_rank_cache_path();
        if let Err(err) = fs::write(&path, raw) {
            self.log_warn(format!("Smart rank cache write failed: {err}"));
        }
    }

    fn maybe_save_smart_rank_cache(&mut self, force: bool) {
        if !force {
            if let Some(last_saved) = self.smart_rank_cache_last_saved {
                if last_saved.elapsed() < Duration::from_millis(SMART_RANK_CACHE_SAVE_DEBOUNCE_MS) {
                    return;
                }
            }
        }
        self.save_smart_rank_cache();
        self.smart_rank_cache_last_saved = Some(Instant::now());
    }

    fn clear_smart_rank_cache_file(&mut self) {
        let path = self.smart_rank_cache_path();
        if let Err(err) = fs::remove_file(&path) {
            if err.kind() != std::io::ErrorKind::NotFound {
                self.log_warn(format!("Smart rank cache delete failed: {err}"));
            }
        }
    }

    fn start_native_sync(&mut self) {
        if self.native_sync_active {
            return;
        }
        self.native_sync_active = true;
        self.native_sync_progress = None;
        self.status = "Syncing native mods...".to_string();
        let tx = self.native_sync_tx.clone();
        let config = self.config.clone();
        let library = self.library.clone();
        let game_id = self.game_id;
        thread::spawn(move || {
            match sync_native_mods_delta(game_id, &config, &library, Some(&tx)) {
                Ok(delta) => {
                    let _ = tx.send(NativeSyncMessage::Completed(delta));
                }
                Err(reason) => {
                    let _ = tx.send(NativeSyncMessage::Skipped(reason));
                }
            }
        });
    }

    fn run_native_sync_inline(&mut self) {
        match sync_native_mods_delta(self.game_id, &self.config, &self.library, None) {
            Ok(delta) => {
                self.apply_native_sync_delta(delta);
            }
            Err(reason) => {
                self.log_warn(format!("Native mod sync skipped: {reason}"));
            }
        }
    }

    fn poll_native_sync(&mut self) {
        loop {
            match self.native_sync_rx.try_recv() {
                Ok(message) => match message {
                    NativeSyncMessage::Progress(progress) => {
                        let label = progress.stage.label();
                        self.native_sync_progress = Some(progress.clone());
                        if progress.total > 0 {
                            self.status =
                                format!("{label}: {}/{}", progress.current, progress.total);
                        } else {
                            self.status = format!("{label}: working...");
                        }
                    }
                    NativeSyncMessage::Completed(delta) => {
                        self.native_sync_active = false;
                        self.native_sync_progress = None;
                        self.apply_native_sync_delta(delta);
                        if self.startup_post_sync_pending {
                            self.startup_post_sync_pending = false;
                            self.run_post_sync_tasks();
                        }
                    }
                    NativeSyncMessage::Skipped(reason) => {
                        self.native_sync_active = false;
                        self.native_sync_progress = None;
                        self.status = format!("Native mod sync skipped: {reason}");
                        self.log_warn(format!("Native mod sync skipped: {reason}"));
                        if self.startup_post_sync_pending {
                            self.startup_post_sync_pending = false;
                            self.run_post_sync_tasks();
                        }
                    }
                },
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.native_sync_active = false;
                    self.native_sync_progress = None;
                    break;
                }
            }
        }
    }

    fn start_update_check(&mut self) {
        if self.update_active {
            return;
        }
        self.update_status = UpdateStatus::Checking;
        self.update_active = true;
        self.update_started_at = Some(Instant::now());
        let tx = self.update_tx.clone();
        let current_version = env!("CARGO_PKG_VERSION").to_string();
        thread::spawn(move || {
            let message = match update::check_for_updates(&current_version) {
                Ok(result) => UpdateMessage::Completed(result),
                Err(err) => UpdateMessage::Failed {
                    error: err.to_string(),
                },
            };
            let _ = tx.send(message);
        });
    }

    pub fn request_update_check(&mut self) {
        if let UpdateStatus::Available {
            path, instructions, ..
        } = &self.update_status
        {
            let path = path.clone();
            let instructions = instructions.clone();
            self.log_info(format!("Update package ready: {}", path.display()));
            self.log_info(instructions);
            self.set_toast(
                "Update ready: see log",
                ToastLevel::Info,
                Duration::from_secs(3),
            );
        }
        self.start_update_check();
        if self.update_active {
            self.status = "Checking for updates...".to_string();
        }
    }

    pub fn apply_ready_update(&mut self) {
        let UpdateStatus::Available { info, path, .. } = self.update_status.clone() else {
            self.request_update_check();
            return;
        };

        self.status = "Applying update...".to_string();
        self.log_info(format!("Applying update v{}", info.version));
        match update::apply_downloaded_update(&info, &path) {
            Ok(update::ApplyOutcome::Applied) => {
                self.update_status = UpdateStatus::Applied { info: info.clone() };
                self.status = format!("Update applied: v{} (restarting)", info.version);
                self.set_toast(
                    &format!("Updated to v{} (restarting)", info.version),
                    ToastLevel::Info,
                    Duration::from_secs(3),
                );
                self.restart_after_update();
            }
            Ok(update::ApplyOutcome::Manual { instructions }) => {
                self.log_info(instructions.clone());
                self.set_toast(
                    "Update ready: see log",
                    ToastLevel::Info,
                    Duration::from_secs(3),
                );
            }
            Err(err) => {
                self.update_status = UpdateStatus::Failed {
                    error: err.to_string(),
                };
                self.status = format!("Update apply failed: {err}");
                self.log_error(format!("Update apply failed: {err}"));
            }
        }
    }

    fn restart_after_update(&mut self) {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let exec = std::env::var("APPIMAGE")
            .ok()
            .map(PathBuf::from)
            .or_else(|| std::env::current_exe().ok());
        let Some(exec) = exec else {
            self.log_warn("Restart failed: no executable path".to_string());
            return;
        };

        match std::process::Command::new(&exec).args(&args).spawn() {
            Ok(_) => {
                self.log_info("Restarting after update".to_string());
                self.should_quit = true;
            }
            Err(err) => {
                self.log_warn(format!("Restart failed: {err}"));
            }
        }
    }

    pub fn poll_metadata_refresh(&mut self) {
        loop {
            match self.metadata_rx.try_recv() {
                Ok(message) => match message {
                    MetadataMessage::Progress {
                        update,
                        current,
                        total,
                    } => {
                        self.metadata_processed = current;
                        self.metadata_total = total;
                        self.metadata_processed_ids.insert(update.id.clone());
                        let dependencies = update.dependencies;
                        self.dependency_cache
                            .insert(update.id.clone(), dependencies.clone());
                        if let Some(mod_entry) = self
                            .library
                            .mods
                            .iter_mut()
                            .find(|entry| entry.id == update.id)
                        {
                            if mod_entry.created_at != update.created_at {
                                mod_entry.created_at = update.created_at;
                                self.metadata_dirty = true;
                            }
                            if mod_entry.modified_at != update.modified_at {
                                mod_entry.modified_at = update.modified_at;
                                self.metadata_dirty = true;
                            }
                            if mod_entry.dependencies != dependencies {
                                mod_entry.dependencies = dependencies;
                                self.metadata_dirty = true;
                            }
                        }
                    }
                    MetadataMessage::Completed => {
                        self.metadata_active = false;
                        self.dependency_cache_ready =
                            self.metadata_total == 0 || !self.dependency_cache.is_empty();
                        if self.dependency_cache_ready {
                            self.refresh_dependency_blocks();
                        }
                        let cache_key = self.metadata_cache_key();
                        if self.library.metadata_cache_key.as_deref() != Some(&cache_key)
                            || self.library.metadata_cache_version != METADATA_CACHE_VERSION
                        {
                            self.library.metadata_cache_key = Some(cache_key);
                            self.library.metadata_cache_version = METADATA_CACHE_VERSION;
                            self.metadata_dirty = true;
                        }
                        if self.metadata_dirty {
                            let _ = self.library.save(&self.config.data_dir);
                            self.log_info("Metadata refresh applied".to_string());
                            self.metadata_dirty = false;
                        }
                        self.run_startup_dependency_check();
                        self.schedule_smart_rank_warmup();
                        self.maybe_restart_smart_rank();
                        self.maybe_prompt_pending_delete();
                    }
                    MetadataMessage::Failed { error } => {
                        self.metadata_active = false;
                        self.log_warn(format!("Metadata refresh failed: {error}"));
                        self.schedule_smart_rank_warmup();
                        self.maybe_restart_smart_rank();
                    }
                },
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.metadata_active = false;
                    break;
                }
            }
        }
    }

    pub fn poll_updates(&mut self) {
        loop {
            match self.update_rx.try_recv() {
                Ok(message) => {
                    self.update_active = false;
                    self.update_started_at = None;
                    match message {
                        UpdateMessage::Completed(result) => match result {
                            update::UpdateResult::UpToDate => {
                                self.update_status = UpdateStatus::UpToDate {
                                    version: env!("CARGO_PKG_VERSION").to_string(),
                                };
                                self.log_info("Update check: up to date".to_string());
                            }
                            update::UpdateResult::Applied(info) => {
                                self.update_status = UpdateStatus::Applied { info: info.clone() };
                                self.status = format!("Update applied: v{}", info.version);
                                self.log_info(format!(
                                    "Update applied: v{} ({:?}, {})",
                                    info.version, info.kind, info.asset_name
                                ));
                                self.set_toast(
                                    &format!("Updated to v{} (restart to use)", info.version),
                                    ToastLevel::Info,
                                    Duration::from_secs(4),
                                );
                            }
                            update::UpdateResult::Ready {
                                info,
                                path,
                                instructions,
                            } => {
                                self.update_status = UpdateStatus::Available {
                                    info: info.clone(),
                                    path: path.clone(),
                                    instructions: instructions.clone(),
                                };
                                self.status = format!("Update ready: v{}", info.version);
                                self.log_info(format!(
                                    "Update ready: v{} ({:?}, {})",
                                    info.version,
                                    info.kind,
                                    path.display()
                                ));
                                self.log_info(instructions.clone());
                                self.set_toast(
                                    &format!("Update ready: v{} (see log)", info.version),
                                    ToastLevel::Info,
                                    Duration::from_secs(4),
                                );
                            }
                            update::UpdateResult::Skipped { version, reason } => {
                                self.update_status = UpdateStatus::Skipped {
                                    version: version.clone(),
                                    reason: reason.clone(),
                                };
                                self.log_warn(format!(
                                    "Update available (v{version}) skipped: {reason}"
                                ));
                            }
                        },
                        UpdateMessage::Failed { error } => {
                            self.update_active = false;
                            self.update_status = UpdateStatus::Failed {
                                error: error.clone(),
                            };
                            self.log_warn(format!("Update check failed: {error}"));
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.update_active = false;
                    self.update_started_at = None;
                    break;
                }
            }
        }
    }

    pub fn maybe_auto_submit(&mut self) -> Option<(InputPurpose, String)> {
        if self.dialog.is_some() {
            return None;
        }
        let (auto_submit, last_edit_at, value, purpose) = match &self.input_mode {
            InputMode::Editing {
                buffer,
                purpose,
                auto_submit,
                last_edit_at,
                ..
            } => (
                *auto_submit,
                *last_edit_at,
                buffer.trim().to_string(),
                purpose.clone(),
            ),
            _ => return None,
        };

        if !auto_submit {
            return None;
        }

        if value.is_empty() && !matches!(purpose, InputPurpose::FilterMods) {
            return None;
        }

        if last_edit_at.elapsed() < Duration::from_millis(350) {
            return None;
        }

        self.input_mode = InputMode::Normal;
        Some((purpose, value))
    }

    pub fn scroll_log_up(&mut self, lines: usize) {
        self.log_scroll = self.log_scroll.saturating_add(lines);
    }

    pub fn scroll_log_down(&mut self, lines: usize) {
        self.log_scroll = self.log_scroll.saturating_sub(lines);
    }

    pub fn page_mods_up(&mut self) {
        if self.move_mode {
            return;
        }
        let page = self.mods_view_height.saturating_sub(1).max(1);
        self.selected = self.selected.saturating_sub(page);
    }

    pub fn page_mods_down(&mut self) {
        if self.move_mode {
            return;
        }
        let page = self.mods_view_height.saturating_sub(1).max(1);
        self.selected = self.selected.saturating_add(page);
    }

    pub fn jump_mod_selection(&mut self, delta: isize) {
        if self.move_mode {
            return;
        }
        if delta.is_negative() {
            self.selected = self.selected.saturating_sub(delta.wrapping_abs() as usize);
        } else {
            self.selected = self.selected.saturating_add(delta as usize);
        }
        self.clamp_selection();
    }

    pub fn log_info(&mut self, message: String) {
        self.push_log(LogLevel::Info, message);
    }

    pub fn log_warn(&mut self, message: String) {
        self.push_log(LogLevel::Warn, message);
    }

    pub fn log_error(&mut self, message: String) {
        self.push_log(LogLevel::Error, message);
    }

    fn push_log(&mut self, level: LogLevel, message: String) {
        if self.log_scroll > 0 {
            self.log_scroll = self.log_scroll.saturating_add(1);
        }

        self.logs.push(LogEntry {
            level,
            message: message.clone(),
        });

        if self.logs.len() > LOG_CAPACITY {
            let overflow = self.logs.len() - LOG_CAPACITY;
            self.logs.drain(0..overflow);
            self.log_scroll = self.log_scroll.saturating_sub(overflow);
        }

        let _ = append_log_file(&self.log_path, level, &message);
    }

    fn start_next_import(&mut self) {
        if self.import_active.is_some() {
            return;
        }
        if self.dependency_queue.is_some() {
            return;
        }
        if self.dialog.is_some()
            || self.pending_duplicate.is_some()
            || !self.duplicate_queue.is_empty()
            || !self.import_batches.is_empty()
        {
            return;
        }

        let Some(path) = self.import_queue.pop_front() else {
            return;
        };

        self.import_active = Some(path.clone());
        self.import_progress = None;
        self.status = format!("Importing {}", display_path(&path));
        self.log_info(format!("Import started: {}", path.display()));

        let tx = self.import_tx.clone();
        let progress_tx = tx.clone();
        let data_dir = self.config.data_dir.clone();
        thread::spawn(move || {
            let progress = Arc::new(move |progress: importer::ImportProgress| {
                let _ = progress_tx.send(ImportMessage::Progress(progress));
            });
            let result = importer::import_path_with_progress(&path, &data_dir, Some(progress))
                .with_context(|| format!("import {path:?}"));
            let message = match result {
                Ok(result) => ImportMessage::Completed { path, result },
                Err(err) => ImportMessage::Failed {
                    path,
                    error: err.to_string(),
                },
            };
            let _ = tx.send(message);
        });
    }

    fn handle_import_message(&mut self, message: ImportMessage) {
        match message {
            ImportMessage::Progress(progress) => {
                self.import_progress = Some(progress);
            }
            ImportMessage::Completed { path, result } => {
                self.import_active = None;
                self.import_progress = None;
                if !result.failures.is_empty() {
                    for failure in &result.failures {
                        self.log_error(format!(
                            "Import failed: {} ({})",
                            failure.source.label, failure.error
                        ));
                    }
                    self.import_failures.extend(result.failures);
                    self.import_summary_pending = true;
                }
                if result.batches.is_empty() {
                    if result.unrecognized {
                        self.prompt_unrecognized(path);
                        return;
                    }
                    self.status = "No mods found to import".to_string();
                    self.log_warn(format!("No mods detected in {}", path.display()));
                    self.maybe_show_import_summary();
                    return;
                }

                self.import_batches.extend(result.batches);
                self.process_next_import_batch();
            }
            ImportMessage::Failed { path, error } => {
                self.import_active = None;
                self.import_progress = None;
                let display = display_path(&path);
                let reason = summarize_error(&error);
                self.status = format!("Import failed: {display} ({reason})");
                self.log_error(format!("Import failed for {}: {error}", path.display()));
                self.set_toast(
                    &format!("Import failed: {display} ({reason})"),
                    ToastLevel::Error,
                    Duration::from_secs(4),
                );
                self.import_failures.push(importer::ImportFailure {
                    source: importer::ImportSource { label: display },
                    error,
                });
                self.import_summary_pending = true;
                self.maybe_show_import_summary();
            }
        }
    }

    fn stage_imports(&mut self, mods: Vec<ModEntry>, source: &importer::ImportSource) {
        let mut approved = Vec::new();
        let mut duplicates = VecDeque::new();

        for mod_entry in mods {
            if let Some(existing) = self.find_duplicate_by_name(&mod_entry.name) {
                let default_overwrite = duplicate_default_overwrite(&mod_entry, existing);
                duplicates.push_back(DuplicateDecision {
                    mod_entry,
                    existing_id: existing.id.clone(),
                    existing_label: existing.display_name(),
                    kind: DuplicateKind::Exact,
                    default_overwrite,
                });
            } else if let Some(similar) = self.find_similar_by_label(&mod_entry) {
                let existing_label = similar.existing_label.clone();
                let default_overwrite = similar
                    .new_stamp
                    .zip(similar.existing_stamp)
                    .map(|(new_stamp, existing_stamp)| new_stamp > existing_stamp);
                duplicates.push_back(DuplicateDecision {
                    mod_entry,
                    existing_id: similar.existing_id,
                    existing_label: existing_label.clone(),
                    kind: DuplicateKind::Similar {
                        new_label: similar.new_label,
                        existing_label,
                        new_stamp: similar.new_stamp,
                        existing_stamp: similar.existing_stamp,
                        similarity: similar.similarity,
                    },
                    default_overwrite,
                });
            } else {
                approved.push(mod_entry);
            }
        }

        if !duplicates.is_empty() {
            self.approved_imports.extend(approved);
            self.duplicate_queue.extend(duplicates);
            self.log_warn(format!(
                "Duplicate or similar mods found in {}. Awaiting confirmation.",
                source.label
            ));
            self.prompt_next_duplicate();
            return;
        }

        match self.apply_mod_entries(approved) {
            Ok(count) => {
                self.status = format!("Imported {} mod(s)", count);
                self.log_info(format!(
                    "Import complete: {} mod(s) from {}",
                    count, source.label
                ));
                self.process_next_import_batch();
            }
            Err(err) => {
                let display = source.label.clone();
                let reason = summarize_error(&err.to_string());
                self.status = format!("Import failed: {display} ({reason})");
                self.log_error(format!("Import apply failed for {}: {err}", source.label));
                self.set_toast(
                    &format!("Import failed: {display} ({reason})"),
                    ToastLevel::Error,
                    Duration::from_secs(4),
                );
                self.import_failures.push(importer::ImportFailure {
                    source: source.clone(),
                    error: err.to_string(),
                });
                self.import_summary_pending = true;
                self.process_next_import_batch();
            }
        }
    }

    fn process_next_import_batch(&mut self) {
        if self.import_active.is_some()
            || self.dependency_queue.is_some()
            || self.dialog.is_some()
            || self.pending_duplicate.is_some()
            || !self.duplicate_queue.is_empty()
        {
            return;
        }

        let Some(batch) = self.import_batches.pop_front() else {
            self.maybe_show_import_summary();
            return;
        };
        self.stage_imports(batch.mods, &batch.source);
    }

    fn build_dependency_queue_for_mods(&self, mods: &[ModEntry]) -> Option<DependencyQueue> {
        let existing_lookup = DependencyLookup::new(&self.library.mods);
        let mut missing: HashMap<String, DependencyItem> = HashMap::new();

        for mod_entry in mods {
            let deps = self.cached_mod_dependencies(mod_entry);
            if deps.is_empty() {
                continue;
            }
            let required_by = mod_entry.display_name();
            for dep in deps {
                let resolved_ids = resolved_dependency_ids(&existing_lookup, &dep, mod_entry);
                if !resolved_ids.is_empty() {
                    continue;
                }
                if is_unverified_dependency(&dep) {
                    continue;
                }
                let display_label = dependency_display_label(&dep);
                let uuid = dependency_uuid(&dep);
                let signature = dependency_signature(&display_label, &uuid, &dep);
                let entry = missing.entry(signature).or_insert_with(|| {
                    let search_label = dependency_search_label(&display_label, &uuid, &dep);
                    let search_link = dependency_search_link(&search_label);
                    DependencyItem {
                        label: dep.clone(),
                        display_label: display_label.clone(),
                        uuid: uuid.clone(),
                        required_by: Vec::new(),
                        status: DependencyStatus::Missing,
                        link: None,
                        search_link,
                        search_label,
                        kind: DependencyItemKind::Missing,
                    }
                });
                entry.required_by.push(required_by.clone());
                if entry.display_label == "Unknown dependency"
                    && display_label != "Unknown dependency"
                {
                    entry.display_label = display_label.clone();
                    entry.search_label = dependency_search_label(&display_label, &uuid, &dep);
                    entry.search_link = dependency_search_link(&entry.search_label);
                }
                if entry.uuid.is_none() {
                    entry.uuid = uuid;
                }
            }
        }

        if missing.is_empty() {
            return None;
        }

        let mut items: Vec<DependencyItem> = missing.into_values().collect();
        for item in &mut items {
            item.required_by.sort();
            item.required_by.dedup();
        }
        items.sort_by(|a, b| a.label.cmp(&b.label));
        items.push(override_dependency_item());
        Some(DependencyQueue { items, selected: 0 })
    }

    fn collect_mod_dependencies(&self, mod_entry: &ModEntry) -> Vec<String> {
        let mod_root = library_mod_root(&self.config.data_dir).join(&mod_entry.id);
        let use_managed_root = mod_root.exists();
        let mut native_paths = None;
        let mut native_index = None;
        if mod_entry.is_native() && !use_managed_root {
            if let Ok(paths) = game::detect_paths(
                self.game_id,
                Some(&self.config.game_root),
                Some(&self.config.larian_dir),
            ) {
                native_index = Some(native_pak::build_native_pak_index(&paths.larian_mods_dir));
                native_paths = Some(paths);
            }
        }
        let mut deps = Vec::new();
        let mut json_deps = Vec::new();

        for target in &mod_entry.targets {
            if let InstallTarget::Pak { file, .. } = target {
                let mut pak_path = mod_root.join(file);
                if mod_entry.is_native() && !use_managed_root {
                    if let Some(paths) = &native_paths {
                        pak_path = paths.larian_mods_dir.join(file);
                        if !pak_path.exists() {
                            if let Some(info) = mod_entry.targets.iter().find_map(|target| {
                                if let InstallTarget::Pak { info, .. } = target {
                                    Some(info)
                                } else {
                                    None
                                }
                            }) {
                                if let Some(index) = native_index.as_deref() {
                                    if let Some(resolved) =
                                        native_pak::resolve_native_pak_path(info, index)
                                    {
                                        pak_path = resolved;
                                    }
                                }
                            }
                        }
                    }
                }
                if !pak_path.exists() && use_managed_root {
                    let index = native_pak::build_native_pak_index(&mod_root);
                    if let Some(info) = mod_entry.targets.iter().find_map(|target| {
                        if let InstallTarget::Pak { info, .. } = target {
                            Some(info)
                        } else {
                            None
                        }
                    }) {
                        if let Some(resolved) = native_pak::resolve_native_pak_path(info, &index) {
                            pak_path = resolved;
                        }
                    }
                }
                if let Some(meta) = metadata::read_meta_lsx_from_pak(&pak_path) {
                    deps.extend(meta.dependencies);
                }
            }
        }

        if deps.is_empty() {
            if let Some(meta_path) = metadata::find_meta_lsx(&mod_root) {
                if let Some(meta) = metadata::read_meta_lsx(&meta_path) {
                    deps.extend(meta.dependencies);
                }
            }
        }

        if let Some(info_path) = metadata::find_info_json(&mod_root) {
            let infos = metadata::read_json_mods(&info_path);
            for info in infos {
                if !info.dependencies.is_empty() {
                    json_deps.extend(info.dependencies);
                }
            }
        }

        deps.extend(json_deps);
        deps.sort();
        deps.dedup();
        deps.retain(|dep| !dep.eq_ignore_ascii_case(&mod_entry.id));
        filter_ignored_dependencies(&mut deps);
        deps
    }

    fn cached_mod_dependencies(&self, mod_entry: &ModEntry) -> Vec<String> {
        if let Some(deps) = self.dependency_cache.get(&mod_entry.id) {
            let mut deps = deps.clone();
            filter_ignored_dependencies(&mut deps);
            return deps;
        }
        if self.library.metadata_cache_version == METADATA_CACHE_VERSION {
            let mut deps = mod_entry.dependencies.clone();
            filter_ignored_dependencies(&mut deps);
            return deps;
        }
        self.collect_mod_dependencies(mod_entry)
    }

    pub fn dependency_lookup(&self) -> Option<DependencyLookup> {
        if !self.dependency_cache_ready {
            return None;
        }
        Some(DependencyLookup::new(&self.library.mods))
    }

    fn smart_rank_warmup_active(&self) -> bool {
        self.smart_rank_active && matches!(self.smart_rank_mode, Some(SmartRankMode::Warmup))
    }

    #[cfg(debug_assertions)]
    pub fn debug_smart_rank_warmup_block_report(&self) -> String {
        let mut lines = Vec::new();
        lines.push("Smart rank warmup edit gating".to_string());
        lines.push("Allow during warmup: toggle/enable/disable/reorder/remove".to_string());
        lines.push(format!(
            "block_mod_changes toggle: {}",
            self.block_mod_changes_warmup("toggle")
        ));
        lines.push(format!(
            "block_mod_changes enable: {}",
            self.block_mod_changes_warmup("enable")
        ));
        lines.push(format!(
            "block_mod_changes disable: {}",
            self.block_mod_changes_warmup("disable")
        ));
        lines.push(format!(
            "block_mod_changes reorder: {}",
            self.block_mod_changes_warmup("reorder")
        ));
        lines.push(format!(
            "block_mod_changes remove: {}",
            self.block_mod_changes_warmup("remove")
        ));
        let mut reorder_blockers = Vec::new();
        if self.mod_filter_active() {
            reorder_blockers.push("filter active");
        }
        if !self.mod_sort.is_order_default() {
            reorder_blockers.push("sort != Order");
        }
        if reorder_blockers.is_empty() {
            lines.push("Reorder UI gate: allowed".to_string());
        } else {
            lines.push(format!(
                "Reorder UI gate: blocked ({})",
                reorder_blockers.join(", ")
            ));
        }
        lines.join("\n")
    }

    #[cfg(debug_assertions)]
    pub fn debug_smart_rank_restart_check(&self) -> String {
        let mut lines = Vec::new();
        let path = self.smart_rank_cache_path();
        lines.push(format!("Cache path: {}", path.display()));
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(err) => {
                lines.push(format!("Cache load: failed ({err})"));
                return lines.join("\n");
            }
        };
        let cache = match serde_json::from_str::<SmartRankCache>(&raw) {
            Ok(cache) => cache,
            Err(err) => {
                lines.push(format!("Cache parse: failed ({err})"));
                return lines.join("\n");
            }
        };
        let profile_key = App::smart_rank_profile_key_for(&self.library);
        let cache_ready = App::smart_rank_cache_ready_for(&self.library, &cache);
        lines.push(format!("Cache version: {}", cache.version));
        lines.push(format!("Cache result present: {}", cache.result.is_some()));
        lines.push(format!("Cache profile key: {}", cache.profile_key));
        lines.push(format!("Current profile key: {}", profile_key));
        lines.push(format!("Cache ready: {}", cache_ready));

        let mut warmup = "Full".to_string();
        if cache.version == SMART_RANK_CACHE_VERSION {
            if cache.result.is_some() && cache.profile_key == profile_key && cache_ready {
                warmup = "None (cache hit)".to_string();
            } else if cache.profile_key == profile_key {
                warmup = "Incremental".to_string();
            } else if cache.result.is_some() && cache_ready {
                warmup = "ReorderOnly".to_string();
            } else {
                warmup = "Incremental".to_string();
            }
        }
        lines.push(format!("Restart warmup: {warmup}"));
        lines.join("\n")
    }

    pub fn mod_list_loading(&self) -> bool {
        self.metadata_active
            || self.native_sync_active
            || self.startup_dependency_check_pending
            || !self.dependency_cache_ready
    }

    pub fn status_line(&self) -> String {
        if let Some(line) = self.native_sync_status_line() {
            return line;
        }
        if let Some(line) = self.metadata_status_line() {
            return line;
        }
        if let Some(line) = self.smart_rank_status_line() {
            return line;
        }
        self.status.clone()
    }

    fn native_sync_status_line(&self) -> Option<String> {
        if !self.native_sync_active {
            return None;
        }
        if let Some(progress) = &self.native_sync_progress {
            if progress.total > 0 {
                return Some(format!(
                    "{}: {}/{}",
                    progress.stage.label(),
                    progress.current,
                    progress.total
                ));
            }
            return Some(format!("{}: working...", progress.stage.label()));
        }
        Some("Native mods prepass: working...".to_string())
    }

    fn metadata_status_line(&self) -> Option<String> {
        if !self.metadata_active {
            return None;
        }
        if self.metadata_total > 0 {
            return Some(format!(
                "Metadata scan: {}/{}",
                self.metadata_processed, self.metadata_total
            ));
        }
        Some("Metadata scan: working...".to_string())
    }

    fn smart_rank_status_line(&self) -> Option<String> {
        if !self.smart_rank_warmup_active() {
            return None;
        }
        if let Some(progress) = &self.smart_rank_progress {
            if progress.total > 0 {
                return Some(format!(
                    "Smart ranking: {} {}/{} ({})",
                    progress.group.label(),
                    progress.scanned,
                    progress.total,
                    progress.name
                ));
            }
            return Some(format!(
                "Smart ranking: {} ({})",
                progress.group.label(),
                progress.name
            ));
        }
        Some("Smart ranking: warmup...".to_string())
    }

    pub fn mod_row_loading(&self, mod_id: &str, _row_index: usize, _total_rows: usize) -> bool {
        if self.metadata_active {
            return !self.metadata_processed_ids.contains(mod_id);
        }
        if self.native_sync_active
            || self.startup_dependency_check_pending
            || !self.dependency_cache_ready
        {
            return true;
        }
        false
    }

    pub fn missing_dependency_count_for_mod(
        &self,
        mod_entry: &ModEntry,
        lookup: &DependencyLookup,
    ) -> usize {
        if !self.dependency_cache_ready {
            return 0;
        }
        let deps = self.cached_mod_dependencies(mod_entry);
        if deps.is_empty() {
            return 0;
        }
        deps.iter()
            .filter(|dep| {
                let ids = resolved_dependency_ids(lookup, dep, mod_entry);
                if ids.is_empty() {
                    return !is_unverified_dependency(dep);
                }
                false
            })
            .count()
    }

    pub fn debug_dependency_report(&self, query: &str) -> String {
        let needle = normalize_label(query);
        if needle.is_empty() {
            return "Provide a mod id or name to debug.".to_string();
        }
        let lookup = DependencyLookup::new(&self.library.mods);
        let id_to_name: HashMap<String, String> = self
            .library
            .mods
            .iter()
            .map(|entry| (entry.id.clone(), entry.display_name()))
            .collect();
        let matches: Vec<&ModEntry> = self
            .library
            .mods
            .iter()
            .filter(|entry| {
                mod_dependency_keys(entry)
                    .iter()
                    .any(|key| key.contains(&needle))
            })
            .collect();
        if matches.is_empty() {
            return format!("No mods matched \"{}\".", query);
        }
        let mut lines = Vec::new();
        for mod_entry in matches {
            lines.push(format!(
                "Mod: {} ({})",
                mod_entry.display_name(),
                mod_entry.id
            ));
            lines.push(format!("Source: {:?}", mod_entry.source));
            let managed_root = library_mod_root(&self.config.data_dir).join(&mod_entry.id);
            lines.push(format!(
                "Managed root: {} ({})",
                managed_root.display(),
                if managed_root.exists() {
                    "exists"
                } else {
                    "missing"
                }
            ));
            let mut targets = Vec::new();
            for target in &mod_entry.targets {
                match target {
                    InstallTarget::Pak { file, info } => {
                        targets.push(format!(
                            "Pak:{} (uuid {}, folder {})",
                            file, info.uuid, info.folder
                        ));
                    }
                    InstallTarget::Generated { dir } => targets.push(format!("Generated:{dir}")),
                    InstallTarget::Data { dir } => targets.push(format!("Data:{dir}")),
                    InstallTarget::Bin { dir } => targets.push(format!("Bin:{dir}")),
                }
            }
            if targets.is_empty() {
                lines.push("Targets: none".to_string());
            } else {
                lines.push(format!("Targets: {}", targets.join(", ")));
            }
            let keys = mod_dependency_keys(mod_entry);
            if !keys.is_empty() {
                lines.push(format!("Dependency keys: {}", keys.join(", ")));
            }
            let deps = self.collect_mod_dependencies(mod_entry);
            if deps.is_empty() {
                lines.push("Dependencies: none".to_string());
            } else {
                lines.push("Dependencies:".to_string());
                for dep in deps {
                    let matches = lookup.resolve_ids(&dep);
                    if matches.is_empty() {
                        lines.push(format!("  - {dep} (missing)"));
                    } else {
                        let labels: Vec<String> = matches
                            .iter()
                            .filter_map(|id| id_to_name.get(id).cloned())
                            .collect();
                        if labels.is_empty() {
                            lines.push(format!("  - {dep} (matched)"));
                        } else {
                            lines.push(format!("  - {dep} -> {}", labels.join(", ")));
                        }
                    }
                }
            }
            lines.push(String::new());
        }
        lines.join("\n")
    }

    #[cfg(debug_assertions)]
    pub fn debug_smart_rank_report(&self) -> String {
        let current = self.smart_rank_profile_key();
        let cache_path = self.smart_rank_cache_path();
        let mut lines = Vec::new();
        lines.push(format!("Smart rank cache path: {}", cache_path.display()));
        lines.push(format!(
            "Cache loaded: {}",
            if self.smart_rank_cache.is_some() {
                "yes"
            } else {
                "no"
            }
        ));
        if let Some(cache) = &self.smart_rank_cache {
            lines.push(format!("Cache version: {}", cache.version));
            lines.push(format!("Cache profile key: {}", cache.profile_key));
            lines.push(format!("Cache result present: {}", cache.result.is_some()));
            let missing = self.smart_rank_cache_missing_ids(cache);
            lines.push(format!(
                "Cache ready for enabled: {}",
                self.smart_rank_cache_ready(cache)
            ));
            if !missing.is_empty() {
                lines.push(format!("Missing mod cache entries: {}", missing.len()));
                for id in missing.iter().take(8) {
                    lines.push(format!("  - {id}"));
                }
                if missing.len() > 8 {
                    lines.push(format!("  ... {} more", missing.len() - 8));
                }
            }
        }
        lines.push(format!("Current profile key: {}", current));
        lines.push(format!(
            "Profile key match: {}",
            self.smart_rank_cache
                .as_ref()
                .map(|cache| cache.profile_key == current)
                .unwrap_or(false)
        ));
        lines.join("\n")
    }

    #[cfg(debug_assertions)]
    pub fn debug_smart_rank_warmup(&mut self) -> Result<()> {
        if !self.paths_ready() {
            return Err(anyhow::anyhow!(
                "Paths not set (configure game root + Larian dir)"
            ));
        }
        let result = smart_rank::smart_rank_profile_cached_with_progress(
            &self.config,
            &self.library,
            None,
            smart_rank::SmartRankRefreshMode::Full,
            |_| {},
        )?;
        let profile_key = self.smart_rank_profile_key();
        self.smart_rank_cache = Some(SmartRankCache {
            version: SMART_RANK_CACHE_VERSION,
            profile_key,
            mod_cache: result.cache.clone(),
            result: Some(result.result),
        });
        self.maybe_save_smart_rank_cache(true);
        Ok(())
    }

    #[cfg(debug_assertions)]
    pub fn debug_smart_rank_cache_validate(&self) -> String {
        let mut lines = Vec::new();
        let profile_key = self.smart_rank_profile_key();
        lines.push(format!("Profile key: {}", profile_key));
        let Some(cache) = &self.smart_rank_cache else {
            lines.push("Cache loaded: no".to_string());
            return lines.join("\n");
        };
        lines.push("Cache loaded: yes".to_string());
        lines.push(format!("Cache version: {}", cache.version));
        lines.push(format!("Cache profile key: {}", cache.profile_key));
        lines.push(format!("Cache result present: {}", cache.result.is_some()));
        lines.push(format!("Cache mod entries: {}", cache.mod_cache.mods.len()));
        lines.push(format!(
            "Cache ready for enabled: {}",
            self.smart_rank_cache_ready(cache)
        ));
        let missing = self.smart_rank_cache_missing_ids(cache);
        if !missing.is_empty() {
            let mod_map = self.library.index_by_id();
            lines.push(format!("Missing entries: {}", missing.len()));
            for id in missing.iter().take(8) {
                let label = mod_map
                    .get(id)
                    .map(|entry| entry.display_name())
                    .unwrap_or_else(|| id.clone());
                lines.push(format!("  - {label} ({id})"));
            }
            if missing.len() > 8 {
                lines.push(format!("  ... {} more", missing.len() - 8));
            }
        }
        lines.join("\n")
    }

    #[cfg(debug_assertions)]
    pub fn debug_smart_rank_cache_simulate(&self) -> String {
        let mut lines = Vec::new();
        lines.push("Smart rank cache simulate (dry run)".to_string());
        let Some(profile) = self.library.active_profile() else {
            lines.push("No active profile".to_string());
            return lines.join("\n");
        };
        let mut library = self.library.clone();
        let Some(profile_mut) = library.active_profile_mut() else {
            lines.push("No active profile (mutable)".to_string());
            return lines.join("\n");
        };

        let mut toggled = Vec::new();
        for entry in profile_mut.order.iter_mut().take(3) {
            entry.enabled = !entry.enabled;
            toggled.push(entry.id.clone());
        }
        let order_len = profile_mut.order.len();
        if order_len > 1 {
            profile_mut.order.swap(0, order_len - 1);
        }

        lines.push(format!("Base profile: {}", profile.name));
        lines.push(format!("Toggled mods: {}", toggled.len()));
        if !toggled.is_empty() {
            lines.push(format!("First toggled: {}", toggled[0]));
        }

        let cache_data = self
            .smart_rank_cache
            .as_ref()
            .map(|cache| cache.mod_cache.clone());
        let result = smart_rank::smart_rank_profile_cached_with_progress(
            &self.config,
            &library,
            cache_data.as_ref(),
            smart_rank::SmartRankRefreshMode::Incremental,
            |_| {},
        );

        match result {
            Ok(computed) => {
                lines.push(format!("Scanned mods: {}", computed.scanned_mods));
                lines.push(format!("Moved entries: {}", computed.result.report.moved));
                lines.push(format!("Warnings: {}", computed.result.warnings.len()));
            }
            Err(err) => {
                lines.push(format!("Simulation failed: {err}"));
            }
        }

        lines.join("\n")
    }

    #[cfg(debug_assertions)]
    pub fn debug_smart_rank_scenario(&self) -> String {
        use smart_rank::SmartRankRefreshMode;

        let mut lines = Vec::new();
        lines.push("Smart rank scenario (headless)".to_string());

        let config = self.config.clone();
        let mut library = self.library.clone();
        let mut cache = SmartRankCache {
            version: SMART_RANK_CACHE_VERSION,
            profile_key: Self::smart_rank_profile_key_for(&library),
            mod_cache: smart_rank::SmartRankCacheData::default(),
            result: None,
        };

        fn run_step(
            label: &str,
            requested: SmartRankRefreshMode,
            config: &GameConfig,
            library: &Library,
            cache: &mut SmartRankCache,
            lines: &mut Vec<String>,
        ) {
            let resolved = if matches!(requested, SmartRankRefreshMode::Full) {
                SmartRankRefreshMode::Full
            } else if cache.version != SMART_RANK_CACHE_VERSION || cache.result.is_none() {
                SmartRankRefreshMode::Full
            } else if matches!(requested, SmartRankRefreshMode::ReorderOnly)
                && !App::smart_rank_cache_ready_for(library, cache)
            {
                SmartRankRefreshMode::Incremental
            } else {
                requested
            };

            let cache_data = Some(&cache.mod_cache);
            let result = smart_rank::smart_rank_profile_cached_with_progress(
                config,
                library,
                cache_data,
                resolved,
                |_| {},
            );

            match result {
                Ok(computed) => {
                    let profile_key = App::smart_rank_profile_key_for(library);
                    *cache = SmartRankCache {
                        version: SMART_RANK_CACHE_VERSION,
                        profile_key,
                        mod_cache: computed.cache.clone(),
                        result: Some(computed.result.clone()),
                    };
                    lines.push(format!(
                        "{label}: requested={requested:?} resolved={resolved:?} scanned_mods={} full_rebuild={}",
                        computed.scanned_mods,
                        matches!(resolved, SmartRankRefreshMode::Full)
                    ));
                }
                Err(err) => {
                    lines.push(format!(
                        "{label}: requested={requested:?} resolved={resolved:?} error={err}"
                    ));
                }
            }
        }

        run_step(
            "baseline",
            SmartRankRefreshMode::Full,
            &config,
            &library,
            &mut cache,
            &mut lines,
        );

        if library
            .active_profile()
            .map(|profile| profile.order.is_empty())
            .unwrap_or(true)
        {
            lines.push("Scenario aborted: active profile has no mods".to_string());
            return lines.join("\n");
        }

        {
            let Some(profile) = library.active_profile_mut() else {
                lines.push("Scenario aborted: no active profile".to_string());
                return lines.join("\n");
            };
            if let Some(entry) = profile.order.get_mut(0) {
                entry.enabled = !entry.enabled;
            }
        }
        run_step(
            "toggle",
            SmartRankRefreshMode::Incremental,
            &config,
            &library,
            &mut cache,
            &mut lines,
        );

        let mut reordered = false;
        if let Some(profile) = library.active_profile_mut() {
            if profile.order.len() > 1 {
                profile.order.swap(0, 1);
                reordered = true;
            }
        } else {
            lines.push("reorder: skipped (no active profile)".to_string());
        }
        if reordered {
            run_step(
                "reorder",
                SmartRankRefreshMode::ReorderOnly,
                &config,
                &library,
                &mut cache,
                &mut lines,
            );
        } else {
            lines.push("reorder: skipped (need >=2 mods)".to_string());
        }

        if library
            .active_profile()
            .map(|profile| !profile.order.is_empty())
            .unwrap_or(false)
        {
            let iterations = library
                .active_profile()
                .map(|profile| 12usize.min(profile.order.len().max(1)))
                .unwrap_or(0);
            let mut stress_scans = 0usize;
            let mut stress_full = 0usize;
            for step in 0..iterations {
                if let Some(profile) = library.active_profile_mut() {
                    if let Some(entry) = profile.order.get_mut(0) {
                        entry.enabled = !entry.enabled;
                    }
                    if profile.order.len() > 1 {
                        profile.order.swap(0, 1);
                    }
                }
                let requested = if step % 2 == 0 {
                    SmartRankRefreshMode::Incremental
                } else {
                    SmartRankRefreshMode::ReorderOnly
                };
                let resolved = if matches!(requested, SmartRankRefreshMode::Full) {
                    SmartRankRefreshMode::Full
                } else if cache.version != SMART_RANK_CACHE_VERSION || cache.result.is_none() {
                    SmartRankRefreshMode::Full
                } else if matches!(requested, SmartRankRefreshMode::ReorderOnly)
                    && !App::smart_rank_cache_ready_for(&library, &cache)
                {
                    SmartRankRefreshMode::Incremental
                } else {
                    requested
                };
                let cache_data = Some(&cache.mod_cache);
                if let Ok(computed) = smart_rank::smart_rank_profile_cached_with_progress(
                    &config,
                    &library,
                    cache_data,
                    resolved,
                    |_| {},
                ) {
                    stress_scans += computed.scanned_mods;
                    if matches!(resolved, SmartRankRefreshMode::Full) {
                        stress_full += 1;
                    }
                    let profile_key = App::smart_rank_profile_key_for(&library);
                    cache = SmartRankCache {
                        version: SMART_RANK_CACHE_VERSION,
                        profile_key,
                        mod_cache: computed.cache.clone(),
                        result: Some(computed.result),
                    };
                }
            }
            lines.push(format!(
                "stress: iterations={} full_rebuilds={} scanned_mods={}",
                iterations, stress_full, stress_scans
            ));
        }

        let mut remove_entry = None;
        let mut remove_profile_entry = None;
        if let Some(profile) = library.active_profile() {
            for entry in &profile.order {
                let Some(mod_entry) = library
                    .mods
                    .iter()
                    .find(|mod_entry| mod_entry.id == entry.id)
                else {
                    continue;
                };
                let mod_root = config.data_dir.join("mods").join(&mod_entry.id);
                if mod_root.exists() {
                    remove_entry = Some(mod_entry.clone());
                    remove_profile_entry = Some(entry.clone());
                    break;
                }
            }
        }

        if let (Some(mod_entry), Some(_profile_entry)) = (remove_entry, remove_profile_entry) {
            let remove_id = mod_entry.id.clone();
            library.mods.retain(|entry| entry.id != remove_id);
            for profile in &mut library.profiles {
                profile.order.retain(|entry| entry.id != remove_id);
            }
            cache.mod_cache.mods.remove(&remove_id);
            run_step(
                "remove",
                SmartRankRefreshMode::Incremental,
                &config,
                &library,
                &mut cache,
                &mut lines,
            );

            library.mods.push(mod_entry.clone());
            if let Some(profile) = library.active_profile_mut() {
                profile.order.push(ProfileEntry {
                    id: remove_id,
                    enabled: true,
                });
            }
            run_step(
                "add",
                SmartRankRefreshMode::Incremental,
                &config,
                &library,
                &mut cache,
                &mut lines,
            );
        } else {
            lines.push("remove/add: skipped (no managed mod root found)".to_string());
        }

        lines.join("\n")
    }

    #[cfg(debug_assertions)]
    pub fn debug_smart_rank_warmup_flow(&mut self) -> String {
        use smart_rank::SmartRankRefreshMode;

        let mut lines = Vec::new();
        lines.push("Smart rank warmup flow (app edits)".to_string());

        let original_library = self.library.clone();
        let original_dependency_cache = self.dependency_cache.clone();
        let original_dependency_ready = self.dependency_cache_ready;
        let original_selected = self.selected;
        let original_suppress = self.debug_suppress_persistence;
        let original_data_dir = self.config.data_dir.clone();
        let temp_data_dir = std::env::temp_dir().join("sigilsmith-debug-warmup");
        let _ = fs::create_dir_all(&temp_data_dir);

        let mut trimmed = self.library.clone();
        let keep = 8usize.min(trimmed.mods.len());
        let keep_ids: HashSet<String> = trimmed
            .mods
            .iter()
            .take(keep)
            .map(|entry| entry.id.clone())
            .collect();
        trimmed.mods.retain(|entry| keep_ids.contains(&entry.id));
        for profile in &mut trimmed.profiles {
            profile.order.retain(|entry| keep_ids.contains(&entry.id));
        }
        if let Some(profile) = trimmed.active_profile_mut() {
            if let Some(entry) = profile.order.get_mut(0) {
                entry.enabled = true;
            }
        }

        self.library = trimmed;
        self.config.data_dir = temp_data_dir;
        self.dependency_cache.clear();
        self.dependency_cache_ready = false;
        self.prime_dependency_cache_from_library();

        let Some(profile) = self.library.active_profile() else {
            lines.push("No active profile".to_string());
            self.library = original_library;
            self.dependency_cache = original_dependency_cache;
            self.dependency_cache_ready = original_dependency_ready;
            self.selected = original_selected;
            self.config.data_dir = original_data_dir;
            return lines.join("\n");
        };
        if profile.order.len() < 2 {
            lines.push("Need at least 2 mods for warmup flow".to_string());
            self.library = original_library;
            self.dependency_cache = original_dependency_cache;
            self.dependency_cache_ready = original_dependency_ready;
            self.selected = original_selected;
            self.config.data_dir = original_data_dir;
            return lines.join("\n");
        }

        self.debug_suppress_persistence = true;
        let seed_profile_key = self.smart_rank_profile_key();
        let seed_cache = self
            .smart_rank_cache
            .as_ref()
            .map(|cache| cache.mod_cache.clone())
            .unwrap_or_default();
        self.smart_rank_cache = Some(SmartRankCache {
            version: SMART_RANK_CACHE_VERSION,
            profile_key: seed_profile_key,
            mod_cache: seed_cache,
            result: None,
        });
        self.startup_pending = false;
        self.native_sync_active = false;
        self.import_active = None;
        self.deploy_active = false;
        self.deploy_pending = false;
        self.conflict_active = false;
        self.conflict_pending = false;
        self.metadata_active = false;
        self.update_active = false;

        self.start_smart_rank_scan(SmartRankMode::Warmup, SmartRankRefreshMode::Full);
        lines.push(format!(
            "warmup active: {}",
            self.smart_rank_warmup_active()
        ));
        lines.push(format!(
            "start scan id={:?} kind={:?}",
            self.smart_rank_scan_active, self.smart_rank_refresh_kind
        ));

        self.selected = 0;
        self.toggle_selected();
        lines.push(format!(
            "after toggle pending={:?}",
            self.smart_rank_refresh_pending
        ));

        self.selected = 0;
        self.move_selected_down();
        lines.push(format!(
            "after reorder pending={:?}",
            self.smart_rank_refresh_pending
        ));

        self.selected = 0;
        self.remove_selected();
        lines.push(format!(
            "after remove pending={:?}",
            self.smart_rank_refresh_pending
        ));

        let mut refresh_events = Vec::new();
        let mut full_rebuilds = 0usize;
        let mut last_scan_id: Option<u64> = None;
        let started = Instant::now();
        let timeout = Duration::from_secs(60);
        loop {
            self.poll_smart_rank();
            if !self.smart_rank_active {
                if let Some(pending) = self.smart_rank_refresh_pending {
                    if let Some(ready_at) = self.smart_rank_refresh_at {
                        if Instant::now() >= ready_at {
                            self.smart_rank_refresh_pending = None;
                            self.smart_rank_refresh_at = None;
                            let refresh = self.resolve_smart_rank_refresh_kind(pending);
                            self.start_smart_rank_scan(SmartRankMode::Warmup, refresh);
                        }
                    } else {
                        self.smart_rank_refresh_pending = None;
                        let refresh = self.resolve_smart_rank_refresh_kind(pending);
                        self.start_smart_rank_scan(SmartRankMode::Warmup, refresh);
                    }
                }
            }
            if let Some(scan_id) = self.smart_rank_scan_active {
                if last_scan_id != Some(scan_id) {
                    if let Some(kind) = self.smart_rank_refresh_kind {
                        refresh_events.push(format!("scan kind={kind:?}"));
                        if matches!(kind, SmartRankRefreshMode::Full) {
                            full_rebuilds += 1;
                        }
                    }
                    last_scan_id = Some(scan_id);
                }
            }
            if !self.smart_rank_active && self.smart_rank_refresh_pending.is_none() {
                break;
            }
            if started.elapsed() > timeout {
                lines.push(format!(
                    "timeout waiting for warmup flow scans (active={} pending={:?} interrupt={} scan_id={:?})",
                    self.smart_rank_active,
                    self.smart_rank_refresh_pending,
                    self.smart_rank_interrupt,
                    self.smart_rank_scan_active
                ));
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }

        lines.push(format!("refresh events: {}", refresh_events.len()));
        lines.extend(refresh_events);
        lines.push(format!("full rebuilds: {full_rebuilds}"));

        self.debug_suppress_persistence = original_suppress;
        self.library = original_library;
        self.dependency_cache = original_dependency_cache;
        self.dependency_cache_ready = original_dependency_ready;
        self.selected = original_selected;
        self.config.data_dir = original_data_dir;
        lines.join("\n")
    }

    #[cfg(debug_assertions)]
    pub fn debug_smart_rank_zip_flow(&mut self) -> String {
        use smart_rank::SmartRankRefreshMode;

        let mut lines = Vec::new();
        lines.push("Smart rank zip flow (real imports)".to_string());

        let source_dir = PathBuf::from("/home/ryan/Documents/mod zips");
        if !source_dir.exists() {
            lines.push(format!("Source dir missing: {}", source_dir.display()));
            return lines.join("\n");
        }

        let mut archives: Vec<PathBuf> = match fs::read_dir(&source_dir) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| matches!(ext, "zip" | "ZIP" | "7z" | "7Z" | "rar" | "RAR"))
                        .unwrap_or(false)
                })
                .collect(),
            Err(err) => {
                lines.push(format!("Read source dir failed: {err}"));
                return lines.join("\n");
            }
        };
        archives.sort();
        if archives.is_empty() {
            lines.push("No mod archives found".to_string());
            return lines.join("\n");
        }
        archives.truncate(2);
        lines.push(format!("Using {} archive(s)", archives.len()));

        let original_library = self.library.clone();
        let original_dependency_cache = self.dependency_cache.clone();
        let original_dependency_ready = self.dependency_cache_ready;
        let original_selected = self.selected;
        let original_data_dir = self.config.data_dir.clone();
        let original_profile = self.config.active_profile.clone();
        let original_suppress = self.debug_suppress_persistence;
        let original_cache = self.smart_rank_cache.clone();
        let original_refresh_pending = self.smart_rank_refresh_pending;
        let original_refresh_kind = self.smart_rank_refresh_kind;
        let original_refresh_at = self.smart_rank_refresh_at;
        let original_last_saved = self.smart_rank_cache_last_saved;
        let original_scan_id = self.smart_rank_scan_id;
        let original_scan_active = self.smart_rank_scan_active;
        let original_scan_profile = self.smart_rank_scan_profile_key.clone();
        let original_status = self.status.clone();

        let temp_data_dir =
            std::env::temp_dir().join(format!("sigilsmith-debug-import-{}", now_timestamp()));
        if let Err(err) = fs::create_dir_all(&temp_data_dir) {
            lines.push(format!("Create temp dir failed: {err}"));
            return lines.join("\n");
        }

        self.library = Library {
            mods: Vec::new(),
            profiles: vec![Profile::new("Default")],
            active_profile: "Default".to_string(),
            dependency_blocks: HashSet::new(),
            metadata_cache_version: 0,
            metadata_cache_key: None,
            modsettings_hash: None,
            modsettings_sync_enabled: true,
        };
        self.config.active_profile = "Default".to_string();
        self.config.data_dir = temp_data_dir;
        self.dependency_cache.clear();
        self.dependency_cache_ready = false;
        self.prime_dependency_cache_from_library();
        self.debug_suppress_persistence = true;
        self.smart_rank_cache = None;
        self.smart_rank_refresh_pending = None;
        self.smart_rank_refresh_kind = None;
        self.smart_rank_refresh_at = None;
        self.smart_rank_cache_last_saved = None;
        self.smart_rank_scan_id = 0;
        self.smart_rank_scan_active = None;
        self.smart_rank_scan_profile_key = None;

        fn run_scans(app: &mut App, label: &str, lines: &mut Vec<String>) {
            let mut refresh_events = Vec::new();
            let mut full_rebuilds = 0usize;
            let mut last_scan_id: Option<u64> = None;
            let started = Instant::now();
            let timeout = Duration::from_secs(60);
            loop {
                app.poll_smart_rank();
                if !app.smart_rank_active {
                    if let Some(pending) = app.smart_rank_refresh_pending {
                        if let Some(ready_at) = app.smart_rank_refresh_at {
                            if Instant::now() >= ready_at {
                                app.smart_rank_refresh_pending = None;
                                app.smart_rank_refresh_at = None;
                                let refresh = app.resolve_smart_rank_refresh_kind(pending);
                                app.start_smart_rank_scan(SmartRankMode::Warmup, refresh);
                            }
                        } else {
                            app.smart_rank_refresh_pending = None;
                            let refresh = app.resolve_smart_rank_refresh_kind(pending);
                            app.start_smart_rank_scan(SmartRankMode::Warmup, refresh);
                        }
                    }
                }
                if let Some(scan_id) = app.smart_rank_scan_active {
                    if last_scan_id != Some(scan_id) {
                        if let Some(kind) = app.smart_rank_refresh_kind {
                            refresh_events.push(format!("scan kind={kind:?}"));
                            if matches!(kind, SmartRankRefreshMode::Full) {
                                full_rebuilds += 1;
                            }
                        }
                        last_scan_id = Some(scan_id);
                    }
                }
                if !app.smart_rank_active && app.smart_rank_refresh_pending.is_none() {
                    break;
                }
                if started.elapsed() > timeout {
                    refresh_events.push("timeout waiting for scans".to_string());
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }

            lines.push(format!("{label} refresh events: {}", refresh_events.len()));
            for event in refresh_events {
                lines.push(event);
            }
            lines.push(format!("{label} full rebuilds: {full_rebuilds}"));
        }

        for (index, path) in archives.iter().enumerate() {
            lines.push(format!("import {}: {}", index + 1, path.display()));
            let result =
                match importer::import_path_with_progress(path, &self.config.data_dir, None) {
                    Ok(result) => result,
                    Err(err) => {
                        lines.push(format!("  import failed: {err}"));
                        continue;
                    }
                };
            if result.batches.is_empty() {
                lines.push("  no mods found".to_string());
                continue;
            }
            for batch in result.batches {
                let count = match self.apply_mod_entries(batch.mods) {
                    Ok(count) => count,
                    Err(err) => {
                        lines.push(format!("  apply failed: {err}"));
                        0
                    }
                };
                lines.push(format!("  applied mods: {count}"));
                run_scans(self, "post-import", &mut lines);
            }
        }

        if let Some(mod_entry) = self.library.mods.first().cloned() {
            let remove_id = mod_entry.id.clone();
            let removed = self.remove_mod_by_id(&remove_id);
            lines.push(format!("remove: {removed} ({remove_id})"));
            run_scans(self, "post-remove", &mut lines);
        } else {
            lines.push("remove: skipped (no mods)".to_string());
        }

        self.library = original_library;
        self.dependency_cache = original_dependency_cache;
        self.dependency_cache_ready = original_dependency_ready;
        self.selected = original_selected;
        self.config.data_dir = original_data_dir;
        self.config.active_profile = original_profile;
        self.debug_suppress_persistence = original_suppress;
        self.smart_rank_cache = original_cache;
        self.smart_rank_refresh_pending = original_refresh_pending;
        self.smart_rank_refresh_kind = original_refresh_kind;
        self.smart_rank_refresh_at = original_refresh_at;
        self.smart_rank_cache_last_saved = original_last_saved;
        self.smart_rank_scan_id = original_scan_id;
        self.smart_rank_scan_active = original_scan_active;
        self.smart_rank_scan_profile_key = original_scan_profile;
        self.status = original_status;

        lines.join("\n")
    }

    #[cfg(debug_assertions)]
    pub fn debug_cache_report(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "Metadata cache key (stored): {}",
            self.library.metadata_cache_key.as_deref().unwrap_or("none")
        ));
        lines.push(format!(
            "Metadata cache key (current): {}",
            self.metadata_cache_key()
        ));
        lines.push(format!(
            "Modsettings hash (stored): {}",
            self.library.modsettings_hash.as_deref().unwrap_or("none")
        ));
        lines.push(format!(
            "Modsettings sync enabled: {}",
            self.library.modsettings_sync_enabled
        ));

        match game::detect_paths(
            self.game_id,
            Some(&self.config.game_root),
            Some(&self.config.larian_dir),
        ) {
            Ok(paths) => {
                if paths.modsettings_path.exists() {
                    match deploy::read_modsettings_snapshot(&paths.modsettings_path) {
                        Ok(snapshot) => {
                            let current = modsettings_fingerprint(&snapshot);
                            lines.push(format!("Modsettings hash (current): {current}"));
                            let matches = self
                                .library
                                .modsettings_hash
                                .as_ref()
                                .map(|stored| stored == &current)
                                .unwrap_or(false);
                            lines.push(format!("Modsettings hash match: {matches}"));
                        }
                        Err(err) => {
                            lines.push(format!("Modsettings read failed: {err}"));
                        }
                    }
                } else {
                    lines.push("Modsettings path missing".to_string());
                }
            }
            Err(err) => {
                lines.push(format!("Path detection failed: {err}"));
            }
        }

        lines.join("\n")
    }

    fn update_dependency_cache_for_entries(&mut self, entries: &[ModEntry]) {
        let mut changed = false;
        for mod_entry in entries {
            let deps = self.collect_mod_dependencies(mod_entry);
            self.dependency_cache.insert(mod_entry.id.clone(), deps);
            if let Some(entry) = self
                .library
                .mods
                .iter_mut()
                .find(|entry| entry.id == mod_entry.id)
            {
                let deps = self
                    .dependency_cache
                    .get(&mod_entry.id)
                    .cloned()
                    .unwrap_or_default();
                if entry.dependencies != deps {
                    entry.dependencies = deps;
                    changed = true;
                }
            }
        }
        if changed {
            self.library.metadata_cache_key = Some(self.metadata_cache_key());
            self.library.metadata_cache_version = METADATA_CACHE_VERSION;
            let _ = self.library.save(&self.config.data_dir);
        }
    }

    fn normalize_mod_sources(&mut self) -> bool {
        let mods_root = library_mod_root(&self.config.data_dir);
        let mut changed = false;
        for mod_entry in &mut self.library.mods {
            if mods_root.join(&mod_entry.id).exists() {
                if mod_entry.source != ModSource::Managed {
                    mod_entry.source = ModSource::Managed;
                    changed = true;
                }
            }
        }
        changed
    }

    fn resume_pending_import_batch(&mut self) {
        if self.dependency_queue.is_some() {
            return;
        }
        let Some(batch) = self.pending_import_batch.take() else {
            return;
        };
        if self.import_active.is_some()
            || !self.import_queue.is_empty()
            || !self.import_batches.is_empty()
            || self.pending_duplicate.is_some()
            || !self.duplicate_queue.is_empty()
            || self.dialog.is_some()
        {
            self.pending_import_batch = Some(batch);
            return;
        }

        self.stage_imports(batch.mods, &batch.source);
    }

    fn finish_dependency_queue(&mut self, proceed: bool) {
        let queue = self.dependency_queue.take();
        let Some(mut queue) = queue else {
            return;
        };

        if !proceed {
            if self.pending_import_batch.is_some() {
                self.cancel_pending_import(false);
            } else {
                self.pending_dependency_enable = None;
                self.status = "Dependency check canceled".to_string();
            }
            return;
        }

        queue.items.clear();

        if self.pending_dependency_enable.is_some() {
            self.apply_pending_dependency_enable();
        }
    }

    fn cancel_pending_import(&mut self, keep_files: bool) {
        let Some(batch) = self.pending_import_batch.take() else {
            return;
        };
        if !keep_files {
            for mod_entry in &batch.mods {
                self.remove_mod_root(&mod_entry.id);
            }
        }
        self.status = "Import canceled".to_string();
        self.log_warn("Import canceled during dependency check".to_string());
        self.import_summary_pending = true;
    }

    fn maybe_show_import_summary(&mut self) {
        if !self.import_summary_pending {
            return;
        }
        if self.import_active.is_some()
            || !self.import_batches.is_empty()
            || !self.import_queue.is_empty()
            || self.pending_duplicate.is_some()
            || !self.duplicate_queue.is_empty()
            || self.dialog.is_some()
        {
            return;
        }
        if self.import_failures.is_empty() {
            self.import_summary_pending = false;
            return;
        }

        let total = self.import_failures.len();
        let mut lines = Vec::new();
        lines.push(format!("Import completed with {total} failure(s)."));
        lines.push("".to_string());
        for failure in self.import_failures.iter().take(6) {
            lines.push(format!(
                "- {}: {}",
                failure.source.label,
                summarize_error(&failure.error)
            ));
        }
        if total > 6 {
            lines.push(format!("...and {} more (see log)", total - 6));
        }

        self.import_summary_pending = false;
        self.import_failures.clear();
        self.open_dialog(Dialog {
            title: "Import Summary".to_string(),
            message: lines.join("\n"),
            yes_label: "Close".to_string(),
            no_label: "Close".to_string(),
            choice: DialogChoice::Yes,
            kind: DialogKind::ImportSummary,
            toggle: None,
            scroll: 0,
        });
    }

    fn apply_mod_entries(&mut self, mods: Vec<ModEntry>) -> Result<usize> {
        let count = mods.len();
        if count == 0 {
            return Ok(0);
        }
        self.schedule_smart_rank_refresh(
            smart_rank::SmartRankRefreshMode::Incremental,
            "import",
            true,
        );

        let mut added = Vec::new();
        for mod_entry in mods {
            self.library.mods.retain(|entry| entry.id != mod_entry.id);
            self.library.mods.push(mod_entry.clone());
            added.push(mod_entry);
        }

        self.library.ensure_mods_in_profiles();
        if self.allow_persistence() {
            self.library.save(&self.config.data_dir)?;
        }
        self.update_dependency_cache_for_entries(&added);
        self.library.metadata_cache_key = Some(self.metadata_cache_key());
        self.library.metadata_cache_version = METADATA_CACHE_VERSION;
        if self.allow_persistence() {
            self.library.save(&self.config.data_dir)?;
            if self.normalize_mod_sources() {
                let _ = self.library.save(&self.config.data_dir);
            }
            self.queue_conflict_scan("library update");
        }
        Ok(count)
    }

    fn prompt_next_duplicate(&mut self) {
        if self.pending_duplicate.is_some() {
            return;
        }

        if self.dialog.is_some() {
            return;
        }

        if let Some(overwrite_all) = self.duplicate_apply_all {
            while let Some(next) = self.duplicate_queue.pop_front() {
                self.apply_duplicate_decision(next, overwrite_all);
            }
        }

        let Some(next) = self.duplicate_queue.pop_front() else {
            let approved = std::mem::take(&mut self.approved_imports);
            if approved.is_empty() {
                self.duplicate_apply_all = None;
                self.process_next_import_batch();
                return;
            }

            match self.apply_mod_entries(approved) {
                Ok(count) => {
                    self.status = format!("Imported {} mod(s)", count);
                    self.log_info(format!("Import complete: {count} mod(s)"));
                }
                Err(err) => {
                    let reason = summarize_error(&err.to_string());
                    self.status = format!("Import failed: {reason}");
                    self.log_error(format!("Import apply failed: {err}"));
                    self.set_toast(
                        &format!("Import failed: {reason}"),
                        ToastLevel::Error,
                        Duration::from_secs(4),
                    );
                    self.import_failures.push(importer::ImportFailure {
                        source: importer::ImportSource {
                            label: "Import batch".to_string(),
                        },
                        error: err.to_string(),
                    });
                    self.import_summary_pending = true;
                }
            }
            self.duplicate_apply_all = None;
            self.process_next_import_batch();
            return;
        };

        let display_name = next.mod_entry.display_name();
        let existing_label = next.existing_label.clone();
        let (title, message, kind) = match &next.kind {
            DuplicateKind::Exact => (
                "Overwrite Duplicate".to_string(),
                format!(
                    "Mod \"{}\" already exists.\nOverwrite \"{}\"?",
                    display_name, existing_label
                ),
                DialogKind::Overwrite,
            ),
            DuplicateKind::Similar {
                new_label,
                existing_label,
                new_stamp,
                existing_stamp,
                similarity,
            } => {
                let mut message = format!(
                    "Similar mod detected ({:.0}% match).\nNew: {}\nExisting: {}",
                    similarity * 100.0,
                    new_label,
                    existing_label
                );
                if let (Some(new_stamp), Some(existing_stamp)) = (new_stamp, existing_stamp) {
                    if new_stamp > existing_stamp {
                        message.push_str(&format!(
                            "\nNewer archive detected ({new_stamp} > {existing_stamp})."
                        ));
                    } else if new_stamp < existing_stamp {
                        message.push_str(&format!(
                            "\nExisting archive looks newer ({existing_stamp} > {new_stamp})."
                        ));
                    }
                }
                (
                    "Similar Mod Detected".to_string(),
                    message,
                    DialogKind::Similar,
                )
            }
        };

        let default_choice = if matches!(next.default_overwrite, Some(true)) {
            DialogChoice::Yes
        } else {
            DialogChoice::No
        };
        self.pending_duplicate = Some(next);
        self.open_dialog(Dialog {
            title,
            message,
            yes_label: "Overwrite".to_string(),
            no_label: "Skip".to_string(),
            choice: default_choice,
            kind,
            toggle: Some(DialogToggle {
                label: "Apply this choice to all remaining duplicates".to_string(),
                checked: false,
            }),
            scroll: 0,
        });
    }

    pub fn confirm_duplicate(&mut self, overwrite: bool, apply_all: bool) {
        let Some(decision) = self.pending_duplicate.take() else {
            return;
        };

        if apply_all {
            self.duplicate_apply_all = Some(overwrite);
        }
        self.apply_duplicate_decision(decision, overwrite);

        self.input_mode = InputMode::Normal;
        self.prompt_next_duplicate();
    }

    fn apply_duplicate_decision(&mut self, decision: DuplicateDecision, overwrite: bool) {
        if overwrite {
            let same_id = decision.existing_id == decision.mod_entry.id;
            let removed = if same_id {
                false
            } else {
                self.remove_mod_by_id(&decision.existing_id)
            };
            let label = match decision.kind {
                DuplicateKind::Exact => "duplicate",
                DuplicateKind::Similar { .. } => "similar",
            };
            if same_id {
                self.log_info(format!(
                    "Overwriting {label} mod \"{}\" (keeping files)",
                    decision.existing_label
                ));
            } else if removed {
                self.log_info(format!(
                    "Overwriting {label} mod \"{}\"",
                    decision.existing_label
                ));
            }
            self.approved_imports.push(decision.mod_entry);
        } else {
            let label = match decision.kind {
                DuplicateKind::Exact => "duplicate",
                DuplicateKind::Similar { .. } => "similar",
            };
            self.log_warn(format!(
                "Skipped {label} \"{}\"",
                decision.mod_entry.display_name()
            ));
            self.remove_mod_root(&decision.mod_entry.id);
        }
    }

    fn prompt_unrecognized(&mut self, path: PathBuf) {
        let label = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.to_string())
            .unwrap_or_else(|| path.display().to_string());

        self.open_dialog(Dialog {
            title: "Unrecognized Layout".to_string(),
            message: format!(
                "Mod directory paths are not recognized for:\n{label}\nImport anyway?"
            ),
            yes_label: "Import".to_string(),
            no_label: "Cancel".to_string(),
            choice: DialogChoice::No,
            kind: DialogKind::Unrecognized { path, label },
            toggle: None,
            scroll: 0,
        });
    }

    fn open_dialog(&mut self, mut dialog: Dialog) {
        dialog.scroll = 0;
        self.dialog = Some(dialog);
        self.move_mode = false;
        self.input_mode = InputMode::Normal;
    }

    pub fn dialog_choice_left(&mut self) {
        if let Some(dialog) = &mut self.dialog {
            dialog.choice = DialogChoice::Yes;
        }
    }

    pub fn dialog_choice_right(&mut self) {
        if let Some(dialog) = &mut self.dialog {
            dialog.choice = DialogChoice::No;
        }
    }

    pub fn dialog_set_choice(&mut self, choice: DialogChoice) {
        if let Some(dialog) = &mut self.dialog {
            dialog.choice = choice;
        }
    }

    pub fn dialog_confirm(&mut self) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };

        let choice = dialog.choice;
        match dialog.kind {
            DialogKind::Overwrite | DialogKind::Similar => {
                let apply_all = dialog
                    .toggle
                    .as_ref()
                    .map(|toggle| toggle.checked)
                    .unwrap_or(false);
                self.confirm_duplicate(matches!(choice, DialogChoice::Yes), apply_all);
            }
            DialogKind::Unrecognized { path, label } => {
                if matches!(choice, DialogChoice::Yes) {
                    let entry = build_unknown_entry(&path, &label);
                    self.log_warn(format!("Importing unknown layout: {label}"));
                    self.stage_imports(
                        vec![entry],
                        &importer::ImportSource {
                            label: label.clone(),
                        },
                    );
                } else {
                    self.log_warn(format!("Skipped unrecognized layout: {label}"));
                    self.process_next_import_batch();
                }
            }
            DialogKind::DeleteProfile { name } => {
                if matches!(choice, DialogChoice::Yes) {
                    if let Some(toggle) = dialog.toggle {
                        if toggle.checked {
                            self.app_config.confirm_profile_delete = false;
                            let _ = self.app_config.save();
                        }
                    }
                    if let Err(err) = self.delete_profile(name) {
                        self.status = format!("Profile delete failed: {err}");
                        self.log_error(format!("Profile delete failed: {err}"));
                    }
                }
            }
            DialogKind::DeleteMod {
                id,
                name,
                native,
                dependents,
            } => {
                if matches!(choice, DialogChoice::Yes) {
                    let mut remove_native_files = false;
                    if native {
                        remove_native_files = true;
                    } else if let Some(toggle) = dialog.toggle {
                        if toggle.checked {
                            self.app_config.confirm_mod_delete = false;
                            let _ = self.app_config.save();
                        }
                    }
                    if !self.remove_mod_by_id_with_options(&id, remove_native_files) {
                        self.status = "No mod removed".to_string();
                        return;
                    }
                    self.status = format!("Mod removed: {name}");
                    self.log_info(format!("Mod removed: {name}"));
                    let dependent_ids: Vec<String> =
                        dependents.iter().map(|item| item.id.clone()).collect();
                    let disabled = self.disable_mods_by_id(&dependent_ids);
                    if disabled > 0 {
                        self.status = format!("Disabled {disabled} dependent mod(s)");
                        self.log_warn(format!("Disabled {disabled} dependent mod(s)"));
                        self.queue_auto_deploy("dependency disabled");
                    }
                    self.clamp_selection();
                    self.queue_auto_deploy("mod removed");
                }
            }
            DialogKind::MoveBlocked {
                resume_move_mode,
                clear_filter,
            } => {
                if matches!(choice, DialogChoice::Yes) {
                    let previous_id = self.selected_profile_id();
                    if clear_filter {
                        self.mod_filter_snapshot = None;
                        self.apply_mod_filter(String::new(), false);
                    }
                    self.mod_sort = ModSort::default();
                    self.reselect_mod_by_id(previous_id);
                    if resume_move_mode {
                        self.toggle_move_mode();
                    } else {
                        self.status = "Order view restored".to_string();
                    }
                }
            }
            DialogKind::CancelImport => {
                if matches!(choice, DialogChoice::No) {
                    let keep_files = dialog
                        .toggle
                        .as_ref()
                        .map(|toggle| toggle.checked)
                        .unwrap_or(false);
                    self.dependency_queue = None;
                    self.cancel_pending_import(keep_files);
                }
            }
            DialogKind::OverrideDependencies => {
                if matches!(choice, DialogChoice::Yes) {
                    self.dependency_queue_continue();
                } else {
                    self.status = "Dependency override canceled".to_string();
                }
            }
            DialogKind::CopyDependencySearchLink { link } => {
                if let Some(toggle) = dialog.toggle {
                    if toggle.checked {
                        self.app_config.dependency_search_copy_preference =
                            Some(matches!(choice, DialogChoice::Yes));
                        let _ = self.app_config.save();
                    }
                }
                if matches!(choice, DialogChoice::Yes) {
                    if self.copy_to_clipboard(&link) {
                        self.status = "Search link copied".to_string();
                    }
                } else {
                    self.status = "Search link skipped".to_string();
                }
            }
            DialogKind::StartupDependencyNotice => {
                if matches!(choice, DialogChoice::No) {
                    self.app_config.show_startup_dependency_notice = false;
                    let _ = self.app_config.save();
                    self.status = "Startup dependency notice hidden".to_string();
                }
            }
            DialogKind::ImportSummary => {}
            DialogKind::EnableAllVisible => {}
            DialogKind::DisableAllVisible => {}
            DialogKind::InvertVisible => {}
        }
    }

    fn find_duplicate_by_name(&self, name: &str) -> Option<&ModEntry> {
        let needle = name.trim();
        self.library
            .mods
            .iter()
            .find(|entry| entry.name.trim().eq_ignore_ascii_case(needle))
    }

    fn find_similar_by_label(&self, mod_entry: &ModEntry) -> Option<SimilarMatch> {
        let new_raw = mod_entry.source_label().unwrap_or(mod_entry.name.as_str());
        let new_normalized = normalize_label(new_raw);
        if new_normalized.len() < 6 {
            return None;
        }

        let mut best: Option<SimilarMatch> = None;
        for existing in &self.library.mods {
            let existing_raw = existing.source_label().unwrap_or(existing.name.as_str());
            let existing_normalized = normalize_label(existing_raw);
            if existing_normalized.len() < 6 {
                continue;
            }
            let similarity = similarity_ratio(&new_normalized, &existing_normalized);
            if similarity < 0.88 {
                continue;
            }

            let candidate = SimilarMatch {
                existing_id: existing.id.clone(),
                existing_label: existing.display_name(),
                new_label: mod_entry.display_name(),
                new_stamp: extract_timestamp(new_raw),
                existing_stamp: extract_timestamp(existing_raw),
                similarity,
            };

            match &best {
                Some(current) if current.similarity >= similarity => {}
                _ => best = Some(candidate),
            }
        }

        best
    }

    fn mod_stamp(entry: &ModEntry) -> Option<i64> {
        let label = entry.source_label().unwrap_or(entry.name.as_str());
        extract_timestamp(label)
            .map(|stamp| stamp as i64)
            .or(entry.created_at)
            .or(entry.modified_at)
    }

    fn remove_mod_by_id(&mut self, id: &str) -> bool {
        self.remove_mod_by_id_with_options(id, false)
    }

    fn remove_mod_by_id_with_options(&mut self, id: &str, delete_native_files: bool) -> bool {
        self.schedule_smart_rank_refresh(
            smart_rank::SmartRankRefreshMode::Incremental,
            "remove",
            true,
        );
        let mod_entry = match self.library.mods.iter().find(|entry| entry.id == id) {
            Some(entry) => entry.clone(),
            None => return false,
        };
        if delete_native_files && mod_entry.is_native() && self.allow_persistence() {
            self.remove_native_mod_files(&mod_entry);
        }

        let before = self.library.mods.len();
        self.library.mods.retain(|mod_entry| mod_entry.id != id);
        if before == self.library.mods.len() {
            return false;
        }
        self.library.dependency_blocks.remove(id);

        for profile in &mut self.library.profiles {
            profile.order.retain(|entry| entry.id != id);
            profile
                .file_overrides
                .retain(|override_entry| override_entry.mod_id != id);
        }

        if self.allow_persistence() {
            self.queue_remove_mod_root(id);
        }
        self.dependency_cache.remove(id);
        self.library.metadata_cache_key = Some(self.metadata_cache_key());
        self.library.metadata_cache_version = METADATA_CACHE_VERSION;
        if self.allow_persistence() {
            let _ = self.library.save(&self.config.data_dir);
        }
        self.queue_conflict_scan("mod removed");
        true
    }

    fn disable_mods_by_id(&mut self, ids: &[String]) -> usize {
        if ids.is_empty() {
            return 0;
        }
        let id_set: HashSet<&str> = ids.iter().map(|id| id.as_str()).collect();
        let mut changed = 0;
        for profile in &mut self.library.profiles {
            for entry in &mut profile.order {
                if id_set.contains(entry.id.as_str()) && entry.enabled {
                    entry.enabled = false;
                    changed += 1;
                }
            }
        }
        if changed > 0 {
            if self.allow_persistence() {
                let _ = self.library.save(&self.config.data_dir);
            }
        }
        changed
    }

    fn remove_native_mod_files(&mut self, mod_entry: &ModEntry) {
        let paths = match game::detect_paths(
            self.game_id,
            Some(&self.config.game_root),
            Some(&self.config.larian_dir),
        ) {
            Ok(paths) => paths,
            Err(err) => {
                self.log_warn(format!("Native mod file remove skipped: {err}"));
                return;
            }
        };
        let pak_info = mod_entry.targets.iter().find_map(|target| match target {
            crate::library::InstallTarget::Pak { info, .. } => Some(info.clone()),
            _ => None,
        });
        let Some(info) = pak_info else {
            self.log_warn("Native mod file remove skipped: missing pak info".to_string());
            return;
        };
        let native_pak_index = native_pak::build_native_pak_index(&paths.larian_mods_dir);
        let file_name = mod_entry
            .targets
            .iter()
            .find_map(|target| match target {
                crate::library::InstallTarget::Pak { file, .. } => Some(file.clone()),
                _ => None,
            })
            .or_else(|| native_pak::resolve_native_pak_filename(&info, &native_pak_index))
            .unwrap_or_else(|| format!("{}.pak", info.folder));
        let pak_path = paths.larian_mods_dir.join(&file_name);
        if !pak_path.exists() {
            self.log_warn(format!(
                "Native mod file not found in Mods folder: {file_name}"
            ));
            return;
        }
        if let Err(err) = fs::remove_file(&pak_path) {
            self.log_warn(format!("Native mod file remove failed: {err}"));
        } else {
            self.log_info(format!("Native mod file removed: {file_name}"));
        }
    }

    fn remove_mod_root(&self, id: &str) {
        let mod_root = self.config.data_dir.join("mods").join(id);
        let _ = fs::remove_dir_all(&mod_root);
    }

    fn queue_remove_mod_root(&mut self, id: &str) {
        let mod_root = self.config.data_dir.join("mods").join(id);
        if !mod_root.exists() {
            return;
        }
        let trash_root = self.config.data_dir.join("trash");
        if let Err(err) = fs::create_dir_all(&trash_root) {
            self.log_warn(format!("Remove mod files skipped: {err}"));
            return;
        }
        let stamp = now_timestamp();
        let trash_path = trash_root.join(format!("{id}-{stamp}"));
        match fs::rename(&mod_root, &trash_path) {
            Ok(()) => {
                thread::spawn(move || {
                    let _ = fs::remove_dir_all(&trash_path);
                });
            }
            Err(err) => {
                self.log_warn(format!("Remove mod files skipped: {err}"));
            }
        }
    }

    fn apply_native_sync_delta(&mut self, delta: NativeSyncDelta) {
        let mut changed = false;
        let mut dependencies_changed = false;
        let updated_native_files = delta.updated_native_files;
        let adopted_native = delta.adopted_native;
        let modsettings_hash_changed = delta.modsettings_hash != self.library.modsettings_hash;

        for update in delta.updates {
            let Some(entry) = self
                .library
                .mods
                .iter_mut()
                .find(|entry| entry.id == update.id)
            else {
                continue;
            };
            let managed_root = self.config.data_dir.join("mods").join(&update.id);
            if update.source == ModSource::Native && managed_root.exists() {
                if entry.source != ModSource::Managed {
                    entry.source = ModSource::Managed;
                    changed = true;
                }
                continue;
            }
            if entry.source != update.source {
                entry.source = update.source;
                changed = true;
            }
            if entry.name != update.name {
                entry.name = update.name;
                changed = true;
            }
            if entry.source_label != update.source_label {
                entry.source_label = update.source_label;
                changed = true;
            }
            if entry.targets != update.targets {
                entry.targets = update.targets;
                changed = true;
            }
            if entry.created_at != update.created_at {
                entry.created_at = update.created_at;
                changed = true;
            }
            if entry.modified_at != update.modified_at {
                entry.modified_at = update.modified_at;
                changed = true;
            }
            if entry.dependencies != update.dependencies {
                entry.dependencies = update.dependencies;
                dependencies_changed = true;
                changed = true;
            }
        }

        let mut added = 0usize;
        // Only import missing mods when modsettings changed to avoid resurrecting removals.
        if modsettings_hash_changed && !delta.added.is_empty() {
            let mut existing_ids: HashSet<String> = self
                .library
                .mods
                .iter()
                .map(|entry| entry.id.clone())
                .collect();
            for mod_entry in delta.added {
                if existing_ids.insert(mod_entry.id.clone()) {
                    if !mod_entry.dependencies.is_empty() {
                        dependencies_changed = true;
                    }
                    self.library.mods.push(mod_entry);
                    added += 1;
                    changed = true;
                }
            }
            if added > 0 {
                self.library.ensure_mods_in_profiles();
            }
        }

        if modsettings_hash_changed {
            self.library.modsettings_hash = delta.modsettings_hash.clone();
        }

        let mut updated_enabled = false;
        let mut reordered = false;
        let should_apply_modsettings = delta.modsettings_exists
            && modsettings_hash_changed
            && self.library.modsettings_sync_enabled;
        if should_apply_modsettings {
            let mod_has_pak: HashMap<String, bool> = self
                .library
                .mods
                .iter()
                .map(|mod_entry| {
                    (
                        mod_entry.id.clone(),
                        mod_entry.has_target_kind(TargetKind::Pak),
                    )
                })
                .collect();
            let dependency_blocks = self.library.dependency_blocks.clone();
            if let Some(profile) = self.library.active_profile_mut() {
                for entry in &mut profile.order {
                    let has_pak = mod_has_pak.get(&entry.id).copied().unwrap_or(false);
                    if !has_pak {
                        continue;
                    }
                    let mut desired = delta.enabled_set.contains(&entry.id);
                    if desired && dependency_blocks.contains(&entry.id) {
                        desired = false;
                    }
                    if entry.enabled != desired {
                        entry.enabled = desired;
                        updated_enabled = true;
                    }
                }
            }
            if let Some(profile) = self.library.active_profile_mut() {
                if !delta.order.is_empty() {
                    let entry_map: HashMap<String, ProfileEntry> = profile
                        .order
                        .iter()
                        .cloned()
                        .map(|entry| (entry.id.clone(), entry))
                        .collect();
                    let mut loose_ids = Vec::new();
                    let mut pak_ids = Vec::new();
                    for entry in &profile.order {
                        let has_pak = mod_has_pak.get(&entry.id).copied().unwrap_or(false);
                        if has_pak {
                            pak_ids.push(entry.id.clone());
                        } else {
                            loose_ids.push(entry.id.clone());
                        }
                    }
                    let mut pak_set: HashSet<String> = pak_ids.iter().cloned().collect();
                    let mut pak_ordered = Vec::new();
                    for uuid in &delta.order {
                        if pak_set.remove(uuid) {
                            pak_ordered.push(uuid.clone());
                        }
                    }
                    for id in pak_ids {
                        if pak_set.contains(&id) {
                            pak_ordered.push(id);
                        }
                    }
                    let mut new_order = Vec::new();
                    new_order.extend(loose_ids);
                    new_order.extend(pak_ordered);
                    let mut reordered_entries = Vec::new();
                    for id in new_order {
                        if let Some(entry) = entry_map.get(&id) {
                            reordered_entries.push(entry.clone());
                        }
                    }
                    if reordered_entries.len() == profile.order.len()
                        && reordered_entries != profile.order
                    {
                        profile.order = reordered_entries;
                        reordered = true;
                    }
                }
            }
        }

        if self.normalize_mod_sources() {
            changed = true;
        }

        if added > 0
            || updated_enabled
            || reordered
            || updated_native_files > 0
            || adopted_native > 0
            || changed
            || modsettings_hash_changed
        {
            self.library.metadata_cache_key = Some(self.metadata_cache_key());
            self.library.metadata_cache_version = METADATA_CACHE_VERSION;
            if let Err(err) = self.library.save(&self.config.data_dir) {
                self.log_warn(format!("Native mod sync save failed: {err}"));
            }
            if dependencies_changed {
                self.prime_dependency_cache_from_library();
            }
            self.schedule_smart_rank_refresh(
                smart_rank::SmartRankRefreshMode::Incremental,
                "native sync",
                true,
            );
            if added > 0 {
                self.log_info(format!("Native mods added: {added}"));
            }
            if updated_native_files > 0 {
                self.log_info(format!(
                    "Native mod filenames updated: {updated_native_files}"
                ));
            }
            if adopted_native > 0 {
                self.log_info(format!("Native mods reconciled: {adopted_native}"));
            }
            if reordered {
                self.log_info("Native mod order synced".to_string());
            }
            self.status = "Native mods synced".to_string();
        } else {
            self.status = "Native mods already synced".to_string();
        }
    }

    fn self_heal_missing_paks(&mut self) -> usize {
        let mod_map = self.library.index_by_id();
        let mut actions = Vec::new();
        let mut restores = Vec::new();
        let mut rename_targets: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for (id, mod_entry) in &mod_map {
            if mod_entry.is_native() {
                continue;
            }
            let mut missing_pak = false;
            let mut pak_exists = false;
            let mut has_other_enabled = false;
            let pak_enabled = mod_entry.is_target_enabled(TargetKind::Pak);
            let mut has_pak_target = false;
            let mod_root = library_mod_root(&self.config.data_dir).join(id);
            let pak_index = if mod_root.exists() {
                Some(native_pak::build_native_pak_index(&mod_root))
            } else {
                None
            };
            for target in &mod_entry.targets {
                let kind = target.kind();
                match target {
                    InstallTarget::Pak { file, .. } => {
                        has_pak_target = true;
                        let source = mod_root.join(file);
                        if source.exists() {
                            pak_exists = true;
                        } else if let Some(index) = pak_index.as_ref() {
                            if let InstallTarget::Pak { info, .. } = target {
                                if let Some(resolved) =
                                    native_pak::resolve_native_pak_path(info, index)
                                {
                                    if resolved.exists() {
                                        pak_exists = true;
                                        if let Some(name) =
                                            resolved.file_name().and_then(|name| name.to_str())
                                        {
                                            if name != file {
                                                rename_targets
                                                    .entry(id.clone())
                                                    .or_default()
                                                    .push((file.clone(), name.to_string()));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {
                        if mod_entry.is_target_enabled(kind) {
                            has_other_enabled = true;
                        }
                    }
                }
            }
            if has_pak_target && !pak_exists {
                pak_exists =
                    !resolve_pak_paths(mod_entry, &self.config.data_dir, None, None).is_empty();
            }
            if has_pak_target && pak_enabled && !pak_exists {
                missing_pak = true;
            }
            if missing_pak {
                actions.push((id.clone(), mod_entry.display_name(), has_other_enabled));
                continue;
            }
            if pak_exists && !pak_enabled && !has_other_enabled {
                if mod_entry.target_overrides.len() == 1
                    && mod_entry.target_overrides[0].kind == TargetKind::Pak
                    && !mod_entry.target_overrides[0].enabled
                {
                    restores.push(id.clone());
                }
            }
        }

        if actions.is_empty() && restores.is_empty() && rename_targets.is_empty() {
            return 0;
        }

        let mut changed = false;
        if !rename_targets.is_empty() {
            for mod_entry in &mut self.library.mods {
                let Some(renames) = rename_targets.get(&mod_entry.id) else {
                    continue;
                };
                for target in &mut mod_entry.targets {
                    if let InstallTarget::Pak { file, .. } = target {
                        if let Some((_, new_name)) = renames.iter().find(|(old, _)| old == file) {
                            *file = new_name.clone();
                            changed = true;
                        }
                    }
                }
            }
        }
        if let Some(profile) = self.library.active_profile_mut() {
            for (id, _, has_other_enabled) in &actions {
                if *has_other_enabled {
                    continue;
                }
                if let Some(entry) = profile.order.iter_mut().find(|entry| entry.id == *id) {
                    if entry.enabled {
                        entry.enabled = false;
                        changed = true;
                    }
                }
            }
        }

        for (id, _, _) in &actions {
            if let Some(mod_entry) = self.library.mods.iter_mut().find(|entry| entry.id == *id) {
                if set_target_override(mod_entry, TargetKind::Pak, false) {
                    changed = true;
                }
            }
        }
        for id in &restores {
            if let Some(mod_entry) = self.library.mods.iter_mut().find(|entry| entry.id == *id) {
                if mod_entry.target_overrides.len() == 1
                    && mod_entry.target_overrides[0].kind == TargetKind::Pak
                    && !mod_entry.target_overrides[0].enabled
                {
                    mod_entry.target_overrides.clear();
                    changed = true;
                }
            }
        }

        if changed {
            let _ = self.library.save(&self.config.data_dir);
        }

        actions.len() + restores.len()
    }

    pub fn deploy(&mut self) -> Result<()> {
        self.queue_deploy("manual deploy");
        Ok(())
    }

    pub fn rollback_last_backup(&mut self) -> Result<()> {
        if self.import_active.is_some() || self.deploy_active || self.deploy_pending {
            self.status = "Rollback blocked: active tasks".to_string();
            self.log_warn("Rollback blocked: active tasks".to_string());
            self.set_toast(
                "Rollback blocked: active tasks",
                ToastLevel::Warn,
                Duration::from_secs(3),
            );
            return Ok(());
        }

        let Some(backup_dir) = backup::load_last_backup(&self.config.data_dir)? else {
            self.status = "No backup available".to_string();
            self.log_warn("No backup available".to_string());
            self.set_toast(
                "No backup available",
                ToastLevel::Warn,
                Duration::from_secs(3),
            );
            return Ok(());
        };

        let mut library = backup::load_backup_library(&backup_dir)?;
        if library.profiles.is_empty() {
            library
                .profiles
                .push(crate::library::Profile::new("Default"));
        }
        if library.active_profile.is_empty()
            || !library
                .profiles
                .iter()
                .any(|profile| profile.name == library.active_profile)
        {
            library.active_profile = library.profiles[0].name.clone();
        }
        library.ensure_mods_in_profiles();
        self.library = library;
        self.config.active_profile = self.library.active_profile.clone();
        self.library.save(&self.config.data_dir)?;
        self.config.save()?;
        self.conflicts.clear();
        self.conflict_selected = 0;

        self.queue_deploy_with_options("rollback", false);
        self.queue_conflict_scan("rollback");
        self.status = "Rollback queued".to_string();
        self.log_info(format!("Rollback queued from {}", backup_dir.display()));
        Ok(())
    }

    pub fn toggle_selected(&mut self) {
        if self.block_mod_changes("toggle") {
            return;
        }
        let Some(index) = self.selected_profile_index() else {
            return;
        };
        let Some(profile) = self.library.active_profile() else {
            return;
        };
        let Some(entry) = profile.order.get(index) else {
            return;
        };
        let id = entry.id.clone();
        let enabled = entry.enabled;
        if enabled {
            self.set_mods_enabled_in_active(&[id], false);
            self.queue_auto_deploy("enable toggle");
        } else {
            self.enable_mods_with_dependencies(vec![id]);
        }
    }

    fn enable_mods_with_dependencies(&mut self, ids: Vec<String>) {
        if self.block_mod_changes("enable") {
            return;
        }
        let mut mods = Vec::new();
        for id in &ids {
            if let Some(entry) = self.library.mods.iter().find(|entry| entry.id == *id) {
                mods.push(entry.clone());
            }
        }
        if mods.is_empty() {
            self.status = "No mods to enable".to_string();
            return;
        }

        let mut dependency_labels: Vec<String> = Vec::new();
        for mod_entry in &mods {
            dependency_labels.extend(self.cached_mod_dependencies(mod_entry));
        }

        let lookup = DependencyLookup::new(&self.library.mods);
        let mut present: HashSet<String> = HashSet::new();
        let mut missing = Vec::new();
        for dep in dependency_labels {
            let ids = lookup.resolve_ids(&dep);
            if ids.is_empty() {
                if is_unverified_dependency(&dep) {
                    continue;
                }
                missing.push(dep);
            } else {
                for id in ids {
                    present.insert(id);
                }
            }
        }
        missing.sort();
        missing.dedup();

        if !missing.is_empty() {
            if !self.app_config.offer_dependency_downloads
                && !self.app_config.warn_missing_dependencies
            {
                self.status = "Missing dependencies; enable blocked".to_string();
                self.log_warn("Missing dependencies; enable blocked".to_string());
                return;
            }
            if let Some(queue) = self.build_dependency_queue_for_mods(&mods) {
                let mut to_enable = ids.clone();
                to_enable.extend(present.into_iter());
                to_enable.sort();
                to_enable.dedup();
                self.pending_dependency_enable = Some(to_enable);
                self.dependency_queue = Some(queue);
                self.status = "Missing dependencies detected".to_string();
                self.log_warn("Missing dependencies detected".to_string());
                return;
            }
            self.status = "Missing dependencies; enable blocked".to_string();
            self.log_warn("Missing dependencies; enable blocked".to_string());
            return;
        }

        let mut to_enable = ids;
        to_enable.extend(present.into_iter());
        to_enable.sort();
        to_enable.dedup();
        let changed = self.set_mods_enabled_in_active(&to_enable, true);
        if changed == 0 {
            self.status = "Mods already enabled".to_string();
            return;
        }
        self.status = format!("Enabled {changed} mod(s)");
        self.log_info(format!("Enabled {changed} mod(s)"));
        self.queue_auto_deploy("enable dependencies");
    }

    fn apply_pending_dependency_enable(&mut self) {
        let Some(ids) = self.pending_dependency_enable.take() else {
            return;
        };
        let changed = self.set_mods_enabled_in_active(&ids, true);
        if changed == 0 {
            self.status = "Dependencies already enabled".to_string();
            return;
        }
        self.status = format!("Enabled {changed} dependency mod(s)");
        self.log_info(format!("Enabled {changed} dependency mod(s)"));
        self.queue_auto_deploy("dependency enable");
    }

    fn set_mods_enabled_in_active(&mut self, ids: &[String], enabled: bool) -> usize {
        let Some(profile) = self.library.active_profile_mut() else {
            return 0;
        };
        let id_set: HashSet<&str> = ids.iter().map(|id| id.as_str()).collect();
        let mut changed = 0;
        for entry in &mut profile.order {
            if id_set.contains(entry.id.as_str()) && entry.enabled != enabled {
                entry.enabled = enabled;
                changed += 1;
            }
        }
        if changed > 0 {
            self.library.modsettings_sync_enabled = false;
            self.schedule_smart_rank_refresh(
                smart_rank::SmartRankRefreshMode::Incremental,
                if enabled { "enable" } else { "disable" },
                true,
            );
            if self.allow_persistence() {
                let _ = self.library.save(&self.config.data_dir);
            }
        }
        changed
    }

    pub fn toggle_move_mode(&mut self) {
        if self.move_mode {
            self.move_mode = false;
            self.status = "Move mode disabled".to_string();
            if self.move_dirty {
                self.move_dirty = false;
                self.schedule_smart_rank_refresh(
                    smart_rank::SmartRankRefreshMode::ReorderOnly,
                    "order changed",
                    true,
                );
                self.queue_auto_deploy("order changed");
            }
        } else {
            self.move_mode = true;
            self.move_dirty = false;
            self.status = "Move mode: use arrows to reorder, Enter/Esc to exit".to_string();
        }
    }

    pub fn remove_selected(&mut self) {
        let selected_id = self.selected_profile_id();
        let Some(selected_id) = selected_id else {
            return;
        };
        let Some(entry) = self
            .library
            .mods
            .iter()
            .find(|mod_entry| mod_entry.id == selected_id)
        else {
            return;
        };
        if self.metadata_active && !self.dependency_cache_ready {
            self.queue_pending_delete(entry.id.clone(), entry.display_name());
            return;
        }
        let dependents = self.find_dependents(&selected_id);

        if !self.remove_mod_by_id(&selected_id) {
            self.status = "No mod removed".to_string();
            return;
        }

        self.status = "Mod removed from library".to_string();
        self.log_info("Mod removed from library".to_string());
        let dependent_ids: Vec<String> = dependents.iter().map(|item| item.id.clone()).collect();
        let disabled = self.disable_mods_by_id(&dependent_ids);
        if disabled > 0 {
            self.status = format!("Disabled {disabled} dependent mod(s)");
            self.log_warn(format!("Disabled {disabled} dependent mod(s)"));
            self.queue_auto_deploy("dependency disabled");
        }
        self.clamp_selection();
        self.queue_auto_deploy("mod removed");
    }

    pub fn request_remove_selected(&mut self) {
        if self.block_mod_changes("remove") {
            return;
        }
        let Some(selected_id) = self.selected_profile_id() else {
            return;
        };
        let Some(entry) = self
            .library
            .mods
            .iter()
            .find(|mod_entry| mod_entry.id == selected_id)
        else {
            return;
        };

        if self.metadata_active && !self.dependency_cache_ready {
            self.queue_pending_delete(entry.id.clone(), entry.display_name());
            return;
        }

        let dependents = self.find_dependents(&entry.id);
        if entry.is_native() || self.app_config.confirm_mod_delete || !dependents.is_empty() {
            self.prompt_delete_mod(entry.id.clone(), entry.display_name());
        } else {
            self.remove_selected();
        }
    }

    pub fn select_target_override(&mut self, selection: Option<TargetKind>) {
        let selected_id = self.selected_profile_id();
        let Some(selected_id) = selected_id else {
            return;
        };
        let Some(mod_entry) = self
            .library
            .mods
            .iter_mut()
            .find(|mod_entry| mod_entry.id == selected_id)
        else {
            return;
        };

        if let Some(kind) = selection {
            if !mod_entry.has_target_kind(kind) {
                self.status = "Target not present for this mod".to_string();
                return;
            }
            let mut present = HashSet::new();
            for target in &mod_entry.targets {
                present.insert(target.kind());
            }
            mod_entry.target_overrides.clear();
            for present_kind in present {
                mod_entry.target_overrides.push(TargetOverride {
                    kind: present_kind,
                    enabled: present_kind == kind,
                });
            }
            let label = match kind {
                TargetKind::Pak => "Pak",
                TargetKind::Generated => "Generated",
                TargetKind::Data => "Data",
                TargetKind::Bin => "Bin",
            };
            self.status = format!("Target override: {label}");
        } else {
            mod_entry.target_overrides.clear();
            self.status = "Target override: Auto".to_string();
        }
        let _ = self.library.save(&self.config.data_dir);
        self.queue_auto_deploy("target override");
    }

    pub fn move_selected_up(&mut self) {
        let indices = self.visible_profile_indices();
        if indices.is_empty() || self.selected == 0 {
            return;
        }
        let current_index = match indices.get(self.selected) {
            Some(index) => *index,
            None => return,
        };
        let prev_index = match indices.get(self.selected - 1) {
            Some(index) => *index,
            None => return,
        };
        let Some(profile) = self.library.active_profile_mut() else {
            return;
        };
        if current_index >= profile.order.len() || prev_index >= profile.order.len() {
            return;
        }
        if current_index == prev_index + 1 {
            profile.move_up(current_index);
        } else {
            profile.order.swap(current_index, prev_index);
        }
        self.selected = self.selected.saturating_sub(1);
        if self.move_mode {
            self.move_dirty = true;
        } else {
            self.schedule_smart_rank_refresh(
                smart_rank::SmartRankRefreshMode::ReorderOnly,
                "order changed",
                true,
            );
            self.queue_auto_deploy("order changed");
        }
    }

    pub fn move_selected_down(&mut self) {
        let indices = self.visible_profile_indices();
        if indices.is_empty() || self.selected + 1 >= indices.len() {
            return;
        }
        let current_index = match indices.get(self.selected) {
            Some(index) => *index,
            None => return,
        };
        let next_index = match indices.get(self.selected + 1) {
            Some(index) => *index,
            None => return,
        };
        let Some(profile) = self.library.active_profile_mut() else {
            return;
        };
        if current_index >= profile.order.len() || next_index >= profile.order.len() {
            return;
        }
        if next_index == current_index + 1 {
            profile.move_down(current_index);
        } else {
            profile.order.swap(current_index, next_index);
        }
        self.selected = (self.selected + 1).min(indices.len().saturating_sub(1));
        if self.move_mode {
            self.move_dirty = true;
        } else {
            self.schedule_smart_rank_refresh(
                smart_rank::SmartRankRefreshMode::ReorderOnly,
                "order changed",
                true,
            );
            self.queue_auto_deploy("order changed");
        }
    }

    pub fn enable_visible_mods(&mut self) {
        if self.block_mod_changes("enable") {
            return;
        }
        let indices = self.visible_profile_indices();
        if indices.is_empty() {
            self.status = "No visible mods to enable".to_string();
            return;
        }
        let Some(profile) = self.library.active_profile() else {
            return;
        };
        let mut ids = Vec::new();
        for index in indices {
            if let Some(entry) = profile.order.get(index) {
                if !entry.enabled {
                    ids.push(entry.id.clone());
                }
            }
        }
        if ids.is_empty() {
            self.status = "Visible mods already enabled".to_string();
            return;
        }
        self.enable_mods_with_dependencies(ids);
    }

    pub fn disable_visible_mods(&mut self) {
        if self.block_mod_changes("disable") {
            return;
        }
        let indices = self.visible_profile_indices();
        if indices.is_empty() {
            self.status = "No visible mods to disable".to_string();
            return;
        }
        let Some(profile) = self.library.active_profile() else {
            return;
        };
        let mut ids = Vec::new();
        for index in indices {
            if let Some(entry) = profile.order.get(index) {
                if entry.enabled {
                    ids.push(entry.id.clone());
                }
            }
        }
        let changed = self.set_mods_enabled_in_active(&ids, false);
        if changed == 0 {
            self.status = "Visible mods already disabled".to_string();
            return;
        }
        self.status = format!("Disabled {changed} mod(s)");
        self.log_info(format!("Disabled {changed} mod(s)"));
        self.queue_auto_deploy("disable all");
    }

    pub fn invert_visible_mods(&mut self) {
        if self.block_mod_changes("toggle") {
            return;
        }
        let indices = self.visible_profile_indices();
        if indices.is_empty() {
            self.status = "No visible mods to invert".to_string();
            return;
        }
        let Some(profile) = self.library.active_profile() else {
            return;
        };
        let mut to_disable = Vec::new();
        let mut to_enable = Vec::new();
        for index in indices {
            if let Some(entry) = profile.order.get(index) {
                if entry.enabled {
                    to_disable.push(entry.id.clone());
                } else {
                    to_enable.push(entry.id.clone());
                }
            }
        }
        let disabled = self.set_mods_enabled_in_active(&to_disable, false);
        if !to_enable.is_empty() {
            self.enable_mods_with_dependencies(to_enable);
            if self.dependency_queue.is_some() || self.pending_dependency_enable.is_some() {
                if disabled > 0 {
                    self.queue_auto_deploy("invert selection");
                }
                return;
            }
        } else if disabled == 0 {
            self.status = "No visible mods to invert".to_string();
            return;
        }
        self.status = "Toggled visible mods".to_string();
        self.log_info("Toggled visible mods".to_string());
        if disabled > 0 {
            self.queue_auto_deploy("invert selection");
        }
    }

    pub fn clear_visible_overrides(&mut self) {
        let indices = self.visible_profile_indices();
        if indices.is_empty() {
            self.status = "No visible mods to clear overrides".to_string();
            return;
        }
        let mod_ids: HashSet<String> = {
            let Some(profile) = self.library.active_profile() else {
                return;
            };
            indices
                .iter()
                .filter_map(|index| profile.order.get(*index).map(|entry| entry.id.clone()))
                .collect()
        };
        let mut changed = 0;
        for mod_entry in &mut self.library.mods {
            if mod_ids.contains(&mod_entry.id) && !mod_entry.target_overrides.is_empty() {
                mod_entry.target_overrides.clear();
                changed += 1;
            }
        }
        if changed == 0 {
            self.status = "No overrides to clear".to_string();
            return;
        }
        let _ = self.library.save(&self.config.data_dir);
        self.status = format!("Cleared overrides on {changed} mod(s)");
        self.log_info(format!("Cleared overrides on {changed} mod(s)"));
        self.queue_auto_deploy("clear overrides");
    }

    fn queue_auto_deploy(&mut self, reason: &str) {
        if !self.allow_persistence() {
            return;
        }
        self.queue_deploy(&format!("auto: {reason}"));
        self.queue_conflict_scan(reason);
    }

    fn queue_deploy(&mut self, reason: &str) {
        if !self.paths_ready() {
            self.status = "Game paths not set: open Menu (Esc) to configure".to_string();
            self.log_warn("Deploy skipped: game paths not set".to_string());
            return;
        }

        if self.deploy_pending || self.deploy_active {
            self.deploy_pending = true;
            if self.deploy_reason.is_none() {
                self.deploy_reason = Some(reason.to_string());
            }
            return;
        }

        self.deploy_pending = true;
        self.deploy_reason = Some(reason.to_string());
        self.deploy_backup = true;
        self.status = format!("Deploy queued ({reason})");
        self.log_info(format!("Deploy queued ({reason})"));
    }

    fn queue_deploy_with_options(&mut self, reason: &str, backup: bool) {
        if !self.paths_ready() {
            self.status = "Game paths not set: open Menu (Esc) to configure".to_string();
            self.log_warn("Deploy skipped: game paths not set".to_string());
            return;
        }

        if self.deploy_pending || self.deploy_active {
            return;
        }

        self.deploy_pending = true;
        self.deploy_reason = Some(reason.to_string());
        self.deploy_backup = backup;
        self.status = format!("Deploy queued ({reason})");
        self.log_info(format!("Deploy queued ({reason})"));
    }

    fn queue_conflict_scan(&mut self, _reason: &str) {
        if !self.paths_ready() {
            if !self.conflicts.is_empty() {
                self.conflicts.clear();
                self.conflict_selected = 0;
            }
            return;
        }

        if self.conflict_active {
            self.conflict_pending = true;
            return;
        }
        self.conflict_pending = true;
    }

    fn maybe_start_conflict_scan(&mut self) {
        if !self.conflict_pending || self.conflict_active {
            return;
        }
        if self.import_active.is_some() || self.deploy_active {
            return;
        }

        self.conflict_pending = false;
        self.conflict_active = true;

        let tx = self.conflict_tx.clone();
        let config = self.config.clone();
        let library = self.library.clone();
        thread::spawn(move || {
            let result = deploy::scan_conflicts(&config, &library);
            let message = match result {
                Ok(conflicts) => ConflictMessage::Completed { conflicts },
                Err(err) => ConflictMessage::Failed {
                    error: err.to_string(),
                },
            };
            let _ = tx.send(message);
        });
    }

    fn maybe_start_deploy(&mut self) {
        if !self.deploy_pending || self.deploy_active {
            return;
        }
        if self.import_active.is_some()
            || self.dialog.is_some()
            || self.pending_duplicate.is_some()
            || !self.duplicate_queue.is_empty()
        {
            return;
        }

        let healed = self.self_heal_missing_paks();
        if healed > 0 {
            self.log_warn(format!(
                "Self-heal: disabled missing pak(s) for {healed} mod(s)"
            ));
            self.set_toast(
                &format!("Self-heal: disabled {healed} missing mod(s)"),
                ToastLevel::Warn,
                Duration::from_secs(3),
            );
        }

        let reason = self
            .deploy_reason
            .take()
            .unwrap_or_else(|| "deploy".to_string());
        self.deploy_pending = false;
        self.deploy_active = true;
        let backup = self.deploy_backup;

        self.status = format!("Deploying ({reason})");
        self.log_info(format!("Deploy started ({reason})"));

        let tx = self.deploy_tx.clone();
        let config = self.config.clone();
        let mut library = self.library.clone();
        thread::spawn(move || {
            let result = deploy::deploy_with_options(
                &config,
                &mut library,
                deploy::DeployOptions {
                    backup,
                    reason: Some(reason.clone()),
                },
            );
            let message = match result {
                Ok(report) => DeployMessage::Completed { report },
                Err(err) => DeployMessage::Failed {
                    error: err.to_string(),
                },
            };
            let _ = tx.send(message);
        });
    }

    fn poll_deploys(&mut self) {
        loop {
            match self.deploy_rx.try_recv() {
                Ok(message) => self.handle_deploy_message(message),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
    }

    fn poll_conflicts(&mut self) {
        loop {
            match self.conflict_rx.try_recv() {
                Ok(message) => self.handle_conflict_message(message),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
    }

    fn handle_conflict_message(&mut self, message: ConflictMessage) {
        self.conflict_active = false;
        self.pending_override = None;
        match message {
            ConflictMessage::Completed { conflicts } => {
                let count = conflicts.len();
                self.conflicts = conflicts;
                if self.conflict_selected >= count {
                    self.conflict_selected = 0;
                }
                self.log_info(format!("Override scan complete: {count} override(s)"));
            }
            ConflictMessage::Failed { error } => {
                self.status = format!("Override scan failed: {error}");
                self.log_error(format!("Override scan failed: {error}"));
            }
        }
    }

    fn handle_deploy_message(&mut self, message: DeployMessage) {
        self.deploy_active = false;
        match message {
            DeployMessage::Completed { report } => {
                self.status = format!(
                    "Deployed: {} pak, {} loose | Files: {} | Overrides: {}",
                    report.pak_count,
                    report.loose_count,
                    report.file_count,
                    report.overridden_files
                );
                if report.removed_count > 0 {
                    self.log_info(format!(
                        "Cleanup: removed {} previous files",
                        report.removed_count
                    ));
                }
                self.log_info(format!(
                    "Deploy complete: {} pak, {} loose, {} files, {} overrides",
                    report.pak_count,
                    report.loose_count,
                    report.file_count,
                    report.overridden_files
                ));
                let _ = self.library.save(&self.config.data_dir);
            }
            DeployMessage::Failed { error } => {
                self.status = format!("Deploy failed: {error}");
                self.log_error(format!("Deploy failed: {error}"));
                self.set_toast("Deploy failed", ToastLevel::Error, Duration::from_secs(3));
            }
        }
        self.override_swap = None;

        if self.deploy_pending {
            self.maybe_start_deploy();
        }
    }

    pub fn clamp_selection(&mut self) {
        let len = self.visible_profile_indices().len();
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }

        let explorer_len = self.explorer_items().len();
        if explorer_len == 0 {
            self.explorer_selected = 0;
        } else if self.explorer_selected >= explorer_len {
            self.explorer_selected = explorer_len - 1;
        }

        let conflict_len = self.conflicts.len();
        if conflict_len == 0 {
            self.conflict_selected = 0;
        } else if self.conflict_selected >= conflict_len {
            self.conflict_selected = conflict_len - 1;
        }
    }
}

fn mod_matches_filter(mod_entry: &ModEntry, filter: &str) -> bool {
    let filter = filter.trim();
    if filter.is_empty() {
        return true;
    }
    let filter = filter.to_lowercase();
    let mut haystacks = Vec::new();
    haystacks.push(mod_entry.display_name());
    haystacks.push(mod_entry.name.clone());
    haystacks.push(mod_entry.id.clone());
    if let Some(source) = mod_entry.source_label() {
        haystacks.push(source.to_string());
    }
    if mod_entry.is_native() {
        haystacks.push("native".to_string());
        haystacks.push("mod.io".to_string());
    }
    haystacks
        .into_iter()
        .any(|value| value.to_lowercase().contains(&filter))
}

fn mod_sort_column_index(column: ModSortColumn) -> usize {
    MOD_SORT_COLUMNS
        .iter()
        .position(|col| *col == column)
        .unwrap_or(0)
}

fn mod_sort_next_column(column: ModSortColumn, direction: i32) -> ModSortColumn {
    let total = MOD_SORT_COLUMNS.len();
    if total == 0 {
        return column;
    }
    let current = mod_sort_column_index(column) as i32;
    let step = if direction >= 0 { 1 } else { -1 };
    let next = (current + step).rem_euclid(total as i32) as usize;
    MOD_SORT_COLUMNS.get(next).copied().unwrap_or(column)
}

fn sort_mod_indices(
    indices: &mut Vec<usize>,
    profile: &Profile,
    mod_map: &HashMap<String, ModEntry>,
    sort: ModSort,
) {
    if indices.len() < 2 {
        return;
    }
    indices.sort_by(|a, b| compare_mod_indices(*a, *b, profile, mod_map, sort));
}

fn compare_mod_indices(
    a_index: usize,
    b_index: usize,
    profile: &Profile,
    mod_map: &HashMap<String, ModEntry>,
    sort: ModSort,
) -> Ordering {
    let Some(a_entry) = profile.order.get(a_index) else {
        return Ordering::Greater;
    };
    let Some(b_entry) = profile.order.get(b_index) else {
        return Ordering::Less;
    };
    let Some(a_mod) = mod_map.get(&a_entry.id) else {
        return Ordering::Greater;
    };
    let Some(b_mod) = mod_map.get(&b_entry.id) else {
        return Ordering::Less;
    };

    let ordering = match sort.column {
        ModSortColumn::Order => compare_usize(a_index, b_index, sort.direction),
        ModSortColumn::Name => {
            compare_string(&a_mod.display_name(), &b_mod.display_name(), sort.direction)
        }
        ModSortColumn::Enabled => compare_bool(a_entry.enabled, b_entry.enabled, sort.direction),
        ModSortColumn::Native => compare_bool(a_mod.is_native(), b_mod.is_native(), sort.direction),
        ModSortColumn::Kind => {
            compare_string(mod_kind_label(a_mod), mod_kind_label(b_mod), sort.direction)
        }
        ModSortColumn::Target => compare_string(
            &mod_target_sort_label(a_mod),
            &mod_target_sort_label(b_mod),
            sort.direction,
        ),
        ModSortColumn::Created => {
            compare_option_i64(a_mod.created_at, b_mod.created_at, sort.direction)
        }
        ModSortColumn::Added => compare_i64(a_mod.added_at, b_mod.added_at, sort.direction),
    };

    if ordering == Ordering::Equal {
        a_index.cmp(&b_index)
    } else {
        ordering
    }
}

fn compare_usize(a: usize, b: usize, direction: SortDirection) -> Ordering {
    match direction {
        SortDirection::Asc => a.cmp(&b),
        SortDirection::Desc => b.cmp(&a),
    }
}

fn compare_i64(a: i64, b: i64, direction: SortDirection) -> Ordering {
    match direction {
        SortDirection::Asc => a.cmp(&b),
        SortDirection::Desc => b.cmp(&a),
    }
}

fn compare_option_i64(a: Option<i64>, b: Option<i64>, direction: SortDirection) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => compare_i64(a, b, direction),
    }
}

fn compare_bool(a: bool, b: bool, direction: SortDirection) -> Ordering {
    let a = a as u8;
    let b = b as u8;
    match direction {
        SortDirection::Asc => a.cmp(&b),
        SortDirection::Desc => b.cmp(&a),
    }
}

fn compare_string(a: &str, b: &str, direction: SortDirection) -> Ordering {
    let a = a.to_ascii_lowercase();
    let b = b.to_ascii_lowercase();
    match direction {
        SortDirection::Asc => a.cmp(&b),
        SortDirection::Desc => b.cmp(&a),
    }
}

fn mod_kind_label(mod_entry: &ModEntry) -> &'static str {
    let mut has_pak = false;
    let mut has_loose = false;

    for target in &mod_entry.targets {
        match target {
            InstallTarget::Pak { .. } => has_pak = true,
            _ => has_loose = true,
        }
    }

    match (has_pak, has_loose) {
        (true, true) => "Mixed",
        (true, false) => "Pak",
        (false, true) => "Loose",
        _ => "Unknown",
    }
}

fn mod_target_sort_label(mod_entry: &ModEntry) -> String {
    if mod_entry.targets.is_empty() {
        return "Invalid".to_string();
    }

    let mut kinds = Vec::new();
    for target in &mod_entry.targets {
        let kind = target.kind();
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }

    let enabled: Vec<TargetKind> = kinds
        .iter()
        .copied()
        .filter(|kind| mod_entry.is_target_enabled(*kind))
        .collect();
    let has_overrides = !mod_entry.target_overrides.is_empty();

    let base_label = if has_overrides {
        if enabled.len() == 1 {
            target_kind_label(enabled[0]).to_string()
        } else if enabled.is_empty() {
            "None".to_string()
        } else {
            "Custom".to_string()
        }
    } else {
        "Auto".to_string()
    };

    let kind_for_path = if enabled.len() == 1 {
        Some(enabled[0])
    } else if !has_overrides && kinds.len() == 1 {
        Some(kinds[0])
    } else {
        None
    };

    if let Some(kind) = kind_for_path {
        format!("{base_label} {}", target_kind_label(kind))
    } else if kinds.len() > 1 {
        format!("{base_label} Multiple")
    } else {
        base_label
    }
}

fn target_kind_label(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Pak => "Mods",
        TargetKind::Generated => "Generated",
        TargetKind::Data => "Data",
        TargetKind::Bin => "Bin",
    }
}

const LOG_CAPACITY: usize = 200;

pub(crate) fn expand_tilde(input: &str) -> PathBuf {
    let mut value = input.trim().to_string();
    value = strip_outer_quotes(&value);
    if let Some(rest) = value.strip_prefix("file://") {
        value = rest.trim_start_matches("localhost/").to_string();
    }
    if value.contains('\\') {
        value = unescape_shell(&value);
    }
    if value.contains('%') {
        value = percent_decode(&value);
    }
    if let Some(stripped) = value.strip_prefix('~') {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(stripped.trim_start_matches('/'));
        }
    }
    PathBuf::from(value)
}

fn strip_outer_quotes(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        if (bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'')
        {
            return value[1..bytes.len() - 1].to_string();
        }
    }
    value.to_string()
}

fn unescape_shell(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn percent_decode(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.as_bytes().iter().copied().peekable();

    while let Some(byte) = chars.next() {
        if byte == b'%' {
            let hi = chars.next();
            let lo = chars.next();
            if let (Some(hi), Some(lo)) = (hi, lo) {
                if let (Some(hi), Some(lo)) = (from_hex(hi), from_hex(lo)) {
                    out.push((hi << 4 | lo) as char);
                    continue;
                }
            }
            out.push('%');
            if let Some(hi) = hi {
                out.push(hi as char);
            }
            if let Some(lo) = lo {
                out.push(lo as char);
            }
        } else {
            out.push(byte as char);
        }
    }

    out
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn display_path(path: &PathBuf) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_string())
        .unwrap_or_else(|| path.display().to_string())
}

fn duplicate_default_overwrite(new_mod: &ModEntry, existing: &ModEntry) -> Option<bool> {
    if let (Some(new_version), Some(existing_version)) =
        (mod_version_stamp(new_mod), mod_version_stamp(existing))
    {
        if new_version != existing_version {
            return Some(new_version > existing_version);
        }
    }
    let new_stamp = App::mod_stamp(new_mod)?;
    let existing_stamp = App::mod_stamp(existing)?;
    Some(new_stamp > existing_stamp)
}

#[derive(Debug, Clone, Copy)]
enum CliDuplicateAction {
    Overwrite,
    Skip,
    OverwriteAll,
    SkipAll,
}

struct CliProgressPrinter {
    verbosity: CliVerbosity,
    last_label: Option<String>,
    last_stage: Option<importer::ImportStage>,
    last_tick: Instant,
}

impl CliProgressPrinter {
    fn new(verbosity: CliVerbosity) -> Self {
        Self {
            verbosity,
            last_label: None,
            last_stage: None,
            last_tick: Instant::now(),
        }
    }

    fn handle(&mut self, progress: &importer::ImportProgress) {
        if matches!(self.verbosity, CliVerbosity::Quiet | CliVerbosity::Normal) {
            return;
        }
        let label_changed = self.last_label.as_deref() != Some(progress.label.as_str());
        if label_changed {
            println!(
                "  -> {} ({}/{})",
                progress.label, progress.unit_index, progress.unit_count
            );
            self.last_label = Some(progress.label.clone());
            self.last_stage = None;
        }

        let stage_changed = self.last_stage != Some(progress.stage);
        let should_tick = self.last_tick.elapsed().as_millis() >= 250;
        if stage_changed || (matches!(self.verbosity, CliVerbosity::Debug) && should_tick) {
            let mut line = format!("     {}", progress.stage.label());
            if progress.stage_total > 1 {
                line.push_str(&format!(
                    " ({}/{})",
                    progress.stage_current, progress.stage_total
                ));
            }
            if let Some(detail) = &progress.detail {
                line.push_str(&format!(" - {}", detail));
            }
            println!("{line}");
            self.last_stage = Some(progress.stage);
            self.last_tick = Instant::now();
        }
    }
}

fn prompt_duplicate_cli(
    new_mod: &ModEntry,
    existing: &ModEntry,
    default_overwrite: Option<bool>,
    similarity: Option<f32>,
) -> Result<CliDuplicateAction> {
    println!();
    println!("Duplicate mod detected:");
    println!("  New: {}", new_mod.display_name());
    println!("  Existing: {}", existing.display_name());
    if let Some(similarity) = similarity {
        println!("  Similarity: {:.0}%", similarity * 100.0);
    }
    if let Some(default_overwrite) = default_overwrite {
        let hint = if default_overwrite {
            "overwrite (newer)"
        } else {
            "skip (existing newer)"
        };
        println!("  Default: {}", hint);
    }
    print!("Choose [o]verwrite, [s]kip, overwrite [a]ll, skip all [k] (Enter = default): ");
    io::stdout().flush().ok();
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let choice = input.trim().to_lowercase();
    if choice.is_empty() {
        return Ok(if default_overwrite.unwrap_or(false) {
            CliDuplicateAction::Overwrite
        } else {
            CliDuplicateAction::Skip
        });
    }
    match choice.as_str() {
        "o" | "y" | "yes" => Ok(CliDuplicateAction::Overwrite),
        "s" | "n" | "no" => Ok(CliDuplicateAction::Skip),
        "a" | "all" => Ok(CliDuplicateAction::OverwriteAll),
        "k" | "skipall" | "skip-all" => Ok(CliDuplicateAction::SkipAll),
        _ => Ok(CliDuplicateAction::Skip),
    }
}

fn summarize_error(error: &str) -> String {
    let first_line = error.lines().next().unwrap_or(error).trim();
    let last = first_line.rsplit(": ").next().unwrap_or(first_line).trim();
    let lower = last.to_lowercase();

    if lower.contains("device or resource busy") || lower.contains("text file busy") {
        return "file in use".to_string();
    }
    if lower.contains("permission denied") || lower.contains("access is denied") {
        return "permission denied".to_string();
    }
    if lower.contains("no such file or directory") || lower.contains("file not found") {
        return "file not found".to_string();
    }
    if lower.contains("is a directory") {
        return "expected a file".to_string();
    }
    if lower.contains("not a directory") {
        return "expected a folder".to_string();
    }
    if lower.contains("missing meta.lsx") || lower.contains("meta.lsx") {
        return "mod metadata missing".to_string();
    }

    last.to_string()
}

fn build_dependent_message(dependents: &[DependentMod]) -> String {
    if dependents.is_empty() {
        return String::new();
    }
    let mut lines = Vec::new();
    lines.push("This mod is required by:".to_string());
    let max_list = 6usize;
    for dependent in dependents.iter().take(max_list) {
        lines.push(format!("- {}", dependent.name));
    }
    if dependents.len() > max_list {
        lines.push(format!("...and {} more", dependents.len() - max_list));
    }
    lines.push(String::new());
    lines.push("Removing it will disable those mods.".to_string());
    lines.join("\n")
}

fn dependency_display_label(value: &str) -> String {
    let uuid = dependency_uuid(value);
    let mut base = value.to_string();
    if let Some(uuid) = uuid.as_deref() {
        if let Some(index) = base.find(uuid) {
            base.truncate(index);
        }
    }
    let cleaned = base
        .replace('_', " ")
        .replace('-', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let cleaned = cleaned
        .trim_matches(|ch: char| ch == '.' || ch == '_')
        .to_string();
    if cleaned.is_empty() {
        "Unknown dependency".to_string()
    } else {
        cleaned
    }
}

fn dependency_uuid(value: &str) -> Option<String> {
    extract_uuid_candidates(value).into_iter().next()
}

fn dependency_signature(display_label: &str, uuid: &Option<String>, raw: &str) -> String {
    if let Some(uuid) = uuid.as_ref() {
        return uuid.clone();
    }
    let normalized = normalize_label(display_label);
    if !normalized.is_empty() && normalized != "unknowndependency" {
        return normalized;
    }
    normalize_label(raw)
}

fn override_dependency_item() -> DependencyItem {
    DependencyItem {
        label: "override".to_string(),
        display_label: "Override dependencies".to_string(),
        uuid: None,
        required_by: Vec::new(),
        status: DependencyStatus::Skipped,
        link: None,
        search_link: None,
        search_label: String::new(),
        kind: DependencyItemKind::OverrideAction,
    }
}

fn dependency_search_label(display_label: &str, uuid: &Option<String>, raw: &str) -> String {
    if display_label != "Unknown dependency" {
        return display_label.to_string();
    }
    if let Some(uuid) = uuid.as_ref() {
        return format!("bg3 mod {uuid}");
    }
    format!("bg3 mod {raw}")
}

fn dependency_search_link(query: &str) -> Option<String> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    let encoded = encode_query(query);
    let lower = query.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("bg3 mod ") {
        if is_uuid_like(rest.trim()) {
            return Some(format!("https://duckduckgo.com/?q={encoded}"));
        }
    }
    Some(format!(
        "https://www.nexusmods.com/baldursgate3/search/?gsearch={encoded}&gsearchtype=mods"
    ))
}

fn resolved_dependency_ids(
    lookup: &DependencyLookup,
    dependency: &str,
    mod_entry: &ModEntry,
) -> Vec<String> {
    let mut ids = lookup.resolve_ids(dependency);
    if ids.is_empty() {
        return ids;
    }
    if dependency_is_self_alias(dependency, mod_entry, &ids) {
        return ids;
    }
    let dep_lower = dependency.to_ascii_lowercase();
    let self_id = mod_entry.id.to_ascii_lowercase();
    if !dep_lower.contains(&self_id) {
        ids.retain(|id| id != &mod_entry.id);
    }
    ids
}

fn is_unverified_dependency(dep: &str) -> bool {
    if dep.starts_with('_') {
        return true;
    }
    is_uuid_like(dep) && dependency_display_label(dep) == "Unknown dependency"
}

fn dependency_is_self_alias(
    dependency: &str,
    mod_entry: &ModEntry,
    resolved_ids: &[String],
) -> bool {
    if resolved_ids.len() != 1 || resolved_ids[0] != mod_entry.id {
        return false;
    }
    if dependency.starts_with('_') {
        return true;
    }
    let dep_lower = dependency.to_ascii_lowercase();
    dep_lower.contains(&mod_entry.id.to_ascii_lowercase())
}

fn filter_ignored_dependencies(deps: &mut Vec<String>) {
    deps.retain(|dep| {
        if metadata::is_base_dependency_uuid(dep) || metadata::is_base_dependency_label(dep) {
            return false;
        }
        if let Some(uuid) = dependency_uuid(dep) {
            if metadata::is_base_dependency_uuid(&uuid) {
                return false;
            }
        }
        let display = dependency_display_label(dep);
        if metadata::is_base_dependency_label(&display) {
            return false;
        }
        true
    });
}

fn encode_query(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else if ch == ' ' {
            out.push('+');
        } else {
            let mut buf = [0u8; 4];
            for byte in ch.encode_utf8(&mut buf).as_bytes() {
                out.push('%');
                out.push_str(&format!("{:02X}", byte));
            }
        }
    }
    out
}

fn dependency_match_keys(value: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let normalized = normalize_label(value);
    if !normalized.is_empty() {
        keys.push(normalized);
    }
    for candidate in extract_uuid_candidates(value) {
        let normalized = normalize_label(&candidate);
        if !normalized.is_empty() {
            keys.push(normalized);
        }
    }
    for token in value.split(|ch: char| !ch.is_ascii_alphanumeric()) {
        let normalized = normalize_label(token);
        if !normalized.is_empty() {
            keys.push(normalized);
        }
    }
    keys.sort();
    keys.dedup();
    keys
}

fn extract_uuid_candidates(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in value.chars() {
        if ch.is_ascii_hexdigit() || ch == '-' {
            current.push(ch);
        } else {
            if is_uuid_like(&current) {
                out.push(current.clone());
            }
            current.clear();
        }
    }
    if is_uuid_like(&current) {
        out.push(current);
    }
    out
}

fn is_uuid_like(value: &str) -> bool {
    if value.len() != 36 {
        return false;
    }
    let bytes = value.as_bytes();
    for (idx, byte) in bytes.iter().enumerate() {
        match idx {
            8 | 13 | 18 | 23 => {
                if *byte != b'-' {
                    return false;
                }
            }
            _ => {
                let ch = *byte as char;
                if !ch.is_ascii_hexdigit() {
                    return false;
                }
            }
        }
    }
    true
}

fn mod_dependency_keys(mod_entry: &ModEntry) -> Vec<String> {
    let mut keys = Vec::new();
    let mut push_key = |value: &str| {
        let key = normalize_label(value);
        if !key.is_empty() {
            keys.push(key);
        }
    };

    push_key(&mod_entry.id);
    push_key(&mod_entry.name);
    for token in mod_entry.name.split(|ch: char| !ch.is_ascii_alphanumeric()) {
        if token.len() >= 4 {
            push_key(token);
        }
    }
    push_key(&mod_entry.display_name());
    for token in mod_entry
        .display_name()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
    {
        if token.len() >= 4 {
            push_key(token);
        }
    }
    if let Some(label) = mod_entry.source_label() {
        push_key(label);
        for token in label.split(|ch: char| !ch.is_ascii_alphanumeric()) {
            if token.len() >= 4 {
                push_key(token);
            }
        }
    }
    for target in &mod_entry.targets {
        if let InstallTarget::Pak { file, info } = target {
            push_key(file);
            for token in file.split(|ch: char| !ch.is_ascii_alphanumeric()) {
                if token.len() >= 4 {
                    push_key(token);
                }
            }
            push_key(&info.uuid);
            push_key(&info.folder);
            for token in info.folder.split(|ch: char| !ch.is_ascii_alphanumeric()) {
                if token.len() >= 4 {
                    push_key(token);
                }
            }
            push_key(&info.name);
            for token in info.name.split(|ch: char| !ch.is_ascii_alphanumeric()) {
                if token.len() >= 4 {
                    push_key(token);
                }
            }
        }
    }
    keys.sort();
    keys.dedup();
    keys
}

fn similarity_ratio(a: &str, b: &str) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let max_len = a.len().max(b.len());
    if max_len == 0 {
        return 1.0;
    }
    let distance = levenshtein(a, b);
    1.0 - (distance as f32 / max_len as f32)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    if a_bytes.is_empty() {
        return b_bytes.len();
    }
    if b_bytes.is_empty() {
        return a_bytes.len();
    }

    let mut prev: Vec<usize> = (0..=b_bytes.len()).collect();
    let mut curr = vec![0; b_bytes.len() + 1];

    for (i, a_ch) in a_bytes.iter().enumerate() {
        curr[0] = i + 1;
        for (j, b_ch) in b_bytes.iter().enumerate() {
            let cost = if a_ch == b_ch { 0 } else { 1 };
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        prev.clone_from_slice(&curr);
    }

    prev[b_bytes.len()]
}

fn extract_timestamp(label: &str) -> Option<u64> {
    let mut best: Option<u64> = None;
    let mut current = String::new();

    for ch in label.chars() {
        if ch.is_ascii_digit() {
            current.push(ch);
        } else {
            if current.len() >= 8 {
                if let Ok(value) = current.parse::<u64>() {
                    best = Some(value);
                }
            }
            current.clear();
        }
    }

    if current.len() >= 8 {
        if let Ok(value) = current.parse::<u64>() {
            best = Some(value);
        }
    }

    best
}

fn mod_version_stamp(entry: &ModEntry) -> Option<u64> {
    let label = entry.source_label().unwrap_or(entry.name.as_str());
    if let Some((major, minor, patch, build)) = extract_semver(label) {
        return Some(semver_stamp(major, minor, patch, build));
    }

    let mut best: Option<u64> = None;
    for target in &entry.targets {
        if let InstallTarget::Pak { info, .. } = target {
            if info.version > 0 {
                best = Some(best.map_or(info.version, |current| current.max(info.version)));
            }
        }
    }
    best
}

fn extract_semver(label: &str) -> Option<(u64, u64, u64, u64)> {
    let bytes = label.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        let start = index;
        let (major, next) = parse_number(bytes, index)?;
        index = next;
        if index >= bytes.len() || bytes[index] != b'.' {
            index = start + 1;
            continue;
        }
        index += 1;
        let (minor, next) = parse_number(bytes, index)?;
        index = next;
        let mut patch = 0;
        let mut build = 0;
        if index < bytes.len() && bytes[index] == b'.' {
            index += 1;
            if let Some((value, next)) = parse_number(bytes, index) {
                patch = value;
                index = next;
                if index < bytes.len() && bytes[index] == b'.' {
                    index += 1;
                    if let Some((value, next)) = parse_number(bytes, index) {
                        build = value;
                        let _ = next;
                    }
                }
            }
        }
        return Some((major, minor, patch, build));
    }
    None
}

fn parse_number(bytes: &[u8], mut index: usize) -> Option<(u64, usize)> {
    if index >= bytes.len() || !bytes[index].is_ascii_digit() {
        return None;
    }
    let mut value = 0u64;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        value = value
            .saturating_mul(10)
            .saturating_add((bytes[index] - b'0') as u64);
        index += 1;
    }
    Some((value, index))
}

fn semver_stamp(major: u64, minor: u64, patch: u64, build: u64) -> u64 {
    major
        .saturating_mul(1_000_000_000)
        .saturating_add(minor.saturating_mul(1_000_000))
        .saturating_add(patch.saturating_mul(1_000))
        .saturating_add(build)
}

fn append_log_file(path: &PathBuf, level: LogLevel, message: &str) -> std::io::Result<()> {
    let label = match level {
        LogLevel::Info => "INFO",
        LogLevel::Warn => "WARN",
        LogLevel::Error => "ERROR",
    };
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "[{label}] {message}")
}

fn build_unknown_entry(path: &PathBuf, label: &str) -> ModEntry {
    let (raw_created, raw_modified) = path_times(path);
    let (created_at, modified_at) = normalize_times(raw_created, raw_modified);
    ModEntry {
        id: unknown_id(path),
        name: label.to_string(),
        created_at,
        modified_at,
        added_at: now_timestamp(),
        targets: Vec::new(),
        target_overrides: Vec::new(),
        source_label: Some(label.to_string()),
        source: ModSource::Managed,
        dependencies: Vec::new(),
    }
}

fn unknown_id(path: &PathBuf) -> String {
    let mut hasher = Hasher::new();
    hasher.update(path.to_string_lossy().as_bytes());
    if let Ok(meta) = fs::metadata(path) {
        if let Ok(modified) = meta.modified() {
            if let Ok(duration) = modified.duration_since(UNIX_EPOCH) {
                hasher.update(&duration.as_secs().to_le_bytes());
            }
        }
    }
    format!("unknown-{}", hasher.finalize().to_hex())
}

fn set_target_override(mod_entry: &mut ModEntry, kind: TargetKind, enabled: bool) -> bool {
    if let Some(override_entry) = mod_entry
        .target_overrides
        .iter_mut()
        .find(|entry| entry.kind == kind)
    {
        if override_entry.enabled != enabled {
            override_entry.enabled = enabled;
            return true;
        }
        return false;
    }
    mod_entry
        .target_overrides
        .push(TargetOverride { kind, enabled });
    true
}

fn earliest_timestamp(values: &[Option<i64>]) -> Option<i64> {
    let mut out: Option<i64> = None;
    for value in values.iter().copied().flatten() {
        out = Some(match out {
            Some(current) => current.min(value),
            None => value,
        });
    }
    out
}

fn resolve_native_times(
    primary_created: Option<i64>,
    file_created: Option<i64>,
    file_modified: Option<i64>,
) -> (Option<i64>, Option<i64>) {
    if primary_created.is_some() {
        return resolve_times(primary_created, file_created, file_modified);
    }
    let modified = file_modified.or(file_created);
    (None, modified)
}

fn should_clear_native_created(
    current_created: Option<i64>,
    file_created: Option<i64>,
    file_modified: Option<i64>,
    added_at: i64,
) -> bool {
    let Some(current) = current_created else {
        return false;
    };
    if current == added_at {
        return true;
    }
    file_created.map_or(false, |value| value == current)
        || file_modified.map_or(false, |value| value == current)
}

fn collect_metadata_updates(
    game_id: GameId,
    config: &GameConfig,
    library: &Library,
    progress: Option<&Sender<MetadataMessage>>,
) -> Result<Vec<MetadataUpdate>> {
    let paths = game::detect_paths(game_id, Some(&config.game_root), Some(&config.larian_dir)).ok();
    let native_index = paths
        .as_ref()
        .map(|paths| native_pak::build_native_pak_index(&paths.larian_mods_dir));

    let mut updates = Vec::new();
    let total = library.mods.len();
    for (index, mod_entry) in library.mods.iter().enumerate() {
        let should_refresh_created =
            mod_entry.created_at.is_none() || mod_entry.created_at == Some(mod_entry.added_at);
        let should_refresh_modified = mod_entry.modified_at.is_none()
            || (mod_entry.is_native()
                && mod_entry.created_at.is_some()
                && mod_entry.created_at == mod_entry.modified_at);

        let mut meta_created: Option<i64> = None;
        let mut json_created: Option<i64> = None;
        let mut file_created: Option<i64> = None;
        let mut file_modified: Option<i64> = None;
        let mut dependencies: Vec<String> = Vec::new();

        for pak_path in resolve_pak_paths(
            mod_entry,
            &config.data_dir,
            paths.as_ref(),
            native_index.as_deref(),
        ) {
            if let Some(meta) = metadata::read_meta_lsx_from_pak(&pak_path) {
                if let Some(created) = meta.created_at {
                    meta_created = Some(match meta_created {
                        Some(existing) => existing.min(created),
                        None => created,
                    });
                }
                if !meta.dependencies.is_empty() {
                    dependencies.extend(meta.dependencies);
                }
            }
            let (raw_created, raw_modified) = path_times(&pak_path);
            if let Some(created) = raw_created {
                file_created = Some(match file_created {
                    Some(existing) => existing.min(created),
                    None => created,
                });
            }
            if let Some(modified) = raw_modified {
                file_modified = Some(match file_modified {
                    Some(existing) => existing.max(modified),
                    None => modified,
                });
            }
        }

        let mod_root = library_mod_root(&config.data_dir).join(&mod_entry.id);
        if mod_root.exists() {
            if let Some(meta_path) = metadata::find_meta_lsx(&mod_root) {
                if let Some(meta) = metadata::read_meta_lsx(&meta_path) {
                    if let Some(created) = meta.created_at {
                        meta_created = Some(match meta_created {
                            Some(existing) => existing.min(created),
                            None => created,
                        });
                    }
                    if !meta.dependencies.is_empty() {
                        dependencies.extend(meta.dependencies);
                    }
                }
            }
            if let Some(info_path) = metadata::find_info_json(&mod_root) {
                let json_mods = metadata::read_json_mods(&info_path);
                if let Some(created) = json_mods.iter().filter_map(|info| info.created_at).min() {
                    json_created = Some(match json_created {
                        Some(existing) => existing.min(created),
                        None => created,
                    });
                }
                for info in &json_mods {
                    if !info.dependencies.is_empty() {
                        dependencies.extend(info.dependencies.clone());
                    }
                }
            }
            let (raw_created, raw_modified) = scan_mod_targets_times(mod_entry, &mod_root);
            if let Some(created) = raw_created {
                file_created = Some(match file_created {
                    Some(existing) => existing.min(created),
                    None => created,
                });
            }
            if let Some(modified) = raw_modified {
                file_modified = Some(match file_modified {
                    Some(existing) => existing.max(modified),
                    None => modified,
                });
            }
        }

        dependencies.sort();
        dependencies.dedup();
        dependencies.retain(|dep| !dep.eq_ignore_ascii_case(&mod_entry.id));

        let (primary_created, created_candidate, modified_candidate, should_clear_created) =
            if mod_entry.is_native() {
                let primary_created = earliest_timestamp(&[meta_created]);
                let (created_candidate, modified_candidate) =
                    resolve_native_times(primary_created, file_created, file_modified);
                let should_clear_created = primary_created.is_none()
                    && should_clear_native_created(
                        mod_entry.created_at,
                        file_created,
                        file_modified,
                        mod_entry.added_at,
                    );
                (
                    primary_created,
                    created_candidate,
                    modified_candidate,
                    should_clear_created,
                )
            } else {
                let primary_created = json_created.or(meta_created);
                let (created_candidate, modified_candidate) =
                    resolve_times(primary_created, file_created, file_modified);
                (
                    primary_created,
                    created_candidate,
                    modified_candidate,
                    false,
                )
            };

        let should_update_created = if mod_entry.is_native() {
            (created_candidate.is_some() && mod_entry.created_at != created_candidate)
                || should_clear_created
        } else {
            primary_created.is_some() || should_refresh_created
        };
        let mut next_created = mod_entry.created_at;
        let mut next_modified = mod_entry.modified_at;

        if should_update_created {
            if let Some(created) = created_candidate {
                next_created = Some(created);
            } else if should_clear_created {
                next_created = None;
            }
        }

        if let Some(modified) = modified_candidate {
            if should_refresh_modified
                || next_modified.map(|value| value < modified).unwrap_or(true)
            {
                next_modified = Some(modified);
            }
        }

        let update = MetadataUpdate {
            id: mod_entry.id.clone(),
            created_at: next_created,
            modified_at: next_modified,
            dependencies,
        };
        if let Some(tx) = progress {
            let _ = tx.send(MetadataMessage::Progress {
                update: update.clone(),
                current: index + 1,
                total,
            });
        }
        updates.push(update);
    }

    Ok(updates)
}

fn modsettings_fingerprint(snapshot: &deploy::ModSettingsSnapshot) -> String {
    let mut hasher = Hasher::new();
    hasher.update(b"modsettings-v1");
    let mut module_ids: Vec<&str> = snapshot
        .modules
        .iter()
        .map(|module| module.info.uuid.as_str())
        .collect();
    module_ids.sort();
    for id in module_ids {
        hasher.update(id.as_bytes());
    }
    hasher.update(b"|order|");
    for id in &snapshot.order {
        hasher.update(id.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

fn sync_native_mods_delta(
    game_id: GameId,
    config: &GameConfig,
    library: &Library,
    progress: Option<&Sender<NativeSyncMessage>>,
) -> Result<NativeSyncDelta, String> {
    let paths = game::detect_paths(game_id, Some(&config.game_root), Some(&config.larian_dir))
        .map_err(|err| err.to_string())?;
    let modsettings_exists = paths.modsettings_path.exists();
    let native_pak_index = native_pak::build_native_pak_index(&paths.larian_mods_dir);

    let snapshot = deploy::read_modsettings_snapshot(&paths.modsettings_path)
        .map_err(|err| err.to_string())?;
    let deploy::ModSettingsSnapshot { modules, order } = snapshot;
    let modsettings_hash = if modsettings_exists {
        Some(modsettings_fingerprint(&deploy::ModSettingsSnapshot {
            modules: modules.clone(),
            order: order.clone(),
        }))
    } else {
        None
    };
    let modules_set: HashSet<String> = modules
        .iter()
        .map(|module| module.info.uuid.clone())
        .collect();
    let order_set: HashSet<String> = order.iter().cloned().collect();
    let enabled_set: HashSet<String> = if order.is_empty() {
        modules_set.clone()
    } else {
        order_set
    };
    let module_created_by_uuid: HashMap<String, Option<i64>> = modules
        .iter()
        .map(|module| (module.info.uuid.clone(), module.created_at))
        .collect();

    let mut existing_ids: HashSet<String> =
        library.mods.iter().map(|entry| entry.id.clone()).collect();
    let mut modules_by_uuid: HashMap<String, deploy::ModSettingsModule> = modules
        .into_iter()
        .map(|module| (module.info.uuid.clone(), module))
        .collect();

    let mods_root = config.data_dir.join("mods");
    let mut updates = Vec::new();
    let mut updated_native_files = 0usize;

    let native_mods: Vec<&ModEntry> = library
        .mods
        .iter()
        .filter(|entry| entry.is_native())
        .collect();
    let total_native = native_mods.len();
    for (index, mod_entry) in native_mods.iter().enumerate() {
        if let Some(tx) = progress {
            let _ = tx.send(NativeSyncMessage::Progress(NativeSyncProgress {
                stage: NativeSyncStage::NativeFiles,
                current: index + 1,
                total: total_native,
            }));
        }

        let Some(info) = mod_entry.targets.iter().find_map(|target| match target {
            InstallTarget::Pak { info, .. } => Some(info.clone()),
            _ => None,
        }) else {
            continue;
        };
        let Some(filename) = native_pak::resolve_native_pak_filename(&info, &native_pak_index)
        else {
            continue;
        };
        let mut targets = mod_entry.targets.clone();
        let mut changed = false;
        for target in &mut targets {
            if let InstallTarget::Pak { file, .. } = target {
                if *file != filename {
                    *file = filename.clone();
                    changed = true;
                }
            }
        }

        let pak_path = paths.larian_mods_dir.join(&filename);
        let modsettings_created = module_created_by_uuid.get(&mod_entry.id).copied().flatten();
        let pak_meta = metadata::read_meta_lsx_from_pak(&pak_path);
        let meta_created = pak_meta.as_ref().and_then(|meta| meta.created_at);
        let mut dependencies = pak_meta
            .as_ref()
            .map(|meta| meta.dependencies.clone())
            .unwrap_or_default();
        dependencies.sort();
        dependencies.dedup();
        dependencies.retain(|dep| !dep.eq_ignore_ascii_case(&mod_entry.id));
        let (raw_created, raw_modified) = path_times(&pak_path);
        let primary_created = earliest_timestamp(&[modsettings_created, meta_created]);
        let (created_at, modified_at) =
            resolve_native_times(primary_created, raw_created, raw_modified);

        let mut next_created = mod_entry.created_at;
        let mut next_modified = mod_entry.modified_at;
        if primary_created.is_some() {
            if created_at.is_some() && mod_entry.created_at != created_at {
                next_created = created_at;
            }
        } else if should_clear_native_created(
            mod_entry.created_at,
            raw_created,
            raw_modified,
            mod_entry.added_at,
        ) {
            next_created = None;
        }
        if let Some(modified_at) = modified_at {
            if mod_entry.modified_at.is_none()
                || mod_entry
                    .modified_at
                    .map(|value| value < modified_at)
                    .unwrap_or(true)
            {
                next_modified = Some(modified_at);
            }
        }

        if changed {
            updated_native_files += 1;
        }

        updates.push(NativeModUpdate {
            id: mod_entry.id.clone(),
            source: mod_entry.source,
            name: mod_entry.name.clone(),
            source_label: mod_entry.source_label.clone(),
            targets,
            created_at: next_created,
            modified_at: next_modified,
            dependencies,
        });
    }

    let mut adopted_native = 0usize;
    let non_native_mods: Vec<&ModEntry> = library
        .mods
        .iter()
        .filter(|entry| !entry.is_native())
        .collect();
    let total_adopt = non_native_mods.len();
    for (index, mod_entry) in non_native_mods.iter().enumerate() {
        if let Some(tx) = progress {
            let _ = tx.send(NativeSyncMessage::Progress(NativeSyncProgress {
                stage: NativeSyncStage::AdoptNative,
                current: index + 1,
                total: total_adopt,
            }));
        }
        if mod_entry.source == ModSource::Managed {
            continue;
        }
        if !modules_set.contains(&mod_entry.id) {
            continue;
        }
        if mods_root.join(&mod_entry.id).exists() {
            continue;
        }
        let Some(module) = modules_by_uuid.get(&mod_entry.id) else {
            continue;
        };
        let info = &module.info;
        let modsettings_created = module.created_at;
        let filename = native_pak::resolve_native_pak_filename(info, &native_pak_index)
            .unwrap_or_else(|| format!("{}.pak", info.folder));
        let pak_path = paths.larian_mods_dir.join(&filename);
        let pak_meta = metadata::read_meta_lsx_from_pak(&pak_path);
        let meta_created = pak_meta.as_ref().and_then(|meta| meta.created_at);
        let mut dependencies = pak_meta
            .as_ref()
            .map(|meta| meta.dependencies.clone())
            .unwrap_or_default();
        dependencies.sort();
        dependencies.dedup();
        dependencies.retain(|dep| !dep.eq_ignore_ascii_case(&mod_entry.id));
        let (raw_created, raw_modified) = path_times(&pak_path);
        let primary_created = earliest_timestamp(&[modsettings_created, meta_created]);
        let (created_at, modified_at) =
            resolve_native_times(primary_created, raw_created, raw_modified);

        let mut next_created = mod_entry.created_at;
        let mut next_modified = mod_entry.modified_at;
        if primary_created.is_some() {
            next_created = created_at;
        }
        if let Some(modified_at) = modified_at {
            next_modified = Some(modified_at);
        }

        updates.push(NativeModUpdate {
            id: mod_entry.id.clone(),
            source: ModSource::Native,
            name: info.name.clone(),
            source_label: None,
            targets: vec![InstallTarget::Pak {
                file: filename.clone(),
                info: info.clone(),
            }],
            created_at: next_created,
            modified_at: next_modified,
            dependencies,
        });
        adopted_native += 1;
    }

    let mut ordered = Vec::new();
    for uuid in &order {
        if let Some(module) = modules_by_uuid.remove(uuid) {
            ordered.push(module);
        }
    }
    ordered.extend(modules_by_uuid.into_values());

    let mut added = Vec::new();
    let total_add = ordered.len();
    for (index, module) in ordered.into_iter().enumerate() {
        if let Some(tx) = progress {
            let _ = tx.send(NativeSyncMessage::Progress(NativeSyncProgress {
                stage: NativeSyncStage::AddMissing,
                current: index + 1,
                total: total_add,
            }));
        }
        let info = module.info;
        let modsettings_created = module.created_at;
        let uuid = info.uuid.clone();
        if existing_ids.contains(&uuid) {
            continue;
        }
        let filename = native_pak::resolve_native_pak_filename(&info, &native_pak_index)
            .unwrap_or_else(|| format!("{}.pak", info.folder));
        let pak_path = paths.larian_mods_dir.join(&filename);
        let pak_meta = metadata::read_meta_lsx_from_pak(&pak_path);
        let meta_created = pak_meta.as_ref().and_then(|meta| meta.created_at);
        let mut dependencies = pak_meta
            .as_ref()
            .map(|meta| meta.dependencies.clone())
            .unwrap_or_default();
        dependencies.sort();
        dependencies.dedup();
        dependencies.retain(|dep| !dep.eq_ignore_ascii_case(&uuid));
        let (raw_created, raw_modified) = path_times(&pak_path);
        let primary_created = earliest_timestamp(&[modsettings_created, meta_created]);
        let (created_at, modified_at) =
            resolve_native_times(primary_created, raw_created, raw_modified);
        let mod_entry = ModEntry {
            id: uuid.clone(),
            name: info.name.clone(),
            created_at,
            modified_at,
            added_at: now_timestamp(),
            targets: vec![InstallTarget::Pak {
                file: filename,
                info,
            }],
            target_overrides: Vec::new(),
            source_label: None,
            source: ModSource::Native,
            dependencies,
        };
        added.push(mod_entry);
        existing_ids.insert(uuid);
    }

    Ok(NativeSyncDelta {
        updates,
        added,
        updated_native_files,
        adopted_native,
        modsettings_exists,
        modsettings_hash,
        enabled_set,
        order,
    })
}

fn now_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn resolve_pak_paths(
    mod_entry: &ModEntry,
    data_dir: &PathBuf,
    paths: Option<&crate::bg3::GamePaths>,
    native_index: Option<&[native_pak::NativePakEntry]>,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut push_unique = |path: PathBuf| {
        if !out.iter().any(|existing| existing == &path) {
            out.push(path);
        }
    };
    for target in &mod_entry.targets {
        let InstallTarget::Pak { file, info } = target else {
            continue;
        };
        if mod_entry.is_native() {
            if let Some(paths) = paths {
                let by_folder = paths.larian_mods_dir.join(format!("{}.pak", info.folder));
                if by_folder.exists() {
                    push_unique(by_folder);
                }
                let by_file = paths.larian_mods_dir.join(file);
                if by_file.exists() {
                    push_unique(by_file);
                }
            }
            if let Some(index) = native_index {
                if let Some(path) = native_pak::resolve_native_pak_path(info, index) {
                    push_unique(path);
                }
            }
        } else {
            let path = data_dir.join("mods").join(&mod_entry.id).join(file);
            if path.exists() {
                push_unique(path);
            } else if let Some(index) = if data_dir.join("mods").join(&mod_entry.id).exists() {
                Some(native_pak::build_native_pak_index(
                    &data_dir.join("mods").join(&mod_entry.id),
                ))
            } else {
                None
            } {
                if let Some(path) = native_pak::resolve_native_pak_path(info, &index) {
                    push_unique(path);
                }
            }
        }
    }
    out
}

fn scan_mod_targets_times(mod_entry: &ModEntry, mod_root: &PathBuf) -> (Option<i64>, Option<i64>) {
    let mut created_at: Option<i64> = None;
    let mut modified_at: Option<i64> = None;
    for target in &mod_entry.targets {
        let dir = match target {
            InstallTarget::Data { dir } => mod_root.join(dir),
            InstallTarget::Generated { dir } => mod_root.join(dir),
            InstallTarget::Bin { dir } => mod_root.join(dir),
            InstallTarget::Pak { .. } => continue,
        };
        if !dir.exists() {
            continue;
        }
        for entry in WalkDir::new(&dir).into_iter().filter_map(Result::ok) {
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
    }
    normalize_times(created_at, modified_at)
}
