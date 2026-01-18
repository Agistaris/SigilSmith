use crate::{
    app::{
        expand_tilde, App, DependencyStatus, DialogChoice, DialogKind, ExplorerItem,
        ExplorerItemKind, ExportKind, Focus, InputMode, InputPurpose, LogLevel, ModSort,
        ModSortColumn, PathBrowser, PathBrowserEntryKind, PathBrowserFocus, PathBrowserPurpose,
        SetupStep, SigilLinkCacheAction, SigilLinkMissingTrigger, ToastLevel, UpdateStatus,
    },
    library::{InstallTarget, ModEntry, TargetKind},
};
use anyhow::Result;
use arboard::Clipboard;
use crossterm::{
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Cell, Clear, Gauge, List, ListItem, ListState, Padding,
        Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table, TableState, Wrap,
    },
};
use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const SIDE_PANEL_WIDTH: u16 = 43;
const STATUS_WIDTH: u16 = SIDE_PANEL_WIDTH;
const HEADER_HEIGHT: u16 = 3;
const DETAILS_HEIGHT: u16 = 12;
const CONTEXT_HEIGHT: u16 = 28;
const LOG_MIN_HEIGHT: u16 = 5;
const CONFLICTS_BAR_HEIGHT: u16 = 0;
const FILTER_HEIGHT: u16 = 2;
const TABLE_MIN_HEIGHT: u16 = 6;
const SUBPANEL_PAD_X: u16 = 0;
const SUBPANEL_PAD_TOP: u16 = 0;

#[derive(Clone)]
struct Theme {
    accent: Color,
    accent_soft: Color,
    section_bg: Color,
    border: Color,
    row_alt_bg: Color,
    text: Color,
    muted: Color,
    success: Color,
    warning: Color,
    error: Color,
    header_bg: Color,
    mod_bg: Color,
    log_bg: Color,
    subpanel_bg: Color,
    swap_bg: Color,
    overlay_panel_bg: Color,
    overlay_border: Color,
    overlay_bar: Color,
    overlay_scrim: Color,
}

impl Theme {
    fn new() -> Self {
        Self {
            accent: Color::Rgb(120, 198, 255),
            accent_soft: Color::Rgb(58, 92, 138),
            section_bg: Color::Rgb(84, 146, 200),
            border: Color::Rgb(72, 84, 102),
            row_alt_bg: Color::Rgb(30, 32, 34),
            text: Color::Rgb(216, 226, 236),
            muted: Color::Rgb(124, 134, 146),
            success: Color::Rgb(120, 220, 150),
            warning: Color::Rgb(235, 200, 120),
            error: Color::Rgb(240, 104, 100),
            header_bg: Color::Rgb(18, 24, 34),
            mod_bg: Color::Rgb(22, 23, 25),
            log_bg: Color::Rgb(13, 18, 26),
            subpanel_bg: Color::Rgb(13, 18, 26),
            swap_bg: Color::Rgb(20, 90, 74),
            overlay_panel_bg: Color::Rgb(12, 20, 32),
            overlay_border: Color::Rgb(90, 140, 190),
            overlay_bar: Color::Rgb(120, 198, 255),
            overlay_scrim: Color::Rgb(10, 14, 22),
        }
    }

    fn block(&self, title: &'static str) -> Block<'static> {
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(Style::default().fg(self.border))
            .title(Span::styled(
                title,
                Style::default()
                    .fg(self.accent)
                    .add_modifier(Modifier::BOLD),
            ))
    }

    fn panel(&self, title: &'static str) -> Block<'static> {
        self.block(title)
    }

    fn panel_tight(&self, title: &'static str) -> Block<'static> {
        self.block(title)
    }

    fn subpanel(&self, title: &'static str) -> Block<'static> {
        let mut block = Block::default()
            .borders(Borders::NONE)
            .style(Style::default().bg(self.subpanel_bg));
        if !title.is_empty() {
            let title = format!(" {title} ");
            block = block.title(Span::styled(
                title,
                Style::default()
                    .fg(self.header_bg)
                    .bg(self.accent_soft)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        block
    }
}

pub fn run(app: &mut App) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;

    result
}

fn run_loop(terminal: &mut Terminal<impl Backend>, app: &mut App) -> Result<()> {
    let mut startup_complete = false;
    loop {
        app.tick();
        if let Some((purpose, value)) = app.maybe_auto_submit() {
            if let Err(err) = app.handle_submit(purpose, value) {
                app.status = format!("Action failed: {err}");
                app.log_error(format!("Action failed: {err}"));
            }
        }
        app.poll_imports();
        app.poll_metadata_refresh();
        app.poll_missing_pak_scan();
        app.poll_smart_rank();
        app.poll_updates();
        app.clamp_selection();
        terminal.draw(|frame| draw(frame, app))?;
        if !startup_complete {
            app.finish_startup();
            startup_complete = true;
        }

        if app.should_quit {
            break;
        }

        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) => {
                    handle_key(app, key)?;
                }
                Event::Paste(text) => {
                    if let Err(err) = handle_paste(app, text) {
                        app.status = format!("Paste failed: {err}");
                        app.log_error(format!("Paste failed: {err}"));
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if app.dialog.is_some() {
        return handle_dialog_mode(app, key);
    }
    if app.override_picker_active() {
        return handle_override_picker(app, key);
    }
    if app.sigillink_missing_queue_active() {
        return handle_sigillink_missing_queue(app, key);
    }
    if app.dependency_queue_active() {
        return handle_dependency_queue(app, key);
    }
    if app.paths_overlay_open {
        return handle_paths_overlay(app, key);
    }
    if app.whats_new_open {
        return handle_whats_new_mode(app, key);
    }
    if app.help_open {
        return handle_help_mode(app, key);
    }
    if app.smart_rank_preview.is_some() {
        return handle_smart_rank_preview(app, key);
    }
    if app.mod_list_preview.is_some() {
        return handle_mod_list_preview(app, key);
    }
    if app.export_menu.is_some() {
        return handle_export_menu(app, key);
    }
    if app.settings_menu.is_some() {
        return handle_settings_menu(app, key);
    }

    let mode = std::mem::replace(&mut app.input_mode, InputMode::Normal);
    match mode {
        InputMode::Normal => {
            app.input_mode = InputMode::Normal;
            handle_normal_mode(app, key)
        }
        InputMode::Browsing(mut browser) => {
            let close = handle_browser_mode(app, key, &mut browser)?;
            if !close {
                app.input_mode = InputMode::Browsing(browser);
            }
            Ok(())
        }
        InputMode::Editing {
            prompt,
            mut buffer,
            purpose,
            auto_submit,
            mut last_edit_at,
        } => handle_input_mode(
            app,
            key,
            &mut buffer,
            purpose.clone(),
            prompt,
            auto_submit,
            &mut last_edit_at,
        ),
    }
}

fn handle_dialog_mode(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('H') => {
            app.dialog_choice_left();
        }
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('L') | KeyCode::Tab => {
            if matches!(key.code, KeyCode::Tab) {
                if let Some(dialog) = app.dialog.as_ref() {
                    if matches!(dialog.kind, DialogKind::SigilLinkOnboarding) {
                        return Ok(());
                    }
                }
            }
            app.dialog_choice_right();
        }
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => {
            if let Some(dialog) = &mut app.dialog {
                dialog.scroll = dialog.scroll.saturating_sub(1);
            }
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => {
            if let Some(dialog) = &mut app.dialog {
                dialog.scroll = dialog.scroll.saturating_add(1);
            }
        }
        KeyCode::PageUp => {
            if let Some(dialog) = &mut app.dialog {
                dialog.scroll = dialog.scroll.saturating_sub(6);
            }
        }
        KeyCode::PageDown => {
            if let Some(dialog) = &mut app.dialog {
                dialog.scroll = dialog.scroll.saturating_add(6);
            }
        }
        KeyCode::Home => {
            if let Some(dialog) = &mut app.dialog {
                dialog.scroll = 0;
            }
        }
        KeyCode::End => {
            if let Some(dialog) = &mut app.dialog {
                dialog.scroll = usize::MAX;
            }
        }
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            app.dialog_set_choice(DialogChoice::Yes);
        }
        KeyCode::Char('n') | KeyCode::Char('N') => {
            app.dialog_set_choice(DialogChoice::No);
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            if let Some(dialog) = &mut app.dialog {
                if let Some(toggle) = &mut dialog.toggle {
                    toggle.checked = !toggle.checked;
                }
            }
        }
        KeyCode::Char('a') | KeyCode::Char('A') => {
            if let Some(dialog) = &mut app.dialog {
                if let Some(toggle) = &mut dialog.toggle_alt {
                    toggle.checked = !toggle.checked;
                }
            }
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            app.dialog_confirm();
        }
        KeyCode::Esc => {
            if let Some(dialog) = app.dialog.as_ref() {
                match &dialog.kind {
                    DialogKind::DeleteMod { .. } => {
                        app.close_dialog();
                    }
                    DialogKind::DisableDependents { .. } => {
                        app.dialog_set_choice(DialogChoice::Yes);
                        app.dialog_confirm();
                    }
                    _ => {
                        app.dialog_set_choice(DialogChoice::No);
                        app.dialog_confirm();
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn handle_help_mode(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('?') => app.close_help(),
        KeyCode::Char('q') | KeyCode::Char('Q') => app.should_quit = true,
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => {
            app.help_scroll = app.help_scroll.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => {
            app.help_scroll = app.help_scroll.saturating_add(1);
        }
        KeyCode::PageUp => {
            app.help_scroll = app.help_scroll.saturating_sub(6);
        }
        KeyCode::PageDown => {
            app.help_scroll = app.help_scroll.saturating_add(6);
        }
        KeyCode::Home => {
            app.help_scroll = 0;
        }
        KeyCode::End => {
            app.help_scroll = usize::MAX;
        }
        _ => {}
    }
    Ok(())
}

fn handle_whats_new_mode(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char(' ') => {
            if app.whats_new_can_close() {
                app.close_whats_new();
            }
        }
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => {
            app.whats_new_scroll = app.whats_new_scroll.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => {
            app.whats_new_scroll = app.whats_new_scroll.saturating_add(1);
        }
        KeyCode::PageUp => {
            app.whats_new_scroll = app.whats_new_scroll.saturating_sub(6);
        }
        KeyCode::PageDown => {
            app.whats_new_scroll = app.whats_new_scroll.saturating_add(6);
        }
        KeyCode::Home => {
            app.whats_new_scroll = 0;
        }
        KeyCode::End => {
            app.whats_new_scroll = usize::MAX;
        }
        _ => {}
    }
    Ok(())
}

fn handle_paths_overlay(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char(' ') => {
            app.close_paths_overlay();
        }
        _ => {}
    }
    Ok(())
}

fn handle_smart_rank_preview(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
            app.apply_smart_rank_preview();
        }
        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
            app.cancel_smart_rank_preview();
        }
        KeyCode::Tab => {
            app.smart_rank_view = match app.smart_rank_view {
                crate::app::SmartRankView::Changes => crate::app::SmartRankView::Explain,
                crate::app::SmartRankView::Explain => crate::app::SmartRankView::Changes,
            };
            app.smart_rank_scroll = 0;
        }
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => {
            app.smart_rank_scroll = app.smart_rank_scroll.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => {
            app.smart_rank_scroll = app.smart_rank_scroll.saturating_add(1);
        }
        KeyCode::PageUp => {
            app.smart_rank_scroll = app.smart_rank_scroll.saturating_sub(6);
        }
        KeyCode::PageDown => {
            app.smart_rank_scroll = app.smart_rank_scroll.saturating_add(6);
        }
        KeyCode::Home => {
            app.smart_rank_scroll = 0;
        }
        KeyCode::End => {
            app.smart_rank_scroll = usize::MAX;
        }
        _ => {}
    }
    Ok(())
}

fn handle_mod_list_preview(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Enter | KeyCode::Char(' ') => {
            if let Err(err) = app.apply_mod_list_preview() {
                app.status = format!("Mod list import failed: {err}");
                app.log_error(format!("Mod list import failed: {err}"));
            }
        }
        KeyCode::Esc => {
            app.cancel_mod_list_preview();
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            app.toggle_mod_list_destination();
        }
        KeyCode::Char('m') | KeyCode::Char('M') => {
            app.toggle_mod_list_mode();
        }
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => {
            app.mod_list_scroll = app.mod_list_scroll.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => {
            app.mod_list_scroll = app.mod_list_scroll.saturating_add(1);
        }
        KeyCode::PageUp => {
            app.mod_list_scroll = app.mod_list_scroll.saturating_sub(6);
        }
        KeyCode::PageDown => {
            app.mod_list_scroll = app.mod_list_scroll.saturating_add(6);
        }
        KeyCode::Home => {
            app.mod_list_scroll = 0;
        }
        KeyCode::End => {
            app.mod_list_scroll = usize::MAX;
        }
        _ => {}
    }
    Ok(())
}

fn handle_dependency_queue(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => app.dependency_queue_move(-1),
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => app.dependency_queue_move(1),
        KeyCode::PageUp => app.dependency_queue_move(-app.dependency_queue_page_step()),
        KeyCode::PageDown => app.dependency_queue_move(app.dependency_queue_page_step()),
        KeyCode::Home => app.dependency_queue_home(),
        KeyCode::End => app.dependency_queue_end(),
        KeyCode::Enter | KeyCode::Char(' ') => app.dependency_queue_open_selected(),
        KeyCode::Char('c') | KeyCode::Char('C') => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                app.dependency_queue_copy_link();
            } else {
                app.dependency_queue_copy_uuid();
            }
        }
        KeyCode::Esc => app.dependency_queue_cancel(),
        _ => {}
    }
    Ok(())
}

fn handle_sigillink_missing_queue(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => {
            app.sigillink_missing_queue_move(-1)
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => {
            app.sigillink_missing_queue_move(1)
        }
        KeyCode::PageUp => {
            app.sigillink_missing_queue_move(-app.sigillink_missing_queue_page_step())
        }
        KeyCode::PageDown => {
            app.sigillink_missing_queue_move(app.sigillink_missing_queue_page_step())
        }
        KeyCode::Home => app.sigillink_missing_queue_home(),
        KeyCode::End => app.sigillink_missing_queue_end(),
        KeyCode::Enter | KeyCode::Char(' ') => app.sigillink_missing_queue_open_selected(),
        KeyCode::Char('c') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.sigillink_missing_queue_copy_uuid();
        }
        KeyCode::Char('C') => app.sigillink_missing_queue_copy_uuid(),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.sigillink_missing_queue_copy_link();
        }
        KeyCode::Esc => app.sigillink_missing_queue_cancel(),
        _ => {}
    }
    Ok(())
}

fn handle_override_picker(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => app.override_picker_move(-1),
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => app.override_picker_move(1),
        KeyCode::PageUp => app.override_picker_move(-app.override_picker_page_step()),
        KeyCode::PageDown => app.override_picker_move(app.override_picker_page_step()),
        KeyCode::Home => app.override_picker_home(),
        KeyCode::End => app.override_picker_end(),
        KeyCode::Enter => app.override_picker_select(),
        KeyCode::Esc => app.override_picker_cancel(),
        _ => {}
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum SettingsItemKind {
    ActionSetupPaths,
    ActionShowPaths,
    ActionMoveSigilLinkCache,
    ActionClearFrameworkCaches,
    ActionClearSigilLinkCaches,
    ActionCopyLogTail,
    ActionCopyLogAll,
    ActionExportLogFile,
    ProfilesHeader,
    ActionExportModList,
    ActionImportModList,
    SigilLinkHeader,
    SigilLinkDebugHeader,
    SigilLinkToggle,
    SigilLinkAutoPreview,
    SigilLinkInfo,
    ActionSigilLinkSoloRank,
    ActionClearSigilLinkPins,
    ToggleModDelete,
    ToggleProfileDelete,
    ToggleAutoDeploy,
    ToggleEnableModsAfterImport,
    ToggleDeleteModFilesOnRemove,
    ToggleDependencyDownloads,
    ToggleDependencyWarnings,
    ToggleStartupDependencyNotice,
    DefaultSortColumn,
    ActionCheckUpdates,
    ActionWhatsNew,
}

#[derive(Debug, Clone, Copy)]
enum ExportMenuItemKind {
    ExportModList,
    ExportModListClipboard,
    ExportModsettings,
}

#[derive(Debug, Clone)]
struct SettingsItem {
    label: String,
    kind: SettingsItemKind,
    checked: Option<bool>,
    selectable: bool,
}

#[derive(Debug, Clone)]
struct ExportMenuItem {
    label: String,
    kind: ExportMenuItemKind,
}

fn settings_items(app: &App) -> Vec<SettingsItem> {
    let sigillink_meta = app.sigillink_rank_meta();
    let last_rank = format_rank_timestamp(sigillink_meta.last_ranked_at);
    let last_diff = format!(
        "{} moves, {} unlinked",
        sigillink_meta.last_moves, sigillink_meta.last_pins
    );
    let mut items = vec![
        SettingsItem {
            label: "Configure Game Paths".to_string(),
            kind: SettingsItemKind::ActionSetupPaths,
            checked: None,
            selectable: true,
        },
        SettingsItem {
            label: "Display SigilSmith Paths".to_string(),
            kind: SettingsItemKind::ActionShowPaths,
            checked: None,
            selectable: true,
        },
        SettingsItem {
            label: "Clear Framework Caches".to_string(),
            kind: SettingsItemKind::ActionClearFrameworkCaches,
            checked: None,
            selectable: true,
        },
        SettingsItem {
            label: "Auto Deploy".to_string(),
            kind: SettingsItemKind::ToggleAutoDeploy,
            checked: Some(app.app_config.auto_deploy_enabled),
            selectable: true,
        },
        SettingsItem {
            label: "Confirm Mod Delete".to_string(),
            kind: SettingsItemKind::ToggleModDelete,
            checked: Some(app.app_config.confirm_mod_delete),
            selectable: true,
        },
        SettingsItem {
            label: "Confirm Profile Delete".to_string(),
            kind: SettingsItemKind::ToggleProfileDelete,
            checked: Some(app.app_config.confirm_profile_delete),
            selectable: true,
        },
        SettingsItem {
            label: "Auto Dependency Downloads".to_string(),
            kind: SettingsItemKind::ToggleDependencyDownloads,
            checked: Some(app.app_config.offer_dependency_downloads),
            selectable: true,
        },
        SettingsItem {
            label: "Startup Dependency Notice".to_string(),
            kind: SettingsItemKind::ToggleStartupDependencyNotice,
            checked: Some(app.app_config.show_startup_dependency_notice),
            selectable: true,
        },
        SettingsItem {
            label: "Warn On Missing Dependencies".to_string(),
            kind: SettingsItemKind::ToggleDependencyWarnings,
            checked: Some(app.app_config.warn_missing_dependencies),
            selectable: true,
        },
        SettingsItem {
            label: "Enable Mods After Import".to_string(),
            kind: SettingsItemKind::ToggleEnableModsAfterImport,
            checked: Some(app.app_config.enable_mods_after_import),
            selectable: true,
        },
        SettingsItem {
            label: "Delete Mod Files on Remove".to_string(),
            kind: SettingsItemKind::ToggleDeleteModFilesOnRemove,
            checked: Some(app.app_config.delete_mod_files_on_remove),
            selectable: true,
        },
        SettingsItem {
            label: "Default Sort Column".to_string(),
            kind: SettingsItemKind::DefaultSortColumn,
            checked: None,
            selectable: true,
        },
        SettingsItem {
            label: update_menu_label(app),
            kind: SettingsItemKind::ActionCheckUpdates,
            checked: None,
            selectable: true,
        },
    ];

    items.extend(vec![
        SettingsItem {
            label: "SigiLink".to_string(),
            kind: SettingsItemKind::SigilLinkHeader,
            checked: None,
            selectable: false,
        },
        SettingsItem {
            label: "SigiLink Auto Ranking".to_string(),
            kind: SettingsItemKind::SigilLinkToggle,
            checked: Some(app.sigillink_ranking_enabled()),
            selectable: true,
        },
        SettingsItem {
            label: format!("Last Rank: {last_rank}"),
            kind: SettingsItemKind::SigilLinkInfo,
            checked: None,
            selectable: false,
        },
        SettingsItem {
            label: format!("Last Diff: {last_diff}"),
            kind: SettingsItemKind::SigilLinkInfo,
            checked: None,
            selectable: false,
        },
        SettingsItem {
            label: "Auto-Rank: Import + Enable".to_string(),
            kind: SettingsItemKind::SigilLinkInfo,
            checked: None,
            selectable: false,
        },
        SettingsItem {
            label: "Auto Accept Diffs".to_string(),
            kind: SettingsItemKind::SigilLinkAutoPreview,
            checked: Some(app.app_config.sigillink_auto_preview),
            selectable: true,
        },
        SettingsItem {
            label: "SigiLink Ranking Solo Run".to_string(),
            kind: SettingsItemKind::ActionSigilLinkSoloRank,
            checked: None,
            selectable: true,
        },
        SettingsItem {
            label: "Reset All SigiLink Pins".to_string(),
            kind: SettingsItemKind::ActionClearSigilLinkPins,
            checked: None,
            selectable: true,
        },
        SettingsItem {
            label: "Clear SigiLink Caches".to_string(),
            kind: SettingsItemKind::ActionClearSigilLinkCaches,
            checked: None,
            selectable: true,
        },
        SettingsItem {
            label: "Move SigiLink Cache".to_string(),
            kind: SettingsItemKind::ActionMoveSigilLinkCache,
            checked: None,
            selectable: true,
        },
        SettingsItem {
            label: "Profiles".to_string(),
            kind: SettingsItemKind::ProfilesHeader,
            checked: None,
            selectable: false,
        },
        SettingsItem {
            label: "Export Mod List".to_string(),
            kind: SettingsItemKind::ActionExportModList,
            checked: None,
            selectable: true,
        },
        SettingsItem {
            label: "Import Mod List".to_string(),
            kind: SettingsItemKind::ActionImportModList,
            checked: None,
            selectable: true,
        },
        SettingsItem {
            label: "Debug".to_string(),
            kind: SettingsItemKind::SigilLinkDebugHeader,
            checked: None,
            selectable: false,
        },
        SettingsItem {
            label: "Copy Last 200 Log Lines".to_string(),
            kind: SettingsItemKind::ActionCopyLogTail,
            checked: None,
            selectable: true,
        },
        SettingsItem {
            label: "Copy Log To Clipboard".to_string(),
            kind: SettingsItemKind::ActionCopyLogAll,
            checked: None,
            selectable: true,
        },
        SettingsItem {
            label: "Export Log File".to_string(),
            kind: SettingsItemKind::ActionExportLogFile,
            checked: None,
            selectable: true,
        },
        SettingsItem {
            label: "What's New?".to_string(),
            kind: SettingsItemKind::ActionWhatsNew,
            checked: None,
            selectable: true,
        },
    ]);

    items
}

fn export_menu_items() -> Vec<ExportMenuItem> {
    vec![
        ExportMenuItem {
            label: "Export SigilSmith Mod List (JSON)".to_string(),
            kind: ExportMenuItemKind::ExportModList,
        },
        ExportMenuItem {
            label: "Copy SigilSmith Mod List (Clipboard)".to_string(),
            kind: ExportMenuItemKind::ExportModListClipboard,
        },
        ExportMenuItem {
            label: "Export modsettings.lsx (Interop)".to_string(),
            kind: ExportMenuItemKind::ExportModsettings,
        },
    ]
}

fn update_menu_label(app: &App) -> String {
    match &app.update_status {
        UpdateStatus::Checking => "Check For Updates (Checking...)".to_string(),
        UpdateStatus::Available { info, .. } => {
            format!("Update Available: v{} (Enter To Update)", info.version)
        }
        UpdateStatus::Applied { info } => format!("Update Applied: v{} (Restart)", info.version),
        UpdateStatus::UpToDate { .. } => "Check For Updates (Latest)".to_string(),
        UpdateStatus::Failed { .. } => "Check For Updates (Failed; Retry)".to_string(),
        UpdateStatus::Skipped { .. } => "Check For Updates (See Log)".to_string(),
        UpdateStatus::Idle => "Check For Updates".to_string(),
    }
}

fn handle_export_menu(app: &mut App, key: KeyEvent) -> Result<()> {
    if app.export_menu.is_none() {
        return Ok(());
    }
    let items = export_menu_items();
    let items_len = items.len();
    match key.code {
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => {
            if let Some(menu) = &mut app.export_menu {
                menu.selected = menu.selected.saturating_sub(1);
            }
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => {
            if let Some(menu) = &mut app.export_menu {
                menu.selected = (menu.selected + 1).min(items_len.saturating_sub(1));
            }
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            let (action, profile) = match app.export_menu.as_ref() {
                Some(menu) => (
                    items.get(menu.selected).map(|item| item.kind),
                    menu.profile.clone(),
                ),
                None => return Ok(()),
            };
            if let Some(item) = action {
                match item {
                    ExportMenuItemKind::ExportModList => {
                        app.close_export_menu();
                        app.open_export_path_browser(&profile, ExportKind::ModList);
                    }
                    ExportMenuItemKind::ExportModListClipboard => {
                        if let Err(err) = app.export_mod_list_clipboard(&profile) {
                            app.status = format!("Export failed: {err}");
                            app.log_error(format!("Export failed: {err}"));
                        }
                    }
                    ExportMenuItemKind::ExportModsettings => {
                        app.close_export_menu();
                        app.open_export_path_browser(&profile, ExportKind::Modsettings);
                    }
                }
            }
        }
        KeyCode::Esc => app.close_export_menu(),
        _ => {}
    }
    Ok(())
}

fn handle_settings_menu(app: &mut App, key: KeyEvent) -> Result<()> {
    if app.settings_menu.is_none() {
        return Ok(());
    }
    let items = settings_items(app);
    let Some(menu) = &mut app.settings_menu else {
        return Ok(());
    };
    let items_len = items.len();
    match key.code {
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => {
            if items_len == 0 {
                return Ok(());
            }
            let mut next = menu.selected;
            for _ in 0..items_len {
                next = if next == 0 { items_len - 1 } else { next - 1 };
                if items[next].selectable {
                    break;
                }
            }
            menu.selected = next;
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => {
            if items_len == 0 {
                return Ok(());
            }
            let mut next = menu.selected;
            for _ in 0..items_len {
                next = (next + 1) % items_len;
                if items[next].selectable {
                    break;
                }
            }
            menu.selected = next;
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            if let Some(item) = items.get(menu.selected) {
                if !item.selectable {
                    return Ok(());
                }
                match item.kind {
                    SettingsItemKind::ActionSetupPaths => {
                        app.request_settings_menu_return();
                        app.close_settings_menu();
                        app.enter_setup_game_root();
                    }
                    SettingsItemKind::ActionShowPaths => {
                        app.request_settings_menu_return();
                        app.close_settings_menu();
                        app.open_paths_overlay();
                    }
                    SettingsItemKind::ToggleProfileDelete => {
                        if let Err(err) = app.toggle_confirm_profile_delete() {
                            app.status = format!("Settings update failed: {err}");
                            app.log_error(format!("Settings update failed: {err}"));
                        }
                    }
                    SettingsItemKind::ToggleModDelete => {
                        if let Err(err) = app.toggle_confirm_mod_delete() {
                            app.status = format!("Settings update failed: {err}");
                            app.log_error(format!("Settings update failed: {err}"));
                        }
                    }
                    SettingsItemKind::ToggleAutoDeploy => {
                        if let Err(err) = app.toggle_auto_deploy() {
                            app.status = format!("Settings update failed: {err}");
                            app.log_error(format!("Settings update failed: {err}"));
                        }
                    }
                    SettingsItemKind::ToggleEnableModsAfterImport => {
                        if let Err(err) = app.toggle_enable_mods_after_import() {
                            app.status = format!("Settings update failed: {err}");
                            app.log_error(format!("Settings update failed: {err}"));
                        }
                    }
                    SettingsItemKind::ToggleDeleteModFilesOnRemove => {
                        if let Err(err) = app.toggle_delete_mod_files_on_remove() {
                            app.status = format!("Settings update failed: {err}");
                            app.log_error(format!("Settings update failed: {err}"));
                        }
                    }
                    SettingsItemKind::DefaultSortColumn => {
                        if let Err(err) = app.cycle_default_sort_column() {
                            app.status = format!("Settings update failed: {err}");
                            app.log_error(format!("Settings update failed: {err}"));
                        }
                    }
                    SettingsItemKind::SigilLinkToggle => {
                        if let Err(err) = app.toggle_sigillink_ranking() {
                            app.status = format!("Settings update failed: {err}");
                            app.log_error(format!("Settings update failed: {err}"));
                        }
                    }
                    SettingsItemKind::SigilLinkAutoPreview => {
                        if let Err(err) = app.toggle_sigillink_auto_preview() {
                            app.status = format!("Settings update failed: {err}");
                            app.log_error(format!("Settings update failed: {err}"));
                        }
                    }
                    SettingsItemKind::ToggleDependencyDownloads => {
                        if let Err(err) = app.toggle_dependency_downloads() {
                            app.status = format!("Settings update failed: {err}");
                            app.log_error(format!("Settings update failed: {err}"));
                        }
                    }
                    SettingsItemKind::ToggleDependencyWarnings => {
                        if let Err(err) = app.toggle_dependency_warnings() {
                            app.status = format!("Settings update failed: {err}");
                            app.log_error(format!("Settings update failed: {err}"));
                        }
                    }
                    SettingsItemKind::ToggleStartupDependencyNotice => {
                        if let Err(err) = app.toggle_startup_dependency_notice() {
                            app.status = format!("Settings update failed: {err}");
                            app.log_error(format!("Settings update failed: {err}"));
                        }
                    }
                    SettingsItemKind::ActionMoveSigilLinkCache => {
                        app.request_settings_menu_return();
                        app.close_settings_menu();
                        app.open_sigillink_cache_move();
                    }
                    SettingsItemKind::ActionClearFrameworkCaches => {
                        app.clear_framework_caches();
                    }
                    SettingsItemKind::ActionClearSigilLinkCaches => {
                        app.clear_sigillink_caches();
                    }
                    SettingsItemKind::ActionClearSigilLinkPins => {
                        app.request_settings_menu_return();
                        app.close_settings_menu();
                        app.prompt_clear_sigillink_pins();
                    }
                    SettingsItemKind::ActionExportModList => {
                        let active = app.library.active_profile.clone();
                        app.request_settings_menu_return();
                        app.close_settings_menu();
                        app.enter_export_profile(&active);
                    }
                    SettingsItemKind::ActionImportModList => {
                        app.request_settings_menu_return();
                        app.close_settings_menu();
                        app.enter_import_profile();
                    }
                    SettingsItemKind::ActionSigilLinkSoloRank => {
                        app.request_settings_menu_return();
                        app.close_settings_menu();
                        app.run_sigillink_ranking_solo();
                    }
                    SettingsItemKind::ActionCopyLogTail => {
                        app.copy_log_tail_to_clipboard(200);
                    }
                    SettingsItemKind::ActionCopyLogAll => {
                        app.copy_log_to_clipboard();
                    }
                    SettingsItemKind::ActionExportLogFile => {
                        app.request_settings_menu_return();
                        app.close_settings_menu();
                        app.open_log_export();
                    }
                    SettingsItemKind::ActionWhatsNew => {
                        app.request_settings_menu_return();
                        app.close_settings_menu();
                        app.open_whats_new();
                    }
                    SettingsItemKind::ActionCheckUpdates => {
                        if matches!(app.update_status, UpdateStatus::Available { .. }) {
                            app.apply_ready_update();
                        } else {
                            app.request_update_check();
                        }
                    }
                    SettingsItemKind::SigilLinkHeader
                    | SettingsItemKind::SigilLinkDebugHeader
                    | SettingsItemKind::ProfilesHeader
                    | SettingsItemKind::SigilLinkInfo => {}
                }
            }
        }
        KeyCode::Esc => app.close_settings_menu(),
        _ => {}
    }

    Ok(())
}

fn handle_normal_mode(app: &mut App, key: KeyEvent) -> Result<()> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('f'), mods) | (KeyCode::Char('F'), mods)
            if mods.contains(KeyModifiers::CONTROL) =>
        {
            app.focus_mods();
            app.enter_mod_filter();
            return Ok(());
        }
        (KeyCode::Char('e'), mods) | (KeyCode::Char('E'), mods)
            if mods.contains(KeyModifiers::CONTROL) =>
        {
            let active = app.library.active_profile.clone();
            app.enter_export_profile(&active);
            return Ok(());
        }
        (KeyCode::Char('p'), mods) | (KeyCode::Char('P'), mods)
            if mods.contains(KeyModifiers::CONTROL) =>
        {
            app.enter_import_profile();
            return Ok(());
        }
        (KeyCode::Char('/'), _) => {
            app.focus_mods();
            app.enter_mod_filter();
            return Ok(());
        }
        (KeyCode::Left, mods) if mods.contains(KeyModifiers::CONTROL) => {
            app.focus_mods();
            app.cycle_mod_sort_column(-1);
            return Ok(());
        }
        (KeyCode::Right, mods) if mods.contains(KeyModifiers::CONTROL) => {
            app.focus_mods();
            app.cycle_mod_sort_column(1);
            return Ok(());
        }
        (KeyCode::Up, mods) if mods.contains(KeyModifiers::CONTROL) => {
            app.focus_mods();
            app.toggle_mod_sort_direction();
            return Ok(());
        }
        (KeyCode::Down, mods) if mods.contains(KeyModifiers::CONTROL) => {
            app.focus_mods();
            app.toggle_mod_sort_direction();
            return Ok(());
        }
        (KeyCode::Char('q'), _) | (KeyCode::Char('Q'), _) => app.should_quit = true,
        (KeyCode::Char('i'), _) | (KeyCode::Char('I'), _) => app.enter_import_mode(),
        (KeyCode::Char('d'), _) | (KeyCode::Char('D'), _) => {
            if let Err(err) = app.deploy() {
                app.status = format!("Deploy failed: {err}");
                app.log_error(format!("Deploy failed: {err}"));
            }
        }
        (KeyCode::Char('b'), _) | (KeyCode::Char('B'), _) => {
            if let Err(err) = app.rollback_last_backup() {
                app.status = format!("Rollback failed: {err}");
                app.log_error(format!("Rollback failed: {err}"));
            }
        }
        (KeyCode::Esc, _) if app.move_mode => {}
        (KeyCode::Esc, _) => app.toggle_settings_menu(),
        (KeyCode::Tab, _) => app.cycle_focus(),
        (KeyCode::Char('?'), _) => app.toggle_help(),
        _ => {}
    }

    match app.focus {
        Focus::Explorer => handle_explorer_mode(app, key)?,
        Focus::Mods => handle_mods_mode(app, key)?,
        Focus::Conflicts => handle_conflicts_mode(app, key)?,
        Focus::Log => handle_log_mode(app, key)?,
    }

    Ok(())
}

fn handle_explorer_mode(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => app.explorer_move_up(),
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => app.explorer_move_down(),
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('H') => app.explorer_toggle_collapse(),
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('L') => app.explorer_toggle_expand(),
        KeyCode::Enter | KeyCode::Char(' ') => {
            if let Err(err) = app.explorer_activate() {
                app.status = format!("Explorer action failed: {err}");
                app.log_error(format!("Explorer action failed: {err}"));
            }
        }
        KeyCode::Char('a') | KeyCode::Char('A') => app.enter_create_profile(),
        KeyCode::Char('r') | KeyCode::Char('R') | KeyCode::F(2) => {
            if let Some(ExplorerItem {
                kind: ExplorerItemKind::Profile { name, .. },
                disabled: false,
                ..
            }) = app.explorer_selected_item()
            {
                app.enter_rename_profile(&name);
            } else if !app.library.active_profile.is_empty() {
                let active = app.library.active_profile.clone();
                app.enter_rename_profile(&active);
            }
        }
        KeyCode::Char('c') | KeyCode::Char('C') => {
            if let Some(ExplorerItem {
                kind: ExplorerItemKind::Profile { name, .. },
                disabled: false,
                ..
            }) = app.explorer_selected_item()
            {
                app.enter_duplicate_profile(&name);
            }
        }
        KeyCode::Char('e') | KeyCode::Char('E') => {
            if let Some(ExplorerItem {
                kind: ExplorerItemKind::Profile { name, .. },
                disabled: false,
                ..
            }) = app.explorer_selected_item()
            {
                app.enter_export_profile(&name);
            } else {
                let active = app.library.active_profile.clone();
                app.enter_export_profile(&active);
            }
        }
        KeyCode::Delete | KeyCode::Backspace => {
            if let Some(ExplorerItem {
                kind: ExplorerItemKind::Profile { name, .. },
                disabled: false,
                ..
            }) = app.explorer_selected_item()
            {
                if app.app_config.confirm_profile_delete {
                    app.prompt_delete_profile(name);
                } else if let Err(err) = app.delete_profile(name) {
                    app.status = format!("Profile delete failed: {err}");
                    app.log_error(format!("Profile delete failed: {err}"));
                }
            }
        }
        KeyCode::Char('p') | KeyCode::Char('P') => app.enter_import_profile(),
        _ => {}
    }

    Ok(())
}

fn ignore_repeat_toggle(key: &KeyEvent) -> bool {
    if key.kind != KeyEventKind::Repeat {
        return false;
    }
    matches!(
        key.code,
        KeyCode::Char(' ')
            | KeyCode::Enter
            | KeyCode::Char('A')
            | KeyCode::Char('S')
            | KeyCode::Char('X')
            | KeyCode::Char('c')
            | KeyCode::Char('C')
            | KeyCode::Delete
            | KeyCode::Backspace
    )
}

fn handle_mods_mode(app: &mut App, key: KeyEvent) -> Result<()> {
    if ignore_repeat_toggle(&key) {
        return Ok(());
    }
    if app.move_mode {
        match key.code {
            KeyCode::Enter | KeyCode::Char(' ') => app.toggle_move_mode(),
            KeyCode::Esc => app.cancel_move_mode(),
            KeyCode::Char('m') | KeyCode::Char('M') => app.toggle_move_mode(),
            KeyCode::Char('k')
            | KeyCode::Char('K')
            | KeyCode::Up
            | KeyCode::Char('u')
            | KeyCode::Char('U') => {
                if app.mod_filter_active() || !app.mod_sort.is_order_default() {
                    app.prompt_move_blocked(true);
                } else {
                    app.move_selected_up();
                }
            }
            KeyCode::Char('j')
            | KeyCode::Char('J')
            | KeyCode::Down
            | KeyCode::Char('n')
            | KeyCode::Char('N') => {
                if app.mod_filter_active() || !app.mod_sort.is_order_default() {
                    app.prompt_move_blocked(true);
                } else {
                    app.move_selected_down();
                }
            }
            _ => {}
        }
        return Ok(());
    }
    match (key.code, key.modifiers) {
        (KeyCode::Char('f'), mods) | (KeyCode::Char('F'), mods)
            if mods.contains(KeyModifiers::CONTROL) =>
        {
            app.enter_mod_filter();
        }
        (KeyCode::F(12), _) => {
            app.prompt_clear_sigillink_pins();
        }
        (KeyCode::Char('r'), mods) if mods.contains(KeyModifiers::CONTROL) => {
            app.restore_sigillink_rank_for_selected();
        }
        (KeyCode::Char('/'), _) => app.enter_mod_filter(),
        (KeyCode::Char('l'), mods) | (KeyCode::Char('L'), mods)
            if mods.contains(KeyModifiers::CONTROL) =>
        {
            app.clear_mod_filter();
        }
        (KeyCode::Up, mods) if mods.contains(KeyModifiers::CONTROL) => {
            app.toggle_mod_sort_direction();
        }
        (KeyCode::Down, mods) if mods.contains(KeyModifiers::CONTROL) => {
            app.toggle_mod_sort_direction();
        }
        (KeyCode::Left, mods) if mods.contains(KeyModifiers::CONTROL) => {
            app.cycle_mod_sort_column(-1);
        }
        (KeyCode::Right, mods) if mods.contains(KeyModifiers::CONTROL) => {
            app.cycle_mod_sort_column(1);
        }
        (KeyCode::Up, mods) if mods.contains(KeyModifiers::SHIFT) => {
            app.jump_mod_selection(-10);
        }
        (KeyCode::Down, mods) if mods.contains(KeyModifiers::SHIFT) => {
            app.jump_mod_selection(10);
        }
        (KeyCode::Char('m'), _) | (KeyCode::Char('M'), _) => {
            if app.move_mode {
                app.toggle_move_mode();
            } else if app.mod_filter_active() || !app.mod_sort.is_order_default() {
                app.prompt_move_blocked(true);
            } else {
                app.toggle_move_mode();
            }
        }
        (KeyCode::Char(' '), _) | (KeyCode::Enter, _) => app.toggle_selected(),
        (KeyCode::Char('A'), _) => app.enable_visible_mods(),
        (KeyCode::Char('S'), _) => app.disable_visible_mods(),
        (KeyCode::Char('X'), _) => app.invert_visible_mods(),
        (KeyCode::Char('c'), _) | (KeyCode::Char('C'), _) => app.clear_visible_overrides(),
        (KeyCode::Delete, _) | (KeyCode::Backspace, _) => app.request_remove_selected(),
        (KeyCode::Char('k'), _) | (KeyCode::Char('K'), _) | (KeyCode::Up, _) => {
            if app.move_mode {
                if app.mod_filter_active() || !app.mod_sort.is_order_default() {
                    app.prompt_move_blocked(true);
                } else {
                    app.move_selected_up();
                }
            } else if app.selected > 0 {
                app.selected -= 1;
            }
        }
        (KeyCode::Char('j'), _) | (KeyCode::Char('J'), _) | (KeyCode::Down, _) => {
            if app.move_mode {
                if app.mod_filter_active() || !app.mod_sort.is_order_default() {
                    app.prompt_move_blocked(true);
                } else {
                    app.move_selected_down();
                }
            } else {
                app.selected += 1
            }
        }
        (KeyCode::Char('u'), _) | (KeyCode::Char('U'), _) => {
            if app.mod_filter_active() || !app.mod_sort.is_order_default() {
                app.prompt_move_blocked(false);
            } else {
                app.move_selected_up();
            }
        }
        (KeyCode::Char('n'), _) | (KeyCode::Char('N'), _) => {
            if app.mod_filter_active() || !app.mod_sort.is_order_default() {
                app.prompt_move_blocked(false);
            } else {
                app.move_selected_down();
            }
        }
        (KeyCode::Char('1'), _) => app.select_target_override(None),
        (KeyCode::Char('2'), _) => app.select_target_override(Some(TargetKind::Pak)),
        (KeyCode::Char('3'), _) => app.select_target_override(Some(TargetKind::Generated)),
        (KeyCode::Char('4'), _) => app.select_target_override(Some(TargetKind::Data)),
        (KeyCode::Char('5'), _) => app.select_target_override(Some(TargetKind::Bin)),
        (KeyCode::PageUp, _) => app.page_mods_up(),
        (KeyCode::PageDown, _) => app.page_mods_down(),
        _ => {}
    }

    Ok(())
}

fn handle_conflicts_mode(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => app.conflict_move_up(),
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => app.conflict_move_down(),
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('H') => app.cycle_conflict_winner(-1),
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('L') => app.cycle_conflict_winner(1),
        KeyCode::Char('1') => app.select_conflict_candidate(0),
        KeyCode::Char('2') => app.select_conflict_candidate(1),
        KeyCode::Char('3') => app.select_conflict_candidate(2),
        KeyCode::Char('4') => app.select_conflict_candidate(3),
        KeyCode::Char('5') => app.select_conflict_candidate(4),
        KeyCode::Char('6') => app.select_conflict_candidate(5),
        KeyCode::Char('7') => app.select_conflict_candidate(6),
        KeyCode::Char('8') => app.select_conflict_candidate(7),
        KeyCode::Char('9') => app.select_conflict_candidate(8),
        KeyCode::Char('p') | KeyCode::Char('P') => app.open_override_picker(),
        KeyCode::Char('c') | KeyCode::Char('C') => {
            if !key.modifiers.contains(KeyModifiers::CONTROL) {
                app.clear_conflict_override();
            }
        }
        KeyCode::Enter => app.apply_pending_override(),
        KeyCode::Backspace | KeyCode::Delete => app.clear_conflict_override(),
        _ => {}
    }

    Ok(())
}

fn handle_log_mode(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => app.scroll_log_up(1),
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => app.scroll_log_down(1),
        KeyCode::PageUp => app.scroll_log_up(3),
        KeyCode::PageDown => app.scroll_log_down(3),
        _ => {}
    }
    Ok(())
}

fn handle_browser_mode(app: &mut App, key: KeyEvent, browser: &mut PathBrowser) -> Result<bool> {
    let invalid_hint = match &browser.purpose {
        PathBrowserPurpose::Setup(SetupStep::GameRoot) => {
            "Not a BG3 install root (needs Data/ + bin/)."
        }
        PathBrowserPurpose::Setup(SetupStep::LarianDir) => {
            "Not a Larian data dir (needs PlayerProfiles/)."
        }
        PathBrowserPurpose::Setup(SetupStep::DownloadsDir) => "Not a folder.",
        PathBrowserPurpose::ImportProfile => "Select a file to import.",
        PathBrowserPurpose::ExportProfile { .. } => "Enter a file name to export.",
        PathBrowserPurpose::ExportLog => "Select a folder to export the log.",
        PathBrowserPurpose::SigilLinkCache { require_dev, .. } => {
            if require_dev.is_some() {
                "Select a directory on the same drive as BG3 to use SigiLink without symlinks."
            } else {
                "Select a folder for the SigiLink cache."
            }
        }
    };
    let len = browser.entries.len();
    match browser.focus {
        PathBrowserFocus::PathInput => match key.code {
            KeyCode::Esc => {
                app.remember_last_browser_dir(&browser.purpose, &browser.current);
                app.input_mode = InputMode::Normal;
                if !app.paths_ready() {
                    app.status = "Setup required: open Menu (Esc) to configure paths".to_string();
                }
                return Ok(true);
            }
            KeyCode::Tab => {
                browser.focus = PathBrowserFocus::List;
            }
            KeyCode::Enter => {
                let path = expand_tilde(&browser.path_input);
                if path.is_dir() {
                    path_browser_set_current(app, browser, path);
                    browser.focus = PathBrowserFocus::List;
                } else if app.path_browser_selectable(&browser.purpose, &path) {
                    app.apply_path_browser_selection(
                        &browser.purpose,
                        path,
                        Some(browser.path_input.as_str()),
                    )?;
                    return Ok(true);
                } else {
                    app.status = invalid_hint.to_string();
                }
            }
            KeyCode::Backspace | KeyCode::Delete => {
                path_input_backspace(app, browser);
            }
            KeyCode::Char('h') | KeyCode::Char('H')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                path_input_backspace(app, browser);
            }
            KeyCode::Char('u') | KeyCode::Char('U')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                browser.path_input.clear();
                browser.entries = app.build_path_browser_entries(
                    &browser.purpose,
                    &browser.current,
                    &browser.path_input,
                );
            }
            KeyCode::Char('c') | KeyCode::Char('C')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                copy_text_to_clipboard(app, &browser.path_input);
            }
            KeyCode::Char('v') | KeyCode::Char('V')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                paste_clipboard_into(app, &mut browser.path_input);
                browser.entries = app.build_path_browser_entries(
                    &browser.purpose,
                    &browser.current,
                    &browser.path_input,
                );
            }
            KeyCode::Char(c) => {
                if c == '\u{7f}' || c == '\u{8}' {
                    path_input_backspace(app, browser);
                } else if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                {
                    browser.path_input.push(c);
                    browser.entries = app.build_path_browser_entries(
                        &browser.purpose,
                        &browser.current,
                        &browser.path_input,
                    );
                }
            }
            _ => {}
        },
        PathBrowserFocus::List => match key.code {
            KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => {
                browser.selected = browser.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => {
                if len > 0 {
                    browser.selected = (browser.selected + 1).min(len.saturating_sub(1));
                }
            }
            KeyCode::PageUp => {
                browser.selected = browser.selected.saturating_sub(10);
            }
            KeyCode::PageDown => {
                if len > 0 {
                    browser.selected = (browser.selected + 10).min(len.saturating_sub(1));
                }
            }
            KeyCode::End => {
                if len > 0 {
                    browser.selected = len.saturating_sub(1);
                }
            }
            KeyCode::Tab => {
                browser.focus = PathBrowserFocus::PathInput;
                sync_path_input_for_browser(app, browser);
            }
            KeyCode::Left | KeyCode::Home | KeyCode::Backspace | KeyCode::Char('\u{8}') => {
                if let Some(parent) = browser.current.parent() {
                    path_browser_set_current(app, browser, parent.to_path_buf());
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(entry) = browser.entries.get(browser.selected) {
                    match entry.kind {
                        PathBrowserEntryKind::Select | PathBrowserEntryKind::SaveHere => {
                            if entry.selectable {
                                app.apply_path_browser_selection(
                                    &browser.purpose,
                                    entry.path.clone(),
                                    None,
                                )?;
                                return Ok(true);
                            }
                            app.status = invalid_hint.to_string();
                            app.set_toast(invalid_hint, ToastLevel::Warn, Duration::from_secs(2));
                        }
                        PathBrowserEntryKind::Parent | PathBrowserEntryKind::Dir => {
                            path_browser_set_current(app, browser, entry.path.clone());
                        }
                        PathBrowserEntryKind::File => {
                            if app.path_browser_selectable(&browser.purpose, &entry.path) {
                                app.apply_path_browser_selection(
                                    &browser.purpose,
                                    entry.path.clone(),
                                    None,
                                )?;
                                return Ok(true);
                            }
                            app.status = invalid_hint.to_string();
                            app.set_toast(invalid_hint, ToastLevel::Warn, Duration::from_secs(2));
                        }
                    }
                }
            }
            KeyCode::Esc => {
                app.remember_last_browser_dir(&browser.purpose, &browser.current);
                app.input_mode = InputMode::Normal;
                if !app.paths_ready() {
                    app.status = "Setup required: open Menu (Esc) to configure paths".to_string();
                }
                return Ok(true);
            }
            _ => {}
        },
    }
    Ok(false)
}

