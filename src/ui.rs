use crate::{
    app::{
        expand_tilde, App, DialogChoice, DialogKind, ExplorerItem, ExplorerItemKind, Focus,
        InputMode, InputPurpose, LogLevel, ToastLevel, PathBrowser, PathBrowserEntryKind,
        PathBrowserFocus, SetupStep,
    },
    library::{InstallTarget, ModEntry, TargetKind},
};
use anyhow::Result;
use crossterm::{
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Cell, Clear, List, ListItem, ListState, Padding, Paragraph,
        Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table, TableState,
    },
};
use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const SIDE_PANEL_WIDTH: u16 = 40;
const STATUS_WIDTH: u16 = SIDE_PANEL_WIDTH;
const STATUS_HEIGHT: u16 = 3;
const HEADER_HEIGHT: u16 = 3;
const DETAILS_HEIGHT: u16 = 10;
const CONTEXT_HEIGHT: u16 = 19;
const LOG_MIN_HEIGHT: u16 = 5;
const CONFLICTS_BAR_HEIGHT: u16 = STATUS_HEIGHT;
const FILTER_HEIGHT: u16 = 1;
const TABLE_MIN_HEIGHT: u16 = 6;
const SUBPANEL_PAD_X: u16 = 0;
const SUBPANEL_PAD_TOP: u16 = 0;

#[derive(Clone)]
struct Theme {
    accent: Color,
    accent_soft: Color,
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
}

