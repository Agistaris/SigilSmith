use crate::{
    backup,
    config::{AppConfig, GameConfig},
    deploy,
    game::{self, GameId},
    importer,
    library::{
        FileOverride, normalize_label, Library, ModEntry, ProfileEntry, TargetKind, TargetOverride,
    },
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashSet, VecDeque},
    fs,
    io::Write,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use blake3::Hasher;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputPurpose {
    ImportPath,
    SetupGameRoot,
    SetupLarianDir,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupStep {
    GameRoot,
    LarianDir,
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
    Unrecognized { path: PathBuf, label: String },
    DeleteProfile { name: String },
    DeleteMod { id: String, name: String },
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

enum ImportMessage {
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
    Completed {
        report: deploy::DeployReport,
    },
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
}

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
    pub should_quit: bool,
    pub move_mode: bool,
    pub dialog: Option<Dialog>,
    pub logs: Vec<LogEntry>,
    pub log_scroll: usize,
    pub move_dirty: bool,
    pub focus: Focus,
    pub explorer_selected: usize,
    pub toast: Option<Toast>,
    pub mod_filter: String,
    pub settings_menu: Option<SettingsMenu>,
    import_queue: VecDeque<PathBuf>,
    import_active: Option<PathBuf>,
    import_tx: Sender<ImportMessage>,
    import_rx: Receiver<ImportMessage>,
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
    approved_imports: Vec<ModEntry>,
    pub conflicts: Vec<deploy::ConflictEntry>,
    pub conflict_selected: usize,
    explorer_game_expanded: HashSet<GameId>,
    explorer_profiles_expanded: HashSet<GameId>,
}

#[derive(Debug, Clone)]
pub struct SettingsMenu {
    pub selected: usize,
}

impl App {
    pub fn initialize() -> Result<Self> {
        let mut setup_error = None;
        let app_config = AppConfig::load_or_create()?;
        let game_id = app_config.active_game;
        let mut config = GameConfig::load_or_create(game_id)?;
        if let Err(err) = game::detect_paths(
            game_id,
            Some(&config.game_root),
            Some(&config.larian_dir),
        ) {
            setup_error = Some(err.to_string());
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
        let log_path = config.data_dir.join("sigilsmith.log");

        let mut app = Self {
            app_config,
            game_id,
            config,
            library,
            status: "Ready".to_string(),
            selected: 0,
            input_mode: InputMode::Normal,
            should_quit: false,
            move_mode: false,
            dialog: None,
            logs: Vec::new(),
            log_scroll: 0,
            move_dirty: false,
            focus: Focus::Mods,
            explorer_selected: 0,
            toast: None,
            mod_filter: String::new(),
            settings_menu: None,
            import_queue: VecDeque::new(),
            import_active: None,
            import_tx,
            import_rx,
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
            approved_imports: Vec::new(),
            conflicts: Vec::new(),
            conflict_selected: 0,
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

        let mod_count = app.library.mods.len();
        app.log_info(format!("Library loaded: {mod_count} mod(s)"));
        if let Some(error) = setup_error {
            app.log_warn(format!("Path auto-detect failed: {error}"));
        }
        app.ensure_setup();
        app.queue_conflict_scan("startup");
        Ok(app)
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
        profile
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
            .collect()
    }

    pub fn visible_profile_entries(&self) -> Vec<(usize, ProfileEntry)> {
        let Some(profile) = self.library.active_profile() else {
            return Vec::new();
        };
        let indices = self.visible_profile_indices();
        indices
            .into_iter()
            .filter_map(|index| profile.order.get(index).cloned().map(|entry| (index, entry)))
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

    pub fn conflicts_scanning(&self) -> bool {
        self.conflict_active
    }

    pub fn conflicts_pending(&self) -> bool {
        self.conflict_pending
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
        self.focus = match self.focus {
            Focus::Explorer => Focus::Mods,
            Focus::Mods => Focus::Conflicts,
            Focus::Conflicts => Focus::Explorer,
        };
        self.move_mode = false;
        self.status = match self.focus {
            Focus::Explorer => "Focus: explorer".to_string(),
            Focus::Mods => "Focus: mod stack".to_string(),
            Focus::Conflicts => "Focus: overrides".to_string(),
        };
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
        self.move_mode = false;
        self.focus = Focus::Mods;
        self.status = format!("Active game: {}", game_id.display_name());
        self.log_info(format!("Active game: {}", game_id.display_name()));
        self.ensure_setup();
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
            prompt: "Import profile list".to_string(),
            buffer: String::new(),
            purpose: InputPurpose::ImportProfile,
            auto_submit: false,
            last_edit_at: Instant::now(),
        };
        self.status = "Import profile list: enter path".to_string();
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
            self.set_toast(
                "Rename cancelled",
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

        let message = format!("Delete profile \"{name}\"?\nThis cannot be undone.");
        self.open_dialog(Dialog {
            title: "Delete Profile".to_string(),
            message,
            yes_label: "Delete".to_string(),
            no_label: "Cancel".to_string(),
            choice: DialogChoice::No,
            kind: DialogKind::DeleteProfile { name },
            toggle: Some(DialogToggle {
                label: "Don't ask again for this action".to_string(),
                checked: false,
            }),
        });
    }

    pub fn prompt_delete_mod(&mut self, id: String, name: String) {
        if self.dialog.is_some() {
            return;
        }

        let message = format!("Remove mod \"{name}\"?\nThis will delete it from the library.");
        self.open_dialog(Dialog {
            title: "Remove Mod".to_string(),
            message,
            yes_label: "Remove".to_string(),
            no_label: "Cancel".to_string(),
            choice: DialogChoice::No,
            kind: DialogKind::DeleteMod { id, name },
            toggle: Some(DialogToggle {
                label: "Don't ask again for this action".to_string(),
                checked: false,
            }),
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
        if !self.library.profiles.iter().any(|profile| profile.name == name) {
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
        self.queue_auto_deploy("profile changed");
        Ok(())
    }

    pub fn conflict_move_up(&mut self) {
        if self.conflict_selected == 0 {
            return;
        }
        self.conflict_selected -= 1;
    }

    pub fn conflict_move_down(&mut self) {
        if self.conflict_selected + 1 >= self.conflicts.len() {
            return;
        }
        self.conflict_selected += 1;
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
        if let Err(err) = self.set_conflict_winner(self.conflict_selected, winner_id) {
            self.status = format!("Override failed: {err}");
            self.log_error(format!("Override failed: {err}"));
        }
    }

    pub fn clear_conflict_override(&mut self) {
        let Some(conflict) = self.conflicts.get(self.conflict_selected).cloned() else {
            return;
        };
        if let Err(err) =
            self.set_conflict_winner(self.conflict_selected, conflict.default_winner_id)
        {
            self.status = format!("Override failed: {err}");
            self.log_error(format!("Override failed: {err}"));
        }
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
                override_entry.kind != conflict.target
                    || override_entry.relative_path != rel_path
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
        let mut updated = conflict.clone();
        updated.winner_id = winner_id.clone();
        if let Some(candidate) = updated
            .candidates
            .iter()
            .find(|candidate| candidate.mod_id == winner_id)
        {
            updated.winner_name = candidate.mod_name.clone();
        }
        updated.overridden = updated.winner_id != updated.default_winner_id;
        self.conflicts[index] = updated;

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
            if self.library.mods.iter().any(|mod_entry| mod_entry.id == entry.id) {
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
            toast_message = format!(
                "Profile imported: {name} (missing {} mods)",
                missing.len()
            );
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
    }

    pub fn paths_ready(&self) -> bool {
        game::looks_like_game_root(self.game_id, &self.config.game_root)
            && game::looks_like_user_dir(self.game_id, &self.config.larian_dir)
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
        }
    }

    pub fn enter_setup_game_root(&mut self) {
        self.move_mode = false;
        self.input_mode = InputMode::Editing {
            prompt: self.game_id.root_prompt().to_string(),
            buffer: String::new(),
            purpose: InputPurpose::SetupGameRoot,
            auto_submit: false,
            last_edit_at: Instant::now(),
        };
        self.status = self.game_id.root_hint().to_string();
    }

    pub fn enter_setup_larian_dir(&mut self) {
        self.move_mode = false;
        self.input_mode = InputMode::Editing {
            prompt: self.game_id.user_dir_prompt().to_string(),
            buffer: String::new(),
            purpose: InputPurpose::SetupLarianDir,
            auto_submit: false,
            last_edit_at: Instant::now(),
        };
        self.status = self.game_id.user_dir_hint().to_string();
    }

    pub fn enter_import_mode(&mut self) {
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

    pub fn enter_import_with(&mut self, seed: String) {
        if self.dialog.is_some() {
            self.log_warn("Drop ignored: dialog active".to_string());
            return;
        }
        self.move_mode = false;
        self.input_mode = InputMode::Editing {
            prompt: "Import path".to_string(),
            buffer: seed,
            purpose: InputPurpose::ImportPath,
            auto_submit: true,
            last_edit_at: Instant::now(),
        };
        self.status = "Drop detected: importing after pause".to_string();
    }

    pub fn enter_mod_filter(&mut self) {
        self.move_mode = false;
        self.input_mode = InputMode::Editing {
            prompt: "Filter mods".to_string(),
            buffer: self.mod_filter.clone(),
            purpose: InputPurpose::FilterMods,
            auto_submit: false,
            last_edit_at: Instant::now(),
        };
        self.status = "Filter mods: type to match, Enter to apply".to_string();
    }

    pub fn clear_mod_filter(&mut self) {
        if self.mod_filter.trim().is_empty() {
            self.status = "Filter already cleared".to_string();
            return;
        }
        self.set_mod_filter(String::new());
    }

    pub fn handle_submit(&mut self, purpose: InputPurpose, value: String) -> Result<()> {
        match purpose {
            InputPurpose::ImportPath => self.import_mod(value),
            InputPurpose::SetupGameRoot => self.submit_game_root(value),
            InputPurpose::SetupLarianDir => self.submit_larian_dir(value),
            InputPurpose::CreateProfile => self.create_profile(value),
            InputPurpose::RenameProfile { original } => self.rename_profile(original, value),
            InputPurpose::DuplicateProfile { source } => self.duplicate_profile(source, value),
            InputPurpose::ExportProfile { profile } => self.export_profile(profile, value),
            InputPurpose::ImportProfile => self.import_profile(value),
            InputPurpose::FilterMods => {
                self.set_mod_filter(value);
                Ok(())
            }
        }
    }

    fn set_mod_filter(&mut self, value: String) {
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
        if self.mod_filter.is_empty() {
            self.status = "Filter cleared".to_string();
            self.log_info("Filter cleared".to_string());
        } else {
            self.status = format!("Filter set: \"{}\"", self.mod_filter);
            self.log_info(format!("Filter set: \"{}\"", self.mod_filter));
        }
        self.clamp_selection();
    }

    pub fn import_mod(&mut self, raw_path: String) -> Result<()> {
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
            self.status = format!(
                "Importing {} (queued {})",
                display_path(active),
                queued
            );
        } else {
            self.status = format!("Queued import: {}", display_path(&path));
        }
        self.start_next_import();

        Ok(())
    }

    fn submit_game_root(&mut self, raw_path: String) -> Result<()> {
        let path = expand_tilde(raw_path.trim());
        if !path.exists() {
            self.status = format!("Path not found: {}", path.display());
            self.log_warn(format!("Game root not found: {}", path.display()));
            self.enter_setup_game_root();
            return Ok(());
        }

        if !game::looks_like_game_root(self.game_id, &path) {
            self.status = "Invalid game root: expected Data/ and bin/".to_string();
            self.log_warn(format!("Invalid game root: {}", path.display()));
            self.enter_setup_game_root();
            return Ok(());
        }

        self.config.game_root = path.clone();
        match game::detect_paths(self.game_id, Some(&path), None) {
            Ok(paths) => {
                self.config.larian_dir = paths.larian_dir;
                self.config.save()?;
                self.status = "Game paths set".to_string();
                self.log_info(format!("Game root set: {}", path.display()));
            }
            Err(err) => {
                self.status =
                    "Game root set. Larian data dir not found; please enter it.".to_string();
                self.log_warn(format!("Larian dir auto-detect failed: {err}"));
                self.start_setup(SetupStep::LarianDir);
            }
        }

        Ok(())
    }

    fn submit_larian_dir(&mut self, raw_path: String) -> Result<()> {
        let path = expand_tilde(raw_path.trim());
        if !path.exists() {
            self.status = format!("Path not found: {}", path.display());
            self.log_warn(format!("Larian dir not found: {}", path.display()));
            self.enter_setup_larian_dir();
            return Ok(());
        }

        if !game::looks_like_user_dir(self.game_id, &path) {
            self.status = "Invalid Larian dir: expected PlayerProfiles/".to_string();
            self.log_warn(format!("Invalid Larian dir: {}", path.display()));
            self.enter_setup_larian_dir();
            return Ok(());
        }

        if !game::looks_like_game_root(self.game_id, &self.config.game_root) {
            self.status = "Game root missing: enter BG3 install root".to_string();
            self.log_warn("Game root missing while setting Larian dir".to_string());
            self.start_setup(SetupStep::GameRoot);
            return Ok(());
        }

        self.config.larian_dir = path.clone();
        self.config.save()?;
        self.status = "Game paths set".to_string();
        self.log_info(format!("Larian dir set: {}", path.display()));
        Ok(())
    }

    pub fn import_mod_blocking(&mut self, raw_path: String) -> Result<usize> {
        let path = expand_tilde(raw_path.trim());
        if !path.exists() {
            self.log_warn(format!("Import path not found: {}", path.display()));
            return Ok(0);
        }

        self.log_info(format!("Import started: {}", path.display()));
        let imports = match importer::import_path(&path, &self.config.data_dir)
            .with_context(|| format!("import {path:?}"))
        {
            Ok(imports) => imports,
            Err(err) => {
                self.log_error(format!("Import failed for {}: {err}", path.display()));
                return Err(err);
            }
        };
        if imports.mods.is_empty() {
            if imports.unrecognized {
                self.log_warn(format!(
                    "Unrecognized mod layout for {} (skipped)",
                    path.display()
                ));
                return Ok(0);
            }
            self.log_warn(format!("No mods detected in {}", path.display()));
            return Ok(0);
        }

        let mods = imports.mods;
        let mut overwritten = 0;
        for mod_entry in &mods {
            if let Some(existing_id) = self
                .find_duplicate_by_name(&mod_entry.name)
                .map(|entry| entry.id.clone())
            {
                if self.remove_mod_by_id(&existing_id) {
                    overwritten += 1;
                }
            }
        }
        if overwritten > 0 {
            self.log_warn(format!("Overwrote {overwritten} duplicate mod(s)"));
        }

        let count = self.apply_mod_entries(mods)?;
        self.log_info(format!(
            "Import complete: {} mod(s) from {}",
            count,
            path.display()
        ));
        Ok(count)
    }

    pub fn poll_imports(&mut self) {
        loop {
            match self.import_rx.try_recv() {
                Ok(message) => self.handle_import_message(message),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        if self.import_active.is_none() {
            self.start_next_import();
        }

        self.poll_deploys();
        self.maybe_start_deploy();
        self.poll_conflicts();
        self.maybe_start_conflict_scan();
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
            } => (*auto_submit, *last_edit_at, buffer.trim().to_string(), purpose.clone()),
            _ => return None,
        };

        if !auto_submit {
            return None;
        }

        if value.is_empty() {
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
        if self.dialog.is_some() || self.pending_duplicate.is_some() || !self.duplicate_queue.is_empty()
        {
            return;
        }

        let Some(path) = self.import_queue.pop_front() else {
            return;
        };

        self.import_active = Some(path.clone());
        self.status = format!("Importing {}", display_path(&path));
        self.log_info(format!("Import started: {}", path.display()));

        let tx = self.import_tx.clone();
        let data_dir = self.config.data_dir.clone();
        thread::spawn(move || {
            let result = importer::import_path(&path, &data_dir)
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
            ImportMessage::Completed { path, result } => {
                self.import_active = None;
                if result.mods.is_empty() {
                    if result.unrecognized {
                        self.prompt_unrecognized(path);
                        return;
                    }
                    self.status = "No mods found to import".to_string();
                    self.log_warn(format!("No mods detected in {}", path.display()));
                    return;
                }

                self.stage_imports(result.mods, &path);
            }
            ImportMessage::Failed { path, error } => {
                self.import_active = None;
                let display = display_path(&path);
                let reason = summarize_error(&error);
                self.status = format!("Import failed: {display} ({reason})");
                self.log_error(format!("Import failed for {}: {error}", path.display()));
                self.set_toast(
                    &format!("Import failed: {display} ({reason})"),
                    ToastLevel::Error,
                    Duration::from_secs(4),
                );
            }
        }
    }

    fn stage_imports(&mut self, mods: Vec<ModEntry>, source: &PathBuf) {
        let mut approved = Vec::new();
        let mut duplicates = VecDeque::new();

        for mod_entry in mods {
            if let Some(existing) = self.find_duplicate_by_name(&mod_entry.name) {
                duplicates.push_back(DuplicateDecision {
                    mod_entry,
                    existing_id: existing.id.clone(),
                    existing_label: existing.display_name(),
                    kind: DuplicateKind::Exact,
                });
            } else if let Some(similar) = self.find_similar_by_label(&mod_entry) {
                let existing_label = similar.existing_label.clone();
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
                source.display()
            ));
            self.prompt_next_duplicate();
            return;
        }

        match self.apply_mod_entries(approved) {
            Ok(count) => {
                self.status = format!("Imported {} mod(s)", count);
                self.log_info(format!(
                    "Import complete: {} mod(s) from {}",
                    count,
                    source.display()
                ));
            }
            Err(err) => {
                let display = display_path(source);
                let reason = summarize_error(&err.to_string());
                self.status = format!("Import failed: {display} ({reason})");
                self.log_error(format!(
                    "Import apply failed for {}: {err}",
                    source.display()
                ));
                self.set_toast(
                    &format!("Import failed: {display} ({reason})"),
                    ToastLevel::Error,
                    Duration::from_secs(4),
                );
            }
        }
    }

    fn apply_mod_entries(&mut self, mods: Vec<ModEntry>) -> Result<usize> {
        let count = mods.len();
        if count == 0 {
            return Ok(0);
        }

        for mod_entry in mods {
            self.library.mods.retain(|entry| entry.id != mod_entry.id);
            self.library.mods.push(mod_entry);
        }

        self.library.ensure_mods_in_profiles();
        self.library.save(&self.config.data_dir)?;
        self.queue_conflict_scan("library update");
        Ok(count)
    }

    fn prompt_next_duplicate(&mut self) {
        if self.pending_duplicate.is_some() {
            return;
        }

        if self.dialog.is_some() {
            return;
        }

        let Some(next) = self.duplicate_queue.pop_front() else {
            let approved = std::mem::take(&mut self.approved_imports);
            if approved.is_empty() {
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
                }
            }
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

        self.pending_duplicate = Some(next);
        self.open_dialog(Dialog {
            title,
            message,
            yes_label: "Overwrite".to_string(),
            no_label: "Skip".to_string(),
            choice: DialogChoice::No,
            kind,
            toggle: None,
        });
    }

    pub fn confirm_duplicate(&mut self, overwrite: bool) {
        let Some(decision) = self.pending_duplicate.take() else {
            return;
        };

        if overwrite {
            let removed = self.remove_mod_by_id(&decision.existing_id);
            if removed {
                let label = match decision.kind {
                    DuplicateKind::Exact => "duplicate",
                    DuplicateKind::Similar { .. } => "similar",
                };
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

        self.input_mode = InputMode::Normal;
        self.prompt_next_duplicate();
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
        });
    }

    fn open_dialog(&mut self, dialog: Dialog) {
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
                self.confirm_duplicate(matches!(choice, DialogChoice::Yes));
            }
            DialogKind::Unrecognized { path, label } => {
                if matches!(choice, DialogChoice::Yes) {
                    let entry = build_unknown_entry(&path, &label);
                    self.log_warn(format!("Importing unknown layout: {label}"));
                    self.stage_imports(vec![entry], &path);
                } else {
                    self.log_warn(format!("Skipped unrecognized layout: {label}"));
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
            DialogKind::DeleteMod { id, name } => {
                if matches!(choice, DialogChoice::Yes) {
                    if let Some(toggle) = dialog.toggle {
                        if toggle.checked {
                            self.app_config.confirm_mod_delete = false;
                            let _ = self.app_config.save();
                        }
                    }
                    if !self.remove_mod_by_id(&id) {
                        self.status = "No mod removed".to_string();
                        return;
                    }
                    self.status = format!("Mod removed: {name}");
                    self.log_info(format!("Mod removed: {name}"));
                    self.clamp_selection();
                    self.queue_auto_deploy("mod removed");
                }
            }
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
        let new_raw = mod_entry
            .source_label()
            .unwrap_or(mod_entry.name.as_str());
        let new_normalized = normalize_label(new_raw);
        if new_normalized.len() < 6 {
            return None;
        }

        let mut best: Option<SimilarMatch> = None;
        for existing in &self.library.mods {
            let existing_raw = existing
                .source_label()
                .unwrap_or(existing.name.as_str());
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

    fn remove_mod_by_id(&mut self, id: &str) -> bool {
        let mod_root = self.config.data_dir.join("mods").join(id);
        let before = self.library.mods.len();
        self.library.mods.retain(|mod_entry| mod_entry.id != id);
        if before == self.library.mods.len() {
            return false;
        }

        for profile in &mut self.library.profiles {
            profile.order.retain(|entry| entry.id != id);
            profile.file_overrides.retain(|override_entry| override_entry.mod_id != id);
        }

        let _ = fs::remove_dir_all(&mod_root);
        let _ = self.library.save(&self.config.data_dir);
        self.queue_conflict_scan("mod removed");
        true
    }

    fn remove_mod_root(&self, id: &str) {
        let mod_root = self.config.data_dir.join("mods").join(id);
        let _ = fs::remove_dir_all(&mod_root);
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
            library.profiles.push(crate::library::Profile::new("Default"));
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
        let Some(index) = self.selected_profile_index() else {
            return;
        };
        let Some(profile) = self.library.active_profile_mut() else {
            return;
        };
        if let Some(entry) = profile.order.get_mut(index) {
            entry.enabled = !entry.enabled;
            self.queue_auto_deploy("enable toggle");
        }
    }

    pub fn toggle_move_mode(&mut self) {
        if self.move_mode {
            self.move_mode = false;
            self.status = "Move mode disabled".to_string();
            if self.move_dirty {
                self.move_dirty = false;
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

        if !self.remove_mod_by_id(&selected_id) {
            self.status = "No mod removed".to_string();
            return;
        }

        self.status = "Mod removed from library".to_string();
        self.log_info("Mod removed from library".to_string());
        self.clamp_selection();
        self.queue_auto_deploy("mod removed");
    }

    pub fn request_remove_selected(&mut self) {
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

        if self.app_config.confirm_mod_delete {
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
            self.queue_auto_deploy("order changed");
        }
    }

    pub fn enable_visible_mods(&mut self) {
        let indices = self.visible_profile_indices();
        if indices.is_empty() {
            self.status = "No visible mods to enable".to_string();
            return;
        }
        let Some(profile) = self.library.active_profile_mut() else {
            return;
        };
        let mut changed = 0;
        for index in indices {
            if let Some(entry) = profile.order.get_mut(index) {
                if !entry.enabled {
                    entry.enabled = true;
                    changed += 1;
                }
            }
        }
        if changed == 0 {
            self.status = "Visible mods already enabled".to_string();
            return;
        }
        self.status = format!("Enabled {changed} mod(s)");
        self.log_info(format!("Enabled {changed} mod(s)"));
        self.queue_auto_deploy("enable all");
    }

    pub fn disable_visible_mods(&mut self) {
        let indices = self.visible_profile_indices();
        if indices.is_empty() {
            self.status = "No visible mods to disable".to_string();
            return;
        }
        let Some(profile) = self.library.active_profile_mut() else {
            return;
        };
        let mut changed = 0;
        for index in indices {
            if let Some(entry) = profile.order.get_mut(index) {
                if entry.enabled {
                    entry.enabled = false;
                    changed += 1;
                }
            }
        }
        if changed == 0 {
            self.status = "Visible mods already disabled".to_string();
            return;
        }
        self.status = format!("Disabled {changed} mod(s)");
        self.log_info(format!("Disabled {changed} mod(s)"));
        self.queue_auto_deploy("disable all");
    }

    pub fn invert_visible_mods(&mut self) {
        let indices = self.visible_profile_indices();
        if indices.is_empty() {
            self.status = "No visible mods to invert".to_string();
            return;
        }
        let Some(profile) = self.library.active_profile_mut() else {
            return;
        };
        for index in indices {
            if let Some(entry) = profile.order.get_mut(index) {
                entry.enabled = !entry.enabled;
            }
        }
        self.status = "Toggled visible mods".to_string();
        self.log_info("Toggled visible mods".to_string());
        self.queue_auto_deploy("invert selection");
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
        self.queue_deploy(&format!("auto: {reason}"));
        self.queue_conflict_scan(reason);
    }

    fn queue_deploy(&mut self, reason: &str) {
        if !self.paths_ready() {
            self.status = "Game paths not set: press g to configure".to_string();
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
            self.status = "Game paths not set: press g to configure".to_string();
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
    haystacks
        .into_iter()
        .any(|value| value.to_lowercase().contains(&filter))
}

const LOG_CAPACITY: usize = 200;

fn expand_tilde(input: &str) -> PathBuf {
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

fn summarize_error(error: &str) -> String {
    let first_line = error.lines().next().unwrap_or(error).trim();
    let last = first_line
        .rsplit(": ")
        .next()
        .unwrap_or(first_line)
        .trim();
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
            curr[j + 1] = (prev[j + 1] + 1)
                .min(curr[j] + 1)
                .min(prev[j] + cost);
        }
        prev.clone_from_slice(&curr);
    }

    prev[b_bytes.len()]
}

fn extract_timestamp(label: &str) -> Option<u64> {
    let mut best = None;
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
    ModEntry {
        id: unknown_id(path),
        name: label.to_string(),
        added_at: now_timestamp(),
        targets: Vec::new(),
        target_overrides: Vec::new(),
        source_label: Some(label.to_string()),
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

fn now_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