fn path_browser_set_current(app: &mut App, browser: &mut PathBrowser, path: PathBuf) {
    browser.current = path.clone();
    app.remember_last_browser_dir(&browser.purpose, &browser.current);
    sync_path_input_for_browser(app, browser);
    browser.selected = 0;
}

fn sync_path_input_for_browser(app: &App, browser: &mut PathBrowser) {
    let path_input = match &browser.purpose {
        PathBrowserPurpose::ExportProfile { profile, kind } => {
            let trimmed = browser.path_input.trim();
            let keep_name = if trimmed.is_empty() {
                None
            } else {
                let candidate = PathBuf::from(trimmed);
                if candidate.is_dir() || trimmed.ends_with(std::path::MAIN_SEPARATOR) {
                    None
                } else {
                    candidate
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(|name| name.to_string())
                }
            };
            let default_name = app
                .default_profile_export_path(profile, *kind)
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.to_string());
            let keep_name = keep_name.or(default_name);
            match keep_name {
                Some(name) => browser.current.join(name).display().to_string(),
                None => browser.current.display().to_string(),
            }
        }
        PathBrowserPurpose::ImportProfile => {
            let trimmed = browser.path_input.trim();
            let keep_name = if trimmed.is_empty() {
                None
            } else {
                let candidate = PathBuf::from(trimmed);
                if candidate.is_dir() || trimmed.ends_with(std::path::MAIN_SEPARATOR) {
                    None
                } else {
                    candidate
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(|name| name.to_string())
                }
            };
            match keep_name {
                Some(name) => browser.current.join(name).display().to_string(),
                None => browser.current.display().to_string(),
            }
        }
        _ => browser.current.display().to_string(),
    };
    browser.path_input = path_input;
    browser.entries =
        app.build_path_browser_entries(&browser.purpose, &browser.current, &browser.path_input);
}

fn path_input_backspace(app: &mut App, browser: &mut PathBrowser) {
    if browser.path_input.is_empty() {
        return;
    }
    let trimmed = browser.path_input.trim_end();
    if trimmed.ends_with(std::path::MAIN_SEPARATOR) {
        let candidate = PathBuf::from(trimmed);
        if let Some(parent) = candidate.parent() {
            path_browser_set_current(app, browser, parent.to_path_buf());
            return;
        }
    }
    browser.path_input.pop();
    browser.entries =
        app.build_path_browser_entries(&browser.purpose, &browser.current, &browser.path_input);
}

fn sanitize_paste_text(text: &str) -> String {
    text.replace('\r', " ")
        .replace('\n', " ")
        .trim()
        .to_string()
}

fn copy_text_to_clipboard(app: &mut App, text: &str) -> bool {
    if app.copy_to_clipboard(text) {
        app.status = "Path copied to clipboard".to_string();
        app.set_toast(
            "Path copied to clipboard",
            ToastLevel::Info,
            Duration::from_secs(2),
        );
        true
    } else {
        app.set_toast(
            "Clipboard unavailable",
            ToastLevel::Warn,
            Duration::from_secs(2),
        );
        false
    }
}

fn paste_clipboard_into(app: &mut App, target: &mut String) -> bool {
    let mut clipboard = match Clipboard::new() {
        Ok(clipboard) => clipboard,
        Err(err) => {
            app.status = format!("Clipboard unavailable: {err}");
            app.log_warn(format!("Clipboard unavailable: {err}"));
            return false;
        }
    };
    let text = match clipboard.get_text() {
        Ok(text) => text,
        Err(err) => {
            app.status = format!("Clipboard paste failed: {err}");
            app.log_warn(format!("Clipboard paste failed: {err}"));
            return false;
        }
    };
    let cleaned = sanitize_paste_text(&text);
    if cleaned.is_empty() {
        return false;
    }
    target.push_str(&cleaned);
    true
}

fn handle_input_mode(
    app: &mut App,
    key: KeyEvent,
    buffer: &mut String,
    purpose: InputPurpose,
    prompt: String,
    auto_submit: bool,
    last_edit_at: &mut std::time::Instant,
) -> Result<()> {
    let mut keep_editing = true;
    match key.code {
        KeyCode::Esc => {
            app.input_mode = InputMode::Normal;
            keep_editing = false;
            let cancel_message = match &purpose {
                InputPurpose::CreateProfile => "Create profile cancelled".to_string(),
                InputPurpose::RenameProfile { original } => {
                    format!("Rename cancelled: {original}")
                }
                InputPurpose::DuplicateProfile { source } => {
                    format!("Duplicate cancelled: {source}")
                }
                InputPurpose::ExportProfile { profile, .. } => {
                    format!("Export cancelled: {profile}")
                }
                InputPurpose::ImportProfile | InputPurpose::ImportPath => {
                    "Import cancelled".to_string()
                }
                InputPurpose::FilterMods => "Search cancelled".to_string(),
            };
            app.set_toast(&cancel_message, ToastLevel::Warn, Duration::from_secs(2));
            if matches!(purpose, InputPurpose::FilterMods) {
                app.cancel_mod_filter();
                app.status = "Search cancelled".to_string();
            }
        }
        KeyCode::Enter => {
            let value = buffer.trim().to_string();
            app.input_mode = InputMode::Normal;
            keep_editing = false;
            let should_submit = !value.is_empty() || matches!(purpose, InputPurpose::FilterMods);
            if should_submit {
                if let Err(err) = app.handle_submit(purpose.clone(), value) {
                    app.status = format!("Action failed: {err}");
                    app.log_error(format!("Action failed: {err}"));
                }
            }
        }
        KeyCode::Char('v') | KeyCode::Char('V')
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && key.modifiers.contains(KeyModifiers::ALT) =>
        {
            if paste_clipboard_into(app, buffer) {
                *last_edit_at = std::time::Instant::now();
            }
        }
        KeyCode::Char(c) => {
            if (c == 'h' || c == 'H') && key.modifiers.contains(KeyModifiers::CONTROL) {
                buffer.pop();
                *last_edit_at = std::time::Instant::now();
            } else if c == '\u{8}' || c == '\u{7f}' {
                buffer.pop();
                *last_edit_at = std::time::Instant::now();
            } else if key.modifiers.contains(KeyModifiers::CONTROL)
                || key.modifiers.contains(KeyModifiers::ALT)
            {
            } else {
                buffer.push(c);
                *last_edit_at = std::time::Instant::now();
            }
        }
        KeyCode::Backspace | KeyCode::Delete => {
            buffer.pop();
            *last_edit_at = std::time::Instant::now();
        }
        _ => {}
    }

    if keep_editing {
        app.input_mode = InputMode::Editing {
            prompt,
            buffer: buffer.clone(),
            purpose,
            auto_submit,
            last_edit_at: *last_edit_at,
        };
    }

    Ok(())
}

fn handle_paste(app: &mut App, text: String) -> Result<()> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(());
    }

    if app.dialog.is_some() {
        app.log_warn("Drop ignored while dialog is active".to_string());
        return Ok(());
    }

    let preview = preview_drop(trimmed);
    app.log_info(format!("Drop received: {preview}"));

    if let InputMode::Editing {
        buffer,
        last_edit_at,
        ..
    } = &mut app.input_mode
    {
        if !buffer.is_empty() {
            buffer.push(' ');
        }
        buffer.push_str(trimmed);
        *last_edit_at = std::time::Instant::now();
        return Ok(());
    }

    let paths = parse_drop_paths(trimmed);
    if paths.is_empty() {
        app.status = "Drop contained no paths".to_string();
        app.log_warn("Drop contained no paths".to_string());
        return Ok(());
    }

    app.log_info(format!("Drop parsed: {} path(s)", paths.len()));
    for path in paths {
        if let Err(err) = app.import_mod(path) {
            app.status = format!("Import failed: {err}");
            app.log_error(format!("Import failed: {err}"));
            return Ok(());
        }
    }

    Ok(())
}

fn preview_drop(text: &str) -> String {
    let mut preview = text.replace('\n', " ").replace('\r', " ");
    preview = preview.trim().to_string();
    if preview.len() > 120 {
        preview.truncate(120);
        preview.push_str("...");
    }
    preview
}

fn parse_drop_paths(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(path) = normalize_line_path(line) {
            paths.push(path);
            continue;
        }
        let parts = split_shell_like(line);
        for part in parts {
            let cleaned = strip_quotes(&part);
            let normalized = normalize_drop_path(&cleaned);
            if !normalized.is_empty() {
                paths.push(normalized);
            }
        }
    }
    paths
}

fn normalize_line_path(line: &str) -> Option<String> {
    let cleaned = strip_quotes(line);
    let normalized = normalize_drop_path(&cleaned);
    if normalized.is_empty() {
        return None;
    }
    if Path::new(&normalized).exists() {
        return Some(normalized);
    }
    None
}

fn split_shell_like(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;

    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if !current.is_empty() {
                    out.push(current.clone());
                    current.clear();
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        out.push(current);
    }

    out
}

fn strip_quotes(value: &str) -> String {
    let trimmed = value.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2 {
        if (bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'')
        {
            return trimmed[1..bytes.len() - 1].to_string();
        }
    }
    trimmed.to_string()
}