impl Theme {
    fn new() -> Self {
        Self {
            accent: Color::Rgb(120, 198, 255),
            accent_soft: Color::Rgb(58, 92, 138),
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
        Block::default()
            .borders(Borders::NONE)
            .title(Span::styled(
                title,
                Style::default()
                    .fg(self.accent)
                    .add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(self.subpanel_bg))
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
    execute!(terminal.backend_mut(), DisableBracketedPaste, LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_loop(terminal: &mut Terminal<impl Backend>, app: &mut App) -> Result<()> {
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
        app.poll_smart_rank();
        app.clamp_selection();
        terminal.draw(|frame| draw(frame, app))?;

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
    if app.smart_rank_preview.is_some() {
        return handle_smart_rank_preview(app, key);
    }
    if app.dialog.is_some() {
        return handle_dialog_mode(app, key);
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
        } => {
            handle_input_mode(
                app,
                key,
                &mut buffer,
                purpose.clone(),
                prompt,
                auto_submit,
                &mut last_edit_at,
            )
        }
    }
}

fn handle_dialog_mode(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('H') => {
            app.dialog_choice_left();
        }
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('L') | KeyCode::Tab => {
            app.dialog_choice_right();
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
        KeyCode::Enter | KeyCode::Char(' ') => {
            app.dialog_confirm();
        }
        KeyCode::Esc => {
            app.dialog_set_choice(DialogChoice::No);
            app.dialog_confirm();
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

#[derive(Debug, Clone, Copy)]
enum SettingsItemKind {
    ActionSetupPaths,
    ToggleProfileDelete,
    ToggleModDelete,
    ActionSmartRank,
}

#[derive(Debug, Clone)]
struct SettingsItem {
    label: &'static str,
    kind: SettingsItemKind,
    checked: Option<bool>,
}

fn settings_items(app: &App) -> Vec<SettingsItem> {
    vec![
        SettingsItem {
            label: "Confirm profile delete",
            kind: SettingsItemKind::ToggleProfileDelete,
            checked: Some(app.app_config.confirm_profile_delete),
        },
        SettingsItem {
            label: "Confirm mod delete",
            kind: SettingsItemKind::ToggleModDelete,
            checked: Some(app.app_config.confirm_mod_delete),
        },
        SettingsItem {
            label: "Configure game paths",
            kind: SettingsItemKind::ActionSetupPaths,
            checked: None,
        },
        SettingsItem {
            label: "AI Smart Ranking",
            kind: SettingsItemKind::ActionSmartRank,
            checked: None,
        },
    ]
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
            menu.selected = menu.selected.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => {
            menu.selected = (menu.selected + 1).min(items_len.saturating_sub(1));
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            if let Some(item) = items.get(menu.selected) {
                match item.kind {
                    SettingsItemKind::ActionSetupPaths => {
                        app.close_settings_menu();
                        app.enter_setup_game_root();
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
                    SettingsItemKind::ActionSmartRank => {
                        app.close_settings_menu();
                        app.open_smart_rank_preview();
                    }
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
        (KeyCode::PageUp, _) => app.scroll_log_up(3),
        (KeyCode::PageDown, _) => app.scroll_log_down(3),
        (KeyCode::Tab, _) => app.cycle_focus(),
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
        KeyCode::Enter => {
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

fn handle_mods_mode(app: &mut App, key: KeyEvent) -> Result<()> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('f'), mods) | (KeyCode::Char('F'), mods)
            if mods.contains(KeyModifiers::CONTROL) =>
        {
            app.enter_mod_filter();
        }
        (KeyCode::Char('l'), mods) | (KeyCode::Char('L'), mods)
            if mods.contains(KeyModifiers::CONTROL) =>
        {
            app.clear_mod_filter();
        }
        (KeyCode::Char('m'), _) | (KeyCode::Char('M'), _) => app.toggle_move_mode(),
        (KeyCode::Enter, _) | (KeyCode::Esc, _) if app.move_mode => app.toggle_move_mode(),
        (KeyCode::Char(' '), _) => app.toggle_selected(),
        (KeyCode::Char('a'), _) | (KeyCode::Char('A'), _) => app.enable_visible_mods(),
        (KeyCode::Char('s'), _) | (KeyCode::Char('S'), _) => app.disable_visible_mods(),
        (KeyCode::Char('x'), _) | (KeyCode::Char('X'), _) => app.invert_visible_mods(),
        (KeyCode::Char('c'), _) | (KeyCode::Char('C'), _) => app.clear_visible_overrides(),
        (KeyCode::Delete, _) | (KeyCode::Backspace, _) => app.request_remove_selected(),
        (KeyCode::Char('k'), _) | (KeyCode::Char('K'), _) | (KeyCode::Up, _) => {
            if app.move_mode {
                app.move_selected_up();
            } else if app.selected > 0 {
                app.selected -= 1;
            }
        }
        (KeyCode::Char('j'), _) | (KeyCode::Char('J'), _) | (KeyCode::Down, _) => {
            if app.move_mode {
                app.move_selected_down();
            } else {
                app.selected += 1
            }
        }
        (KeyCode::Char('u'), _) | (KeyCode::Char('U'), _) => app.move_selected_up(),
        (KeyCode::Char('n'), _) | (KeyCode::Char('N'), _) => app.move_selected_down(),
        (KeyCode::Char('1'), _) => app.select_target_override(None),
        (KeyCode::Char('2'), _) => app.select_target_override(Some(TargetKind::Pak)),
        (KeyCode::Char('3'), _) => app.select_target_override(Some(TargetKind::Generated)),
        (KeyCode::Char('4'), _) => app.select_target_override(Some(TargetKind::Data)),
        (KeyCode::Char('5'), _) => app.select_target_override(Some(TargetKind::Bin)),
        (KeyCode::Char(c), mods)
            if !mods.contains(KeyModifiers::CONTROL) && !mods.contains(KeyModifiers::ALT) =>
        {
            if should_start_import(c) {
                app.enter_import_with(c.to_string());
            }
        }
        _ => {}
    }

    Ok(())
}

fn handle_conflicts_mode(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('H') => app.conflict_move_up(),
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('L') => app.conflict_move_down(),
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => app.cycle_conflict_winner(-1),
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') | KeyCode::Enter => {
            app.cycle_conflict_winner(1)
        }
        KeyCode::Backspace | KeyCode::Delete => app.clear_conflict_override(),
        _ => {}
    }

    Ok(())
}

fn handle_log_mode(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => app.scroll_log_up(1),
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => app.scroll_log_down(1),
        _ => {}
    }
    Ok(())
}

fn handle_browser_mode(
    app: &mut App,
    key: KeyEvent,
    browser: &mut PathBrowser,
) -> Result<bool> {
    let invalid_hint = match browser.step {
        SetupStep::GameRoot => "Not a BG3 install root (needs Data/ + bin/).",
        SetupStep::LarianDir => "Not a Larian data dir (needs PlayerProfiles/).",
    };
    let len = browser.entries.len();
    match browser.focus {
        PathBrowserFocus::PathInput => match key.code {
            KeyCode::Esc => {
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
                } else {
                    app.status = format!("Path not found: {}", browser.path_input.trim());
                }
            }
            KeyCode::Backspace | KeyCode::Delete => {
                browser.path_input.pop();
            }
            KeyCode::Char('u') | KeyCode::Char('U')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                browser.path_input.clear();
            }
            KeyCode::Char(c) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                {
                    browser.path_input.push(c);
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
            KeyCode::Home => {
                browser.selected = 0;
            }
            KeyCode::End => {
                if len > 0 {
                    browser.selected = len.saturating_sub(1);
                }
            }
            KeyCode::Tab => {
                browser.focus = PathBrowserFocus::PathInput;
                browser.path_input = browser.current.display().to_string();
            }
            KeyCode::Left | KeyCode::Backspace => {
                if let Some(parent) = browser.current.parent() {
                    path_browser_set_current(app, browser, parent.to_path_buf());
                }
            }
        KeyCode::Char('s') | KeyCode::Char('S') => {
            if let Some(select) = browser
                .entries
                .iter()
                .find(|entry| entry.kind == PathBrowserEntryKind::Select)
            {
                if select.selectable {
                    app.apply_path_browser_selection(browser.step, select.path.clone())?;
                    return Ok(true);
                }
                app.status = invalid_hint.to_string();
                app.set_toast(invalid_hint, ToastLevel::Warn, Duration::from_secs(2));
            }
        }
        KeyCode::Enter => {
            if let Some(entry) = browser.entries.get(browser.selected) {
                match entry.kind {
                    PathBrowserEntryKind::Select => {
                        if entry.selectable {
                            app.apply_path_browser_selection(browser.step, entry.path.clone())?;
                            return Ok(true);
                        }
                        app.status = invalid_hint.to_string();
                        app.set_toast(invalid_hint, ToastLevel::Warn, Duration::from_secs(2));
                    }
                    PathBrowserEntryKind::Parent | PathBrowserEntryKind::Dir => {
                        path_browser_set_current(app, browser, entry.path.clone());
                    }
                }
                }
            }
            KeyCode::Esc => {
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

fn path_browser_set_current(app: &App, browser: &mut PathBrowser, path: PathBuf) {
    browser.current = path.clone();
    browser.entries = app.build_path_browser_entries(browser.step, &browser.current);
    browser.selected = 0;
    browser.path_input = path.display().to_string();
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
                InputPurpose::ExportProfile { profile } => format!("Export cancelled: {profile}"),
                InputPurpose::ImportProfile | InputPurpose::ImportPath => {
                    "Import cancelled".to_string()
                }
                InputPurpose::FilterMods => "Filter cancelled".to_string(),
            };
            app.set_toast(&cancel_message, ToastLevel::Warn, Duration::from_secs(2));
            if matches!(purpose, InputPurpose::FilterMods) {
                app.status = "Filter cancelled".to_string();
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
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL)
                || key.modifiers.contains(KeyModifiers::ALT)
            {
                return Ok(());
            }
            buffer.push(c);
            *last_edit_at = std::time::Instant::now();
        }
        KeyCode::Backspace => {
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

fn should_start_import(c: char) -> bool {
    matches!(c, '/' | '~' | '.' | 'f' | 'F' | '"' | '\'')
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
    let lower_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[2]);

    let (rows, counts, target_width) = build_rows(app, &theme);
    let profile_label = app.active_profile_label();
    let renaming_active = app.is_renaming_active_profile();
    let filter_active = app.mod_filter_active();
    let mods_label = format_visible_count(counts.visible_total, counts.total);
    let enabled_label = format_enabled_count(
        counts.visible_enabled,
        counts.visible_total,
        counts.enabled,
        filter_active,
    );
    let status_color = status_color(app, &theme);
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

    let title_text = format!("SigilSmith | {}", app.game_id.display_name());
    let stats_text = format!(
        "Profile: {} | Mods {} | Enabled {}",
        profile_label, mods_label, enabled_label
    );
    let available = header_line_area.width as usize;
    let mut left_width = title_text.chars().count().min(available);
    let mut right_width = stats_text.chars().count().min(available.saturating_sub(left_width));
    let min_middle = 1usize;
    if left_width + right_width + min_middle > available {
        let overflow = left_width + right_width + min_middle - available;
        if right_width >= left_width {
            right_width = right_width.saturating_sub(overflow);
        } else {
            left_width = left_width.saturating_sub(overflow);
        }
    }
    let middle_width = available.saturating_sub(left_width + right_width);
    let header_line_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(left_width as u16),
            Constraint::Length(middle_width as u16),
            Constraint::Length(right_width as u16),
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
        let game_width = max_title.saturating_sub(title_prefix.len() + title_bar.len());
        let game_name = truncate_text(app.game_id.display_name(), game_width);
        Line::from(vec![
            Span::styled(
                title_prefix,
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(title_bar, Style::default().fg(theme.muted)),
            Span::styled(game_name, Style::default().fg(theme.text)),
        ])
    };
    let title = Paragraph::new(title_line)
        .style(Style::default().bg(theme.header_bg))
        .alignment(Alignment::Left);
    frame.render_widget(title, header_line_chunks[0]);

    if middle_width > 0 {
        let tabs_line = build_focus_tabs_line(app, &theme);
        let tabs = Paragraph::new(tabs_line)
            .style(Style::default().bg(theme.header_bg))
            .alignment(Alignment::Center);
        frame.render_widget(tabs, header_line_chunks[1]);
    }

    let stats_line = Line::from(vec![
        Span::styled("Profile: ", Style::default().fg(theme.muted)),
        Span::styled(
            profile_label,
            Style::default().fg(if renaming_active {
                theme.warning
            } else {
                theme.accent
            }),
        ),
        Span::styled(" | ", Style::default().fg(theme.muted)),
        Span::styled("Mods ", Style::default().fg(theme.muted)),
        Span::styled(mods_label.clone(), Style::default().fg(theme.text)),
        Span::styled(" | ", Style::default().fg(theme.muted)),
        Span::styled("Enabled ", Style::default().fg(theme.muted)),
        Span::styled(
            enabled_label.clone(),
            Style::default().fg(theme.success).add_modifier(Modifier::BOLD),
        ),
    ]);
    let stats = Paragraph::new(stats_line)
        .style(Style::default().bg(theme.header_bg))
        .alignment(Alignment::Right);
    frame.render_widget(stats, header_line_chunks[2]);

    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(SIDE_PANEL_WIDTH), Constraint::Min(20)])
        .split(chunks[1]);
    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(CONTEXT_HEIGHT)])
        .split(body_chunks[0]);

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
        frame.render_widget(empty, left_chunks[0]);
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
        frame.render_stateful_widget(explorer, left_chunks[0], &mut state);
    }

    let mod_stack_block = theme
        .block("Mod Stack")
        .border_style(Style::default().fg(if app.focus == Focus::Mods {
            theme.accent
        } else {
            theme.border
        }))
        .style(Style::default().bg(theme.mod_bg));
    let mod_stack_inner = mod_stack_block.inner(body_chunks[1]);
    frame.render_widget(mod_stack_block, body_chunks[1]);

    let mod_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(FILTER_HEIGHT), Constraint::Min(TABLE_MIN_HEIGHT)])
        .split(mod_stack_inner);

    render_filter_bar(frame, app, &theme, mod_chunks[0], &counts);

    let table_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(10), Constraint::Length(1)])
        .split(mod_chunks[1]);