fn normalize_drop_path(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut path = if let Some(rest) = trimmed.strip_prefix("file://") {
        rest.trim_start_matches("localhost/").to_string()
    } else {
        trimmed.to_string()
    };

    if path.contains('%') {
        path = percent_decode(&path);
    }

    path
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

fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let area = frame.size();
    let theme = Theme::new();
    let bottom_height = CONFLICTS_BAR_HEIGHT.min(area.height.saturating_sub(3));
    let available = area
        .height
        .saturating_sub(HEADER_HEIGHT)
        .saturating_sub(bottom_height);
    let details_height = DETAILS_HEIGHT.min(available.saturating_sub(10).max(LOG_MIN_HEIGHT));
    let main_height = available.saturating_sub(details_height).max(6);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(HEADER_HEIGHT),
            Constraint::Length(main_height),
            Constraint::Length(details_height),
            Constraint::Length(bottom_height),
        ])
        .split(area);
    let (rows, counts, target_width, mod_width) = build_rows(app, &theme);
    let profile_label = app.active_profile_label();
    let mut status_text = app.status_line();
    if app.is_busy() {
        let spinner = status_spinner_symbol();
        if status_text.is_empty() {
            status_text = spinner.to_string();
        } else {
            status_text = format!("{status_text} {spinner}");
        }
    }
    let status_color = status_color_text(&status_text, &theme);
    let overrides_total = app.conflicts.len();
    let overrides_manual = app
        .conflicts
        .iter()
        .filter(|entry| entry.overridden)
        .count();
    let overrides_auto = overrides_total.saturating_sub(overrides_manual);
    let total_mods = counts.total;
    let enabled_mods = counts.enabled;
    let disabled_mods = total_mods.saturating_sub(enabled_mods);
    let label_style = Style::default().fg(theme.muted);
    let mut context_labels = vec![
        "Game",
        "Profile",
        "Overrides",
        "Auto-Deploy",
        "SigiLink",
        "Help",
    ];
    if !app.paths_ready() {
        context_labels.push("Setup");
    }
    let legend_rows = legend_rows(app);
    let hotkey_rows = hotkey_rows(app);
    let base_context_height = context_labels.len().saturating_add(1);
    let desired_context_height = CONTEXT_HEIGHT.saturating_add(8);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.header_bg)),
        chunks[0],
    );
    let header_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(chunks[0]);
    let mut header_line_area = header_rows[1];
    if header_line_area.width > 2 {
        header_line_area.x = header_line_area.x.saturating_add(1);
        header_line_area.width = header_line_area.width.saturating_sub(2);
    }
    let tabs_area = header_line_area;

    let title_text = format!(
        "SigilSmith | {} | {}",
        app.game_id.display_name(),
        profile_label
    );
    let initial_available = header_line_area.width as usize;
    let min_middle = 1usize;
    let min_left = 20usize;
    let max_status = 0usize;
    let mut status_width = if initial_available > min_left + min_middle {
        max_status.min(initial_available.saturating_sub(min_left + min_middle))
    } else {
        0
    };
    if status_width > initial_available {
        status_width = initial_available;
    }
    let status_area = Rect::default();
    let available = header_line_area.width as usize;
    let left_available = available.saturating_sub(status_width);
    let mut left_width = title_text.chars().count().min(left_available);
    let mut middle_width = left_available.saturating_sub(left_width);
    if middle_width < min_middle && left_width > 0 {
        let need = min_middle.saturating_sub(middle_width);
        let reduce = need.min(left_width);
        left_width = left_width.saturating_sub(reduce);
        middle_width = middle_width.saturating_add(reduce);
    }
    let header_line_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(left_width as u16),
            Constraint::Length(middle_width as u16),
            Constraint::Length(status_width as u16),
        ])
        .split(header_line_area);

    let title_prefix = "SigilSmith";
    let title_bar = " | ";
    let max_title = header_line_chunks[0].width as usize;
    let title_line = if max_title <= title_prefix.len() {
        Line::from(Span::styled(
            truncate_text(title_prefix, max_title),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
    } else {
        let suffix = format!(
            "{title_bar}{}{title_bar}{profile_label}",
            app.game_id.display_name()
        );
        let suffix_text = truncate_text(&suffix, max_title.saturating_sub(title_prefix.len()));
        Line::from(vec![
            Span::styled(
                title_prefix,
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(suffix_text, Style::default().fg(theme.text)),
        ])
    };
    if tabs_area.width > 0 {
        let tabs_line = build_focus_tabs_line(app, &theme);
        let tabs = Paragraph::new(tabs_line)
            .style(Style::default().bg(theme.header_bg))
            .alignment(Alignment::Center);
        frame.render_widget(tabs, tabs_area);
    }

    let title = Paragraph::new(title_line)
        .style(Style::default().bg(theme.header_bg))
        .alignment(Alignment::Left);
    frame.render_widget(title, header_line_chunks[0]);
    if status_area.width > 0 && status_area.height > 0 {
        let overrides_focused = app.focus == Focus::Conflicts;
        draw_status_panel(
            frame,
            app,
            &theme,
            status_area,
            &status_text,
            status_color,
            overrides_focused,
        );
    }

    let left_panel_height = chunks[1].height.saturating_add(chunks[2].height);
    let left_panel_area = Rect {
        x: chunks[1].x,
        y: chunks[1].y,
        width: SIDE_PANEL_WIDTH,
        height: left_panel_height,
    };
    let context_height = desired_context_height.min(left_panel_area.height);
    let explorer_height = left_panel_area.height.saturating_sub(context_height);
    let explorer_area = Rect {
        x: left_panel_area.x,
        y: left_panel_area.y,
        width: left_panel_area.width,
        height: explorer_height,
    };
    let context_area = Rect {
        x: left_panel_area.x,
        y: left_panel_area.y.saturating_add(explorer_height),
        width: left_panel_area.width,
        height: context_height,
    };
    let mod_stack_area = Rect {
        x: chunks[1].x.saturating_add(SIDE_PANEL_WIDTH),
        y: chunks[1].y,
        width: chunks[1].width.saturating_sub(SIDE_PANEL_WIDTH),
        height: chunks[1].height,
    };
    let details_row_area = Rect {
        x: chunks[2].x.saturating_add(SIDE_PANEL_WIDTH),
        y: chunks[2].y,
        width: chunks[2].width.saturating_sub(SIDE_PANEL_WIDTH),
        height: chunks[2].height,
    };
    let lower_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(details_row_area);

    if explorer_area.height > 0 {
        let explorer_block = theme
            .panel_tight("Explorer")
            .border_style(Style::default().fg(if app.focus == Focus::Explorer {
                theme.accent
            } else {
                theme.border
            }))
            .style(Style::default().bg(theme.subpanel_bg));
        let explorer_items = build_explorer_items(app, &theme);
        if explorer_items.is_empty() {
            let empty = Paragraph::new("No games available.")
                .style(Style::default().fg(theme.muted).bg(theme.subpanel_bg))
                .block(explorer_block)
                .alignment(Alignment::Center);
            frame.render_widget(empty, explorer_area);
        } else {
            let highlight_style = if app.focus == Focus::Explorer {
                Style::default()
                    .bg(theme.accent_soft)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().bg(theme.header_bg).fg(theme.text)
            };
            let explorer = List::new(explorer_items)
                .block(explorer_block)
                .style(Style::default().bg(theme.subpanel_bg))
                .highlight_style(highlight_style)
                .highlight_symbol("");
            let mut state = ListState::default();
            state.select(Some(app.explorer_selected));
            frame.render_stateful_widget(explorer, explorer_area, &mut state);
        }
    }

    let mod_stack_block = theme
        .block("Mod Stack")
        .border_style(Style::default().fg(if app.focus == Focus::Mods {
            theme.accent
        } else {
            theme.border
        }))
        .style(Style::default().bg(theme.mod_bg));
    let mod_stack_inner = mod_stack_block.inner(mod_stack_area);
    frame.render_widget(mod_stack_block, mod_stack_area);

    let mod_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(FILTER_HEIGHT),
            Constraint::Min(TABLE_MIN_HEIGHT),
        ])
        .split(mod_stack_inner);

    render_filter_bar(frame, app, &theme, mod_chunks[0], &counts);

    let table_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(10), Constraint::Length(1)])
        .split(mod_chunks[1]);

    let row_count = rows.len();
    let view_height = table_chunks[0].height.saturating_sub(1) as usize;
    app.mods_view_height = view_height;
    if rows.is_empty() {
        let empty = Paragraph::new("Drop a mod archive or folder to import.")
            .style(Style::default().fg(theme.muted).bg(theme.mod_bg))
            .alignment(Alignment::Center);
        frame.render_widget(empty, table_chunks[0]);
    } else {
        if table_chunks[0].height > 0 {
            let header_bg_area = Rect {
                x: table_chunks[0].x,
                y: table_chunks[0].y,
                width: table_chunks[0].width.saturating_add(table_chunks[1].width),
                height: 1,
            };
            frame.render_widget(
                Block::default().style(Style::default().bg(theme.header_bg)),
                header_bg_area,
            );
        }
        let table_width = table_chunks[0].width;
        let spacing = 0u16;
        let link_width = 2u16;
        let dep_width = 6u16;
        let date_width = 10u16;
        let mod_gap_width = 4u16;
        let created_gap_width = 2u16;
        let added_gap_width = 2u16;
        let fixed_without_mod_target = 4
            + 3
            + 3
            + 6
            + dep_width
            + link_width
            + mod_gap_width
            + created_gap_width
            + added_gap_width
            + date_width
            + date_width
            + spacing * 13;
        let max_mod = table_width.saturating_sub(fixed_without_mod_target + 1);
        let mut mod_col = mod_width as u16;
        if max_mod > 0 {
            mod_col = mod_col.min(max_mod);
        } else {
            mod_col = 1;
        }
        let fixed_without_target = fixed_without_mod_target + mod_col;
        let max_target = table_width.saturating_sub(fixed_without_target);
        let mut target_col = target_width as u16;
        if max_target > 0 {
            target_col = target_col.min(max_target);
            if max_target >= 8 {
                target_col = target_col.max(8);
            }
        }
        if target_col == 0 {
            target_col = 1;
        }
        let header = Row::new(vec![
            mod_header_cell("On", ModSortColumn::Enabled, app.mod_sort, &theme),
            mod_header_cell(" # ", ModSortColumn::Order, app.mod_sort, &theme),
            mod_header_cell(" N ", ModSortColumn::Native, app.mod_sort, &theme),
            mod_header_cell("Kind", ModSortColumn::Kind, app.mod_sort, &theme),
            mod_header_cell_static("Dep", &theme),
            mod_header_cell_static(" ", &theme),
            mod_header_cell("Mod Name", ModSortColumn::Name, app.mod_sort, &theme),
            mod_header_cell_static(" ", &theme),
            mod_header_cell("Created", ModSortColumn::Created, app.mod_sort, &theme),
            mod_header_cell_static(" ", &theme),
            mod_header_cell("Added", ModSortColumn::Added, app.mod_sort, &theme),
            mod_header_cell_static(" ", &theme),
            mod_header_cell("Target", ModSortColumn::Target, app.mod_sort, &theme),
        ])
        .style(Style::default().bg(theme.header_bg));
        let table = Table::new(
            rows,
            [
                Constraint::Length(4),
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Length(6),
                Constraint::Length(dep_width),
                Constraint::Length(link_width),
                Constraint::Length(mod_col),
                Constraint::Length(mod_gap_width),
                Constraint::Length(date_width),
                Constraint::Length(created_gap_width),
                Constraint::Length(date_width),
                Constraint::Length(added_gap_width),
                Constraint::Length(target_col),
            ],
        )
        .style(Style::default().bg(theme.mod_bg).fg(theme.text))
        .header(header)
        .column_spacing(spacing)
        .highlight_style(if app.focus == Focus::Mods {
            Style::default()
                .bg(theme.accent_soft)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(theme.header_bg).fg(theme.text)
        })
        .highlight_symbol("");

        let mut state = TableState::default();
        state.select(Some(app.selected));
        if row_count > view_height && view_height > 0 {
            let max_offset = row_count.saturating_sub(view_height);
            let mut offset = state.offset();
            if app.selected < offset {
                offset = app.selected;
            } else if app.selected >= offset.saturating_add(view_height) {
                offset = app.selected.saturating_add(1).saturating_sub(view_height);
            }
            if offset > max_offset {
                offset = max_offset;
            }
            *state.offset_mut() = offset;
        }
        let table_area = Rect {
            x: table_chunks[0].x,
            y: table_chunks[0].y,
            width: table_chunks[0].width.saturating_add(table_chunks[1].width),
            height: table_chunks[0].height,
        };
        extend_table_stripes(
            frame,
            &theme,
            table_area,
            state.offset(),
            row_count,
            view_height,
            app.selected,
            app.focus == Focus::Mods,
        );
        frame.render_stateful_widget(table, table_chunks[0], &mut state);
        if row_count > view_height && view_height > 0 {
            let scroll_len = row_count.saturating_sub(view_height).saturating_add(1);
            let mut scroll_state = ScrollbarState::new(scroll_len)
                .position(state.offset())
                .viewport_content_length(view_height);
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .track_symbol(Some("░"))
                .thumb_symbol("▓")
                .begin_symbol(None)
                .end_symbol(None)
                .track_style(Style::default().fg(theme.border))
                .thumb_style(Style::default().fg(theme.accent));
            let mut scroll_area = table_chunks[1];
            if scroll_area.height > 1 {
                scroll_area.y = scroll_area.y.saturating_add(1);
                scroll_area.height = scroll_area.height.saturating_sub(1);
            }
            frame.render_stateful_widget(scrollbar, scroll_area, &mut scroll_state);
        }
    }

    let details_focus = app.focus == Focus::Conflicts;
    let details_title = if details_focus {
        "Overrides"
    } else {
        "Details"
    };
    let details_border = if details_focus {
        theme.accent
    } else {
        theme.border
    };
    let swap_active = app.override_swap.is_some();
    let details_bg = if swap_active {
        theme.swap_bg
    } else {
        theme.log_bg
    };
    let details_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(details_border))
        .title(Span::styled(
            details_title,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(details_bg));
    let details_fill = Block::default().style(Style::default().bg(details_bg));
    let details_area = lower_chunks[0];
    frame.render_widget(details_fill, details_area);
    let details_inner = details_block.inner(details_area);
    let mut details_content_width = details_inner
        .width
        .saturating_sub(SUBPANEL_PAD_X.saturating_mul(2))
        as usize;
    let details_content_height = details_inner.height.saturating_sub(SUBPANEL_PAD_TOP) as usize;
    let conflict_metrics = if app.focus == Focus::Conflicts
        && !app.conflicts.is_empty()
        && !app.conflicts_scanning()
        && !app.conflicts_pending()
        && details_content_height > 0
    {
        let total = app.conflicts.len();
        let selected = app.conflict_selected.min(total.saturating_sub(1));
        let mut footer_len = 3usize;
        if conflict_status_label(app).is_some() {
            footer_len += 1;
        }
        let metrics = conflict_list_metrics(total, selected, details_content_height, footer_len);
        if metrics.show_scroll && details_content_width > 1 {
            details_content_width = details_content_width.saturating_sub(1);
        }
        Some(metrics)
    } else {
        None
    };
    let details_lines = build_details(app, &theme, details_content_width, details_content_height);
    let details_lines = pad_lines(
        details_lines,
        SUBPANEL_PAD_X as usize,
        SUBPANEL_PAD_TOP as usize,
    );
    let details = Paragraph::new(details_lines)
        .style(Style::default().fg(theme.text).bg(details_bg))
        .block(details_block);
    frame.render_widget(details, details_area);
    if let Some(metrics) = conflict_metrics {
        if metrics.show_scroll && details_inner.width > 0 && details_inner.height > 0 {
            let scroll_len = metrics
                .total
                .saturating_sub(metrics.list_height)
                .saturating_add(1);
            let mut scroll_state = ScrollbarState::new(scroll_len)
                .position(metrics.offset)
                .viewport_content_length(metrics.list_height);
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .track_symbol(Some("░"))
                .thumb_symbol("▓")
                .begin_symbol(None)
                .end_symbol(None)
                .track_style(Style::default().fg(theme.border))
                .thumb_style(Style::default().fg(theme.accent));
            let mut scroll_area = Rect {
                x: details_inner
                    .x
                    .saturating_add(details_inner.width.saturating_sub(1)),
                y: details_inner.y.saturating_add(1),
                width: 1,
                height: metrics.list_height as u16,
            };
            if scroll_area.y >= details_inner.y.saturating_add(details_inner.height) {
                scroll_area.height = 0;
            }
            if scroll_area.height > 0 {
                frame.render_stateful_widget(scrollbar, scroll_area, &mut scroll_state);
            }
        }
    }
    let context_block = theme
        .block("Context")
        .style(Style::default().bg(theme.subpanel_bg));
    let context_inner = context_block.inner(context_area);
    frame.render_widget(context_block, context_area);

    let available_context = context_inner.height as usize;
    let context_line_count = base_context_height;
    let context_slots = context_line_count.min(available_context);
    let legend_slots = available_context.saturating_sub(context_slots);
    let context_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(context_slots as u16),
            Constraint::Length(legend_slots as u16),
        ])
        .split(context_inner);
    let legend_block = theme.subpanel("");
    let legend_fill = Block::default().style(Style::default().bg(theme.subpanel_bg));
    frame.render_widget(legend_fill, context_chunks[1]);
    let legend_inner = legend_block.inner(context_chunks[1]);
    let legend_content_width = legend_inner
        .width
        .saturating_sub(SUBPANEL_PAD_X.saturating_mul(2)) as usize;
    let legend_content_height = legend_inner.height.saturating_sub(SUBPANEL_PAD_TOP) as usize;
    let context_label_width_raw = context_labels
        .iter()
        .map(|label| display_width(label))
        .max()
        .unwrap_or(0);
    let max_context_label = context_chunks[0].width.saturating_sub(2) as usize;
    let max_key_len = legend_rows
        .iter()
        .chain(hotkey_rows.global.iter())
        .chain(hotkey_rows.context.iter())
        .map(|row| display_width(&row.key))
        .max()
        .unwrap_or(context_label_width_raw);
    let min_action_width = 12usize;
    let max_key_width = legend_content_width.saturating_sub(min_action_width + 2);
    let mut legend_key_width = max_key_len.min(max_key_width.max(1)).max(1);
    if legend_key_width == 0 {
        legend_key_width = 1;
    }
    let align_label_width = legend_key_width.min(max_context_label).max(1);
    let context_label_width = align_label_width;
    let legend_key_width = align_label_width;

    let mut context_lines = Vec::new();
    context_lines.push(Line::from(Span::styled(
        "Active",
        Style::default().fg(theme.accent),
    )));
    let context_width = context_chunks[0].width as usize;
    let game_row = KvRow {
        label: "Game".to_string(),
        value: app.game_id.display_name().to_string(),
        label_style,
        value_style: Style::default().fg(theme.text),
    };
    context_lines.push(format_kv_line_aligned(
        &game_row,
        context_width,
        context_label_width,
    ));
    let profile_name = app.active_profile_label();
    let profile_style = Style::default().fg(if app.is_renaming_active_profile() {
        theme.warning
    } else {
        theme.text
    });
    let profile_counts = format!("{disabled_mods} / {enabled_mods} | {total_mods} ");
    let profile_spans = vec![
        Span::styled(disabled_mods.to_string(), Style::default().fg(theme.muted)),
        Span::styled(" / ", Style::default().fg(theme.muted)),
        Span::styled(enabled_mods.to_string(), Style::default().fg(theme.success)),
        Span::styled(" | ", Style::default().fg(theme.muted)),
        Span::styled(total_mods.to_string(), Style::default().fg(theme.accent)),
        Span::styled(" ", Style::default().fg(theme.muted)),
    ];
    let overrides_left = format!("Auto ({overrides_auto})");
    let overrides_right = format!("│ Manual ({overrides_manual}) ");
    let overrides_right_spans = vec![
        Span::styled("│ ", Style::default().fg(theme.muted)),
        Span::styled(
            format!("Manual ({overrides_manual})"),
            Style::default().fg(if overrides_manual > 0 {
                theme.warning
            } else {
                theme.muted
            }),
        ),
        Span::styled(" ", Style::default().fg(theme.muted)),
    ];
    let value_width = context_width.saturating_sub(context_label_width + 2);
    let desired_width = split_value_width(&profile_name, &profile_counts)
        .max(split_value_width(&overrides_left, &overrides_right));
    let split_width = context_label_width + 2 + desired_width.min(value_width);
    context_lines.push(format_kv_line_split(
        "Profile",
        label_style,
        &profile_name,
        profile_style,
        &profile_counts,
        profile_spans,
        split_width,
        context_label_width,
        Style::default().fg(theme.muted),
    ));
    context_lines.push(format_kv_line_split(
        "Overrides",
        label_style,
        &overrides_left,
        Style::default().fg(theme.success),
        &overrides_right,
        overrides_right_spans,
        split_width,
        context_label_width,
        Style::default().fg(theme.muted),
    ));
    let auto_deploy_enabled = app.app_config.auto_deploy_enabled;
    let auto_row = KvRow {
        label: "Auto-Deploy".to_string(),
        value: if auto_deploy_enabled {
            "ON".to_string()
        } else {
            "OFF".to_string()
        },
        label_style,
        value_style: Style::default().fg(if auto_deploy_enabled {
            theme.success
        } else {
            theme.muted
        }),
    };
    context_lines.push(format_kv_line_aligned(
        &auto_row,
        context_width,
        context_label_width,
    ));
    let sigilink_line = if app.sigillink_ranking_enabled() {
        let pin_count = app.sigillink_pin_count();
        let value_parts = vec![
            ("ON".to_string(), Style::default().fg(theme.success)),
            ("  ".to_string(), Style::default().fg(theme.muted)),
            ("Unlinked: ".to_string(), Style::default().fg(theme.muted)),
            (pin_count.to_string(), Style::default().fg(theme.warning)),
        ];
        format_kv_line_aligned_spans(
            "SigiLink",
            label_style,
            value_parts,
            context_width,
            context_label_width,
        )
    } else {
        format_kv_line_aligned(
            &KvRow {
                label: "SigiLink".to_string(),
                value: "OFF".to_string(),
                label_style,
                value_style: Style::default().fg(theme.muted),
            },
            context_width,
            context_label_width,
        )
    };
    context_lines.push(sigilink_line);
    let help_row = KvRow {
        label: "Help".to_string(),
        value: "? Shortcuts".to_string(),
        label_style,
        value_style: Style::default().fg(theme.accent),
    };
    context_lines.push(format_kv_line_aligned(
        &help_row,
        context_width,
        context_label_width,
    ));
    if !app.paths_ready() {
        let setup_row = KvRow {
            label: "Setup".to_string(),
            value: "Open Menu (Esc) To Configure".to_string(),
            label_style,
            value_style: Style::default().fg(theme.warning),
        };
        context_lines.push(format_kv_line_aligned(
            &setup_row,
            context_width,
            context_label_width,
        ));
    }
    let context_widget =
        Paragraph::new(context_lines).style(Style::default().fg(theme.text).bg(theme.subpanel_bg));
    frame.render_widget(context_widget, context_chunks[0]);

    let legend_lines = build_legend_lines(
        &legend_rows,
        &hotkey_rows,
        &theme,
        legend_content_width,
        legend_content_height,
        legend_key_width,
        app.hotkey_fade_active(),
    );
    let legend_lines = pad_lines(
        legend_lines,
        SUBPANEL_PAD_X as usize,
        SUBPANEL_PAD_TOP as usize,
    );
    let overrides = Paragraph::new(legend_lines)
        .style(Style::default().fg(theme.text).bg(theme.subpanel_bg))
        .block(legend_block);
    frame.render_widget(overrides, context_chunks[1]);

    let log_area = lower_chunks[1];
    let overrides_focused = app.focus == Focus::Conflicts;
    let log_bg = theme.log_bg;
    let log_block = theme
        .panel("Log")
        .border_style(Style::default().fg(if app.focus == Focus::Log {
            theme.accent
        } else {
            theme.border
        }))
        .style(Style::default().bg(log_bg));
    let log_inner = log_block.inner(log_area);
    frame.render_widget(log_block, log_area);
    let mut status_area = status_badge_area(log_inner, &status_text);
    if status_area.height > 0 {
        let band_y = log_inner
            .y
            .saturating_add(log_inner.height.saturating_sub(status_area.height));
        status_area.y = band_y;
    }
    let log_content = if status_area.height > 0 {
        Rect {
            x: log_inner.x,
            y: log_inner.y,
            width: log_inner.width,
            height: log_inner.height.saturating_sub(status_area.height),
        }
    } else {
        log_inner
    };
    let log_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(10), Constraint::Length(1)])
        .split(log_content);
    let log_total = app.logs.len();
    let log_view = log_chunks[0].height.max(1) as usize;
    let max_scroll = log_total.saturating_sub(log_view);
    if app.log_scroll > max_scroll {
        app.log_scroll = max_scroll;
    }
    let log_lines = build_log_lines(app, &theme, log_view);
    if log_content.height > 0 {
        let log = Paragraph::new(log_lines).style(Style::default().fg(theme.text).bg(log_bg));
        frame.render_widget(log, log_chunks[0]);
    }
    let scroll = app.log_scroll;
    let log_start = log_total.saturating_sub(log_view + scroll);
    if log_total > log_view && log_view > 0 && log_content.height > 0 {
        let scroll_len = log_total.saturating_sub(log_view).saturating_add(1);
        let mut scroll_state = ScrollbarState::new(scroll_len)
            .position(log_start)
            .viewport_content_length(log_view);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .track_symbol(Some("░"))
            .thumb_symbol("▓")
            .begin_symbol(None)
            .end_symbol(None)
            .track_style(Style::default().fg(theme.border))
            .thumb_style(Style::default().fg(theme.accent));
        frame.render_stateful_widget(scrollbar, log_chunks[1], &mut scroll_state);
    }

    if status_area.width > 0 && status_area.height > 0 {
        draw_status_panel(
            frame,
            app,
            &theme,
            status_area,
            &status_text,
            status_color,
            overrides_focused,
        );
    }

    let status_row = chunks[3];
    if status_row.height > 0 && status_row.width > 0 {
        let conflict_area = Rect {
            x: chunks[2].x.saturating_add(SIDE_PANEL_WIDTH),
            y: status_row.y,
            width: chunks[2].width.saturating_sub(SIDE_PANEL_WIDTH),
            height: status_row.height,
        };
        if conflict_area.width > 0 {
            let conflict_bg = theme.log_bg;
            let conflict_block = Block::default()
                .borders(Borders::NONE)
                .style(Style::default().bg(conflict_bg))
                .padding(Padding {
                    left: 1,
                    right: 1,
                    top: 0,
                    bottom: 0,
                });
            frame.render_widget(conflict_block.clone(), conflict_area);
            let conflict_inner = conflict_block.inner(conflict_area);
            if conflict_inner.height > 0 && conflict_inner.width > 0 {
                let line_y = conflict_inner.y + (conflict_inner.height.saturating_sub(1) / 2);
                let line_area = Rect {
                    x: conflict_inner.x,
                    y: line_y,
                    width: conflict_inner.width,
                    height: 1,
                };
                if overrides_focused && line_area.width > 0 {
                    let bar_width = conflict_line_width(app, &theme, line_area.width as usize);
                    if bar_width > 0 {
                        let bar_area = Rect {
                            x: line_area.x,
                            y: line_area.y,
                            width: bar_width.min(line_area.width),
                            height: 1,
                        };
                        frame.render_widget(
                            Block::default().style(Style::default().bg(theme.row_alt_bg)),
                            bar_area,
                        );
                    }
                }
                let conflict_line = build_conflict_banner(app, &theme, line_area.width as usize);
                let conflicts = Paragraph::new(conflict_line)
                    .style(Style::default().bg(conflict_bg))
                    .alignment(Alignment::Left);
                frame.render_widget(conflicts, line_area);
            }
        }
    }

    if app.dependency_queue_active() {
        draw_dependency_queue(frame, app, &theme);
    }
    if app.override_picker_active() {
        draw_override_picker(frame, app, &theme);
    }
    if app.sigillink_missing_queue_active() {
        draw_sigillink_missing_queue(frame, app, &theme);
    }
    if app.dialog.is_some() {
        draw_dialog(frame, app, &theme);
    }
    if let InputMode::Browsing(browser) = &app.input_mode {
        draw_path_browser(frame, app, &theme, browser);
    }
    if app.smart_rank_preview.is_some() {
        draw_smart_rank_preview(frame, app, &theme);
    }
    if app.mod_list_preview.is_some() {
        draw_mod_list_preview(frame, app, &theme);
    }
    if app.export_menu.is_some() {
        draw_export_menu(frame, app, &theme);
    }
    if app.settings_menu.is_some() {
        draw_settings_menu(frame, app, &theme);
    }
    if app.paths_overlay_open {
        draw_paths_overlay(frame, app, &theme);
    }
    if app.help_open {
        draw_help_menu(frame, app, &theme);
    }
    if app.whats_new_open {
        draw_whats_new(frame, app, &theme);
    }
    draw_import_overlay(frame, app, &theme);
    draw_startup_overlay(frame, app, &theme);
    draw_toast(frame, app, &theme, chunks[1]);
}

fn current_filter_value(app: &App) -> (String, bool) {
    match &app.input_mode {
        InputMode::Editing {
            purpose: InputPurpose::FilterMods,
            buffer,
            ..
        } => (buffer.clone(), true),
        _ => (app.mod_filter.clone(), false),
    }
}

fn render_filter_bar(
    frame: &mut Frame<'_>,
    app: &App,
    theme: &Theme,
    area: Rect,
    _counts: &ModCounts,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.header_bg)),
        area,
    );
    let (filter_value, editing) = current_filter_value(app);
    let trimmed = filter_value.trim();
    let filter_active = !trimmed.is_empty();
    let placeholder = if editing {
        "Type to search..."
    } else {
        "Search mods..."
    };
    let value_text = if filter_active { trimmed } else { placeholder };
    let value_style = if editing {
        Style::default()
            .fg(theme.warning)
            .add_modifier(Modifier::BOLD)
    } else if filter_active {
        Style::default().fg(theme.accent)
    } else {
        Style::default().fg(theme.muted)
    };
    let show_clear = app.mod_filter_active();
    let search_hint = if editing {
        "Enter search | Esc cancel"
    } else {
        "Ctrl+F or /"
    };
    let hint_style = if editing {
        Style::default().fg(theme.warning)
    } else {
        Style::default().fg(theme.muted)
    };
    let bar_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);
    let search_area = bar_chunks[0];
    let sort_label = format!(
        "Sort: {} {}",
        app.mod_sort.column_label(),
        app.mod_sort.direction_arrow()
    );
    let sort_label = format!(" {sort_label} ");
    let search_right_width = sort_label.chars().count() as u16;
    let search_right_width = search_right_width.min(search_area.width);
    let sort_style = if app.mod_sort.is_order_default() {
        Style::default().fg(theme.muted)
    } else {
        Style::default()
            .fg(theme.header_bg)
            .bg(theme.section_bg)
            .add_modifier(Modifier::BOLD)
    };
    let min_search_width = 12u16;
    let search_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(min_search_width),
            Constraint::Length(search_right_width),
        ])
        .split(search_area);
    let left_line = Line::from(vec![
        Span::styled(
            " Search",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" | ", Style::default().fg(theme.border)),
        Span::styled(value_text.to_string(), value_style),
        Span::styled(" | ", Style::default().fg(theme.border)),
        Span::styled(search_hint, hint_style),
    ]);
    let left = Paragraph::new(left_line)
        .style(Style::default().bg(theme.header_bg))
        .alignment(Alignment::Left);
    frame.render_widget(left, search_chunks[0]);
    let right_label = truncate_text(&sort_label, search_chunks[1].width as usize);
    let right = Paragraph::new(Line::from(Span::styled(right_label, sort_style)))
        .style(Style::default().bg(theme.header_bg))
        .alignment(Alignment::Right);
    frame.render_widget(right, search_chunks[1]);

    if bar_chunks[1].height > 0 && show_clear {
        let meta_area = bar_chunks[1];
        let meta_left = Paragraph::new(Line::from(Span::styled(
            "Clear search: Ctrl+L",
            Style::default()
                .fg(theme.header_bg)
                .bg(theme.accent_soft)
                .add_modifier(Modifier::BOLD),
        )))
        .style(Style::default().bg(theme.header_bg))
        .alignment(Alignment::Left);
        frame.render_widget(meta_left, meta_area);
    }
}

fn build_focus_tabs_line(app: &App, theme: &Theme) -> Line<'static> {
    let tabs = [
        ("Explorer", Focus::Explorer),
        ("Mods", Focus::Mods),
        ("Overrides", Focus::Conflicts),
        ("Log", Focus::Log),
    ];
    let mut spans = Vec::new();
    for (index, (label, focus)) in tabs.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" | ", Style::default().fg(theme.muted)));
        }
        let style = if app.focus == *focus {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted)
        };
        spans.push(Span::styled((*label).to_string(), style));
    }
    Line::from(spans)
}

fn status_color_text(status: &str, theme: &Theme) -> Color {
    let lower = status.to_lowercase();
    if lower.contains("failed")
        || lower.contains("error")
        || lower.contains("denied")
        || lower.contains("blocked")
    {
        return theme.error;
    }
    if lower.contains("missing")
        || lower.contains("invalid")
        || lower.contains("not found")
        || lower.contains("warning")
    {
        return theme.warning;
    }
    if lower.contains("ready") || lower.contains("complete") {
        return theme.success;
    }
    theme.accent
}

fn status_spinner_symbol() -> &'static str {
    const FRAMES: [&str; 8] = ["⠁", "⠂", "⠄", "⡀", "⢀", "⠠", "⠐", "⠈"];
    let tick = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        / 120;
    let idx = (tick as usize) % FRAMES.len();
    FRAMES[idx]
}

fn status_badge_area(area: Rect, status_text: &str) -> Rect {
    if area.width == 0 || area.height == 0 {
        return Rect::default();
    }
    let text_width = status_text.chars().count() as u16;
    let desired = text_width.saturating_add(4);
    let max_width = STATUS_WIDTH.min(area.width);
    if max_width == 0 {
        return Rect::default();
    }
    let min_width = 18u16;
    let width = if max_width < min_width {
        max_width
    } else {
        desired.clamp(min_width, max_width)
    };
    let height = if area.height >= 1 { 1 } else { area.height };
    if height == 0 {
        return Rect::default();
    }
    Rect {
        x: area.x + area.width.saturating_sub(width),
        y: area.y + area.height.saturating_sub(height),
        width,
        height,
    }
}

fn draw_status_panel(
    frame: &mut Frame<'_>,
    _app: &App,
    theme: &Theme,
    area: Rect,
    status_text: &str,
    status_color: Color,
    overrides_focused: bool,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let status_bg = theme.log_bg;
    let status_block = Block::default()
        .borders(Borders::NONE)
        .style(Style::default().bg(status_bg))
        .padding(Padding {
            left: 1,
            right: 1,
            top: 0,
            bottom: 0,
        });
    frame.render_widget(status_block.clone(), area);
    let status_inner = status_block.inner(area);
    let _ = overrides_focused;
    let status_text = truncate_text(status_text, status_inner.width as usize);
    let text_y = status_inner
        .y
        .saturating_add(status_inner.height.saturating_sub(1));
    let text_area = Rect {
        x: status_inner.x,
        y: text_y,
        width: status_inner.width,
        height: 1,
    };
    let status_widget = Paragraph::new(Line::from(Span::styled(
        status_text,
        Style::default().fg(status_color).bg(status_bg),
    )))
    .alignment(Alignment::Center);
    frame.render_widget(status_widget, text_area);
}

fn build_log_lines(app: &App, theme: &Theme, height: usize) -> Vec<Line<'static>> {
    if height == 0 {
        return Vec::new();
    }

    if app.logs.is_empty() {
        return vec![Line::from(Span::styled(
            "No recent events.",
            Style::default().fg(theme.muted),
        ))];
    }

    let total = app.logs.len();
    let view = height.max(1);
    let max_scroll = total.saturating_sub(view);
    let scroll = app.log_scroll.min(max_scroll);
    let start = total.saturating_sub(view + scroll);
    let end = (start + view).min(total);

    app.logs[start..end]
        .iter()
        .map(|entry| {
            let (label, color) = match entry.level {
                LogLevel::Info => ("[i]", theme.accent),
                LogLevel::Warn => ("[!]", theme.warning),
                LogLevel::Error => ("[x]", theme.error),
            };
            Line::from(vec![
                Span::styled(
                    label,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(entry.message.clone(), Style::default().fg(theme.text)),
            ])
        })
        .collect()
}

fn extend_table_stripes(
    frame: &mut Frame<'_>,
    theme: &Theme,
    area: Rect,
    offset: usize,
    row_count: usize,
    view_height: usize,
    selected: usize,
    focused: bool,
) {
    if area.width == 0 || area.height <= 1 || view_height == 0 {
        return;
    }
    let body_height = area.height.saturating_sub(1) as usize;
    let visible = row_count
        .saturating_sub(offset)
        .min(body_height)
        .min(view_height);
    if visible == 0 {
        return;
    }
    let start_y = area.y + 1;
    for i in 0..visible {
        let row_index = offset + i;
        let bg = if focused && row_index == selected {
            Some(theme.accent_soft)
        } else if row_index % 2 == 1 {
            Some(theme.row_alt_bg)
        } else {
            None
        };
        if let Some(color) = bg {
            let stripe_area = Rect {
                x: area.x,
                y: start_y + i as u16,
                width: area.width,
                height: 1,
            };
            frame.render_widget(
                Block::default().style(Style::default().bg(color)),
                stripe_area,
            );
        }
    }
}

fn build_conflict_banner(app: &App, theme: &Theme, width: usize) -> Line<'static> {
    if width == 0 {
        return Line::from("");
    }

    let label = "Overrides: ";
    if width <= label.len() {
        return Line::from(Span::styled(
            truncate_text(label.trim_end(), width),
            Style::default().fg(theme.accent),
        ));
    }

    let focused = app.focus == Focus::Conflicts;
    let label_style = if focused {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.accent)
    };
    let available = width.saturating_sub(label.len());

    if app.conflicts_scanning() {
        return Line::from(vec![
            Span::styled(label, label_style),
            Span::styled("scanning...", Style::default().fg(theme.muted)),
        ]);
    }
    if app.conflicts_pending() {
        return Line::from(vec![
            Span::styled(label, label_style),
            Span::styled("scan queued...", Style::default().fg(theme.muted)),
        ]);
    }
    if app.conflicts.is_empty() {
        return Line::from(vec![
            Span::styled(label, label_style),
            Span::styled("none", Style::default().fg(theme.muted)),
        ]);
    }

    let total = app.conflicts.len();
    let selected_index = app.conflict_selected.min(total.saturating_sub(1));
    let conflict = &app.conflicts[selected_index];
    let manual_count = app
        .conflicts
        .iter()
        .filter(|entry| entry.overridden)
        .count();
    let auto_count = total.saturating_sub(manual_count);
    let auto_text = format!("Auto ({auto_count})");
    let manual_text = format!("Manual ({manual_count})");

    let auto_style =
        Style::default()
            .fg(theme.success)
            .add_modifier(if focused && !conflict.overridden {
                Modifier::BOLD
            } else {
                Modifier::empty()
            });
    let manual_style = Style::default()
        .fg(if manual_count > 0 {
            theme.warning
        } else {
            theme.muted
        })
        .add_modifier(if focused && conflict.overridden {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });
    let sep_style = Style::default().fg(theme.muted);
    let hint_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);

    if !focused {
        let mut short_auto = auto_text.clone();
        let mut short_manual = if manual_count > 0 {
            manual_text.clone()
        } else {
            String::new()
        };
        let mut sep = if short_manual.is_empty() { "" } else { " | " };
        let mut total_len = short_auto.len() + sep.len() + short_manual.len();
        if total_len > available {
            short_auto = format!("A({auto_count})");
            if !short_manual.is_empty() {
                short_manual = format!("M({manual_count})");
                sep = " ";
                total_len = short_auto.len() + sep.len() + short_manual.len();
            } else {
                total_len = short_auto.len();
            }
        }
        if total_len > available && !short_manual.is_empty() {
            short_manual.clear();
            sep = "";
            total_len = short_auto.len();
        }
        if total_len > available {
            short_auto = truncate_text(&short_auto, available);
        }
        let mut spans = vec![Span::styled(label, label_style)];
        spans.push(Span::styled(short_auto, auto_style));
        if !short_manual.is_empty() {
            spans.push(Span::styled(sep, sep_style));
            spans.push(Span::styled(short_manual, manual_style));
        }
        return Line::from(spans);
    }

    let mut short_auto = auto_text.clone();
    let mut short_manual = manual_text.clone();
    let mut sep = " | ";
    let mut hint = " ←/→ cycle  ↑/↓ choose";
    let index_text = format!("{}/{} ", selected_index + 1, total);
    let index_len = index_text.chars().count();
    let mut remaining = available.saturating_sub(index_len);
    if hint.chars().count() > remaining {
        hint = "";
    }
    let hint_len = hint.chars().count();
    remaining = remaining.saturating_sub(hint_len);
    let mut total_len = short_auto.len() + sep.len() + short_manual.len();
    if total_len > remaining {
        short_auto = format!("A({auto_count})");
        short_manual = format!("M({manual_count})");
        sep = " ";
        total_len = short_auto.len() + sep.len() + short_manual.len();
    }
    if total_len > remaining {
        short_manual.clear();
        sep = "";
        total_len = short_auto.len();
    }
    if total_len > remaining {
        short_auto = truncate_text(&short_auto, remaining);
    }

    let mut spans = Vec::new();
    spans.push(Span::styled(label, label_style));
    spans.push(Span::styled(index_text, Style::default().fg(theme.accent)));
    spans.push(Span::styled(short_auto, auto_style));
    if !short_manual.is_empty() {
        spans.push(Span::styled(sep, sep_style));
        spans.push(Span::styled(short_manual, manual_style));
    }
    if !hint.is_empty() {
        spans.push(Span::styled(hint, hint_style));
    }

    Line::from(spans)
}

fn conflict_line_width(app: &App, _theme: &Theme, width: usize) -> u16 {
    if width == 0 {
        return 0;
    }
    let label = "Overrides: ";
    if width <= label.len() {
        return width as u16;
    }
    if app.conflicts_scanning() || app.conflicts_pending() || app.conflicts.is_empty() {
        return width as u16;
    }
    let total = app.conflicts.len();
    let selected_index = app.conflict_selected.min(total.saturating_sub(1));
    let manual_count = app
        .conflicts
        .iter()
        .filter(|entry| entry.overridden)
        .count();
    let auto_count = total.saturating_sub(manual_count);
    let auto_text = format!("Auto ({auto_count})");
    let manual_text = format!("Manual ({manual_count})");

    let available = width.saturating_sub(label.len());
    if !matches!(app.focus, Focus::Conflicts) {
        let mut short_auto = auto_text.clone();
        let mut short_manual = if manual_count > 0 {
            manual_text.clone()
        } else {
            String::new()
        };
        let mut total_len =
            short_auto.len() + if short_manual.is_empty() { 0 } else { 3 } + short_manual.len();
        if total_len > available {
            short_auto = format!("A({auto_count})");
            if !short_manual.is_empty() {
                short_manual = format!("M({manual_count})");
                total_len = short_auto.len() + 1 + short_manual.len();
            } else {
                total_len = short_auto.len();
            }
        }
        if total_len > available && !short_manual.is_empty() {
            short_manual.clear();
            total_len = short_auto.len();
        }
        if total_len > available {
            short_auto = truncate_text(&short_auto, available);
            total_len = short_auto.chars().count();
        }
        return (label.len() + total_len).min(width) as u16;
    }

    let hint = " ←/→ cycle  ↑/↓ choose";
    let index_text = format!("{}/{} ", selected_index + 1, total);
    let index_len = index_text.chars().count();
    let mut remaining = available.saturating_sub(index_len);
    let hint_len = if hint.chars().count() > remaining {
        0
    } else {
        hint.chars().count()
    };
    remaining = remaining.saturating_sub(hint_len);
    let mut short_auto = auto_text;
    let mut short_manual = manual_text;
    let mut total_len = short_auto.len() + 3 + short_manual.len();
    if total_len > remaining {
        short_auto = format!("A({auto_count})");
        short_manual = format!("M({manual_count})");
        total_len = short_auto.len() + 1 + short_manual.len();
    }
    if total_len > remaining {
        short_manual.clear();
        total_len = short_auto.len();
    }
    if total_len > remaining {
        short_auto = truncate_text(&short_auto, remaining);
        total_len = short_auto.chars().count();
    }
    let hint_used = if hint_len > 0 { hint_len } else { 0 };
    let full_len = label.len() + index_len + total_len + hint_used;
    full_len.min(width) as u16
}

fn draw_dialog(frame: &mut Frame<'_>, app: &mut App, theme: &Theme) {
    let Some(dialog) = &mut app.dialog else {
        return;
    };

    let area = frame.size();
    let message_lines = build_dialog_message_lines(dialog, theme);

    let has_cancel = matches!(dialog.kind, DialogKind::DeleteMod { .. });
    let yes_selected = matches!(dialog.choice, DialogChoice::Yes);
    let no_selected = if has_cancel {
        matches!(dialog.choice, DialogChoice::No)
    } else {
        !yes_selected
    };
    let cancel_selected = matches!(dialog.choice, DialogChoice::Cancel);
    let yes_style = if yes_selected {
        Style::default()
            .fg(Color::Black)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text)
    };
    let no_style = if no_selected {
        Style::default()
            .fg(Color::Black)
            .bg(theme.warning)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text)
    };

    let buttons = if has_cancel {
        let cancel_style = if cancel_selected {
            Style::default()
                .fg(Color::Black)
                .bg(theme.muted)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text)
        };
        Line::from(vec![
            Span::raw(" "),
            Span::styled(" Cancel ".to_string(), cancel_style),
            Span::raw("   "),
            Span::styled(format!(" {} ", dialog.yes_label), yes_style),
            Span::raw("   "),
            Span::styled(format!(" {} ", dialog.no_label), no_style),
        ])
    } else {
        Line::from(vec![
            Span::raw(" "),
            Span::styled(format!(" {} ", dialog.yes_label), yes_style),
            Span::raw("   "),
            Span::styled(format!(" {} ", dialog.no_label), no_style),
        ])
    };

    let header_lines = vec![
        Line::from(Span::styled(
            dialog.title.clone(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    let mut footer_lines = Vec::new();
    if dialog.toggle.is_some() || dialog.toggle_alt.is_some() {
        footer_lines.push(Line::from(""));
    }
    if let Some(toggle) = &dialog.toggle {
        let marker = if toggle.checked { "[x]" } else { "[ ]" };
        footer_lines.push(Line::from(vec![
            Span::styled(marker, Style::default().fg(theme.accent)),
            Span::raw(" "),
            Span::styled(toggle.label.clone(), Style::default().fg(theme.text)),
        ]));
        footer_lines.push(Line::from(Span::styled(
            "Press D to toggle",
            Style::default().fg(theme.muted),
        )));
    }
    if let Some(toggle) = &dialog.toggle_alt {
        let marker = if toggle.checked { "[x]" } else { "[ ]" };
        footer_lines.push(Line::from(vec![
            Span::styled(marker, Style::default().fg(theme.accent)),
            Span::raw(" "),
            Span::styled(toggle.label.clone(), Style::default().fg(theme.text)),
        ]));
        footer_lines.push(Line::from(Span::styled(
            "Press A to toggle",
            Style::default().fg(theme.muted),
        )));
    }
    footer_lines.push(Line::from(""));
    footer_lines.push(buttons);

    let mut max_line = 0usize;
    for line in header_lines
        .iter()
        .chain(message_lines.iter())
        .chain(footer_lines.iter())
    {
        let width = line.to_string().chars().count();
        if width > max_line {
            max_line = width;
        }
    }
    let max_width = area.width.saturating_sub(2).max(1);
    let width = (max_line as u16 + 6).clamp(38, max_width.min(72));
    let content_height = header_lines
        .len()
        .saturating_add(message_lines.len())
        .saturating_add(footer_lines.len())
        .max(1) as u16;
    let mut height = content_height + 2;
    if height < 8 {
        height = 8;
    }
    if height > area.height.saturating_sub(2) {
        height = area.height.saturating_sub(2);
    }
    let (outer_area, dialog_area) = padded_modal(area, width, height, 2, 1);
    render_modal_backdrop(frame, outer_area, theme);
    let dialog_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent_soft))
        .style(Style::default().bg(theme.header_bg));
    let inner = dialog_block.inner(dialog_area);
    frame.render_widget(dialog_block, dialog_area);

    let header_height = header_lines.len() as u16;
    let footer_height = footer_lines.len() as u16;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Min(1),
            Constraint::Length(footer_height),
        ])
        .split(inner);

    let header_widget = Paragraph::new(header_lines)
        .style(Style::default().fg(theme.text).bg(theme.header_bg))
        .alignment(Alignment::Center);
    frame.render_widget(header_widget, chunks[0]);

    let body_area = chunks[1];
    let body_height = body_area.height.max(1) as usize;
    let max_scroll = message_lines.len().saturating_sub(body_height);
    if dialog.scroll > max_scroll {
        dialog.scroll = max_scroll;
    }

    let show_scroll = max_scroll > 0 && body_area.width > 4;
    let body_chunks = if show_scroll {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(body_area)
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(0)])
            .split(body_area)
    };

    let body_widget = Paragraph::new(message_lines)
        .scroll((dialog.scroll as u16, 0))
        .style(Style::default().fg(theme.text).bg(theme.header_bg))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false });
    frame.render_widget(body_widget, body_chunks[0]);

    if show_scroll && body_chunks[1].width > 0 {
        let scroll_len = max_scroll.saturating_add(1);
        let mut scroll_state = ScrollbarState::new(scroll_len)
            .position(dialog.scroll)
            .viewport_content_length(body_height);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .track_symbol(Some("░"))
            .thumb_symbol("▓")
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"))
            .track_style(Style::default().fg(theme.border))
            .thumb_style(Style::default().fg(theme.accent));
        frame.render_stateful_widget(scrollbar, body_chunks[1], &mut scroll_state);
    }

    let footer_widget = Paragraph::new(footer_lines)
        .style(Style::default().fg(theme.text).bg(theme.header_bg))
        .alignment(Alignment::Center);
    frame.render_widget(footer_widget, chunks[2]);
}

fn build_dialog_message_lines(dialog: &crate::app::Dialog, theme: &Theme) -> Vec<Line<'static>> {
    match &dialog.kind {
        DialogKind::DeleteProfile { name } => {
            let line1 = Line::from(vec![
                Span::styled("Delete Profile \"", Style::default().fg(theme.text)),
                Span::styled(name.clone(), Style::default().fg(theme.text)),
                Span::styled("\" from ", Style::default().fg(theme.text)),
                Span::styled(
                    "SigilSmith",
                    Style::default()
                        .fg(theme.success)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("?", Style::default().fg(theme.text)),
            ]);
            let line2 = Line::from(Span::styled(
                "This action cannot be undone.",
                Style::default().fg(theme.warning),
            ));
            vec![line1, line2]
        }
        DialogKind::DeleteMod {
            name,
            native,
            dependents,
            ..
        } => {
            let mut lines = Vec::new();
            if *native {
                let line1 = Line::from(vec![
                    Span::styled("Remove native mod \"", Style::default().fg(theme.text)),
                    Span::styled(name.clone(), Style::default().fg(theme.text)),
                    Span::styled("\"?", Style::default().fg(theme.text)),
                ]);
                let line2 = Line::from(vec![
                    Span::styled(
                        "Remove keeps the .pak in the Larian Mods folder and leaves a ",
                        Style::default().fg(theme.text),
                    ),
                    Span::styled("!", Style::default().fg(theme.warning)),
                    Span::styled(
                        "ghost",
                        Style::default()
                            .fg(theme.text)
                            .add_modifier(Modifier::CROSSED_OUT),
                    ),
                    Span::styled(" entry.", Style::default().fg(theme.text)),
                ]);
                let line3 = Line::from(Span::styled(
                    "Remove & update cache deletes the .pak from the Larian Mods folder.",
                    Style::default().fg(theme.warning),
                ));
                let line4 = Line::from(Span::styled(
                    "Unsubscribe in-game to stop updates.",
                    Style::default().fg(theme.muted),
                ));
                lines.extend([line1, line2, Line::from(""), line3, line4]);
            } else {
                let line1 = Line::from(vec![
                    Span::styled("Remove mod \"", Style::default().fg(theme.text)),
                    Span::styled(name.clone(), Style::default().fg(theme.text)),
                    Span::styled("\"?", Style::default().fg(theme.text)),
                ]);
                let line2 = Line::from(vec![
                    Span::styled(
                        "This will remove it from the ",
                        Style::default().fg(theme.text),
                    ),
                    Span::styled(
                        "SigilSmith Library",
                        Style::default()
                            .fg(theme.success)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(".", Style::default().fg(theme.text)),
                ]);
                let line3 = Line::from(vec![
                    Span::styled(
                        "Remove keeps files in SigilSmith storage and leaves a ",
                        Style::default().fg(theme.text),
                    ),
                    Span::styled("!", Style::default().fg(theme.warning)),
                    Span::styled(
                        "ghost",
                        Style::default()
                            .fg(theme.text)
                            .add_modifier(Modifier::CROSSED_OUT),
                    ),
                    Span::styled(" entry.", Style::default().fg(theme.text)),
                ]);
                let line4 = Line::from(Span::styled(
                    "Remove & update cache deletes stored files from SigilSmith storage.",
                    Style::default().fg(theme.warning),
                ));
                lines.extend([line1, line2, line3, Line::from(""), line4]);
            }
            if !dependents.is_empty() {
                lines.push(Line::from(""));
                lines.extend(delete_dependents_lines(dependents, theme));
            }
            lines
        }
        DialogKind::DisableDependents { dependents, .. } => {
            dependency_action_lines("Will disable", dependents, theme)
        }
        DialogKind::EnableRequiredDependencies { dependencies, .. } => {
            dependency_action_lines("Will enable", dependencies, theme)
        }
        _ => dialog
            .message
            .lines()
            .map(|line| Line::from(line.to_string()))
            .collect(),
    }
}

fn dependency_action_lines(
    action: &str,
    dependents: &[crate::app::DependentMod],
    theme: &Theme,
) -> Vec<Line<'static>> {
    if dependents.is_empty() {
        return Vec::new();
    }
    let highlight_style = Style::default()
        .fg(theme.header_bg)
        .bg(theme.warning)
        .add_modifier(Modifier::BOLD);
    let mut lines = Vec::new();
    let max_list = 4usize;
    if dependents.len() == 1 {
        lines.push(Line::from(Span::styled(
            format!("{action}: {}", dependents[0].name),
            highlight_style,
        )));
        return lines;
    }
    lines.push(Line::from(Span::styled(
        format!("{action} {} mods:", dependents.len()),
        highlight_style,
    )));
    for dependent in dependents.iter().take(max_list) {
        lines.push(Line::from(Span::styled(
            dependent.name.clone(),
            Style::default().fg(theme.warning),
        )));
    }
    if dependents.len() > max_list {
        lines.push(Line::from(Span::styled(
            format!("...and {} more", dependents.len() - max_list),
            Style::default().fg(theme.warning),
        )));
    }
    lines
}

fn delete_dependents_lines(
    dependents: &[crate::app::DependentMod],
    theme: &Theme,
) -> Vec<Line<'static>> {
    dependency_action_lines("Will disable", dependents, theme)
}

fn padded_modal(area: Rect, width: u16, height: u16, pad_x: u16, pad_y: u16) -> (Rect, Rect) {
    let outer_width = width
        .saturating_add(pad_x.saturating_mul(2))
        .min(area.width);
    let outer_height = height
        .saturating_add(pad_y.saturating_mul(2))
        .min(area.height);
    let x = area.x + (area.width.saturating_sub(outer_width)) / 2;
    let y = area.y + (area.height.saturating_sub(outer_height)) / 2;
    let outer = Rect::new(x, y, outer_width, outer_height);
    let pad_x = pad_x.min(outer_width / 2);
    let pad_y = pad_y.min(outer_height / 2);
    let inner_width = outer_width.saturating_sub(pad_x.saturating_mul(2)).max(1);
    let inner_height = outer_height.saturating_sub(pad_y.saturating_mul(2)).max(1);
    let inner = Rect::new(outer.x + pad_x, outer.y + pad_y, inner_width, inner_height);
    (outer, inner)
}

fn render_modal_backdrop(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.overlay_scrim)),
        area,
    );
}

fn draw_settings_menu(frame: &mut Frame<'_>, app: &App, theme: &Theme) {
    let Some(menu) = &app.settings_menu else {
        return;
    };

    let area = frame.size();
    let mut lines = build_settings_menu_lines(app, theme, menu.selected, None);
    let mut max_line = 0usize;
    for line in &lines {
        let width = line.to_string().chars().count();
        if width > max_line {
            max_line = width;
        }
    }
    let content_height = lines.len().max(1) as u16;
    let mut height = content_height + 3;
    if height < 10 {
        height = 10;
    }
    if height > area.height.saturating_sub(2) {
        height = area.height.saturating_sub(2);
    }
    let max_width = area.width.saturating_sub(2).max(1);
    let width = (max_line as u16 + 8).clamp(40, max_width.min(70));
    let inner_width = width.saturating_sub(2) as usize;
    lines = build_settings_menu_lines(app, theme, menu.selected, Some(inner_width));
    let (outer_area, menu_area) = padded_modal(area, width, height, 2, 1);

    render_modal_backdrop(frame, outer_area, theme);
    let menu_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent_soft))
        .style(Style::default().bg(theme.header_bg))
        .title(Span::styled(
            "Menu",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    let menu_widget = Paragraph::new(lines)
        .block(menu_block)
        .style(Style::default().fg(theme.text));
    frame.render_widget(menu_widget, menu_area);
}

fn draw_paths_overlay(frame: &mut Frame<'_>, app: &App, theme: &Theme) {
    let area = frame.size();
    let root = if app.config.game_root.as_os_str().is_empty() {
        "<not set>".to_string()
    } else {
        app.config.game_root.display().to_string()
    };
    let user_dir = if app.config.larian_dir.as_os_str().is_empty() {
        "<not set>".to_string()
    } else {
        app.config.larian_dir.display().to_string()
    };
    let config_path = app.config.data_dir.join("config.json");
    let label_style = Style::default().fg(theme.muted);
    let value_style = Style::default().fg(theme.text);
    let lines = vec![
        Line::from(vec![
            Span::styled("Root: ", label_style),
            Span::styled(root, value_style),
        ]),
        Line::from(vec![
            Span::styled("User: ", label_style),
            Span::styled(user_dir, value_style),
        ]),
        Line::from(vec![
            Span::styled("Config: ", label_style),
            Span::styled(config_path.display().to_string(), value_style),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Enter/Esc: close",
            Style::default().fg(theme.muted),
        )),
    ];

    let mut max_line = 0usize;
    for line in &lines {
        let width = line.to_string().chars().count();
        if width > max_line {
            max_line = width;
        }
    }
    let max_width = area.width.saturating_sub(4).max(1);
    let width = (max_line as u16 + 6).clamp(40, max_width.min(100));
    let content_height = lines.len().max(1) as u16;
    let mut height = content_height + 2;
    if height < 8 {
        height = 8;
    }
    if height > area.height.saturating_sub(2) {
        height = area.height.saturating_sub(2);
    }
    let (outer_area, modal) = padded_modal(area, width, height, 2, 1);

    render_modal_backdrop(frame, outer_area, theme);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent_soft))
        .style(Style::default().bg(theme.header_bg))
        .title(Span::styled(
            "SigilSmith Paths",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    let widget = Paragraph::new(lines)
        .block(block)
        .style(Style::default().fg(theme.text))
        .wrap(Wrap { trim: false })
        .alignment(Alignment::Left);
    frame.render_widget(widget, modal);
}

fn draw_export_menu(frame: &mut Frame<'_>, app: &App, theme: &Theme) {
    let Some(menu) = &app.export_menu else {
        return;
    };

    let area = frame.size();
    let lines = build_export_menu_lines(theme, menu);
    let mut max_line = 0usize;
    for line in &lines {
        let width = line.to_string().chars().count();
        if width > max_line {
            max_line = width;
        }
    }
    let content_height = lines.len().max(1) as u16;
    let mut height = content_height + 3;
    if height < 10 {
        height = 10;
    }
    if height > area.height.saturating_sub(2) {
        height = area.height.saturating_sub(2);
    }
    let max_width = area.width.saturating_sub(2).max(1);
    let width = (max_line as u16 + 6).clamp(38, max_width.min(64));
    let (outer_area, menu_area) = padded_modal(area, width, height, 2, 1);

    render_modal_backdrop(frame, outer_area, theme);
    let menu_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent_soft))
        .style(Style::default().bg(theme.header_bg))
        .title(Span::styled(
            "Export",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    let menu_widget = Paragraph::new(lines)
        .block(menu_block)
        .style(Style::default().fg(theme.text));
    frame.render_widget(menu_widget, menu_area);
}

fn draw_help_menu(frame: &mut Frame<'_>, app: &mut App, theme: &Theme) {
    if !app.help_open {
        return;
    }

    let area = frame.size();
    let max_width = area.width.saturating_sub(4).max(1);
    let width = max_width.clamp(52, 96);
    let mut height = 14;
    let content_width = width.saturating_sub(2) as usize;
    let mut lines = build_help_lines(theme, content_width);
    let content_height = lines.len().max(1) as u16;
    height = height.max(content_height + 2);
    if height < 14 {
        height = 14;
    }
    let max_height = area.height.saturating_sub(2).max(1);
    if height > max_height {
        height = max_height;
    }
    let (outer_area, modal) = padded_modal(area, width, height, 2, 1);

    let view_height = modal.height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(view_height);
    if app.help_scroll > max_scroll {
        app.help_scroll = max_scroll;
    }

    let show_scroll = max_scroll > 0;

    render_modal_backdrop(frame, outer_area, theme);
    let help_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent_soft))
        .style(Style::default().bg(theme.header_bg))
        .title(Span::styled(
            "Help",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    let help_inner = help_block.inner(modal);
    frame.render_widget(help_block, modal);

    let help_chunks = if show_scroll {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(help_inner)
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(0)])
            .split(help_inner)
    };
    let content_width = help_chunks[0].width.max(1) as usize;
    lines = build_help_lines(theme, content_width);
    let view_height = help_chunks[0].height.max(1) as usize;
    let max_scroll = lines.len().saturating_sub(view_height);
    if app.help_scroll > max_scroll {
        app.help_scroll = max_scroll;
    }
    let help_widget = Paragraph::new(lines)
        .scroll((app.help_scroll as u16, 0))
        .style(Style::default().fg(theme.text).bg(theme.header_bg));
    frame.render_widget(help_widget, help_chunks[0]);
    if show_scroll && help_chunks[1].width > 0 {
        let scroll_len = max_scroll.saturating_add(1);
        let mut scroll_state = ScrollbarState::new(scroll_len)
            .position(app.help_scroll)
            .viewport_content_length(view_height);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .track_symbol(Some("░"))
            .thumb_symbol("▓")
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"))
            .track_style(Style::default().fg(theme.border))
            .thumb_style(Style::default().fg(theme.accent));
        frame.render_stateful_widget(scrollbar, help_chunks[1], &mut scroll_state);
    }
}

fn draw_whats_new(frame: &mut Frame<'_>, app: &mut App, theme: &Theme) {
    if !app.whats_new_open {
        return;
    }

    let area = frame.size();
    let max_width = area.width.saturating_sub(4).max(1);
    let width = max_width.clamp(64, 110);
    let max_height = area.height.saturating_sub(2).max(1);
    let content_width = width.saturating_sub(2) as usize;
    let mut lines = build_whats_new_lines(theme, content_width);
    let content_height = lines.len().max(1) as u16;
    let mut height = content_height.saturating_add(3).max(14);
    if height > max_height {
        height = max_height;
    }
    let (outer_area, modal) = padded_modal(area, width, height, 2, 1);

    render_modal_backdrop(frame, outer_area, theme);
    let panel_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent_soft))
        .style(Style::default().bg(theme.header_bg))
        .title(Span::styled(
            "What's New?!",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = panel_block.inner(modal);
    frame.render_widget(panel_block, modal);

    let footer_height = 1;
    let body_height = inner.height.saturating_sub(footer_height);
    let body_rect = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: body_height,
    };
    let footer_rect = Rect {
        x: inner.x,
        y: inner.y.saturating_add(body_height),
        width: inner.width,
        height: footer_height,
    };

    let view_height = body_rect.height.max(1) as usize;
    let max_scroll = lines.len().saturating_sub(view_height);
    if app.whats_new_scroll > max_scroll {
        app.whats_new_scroll = max_scroll;
    }
    let show_scroll = max_scroll > 0;
    let body_chunks = if show_scroll {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(body_rect)
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(0)])
            .split(body_rect)
    };
    let content_width = body_chunks[0].width.max(1) as usize;
    lines = build_whats_new_lines(theme, content_width);
    let view_height = body_chunks[0].height.max(1) as usize;
    let max_scroll = lines.len().saturating_sub(view_height);
    if app.whats_new_scroll > max_scroll {
        app.whats_new_scroll = max_scroll;
    }
    let body_widget = Paragraph::new(lines)
        .scroll((app.whats_new_scroll as u16, 0))
        .style(Style::default().fg(theme.text).bg(theme.header_bg));
    frame.render_widget(body_widget, body_chunks[0]);
    if show_scroll && body_chunks[1].width > 0 {
        let scroll_len = max_scroll.saturating_add(1);
        let mut scroll_state = ScrollbarState::new(scroll_len)
            .position(app.whats_new_scroll)
            .viewport_content_length(view_height);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .track_symbol(Some("░"))
            .thumb_symbol("▓")
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"))
            .track_style(Style::default().fg(theme.border))
            .thumb_style(Style::default().fg(theme.accent));
        frame.render_stateful_widget(scrollbar, body_chunks[1], &mut scroll_state);
    }

    let remaining = app.whats_new_remaining_secs();
    let footer_text = if remaining > 0 {
        format!("Continue in {remaining}s")
    } else {
        "Enter/Esc to close".to_string()
    };
    let footer_widget = Paragraph::new(Line::from(Span::styled(
        footer_text,
        Style::default().fg(theme.muted),
    )))
    .alignment(Alignment::Right);
    frame.render_widget(footer_widget, footer_rect);
}

fn dependency_status_label(status: DependencyStatus) -> &'static str {
    match status {
        DependencyStatus::Missing => "Missing",
        DependencyStatus::Waiting => "Waiting",
        DependencyStatus::Downloaded => "Ready",
        DependencyStatus::Skipped => "Skipped",
    }
}

fn dependency_status_style(theme: &Theme, status: DependencyStatus) -> Style {
    match status {
        DependencyStatus::Missing => Style::default().fg(theme.warning),
        DependencyStatus::Waiting => Style::default().fg(theme.accent),
        DependencyStatus::Downloaded => Style::default().fg(theme.success),
        DependencyStatus::Skipped => Style::default().fg(theme.muted),
    }
}

fn draw_dependency_queue(frame: &mut Frame<'_>, app: &mut App, theme: &Theme) {
    let (total, missing) = {
        let Some(queue) = app.dependency_queue() else {
            return;
        };
        let total = queue
            .items
            .iter()
            .filter(|item| !item.is_override_action())
            .count();
        let missing = queue
            .items
            .iter()
            .filter(|item| !item.is_override_action() && item.status == DependencyStatus::Missing)
            .count();
        (total, missing)
    };

    let area = frame.size();

    let max_width = area.width.saturating_sub(4).max(1);
    let width = max_width.clamp(60, 112);
    let max_height = area.height.saturating_sub(4).max(1);
    let height = max_height.clamp(16, 26);
    let (outer_area, modal) = padded_modal(area, width, height, 2, 1);

    render_modal_backdrop(frame, outer_area, theme);
    let panel_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.overlay_border))
        .style(Style::default().bg(theme.overlay_panel_bg))
        .title(Span::styled(
            "Missing dependencies",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = panel_block.inner(modal);
    frame.render_widget(panel_block, modal);

    let mut header_lines = Vec::new();
    let header_text = if app.dependency_queue_enable_pending() {
        "Resolve missing dependencies before enabling mods."
    } else {
        "Resolve missing dependencies before import continues."
    };
    header_lines.push(Line::from(Span::styled(
        header_text,
        Style::default().fg(theme.text),
    )));
    let summary = format!("Missing {missing} of {total}");
    header_lines.push(Line::from(Span::styled(
        truncate_text(&summary, inner.width as usize),
        Style::default().fg(theme.muted),
    )));
    header_lines.push(Line::from(Span::styled(
        "Download the dependency and drag it into SigilSmith to continue.",
        Style::default().fg(theme.muted),
    )));

    let header_height = header_lines.len() as u16 + 1;
    let footer_height = 2u16;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Min(4),
            Constraint::Length(footer_height),
        ])
        .split(inner);

    let header_widget =
        Paragraph::new(header_lines).style(Style::default().bg(theme.overlay_panel_bg));
    frame.render_widget(header_widget, chunks[0]);

    let list_area = chunks[1];
    let list_width = list_area.width as usize;
    let item_height = 3usize;
    let view_items = (list_area.height as usize / item_height).max(1);
    app.set_dependency_queue_view(view_items);
    let (items, total_items, selected) = {
        let Some(queue) = app.dependency_queue() else {
            return;
        };
        let total_items = queue.items.len();
        let selected = queue.selected;
        let mut items = Vec::new();
        let mut dep_index = 0usize;
        let tick = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            / 200;
        let pulse = (tick % 5) as usize;
        let label = "OVERRIDE DEPENDENCIES";
        let label_chars: Vec<char> = label.chars().collect();
        let mask_one = if label_chars.len() > 2 {
            label_chars[1..label_chars.len() - 1]
                .iter()
                .collect::<String>()
        } else {
            label.to_string()
        };
        let mask_two = if label_chars.len() > 4 {
            label_chars[2..label_chars.len() - 2]
                .iter()
                .collect::<String>()
        } else {
            mask_one.clone()
        };
        let override_lines = match pulse {
            0 => [
                format!(">  {label}  <"),
                format!(" > {label} < "),
                format!("  >{label}<  "),
            ],
            1 => [
                format!(" > {label} < "),
                format!("  >{label}<  "),
                format!("   █{mask_one}█   "),
            ],
            2 => [
                format!("  >{label}<  "),
                format!("   █{mask_one}█   "),
                format!("    █{mask_two}█    "),
            ],
            3 => [
                format!("   █{mask_one}█   "),
                format!("    █{mask_two}█    "),
                format!("   █{mask_one}█   "),
            ],
            _ => [
                format!("    █{mask_two}█    "),
                format!("   █{mask_one}█   "),
                format!("  >{label}<  "),
            ],
        };
        for item in queue.items.iter() {
            if item.is_override_action() {
                let override_style = Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD);
                let center_override = |value: &str| -> String {
                    let trimmed = truncate_text(value, list_width);
                    let len = trimmed.chars().count();
                    if list_width <= len {
                        return trimmed;
                    }
                    let pad_total = list_width - len;
                    let left = pad_total / 2;
                    let right = pad_total - left;
                    format!("{}{}{}", " ".repeat(left), trimmed, " ".repeat(right))
                };
                let line_one = Line::from(Span::styled(
                    center_override(&override_lines[0]),
                    override_style,
                ));
                let line_two = Line::from(Span::styled(
                    center_override(&override_lines[1]),
                    override_style,
                ));
                let line_three = Line::from(Span::styled(
                    center_override(&override_lines[2]),
                    override_style,
                ));
                items.push(ListItem::new(vec![line_one, line_two, line_three]));
                continue;
            }

            dep_index = dep_index.saturating_add(1);
            let status_label = dependency_status_label(item.status);
            let status_text = format!("{status_label:<9}");
            let status_style = dependency_status_style(theme, item.status);
            let index_label = format!("{:>2}. ", dep_index);
            let label_width = list_width
                .saturating_sub(status_text.chars().count() + 1 + index_label.chars().count());
            let label_value = if item.display_label.trim().is_empty() {
                item.label.clone()
            } else {
                item.display_label.clone()
            };
            let label_text = truncate_text(&label_value, label_width);
            let label_line = Line::from(vec![
                Span::styled(status_text, status_style),
                Span::raw(" "),
                Span::styled(index_label, Style::default().fg(theme.muted)),
                Span::styled(label_text, Style::default().fg(theme.text)),
            ]);
            let uuid_text = item
                .uuid
                .as_ref()
                .map(|uuid| format!("UUID: {uuid}"))
                .unwrap_or_else(|| "UUID: unknown".to_string());
            let uuid_line = Line::from(Span::styled(
                truncate_text(&uuid_text, list_width),
                Style::default().fg(theme.muted),
            ));
            let required_by = if item.required_by.is_empty() {
                "Required by: Unknown".to_string()
            } else {
                format!("Required by: {}", item.required_by.join(", "))
            };
            let link_label = if item.link.is_some() {
                "Link: available".to_string()
            } else {
                "Link: none".to_string()
            };
            let search_label = if item.search_link.is_some() {
                format!("Search: {}", item.search_label)
            } else {
                "Search: none".to_string()
            };
            let details = format!("{required_by} | {link_label} | {search_label}");
            let required_line = Line::from(Span::styled(
                truncate_text(&details, list_width),
                Style::default().fg(theme.muted),
            ));
            items.push(ListItem::new(vec![label_line, uuid_line, required_line]));
        }
        (items, total_items, selected)
    };

    let highlight_style = Style::default()
        .bg(theme.accent_soft)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD);
    let list = List::new(items)
        .style(Style::default().bg(theme.overlay_panel_bg))
        .highlight_style(highlight_style)
        .highlight_symbol("");

    let mut state = ListState::default();
    let mut offset = 0usize;
    if total_items > view_items {
        if selected >= view_items {
            offset = selected + 1 - view_items;
        }
        let max_offset = total_items.saturating_sub(view_items);
        if offset > max_offset {
            offset = max_offset;
        }
    }
    if total_items > 0 {
        state.select(Some(selected));
        *state.offset_mut() = offset;
    }

    let show_scroll = total_items > view_items;
    let list_chunks = if show_scroll {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(list_area)
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(0)])
            .split(list_area)
    };
    frame.render_stateful_widget(list, list_chunks[0], &mut state);

    if show_scroll && list_chunks[1].width > 0 {
        let scroll_len = total_items.saturating_sub(view_items).saturating_add(1);
        let mut scroll_state = ScrollbarState::new(scroll_len)
            .position(offset)
            .viewport_content_length(view_items);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .track_symbol(Some("░"))
            .thumb_symbol("▓")
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"))
            .track_style(Style::default().fg(theme.border))
            .thumb_style(Style::default().fg(theme.accent));
        frame.render_stateful_widget(scrollbar, list_chunks[1], &mut scroll_state);
    }

    let key_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(theme.muted);
    let footer_line_one = Line::from(vec![
        Span::styled("↑/↓", key_style),
        Span::styled(" Move  ", text_style),
        Span::styled("PgUp/PgDn", key_style),
        Span::styled(" Jump  ", text_style),
        Span::styled("[Enter]", key_style),
        Span::styled(" Open/override  ", text_style),
        Span::styled("[Ctrl+C]", key_style),
        Span::styled(" Copy link  ", text_style),
    ]);
    let footer_line_two = vec![
        Span::styled("[C]", key_style),
        Span::styled(" Copy UUID  ", text_style),
        Span::styled("[Esc]", key_style),
        Span::styled(" Cancel", text_style),
    ];
    let footer_line_two = Line::from(footer_line_two);
    let footer_widget = Paragraph::new(vec![footer_line_one, footer_line_two])
        .style(Style::default().bg(theme.overlay_panel_bg))
        .alignment(Alignment::Left);
    frame.render_widget(footer_widget, chunks[2]);
}

fn draw_override_picker(frame: &mut Frame<'_>, app: &mut App, theme: &Theme) {
    let (items, selected, conflict_index) = {
        let Some(picker) = app.override_picker() else {
            return;
        };
        (picker.items.clone(), picker.selected, picker.conflict_index)
    };
    let (conflict_path, conflict_winner_id) = {
        let Some(conflict) = app.conflicts.get(conflict_index) else {
            return;
        };
        (conflict.relative_path.clone(), conflict.winner_id.clone())
    };

    let area = frame.size();
    let max_width = area.width.saturating_sub(6).max(1);
    let width = max_width.clamp(52, 96);
    let max_height = area.height.saturating_sub(6).max(1);
    let height = max_height.clamp(10, 20);
    let (outer_area, modal) = padded_modal(area, width, height, 2, 1);

    render_modal_backdrop(frame, outer_area, theme);
    let panel_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.overlay_border))
        .style(Style::default().bg(theme.overlay_panel_bg))
        .title(Span::styled(
            "Override candidates",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = panel_block.inner(modal);
    frame.render_widget(panel_block, modal);

    let file_name = conflict_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_string())
        .unwrap_or_else(|| conflict_path.to_string_lossy().to_string());
    let header_lines = vec![
        Line::from(Span::styled(
            truncate_text(&format!("File: {file_name}"), inner.width as usize),
            Style::default().fg(theme.text),
        )),
        Line::from(Span::styled(
            "Select the winner for this file.",
            Style::default().fg(theme.muted),
        )),
    ];
    let header_height = header_lines.len() as u16 + 1;
    let footer_height = 2u16;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Min(4),
            Constraint::Length(footer_height),
        ])
        .split(inner);
    let header_widget =
        Paragraph::new(header_lines).style(Style::default().bg(theme.overlay_panel_bg));
    frame.render_widget(header_widget, chunks[0]);

    let list_area = chunks[1];
    let list_width = list_area.width as usize;
    let view_items = list_area.height as usize;
    app.set_override_picker_view(view_items.max(1));

    let pending_winner = app
        .pending_overrides
        .get(&conflict_index)
        .map(|pending| pending.winner_id.as_str());
    let winner_id = pending_winner.unwrap_or(conflict_winner_id.as_str());

    let list_items: Vec<ListItem<'_>> = items
        .iter()
        .map(|item| {
            let selected = item.mod_id == winner_id;
            let marker = if selected { "[x]" } else { "[ ]" };
            let label_width = list_width.saturating_sub(marker.len() + 1);
            let label = truncate_text(&item.name, label_width);
            ListItem::new(Line::from(vec![
                Span::styled(marker.to_string(), Style::default().fg(theme.muted)),
                Span::raw(" "),
                Span::styled(label, Style::default().fg(theme.text)),
            ]))
        })
        .collect();

    let highlight_style = Style::default()
        .bg(theme.accent_soft)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD);
    let list = List::new(list_items)
        .style(Style::default().bg(theme.overlay_panel_bg))
        .highlight_style(highlight_style)
        .highlight_symbol("");

    let total_items = items.len();
    let mut state = ListState::default();
    let mut offset = 0usize;
    if total_items > view_items {
        if selected >= view_items {
            offset = selected + 1 - view_items;
        }
        let max_offset = total_items.saturating_sub(view_items);
        if offset > max_offset {
            offset = max_offset;
        }
    }
    if total_items > 0 {
        state.select(Some(selected));
        *state.offset_mut() = offset;
    }

    let show_scroll = total_items > view_items;
    let list_chunks = if show_scroll {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(list_area)
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(0)])
            .split(list_area)
    };
    frame.render_stateful_widget(list, list_chunks[0], &mut state);
    if show_scroll && list_chunks[1].width > 0 {
        let scroll_len = total_items.saturating_sub(view_items).saturating_add(1);
        let mut scroll_state = ScrollbarState::new(scroll_len)
            .position(offset)
            .viewport_content_length(view_items);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .track_symbol(Some("░"))
            .thumb_symbol("▓")
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"))
            .track_style(Style::default().fg(theme.border))
            .thumb_style(Style::default().fg(theme.accent));
        frame.render_stateful_widget(scrollbar, list_chunks[1], &mut scroll_state);
    }

    let key_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(theme.muted);
    let footer_line_one = Line::from(vec![
        Span::styled("↑/↓", key_style),
        Span::styled(" Move  ", text_style),
        Span::styled("[Enter]", key_style),
        Span::styled(" Select  ", text_style),
        Span::styled("[Esc]", key_style),
        Span::styled(" Cancel", text_style),
    ]);
    let footer_widget = Paragraph::new(vec![footer_line_one])
        .style(Style::default().bg(theme.overlay_panel_bg))
        .alignment(Alignment::Left);
    frame.render_widget(footer_widget, chunks[2]);
}