    let row_count = rows.len();
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
        let spacing = 1u16;
        let min_mod = 16u16;
        let date_width = 10u16;
        let fixed_without_target =
            3 + 2 + 5 + 6 + date_width + date_width + min_mod + spacing * 7;
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
        let table = Table::new(
            rows,
            [
                Constraint::Length(3),
                Constraint::Length(2),
                Constraint::Length(5),
                Constraint::Length(6),
                Constraint::Length(target_col),
                Constraint::Length(date_width),
                Constraint::Length(date_width),
                Constraint::Min(min_mod),
            ],
        )
        .style(Style::default().bg(theme.mod_bg).fg(theme.text))
        .header(Row::new(vec![
            Cell::from("On"),
            Cell::from("N"),
            Cell::from("Order"),
            Cell::from("Kind"),
            Cell::from("Target"),
            Cell::from("Created"),
            Cell::from("Added"),
            Cell::from("Mod"),
        ])
        .style(
            Style::default()
                .fg(theme.accent)
                .bg(theme.header_bg)
                .add_modifier(Modifier::BOLD),
        ))
        .column_spacing(1)
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
        let view_height = table_chunks[0].height.saturating_sub(1) as usize;
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
            width: table_chunks[0]
                .width
                .saturating_add(table_chunks[1].width),
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
        "Override Actions"
    } else {
        "Details"
    };
    let details_border = if details_focus {
        theme.accent
    } else {
        theme.border
    };
    let swap_info = app.override_swap_info();
    let details_bg = if swap_info.is_some() {
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
    let details_content_width =
        details_inner.width.saturating_sub(SUBPANEL_PAD_X.saturating_mul(2)) as usize;
    let details_lines = build_details(app, &theme, details_content_width);
    let details_lines = pad_lines(
        details_lines,
        SUBPANEL_PAD_X as usize,
        SUBPANEL_PAD_TOP as usize,
    );
    let details = Paragraph::new(details_lines)
        .style(Style::default().fg(theme.text).bg(details_bg))
        .block(details_block);
    frame.render_widget(details, details_area);
    if let Some(swap) = swap_info {
        if details_inner.height > 0 && details_inner.width > 0 {
            let overlay_height = details_inner.height.min(2);
            let overlay_area = Rect {
                x: details_inner.x,
                y: details_inner.y + details_inner.height.saturating_sub(overlay_height),
                width: details_inner.width,
                height: overlay_height,
            };
            let swap_text = format!("Swap: {} → {}", swap.from, swap.to);
            let mut overlay_lines = Vec::new();
            let overlay_width = overlay_area.width as usize;
            if overlay_height >= 1 {
                overlay_lines.push(Line::from(Span::styled(
                    truncate_text(&swap_text, overlay_width),
                    Style::default().fg(theme.header_bg),
                )));
            }
            if overlay_height >= 2 {
                overlay_lines.push(Line::from(Span::styled(
                    "Loading swap...",
                    Style::default()
                        .fg(theme.header_bg)
                        .add_modifier(Modifier::BOLD),
                )));
            }
            let overlay = Paragraph::new(overlay_lines)
                .style(Style::default().bg(details_bg))
                .alignment(Alignment::Center);
            frame.render_widget(overlay, overlay_area);
        }
    }

    let context_block = theme
        .block("Context")
        .style(Style::default().bg(theme.subpanel_bg));
    let context_inner = context_block.inner(left_chunks[1]);
    frame.render_widget(context_block, left_chunks[1]);

    let min_context_lines = 8u16;
    let legend_height = details_area.height.min(
        context_inner
            .height
            .saturating_sub(min_context_lines)
            .max(3),
    );
    let context_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(min_context_lines), Constraint::Length(legend_height)])
        .split(context_inner);

    let overrides_total = app.conflicts.len();
    let overrides_manual = app
        .conflicts
        .iter()
        .filter(|entry| entry.overridden)
        .count();
    let overrides_auto = overrides_total.saturating_sub(overrides_manual);
    let label_style = Style::default().fg(theme.muted);
    let mut rows = Vec::new();
    rows.push(KvRow {
        label: "Game".to_string(),
        value: app.game_id.display_name().to_string(),
        label_style,
        value_style: Style::default().fg(theme.text),
    });
    rows.push(KvRow {
        label: "Profile".to_string(),
        value: app.active_profile_label(),
        label_style,
        value_style: Style::default().fg(if app.is_renaming_active_profile() {
            theme.warning
        } else {
            theme.text
        }),
    });
    rows.push(KvRow {
        label: "Mods".to_string(),
        value: mods_label.clone(),
        label_style,
        value_style: Style::default().fg(theme.text),
    });
    rows.push(KvRow {
        label: "Enabled".to_string(),
        value: enabled_label.clone(),
        label_style,
        value_style: Style::default().fg(theme.text),
    });
    rows.push(KvRow {
        label: "Overrides".to_string(),
        value: format!("Auto ({overrides_auto})"),
        label_style,
        value_style: Style::default().fg(theme.success),
    });
    rows.push(KvRow {
        label: "".to_string(),
        value: format!("Manual ({overrides_manual})"),
        label_style,
        value_style: Style::default().fg(if overrides_manual > 0 {
            theme.warning
        } else {
            theme.muted
        }),
    });
    rows.push(KvRow {
        label: "Auto-deploy".to_string(),
        value: "On".to_string(),
        label_style,
        value_style: Style::default().fg(theme.muted),
    });
    if !app.paths_ready() {
        rows.push(KvRow {
            label: "Setup".to_string(),
            value: "Open Menu (Esc) to configure".to_string(),
            label_style,
            value_style: Style::default().fg(theme.warning),
        });
    }
    let legend_block = theme.subpanel("Legend");
    let legend_fill = Block::default().style(Style::default().bg(theme.subpanel_bg));
    frame.render_widget(legend_fill, context_chunks[1]);
    let legend_inner = legend_block.inner(context_chunks[1]);
    let legend_content_width =
        legend_inner.width.saturating_sub(SUBPANEL_PAD_X.saturating_mul(2)) as usize;
    let legend_content_height = legend_inner.height.saturating_sub(SUBPANEL_PAD_TOP) as usize;
    let legend_rows = legend_rows(app);
    let legend_key_width = legend_key_width(&legend_rows, legend_content_width);
    let context_label_width = rows
        .iter()
        .map(|row| row.label.chars().count())
        .max()
        .unwrap_or(0);
    let max_context_label = context_chunks[0]
        .width
        .saturating_sub(2) as usize;
    let shared_label_width = legend_key_width
        .max(context_label_width)
        .min(legend_content_width)
        .min(max_context_label);

    let mut context_lines = Vec::new();
    context_lines.push(Line::from(Span::styled(
        "Active",
        Style::default().fg(theme.accent),
    )));
    context_lines.extend(format_kv_lines_aligned(
        &rows,
        context_chunks[0].width as usize,
        shared_label_width,
    ));
    context_lines.push(Line::from(""));
    let context_widget =
        Paragraph::new(context_lines).style(Style::default().fg(theme.text).bg(theme.subpanel_bg));
    frame.render_widget(context_widget, context_chunks[0]);

    let legend_lines = build_legend_lines(
        &legend_rows,
        &theme,
        legend_content_width,
        legend_content_height,
        shared_label_width,
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
    let log_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(10), Constraint::Length(1)])
        .split(log_inner);
    let log_total = app.logs.len();
    let log_view = log_chunks[0].height.max(1) as usize;
    let max_scroll = log_total.saturating_sub(log_view);
    if app.log_scroll > max_scroll {
        app.log_scroll = max_scroll;
    }
    let log_lines = build_log_lines(app, &theme, log_view);
    let log = Paragraph::new(log_lines).style(Style::default().fg(theme.text).bg(log_bg));
    frame.render_widget(log, log_chunks[0]);
    let scroll = app.log_scroll;
    let log_start = log_total.saturating_sub(log_view + scroll);
    if log_total > log_view && log_view > 0 {
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

    let status_row = chunks[3];
    let bottom_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(10), Constraint::Length(STATUS_WIDTH)])
        .split(status_row);

    let conflict_area = bottom_chunks[0];
    let status_area = bottom_chunks[1];

    let overrides_focused = app.focus == Focus::Conflicts;
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

    let status_bg = theme.log_bg;
    let status_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(status_color))
        .style(Style::default().bg(status_bg))
        .padding(Padding {
            left: 1,
            right: 1,
            top: 0,
            bottom: 0,
        });
    frame.render_widget(status_block.clone(), status_area);
    let status_inner = status_block.inner(status_area);
    if app.is_busy() && status_inner.width > 0 && status_inner.height > 0 {
        let mut bar_width = status_inner.width / 3;
        if bar_width < 6 {
            bar_width = status_inner.width.min(6);
        }
        if bar_width == 0 {
            bar_width = status_inner.width;
        }
        let travel = status_inner.width.saturating_sub(bar_width);
        let tick = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            / 120;
        let cycle = travel.saturating_mul(2).max(1) as u128;
        let step = (tick % cycle) as u16;
        let offset = if step > travel {
            travel.saturating_mul(2).saturating_sub(step)
        } else {
            step
        };
        let bar_area = Rect {
            x: status_inner.x + offset,
            y: status_inner.y,
            width: bar_width.min(status_inner.width),
            height: status_inner.height,
        };
        let bar_color = if overrides_focused {
            theme.accent
        } else {
            theme.accent_soft
        };
        frame.render_widget(
            Block::default().style(Style::default().bg(bar_color)),
            bar_area,
        );
    }
    let status_text = truncate_text(&app.status, status_inner.width as usize);
    let status_widget = Paragraph::new(status_text)
        .style(Style::default().fg(status_color))
        .alignment(Alignment::Center);
    frame.render_widget(status_widget, status_inner);

    if app.dialog.is_some() {
        draw_dialog(frame, app, &theme);
    }
    draw_toast(frame, app, &theme, chunks[1]);
    if let InputMode::Browsing(browser) = &app.input_mode {
        draw_path_browser(frame, app, &theme, browser);
    }
    if app.smart_rank_preview.is_some() {
        draw_smart_rank_preview(frame, app, &theme);
    }
    if app.settings_menu.is_some() {
        draw_settings_menu(frame, app, &theme);
    }
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
    counts: &ModCounts,
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
    let placeholder = if editing { "<type to filter>" } else { "<all>" };
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
    let right_label = if app.mod_filter_active() {
        format!("Showing: {}/{}", counts.visible_total, counts.total)
    } else {
        format!("Total: {}", counts.total)
    };
    let right_width = right_label.chars().count() as u16;
    let right_width = right_width.min(area.width);
    let filter_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(10), Constraint::Length(right_width)])
        .split(area);
    let left_line = Line::from(vec![
        Span::styled("Filter: ", Style::default().fg(theme.muted)),
        Span::styled(value_text.to_string(), value_style),
    ]);
    let left = Paragraph::new(left_line)
        .style(Style::default().bg(theme.header_bg))
        .alignment(Alignment::Left);
    frame.render_widget(left, filter_chunks[0]);
    let right = Paragraph::new(Line::from(Span::styled(
        right_label,
        Style::default().fg(theme.muted),
    )))
    .style(Style::default().bg(theme.header_bg))
    .alignment(Alignment::Right);
    frame.render_widget(right, filter_chunks[1]);
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