fn draw_sigillink_missing_queue(frame: &mut Frame<'_>, app: &mut App, theme: &Theme) {
    let (total, trigger) = {
        let Some(queue) = app.sigillink_missing_queue() else {
            return;
        };
        (queue.items.len(), queue.trigger)
    };

    let area = frame.size();
    let max_width = area.width.saturating_sub(4).max(1);
    let width = max_width.clamp(56, 104);
    let max_height = area.height.saturating_sub(4).max(1);
    let height = max_height.clamp(14, 24);
    let (outer_area, modal) = padded_modal(area, width, height, 2, 1);

    render_modal_backdrop(frame, outer_area, theme);
    let panel_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.overlay_border))
        .style(Style::default().bg(theme.overlay_panel_bg))
        .title(Span::styled(
            "SigiLink missing mod files",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = panel_block.inner(modal);
    frame.render_widget(panel_block, modal);

    let mut header_lines = Vec::new();
    let header_text = match trigger {
        SigilLinkMissingTrigger::Enable => {
            "Enable blocked: missing .pak files for the selected mods."
        }
        SigilLinkMissingTrigger::Auto => {
            "SigiLink ranking found missing .pak files for these mods."
        }
    };
    header_lines.push(Line::from(Span::styled(
        header_text,
        Style::default().fg(theme.text),
    )));
    let summary = format!("Missing {total} mod(s)");
    header_lines.push(Line::from(Span::styled(
        truncate_text(&summary, inner.width as usize),
        Style::default().fg(theme.muted),
    )));
    header_lines.push(Line::from(Span::styled(
        "Open the Nexus search page and re-import to resolve.",
        Style::default().fg(theme.muted),
    )));

    let header_height = header_lines.len() as u16 + 1;
    let footer_height = 2u16;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Min(4),
            Constraint::Length(footer_height),
        ])
        .split(inner);

    let header_widget =
        Paragraph::new(header_lines).style(Style::default().bg(theme.overlay_panel_bg));
    frame.render_widget(header_widget, chunks[0]);

    let list_area = chunks[1];
    let list_width = list_area.width as usize;
    let item_height = 2usize;
    let view_items = (list_area.height as usize / item_height).max(1);
    app.set_sigillink_missing_queue_view(view_items);
    let (items, total_items, selected) = {
        let Some(queue) = app.sigillink_missing_queue() else {
            return;
        };
        let total_items = queue.items.len();
        let selected = queue.selected;
        let mut items = Vec::new();
        for (index, item) in queue.items.iter().enumerate() {
            let index_label = format!("{:>2}. ", index + 1);
            let label_width = list_width.saturating_sub(index_label.chars().count());
            let label_text = truncate_text(&item.name, label_width);
            let label_line = Line::from(vec![
                Span::styled(index_label, Style::default().fg(theme.muted)),
                Span::styled(label_text, Style::default().fg(theme.text)),
            ]);
            let detail_text = if item.search_link.is_some() {
                format!("UUID: {} | Nexus search", item.uuid)
            } else {
                format!("UUID: {}", item.uuid)
            };
            let detail_line = Line::from(Span::styled(
                truncate_text(&detail_text, list_width),
                Style::default().fg(theme.muted),
            ));
            items.push(ListItem::new(vec![label_line, detail_line]));
        }
        (items, total_items, selected)
    };

    let highlight_style = Style::default()
        .bg(theme.accent_soft)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD);
    let list = List::new(items)
        .style(Style::default().bg(theme.overlay_panel_bg))
        .highlight_style(highlight_style)
        .highlight_symbol("");

    let mut state = ListState::default();
    let mut offset = 0usize;
    if total_items > view_items {
        if selected >= view_items {
            offset = selected + 1 - view_items;
        }
        let max_offset = total_items.saturating_sub(view_items);
        if offset > max_offset {
            offset = max_offset;
        }
    }
    if total_items > 0 {
        state.select(Some(selected));
        *state.offset_mut() = offset;
    }

    let show_scroll = total_items > view_items;
    let list_chunks = if show_scroll {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(list_area)
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(0)])
            .split(list_area)
    };
    frame.render_stateful_widget(list, list_chunks[0], &mut state);

    if show_scroll && list_chunks[1].width > 0 {
        let scroll_len = total_items.saturating_sub(view_items).saturating_add(1);
        let mut scroll_state = ScrollbarState::new(scroll_len)
            .position(offset)
            .viewport_content_length(view_items);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .track_symbol(Some("░"))
            .thumb_symbol("▓")
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"))
            .track_style(Style::default().fg(theme.border))
            .thumb_style(Style::default().fg(theme.accent));
        frame.render_stateful_widget(scrollbar, list_chunks[1], &mut scroll_state);
    }

    let key_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(theme.muted);
    let cancel_label = match trigger {
        SigilLinkMissingTrigger::Enable => "Cancel",
        SigilLinkMissingTrigger::Auto => "Ignore",
    };
    let footer_line_one = Line::from(vec![
        Span::styled("↑/↓", key_style),
        Span::styled(" Move  ", text_style),
        Span::styled("[Enter]", key_style),
        Span::styled(" Open link  ", text_style),
        Span::styled("[c/C]", key_style),
        Span::styled(" Copy UUID  ", text_style),
        Span::styled("[Ctrl+C]", key_style),
        Span::styled(" Copy link  ", text_style),
    ]);
    let footer_line_two = Line::from(vec![
        Span::styled("[Esc]", key_style),
        Span::styled(format!(" {cancel_label}"), text_style),
    ]);
    let footer_widget = Paragraph::new(vec![footer_line_one, footer_line_two])
        .style(Style::default().bg(theme.overlay_panel_bg))
        .alignment(Alignment::Left);
    frame.render_widget(footer_widget, chunks[2]);
}

fn draw_path_browser(frame: &mut Frame<'_>, app: &App, theme: &Theme, browser: &PathBrowser) {
    let area = frame.size();
    let width = (area.width.saturating_sub(4)).clamp(46, 86);
    let height = (area.height.saturating_sub(4)).clamp(12, 22);
    let (outer_area, modal) = padded_modal(area, width, height, 2, 1);

    render_modal_backdrop(frame, outer_area, theme);

    let title = match &browser.purpose {
        PathBrowserPurpose::Setup(SetupStep::GameRoot) => "Select BG3 install root",
        PathBrowserPurpose::Setup(SetupStep::LarianDir) => "Select Larian data dir",
        PathBrowserPurpose::Setup(SetupStep::DownloadsDir) => "Select downloads folder",
        PathBrowserPurpose::ImportProfile => "Import mod list",
        PathBrowserPurpose::ExportProfile { kind, .. } => match kind {
            ExportKind::ModList => "Export mod list",
            ExportKind::Modsettings => "Export modsettings.lsx",
        },
        PathBrowserPurpose::ExportLog => "Export Log File",
        PathBrowserPurpose::SigilLinkCache { action, .. } => match action {
            SigilLinkCacheAction::Move => "Move SigiLink Cache",
            SigilLinkCacheAction::Relocate { .. } => "Select SigiLink Cache Folder",
        },
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent_soft))
        .style(Style::default().bg(theme.header_bg))
        .title(Span::styled(
            title,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(modal);
    frame.render_widget(block, modal);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(1),
            Constraint::Length(2),
        ])
        .split(inner);

    let path_focus = browser.focus == PathBrowserFocus::PathInput;
    let mut path_value = browser.path_input.clone();
    if path_focus {
        path_value.push('|');
    }
    let path_width = chunks[0].width.saturating_sub(6) as usize;
    let path_value = truncate_text(&path_value, path_width.max(1));
    let path_style = if path_focus {
        Style::default()
            .bg(theme.accent_soft)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text)
    };
    let path_line = Line::from(vec![
        Span::styled("Path: ", Style::default().fg(theme.muted)),
        Span::styled(path_value, path_style),
    ]);
    let current_label = format!(
        "Current: {}",
        truncate_text(
            &browser.current.display().to_string(),
            chunks[0].width as usize
        )
    );
    let current_line = Line::from(Span::styled(
        current_label,
        Style::default().fg(theme.muted),
    ));
    let raw_input = browser.path_input.trim();
    let selectable_path = match &browser.purpose {
        PathBrowserPurpose::Setup(_) => browser.current.clone(),
        _ => expand_tilde(raw_input),
    };
    let selectable = if matches!(browser.purpose, PathBrowserPurpose::ImportProfile)
        && raw_input.trim_start().starts_with('{')
    {
        true
    } else {
        app.path_browser_selectable(&browser.purpose, &selectable_path)
    };
    let (valid_label, invalid_label) = match &browser.purpose {
        PathBrowserPurpose::Setup(SetupStep::GameRoot) => (
            " BG3 install root valid ",
            "Not a BG3 install root (needs Data/ + bin/)",
        ),
        PathBrowserPurpose::Setup(SetupStep::LarianDir) => (
            " Larian data dir valid ",
            "Not a Larian data dir (needs PlayerProfiles/)",
        ),
        PathBrowserPurpose::Setup(SetupStep::DownloadsDir) => (" Folder valid ", "Not a folder."),
        PathBrowserPurpose::ImportProfile => (" File selected ", "Select a file to import."),
        PathBrowserPurpose::ExportProfile { .. } => (" Export path valid ", "Enter a file name."),
        PathBrowserPurpose::ExportLog => (" Folder selected ", "Select a folder to export."),
        PathBrowserPurpose::SigilLinkCache { require_dev, .. } => {
            if require_dev.is_some() {
                (
                    " BG3 cache location valid ",
                    "Select a directory on the same drive as BG3 to use SigiLink without symlinks.",
                )
            } else {
                (
                    " Folder selected ",
                    "Select a folder for the SigiLink cache.",
                )
            }
        }
    };
    let status_span = if selectable {
        Span::styled(
            valid_label,
            Style::default()
                .fg(Color::Black)
                .bg(theme.success)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(invalid_label, Style::default().fg(theme.warning))
    };
    let status_line = Line::from(vec![
        Span::styled("Status: ", Style::default().fg(theme.muted)),
        status_span,
    ]);
    let header = Paragraph::new(vec![path_line, current_line, status_line, Line::from("")]);
    frame.render_widget(header, chunks[0]);

    let show_select = matches!(
        browser.purpose,
        PathBrowserPurpose::Setup(_)
            | PathBrowserPurpose::ExportLog
            | PathBrowserPurpose::SigilLinkCache { .. }
    );
    let hide_select = !show_select;
    let mut entries: Vec<ListItem> = Vec::new();
    if hide_select {
        entries.push(ListItem::new(Line::from("")));
    }
    entries.extend(browser.entries.iter().map(|entry| {
        let style = match entry.kind {
            PathBrowserEntryKind::Select => {
                if entry.selectable {
                    Style::default()
                        .fg(theme.success)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.warning)
                }
            }
            PathBrowserEntryKind::SaveHere => {
                if entry.selectable {
                    Style::default()
                        .fg(theme.success)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.warning)
                }
            }
            PathBrowserEntryKind::Parent => Style::default().fg(theme.muted),
            PathBrowserEntryKind::Dir => Style::default().fg(theme.text),
            PathBrowserEntryKind::File => Style::default().fg(theme.text),
        };
        ListItem::new(Line::from(Span::styled(entry.label.clone(), style)))
    }));
    let highlight_style = if browser.focus == PathBrowserFocus::List {
        Style::default()
            .bg(theme.accent_soft)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(theme.header_bg).fg(theme.text)
    };
    let list = List::new(entries)
        .style(Style::default().bg(theme.header_bg))
        .highlight_style(highlight_style)
        .highlight_symbol("");
    let list_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(chunks[1]);
    let spacer = Paragraph::new(Line::from("")).style(Style::default().bg(theme.header_bg));
    frame.render_widget(spacer, list_chunks[0]);

    let mut state = ListState::default();
    let view_height = list_chunks[1].height as usize;
    let list_offset = if hide_select { 1usize } else { 0usize };
    let total = browser.entries.len().saturating_add(list_offset);
    let selected = browser
        .selected
        .min(browser.entries.len().saturating_sub(1));
    let display_selected = selected.saturating_add(list_offset);
    let mut offset = 0usize;
    if total > view_height && view_height > 0 {
        if display_selected >= view_height {
            offset = display_selected + 1 - view_height;
        }
        let max_offset = total.saturating_sub(view_height);
        if offset > max_offset {
            offset = max_offset;
        }
    }
    if total > 0 {
        state.select(Some(display_selected));
        *state.offset_mut() = offset;
    }
    frame.render_stateful_widget(list, list_chunks[1], &mut state);

    if total > view_height && view_height > 0 {
        let scroll_len = total.saturating_sub(view_height).saturating_add(1);
        let mut scroll_state = ScrollbarState::new(scroll_len)
            .position(offset)
            .viewport_content_length(view_height);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .track_symbol(Some("░"))
            .thumb_symbol("▓")
            .begin_symbol(None)
            .end_symbol(None)
            .track_style(Style::default().fg(theme.border))
            .thumb_style(Style::default().fg(theme.accent));
        let scroll_area = Rect {
            x: list_chunks[1].x + list_chunks[1].width.saturating_sub(1),
            y: list_chunks[1].y,
            width: 1,
            height: list_chunks[1].height,
        };
        frame.render_stateful_widget(scrollbar, scroll_area, &mut scroll_state);
    }

    let footer_area = chunks[3];
    let spacer = Paragraph::new(Line::from("")).style(Style::default().bg(theme.header_bg));
    frame.render_widget(spacer, chunks[2]);

    let tab_label = if path_focus { "Browse Folder" } else { "Path" };
    let key_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(theme.muted);
    let footer_parts = vec![
        ("[Tab]".to_string(), key_style),
        (format!(" {tab_label}  "), text_style),
        ("[Enter/Space]".to_string(), key_style),
        (" Open/Select  ".to_string(), text_style),
        ("[Backspace]".to_string(), key_style),
        (" Parent  ".to_string(), text_style),
        ("[Esc]".to_string(), key_style),
        (" Cancel".to_string(), text_style),
    ];
    let footer_line = Line::from(truncate_spans(footer_parts, footer_area.width as usize));
    let footer_widget = Paragraph::new(footer_line)
        .style(Style::default().fg(theme.muted))
        .alignment(Alignment::Center);
    frame.render_widget(footer_widget, footer_area);
}

struct SmartRankPreviewRender {
    lines: Vec<Line<'static>>,
    scroll: Option<SmartRankScroll>,
}

struct SmartRankScroll {
    total: usize,
    view: usize,
    position: usize,
    header_lines: usize,
}

struct ModListPreviewRender {
    lines: Vec<Line<'static>>,
    scroll: usize,
    max_scroll: usize,
}

fn draw_smart_rank_preview(frame: &mut Frame<'_>, app: &mut App, theme: &Theme) {
    let Some(preview) = &app.smart_rank_preview else {
        return;
    };

    let area = frame.size();
    let max_width = area.width.saturating_sub(2).max(1);
    let width = max_width.min(120).max(60).min(max_width);
    let max_height = area.height.saturating_sub(2).max(1);
    let height = max_height.min(22).max(10);
    let (outer_area, preview_area) = padded_modal(area, width, height, 2, 1);

    let inner_width = preview_area.width.saturating_sub(3) as usize;
    let inner_height = preview_area.height.saturating_sub(2) as usize;
    let notice = app
        .sigillink_preview_notice()
        .map(|value| value.to_string());
    let render = build_smart_rank_preview_render(
        preview,
        theme,
        inner_width,
        inner_height,
        app.smart_rank_scroll,
        app.smart_rank_view,
        notice,
    );

    render_modal_backdrop(frame, outer_area, theme);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent_soft))
        .style(Style::default().bg(theme.header_bg))
        .title(Span::styled(
            "SigiLink Intelligent Ranking",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(preview_area);
    let widget = Paragraph::new(render.lines)
        .block(block)
        .style(Style::default().fg(theme.text))
        .alignment(Alignment::Left);
    frame.render_widget(widget, preview_area);

    let scroll_meta = render.scroll;
    if let Some(scroll) = scroll_meta {
        app.smart_rank_scroll = scroll.position;
        if scroll.total > scroll.view && inner.width > 0 && inner.height > 0 {
            let body_height = scroll.view.min(inner.height as usize) as u16;
            if body_height > 0 {
                let scroll_area = Rect {
                    x: inner.x + inner.width.saturating_sub(1),
                    y: inner.y.saturating_add(scroll.header_lines as u16),
                    width: 1,
                    height: body_height,
                };
                let scroll_len = scroll.total.saturating_sub(scroll.view).saturating_add(1);
                let mut scroll_state = ScrollbarState::new(scroll_len)
                    .position(scroll.position)
                    .viewport_content_length(scroll.view);
                let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .track_symbol(Some("░"))
                    .thumb_symbol("▓")
                    .begin_symbol(None)
                    .end_symbol(None)
                    .track_style(Style::default().fg(theme.border))
                    .thumb_style(Style::default().fg(theme.accent));
                frame.render_stateful_widget(scrollbar, scroll_area, &mut scroll_state);
            }
        }
    } else {
        app.smart_rank_scroll = 0;
    }
}

fn draw_mod_list_preview(frame: &mut Frame<'_>, app: &mut App, theme: &Theme) {
    let Some(preview) = &app.mod_list_preview else {
        return;
    };

    let area = frame.size();
    let max_width = area.width.saturating_sub(2).max(1);
    let width = max_width.min(110).max(58).min(max_width);
    let max_height = area.height.saturating_sub(2).max(1);
    let mut height = max_height.min(16).max(8);
    let mut outer_area = Rect::default();
    let mut preview_area = Rect::default();
    let mut render = ModListPreviewRender {
        lines: Vec::new(),
        scroll: 0,
        max_scroll: 0,
    };
    for _ in 0..2 {
        (outer_area, preview_area) = padded_modal(area, width, height, 2, 1);
        let inner_width = preview_area.width.saturating_sub(3) as usize;
        let inner_height = preview_area.height.saturating_sub(2) as usize;
        render = build_mod_list_preview_render(
            app,
            preview,
            theme,
            inner_width,
            inner_height,
            app.mod_list_scroll,
        );
        if render.max_scroll == 0 {
            let desired = (render.lines.len() as u16 + 2).clamp(8, max_height);
            if desired < height {
                height = desired;
                continue;
            }
        }
        break;
    }
    app.mod_list_scroll = render.scroll;

    render_modal_backdrop(frame, outer_area, theme);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent_soft))
        .style(Style::default().bg(theme.header_bg))
        .title(Span::styled(
            "Mod list preview",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(preview_area);
    let widget = Paragraph::new(render.lines)
        .block(block)
        .style(Style::default().fg(theme.text))
        .alignment(Alignment::Left);
    frame.render_widget(widget, preview_area);

    if render.max_scroll > 0 && inner.width > 0 && inner.height > 0 {
        let body_height = inner.height.saturating_sub(2);
        if body_height > 0 {
            let scroll_area = Rect {
                x: inner.x + inner.width.saturating_sub(1),
                y: inner.y + 1,
                width: 1,
                height: body_height,
            };
            let scroll_len = render.max_scroll.saturating_add(1);
            let mut scroll_state = ScrollbarState::new(scroll_len)
                .position(render.scroll)
                .viewport_content_length(body_height as usize);
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .track_symbol(Some("░"))
                .thumb_symbol("▓")
                .begin_symbol(None)
                .end_symbol(None)
                .track_style(Style::default().fg(theme.border))
                .thumb_style(Style::default().fg(theme.accent));
            frame.render_stateful_widget(scrollbar, scroll_area, &mut scroll_state);
        }
    }
}

fn build_smart_rank_preview_render(
    preview: &crate::app::SmartRankPreview,
    theme: &Theme,
    width: usize,
    height: usize,
    scroll: usize,
    view: crate::app::SmartRankView,
    notice: Option<String>,
) -> SmartRankPreviewRender {
    if width == 0 || height == 0 {
        return SmartRankPreviewRender {
            lines: Vec::new(),
            scroll: None,
        };
    }

    let mut lines = Vec::new();
    let has_notice = notice.is_some();
    if let Some(notice) = notice {
        lines.push(Line::from(Span::styled(
            notice,
            Style::default()
                .fg(theme.success)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
    }
    let report = &preview.report;
    lines.push(Line::from(Span::styled(
        format!("Moved: {} | Scan: {}ms", report.moved, report.elapsed_ms),
        Style::default().fg(theme.text),
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "Conflicts: Loose {} | Pak {}",
            report.conflicts_loose, report.conflicts_pak
        ),
        Style::default().fg(theme.text),
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "Scanned: Loose {}/{} | Pak {}/{} | Missing: Loose {} | Pak {}",
            report.scanned_loose,
            report.enabled_loose,
            report.scanned_pak,
            report.enabled_pak,
            report.missing_loose,
            report.missing_pak
        ),
        Style::default().fg(theme.text),
    )));
    lines.push(Line::from(Span::styled(
        "Conflicting mods are ordered by size (big → small).",
        Style::default().fg(theme.muted),
    )));
    lines.push(Line::from(Span::styled(
        "Non-conflicting mods keep their relative order.",
        Style::default().fg(theme.muted),
    )));

    if !preview.warnings.is_empty() {
        lines.push(Line::from(""));
        let warning_label = if preview.warnings.len() > 2 {
            format!("Warnings (showing 2 of {}):", preview.warnings.len())
        } else {
            "Warnings:".to_string()
        };
        lines.push(Line::from(Span::styled(
            warning_label,
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        )));
        for warning in preview.warnings.iter().take(2) {
            lines.push(Line::from(Span::styled(
                truncate_text(warning, width),
                Style::default().fg(theme.warning),
            )));
        }
    }

    lines.push(Line::from(""));

    let min_mod_width = 10usize;
    let min_date_width = 8usize;
    let sep_mod = " | ";
    let sep_move = " → ";
    let mut current_width = "Current".len().max(6);
    let mut proposed_width = "Proposed".len().max(8);
    let mut created_width = "Created".len().max(10);
    let mut added_width = "Added".len().max(10);
    let sep_width = sep_mod.len() * 3 + sep_move.len();
    let mut mod_width = width
        .saturating_sub(current_width + proposed_width + created_width + added_width + sep_width);
    if mod_width < min_mod_width {
        let deficit = min_mod_width.saturating_sub(mod_width);
        let shrink_left = deficit / 2 + deficit % 2;
        let shrink_right = deficit / 2;
        current_width = current_width.saturating_sub(shrink_left).max(3);
        proposed_width = proposed_width.saturating_sub(shrink_right).max(3);
        mod_width = width.saturating_sub(
            current_width + proposed_width + created_width + added_width + sep_width,
        );
        if mod_width < min_mod_width {
            let deficit = min_mod_width.saturating_sub(mod_width);
            let shrink_created = deficit / 2 + deficit % 2;
            let shrink_added = deficit / 2;
            created_width = created_width
                .saturating_sub(shrink_created)
                .max(min_date_width);
            added_width = added_width.saturating_sub(shrink_added).max(min_date_width);
            mod_width = width.saturating_sub(
                current_width + proposed_width + created_width + added_width + sep_width,
            );
        }
        if mod_width == 0 {
            mod_width = 1;
        }
    }
    if matches!(view, crate::app::SmartRankView::Changes) {
        let mod_header = truncate_text("Mod", mod_width);
        let mod_pad = " ".repeat(mod_width.saturating_sub(mod_header.chars().count()));
        let created_header = truncate_text("Created", created_width);
        let created_pad = " ".repeat(created_width.saturating_sub(created_header.chars().count()));
        let added_header = truncate_text("Added", added_width);
        let added_pad = " ".repeat(added_width.saturating_sub(added_header.chars().count()));
        let current_header = truncate_text("Current", current_width);
        let current_pad = " ".repeat(current_width.saturating_sub(current_header.chars().count()));
        let proposed_header = truncate_text("Proposed", proposed_width);
        let proposed_pad =
            " ".repeat(proposed_width.saturating_sub(proposed_header.chars().count()));
        lines.push(Line::from(vec![
            Span::styled(
                format!("{mod_header}{mod_pad}"),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(sep_mod, Style::default().fg(theme.muted)),
            Span::styled(
                format!("{created_header}{created_pad}"),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(sep_mod, Style::default().fg(theme.muted)),
            Span::styled(
                format!("{added_header}{added_pad}"),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(sep_mod, Style::default().fg(theme.muted)),
            Span::styled(
                format!("{current_header}{current_pad}"),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(sep_move, Style::default().fg(theme.muted)),
            Span::styled(
                format!("{proposed_header}{proposed_pad}"),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    } else {
        lines.push(Line::from(Span::styled(
            "Explain",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )));
    }

    let header_lines = lines.len();
    let mut body_lines = Vec::new();
    if matches!(view, crate::app::SmartRankView::Changes) {
        if preview.moves.is_empty() {
            let empty_label =
                if has_notice && preview.warnings.is_empty() && preview.report.missing == 0 {
                    "No changes needed."
                } else {
                    "No ordering changes detected."
                };
            body_lines.push(Line::from(Span::styled(
                empty_label,
                Style::default().fg(theme.muted),
            )));
        } else {
            let mut max_delta = 0usize;
            for entry in &preview.moves {
                let delta = if entry.from > entry.to {
                    entry.from - entry.to
                } else {
                    entry.to - entry.from
                };
                if delta > max_delta {
                    max_delta = delta;
                }
            }
            let highlight_delta = if max_delta > 1 { max_delta } else { 0 };
            for (index, entry) in preview.moves.iter().enumerate() {
                let delta = if entry.from > entry.to {
                    entry.from - entry.to
                } else {
                    entry.to - entry.from
                };
                let is_major = highlight_delta > 0 && delta == highlight_delta;
                let mod_text = format_padded_cell(&entry.name, mod_width);
                let created_text =
                    format_padded_cell(&format_date_cell(entry.created_at), created_width);
                let added_text =
                    format_padded_cell(&format_date_cell(Some(entry.added_at)), added_width);
                let current_text = format!("{:>width$}", entry.from + 1, width = current_width);
                let proposed_text = format!("{:>width$}", entry.to + 1, width = proposed_width);
                let row_bg = if index % 2 == 1 {
                    Some(theme.row_alt_bg)
                } else {
                    None
                };
                let mut mod_style =
                    Style::default().fg(if is_major { theme.accent } else { theme.text });
                if is_major {
                    mod_style = mod_style.add_modifier(Modifier::BOLD).bg(theme.accent_soft);
                }
                let mut date_style = Style::default().fg(theme.muted);
                let mut current_style = Style::default().fg(theme.muted);
                let mut proposed_style =
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD);
                let mut sep_style = Style::default().fg(theme.muted);
                let mut arrow_style =
                    Style::default().fg(if is_major { theme.success } else { theme.muted });
                if let Some(bg) = row_bg {
                    if !is_major {
                        mod_style = mod_style.bg(bg);
                    }
                    date_style = date_style.bg(bg);
                    current_style = current_style.bg(bg);
                    proposed_style = proposed_style.bg(bg);
                    sep_style = sep_style.bg(bg);
                    arrow_style = arrow_style.bg(bg);
                }
                body_lines.push(Line::from(vec![
                    Span::styled(mod_text, mod_style),
                    Span::styled(sep_mod, sep_style),
                    Span::styled(created_text, date_style),
                    Span::styled(sep_mod, sep_style),
                    Span::styled(added_text, date_style),
                    Span::styled(sep_mod, sep_style),
                    Span::styled(current_text, current_style),
                    Span::styled(" ", sep_style),
                    Span::styled("→", arrow_style),
                    Span::styled(" ", sep_style),
                    Span::styled(proposed_text, proposed_style),
                ]));
            }
        }
    } else {
        for line in &preview.explain.lines {
            let style = match line.kind {
                crate::smart_rank::ExplainLineKind::Header => Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
                crate::smart_rank::ExplainLineKind::Muted => Style::default().fg(theme.muted),
                crate::smart_rank::ExplainLineKind::Item => Style::default().fg(theme.text),
            };
            body_lines.push(Line::from(Span::styled(
                truncate_text(&line.text, width),
                style,
            )));
        }
    }

    let available = height.saturating_sub(lines.len() + 1);
    if available == 0 {
        lines.push(Line::from(Span::styled(
            "Enter: apply | Esc: cancel",
            Style::default().fg(theme.muted),
        )));
        return SmartRankPreviewRender {
            lines,
            scroll: None,
        };
    }

    let total = body_lines.len();
    let max_scroll = total.saturating_sub(available);
    let scroll = scroll.min(max_scroll);
    let end = (scroll + available).min(total);
    if total > 0 {
        lines.extend(body_lines[scroll..end].iter().cloned());
    }

    let footer = if total > available {
        format!(
            "Enter: apply | Esc: cancel | Tab: view | ↑/↓ scroll {}/{}",
            scroll + 1,
            max_scroll + 1
        )
    } else {
        "Enter: apply | Esc: cancel | Tab: view".to_string()
    };
    lines.push(Line::from(Span::styled(
        footer,
        Style::default().fg(theme.muted),
    )));

    let scroll_meta = if total > available {
        Some(SmartRankScroll {
            total,
            view: available,
            position: scroll,
            header_lines,
        })
    } else {
        None
    };

    SmartRankPreviewRender {
        lines,
        scroll: scroll_meta,
    }
}

fn build_mod_list_preview_render(
    app: &App,
    preview: &crate::app::ModListPreview,
    theme: &Theme,
    width: usize,
    height: usize,
    scroll: usize,
) -> ModListPreviewRender {
    if width == 0 || height == 0 {
        return ModListPreviewRender {
            lines: Vec::new(),
            scroll: 0,
            max_scroll: 0,
        };
    }

    let mut matched = 0usize;
    let mut missing = Vec::new();
    let mut ambiguous = Vec::new();
    let mut enabled_count = 0usize;
    for entry in &preview.entries {
        let base_label = if entry.source.name.trim().is_empty() {
            entry.source.id.trim()
        } else {
            entry.source.name.trim()
        };
        let label = if base_label.is_empty() {
            "(unnamed)".to_string()
        } else {
            base_label.to_string()
        };
        match &entry.outcome {
            crate::app::ModListMatchOutcome::Matched { .. } => {
                matched += 1;
                if entry.source.enabled {
                    enabled_count += 1;
                }
            }
            crate::app::ModListMatchOutcome::Missing => missing.push(label),
            crate::app::ModListMatchOutcome::Ambiguous { candidates, .. } => {
                ambiguous.push((label, candidates.clone()));
            }
        }
    }

    let total = preview.entries.len();
    let missing_count = missing.len();
    let ambiguous_count = ambiguous.len();
    let disabled_count = total.saturating_sub(enabled_count);
    let active_profile = if app.library.active_profile.is_empty() {
        "<none>".to_string()
    } else {
        app.library.active_profile.clone()
    };
    let dest_label = match preview.destination {
        crate::app::ModListDestination::NewProfile => {
            format!("New profile ({})", preview.new_profile_name)
        }
        crate::app::ModListDestination::ActiveProfile => {
            format!("Active profile ({active_profile})")
        }
    };
    let mode_label = match preview.mode {
        crate::app::ModListApplyMode::Merge => "Merge",
        crate::app::ModListApplyMode::Strict => "Strict",
    };
    let override_label = match preview.override_mode {
        crate::app::ModListOverrideMode::Merge => "Merge",
        crate::app::ModListOverrideMode::Replace => "Replace",
    };
    let (order_changes, enable_changes, new_entries) =
        if let Some(profile) = app.library.active_profile() {
            let mut order_changes = 0usize;
            let mut enable_changes = 0usize;
            let mut new_entries = 0usize;
            let mut current_index = std::collections::HashMap::new();
            for (idx, entry) in profile.order.iter().enumerate() {
                current_index.insert(entry.id.clone(), idx);
            }
            for (idx, entry) in preview.entries.iter().enumerate() {
                let resolved_id = match &entry.outcome {
                    crate::app::ModListMatchOutcome::Matched { resolved_id, .. } => resolved_id,
                    _ => {
                        continue;
                    }
                };
                match current_index.get(resolved_id) {
                    Some(current_idx) => {
                        if *current_idx != idx {
                            order_changes += 1;
                        }
                        if let Some(current_entry) = profile
                            .order
                            .iter()
                            .find(|current| current.id == *resolved_id)
                        {
                            if current_entry.enabled != entry.source.enabled {
                                enable_changes += 1;
                            }
                        }
                    }
                    None => new_entries += 1,
                }
            }
            (order_changes, enable_changes, new_entries)
        } else {
            (0, 0, 0)
        };

    let mut header_lines = Vec::new();
    header_lines.push(Line::from(vec![
        Span::styled("Source: ", Style::default().fg(theme.muted)),
        Span::styled(
            truncate_text(&preview.source_label, width),
            Style::default().fg(theme.text),
        ),
    ]));
    header_lines.push(Line::from(vec![
        Span::styled("Destination: ", Style::default().fg(theme.muted)),
        Span::styled(
            truncate_text(&dest_label, width),
            Style::default().fg(theme.text),
        ),
        Span::styled("  [D]", Style::default().fg(theme.muted)),
    ]));
    header_lines.push(Line::from(vec![
        Span::styled("Mode: ", Style::default().fg(theme.muted)),
        Span::styled(mode_label, Style::default().fg(theme.text)),
        Span::styled("  [M]  Overrides: ", Style::default().fg(theme.muted)),
        Span::styled(override_label, Style::default().fg(theme.text)),
    ]));
    header_lines.push(Line::from(vec![
        Span::styled("Entries: ", Style::default().fg(theme.muted)),
        Span::styled(total.to_string(), Style::default().fg(theme.text)),
        Span::styled("  Matched: ", Style::default().fg(theme.muted)),
        Span::styled(matched.to_string(), Style::default().fg(theme.text)),
        Span::styled("  Enabled: ", Style::default().fg(theme.muted)),
        Span::styled(
            enabled_count.to_string(),
            Style::default().fg(theme.success),
        ),
        Span::styled("  Disabled: ", Style::default().fg(theme.muted)),
        Span::styled(disabled_count.to_string(), Style::default().fg(theme.muted)),
    ]));
    header_lines.push(Line::from(vec![
        Span::styled("Changes vs active: ", Style::default().fg(theme.muted)),
        Span::styled(order_changes.to_string(), Style::default().fg(theme.text)),
        Span::styled(" order  ", Style::default().fg(theme.muted)),
        Span::styled(enable_changes.to_string(), Style::default().fg(theme.text)),
        Span::styled(" enable  ", Style::default().fg(theme.muted)),
        Span::styled(new_entries.to_string(), Style::default().fg(theme.text)),
        Span::styled(" new", Style::default().fg(theme.muted)),
        Span::styled("  Missing: ", Style::default().fg(theme.muted)),
        Span::styled(
            missing_count.to_string(),
            Style::default().fg(theme.warning),
        ),
        Span::styled("  Ambiguous: ", Style::default().fg(theme.muted)),
        Span::styled(
            ambiguous_count.to_string(),
            Style::default().fg(theme.error),
        ),
    ]));
    if ambiguous_count > 0 {
        header_lines.push(Line::from(Span::styled(
            "Ambiguous matches block apply.",
            Style::default().fg(theme.warning),
        )));
    }

    if !preview.warnings.is_empty() {
        header_lines.push(Line::from(""));
        header_lines.push(Line::from(Span::styled(
            "Warnings:",
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        )));
        for warning in &preview.warnings {
            header_lines.push(Line::from(Span::styled(
                truncate_text(warning, width),
                Style::default().fg(theme.warning),
            )));
        }
    }

    let mut body_lines = Vec::new();
    body_lines.push(Line::from(""));
    body_lines.push(Line::from(Span::styled(
        "Missing:",
        Style::default()
            .fg(theme.warning)
            .add_modifier(Modifier::BOLD),
    )));
    if missing.is_empty() {
        body_lines.push(Line::from(Span::styled(
            "None",
            Style::default().fg(theme.muted),
        )));
    } else {
        for label in missing {
            body_lines.push(Line::from(Span::styled(
                truncate_text(&format!("- {label}"), width),
                Style::default().fg(theme.warning),
            )));
        }
    }

    if !ambiguous.is_empty() {
        body_lines.push(Line::from(""));
        body_lines.push(Line::from(Span::styled(
            "Ambiguous:",
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD),
        )));
        for (label, candidates) in ambiguous {
            body_lines.push(Line::from(Span::styled(
                truncate_text(&format!("- {label}"), width),
                Style::default().fg(theme.error),
            )));
            if !candidates.is_empty() {
                body_lines.push(Line::from(Span::styled(
                    truncate_text(&format!("  -> {}", candidates.join(", ")), width),
                    Style::default().fg(theme.muted),
                )));
            }
        }
    }

    let mut lines = header_lines;
    let key_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(theme.muted);
    let build_footer_line = |parts: Vec<(String, Style)>| -> Line<'static> {
        let total_width: usize = parts.iter().map(|(text, _)| display_width(text)).sum();
        let pad = if total_width >= width {
            0
        } else {
            width.saturating_sub(total_width) / 2
        };
        let mut spans = Vec::new();
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
        let mut body = truncate_spans(parts, width.saturating_sub(pad));
        spans.append(&mut body);
        Line::from(spans)
    };
    let available = height.saturating_sub(lines.len() + 1);
    let total_body = body_lines.len();
    let max_scroll = total_body.saturating_sub(available);
    let scroll = scroll.min(max_scroll);
    if available == 0 {
        lines.push(build_footer_line(vec![
            ("[Enter]".to_string(), key_style),
            (" apply  ".to_string(), text_style),
            ("[Esc]".to_string(), key_style),
            (" cancel  ".to_string(), text_style),
            ("[D]".to_string(), key_style),
            (" destination  ".to_string(), text_style),
            ("[M]".to_string(), key_style),
            (" mode".to_string(), text_style),
        ]));
        return ModListPreviewRender {
            lines,
            scroll,
            max_scroll,
        };
    }

    let end = (scroll + available).min(total_body);
    if total_body > 0 {
        lines.extend(body_lines[scroll..end].iter().cloned());
    }

    let mut footer_parts = vec![
        ("[Enter]".to_string(), key_style),
        (" apply  ".to_string(), text_style),
        ("[Esc]".to_string(), key_style),
        (" cancel  ".to_string(), text_style),
        ("[D]".to_string(), key_style),
        (" destination  ".to_string(), text_style),
        ("[M]".to_string(), key_style),
        (" mode".to_string(), text_style),
    ];
    if total_body > available {
        footer_parts.push((
            format!("  ↑/↓ scroll {}/{}", scroll + 1, max_scroll + 1),
            text_style,
        ));
    }
    lines.push(build_footer_line(footer_parts));

    ModListPreviewRender {
        lines,
        scroll,
        max_scroll,
    }
}

fn format_padded_cell(value: &str, width: usize) -> String {
    let text = truncate_text(value, width);
    let pad = width.saturating_sub(text.chars().count());
    format!("{text}{}", " ".repeat(pad))
}

#[derive(Clone, Copy)]
enum MenuRowKind {
    None,
    Action,
}

impl MenuRowKind {
    fn prefix(self) -> &'static str {
        match self {
            MenuRowKind::None => "    ",
            MenuRowKind::Action => "▸   ",
        }
    }
}

fn menu_row(
    selected: bool,
    kind: MenuRowKind,
    label: Span<'static>,
    theme: &Theme,
) -> Line<'static> {
    let sel = "  ";
    let prefix_style = if matches!(kind, MenuRowKind::Action) && selected {
        Style::default()
            .fg(theme.success)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text)
    };
    Line::from(vec![
        Span::raw(sel),
        Span::styled(kind.prefix(), prefix_style),
        label,
    ])
}

fn centered_line(text: &str, width: Option<usize>, style: Style) -> Line<'static> {
    let text_width = display_width(text);
    let pad = width
        .unwrap_or(0)
        .saturating_sub(text_width)
        .saturating_div(2);
    if pad == 0 {
        return Line::from(Span::styled(text.to_string(), style));
    }
    Line::from(vec![
        Span::raw(" ".repeat(pad)),
        Span::styled(text.to_string(), style),
    ])
}

fn kv_row(
    kind: MenuRowKind,
    key: &str,
    key_width: usize,
    key_style: Style,
    value_spans: Vec<Span<'static>>,
) -> Line<'static> {
    let key_text = pad_display_width(key, key_width);
    let mut spans = vec![
        Span::raw("  "),
        Span::raw(kind.prefix()),
        Span::styled(key_text, key_style),
        Span::styled(" : ", key_style),
    ];
    spans.extend(value_spans);
    Line::from(spans)
}

fn pad_display_width(value: &str, width: usize) -> String {
    let len = display_width(value);
    let pad = width.saturating_sub(len);
    format!("{value}{}", " ".repeat(pad))
}

fn build_settings_menu_lines(
    app: &App,
    theme: &Theme,
    selected: usize,
    content_width: Option<usize>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let _muted = Style::default().fg(theme.muted);
    let header_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let version_value = format!("v{}", env!("CARGO_PKG_VERSION"));
    let updates_line = update_status_line(app);
    let updates_value = updates_line
        .split_once(": ")
        .map(|(_, value)| value)
        .unwrap_or(updates_line.as_str());
    lines.push(Line::from(""));
    lines.push(centered_line("Settings", content_width, header_style));
    lines.push(Line::from(""));

    let items = settings_items(app);
    let whats_new_index = items
        .iter()
        .position(|item| matches!(item.kind, SettingsItemKind::ActionWhatsNew));
    let content_width = content_width.unwrap_or(0);
    let key_limit = content_width.saturating_sub(10).max(1);
    let clamp_key = |width: usize| {
        if content_width == 0 {
            width
        } else {
            width.min(key_limit)
        }
    };
    let general_key_w = clamp_key(
        items
            .iter()
            .filter(|item| {
                matches!(
                    item.kind,
                    SettingsItemKind::ToggleEnableModsAfterImport
                        | SettingsItemKind::ToggleDeleteModFilesOnRemove
                        | SettingsItemKind::ToggleProfileDelete
                        | SettingsItemKind::ToggleModDelete
                        | SettingsItemKind::ToggleAutoDeploy
                        | SettingsItemKind::ToggleDependencyDownloads
                        | SettingsItemKind::ToggleDependencyWarnings
                        | SettingsItemKind::ToggleStartupDependencyNotice
                )
            })
            .map(|item| display_width(&item.label))
            .max()
            .unwrap_or(0),
    );
    let default_sort_key_w = clamp_key(display_width("Default Sort Column").max(general_key_w));
    let sigilink_key_w = clamp_key(
        items
            .iter()
            .filter(|item| {
                matches!(
                    item.kind,
                    SettingsItemKind::SigilLinkToggle
                        | SettingsItemKind::SigilLinkInfo
                        | SettingsItemKind::SigilLinkAutoPreview
                )
            })
            .map(|item| {
                item.label
                    .split_once(": ")
                    .map(|(key, _)| display_width(key))
                    .unwrap_or_else(|| display_width(&item.label))
            })
            .max()
            .unwrap_or(0),
    );
    let hotkey_rows = [
        ("Tab", "Cycle Focus"),
        ("Esc", "Close"),
        ("?", "Full Hotkeys"),
        ("Ctrl+E", "Export Mod List"),
        ("Ctrl+P", "Import Mod List"),
    ];
    let hotkey_key_w = clamp_key(
        hotkey_rows
            .iter()
            .map(|(key, _)| display_width(key))
            .max()
            .unwrap_or(0),
    );

    for (index, item) in items.iter().enumerate() {
        if matches!(item.kind, SettingsItemKind::ActionWhatsNew) {
            continue;
        }
        if matches!(
            item.kind,
            SettingsItemKind::SigilLinkHeader
                | SettingsItemKind::SigilLinkDebugHeader
                | SettingsItemKind::ProfilesHeader
        ) {
            lines.push(Line::from(""));
        }
        if !item.selectable && !matches!(item.kind, SettingsItemKind::SigilLinkInfo) {
            let (label, style) = match item.kind {
                SettingsItemKind::SigilLinkHeader => (
                    item.label.to_string(),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                SettingsItemKind::ProfilesHeader => (
                    item.label.to_string(),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                SettingsItemKind::SigilLinkDebugHeader => (
                    item.label.to_string(),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                _ => (item.label.to_string(), Style::default().fg(theme.muted)),
            };
            lines.push(menu_row(
                false,
                MenuRowKind::None,
                Span::styled(label, style),
                theme,
            ));
            continue;
        }

        let style = if index == selected {
            Style::default()
                .fg(theme.success)
                .add_modifier(Modifier::BOLD)
        } else if !item.selectable {
            Style::default().fg(theme.muted)
        } else if matches!(item.kind, SettingsItemKind::ActionWhatsNew) {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text)
        };
        match item.kind {
            SettingsItemKind::ActionSetupPaths
            | SettingsItemKind::ActionShowPaths
            | SettingsItemKind::ActionMoveSigilLinkCache
            | SettingsItemKind::ActionClearFrameworkCaches
            | SettingsItemKind::ActionClearSigilLinkCaches
            | SettingsItemKind::ActionClearSigilLinkPins
            | SettingsItemKind::ActionSigilLinkSoloRank
            | SettingsItemKind::ActionExportModList
            | SettingsItemKind::ActionImportModList
            | SettingsItemKind::ActionCopyLogTail
            | SettingsItemKind::ActionCopyLogAll
            | SettingsItemKind::ActionExportLogFile
            | SettingsItemKind::ActionCheckUpdates
            | SettingsItemKind::ActionWhatsNew => {
                lines.push(menu_row(
                    index == selected,
                    MenuRowKind::Action,
                    Span::styled(item.label.to_string(), style),
                    theme,
                ));
            }
            SettingsItemKind::DefaultSortColumn => {
                let value = app.default_sort_label();
                lines.push(kv_row(
                    MenuRowKind::None,
                    &item.label,
                    default_sort_key_w,
                    style,
                    vec![Span::styled(value, Style::default().fg(theme.text))],
                ));
            }
            SettingsItemKind::ToggleEnableModsAfterImport
            | SettingsItemKind::ToggleDeleteModFilesOnRemove
            | SettingsItemKind::SigilLinkToggle
            | SettingsItemKind::SigilLinkAutoPreview
            | SettingsItemKind::ToggleProfileDelete
            | SettingsItemKind::ToggleModDelete
            | SettingsItemKind::ToggleAutoDeploy
            | SettingsItemKind::ToggleDependencyDownloads
            | SettingsItemKind::ToggleDependencyWarnings
            | SettingsItemKind::ToggleStartupDependencyNotice => {
                let enabled = item.checked.unwrap_or(false);
                let state_label = if enabled { "ON" } else { "OFF" };
                let state_style = Style::default()
                    .fg(if enabled {
                        theme.success
                    } else {
                        theme.warning
                    })
                    .add_modifier(Modifier::BOLD);
                let key_width = if matches!(
                    item.kind,
                    SettingsItemKind::SigilLinkToggle | SettingsItemKind::SigilLinkAutoPreview
                ) {
                    sigilink_key_w
                } else {
                    general_key_w
                };
                lines.push(kv_row(
                    MenuRowKind::None,
                    &item.label,
                    key_width,
                    style,
                    vec![Span::styled(state_label, state_style)],
                ));
            }
            SettingsItemKind::SigilLinkHeader
            | SettingsItemKind::SigilLinkDebugHeader
            | SettingsItemKind::ProfilesHeader
            | SettingsItemKind::SigilLinkInfo => {
                let (key, value) = item
                    .label
                    .split_once(": ")
                    .map(|(key, value)| (key.to_string(), value.to_string()))
                    .unwrap_or_else(|| (item.label.clone(), String::new()));
                let value_spans = if value.is_empty() {
                    vec![Span::styled("", Style::default().fg(theme.muted))]
                } else {
                    vec![Span::styled(value, Style::default().fg(theme.muted))]
                };
                lines.push(kv_row(
                    MenuRowKind::None,
                    &key,
                    sigilink_key_w,
                    Style::default().fg(theme.muted),
                    value_spans,
                ));
            }
        };
    }

    lines.push(Line::from(""));

    lines.push(menu_row(
        false,
        MenuRowKind::None,
        Span::styled(
            "Hotkeys",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        theme,
    ));
    for (key, value) in hotkey_rows {
        lines.push(kv_row(
            MenuRowKind::None,
            key,
            hotkey_key_w,
            Style::default().fg(theme.muted),
            vec![Span::styled(value, Style::default().fg(theme.muted))],
        ));
    }

    lines.push(Line::from(""));
    lines.push(menu_row(
        false,
        MenuRowKind::None,
        Span::styled(
            "Enter: Toggle/Run | Esc: Close",
            Style::default().fg(theme.muted),
        ),
        theme,
    ));

    if let Some(index) = whats_new_index {
        if let Some(item) = items.get(index) {
            lines.push(Line::from(""));
            let style = if index == selected {
                Style::default()
                    .fg(theme.success)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            };
            lines.push(menu_row(
                index == selected,
                MenuRowKind::Action,
                Span::styled(item.label.to_string(), style),
                theme,
            ));
        }
    }

    lines.push(Line::from(""));
    let footer_style = Style::default().fg(theme.accent);
    lines.push(centered_line(
        &format!("Updates : {updates_value}"),
        Some(content_width),
        footer_style,
    ));
    lines.push(centered_line(
        &format!("Version : {version_value}"),
        Some(content_width),
        footer_style,
    ));

    lines
}

fn build_export_menu_lines(theme: &Theme, menu: &crate::app::ExportMenu) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        format!("Profile: {}", menu.profile),
        Style::default().fg(theme.muted),
    )));
    lines.push(Line::from(""));

    let items = export_menu_items();
    for (index, item) in items.iter().enumerate() {
        let prefix = if index == menu.selected { ">" } else { " " };
        let style = if index == menu.selected {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text)
        };
        lines.push(Line::from(vec![
            Span::styled(prefix.to_string(), style),
            Span::raw(" "),
            Span::styled(item.label.clone(), style),
        ]));
        let help = match item.kind {
            ExportMenuItemKind::ExportModList => {
                "Recommended: order + enabled + overrides for SigilSmith sync."
            }
            ExportMenuItemKind::ExportModListClipboard => {
                "Clipboard JSON for quick share/paste into SigilSmith."
            }
            ExportMenuItemKind::ExportModsettings => {
                "Interop for BG3MM/Vortex; disabled state may be lost."
            }
        };
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(help, Style::default().fg(theme.muted)),
        ]));
        lines.push(Line::from(""));
    }

    let content_width = lines
        .iter()
        .map(|line| display_width(&line.to_string()))
        .max()
        .unwrap_or(0)
        .max(1);
    let key_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(theme.muted);
    let footer_parts = vec![
        ("[Enter/Space]".to_string(), key_style),
        (" Select  ".to_string(), text_style),
        ("[Esc]".to_string(), key_style),
        (" Cancel".to_string(), text_style),
    ];
    let footer_width: usize = footer_parts
        .iter()
        .map(|(text, _)| display_width(text))
        .sum();
    let pad = if content_width > footer_width {
        (content_width - footer_width) / 2
    } else {
        0
    };
    let mut spans = Vec::new();
    if pad > 0 {
        spans.push(Span::raw(" ".repeat(pad)));
    }
    for (text, style) in footer_parts {
        spans.push(Span::styled(text, style));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(spans));
    lines
}

fn update_status_line(app: &App) -> String {
    match &app.update_status {
        UpdateStatus::Checking => "Updates: Checking...".to_string(),
        UpdateStatus::Available { info, .. } => {
            format!("Updates: v{} Available (Press Enter)", info.version)
        }
        UpdateStatus::Applied { info } => format!("Updates: Applied v{} (Restart)", info.version),
        UpdateStatus::UpToDate { version } => format!("Updates: Latest (v{})", version),
        UpdateStatus::Failed { error } => format!("Updates: Failed ({error})"),
        UpdateStatus::Skipped { version, reason } => {
            format!("Updates: v{version} Skipped ({reason})")
        }
        UpdateStatus::Idle => "Updates: Not Checked".to_string(),
    }
}

fn mode_toast(app: &App) -> Option<(String, ToastLevel)> {
    if app.dialog.is_some() {
        return None;
    }

    match &app.input_mode {
        InputMode::Editing {
            buffer,
            purpose,
            auto_submit,
            ..
        } => {
            let default_hint = "Enter confirm | Esc cancel";
            let hint = if *auto_submit {
                match purpose {
                    InputPurpose::FilterMods => "Pause/Enter to search | Esc cancel",
                    _ => "Pause/Enter to apply | Esc cancel",
                }
            } else if matches!(purpose, InputPurpose::FilterMods) {
                "Enter search | Esc cancel"
            } else {
                default_hint
            };
            let value = |placeholder: &str| {
                let trimmed = buffer.trim();
                if trimmed.is_empty() {
                    placeholder.to_string()
                } else {
                    buffer.to_string()
                }
            };
            let message = match purpose {
                InputPurpose::CreateProfile => {
                    let name = value("<new name>");
                    format!("Create profile: \"{name}\" | {hint}")
                }
                InputPurpose::RenameProfile { original } => {
                    let name = value("<new name>");
                    format!("Renaming \"{original}\" -> \"{name}\" | {hint}")
                }
                InputPurpose::DuplicateProfile { source } => {
                    let name = value("<new name>");
                    format!("Duplicate \"{source}\" -> \"{name}\" | {hint}")
                }
                InputPurpose::ExportProfile { profile, .. } => {
                    let path = value("<path>");
                    format!("Export \"{profile}\": {path} | {hint}")
                }
                InputPurpose::ImportProfile => {
                    let path = value("<path>");
                    format!("Import mod list: {path} | {hint}")
                }
                InputPurpose::ImportPath => {
                    let path = value("<path>");
                    format!("Import mod: {path} | {hint}")
                }
                InputPurpose::FilterMods => {
                    let filter = value("<all>");
                    format!("Search mods: {filter} | {hint}")
                }
            };
            Some((message, ToastLevel::Info))
        }
        InputMode::Browsing(_) => None,
        InputMode::Normal => {
            if app.move_mode {
                Some((
                    "Move mode: arrows reorder | Enter/Space/M confirm | Esc cancel".to_string(),
                    ToastLevel::Info,
                ))
            } else {
                None
            }
        }
    }
}

fn render_toast(
    frame: &mut Frame<'_>,
    theme: &Theme,
    body_area: Rect,
    message: &str,
    level: ToastLevel,
) {
    let mut message = message.to_string();
    let padding_x = 2u16;
    let padding_y = 1u16;
    let max_width = body_area.width.saturating_sub(4).max(24);
    let max_text = max_width.saturating_sub(2 + padding_x.saturating_mul(2)) as usize;
    if message.len() > max_text {
        message.truncate(max_text.saturating_sub(3));
        message.push_str("...");
    }
    let width = (message.len() as u16 + 2 + padding_x.saturating_mul(2)).clamp(24, max_width);
    let height = 2 + padding_y.saturating_mul(2) + 1;
    let x = body_area.x + (body_area.width.saturating_sub(width)) / 2;
    let mut y = body_area.y + body_area.height / 4;
    if y + height > body_area.y + body_area.height {
        y = body_area.y + body_area.height.saturating_sub(height);
    }
    let toast_area = Rect::new(x, y, width, height);

    let (border, text) = match level {
        crate::app::ToastLevel::Info => (theme.accent, theme.text),
        crate::app::ToastLevel::Warn => (theme.warning, theme.text),
        crate::app::ToastLevel::Error => (theme.error, theme.text),
    };

    frame.render_widget(Clear, toast_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .style(Style::default().bg(theme.header_bg))
        .padding(Padding {
            left: padding_x,
            right: padding_x,
            top: padding_y,
            bottom: padding_y,
        });
    let content = Paragraph::new(message)
        .block(block)
        .style(Style::default().fg(text))
        .alignment(Alignment::Center);
    frame.render_widget(content, toast_area);
}

fn draw_toast(frame: &mut Frame<'_>, app: &App, theme: &Theme, body_area: Rect) {
    if let Some((message, level)) = mode_toast(app) {
        render_toast(frame, theme, body_area, &message, level);
        return;
    }

    let Some(toast) = app.toast.as_ref() else {
        return;
    };
    if toast.expires_at <= Instant::now() {
        return;
    }

    render_toast(frame, theme, body_area, &toast.message, toast.level);
}

fn draw_import_overlay(frame: &mut Frame<'_>, app: &App, theme: &Theme) {
    if app.dialog.is_some() || !app.import_overlay_active() {
        return;
    }

    let area = frame.size();

    let progress = app.import_progress();
    let label = progress
        .map(|progress| progress.label.clone())
        .unwrap_or_else(|| "Importing mods...".to_string());
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        "Importing Mods",
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        format!("Source: {label}"),
        Style::default().fg(theme.text),
    )));
    if let Some(progress) = progress {
        lines.push(Line::from(Span::styled(
            format!("Item {}/{}", progress.unit_index, progress.unit_count),
            Style::default().fg(theme.muted),
        )));
        let stage_label = progress.stage.label();
        let stage_line = if progress.stage_total > 1 {
            format!(
                "Stage: {} ({}/{})",
                stage_label, progress.stage_current, progress.stage_total
            )
        } else {
            format!("Stage: {}", stage_label)
        };
        lines.push(Line::from(Span::styled(
            stage_line,
            Style::default().fg(theme.text),
        )));
        if let Some(detail) = &progress.detail {
            lines.push(Line::from(Span::styled(
                detail.clone(),
                Style::default().fg(theme.muted),
            )));
        }
    }
    if app.import_summary_pending() {
        lines.push(Line::from(Span::styled(
            "Failures will be summarized after import completes.",
            Style::default().fg(theme.muted),
        )));
    }

    let text_height = lines.len().max(1) as u16;
    let width = area.width.saturating_sub(10).clamp(42, 78);
    let height = (text_height + 4).min(area.height.saturating_sub(2)).max(9);
    let (outer_area, panel_area) = padded_modal(area, width, height, 2, 1);

    render_modal_backdrop(frame, outer_area, theme);
    let panel_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.overlay_border))
        .style(Style::default().bg(theme.overlay_panel_bg));
    frame.render_widget(panel_block, panel_area);

    let inner = Rect::new(
        panel_area.x + 2,
        panel_area.y + 1,
        panel_area.width.saturating_sub(4),
        panel_area.height.saturating_sub(2),
    );
    let chunks =
        Layout::vertical([Constraint::Length(text_height), Constraint::Length(1)]).split(inner);

    let paragraph = Paragraph::new(lines)
        .style(Style::default().fg(theme.text))
        .alignment(Alignment::Left);
    frame.render_widget(paragraph, chunks[0]);

    let percent = progress
        .map(|progress| (progress.overall_progress * 100.0).round() as u16)
        .unwrap_or(0);
    let gauge = Gauge::default()
        .percent(percent.min(100))
        .gauge_style(
            Style::default()
                .fg(theme.overlay_bar)
                .bg(theme.overlay_panel_bg),
        )
        .label(Span::styled(
            format!("{percent}%"),
            Style::default().fg(theme.text),
        ));
    frame.render_widget(gauge, chunks[1]);
}

fn draw_startup_overlay(frame: &mut Frame<'_>, app: &App, theme: &Theme) {
    if !app.startup_pending() {
        return;
    }

    let area = frame.size();

    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        "Starting SigilSmith",
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        "Loading mods, metadata, and SigiLink ranking…",
        Style::default().fg(theme.text),
    )));
    lines.push(Line::from(Span::styled(
        "This should only take a moment.",
        Style::default().fg(theme.muted),
    )));

    let text_height = lines.len() as u16;
    let width = area.width.saturating_sub(10).clamp(42, 72);
    let height = (text_height + 4).min(area.height.saturating_sub(2)).max(9);
    let (outer_area, panel_area) = padded_modal(area, width, height, 2, 1);

    render_modal_backdrop(frame, outer_area, theme);
    let panel_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.overlay_border))
        .style(Style::default().bg(theme.overlay_panel_bg));
    frame.render_widget(panel_block, panel_area);

    let inner = Rect::new(
        panel_area.x + 2,
        panel_area.y + 1,
        panel_area.width.saturating_sub(4),
        panel_area.height.saturating_sub(2),
    );
    let paragraph = Paragraph::new(lines)
        .style(Style::default().fg(theme.text))
        .alignment(Alignment::Left);
    frame.render_widget(paragraph, inner);
}

fn build_explorer_items(app: &App, theme: &Theme) -> Vec<ListItem<'static>> {
    let items = app.explorer_items();
    items
        .iter()
        .enumerate()
        .map(|(index, item)| ListItem::new(explorer_line(item, theme, &items, index)))
        .collect()
}

fn has_next_at_depth(items: &[ExplorerItem], index: usize, depth: usize) -> bool {
    for item in items.iter().skip(index + 1) {
        if item.depth < depth {
            return false;
        }
        if item.depth == depth {
            return true;
        }
    }
    false
}

fn explorer_prefix(items: &[ExplorerItem], index: usize) -> String {
    let depth = items[index].depth;
    if depth == 0 {
        return String::new();
    }

    let mut out = String::new();
    for level in 1..depth {
        if has_next_at_depth(items, index, level) {
            out.push_str("│  ");
        } else {
            out.push_str("   ");
        }
    }

    let branch = if has_next_at_depth(items, index, depth) {
        "├─ "
    } else {
        "└─ "
    };
    out.push_str(branch);
    out
}

fn explorer_line(
    item: &ExplorerItem,
    theme: &Theme,
    items: &[ExplorerItem],
    index: usize,
) -> Line<'static> {
    let prefix = explorer_prefix(items, index);
    let muted = Style::default().fg(theme.muted);
    let normal = Style::default().fg(theme.text);
    let disabled = Style::default().fg(theme.muted);

    let mut label_style = if item.disabled { disabled } else { normal };
    if item.renaming {
        label_style = label_style.fg(theme.warning).add_modifier(Modifier::BOLD);
    }
    let mut spans = Vec::new();
    if !prefix.is_empty() {
        spans.push(Span::styled(prefix, muted));
    }

    match &item.kind {
        ExplorerItemKind::Game(_) => {
            let expander = if item.expanded { "▾" } else { "▸" };
            spans.push(Span::styled(expander, muted));
            spans.push(Span::raw(" "));
            let marker_style = if item.active {
                Style::default().fg(theme.success)
            } else {
                muted
            };
            let marker = if item.active { "[x]" } else { "[ ]" };
            spans.push(Span::styled(marker, marker_style));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(item.label.clone(), label_style));
        }
        ExplorerItemKind::ProfilesHeader(_) => {
            let expander = if item.expanded { "▾" } else { "▸" };
            spans.push(Span::styled(expander, muted));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                item.label.clone(),
                label_style.fg(theme.accent),
            ));
        }
        ExplorerItemKind::Profile { .. } => {
            let marker_style = if item.active {
                Style::default().fg(theme.success)
            } else {
                muted
            };
            let marker = if item.active { "[x]" } else { "[ ]" };
            spans.push(Span::styled(marker, marker_style));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(item.label.clone(), label_style));
        }
        ExplorerItemKind::NewProfile(_) => {
            spans.push(Span::styled("+", Style::default().fg(theme.accent)));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(item.label.clone(), label_style));
        }
        ExplorerItemKind::Info(_) => {
            spans.push(Span::styled(item.label.clone(), disabled));
        }
    }

    Line::from(spans)
}

struct ModCounts {
    total: usize,
    enabled: usize,
    #[allow(dead_code)]
    visible_total: usize,
}

fn build_rows(app: &App, theme: &Theme) -> (Vec<Row<'static>>, ModCounts, usize, usize) {
    let mut rows = Vec::new();
    let mut target_width = "Target".chars().count();
    let mut mod_width = "Mod Name".chars().count();
    let (total, enabled) = app.profile_counts();
    let profile_entries = app.visible_profile_entries();
    let mod_map = app.library.index_by_id();
    let dep_lookup = app.dependency_lookup();
    let enabled_ids = app.active_profile_enabled_ids();
    let total_rows = profile_entries.len();

    for (_, entry) in &profile_entries {
        if entry.missing_label.is_some() {
            let label = entry
                .missing_label
                .as_deref()
                .filter(|label| !label.trim().is_empty())
                .unwrap_or(&entry.id);
            let display = format!("{} (missing)", label.trim());
            mod_width = mod_width.max(display.chars().count());
            continue;
        }
        let Some(mod_entry) = mod_map.get(&entry.id) else {
            let label = entry
                .missing_label
                .as_deref()
                .filter(|label| !label.trim().is_empty())
                .unwrap_or(&entry.id);
            let display = format!("{} (missing)", label.trim());
            mod_width = mod_width.max(display.chars().count());
            continue;
        };
        mod_width = mod_width.max(mod_entry.display_name().chars().count());
    }

    for (row_index, (order_index, entry)) in profile_entries.iter().enumerate() {
        if entry.missing_label.is_some() {
            let (row, target_len) =
                row_for_missing_entry(app, row_index, *order_index, entry, theme);
            target_width = target_width.max(target_len);
            rows.push(row);
            continue;
        }
        let Some(mod_entry) = mod_map.get(&entry.id) else {
            let (row, target_len) =
                row_for_missing_entry(app, row_index, *order_index, entry, theme);
            target_width = target_width.max(target_len);
            rows.push(row);
            continue;
        };
        let loading = app.mod_row_loading(&entry.id, row_index, total_rows);
        let effective_enabled = entry.enabled && !app.sigillink_missing_pak(&entry.id);
        let (row, target_len) = row_for_entry(
            app,
            row_index,
            *order_index,
            effective_enabled,
            mod_width,
            mod_entry,
            theme,
            dep_lookup.as_ref(),
            &enabled_ids,
            loading,
        );
        target_width = target_width.max(target_len);
        rows.push(row);
    }

    let visible_total = rows.len();
    (
        rows,
        ModCounts {
            total,
            enabled,
            visible_total,
        },
        target_width,
        mod_width,
    )
}

fn mod_header_cell(
    label: &str,
    column: ModSortColumn,
    sort: ModSort,
    theme: &Theme,
) -> Cell<'static> {
    let is_sorted = sort.column == column;
    let style = if is_sorted {
        Style::default()
            .fg(theme.header_bg)
            .bg(theme.section_bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.accent)
            .bg(theme.header_bg)
            .add_modifier(Modifier::BOLD)
    };
    Cell::from(label.to_string()).style(style)
}

fn mod_header_cell_static(label: &str, theme: &Theme) -> Cell<'static> {
    Cell::from(label.to_string()).style(
        Style::default()
            .fg(theme.accent)
            .bg(theme.header_bg)
            .add_modifier(Modifier::BOLD),
    )
}

fn loading_frame(row_index: usize, column_index: usize) -> &'static str {
    const COLUMN_COUNT: i64 = 10;
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let row_seed = (row_index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let speed_ms = lerp_u64(90, 260, rand_f32(row_seed ^ 0xA5));
    let segment_ms = lerp_u64(900, 1600, rand_f32(row_seed ^ 0xC3));
    let segment = now_ms / segment_ms.max(1);
    let local_ms = now_ms % segment_ms.max(1);
    let pause_roll = rand_f32(row_seed ^ segment.wrapping_mul(0xD1));
    let pause_ms = lerp_u64(120, 620, rand_f32(row_seed ^ segment.wrapping_mul(0xE5)));
    let effective_ms = if pause_roll < 0.25 && local_ms < pause_ms {
        now_ms.saturating_sub(local_ms)
    } else {
        now_ms
    };
    let steps = (effective_ms / speed_ms.max(1)) as i64;
    let cycle = (COLUMN_COUNT - 1) * 2;
    let mut pos = (steps % cycle) as i64;
    if pos < 0 {
        pos += cycle;
    }
    let pos = if pos >= COLUMN_COUNT {
        cycle - pos
    } else {
        pos
    };
    if column_index as i64 == pos {
        if rand_f32(row_seed ^ (column_index as u64).wrapping_mul(0xD7) ^ now_ms.rotate_left(9))
            < 0.12
        {
            random_loading_symbol(row_seed ^ now_ms)
        } else {
            "·"
        }
    } else {
        " "
    }
}

fn rand_f32(seed: u64) -> f32 {
    let mut x = seed ^ 0xA24B_8B6F_1D2F_3A5D;
    x ^= x << 7;
    x ^= x >> 9;
    x ^= x << 8;
    let upper = (x >> 40) as u32;
    (upper as f32) / ((1u32 << 24) as f32)
}