fn format_visible_count(visible: usize, total: usize) -> String {
    if total == 0 {
        "0".to_string()
    } else if visible == total {
        total.to_string()
    } else {
        format!("{visible}/{total}")
    }
}

fn format_enabled_count(
    visible_enabled: usize,
    visible_total: usize,
    enabled_total: usize,
    filter_active: bool,
) -> String {
    if filter_active {
        if visible_total == 0 {
            "0".to_string()
        } else {
            format!("{visible_enabled}/{visible_total}")
        }
    } else {
        enabled_total.to_string()
    }
}

fn status_color(app: &App, theme: &Theme) -> Color {
    let lower = app.status.to_lowercase();
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
                Span::styled(label, Style::default().fg(color).add_modifier(Modifier::BOLD)),
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
    let manual_count = app.conflicts.iter().filter(|entry| entry.overridden).count();
    let auto_count = total.saturating_sub(manual_count);
    let auto_text = format!("Auto ({auto_count})");
    let manual_text = format!("Manual ({manual_count})");

    let auto_style = Style::default()
        .fg(theme.success)
        .add_modifier(if focused && !conflict.overridden {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });
    let manual_style = Style::default()
        .fg(if manual_count > 0 { theme.warning } else { theme.muted })
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
    let manual_count = app.conflicts.iter().filter(|entry| entry.overridden).count();
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
        let mut total_len = short_auto.len()
            + if short_manual.is_empty() { 0 } else { 3 }
            + short_manual.len();
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

fn draw_dialog(frame: &mut Frame<'_>, app: &App, theme: &Theme) {
    let Some(dialog) = &app.dialog else {
        return;
    };

    let area = frame.size();
    let message_lines = build_dialog_message_lines(dialog, theme);

    let yes_selected = matches!(dialog.choice, DialogChoice::Yes);
    let yes_style = if yes_selected {
        Style::default()
            .fg(Color::Black)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text)
    };
    let no_style = if !yes_selected {
        Style::default()
            .fg(Color::Black)
            .bg(theme.warning)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text)
    };

    let buttons = Line::from(vec![
        Span::raw(" "),
        Span::styled(format!(" {} ", dialog.yes_label), yes_style),
        Span::raw("   "),
        Span::styled(format!(" {} ", dialog.no_label), no_style),
    ]);

    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        dialog.title.clone(),
        Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    lines.extend(message_lines);
    if let Some(toggle) = &dialog.toggle {
        let marker = if toggle.checked { "[x]" } else { "[ ]" };
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(marker, Style::default().fg(theme.accent)),
            Span::raw(" "),
            Span::styled(toggle.label.clone(), Style::default().fg(theme.text)),
        ]));
        lines.push(Line::from(Span::styled(
            "Press D to toggle",
            Style::default().fg(theme.muted),
        )));
    }
    lines.push(Line::from(""));
    lines.push(buttons);

    let mut max_line = 0usize;
    for line in &lines {
        let width = line.to_string().chars().count();
        if width > max_line {
            max_line = width;
        }
    }
    let max_width = area.width.saturating_sub(2).max(1);
    let width = (max_line as u16 + 6).clamp(38, max_width.min(72));
    let content_height = lines.len().max(1) as u16;
    let mut height = content_height + 2;
    if height < 8 {
        height = 8;
    }
    if height > area.height.saturating_sub(2) {
        height = area.height.saturating_sub(2);
    }
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let dialog_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, dialog_area);
    let dialog_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent_soft))
        .style(Style::default().bg(theme.header_bg));
    let dialog_widget = Paragraph::new(lines)
        .block(dialog_block)
        .style(Style::default().fg(theme.text))
        .alignment(Alignment::Center);
    frame.render_widget(dialog_widget, dialog_area);
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
        DialogKind::DeleteMod { name, native, .. } => {
            if *native {
                let line1 = Line::from(vec![
                    Span::styled("Remove native mod \"", Style::default().fg(theme.text)),
                    Span::styled(name.clone(), Style::default().fg(theme.text)),
                    Span::styled("\"?", Style::default().fg(theme.text)),
                ]);
                let line2 = Line::from(vec![
                    Span::styled(
                        "Unsubscribe in-game to stop updates.",
                        Style::default().fg(theme.muted),
                    ),
                ]);
                vec![line1, line2]
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
                vec![line1, line2]
            }
        }
        _ => dialog
            .message
            .lines()
            .map(|line| Line::from(line.to_string()))
            .collect(),
    }
}