fn random_loading_symbol(seed: u64) -> &'static str {
    const SYMBOLS: [&str; 4] = ["*", "+", "x", "#"];
    let pick = (seed.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 62) as usize;
    SYMBOLS[pick % SYMBOLS.len()]
}

fn loading_name_overlay(name: &str, row_index: usize, pad_width: usize) -> String {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let row_seed = (row_index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let name_len = name.chars().count();
    let target_width = pad_width.max(name_len);
    let tail_blank = 2usize;
    let mut base: Vec<char> = name.chars().collect();
    if target_width > name_len {
        base.extend(std::iter::repeat(' ').take(target_width - name_len));
    }
    let soft_end = if base.len() > name_len + tail_blank {
        base.len().saturating_sub(tail_blank)
    } else {
        base.len()
    };
    let frame = now_ms / 220;
    let parity = ((frame ^ row_seed) & 1) as usize;
    let mut space_indices: Vec<usize> = base
        .iter()
        .enumerate()
        .filter_map(|(idx, ch)| {
            if idx >= name_len && idx < soft_end && *ch == ' ' && idx % 2 == parity {
                Some(idx)
            } else {
                None
            }
        })
        .collect();
    if space_indices.is_empty() {
        space_indices = base
            .iter()
            .enumerate()
            .filter_map(|(idx, ch)| {
                if idx >= name_len && idx < soft_end && *ch == ' ' {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect();
    }
    if space_indices.is_empty() {
        space_indices = base
            .iter()
            .enumerate()
            .filter_map(|(idx, ch)| if *ch == ' ' { Some(idx) } else { None })
            .collect();
    }
    if space_indices.is_empty() {
        return base.into_iter().collect();
    }

    let last_space_idx = space_indices.last().copied();
    let mut rng = row_seed ^ now_ms.rotate_left(9) ^ frame.rotate_left(7);
    let roll = (rng % 10) as u8;
    let mut desired = if roll < 6 {
        0
    } else if roll < 9 {
        1
    } else {
        2
    };
    let force_last = last_space_idx.is_some()
        && rand_f32(row_seed ^ now_ms.rotate_left(5) ^ frame.rotate_left(11)) < 0.08;
    if force_last && desired == 0 {
        desired = 1;
    }
    if desired == 0 && frame % 8 == 0 {
        desired = 1;
    }
    desired = desired.min(space_indices.len().min(2));
    if desired == 0 {
        if target_width > name_len {
            let tail_start = target_width.saturating_sub(tail_blank).max(name_len);
            for idx in tail_start..target_width {
                if let Some(ch) = base.get_mut(idx) {
                    *ch = ' ';
                }
            }
        }
        return base.into_iter().collect();
    }
    let mut picks: Vec<usize> = Vec::new();
    let mut guard = 0usize;
    while picks.len() < desired && guard < space_indices.len() * 3 {
        rng = rng
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(0xA5A5_5A5A);
        let pos = space_indices[(rng as usize) % space_indices.len()];
        if picks.iter().any(|picked| {
            let delta = (*picked as isize - pos as isize).abs();
            delta <= 2
        }) {
            guard += 1;
            continue;
        }
        picks.push(pos);
        guard += 1;
    }
    if force_last {
        if let Some(last_idx) = last_space_idx {
            if picks.is_empty() {
                picks.push(last_idx);
            } else if !picks.contains(&last_idx) {
                picks[0] = last_idx;
            }
        }
    }
    if picks.is_empty() {
        picks.push(space_indices[(rng as usize) % space_indices.len()]);
    }
    for (idx, pos) in picks.into_iter().enumerate() {
        let seed = row_seed ^ ((idx as u64 + 1) * 0xD7) ^ now_ms;
        base[pos] = random_loading_symbol(seed).chars().next().unwrap_or(' ');
    }
    if target_width > name_len {
        let tail_start = target_width.saturating_sub(tail_blank).max(name_len);
        for idx in tail_start..target_width {
            if let Some(ch) = base.get_mut(idx) {
                *ch = ' ';
            }
        }
    }
    base.into_iter().collect()
}

fn lerp_u64(min: u64, max: u64, t: f32) -> u64 {
    let clamped = t.clamp(0.0, 1.0);
    let span = max.saturating_sub(min) as f32;
    min + (span * clamped) as u64
}

fn row_for_missing_entry(
    _app: &App,
    row_index: usize,
    order_index: usize,
    entry: &crate::library::ProfileEntry,
    theme: &Theme,
) -> (Row<'static>, usize) {
    let label = entry
        .missing_label
        .as_deref()
        .filter(|label| !label.trim().is_empty())
        .unwrap_or(&entry.id);
    let display = format!("{} (missing)", label.trim());
    let enabled_text = if entry.enabled { "[x] " } else { "[ ] " };
    let muted = Style::default().fg(theme.muted);
    let order_text = format_order_cell(order_index);
    let link_cell = Cell::from(" ".to_string()).style(muted);
    let dep_cell = Cell::from(Line::from(vec![
        Span::styled(" ", muted),
        Span::styled("  ", muted),
        Span::styled("  ", muted),
        Span::styled(" ", muted),
    ]));
    let mut row = Row::new(vec![
        Cell::from(enabled_text.to_string()).style(muted),
        Cell::from(order_text).style(muted),
        Cell::from(" ".to_string()).style(muted),
        Cell::from(" ".to_string()).style(muted),
        dep_cell,
        link_cell,
        Cell::from(display).style(muted),
        Cell::from(" ".to_string()).style(muted),
        Cell::from(" ".to_string()).style(muted),
        Cell::from(" ".to_string()).style(muted),
        Cell::from(" ".to_string()).style(muted),
        Cell::from(" ".to_string()).style(muted),
        Cell::from(" ".to_string()).style(muted),
    ]);
    if row_index % 2 == 1 {
        row = row.style(Style::default().bg(theme.row_alt_bg));
    }
    (row, 1)
}

fn row_for_entry(
    app: &App,
    row_index: usize,
    order_index: usize,
    enabled: bool,
    mod_name_pad: usize,
    mod_entry: &ModEntry,
    theme: &Theme,
    dep_lookup: Option<&crate::app::DependencyLookup>,
    enabled_ids: &HashSet<String>,
    loading: bool,
) -> (Row<'static>, usize) {
    let (state_label, state_style) = mod_path_label(app, mod_entry, theme, true);
    let target_len = state_label.chars().count();
    let mut row = if loading {
        let loading_style = Style::default().fg(theme.muted);
        let mut cells = Vec::with_capacity(16);
        let mut loading_index = 0usize;
        let push_loading = |cells: &mut Vec<Cell<'static>>, index: &mut usize| {
            let frame = loading_frame(row_index, *index);
            *index = index.saturating_add(1);
            cells.push(Cell::from(frame.to_string()).style(loading_style));
        };
        push_loading(&mut cells, &mut loading_index); // On
        push_loading(&mut cells, &mut loading_index); // #
        push_loading(&mut cells, &mut loading_index); // N
        push_loading(&mut cells, &mut loading_index); // Kind
        push_loading(&mut cells, &mut loading_index); // Dep
        push_loading(&mut cells, &mut loading_index); // 🔗
        let display_name = mod_entry.display_name();
        let name_overlay = loading_name_overlay(&display_name, row_index, mod_name_pad);
        cells.push(Cell::from(name_overlay).style(loading_style));
        cells.push(Cell::from(" ").style(loading_style));
        push_loading(&mut cells, &mut loading_index); // Created
        cells.push(Cell::from(" ").style(loading_style));
        push_loading(&mut cells, &mut loading_index); // Added
        cells.push(Cell::from(" ").style(loading_style));
        push_loading(&mut cells, &mut loading_index); // Target
        Row::new(cells)
    } else {
        let has_override = !mod_entry.target_overrides.is_empty();
        let (missing, disabled) = dep_lookup
            .map(|lookup| app.dependency_counts_for_mod(mod_entry, lookup, enabled_ids))
            .unwrap_or((0, 0));
        let (enabled_text, enabled_style) = if enabled {
            let color = if has_override || missing > 0 {
                theme.warning
            } else {
                theme.success
            };
            ("[x] ", Style::default().fg(color))
        } else {
            ("[ ] ", Style::default().fg(theme.muted))
        };
        let kind = mod_kind_label(mod_entry);
        let kind_style = match kind {
            "Pak" => Style::default().fg(theme.accent),
            "Loose" => Style::default().fg(theme.success),
            _ => Style::default().fg(theme.text),
        };
        let native_marker = if mod_entry.is_native() {
            " ✓ "
        } else {
            "   "
        };
        let native_style = if mod_entry.is_native() {
            Style::default().fg(theme.success)
        } else {
            Style::default().fg(theme.muted)
        };
        let created_text = format_date_cell(mod_entry.created_at);
        let added_text = format_date_cell(Some(mod_entry.added_at));
        let missing_text = dep_count_segment(missing);
        let disabled_text = dep_count_segment(disabled);
        let missing_style = if missing > 0 {
            Style::default().fg(theme.warning)
        } else {
            Style::default().fg(theme.muted)
        };
        let disabled_style = if disabled > 0 {
            Style::default().fg(theme.accent)
        } else {
            Style::default().fg(theme.muted)
        };
        let dep_cell = Cell::from(Line::from(vec![
            Span::styled(" ", Style::default().fg(theme.muted)),
            Span::styled(missing_text, missing_style),
            Span::styled(disabled_text, disabled_style),
            Span::styled(" ", Style::default().fg(theme.muted)),
        ]));
        let order_style = Style::default().fg(theme.text);
        let link_cell = sigillink_link_cell(app, &mod_entry.id, theme);
        let order_text = format_order_cell(order_index);
        let name_cell = mod_name_cell(app, mod_entry, theme);
        Row::new(vec![
            Cell::from(enabled_text.to_string()).style(enabled_style),
            Cell::from(order_text).style(order_style),
            Cell::from(native_marker.to_string()).style(native_style),
            Cell::from(kind.to_string()).style(kind_style),
            dep_cell,
            link_cell,
            name_cell,
            Cell::from(" "),
            Cell::from(created_text).style(Style::default().fg(theme.muted)),
            Cell::from(" "),
            Cell::from(added_text).style(Style::default().fg(theme.muted)),
            Cell::from(" "),
            Cell::from(state_label).style(state_style),
        ])
    };
    if row_index % 2 == 1 {
        row = row.style(Style::default().bg(theme.row_alt_bg));
    }
    (row, target_len)
}

fn sigillink_link_cell(app: &App, mod_id: &str, theme: &Theme) -> Cell<'static> {
    if app.sigillink_missing_pak(mod_id) {
        return Cell::from("👻".to_string()).style(Style::default().fg(theme.warning));
    }
    if !app.sigillink_ranking_enabled() {
        return Cell::from(" ".to_string()).style(Style::default().fg(theme.muted));
    }
    let (glyph, style) = if app.sigillink_is_pinned(mod_id) {
        ("⛕", Style::default().fg(theme.warning))
    } else {
        ("⛓", Style::default().fg(theme.success))
    };
    Cell::from(glyph.to_string()).style(style)
}

fn mod_name_cell(app: &App, mod_entry: &ModEntry, theme: &Theme) -> Cell<'static> {
    if app.sigillink_missing_pak(&mod_entry.id) {
        let name_style = Style::default()
            .fg(theme.text)
            .add_modifier(Modifier::CROSSED_OUT);
        Cell::from(Line::from(Span::styled(
            mod_entry.display_name(),
            name_style,
        )))
    } else {
        Cell::from(mod_entry.display_name())
    }
}

fn format_order_cell(order_index: usize) -> String {
    format!("{:^3}", order_index.saturating_add(1))
}

fn dep_count_segment(count: usize) -> String {
    let count = count.min(99);
    if count == 0 {
        "  ".to_string()
    } else if count < 10 {
        format!(" {}", count)
    } else {
        format!("{count:02}")
    }
}

fn truncate_text(value: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let len = value.chars().count();
    if len <= max_width {
        return value.to_string();
    }
    if max_width <= 3 {
        return value.chars().take(max_width).collect();
    }
    let take = max_width.saturating_sub(3);
    let mut out = value.chars().take(take).collect::<String>();
    out.push_str("...");
    out
}

fn truncate_spans(parts: Vec<(String, Style)>, max_width: usize) -> Vec<Span<'static>> {
    if max_width == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut used = 0usize;
    for (text, style) in parts {
        let len = text.chars().count();
        if used + len <= max_width {
            out.push(Span::styled(text, style));
            used += len;
            continue;
        }
        let remaining = max_width.saturating_sub(used);
        if remaining == 0 {
            break;
        }
        let truncated = truncate_text(&text, remaining);
        out.push(Span::styled(truncated, style));
        break;
    }
    out
}

fn display_width(value: &str) -> usize {
    value
        .chars()
        .map(|ch| {
            if matches!(ch, '🔗' | '⛓' | '⛕') {
                2
            } else {
                1
            }
        })
        .sum()
}

fn split_value_width(left: &str, right: &str) -> usize {
    let left_len = display_width(left);
    let right_len = display_width(right);
    let gap = if left.is_empty() || right.is_empty() {
        0
    } else {
        1
    };
    left_len + gap + right_len
}

fn format_rank_timestamp(timestamp: Option<i64>) -> String {
    let Some(timestamp) = timestamp else {
        return "never".to_string();
    };
    if timestamp <= 0 {
        return "never".to_string();
    }
    let Some(date) = format_short_date(timestamp) else {
        return "never".to_string();
    };
    let time = time::OffsetDateTime::from_unix_timestamp(timestamp)
        .ok()
        .map(|dt| format!("{:02}:{:02}", dt.hour(), dt.minute()))
        .unwrap_or_else(|| "--:--".to_string());
    format!("{date} {time}")
}

fn format_short_date(timestamp: i64) -> Option<String> {
    if timestamp <= 0 {
        return None;
    }
    let date = time::OffsetDateTime::from_unix_timestamp(timestamp).ok()?;
    let year = date.year();
    let month = date.month() as u8;
    let day = date.day();
    let locale = locale_hint();
    let formatted = if prefers_mdy(&locale) {
        format!("{month:02}-{day:02}-{year:04}")
    } else if prefers_ymd(&locale) {
        format!("{year:04}-{month:02}-{day:02}")
    } else {
        format!("{day:02}-{month:02}-{year:04}")
    };
    Some(formatted)
}

fn locale_hint() -> String {
    std::env::var("LC_TIME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| std::env::var("LANG").ok())
        .unwrap_or_default()
        .to_ascii_uppercase()
}

fn prefers_mdy(locale: &str) -> bool {
    locale.contains("US") || locale.contains("PH")
}

fn prefers_ymd(locale: &str) -> bool {
    locale.contains("CN")
        || locale.contains("JP")
        || locale.contains("KR")
        || locale.contains("TW")
        || locale.contains("HU")
}

fn format_date_cell(value: Option<i64>) -> String {
    if let Some(value) = value {
        if let Some(formatted) = format_short_date(value) {
            return formatted;
        }
    }
    format_blank_date()
}

fn format_blank_date() -> String {
    let locale = locale_hint();
    if prefers_ymd(&locale) {
        "---- -- --".to_string()
    } else {
        "-- -- ----".to_string()
    }
}

fn pad_lines(lines: Vec<Line<'static>>, left_pad: usize, top_pad: usize) -> Vec<Line<'static>> {
    if left_pad == 0 && top_pad == 0 {
        return lines;
    }

    let mut out = Vec::with_capacity(lines.len() + top_pad);
    for _ in 0..top_pad {
        out.push(Line::from(""));
    }

    let prefix = " ".repeat(left_pad);
    for line in lines {
        if left_pad == 0 {
            out.push(line);
            continue;
        }
        let mut spans = Vec::with_capacity(line.spans.len() + 1);
        spans.push(Span::raw(prefix.clone()));
        spans.extend(line.spans);
        out.push(Line::from(spans));
    }

    out
}

#[allow(dead_code)]
fn push_truncated_prefixed(
    lines: &mut Vec<Line<'static>>,
    prefix: &str,
    prefix_style: Style,
    value: &str,
    value_style: Style,
    max_width: usize,
) {
    if max_width == 0 {
        return;
    }

    let prefix_len = prefix.len();
    let available = max_width.saturating_sub(prefix_len);
    let value_text = truncate_text(value, available);
    lines.push(Line::from(vec![
        Span::styled(prefix.to_string(), prefix_style),
        Span::styled(value_text, value_style),
    ]));
}

fn build_details(app: &App, theme: &Theme, width: usize, height: usize) -> Vec<Line<'static>> {
    if app.focus == Focus::Explorer {
        return build_explorer_details(app, theme, width);
    }
    if app.focus == Focus::Conflicts {
        return build_conflict_details(app, theme, width, height);
    }

    let profile_entries = app.visible_profile_entries();
    let mod_map = app.library.index_by_id();

    let Some((order_index, entry)) = profile_entries.get(app.selected) else {
        return vec![Line::from("No mod selected.")];
    };
    if entry.missing_label.is_some() || !mod_map.contains_key(&entry.id) {
        let label = entry
            .missing_label
            .as_deref()
            .filter(|label| !label.trim().is_empty())
            .unwrap_or(&entry.id);
        let display = format!("{} (missing)", label.trim());
        let label_style = Style::default().fg(theme.muted);
        let value_style = Style::default().fg(theme.muted);
        let mut rows = Vec::new();
        rows.push(KvRow {
            label: "Name".to_string(),
            value: display,
            label_style,
            value_style,
        });
        rows.push(KvRow {
            label: "Status".to_string(),
            value: "Missing mod".to_string(),
            label_style,
            value_style,
        });
        rows.push(KvRow {
            label: "Enabled".to_string(),
            value: if entry.enabled { "Yes" } else { "No" }.to_string(),
            label_style,
            value_style,
        });
        rows.push(KvRow {
            label: "Order".to_string(),
            value: (order_index + 1).to_string(),
            label_style,
            value_style,
        });
        rows.push(KvRow {
            label: "ID".to_string(),
            value: entry.id.clone(),
            label_style,
            value_style,
        });
        return format_kv_lines(&rows, width);
    }
    let Some(mod_entry) = mod_map.get(&entry.id) else {
        return vec![Line::from("No mod selected.")];
    };

    let label_style = Style::default().fg(theme.muted);
    let value_style = Style::default().fg(theme.text);
    let mut rows = Vec::new();
    let display_name = mod_entry.display_name();
    rows.push(KvRow {
        label: "Name".to_string(),
        value: display_name.clone(),
        label_style,
        value_style,
    });
    let added_label = format_short_date(mod_entry.added_at).unwrap_or_else(|| "-".to_string());
    rows.push(KvRow {
        label: "Added".to_string(),
        value: added_label,
        label_style,
        value_style,
    });
    if let Some(modified_at) = mod_entry.modified_at {
        if let Some(modified_label) = format_short_date(modified_at) {
            rows.push(KvRow {
                label: "Modified".to_string(),
                value: modified_label,
                label_style,
                value_style,
            });
        }
    }
    if let Some(created_at) = mod_entry.created_at {
        if let Some(created_label) = format_short_date(created_at) {
            rows.push(KvRow {
                label: "Created".to_string(),
                value: created_label,
                label_style,
                value_style,
            });
        }
    }
    if mod_entry.is_native() {
        let is_modio = mod_entry.targets.iter().any(|target| match target {
            InstallTarget::Pak { info, .. } => info.publish_handle.is_some(),
            _ => false,
        });
        let source_value = if is_modio {
            "Native (mod.io)"
        } else {
            "Native (Larian Mods)"
        };
        rows.push(KvRow {
            label: "Source".to_string(),
            value: source_value.to_string(),
            label_style,
            value_style: Style::default().fg(theme.accent),
        });
    }
    if display_name != mod_entry.name {
        rows.push(KvRow {
            label: "Internal".to_string(),
            value: mod_entry.name.clone(),
            label_style,
            value_style,
        });
    }
    if let Some(source_label) = mod_entry.source_label() {
        if source_label != display_name {
            rows.push(KvRow {
                label: "Source".to_string(),
                value: source_label.to_string(),
                label_style,
                value_style,
            });
        }
    }
    let effective_enabled = entry.enabled && !app.sigillink_missing_pak(&entry.id);
    let enabled_label = if effective_enabled { "Yes" } else { "No" };
    let enabled_style = Style::default().fg(if effective_enabled {
        theme.success
    } else {
        theme.muted
    });
    rows.push(KvRow {
        label: "Enabled".to_string(),
        value: enabled_label.to_string(),
        label_style,
        value_style: enabled_style,
    });
    let order_label = (order_index + 1).to_string();
    rows.push(KvRow {
        label: "Order".to_string(),
        value: order_label,
        label_style,
        value_style,
    });
    if app.sigillink_ranking_enabled() && app.sigillink_is_pinned(&entry.id) {
        rows.push(KvRow {
            label: "SigiLink Unlinked".to_string(),
            value: "ON (Ctrl+R to reset)".to_string(),
            label_style,
            value_style: Style::default().fg(theme.warning),
        });
    }
    let type_label = mod_entry.display_type();
    rows.push(KvRow {
        label: "Type".to_string(),
        value: type_label,
        label_style,
        value_style,
    });
    let targets_label = targets_summary(mod_entry);
    rows.push(KvRow {
        label: "Targets".to_string(),
        value: targets_label,
        label_style,
        value_style,
    });
    let (path_label, path_style) = mod_path_label(app, mod_entry, theme, false);
    rows.push(KvRow {
        label: "Path".to_string(),
        value: path_label.to_string(),
        label_style,
        value_style: path_style,
    });
    let (override_label, override_style) = mod_override_label(mod_entry, theme, false);
    rows.push(KvRow {
        label: "Target".to_string(),
        value: override_label,
        label_style,
        value_style: override_style,
    });
    rows.push(KvRow {
        label: "ID".to_string(),
        value: mod_entry.id.clone(),
        label_style,
        value_style,
    });

    if let Some(info) = mod_entry.targets.iter().find_map(|target| match target {
        InstallTarget::Pak { info, .. } => Some(info),
        _ => None,
    }) {
        rows.push(KvRow {
            label: "Folder".to_string(),
            value: info.folder.clone(),
            label_style,
            value_style,
        });
        let version_label = info.version.to_string();
        rows.push(KvRow {
            label: "Version".to_string(),
            value: version_label,
            label_style,
            value_style,
        });
    }

    format_kv_lines(&rows, width)
}

fn build_explorer_details(app: &App, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let Some(item) = app.explorer_selected_item() else {
        return vec![Line::from("No selection.")];
    };

    match item.kind {
        ExplorerItemKind::Game(game_id) => {
            let label_style = Style::default().fg(theme.muted);
            let value_style = Style::default().fg(theme.text);
            let mut rows = vec![KvRow {
                label: "Game".to_string(),
                value: game_id.display_name().to_string(),
                label_style,
                value_style,
            }];
            if game_id == app.game_id {
                let root = if app.config.game_root.as_os_str().is_empty() {
                    "Not set".to_string()
                } else {
                    app.config.game_root.display().to_string()
                };
                let user_dir = if app.config.larian_dir.as_os_str().is_empty() {
                    "Not set".to_string()
                } else {
                    app.config.larian_dir.display().to_string()
                };
                rows.push(KvRow {
                    label: "Root".to_string(),
                    value: root,
                    label_style,
                    value_style,
                });
                rows.push(KvRow {
                    label: "User dir".to_string(),
                    value: user_dir,
                    label_style,
                    value_style,
                });
                rows.push(KvRow {
                    label: "Config".to_string(),
                    value: app
                        .config
                        .data_dir
                        .join("config.json")
                        .display()
                        .to_string(),
                    label_style,
                    value_style,
                });
                let status_style = Style::default().fg(if app.paths_ready() {
                    theme.success
                } else {
                    theme.warning
                });
                let status_label = if app.paths_ready() {
                    "Ready"
                } else {
                    "Setup required"
                };
                rows.push(KvRow {
                    label: "Status".to_string(),
                    value: status_label.to_string(),
                    label_style,
                    value_style: status_style,
                });
                format_kv_lines(&rows, width)
            } else {
                vec![Line::from(Span::styled(
                    truncate_text("Select game to load profiles.", width),
                    Style::default().fg(theme.muted),
                ))]
            }
        }
        ExplorerItemKind::Profile { name, .. } => {
            let label_style = Style::default().fg(theme.muted);
            let value_style = Style::default().fg(theme.text);
            let mut display_name = name.clone();
            if let Some((original, buffer)) = app.rename_preview() {
                if original == name {
                    let trimmed = buffer.trim();
                    display_name = if trimmed.is_empty() {
                        "<new name>".to_string()
                    } else {
                        buffer
                    };
                }
            }
            let mut rows = vec![KvRow {
                label: "Profile".to_string(),
                value: display_name,
                label_style,
                value_style,
            }];
            if let Some(profile) = app
                .library
                .profiles
                .iter()
                .find(|profile| profile.name == name)
            {
                let enabled = profile.order.iter().filter(|entry| entry.enabled).count();
                let mods_label = profile.order.len().to_string();
                let enabled_label = enabled.to_string();
                rows.push(KvRow {
                    label: "Mods".to_string(),
                    value: mods_label,
                    label_style,
                    value_style,
                });
                let enabled_style = Style::default().fg(theme.success);
                rows.push(KvRow {
                    label: "Enabled".to_string(),
                    value: enabled_label,
                    label_style,
                    value_style: enabled_style,
                });
            }
            let mut lines = format_kv_lines(&rows, width);
            if app.is_renaming_profile(&name) {
                lines.push(Line::from(Span::styled(
                    "Renaming...",
                    Style::default().fg(theme.warning),
                )));
            }
            if item.active {
                lines.push(Line::from(Span::styled(
                    "Active profile",
                    Style::default().fg(theme.accent),
                )));
            }
            lines
        }
        ExplorerItemKind::ProfilesHeader(_) => vec![Line::from(Span::styled(
            truncate_text("Profiles in this game.", width),
            Style::default().fg(theme.muted),
        ))],
        ExplorerItemKind::NewProfile(_) => vec![Line::from(Span::styled(
            truncate_text("Press Enter to create a new profile.", width),
            Style::default().fg(theme.muted),
        ))],
        ExplorerItemKind::Info(_) => vec![Line::from(Span::styled(
            truncate_text("Select the game to inspect profiles.", width),
            Style::default().fg(theme.muted),
        ))],
    }
}

struct ConflictListMetrics {
    total: usize,
    list_height: usize,
    offset: usize,
    show_scroll: bool,
}

fn conflict_status_label(app: &App) -> Option<&'static str> {
    let override_reason = app.deploy_reason_contains("conflict override");
    if app.deploy_active() && override_reason {
        Some("Loading...")
    } else if app.deploy_pending() && override_reason {
        Some("Queued...")
    } else if app.override_swap.is_some() {
        Some("Applied")
    } else {
        None
    }
}

fn conflict_list_metrics(
    total: usize,
    selected: usize,
    height: usize,
    footer_len: usize,
) -> ConflictListMetrics {
    let header_height = 1usize;
    let mut list_height = height.saturating_sub(header_height + footer_len);
    if list_height == 0 {
        list_height = 1;
    }

    let mut offset = 0usize;
    if total > list_height {
        if selected >= list_height {
            offset = selected + 1 - list_height;
        }
        let max_offset = total.saturating_sub(list_height);
        if offset > max_offset {
            offset = max_offset;
        }
    }

    ConflictListMetrics {
        total,
        list_height,
        offset,
        show_scroll: total > list_height,
    }
}

fn build_conflict_details(
    app: &App,
    theme: &Theme,
    width: usize,
    height: usize,
) -> Vec<Line<'static>> {
    if width == 0 || height == 0 {
        return Vec::new();
    }

    let center_line = |text: &str, style: Style| -> Line<'static> {
        let trimmed = truncate_text(text, width);
        let len = display_width(&trimmed);
        if len >= width {
            return Line::from(Span::styled(trimmed, style));
        }
        let pad = width.saturating_sub(len) / 2;
        Line::from(Span::styled(
            format!("{}{}", " ".repeat(pad), trimmed),
            style,
        ))
    };

    if app.conflicts_scanning() {
        return vec![center_line(
            "Scanning overrides...",
            Style::default().fg(theme.muted),
        )];
    }
    if app.conflicts_pending() {
        return vec![center_line(
            "Override scan queued...",
            Style::default().fg(theme.muted),
        )];
    }
    if app.conflicts.is_empty() {
        return vec![center_line(
            "No Overrides Available",
            Style::default().fg(theme.muted),
        )];
    }

    let total = app.conflicts.len();
    let selected = app.conflict_selected.min(total.saturating_sub(1));
    let conflict = &app.conflicts[selected];

    let header = format!(
        "Overrides — Target: {}   ({} files)   ({}/{})",
        target_kind_label(conflict.target),
        total,
        selected + 1,
        total
    );
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        truncate_text(&header, width),
        Style::default().fg(theme.accent),
    )));

    let info_bg = theme.header_bg;
    let info_line = |text: &str, style: Style| -> Line<'static> {
        let trimmed = truncate_text(text, width);
        let pad = width.saturating_sub(display_width(&trimmed));
        let mut spans = Vec::new();
        spans.push(Span::styled(trimmed, style.bg(info_bg)));
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), Style::default().bg(info_bg)));
        }
        Line::from(spans)
    };

    let mut footer_lines = Vec::new();
    let path_label = conflict.relative_path.to_string_lossy();
    footer_lines.push(info_line(
        &format!("Path: {path_label}"),
        Style::default().fg(theme.muted),
    ));

    let pending_winner = app
        .pending_overrides
        .get(&selected)
        .map(|pending| pending.winner_id.as_str());
    let winner_id = pending_winner.unwrap_or(conflict.winner_id.as_str());
    let winner_name = conflict
        .candidates
        .iter()
        .find(|candidate| candidate.mod_id == winner_id)
        .map(|candidate| candidate.mod_name.clone())
        .unwrap_or_else(|| conflict.winner_name.clone());
    footer_lines.push(info_line(
        &format!("Winner: {winner_name}"),
        Style::default().fg(theme.text),
    ));

    footer_lines.push(info_line(
        "←/→ cycle  1-9 pick  Auto apply 5s  C clear  P pick",
        Style::default().fg(theme.muted),
    ));

    if let Some(status) = conflict_status_label(app) {
        footer_lines.push(info_line(status, Style::default().fg(theme.muted)));
    }

    let mut footer_len = footer_lines.len();
    let mut metrics = conflict_list_metrics(total, selected, height, footer_len);
    while metrics.list_height == 0 && !footer_lines.is_empty() {
        footer_lines.pop();
        footer_len = footer_lines.len();
        metrics = conflict_list_metrics(total, selected, height, footer_len);
    }
    if metrics.list_height == 0 {
        metrics.list_height = 1;
    }

    let max_label_len = 10usize;
    let mut label_counts: HashMap<String, usize> = HashMap::new();
    let short_label = |name: &str| -> String {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return "Mod".to_string();
        }
        truncate_text(trimmed, max_label_len)
    };

    for index in metrics.offset..metrics.offset.saturating_add(metrics.list_height) {
        let Some(entry) = app.conflicts.get(index) else {
            break;
        };
        let pending_winner = app
            .pending_overrides
            .get(&index)
            .map(|pending| pending.winner_id.as_str());
        let winner_id = pending_winner.unwrap_or(entry.winner_id.as_str());

        label_counts.clear();
        let mut labels: Vec<String> = entry
            .candidates
            .iter()
            .map(|candidate| short_label(&candidate.mod_name))
            .collect();
        for label in &labels {
            *label_counts.entry(label.clone()).or_insert(0) += 1;
        }
        if label_counts.values().any(|count| *count > 1) {
            let mut seen: HashMap<String, usize> = HashMap::new();
            for label in labels.iter_mut() {
                let total = label_counts.get(label).copied().unwrap_or(1);
                if total <= 1 {
                    continue;
                }
                let counter = seen.entry(label.clone()).or_insert(0);
                *counter += 1;
                let suffix = counter.to_string();
                let available = max_label_len.saturating_sub(suffix.len());
                let base = truncate_text(label, available);
                *label = format!("{base}{suffix}");
            }
        }

        let selected_row = index == selected;
        let row_bg = if selected_row {
            Some(theme.accent_soft)
        } else {
            None
        };
        let apply_bg = |style: Style| -> Style {
            if let Some(bg) = row_bg {
                style.bg(bg)
            } else {
                style
            }
        };

        let mut right_spans = Vec::new();
        let mut right_width = 0usize;
        let mut shown = 0usize;
        let min_left = 12usize.min(width);
        let max_right_width = width.saturating_sub(min_left).saturating_sub(1);
        for (candidate, label) in entry.candidates.iter().zip(labels.iter()) {
            let marker = if candidate.mod_id == winner_id {
                "x"
            } else {
                " "
            };
            let chunk = format!("[{marker}]{label}");
            let chunk_width = display_width(&chunk);
            let sep = if right_spans.is_empty() { 0 } else { 2 };
            if right_width + sep + chunk_width > max_right_width {
                break;
            }
            if sep > 0 {
                right_spans.push(Span::styled(" ".repeat(sep), apply_bg(Style::default())));
                right_width += sep;
            }
            let style = if candidate.mod_id == winner_id {
                Style::default().fg(theme.success)
            } else {
                Style::default().fg(theme.muted)
            };
            right_spans.push(Span::styled(chunk, apply_bg(style)));
            right_width += chunk_width;
            shown += 1;
        }

        let hidden = entry.candidates.len().saturating_sub(shown);
        if hidden > 0 && right_width < max_right_width {
            let suffix = format!("+{hidden}");
            let suffix_width = display_width(&suffix);
            let sep = if right_spans.is_empty() { 0 } else { 1 };
            if right_width + sep + suffix_width <= max_right_width {
                if sep > 0 {
                    right_spans.push(Span::styled(" ", apply_bg(Style::default())));
                    right_width += 1;
                }
                right_spans.push(Span::styled(
                    suffix,
                    apply_bg(Style::default().fg(theme.muted)),
                ));
                right_width += suffix_width;
            }
        }

        let gap = if right_width == 0 { 0 } else { 1 };
        let left_width = width.saturating_sub(right_width + gap);
        let file_name = entry
            .relative_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.to_string())
            .unwrap_or_else(|| entry.relative_path.to_string_lossy().to_string());
        let left_text = truncate_text(&file_name, left_width);
        let left_pad = left_width.saturating_sub(display_width(&left_text));
        let mut spans = Vec::new();
        spans.push(Span::styled(
            left_text,
            apply_bg(Style::default().fg(theme.text)),
        ));
        if left_pad > 0 {
            spans.push(Span::styled(
                " ".repeat(left_pad),
                apply_bg(Style::default()),
            ));
        }
        if gap > 0 {
            spans.push(Span::styled(" ".repeat(gap), apply_bg(Style::default())));
        }
        spans.extend(right_spans);
        lines.push(Line::from(spans));
    }

    lines.extend(footer_lines);
    lines
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