fn draw_settings_menu(frame: &mut Frame<'_>, app: &App, theme: &Theme) {
    let Some(menu) = &app.settings_menu else {
        return;
    };

    let area = frame.size();
    let lines = build_settings_menu_lines(app, theme, menu.selected);
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
    let width = (max_line as u16 + 6).clamp(34, max_width.min(58));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let menu_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, menu_area);
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

fn draw_path_browser(frame: &mut Frame<'_>, _app: &App, theme: &Theme, browser: &PathBrowser) {
    let area = frame.size();
    let width = (area.width.saturating_sub(4)).clamp(46, 86);
    let height = (area.height.saturating_sub(4)).clamp(12, 22);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let modal = Rect::new(x, y, width, height);

    frame.render_widget(Clear, modal);

    let title = match browser.step {
        SetupStep::GameRoot => "Select BG3 install root",
        SetupStep::LarianDir => "Select Larian data dir",
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
        .constraints([Constraint::Length(3), Constraint::Min(5), Constraint::Length(2)])
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
        truncate_text(&browser.current.display().to_string(), chunks[0].width as usize)
    );
    let current_line = Line::from(Span::styled(
        current_label,
        Style::default().fg(theme.muted),
    ));
    let selectable = browser
        .entries
        .iter()
        .find(|entry| entry.kind == PathBrowserEntryKind::Select)
        .map(|entry| entry.selectable)
        .unwrap_or(false);
    let (valid_label, invalid_label) = match browser.step {
        SetupStep::GameRoot => (
            " BG3 install root valid ",
            "Not a BG3 install root (needs Data/ + bin/)",
        ),
        SetupStep::LarianDir => (
            " Larian data dir valid ",
            "Not a Larian data dir (needs PlayerProfiles/)",
        ),
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

    let entries: Vec<ListItem> = browser
        .entries
        .iter()
        .map(|entry| {
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
                PathBrowserEntryKind::Parent => Style::default().fg(theme.muted),
                PathBrowserEntryKind::Dir => Style::default().fg(theme.text),
            };
            ListItem::new(Line::from(Span::styled(entry.label.clone(), style)))
        })
        .collect();
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
    let spacer = Paragraph::new(Line::from(""))
        .style(Style::default().bg(theme.header_bg));
    frame.render_widget(spacer, list_chunks[0]);

    let mut state = ListState::default();
    let view_height = list_chunks[1].height as usize;
    let total = browser.entries.len();
    let mut offset = 0usize;
    if total > view_height && view_height > 0 {
        if browser.selected >= view_height {
            offset = browser.selected + 1 - view_height;
        }
        let max_offset = total.saturating_sub(view_height);
        if offset > max_offset {
            offset = max_offset;
        }
    }
    if total > 0 {
        let selected = browser.selected.saturating_sub(offset);
        state.select(Some(selected));
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

    let footer_plain =
        "[Tab] Switch  [Enter] Open/Select  [Backspace] Up  [S] Select  [Esc] Cancel";
    let footer_widget = if footer_plain.chars().count() > chunks[2].width as usize {
        let footer_line = truncate_text(footer_plain, chunks[2].width as usize);
        Paragraph::new(Line::from(Span::styled(
            footer_line,
            Style::default().fg(theme.muted),
        )))
        .alignment(Alignment::Center)
    } else {
        let key_style = Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD);
        let text_style = Style::default().fg(theme.muted);
        let footer_line = Line::from(vec![
            Span::styled("[Tab]", key_style),
            Span::styled(" Switch  ", text_style),
            Span::styled("[Enter]", key_style),
            Span::styled(" Open/Select  ", text_style),
            Span::styled("[Backspace]", key_style),
            Span::styled(" Up  ", text_style),
            Span::styled("[S]", key_style),
            Span::styled(" Select  ", text_style),
            Span::styled("[Esc]", key_style),
            Span::styled(" Cancel", text_style),
        ]);
        Paragraph::new(footer_line)
            .style(Style::default().fg(theme.muted))
            .alignment(Alignment::Center)
    };
    frame.render_widget(footer_widget, chunks[2]);
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

fn draw_smart_rank_preview(frame: &mut Frame<'_>, app: &mut App, theme: &Theme) {
    let Some(preview) = &app.smart_rank_preview else {
        return;
    };

    let area = frame.size();
    let max_width = area.width.saturating_sub(2).max(1);
    let width = max_width.min(120).max(60).min(max_width);
    let max_height = area.height.saturating_sub(2).max(1);
    let height = max_height.min(22).max(10);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let preview_area = Rect::new(x, y, width, height);

    let inner_width = preview_area.width.saturating_sub(3) as usize;
    let inner_height = preview_area.height.saturating_sub(2) as usize;
    let render = build_smart_rank_preview_render(
        preview,
        theme,
        inner_width,
        inner_height,
        app.smart_rank_scroll,
        app.smart_rank_view,
    );

    frame.render_widget(Clear, preview_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent_soft))
        .style(Style::default().bg(theme.header_bg))
        .title(Span::styled(
            "AI Smart Ranking",
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

fn build_smart_rank_preview_render(
    preview: &crate::app::SmartRankPreview,
    theme: &Theme,
    width: usize,
    height: usize,
    scroll: usize,
    view: crate::app::SmartRankView,
) -> SmartRankPreviewRender {
    if width == 0 || height == 0 {
        return SmartRankPreviewRender {
            lines: Vec::new(),
            scroll: None,
        };
    }

    let mut lines = Vec::new();
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
        mod_width = width
            .saturating_sub(current_width + proposed_width + created_width + added_width + sep_width);
        if mod_width < min_mod_width {
            let deficit = min_mod_width.saturating_sub(mod_width);
            let shrink_created = deficit / 2 + deficit % 2;
            let shrink_added = deficit / 2;
            created_width = created_width.saturating_sub(shrink_created).max(min_date_width);
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
        let created_pad =
            " ".repeat(created_width.saturating_sub(created_header.chars().count()));
        let added_header = truncate_text("Added", added_width);
        let added_pad = " ".repeat(added_width.saturating_sub(added_header.chars().count()));
        let current_header = truncate_text("Current", current_width);
        let current_pad =
            " ".repeat(current_width.saturating_sub(current_header.chars().count()));
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
            body_lines.push(Line::from(Span::styled(
                "No ordering changes detected.",
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
                let created_text = format_padded_cell(&format_date_cell(entry.created_at), created_width);
                let added_text =
                    format_padded_cell(&format_date_cell(Some(entry.added_at)), added_width);
                let current_text = format!("{:>width$}", entry.from + 1, width = current_width);
                let proposed_text = format!("{:>width$}", entry.to + 1, width = proposed_width);
                let row_bg = if index % 2 == 1 {
                    Some(theme.row_alt_bg)
                } else {
                    None
                };
                let mut mod_style = Style::default().fg(if is_major {
                    theme.accent
                } else {
                    theme.text
                });
                if is_major {
                    mod_style = mod_style
                        .add_modifier(Modifier::BOLD)
                        .bg(theme.accent_soft);
                }
                let mut date_style = Style::default().fg(theme.muted);
                let mut current_style = Style::default().fg(theme.muted);
                let mut proposed_style =
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD);
                let mut sep_style = Style::default().fg(theme.muted);
                let mut arrow_style = Style::default().fg(if is_major {
                    theme.success
                } else {
                    theme.muted
                });
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

fn format_padded_cell(value: &str, width: usize) -> String {
    let text = truncate_text(value, width);
    let pad = width.saturating_sub(text.chars().count());
    format!("{text}{}", " ".repeat(pad))
}

fn build_settings_menu_lines(app: &App, theme: &Theme, selected: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        "Settings",
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    let items = settings_items(app);
    for (index, item) in items.iter().enumerate() {
        let prefix = if index == selected { ">" } else { " " };
        let style = if index == selected {
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text)
        };
        let row = match item.kind {
            SettingsItemKind::ActionSetupPaths | SettingsItemKind::ActionSmartRank => vec![
                Span::styled(prefix.to_string(), style),
                Span::raw(" "),
                Span::styled("▶", Style::default().fg(theme.accent)),
                Span::raw(" "),
                Span::styled(item.label.to_string(), style),
            ],
            SettingsItemKind::ToggleProfileDelete | SettingsItemKind::ToggleModDelete => {
                let marker = if item.checked.unwrap_or(false) {
                    "[x]"
                } else {
                    "[ ]"
                };
                vec![
                    Span::styled(prefix.to_string(), style),
                    Span::raw(" "),
                    Span::styled(marker, Style::default().fg(theme.accent)),
                    Span::raw(" "),
                    Span::styled(item.label.to_string(), style),
                ]
            }
        };
        lines.push(Line::from(row));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Keybinds",
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        "Tab: cycle focus  Esc: close",
        Style::default().fg(theme.muted),
    )));
    lines.push(Line::from(Span::styled(
        "Ctrl+F: filter    Ctrl+L: clear",
        Style::default().fg(theme.muted),
    )));
    lines.push(Line::from(Span::styled(
        "Del: remove mod   r/F2: rename profile",
        Style::default().fg(theme.muted),
    )));
    lines.push(Line::from(Span::styled(
        "1-5: set target override",
        Style::default().fg(theme.muted),
    )));
    lines.push(Line::from(Span::styled(
        "←/→: select override  ↑/↓: choose",
        Style::default().fg(theme.muted),
    )));

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
    let path_width = 50usize;

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Paths",
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        format!("Root: {}", truncate_text(&root, path_width)),
        Style::default().fg(theme.muted),
    )));
    lines.push(Line::from(Span::styled(
        format!("User: {}", truncate_text(&user_dir, path_width)),
        Style::default().fg(theme.muted),
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "Config: {}",
            truncate_text(&config_path.display().to_string(), path_width)
        ),
        Style::default().fg(theme.muted),
    )));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Enter: toggle/run | Esc: close",
        Style::default().fg(theme.muted),
    )));

    lines
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
            let auto_hint = "Pause/Enter to import | Esc cancel";
            let hint = if *auto_submit { auto_hint } else { default_hint };
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
                InputPurpose::ExportProfile { profile } => {
                    let path = value("<path>");
                    format!("Export \"{profile}\": {path} | {hint}")
                }
                InputPurpose::ImportProfile => {
                    let path = value("<path>");
                    format!("Import profile list: {path} | {hint}")
                }
                InputPurpose::ImportPath => {
                    let path = value("<path>");
                    let hint = if *auto_submit { auto_hint } else { default_hint };
                    format!("Import mod: {path} | {hint}")
                }
                InputPurpose::FilterMods => {
                    let filter = value("<all>");
                    format!("Filter mods: {filter} | {hint}")
                }
            };
            Some((message, ToastLevel::Info))
        }
        InputMode::Browsing(_) => None,
        InputMode::Normal => {
            if app.move_mode {
                Some((
                    "Move mode: arrows reorder | Enter/Esc exit".to_string(),
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
    let max_text = max_width
        .saturating_sub(2 + padding_x.saturating_mul(2)) as usize;
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
        label_style = label_style
            .fg(theme.warning)
            .add_modifier(Modifier::BOLD);
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
            spans.push(Span::styled(item.label.clone(), label_style.fg(theme.accent)));
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
    visible_total: usize,
    visible_enabled: usize,
}

fn build_rows(app: &App, theme: &Theme) -> (Vec<Row<'static>>, ModCounts, usize) {
    let mut rows = Vec::new();
    let mut visible_enabled = 0;
    let mut target_width = "Target".chars().count();
    let (total, enabled) = app.profile_counts();
    let profile_entries = app.visible_profile_entries();
    let mod_map = app.library.index_by_id();

    for (row_index, (order_index, entry)) in profile_entries.iter().enumerate() {
        let Some(mod_entry) = mod_map.get(&entry.id) else {
            continue;
        };
        if entry.enabled {
            visible_enabled += 1;
        }
        let (row, target_len) = row_for_entry(
            app,
            row_index,
            *order_index,
            entry.enabled,
            mod_entry,
            theme,
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
            visible_enabled,
        },
        target_width,
    )
}

fn row_for_entry(
    app: &App,
    row_index: usize,
    order_index: usize,
    enabled: bool,
    mod_entry: &ModEntry,
    theme: &Theme,
) -> (Row<'static>, usize) {
    let (enabled_text, enabled_style) = if enabled {
        ("[x]", Style::default().fg(theme.success))
    } else {
        ("[ ]", Style::default().fg(theme.muted))
    };
    let kind = mod_kind_label(mod_entry);
    let kind_style = match kind {
        "Pak" => Style::default().fg(theme.accent),
        "Loose" => Style::default().fg(theme.success),
        _ => Style::default().fg(theme.text),
    };
    let (state_label, state_style) = mod_path_label(app, mod_entry, theme, true);
    let target_len = state_label.chars().count();
    let native_marker = if mod_entry.is_native() { "✓" } else { " " };
    let native_style = if mod_entry.is_native() {
        Style::default().fg(theme.success)
    } else {
        Style::default().fg(theme.muted)
    };
    let created_text = format_date_cell(mod_entry.created_at);
    let added_text = format_date_cell(Some(mod_entry.added_at));
    let mut row = Row::new(vec![
        Cell::from(enabled_text.to_string()).style(enabled_style),
        Cell::from(native_marker.to_string()).style(native_style),
        Cell::from((order_index + 1).to_string()),
        Cell::from(kind.to_string()).style(kind_style),
        Cell::from(state_label).style(state_style),
        Cell::from(created_text).style(Style::default().fg(theme.muted)),
        Cell::from(added_text).style(Style::default().fg(theme.muted)),
        Cell::from(mod_entry.display_name()),
    ]);
    if row_index % 2 == 1 {
        row = row.style(Style::default().bg(theme.row_alt_bg));
    }
    (row, target_len)
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
    format_short_date(value.unwrap_or_default()).unwrap_or_else(|| "---- -- --".to_string())
}

fn pad_lines(
    lines: Vec<Line<'static>>,
    left_pad: usize,
    top_pad: usize,
) -> Vec<Line<'static>> {
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

fn build_details(app: &App, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    if app.focus == Focus::Explorer {
        return build_explorer_details(app, theme, width);
    }
    if app.focus == Focus::Conflicts {
        return build_conflict_details(app, theme, width);
    }

    let profile_entries = app.visible_profile_entries();
    let mod_map = app.library.index_by_id();

    let Some((order_index, entry)) = profile_entries.get(app.selected) else {
        return vec![Line::from("No mod selected.")];
    };
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
        rows.push(KvRow {
            label: "Source".to_string(),
            value: "Native (mod.io)".to_string(),
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
    let enabled_label = if entry.enabled { "Yes" } else { "No" };
    let enabled_style =
        Style::default().fg(if entry.enabled { theme.success } else { theme.muted });
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
                    value: app.config.data_dir.join("config.json").display().to_string(),
                    label_style,
                    value_style,
                });
                let status_style =
                    Style::default().fg(if app.paths_ready() { theme.success } else { theme.warning });
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

fn build_conflict_details(app: &App, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let swap_active = app.override_swap_info().is_some();
    if !swap_active {
        if app.conflicts_scanning() {
            return vec![Line::from(Span::styled(
                "Scanning overrides...",
                Style::default().fg(theme.muted),
            ))];
        }
        if app.conflicts_pending() {
            return vec![Line::from(Span::styled(
                "Override scan queued...",
                Style::default().fg(theme.muted),
            ))];
        }
    }
    let Some(conflict) = app.conflicts.get(app.conflict_selected) else {
        return vec![Line::from(Span::styled(
            "No overrides detected.",
            Style::default().fg(theme.muted),
        ))];
    };

    let label_style = Style::default().fg(theme.muted);
    let value_style = Style::default().fg(theme.text);
    let mut rows = Vec::new();
    rows.push(KvRow {
        label: "Target".to_string(),
        value: target_kind_label(conflict.target).to_string(),
        label_style,
        value_style,
    });
    let path_label = conflict.relative_path.to_string_lossy().to_string();
    rows.push(KvRow {
        label: "Path".to_string(),
        value: path_label,
        label_style,
        value_style,
    });
    let winner_style = Style::default().fg(theme.success);
    rows.push(KvRow {
        label: "Winner".to_string(),
        value: conflict.winner_name.clone(),
        label_style,
        value_style: winner_style,
    });
    let mut lines = format_kv_lines(&rows, width);
    if conflict.overridden {
        lines.push(Line::from(Span::styled(
            "Manual override active",
            Style::default().fg(theme.warning),
        )));
    }
    lines.push(Line::from(Span::styled(
        "Candidates:",
        Style::default().fg(theme.accent),
    )));
    for candidate in &conflict.candidates {
        let selected = candidate.mod_id == conflict.winner_id;
        let marker = if selected { "[x]" } else { "[ ]" };
        let style = if selected {
            Style::default().fg(theme.success)
        } else {
            Style::default().fg(theme.muted)
        };
        let prefix = format!("{marker} ");
        push_truncated_prefixed(&mut lines, &prefix, style, &candidate.mod_name, style, width);
    }

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

fn mod_path_label(app: &App, mod_entry: &ModEntry, theme: &Theme, _compact: bool) -> (String, Style) {
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
        Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)
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
        let label = format!(
            "{} [{}]",
            kind_label,
            override_key_label(Some(enabled[0]))
        );
        return (
            label,
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
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

struct LegendRow {
    key: String,
    action: String,
}

struct KvRow {
    label: String,
    value: String,
    label_style: Style,
    value_style: Style,
}

fn legend_rows(app: &App) -> Vec<LegendRow> {
    let mut rows = Vec::new();
    match app.focus {
        Focus::Explorer => {
            rows.extend([
                LegendRow {
                    key: "Enter".to_string(),
                    action: "Select or expand".to_string(),
                },
                LegendRow {
                    key: "[x]".to_string(),
                    action: "Active profile".to_string(),
                },
                LegendRow {
                    key: "[ ]".to_string(),
                    action: "Inactive profile".to_string(),
                },
                LegendRow {
                    key: "a".to_string(),
                    action: "New profile".to_string(),
                },
                LegendRow {
                    key: "r/F2".to_string(),
                    action: "Rename profile".to_string(),
                },
                LegendRow {
                    key: "c".to_string(),
                    action: "Duplicate profile".to_string(),
                },
                LegendRow {
                    key: "Del".to_string(),
                    action: "Delete profile".to_string(),
                },
                LegendRow {
                    key: "e".to_string(),
                    action: "Export profile".to_string(),
                },
                LegendRow {
                    key: "p".to_string(),
                    action: "Import profile".to_string(),
                },
                LegendRow {
                    key: "Tab".to_string(),
                    action: "Cycle focus".to_string(),
                },
            ]);
        }
        Focus::Conflicts => {
            rows.extend([
                LegendRow {
                    key: "←/→".to_string(),
                    action: "Select override".to_string(),
                },
                LegendRow {
                    key: "↑/↓".to_string(),
                    action: "Choose winner".to_string(),
                },
                LegendRow {
                    key: "Enter".to_string(),
                    action: "Cycle winner".to_string(),
                },
                LegendRow {
                    key: "Backspace".to_string(),
                    action: "Clear override".to_string(),
                },
                LegendRow {
                    key: "Tab".to_string(),
                    action: "Cycle focus".to_string(),
                },
            ]);
        }
        Focus::Mods => {
            rows.extend([
                LegendRow {
                    key: "Space".to_string(),
                    action: "Toggle enable".to_string(),
                },
                LegendRow {
                    key: "N".to_string(),
                    action: "Native mod (mod.io)".to_string(),
                },
                LegendRow {
                    key: "m".to_string(),
                    action: "Move mode".to_string(),
                },
                LegendRow {
                    key: "u/n".to_string(),
                    action: "Move order".to_string(),
                },
                LegendRow {
                    key: "Del".to_string(),
                    action: "Remove mod".to_string(),
                },
                LegendRow {
                    key: "Target [1-5]".to_string(),
                    action: "Auto/Mods/Gen/Data/Bin".to_string(),
                },
                LegendRow {
                    key: "a/s/x".to_string(),
                    action: "Enable/Disable/Invert visible".to_string(),
                },
                LegendRow {
                    key: "c".to_string(),
                    action: "Clear overrides".to_string(),
                },
                LegendRow {
                    key: "Ctrl+F/Ctrl+L".to_string(),
                    action: "Filter/Clear".to_string(),
                },
                LegendRow {
                    key: "Tab".to_string(),
                    action: "Cycle focus".to_string(),
                },
            ]);
        }
        Focus::Log => {
            rows.extend([
                LegendRow {
                    key: "↑/↓".to_string(),
                    action: "Scroll log".to_string(),
                },
                LegendRow {
                    key: "PgUp/PgDn".to_string(),
                    action: "Page scroll".to_string(),
                },
                LegendRow {
                    key: "Tab".to_string(),
                    action: "Cycle focus".to_string(),
                },
            ]);
        }
    }
    rows.push(LegendRow {
        key: "Esc".to_string(),
        action: "Menu".to_string(),
    });
    rows
}

fn legend_key_width(rows: &[LegendRow], width: usize) -> usize {
    rows.iter()
        .map(|row| row.key.chars().count())
        .max()
        .unwrap_or(0)
        .min(width)
}

fn build_legend_lines(
    rows: &[LegendRow],
    theme: &Theme,
    width: usize,
    height: usize,
    key_width: usize,
) -> Vec<Line<'static>> {
    if width == 0 || height == 0 {
        return Vec::new();
    }

    let mut lines = format_legend_rows(rows, width, key_width, theme);
    if lines.len() > height {
        lines.truncate(height);
    }
    lines
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
    let spacing = 2usize;
    let action_width = width.saturating_sub(key_width + spacing);

    rows.iter()
        .map(|row| {
            let key_text = truncate_text(&row.key, key_width);
            let key_len = key_text.chars().count();
            let pad = " ".repeat(key_width.saturating_sub(key_len) + spacing);
            let action_text = truncate_text(&row.action, action_width);
            Line::from(vec![
                Span::styled(
                    key_text,
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(pad),
                Span::styled(action_text, Style::default().fg(theme.text)),
            ])
        })
        .collect()
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

fn format_kv_lines_aligned(
    rows: &[KvRow],
    width: usize,
    label_width: usize,
) -> Vec<Line<'static>> {
    if width == 0 || rows.is_empty() {
        return Vec::new();
    }
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

    let label_width = label_width.min(width.saturating_sub(2));
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