fn target_root_label(app: &App, kind: TargetKind) -> String {
    let game_root = &app.config.game_root;
    let larian_dir = &app.config.larian_dir;
    if game_root.as_os_str().is_empty() && larian_dir.as_os_str().is_empty() {
        return "<unset>".to_string();
    }

    match kind {
        TargetKind::Pak => "../Mods".to_string(),
        TargetKind::Generated => "../Data/Generated".to_string(),
        TargetKind::Data => "../Data".to_string(),
        TargetKind::Bin => "../bin".to_string(),
    }
}

fn target_kind_path_label(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Pak => "Mods",
        TargetKind::Generated => "Generated",
        TargetKind::Data => "Data",
        TargetKind::Bin => "Bin",
    }
}

fn mod_path_label(
    app: &App,
    mod_entry: &ModEntry,
    theme: &Theme,
    _compact: bool,
) -> (String, Style) {
    if mod_entry.targets.is_empty() {
        return ("Invalid".to_string(), Style::default().fg(theme.warning));
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
            target_kind_path_label(enabled[0]).to_string()
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

    let mut label = base_label;
    if let Some(kind) = kind_for_path {
        let root = target_root_label(app, kind);
        label.push_str(&format!(" [{}]", root));
    } else if kinds.len() > 1 {
        label.push_str(" [Multiple]");
    }

    let style = if has_overrides {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
    };
    (label, style)
}

fn mod_override_label(mod_entry: &ModEntry, theme: &Theme, compact: bool) -> (String, Style) {
    let mut kinds = Vec::new();
    for target in &mod_entry.targets {
        let kind = target.kind();
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    if kinds.is_empty() {
        return ("None".to_string(), Style::default().fg(theme.muted));
    }

    let enabled: Vec<TargetKind> = kinds
        .iter()
        .copied()
        .filter(|kind| mod_entry.is_target_enabled(*kind))
        .collect();

    if mod_entry.target_overrides.is_empty() {
        return (
            format!("Auto [{}]", override_key_label(None)),
            Style::default().fg(theme.muted),
        );
    }

    if enabled.len() == 1 {
        let kind_label = if compact {
            target_kind_short_label(enabled[0])
        } else {
            target_kind_path_label(enabled[0])
        };
        let label = format!("{} [{}]", kind_label, override_key_label(Some(enabled[0])));
        return (
            label,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );
    }

    let (label, color) = if enabled.is_empty() {
        ("None", theme.error)
    } else {
        ("Custom", theme.warning)
    };
    (
        label.to_string(),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}

fn targets_summary(mod_entry: &ModEntry) -> String {
    let mut targets = Vec::new();
    for target in &mod_entry.targets {
        match target {
            InstallTarget::Pak { .. } => targets.push("Pak"),
            InstallTarget::Generated { .. } => targets.push("Generated"),
            InstallTarget::Data { .. } => targets.push("Data"),
            InstallTarget::Bin { .. } => targets.push("Bin"),
        }
    }
    if targets.is_empty() {
        "None".to_string()
    } else {
        targets.join(", ")
    }
}

fn target_kind_label(target: TargetKind) -> &'static str {
    match target {
        TargetKind::Pak => "Pak",
        TargetKind::Generated => "Generated",
        TargetKind::Data => "Data",
        TargetKind::Bin => "Bin",
    }
}

fn target_kind_short_label(target: TargetKind) -> &'static str {
    match target {
        TargetKind::Pak => "Pak",
        TargetKind::Generated => "Gen",
        TargetKind::Data => "Data",
        TargetKind::Bin => "Bin",
    }
}

fn override_key_label(kind: Option<TargetKind>) -> &'static str {
    match kind {
        None => "1",
        Some(TargetKind::Pak) => "2",
        Some(TargetKind::Generated) => "3",
        Some(TargetKind::Data) => "4",
        Some(TargetKind::Bin) => "5",
    }
}

#[derive(Clone)]
struct LegendRow {
    key: String,
    action: String,
}

struct HotkeyRows {
    global: Vec<LegendRow>,
    context: Vec<LegendRow>,
}

struct HelpSection {
    title: &'static str,
    rows: Vec<LegendRow>,
}

struct KvRow {
    label: String,
    value: String,
    label_style: Style,
    value_style: Style,
}

fn legend_rows_for_focus(focus: Focus) -> Vec<LegendRow> {
    let mut legend = Vec::new();

    match focus {
        Focus::Explorer => {
            legend.extend([
                LegendRow {
                    key: "[x]".to_string(),
                    action: "Active Profile".to_string(),
                },
                LegendRow {
                    key: "[ ]".to_string(),
                    action: "Inactive Profile".to_string(),
                },
            ]);
        }
        Focus::Mods => {
            legend.push(LegendRow {
                key: "N".to_string(),
                action: "Native Mod (Mod.io)".to_string(),
            });
            legend.push(LegendRow {
                key: "Dep".to_string(),
                action: "Dependencies Missing/Off".to_string(),
            });
            legend.push(LegendRow {
                key: "⛓".to_string(),
                action: "SigiLink Ranking".to_string(),
            });
            legend.push(LegendRow {
                key: "⛕".to_string(),
                action: "Manual Pin".to_string(),
            });
            legend.push(LegendRow {
                key: "👻".to_string(),
                action: "Missing Mod File".to_string(),
            });
        }
        Focus::Conflicts | Focus::Log => {}
    }

    legend
}

fn legend_rows(app: &App) -> Vec<LegendRow> {
    legend_rows_for_focus(app.hotkey_focus)
}

fn hotkey_rows_for_focus(focus: Focus) -> HotkeyRows {
    let mut context = Vec::new();

    match focus {
        Focus::Explorer => {
            context.extend([
                LegendRow {
                    key: "Enter".to_string(),
                    action: "Select Or Expand".to_string(),
                },
                LegendRow {
                    key: "a".to_string(),
                    action: "New Profile".to_string(),
                },
                LegendRow {
                    key: "r/F2".to_string(),
                    action: "Rename Profile".to_string(),
                },
                LegendRow {
                    key: "c".to_string(),
                    action: "Duplicate Profile".to_string(),
                },
                LegendRow {
                    key: "Del".to_string(),
                    action: "Delete Profile".to_string(),
                },
                LegendRow {
                    key: "e".to_string(),
                    action: "Export Mod List".to_string(),
                },
                LegendRow {
                    key: "p".to_string(),
                    action: "Import Mod List".to_string(),
                },
            ]);
        }
        Focus::Conflicts => {
            context.extend([
                LegendRow {
                    key: "←/→".to_string(),
                    action: "Select Override".to_string(),
                },
                LegendRow {
                    key: "↑/↓".to_string(),
                    action: "Choose Winner".to_string(),
                },
                LegendRow {
                    key: "Enter".to_string(),
                    action: "Cycle Winner".to_string(),
                },
                LegendRow {
                    key: "Backspace".to_string(),
                    action: "Clear Override".to_string(),
                },
            ]);
        }
        Focus::Mods => {
            context.extend([
                LegendRow {
                    key: "Space".to_string(),
                    action: "Toggle Enable".to_string(),
                },
                LegendRow {
                    key: "Shift+↑/↓".to_string(),
                    action: "Jump 10".to_string(),
                },
                LegendRow {
                    key: "m".to_string(),
                    action: "Move Mode".to_string(),
                },
                LegendRow {
                    key: "u/n".to_string(),
                    action: "Move Order".to_string(),
                },
                LegendRow {
                    key: "Enter/Esc".to_string(),
                    action: "Exit Move Mode".to_string(),
                },
                LegendRow {
                    key: "Ctrl+←/→".to_string(),
                    action: "Sort Column".to_string(),
                },
                LegendRow {
                    key: "/ or Ctrl+F".to_string(),
                    action: "Search".to_string(),
                },
                LegendRow {
                    key: "Ctrl+L".to_string(),
                    action: "Clear Search".to_string(),
                },
                LegendRow {
                    key: "PgUp/PgDn".to_string(),
                    action: "Page Scroll".to_string(),
                },
                LegendRow {
                    key: "Del".to_string(),
                    action: "Remove Mod".to_string(),
                },
                LegendRow {
                    key: "Target [1-5]".to_string(),
                    action: "Auto/Mods/Gen/Data/Bin".to_string(),
                },
                LegendRow {
                    key: "A/S/X".to_string(),
                    action: "All On/Off/Invert".to_string(),
                },
                LegendRow {
                    key: "Ctrl+R".to_string(),
                    action: "Reset SigiLink Pin".to_string(),
                },
                LegendRow {
                    key: "F12".to_string(),
                    action: "Reset All SigiLink Pins".to_string(),
                },
            ]);
        }
        Focus::Log => {
            context.extend([
                LegendRow {
                    key: "↑/↓".to_string(),
                    action: "Scroll Log".to_string(),
                },
                LegendRow {
                    key: "PgUp/PgDn".to_string(),
                    action: "Page Scroll".to_string(),
                },
            ]);
        }
    }

    let mut global = Vec::new();
    global.push(LegendRow {
        key: "i".to_string(),
        action: "Import Mod".to_string(),
    });
    global.push(LegendRow {
        key: "Ctrl+E/Ctrl+P".to_string(),
        action: "Export/Import Mod List".to_string(),
    });
    global.push(LegendRow {
        key: "Tab".to_string(),
        action: "Cycle Focus".to_string(),
    });
    global.push(LegendRow {
        key: "?".to_string(),
        action: "Help".to_string(),
    });
    global.push(LegendRow {
        key: "Esc".to_string(),
        action: "Menu".to_string(),
    });

    HotkeyRows { global, context }
}

fn hotkey_rows(app: &App) -> HotkeyRows {
    hotkey_rows_for_focus(app.hotkey_focus)
}

#[allow(dead_code)]
fn legend_line_count(legend: &[LegendRow], hotkeys: &HotkeyRows) -> usize {
    let mut count = 0usize;
    let legend_rows = legend.len().max(5).min(5);
    count = count.saturating_add(1 + legend_rows);
    let hotkey_rows = hotkeys.global.len().saturating_add(hotkeys.context.len());
    if hotkey_rows > 0 {
        count = count.saturating_add(1 + hotkey_rows);
    }
    count
}

fn build_legend_lines(
    legend: &[LegendRow],
    hotkeys: &HotkeyRows,
    theme: &Theme,
    width: usize,
    height: usize,
    key_width: usize,
    hotkey_fade: bool,
) -> Vec<Line<'static>> {
    if width == 0 || height == 0 {
        return Vec::new();
    }

    let mut lines = Vec::new();
    lines.push(context_header_line("Legend", width, theme));
    let mut legend_rows = legend.to_vec();
    if legend_rows.is_empty() {
        legend_rows.push(LegendRow {
            key: "N/A".to_string(),
            action: "None".to_string(),
        });
    }
    while legend_rows.len() < 5 {
        legend_rows.push(LegendRow {
            key: String::new(),
            action: String::new(),
        });
    }
    if legend_rows.len() > 5 {
        legend_rows.truncate(5);
    }
    lines.extend(format_context_rows(
        &legend_rows,
        width,
        key_width,
        theme,
        false,
    ));
    let hotkey_rows = hotkeys.global.len().saturating_add(hotkeys.context.len());
    if hotkey_rows > 0 {
        lines.push(context_header_line("Hotkeys", width, theme));
        lines.extend(format_context_rows(
            &hotkeys.global,
            width,
            key_width,
            theme,
            false,
        ));
        lines.extend(format_context_rows(
            &hotkeys.context,
            width,
            key_width,
            theme,
            hotkey_fade,
        ));
    }
    if lines.len() > height {
        lines.truncate(height);
    }
    lines
}

fn section_header_line(title: &str, width: usize, theme: &Theme) -> Line<'static> {
    let width = width.max(1);
    let title = truncate_text(title, width);
    let padded = format!("{title:<width$}", width = width);
    Line::from(Span::styled(
        padded,
        Style::default()
            .fg(theme.header_bg)
            .bg(theme.accent_soft)
            .add_modifier(Modifier::BOLD),
    ))
}

fn context_header_line(title: &str, width: usize, theme: &Theme) -> Line<'static> {
    let width = width.max(1);
    let title = truncate_text(title, width);
    let padded = format!("{title:<width$}", width = width);
    Line::from(Span::styled(
        padded,
        Style::default()
            .fg(theme.section_bg)
            .bg(theme.header_bg)
            .add_modifier(Modifier::BOLD),
    ))
}

fn format_context_rows(
    rows: &[LegendRow],
    width: usize,
    key_width: usize,
    theme: &Theme,
    dim: bool,
) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let key_width = key_width.min(width);
    let mut key_style = Style::default().fg(theme.accent_soft);
    let mut action_style = Style::default().fg(theme.muted);
    if dim {
        key_style = key_style.add_modifier(Modifier::DIM);
        action_style = action_style.add_modifier(Modifier::DIM);
    }

    rows.iter()
        .map(|row| {
            let spacing = if row.key == "👻" {
                1usize
            } else if row.key == "!" {
                2usize
            } else if row.key == "⛓" || row.key == "⛕" {
                3usize
            } else {
                2usize
            };
            let action_width = width.saturating_sub(key_width + spacing);
            let key_text = truncate_text(&row.key, key_width);
            let key_len = display_width(&key_text);
            let pad = " ".repeat(key_width.saturating_sub(key_len) + spacing);
            let action_text = truncate_text(&row.action, action_width);
            Line::from(vec![
                Span::styled(key_text, key_style),
                Span::raw(pad),
                Span::styled(action_text, action_style),
            ])
        })
        .collect()
}

fn format_legend_rows(
    rows: &[LegendRow],
    width: usize,
    key_width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let key_width = key_width.min(width);
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let spacing = if row.key == "⛓" || row.key == "⛕" {
                3usize
            } else {
                2usize
            };
            let action_width = width.saturating_sub(key_width + spacing);
            let bg = if index % 2 == 1 {
                theme.row_alt_bg
            } else {
                theme.mod_bg
            };
            let key_text = truncate_text(&row.key, key_width);
            let key_len = display_width(&key_text);
            let pad = " ".repeat(key_width.saturating_sub(key_len) + spacing);
            let action_text = truncate_text(&row.action, action_width);
            let action_len = display_width(&action_text);
            let filled = key_len + spacing + action_len;
            let trailing = width.saturating_sub(filled);
            let key_style = if row.key == "!" {
                Style::default()
                    .fg(theme.warning)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(theme.accent)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD)
            };
            Line::from(vec![
                Span::styled(key_text, key_style),
                Span::styled(pad, Style::default().bg(bg)),
                Span::styled(action_text, Style::default().fg(theme.text).bg(bg)),
                Span::styled(" ".repeat(trailing), Style::default().bg(bg)),
            ])
        })
        .collect()
}

fn help_sections() -> Vec<HelpSection> {
    vec![
        HelpSection {
            title: "Global",
            rows: vec![
                LegendRow {
                    key: "Tab".to_string(),
                    action: "Cycle Focus".to_string(),
                },
                LegendRow {
                    key: "?".to_string(),
                    action: "Toggle Help".to_string(),
                },
                LegendRow {
                    key: "Esc".to_string(),
                    action: "Menu".to_string(),
                },
                LegendRow {
                    key: "i".to_string(),
                    action: "Import Mod".to_string(),
                },
                LegendRow {
                    key: "Ctrl+E".to_string(),
                    action: "Export Mod List".to_string(),
                },
                LegendRow {
                    key: "Ctrl+P".to_string(),
                    action: "Import Mod List".to_string(),
                },
                LegendRow {
                    key: "d".to_string(),
                    action: "Deploy".to_string(),
                },
                LegendRow {
                    key: "b".to_string(),
                    action: "Rollback Last Backup".to_string(),
                },
                LegendRow {
                    key: "q".to_string(),
                    action: "Quit".to_string(),
                },
            ],
        },
        HelpSection {
            title: "Explorer",
            rows: vec![
                LegendRow {
                    key: "↑/↓ or j/k".to_string(),
                    action: "Move Selection".to_string(),
                },
                LegendRow {
                    key: "←/→ or h/l".to_string(),
                    action: "Collapse/Expand".to_string(),
                },
                LegendRow {
                    key: "Enter".to_string(),
                    action: "Select/Activate".to_string(),
                },
                LegendRow {
                    key: "a".to_string(),
                    action: "New Profile".to_string(),
                },
                LegendRow {
                    key: "r/F2".to_string(),
                    action: "Rename Profile".to_string(),
                },
                LegendRow {
                    key: "c".to_string(),
                    action: "Duplicate Profile".to_string(),
                },
                LegendRow {
                    key: "e".to_string(),
                    action: "Export Mod List".to_string(),
                },
                LegendRow {
                    key: "p".to_string(),
                    action: "Import Mod List".to_string(),
                },
                LegendRow {
                    key: "Del".to_string(),
                    action: "Delete Profile".to_string(),
                },
            ],
        },
        HelpSection {
            title: "Mods",
            rows: vec![
                LegendRow {
                    key: "↑/↓ or j/k".to_string(),
                    action: "Move Selection".to_string(),
                },
                LegendRow {
                    key: "PgUp/PgDn".to_string(),
                    action: "Page Scroll".to_string(),
                },
                LegendRow {
                    key: "Shift+↑/↓".to_string(),
                    action: "Jump 10".to_string(),
                },
                LegendRow {
                    key: "Space".to_string(),
                    action: "Toggle Enable".to_string(),
                },
                LegendRow {
                    key: "m".to_string(),
                    action: "Move Mode".to_string(),
                },
                LegendRow {
                    key: "u/n".to_string(),
                    action: "Move Order".to_string(),
                },
                LegendRow {
                    key: "Enter/Esc".to_string(),
                    action: "Exit Move Mode".to_string(),
                },
                LegendRow {
                    key: "1-5".to_string(),
                    action: "Target Override (Auto/Mods/Gen/Data/Bin)".to_string(),
                },
                LegendRow {
                    key: "A/S/X".to_string(),
                    action: "Enable/Disable/Invert Visible".to_string(),
                },
                LegendRow {
                    key: "c".to_string(),
                    action: "Clear Overrides".to_string(),
                },
                LegendRow {
                    key: "/ or Ctrl+F".to_string(),
                    action: "Search Mods".to_string(),
                },
                LegendRow {
                    key: "Ctrl+←/→".to_string(),
                    action: "Sort Column".to_string(),
                },
                LegendRow {
                    key: "Ctrl+↑/↓".to_string(),
                    action: "Invert Sort".to_string(),
                },
                LegendRow {
                    key: "Ctrl+L".to_string(),
                    action: "Clear Search".to_string(),
                },
                LegendRow {
                    key: "Del".to_string(),
                    action: "Remove Mod".to_string(),
                },
            ],
        },
        HelpSection {
            title: "Conflicts",
            rows: vec![
                LegendRow {
                    key: "←/→".to_string(),
                    action: "Select Override".to_string(),
                },
                LegendRow {
                    key: "↑/↓".to_string(),
                    action: "Choose Winner".to_string(),
                },
                LegendRow {
                    key: "Enter".to_string(),
                    action: "Cycle Winner".to_string(),
                },
                LegendRow {
                    key: "Backspace/Del".to_string(),
                    action: "Clear Override".to_string(),
                },
            ],
        },
        HelpSection {
            title: "Log",
            rows: vec![
                LegendRow {
                    key: "↑/↓".to_string(),
                    action: "Scroll Log".to_string(),
                },
                LegendRow {
                    key: "PgUp/PgDn".to_string(),
                    action: "Page Scroll".to_string(),
                },
            ],
        },
        HelpSection {
            title: "SigiLink Intelligent Ranking Preview",
            rows: vec![
                LegendRow {
                    key: "Enter/Y".to_string(),
                    action: "Apply Ranking".to_string(),
                },
                LegendRow {
                    key: "Esc/N".to_string(),
                    action: "Cancel".to_string(),
                },
                LegendRow {
                    key: "Tab".to_string(),
                    action: "Toggle View".to_string(),
                },
                LegendRow {
                    key: "↑/↓".to_string(),
                    action: "Scroll".to_string(),
                },
                LegendRow {
                    key: "PgUp/PgDn".to_string(),
                    action: "Page Scroll".to_string(),
                },
                LegendRow {
                    key: "Home/End".to_string(),
                    action: "Top/Bottom".to_string(),
                },
            ],
        },
        HelpSection {
            title: "SigiLink Intelligent Ranking",
            rows: vec![
                LegendRow {
                    key: "What".to_string(),
                    action: "Scans Enabled Pak/Loose Files To Find Conflicts.".to_string(),
                },
                LegendRow {
                    key: "Order".to_string(),
                    action: "Respects Dependencies, Then Patch Tags/Size/Date.".to_string(),
                },
                LegendRow {
                    key: "Source".to_string(),
                    action: "Uses Current Profile Order As The Baseline.".to_string(),
                },
            ],
        },
        HelpSection {
            title: "SigiLink Manual Pins",
            rows: vec![
                LegendRow {
                    key: "Move Mod".to_string(),
                    action: "Creates A Manual Pin (⛕) While Auto Ranking Is ON.".to_string(),
                },
                LegendRow {
                    key: "Ctrl+R".to_string(),
                    action: "Reset SigiLink Pin For Selected Mod.".to_string(),
                },
                LegendRow {
                    key: "F12".to_string(),
                    action: "Reset All SigiLink Pins (Confirm).".to_string(),
                },
                LegendRow {
                    key: "⛓/⛕".to_string(),
                    action: "Auto-Managed vs Manual Pin In The Link Column.".to_string(),
                },
            ],
        },
        HelpSection {
            title: "Dialogs",
            rows: vec![
                LegendRow {
                    key: "←/→ or Tab".to_string(),
                    action: "Change Choice".to_string(),
                },
                LegendRow {
                    key: "Y/N".to_string(),
                    action: "Pick Choice".to_string(),
                },
                LegendRow {
                    key: "D".to_string(),
                    action: "Toggle Checkbox".to_string(),
                },
                LegendRow {
                    key: "Enter/Space".to_string(),
                    action: "Confirm".to_string(),
                },
                LegendRow {
                    key: "Esc".to_string(),
                    action: "Cancel".to_string(),
                },
            ],
        },
        HelpSection {
            title: "Dependencies",
            rows: vec![
                LegendRow {
                    key: "↑/↓".to_string(),
                    action: "Move Selection".to_string(),
                },
                LegendRow {
                    key: "Enter/Space".to_string(),
                    action: "Open Link/Search Or Override".to_string(),
                },
                LegendRow {
                    key: "Ctrl+C".to_string(),
                    action: "Copy Dependency Link".to_string(),
                },
                LegendRow {
                    key: "C".to_string(),
                    action: "Copy UUID".to_string(),
                },
                LegendRow {
                    key: "Esc".to_string(),
                    action: "Cancel Import".to_string(),
                },
            ],
        },
        HelpSection {
            title: "Path Browser",
            rows: vec![
                LegendRow {
                    key: "Tab".to_string(),
                    action: "Switch Focus".to_string(),
                },
                LegendRow {
                    key: "Enter/Space".to_string(),
                    action: "Open/Select".to_string(),
                },
                LegendRow {
                    key: "↑/↓ or j/k".to_string(),
                    action: "Move Selection".to_string(),
                },
                LegendRow {
                    key: "PgUp/PgDn".to_string(),
                    action: "Page".to_string(),
                },
                LegendRow {
                    key: "Home/End".to_string(),
                    action: "Top/Bottom".to_string(),
                },
                LegendRow {
                    key: "←/Backspace/Home".to_string(),
                    action: "Parent Folder".to_string(),
                },
                LegendRow {
                    key: "Ctrl+C".to_string(),
                    action: "Copy Path".to_string(),
                },
                LegendRow {
                    key: "Ctrl+U".to_string(),
                    action: "Clear Path Input".to_string(),
                },
                LegendRow {
                    key: "Ctrl+V".to_string(),
                    action: "Paste Path".to_string(),
                },
                LegendRow {
                    key: "Esc".to_string(),
                    action: "Cancel".to_string(),
                },
            ],
        },
        HelpSection {
            title: "Input Fields",
            rows: vec![
                LegendRow {
                    key: "Enter".to_string(),
                    action: "Submit".to_string(),
                },
                LegendRow {
                    key: "Esc".to_string(),
                    action: "Cancel".to_string(),
                },
                LegendRow {
                    key: "Ctrl+Alt+V".to_string(),
                    action: "Paste".to_string(),
                },
            ],
        },
    ]
}

fn help_key_width(sections: &[HelpSection], width: usize) -> usize {
    sections
        .iter()
        .flat_map(|section| section.rows.iter())
        .map(|row| display_width(&row.key))
        .max()
        .unwrap_or(0)
        .min(width)
}

fn build_help_lines(theme: &Theme, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let sections = help_sections();
    let key_width = help_key_width(&sections, width);

    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        "Esc/? Close | ↑/↓ PgUp/PgDn Scroll | Home/End Jump",
        Style::default().fg(theme.muted),
    )));
    lines.push(Line::from(""));

    for section in sections {
        lines.push(section_header_line(section.title, width, theme));
        lines.extend(format_legend_rows(&section.rows, width, key_width, theme));
        lines.push(Line::from(""));
    }

    while matches!(lines.last(), Some(line) if line.to_string().is_empty()) {
        lines.pop();
    }

    lines
}

fn build_whats_new_lines(theme: &Theme, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let header_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let body_style = Style::default().fg(theme.text);
    let muted_style = Style::default().fg(theme.muted);

    let banner_width = 108usize;
    let banner = [
        "      .-====================-.",
        "   .-'  *  o  *  o  *  o  *  '-.",
        "  /  *   .-''-.  /\\  .-''-.   * \\",
        " |  o   /  /\\  \\ || /  /\\  \\   o |",
        "  \\ *  \\  \\/  / || \\  \\/  /  * /",
        "   '-.  '----'  ||  '----'  .-' v0.9.6",
    ];
    for line in banner {
        let padded = format!("{line:<banner_width$}");
        lines.push(Line::from(Span::styled(
            truncate_text(&padded, width),
            header_style,
        )));
    }
    lines.push(Line::from(""));

    fn push_section(lines: &mut Vec<Line<'static>>, title: &str, width: usize, theme: &Theme) {
        lines.push(section_header_line(title, width, theme));
    }

    fn push_bullet(lines: &mut Vec<Line<'static>>, width: usize, text: &str, style: Style) {
        let mut current = String::from("- ");
        for word in text.split_whitespace() {
            let add_len = if current.ends_with(' ') {
                word.chars().count()
            } else {
                1 + word.chars().count()
            };
            if current.chars().count() + add_len > width {
                lines.push(Line::from(Span::styled(
                    truncate_text(&current, width),
                    style,
                )));
                current = format!("  {word}");
            } else {
                if !current.ends_with(' ') {
                    current.push(' ');
                }
                current.push_str(word);
            }
        }
        lines.push(Line::from(Span::styled(
            truncate_text(&current, width),
            style,
        )));
    }

    push_section(&mut lines, "Patch 8 Compatibility", width, theme);
    push_bullet(
        &mut lines,
        width,
        "Load order now follows the Mods list (Patch 7/8 behavior).",
        body_style,
    );
    push_bullet(
        &mut lines,
        width,
        "Enabled mods only are written to modsettings.lsx (Mods list).",
        body_style,
    );
    push_bullet(
        &mut lines,
        width,
        "ModOrder stays in sync for BG3MM/Vortex interop.",
        body_style,
    );
    lines.push(Line::from(""));

    push_section(&mut lines, "Sync + Safety", width, theme);
    push_bullet(
        &mut lines,
        width,
        "Skip native sync if modsettings list is empty to avoid mass disable.",
        body_style,
    );
    push_bullet(
        &mut lines,
        width,
        "Startup warning when the Larian data dir is a symlink.",
        body_style,
    );
    push_bullet(
        &mut lines,
        width,
        "Duplicate-name guard: prompt to disable other enabled copies.",
        body_style,
    );
    lines.push(Line::from(""));

    push_section(&mut lines, "Debug + Transparency", width, theme);
    push_bullet(
        &mut lines,
        width,
        "Debug cache now shows modsettings version and ModOrder presence.",
        body_style,
    );
    push_bullet(
        &mut lines,
        width,
        "Logs include native duplicate auto-disable events.",
        body_style,
    );
    push_bullet(
        &mut lines,
        width,
        "Safer native mod sync for Patch 8 mod manager.",
        body_style,
    );
    lines.push(Line::from(""));

    push_section(&mut lines, "Overrides + UX", width, theme);
    push_bullet(
        &mut lines,
        width,
        "Overrides panel redesigned for fast left/right selection.",
        body_style,
    );
    push_bullet(
        &mut lines,
        width,
        "Dependency prompts for safe enable/disable cascades.",
        body_style,
    );
    push_bullet(
        &mut lines,
        width,
        "Cleaner layouts, richer help, and export/import workflows.",
        body_style,
    );
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled(
        "Tip: Use JSON for full fidelity. modsettings.lsx cannot store disabled state.",
        muted_style,
    )));
    lines.push(Line::from(Span::styled(
        "You can reopen this panel in Esc → What's New?.",
        muted_style,
    )));

    while matches!(lines.last(), Some(line) if line.to_string().is_empty()) {
        lines.pop();
    }

    lines
}

fn format_kv_lines(rows: &[KvRow], width: usize) -> Vec<Line<'static>> {
    if width == 0 || rows.is_empty() {
        return Vec::new();
    }

    let max_label = rows
        .iter()
        .map(|row| row.label.chars().count())
        .max()
        .unwrap_or(0);
    if width <= 2 {
        return rows
            .iter()
            .map(|row| {
                Line::from(Span::styled(
                    truncate_text(&row.label, width),
                    row.label_style,
                ))
            })
            .collect();
    }

    let label_width = max_label.min(width.saturating_sub(2));
    let value_width = width.saturating_sub(label_width + 2);

    rows.iter()
        .map(|row| {
            if value_width == 0 {
                return Line::from(Span::styled(
                    truncate_text(&row.label, width),
                    row.label_style,
                ));
            }
            let label_text = truncate_text(&row.label, label_width);
            let label_len = label_text.chars().count();
            let pad = " ".repeat(label_width.saturating_sub(label_len));
            let value_text = truncate_text(&row.value, value_width);
            if row.label.trim().is_empty() {
                let indent = " ".repeat(label_width + 2);
                return Line::from(vec![
                    Span::raw(indent),
                    Span::styled(value_text, row.value_style),
                ]);
            }
            Line::from(vec![
                Span::styled(label_text, row.label_style),
                Span::raw(pad),
                Span::styled(": ", row.label_style),
                Span::styled(value_text, row.value_style),
            ])
        })
        .collect()
}

fn format_kv_line_aligned(row: &KvRow, width: usize, label_width: usize) -> Line<'static> {
    if width == 0 {
        return Line::from("");
    }
    if width <= 2 {
        return Line::from(Span::styled(
            truncate_text(&row.label, width),
            row.label_style,
        ));
    }
    let label_width = label_width.min(width.saturating_sub(2));
    let value_width = width.saturating_sub(label_width + 2);
    if value_width == 0 {
        return Line::from(Span::styled(
            truncate_text(&row.label, width),
            row.label_style,
        ));
    }
    let label_text = truncate_text(&row.label, label_width);
    let label_len = label_text.chars().count();
    let pad = " ".repeat(label_width.saturating_sub(label_len));
    let value_text = truncate_text(&row.value, value_width);
    if row.label.trim().is_empty() {
        let indent = " ".repeat(label_width + 2);
        return Line::from(vec![
            Span::raw(indent),
            Span::styled(value_text, row.value_style),
        ]);
    }
    Line::from(vec![
        Span::styled(label_text, row.label_style),
        Span::raw(pad),
        Span::styled(": ", row.label_style),
        Span::styled(value_text, row.value_style),
    ])
}

fn format_kv_line_aligned_spans(
    label: &str,
    label_style: Style,
    value_parts: Vec<(String, Style)>,
    width: usize,
    label_width: usize,
) -> Line<'static> {
    if width == 0 {
        return Line::from("");
    }
    if width <= 2 {
        return Line::from(Span::styled(truncate_text(label, width), label_style));
    }
    let label_width = label_width.min(width.saturating_sub(2));
    let value_width = width.saturating_sub(label_width + 2);
    if value_width == 0 {
        return Line::from(Span::styled(truncate_text(label, width), label_style));
    }
    let label_text = truncate_text(label, label_width);
    let label_len = label_text.chars().count();
    let pad = " ".repeat(label_width.saturating_sub(label_len));
    let value_spans = truncate_spans(value_parts, value_width);
    let mut spans = vec![
        Span::styled(label_text, label_style),
        Span::raw(pad),
        Span::styled(": ", label_style),
    ];
    spans.extend(value_spans);
    Line::from(spans)
}

fn format_kv_line_split(
    label: &str,
    label_style: Style,
    left_text: &str,
    left_style: Style,
    right_text: &str,
    right_spans: Vec<Span<'static>>,
    width: usize,
    label_width: usize,
    fallback_style: Style,
) -> Line<'static> {
    if width == 0 {
        return Line::from("");
    }
    if width <= 2 {
        return Line::from(Span::styled(truncate_text(label, width), label_style));
    }
    let label_width = label_width.min(width.saturating_sub(2));
    let value_width = width.saturating_sub(label_width + 2);
    if value_width == 0 {
        return Line::from(Span::styled(truncate_text(label, width), label_style));
    }
    let label_text = truncate_text(label, label_width);
    let label_len = label_text.chars().count();
    let pad = " ".repeat(label_width.saturating_sub(label_len));
    let right_len = right_text.chars().count();
    if right_len >= value_width {
        let truncated = truncate_text(right_text, value_width);
        return Line::from(vec![
            Span::styled(label_text, label_style),
            Span::raw(pad),
            Span::styled(": ", label_style),
            Span::styled(truncated, fallback_style),
        ]);
    }
    let mut left_available = value_width.saturating_sub(right_len);
    let mut gap = 0usize;
    if !left_text.is_empty() && right_len > 0 {
        gap = 1;
        left_available = left_available.saturating_sub(1);
    }
    let left_trunc = truncate_text(left_text, left_available);
    let left_len = left_trunc.chars().count();
    let pad_len = value_width.saturating_sub(left_len + gap + right_len);
    let spacer = " ".repeat(pad_len + gap);
    let mut spans = vec![
        Span::styled(label_text, label_style),
        Span::raw(pad),
        Span::styled(": ", label_style),
    ];
    if !left_trunc.is_empty() {
        spans.push(Span::styled(left_trunc, left_style));
    }
    spans.push(Span::raw(spacer));
    spans.extend(right_spans);
    Line::from(spans)
}
