use crate::{
    app::{
        App, DialogChoice, ExplorerItem, ExplorerItemKind, Focus, InputMode, InputPurpose,
        LogLevel, ToastLevel,
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
        Row, Table, TableState, Wrap,
    },
};
use std::{io, path::Path, time::{Duration, Instant}};

const SIDE_PANEL_WIDTH: u16 = 44;

#[derive(Clone)]
struct Theme {
    accent: Color,
    accent_soft: Color,
    border: Color,
    text: Color,
    muted: Color,
    success: Color,
    warning: Color,
    error: Color,
    header_bg: Color,
    log_bg: Color,
}

impl Theme {
    fn new() -> Self {
        Self {
            accent: Color::Rgb(120, 190, 255),
            accent_soft: Color::Rgb(70, 110, 160),
            border: Color::Rgb(65, 75, 90),
            text: Color::Rgb(220, 230, 240),
            muted: Color::Rgb(135, 145, 155),
            success: Color::Rgb(120, 220, 140),
            warning: Color::Rgb(230, 200, 120),
            error: Color::Rgb(235, 100, 95),
            header_bg: Color::Rgb(22, 28, 36),
            log_bg: Color::Rgb(16, 20, 26),
        }
    }

    fn block(&self, title: &'static str) -> Block<'static> {
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.border))
            .title(Span::styled(
                title,
                Style::default()
                    .fg(self.accent)
                    .add_modifier(Modifier::BOLD),
            ))
    }

    fn panel(&self, title: &'static str) -> Block<'static> {
        self.block(title).padding(Padding {
            left: 1,
            right: 1,
            top: 1,
            bottom: 0,
        })
    }

    fn panel_dense(&self, title: &'static str) -> Block<'static> {
        self.block(title).padding(Padding {
            left: 0,
            right: 1,
            top: 1,
            bottom: 0,
        })
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
    if app.dialog.is_some() {
        return handle_dialog_mode(app, key);
    }

    let mode = std::mem::replace(&mut app.input_mode, InputMode::Normal);
    match mode {
        InputMode::Normal => {
            app.input_mode = InputMode::Normal;
            handle_normal_mode(app, key)
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

fn handle_normal_mode(app: &mut App, key: KeyEvent) -> Result<()> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), _) | (KeyCode::Char('Q'), _) => app.should_quit = true,
        (KeyCode::Char('i'), _) | (KeyCode::Char('I'), _) => app.enter_import_mode(),
        (KeyCode::Char('g'), _) | (KeyCode::Char('G'), _) => app.enter_setup_game_root(),
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
        (KeyCode::PageUp, _) => app.scroll_log_up(3),
        (KeyCode::PageDown, _) => app.scroll_log_down(3),
        (KeyCode::Tab, _) => app.cycle_focus(),
        _ => {}
    }

    match app.focus {
        Focus::Explorer => handle_explorer_mode(app, key)?,
        Focus::Mods => handle_mods_mode(app, key)?,
        Focus::Conflicts => handle_conflicts_mode(app, key)?,
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
        KeyCode::Char('p') | KeyCode::Char('P') => app.enter_import_profile(),
        _ => {}
    }

    Ok(())
}

fn handle_mods_mode(app: &mut App, key: KeyEvent) -> Result<()> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('m'), _) | (KeyCode::Char('M'), _) => app.toggle_move_mode(),
        (KeyCode::Enter, _) | (KeyCode::Esc, _) if app.move_mode => app.toggle_move_mode(),
        (KeyCode::Char(' '), _) => app.toggle_selected(),
        (KeyCode::Delete, _) | (KeyCode::Backspace, _) => app.remove_selected(),
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
        (KeyCode::Char('1'), _) => app.toggle_target(TargetKind::Pak),
        (KeyCode::Char('2'), _) => app.toggle_target(TargetKind::Generated),
        (KeyCode::Char('3'), _) => app.toggle_target(TargetKind::Data),
        (KeyCode::Char('4'), _) => app.toggle_target(TargetKind::Bin),
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
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => app.conflict_move_up(),
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => app.conflict_move_down(),
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('H') => {
            app.cycle_conflict_winner(-1)
        }
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('L') | KeyCode::Enter => {
            app.cycle_conflict_winner(1)
        }
        KeyCode::Backspace | KeyCode::Delete => app.clear_conflict_override(),
        _ => {}
    }

    Ok(())
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
                InputPurpose::SetupGameRoot => "Game root setup cancelled".to_string(),
                InputPurpose::SetupLarianDir => "User dir setup cancelled".to_string(),
            };
            app.set_toast(&cancel_message, ToastLevel::Warn, Duration::from_secs(2));
            if matches!(
                purpose,
                InputPurpose::SetupGameRoot | InputPurpose::SetupLarianDir
            ) {
                app.status = "Setup required: press g to set game paths".to_string();
            }
        }
        KeyCode::Enter => {
            let value = buffer.trim().to_string();
            app.input_mode = InputMode::Normal;
            keep_editing = false;
            if !value.is_empty() {
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

fn draw(frame: &mut Frame<'_>, app: &App) {
    let area = frame.size();
    let theme = Theme::new();
    let focus_label = match app.focus {
        Focus::Explorer => "Explorer",
        Focus::Mods => "Mod Stack",
        Focus::Conflicts => "Conflicts",
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(10), Constraint::Length(11)])
        .split(area);

    let (rows, total_mods, enabled_mods) = build_rows(app, &theme);
    let profile_label = app.active_profile_label();
    let renaming_active = app.is_renaming_active_profile();
    let header = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(
                "SigilSmith",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(app.game_id.display_name(), Style::default().fg(theme.text)),
            Span::raw("  "),
            Span::styled(
                focus_label,
                Style::default()
                    .fg(theme.muted)
                    .add_modifier(Modifier::BOLD),
            ),
            if app.move_mode {
                Span::styled(
                    "  MOVE",
                    Style::default().fg(theme.warning).add_modifier(Modifier::BOLD),
                )
            } else {
                Span::raw("")
            },
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Profile: ", Style::default().fg(theme.muted)),
            Span::styled(
                profile_label,
                Style::default().fg(if renaming_active {
                    theme.warning
                } else {
                    theme.accent
                }),
            ),
            if renaming_active {
                Span::styled(" (renaming)", Style::default().fg(theme.muted))
            } else {
                Span::raw("")
            },
            Span::raw("   "),
            Span::styled("Mods: ", Style::default().fg(theme.muted)),
            Span::styled(total_mods.to_string(), Style::default().fg(theme.text)),
            Span::raw("   "),
            Span::styled("Enabled: ", Style::default().fg(theme.muted)),
            Span::styled(
                enabled_mods.to_string(),
                Style::default().fg(theme.success).add_modifier(Modifier::BOLD),
            ),
        ]),
    ])
    .style(Style::default().bg(theme.header_bg))
    .alignment(Alignment::Center);
    frame.render_widget(header, chunks[0]);

    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(SIDE_PANEL_WIDTH),
            Constraint::Min(20),
            Constraint::Length(SIDE_PANEL_WIDTH),
        ])
        .split(chunks[1]);

    let explorer_items = build_explorer_items(app, &theme);
    if explorer_items.is_empty() {
        let empty = Paragraph::new("No games available.")
            .style(Style::default().fg(theme.muted))
            .block(theme.panel("Explorer"))
            .alignment(Alignment::Center);
        frame.render_widget(empty, body_chunks[0]);
    } else {
        let highlight_style = if app.focus == Focus::Explorer {
            Style::default()
                .bg(theme.accent_soft)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text)
        };
        let explorer = List::new(explorer_items)
            .block(theme.panel("Explorer"))
            .highlight_style(highlight_style)
            .highlight_symbol(if app.focus == Focus::Explorer { ">" } else { " " });
        let mut state = ListState::default();
        state.select(Some(app.explorer_selected));
        frame.render_stateful_widget(explorer, body_chunks[0], &mut state);
    }

    if rows.is_empty() {
        let empty = Paragraph::new("Drop a mod archive or folder to import.")
            .style(Style::default().fg(theme.muted))
            .block(theme.panel_dense("Mod Stack"))
            .alignment(Alignment::Center);
        frame.render_widget(empty, body_chunks[1]);
    } else {
        let table = Table::new(
            rows,
            [
                Constraint::Length(3),
                Constraint::Length(5),
                Constraint::Length(7),
                Constraint::Length(6),
                Constraint::Min(10),
            ],
        )
        .header(Row::new(vec![
            Cell::from("On"),
            Cell::from("Order"),
            Cell::from("Kind"),
            Cell::from("Path"),
            Cell::from("Mod"),
        ])
        .style(Style::default().fg(theme.text).add_modifier(Modifier::BOLD)))
        .column_spacing(1)
        .block(theme.panel_dense("Mod Stack"))
        .highlight_style(if app.focus == Focus::Mods {
            Style::default()
                .bg(theme.accent_soft)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text)
        })
        .highlight_symbol(if app.focus == Focus::Mods { ">" } else { " " });

        let mut state = TableState::default();
        state.select(Some(app.selected));
        frame.render_stateful_widget(table, body_chunks[1], &mut state);
    }

    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(9), Constraint::Min(8), Constraint::Length(9)])
        .split(body_chunks[2]);

    let navigator = Paragraph::new(vec![
        Line::from(Span::styled("Active", Style::default().fg(theme.accent))),
        Line::from(vec![
            Span::styled("Game: ", Style::default().fg(theme.muted)),
            Span::styled(app.game_id.display_name(), Style::default().fg(theme.text)),
        ]),
        Line::from(vec![
            Span::styled("Profile: ", Style::default().fg(theme.muted)),
            Span::styled(
                app.active_profile_label(),
                Style::default().fg(if app.is_renaming_active_profile() {
                    theme.warning
                } else {
                    theme.text
                }),
            ),
            if app.is_renaming_active_profile() {
                Span::styled(" (renaming)", Style::default().fg(theme.muted))
            } else {
                Span::raw("")
            },
        ]),
        Line::from(vec![
            Span::styled("Mods: ", Style::default().fg(theme.muted)),
            Span::styled(total_mods.to_string(), Style::default().fg(theme.text)),
            Span::raw("   "),
            Span::styled("Enabled: ", Style::default().fg(theme.muted)),
            Span::styled(enabled_mods.to_string(), Style::default().fg(theme.text)),
        ]),
        Line::from(vec![
            Span::styled("Conflicts: ", Style::default().fg(theme.muted)),
            Span::styled(
                app.conflicts.len().to_string(),
                Style::default().fg(if app.conflicts.is_empty() {
                    theme.muted
                } else {
                    theme.warning
                }),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Auto-deploy: On",
            Style::default().fg(theme.muted),
        )),
    ])
    .style(Style::default().fg(theme.text))
    .block(theme.panel("Context"));
    frame.render_widget(navigator, right_chunks[0]);

    let details_block = theme.panel("Details");
    let details_inner = details_block.inner(right_chunks[1]);
    let details_lines = build_details(app, &theme, details_inner.width as usize);
    let details = Paragraph::new(details_lines)
        .style(Style::default().fg(theme.text))
        .block(details_block)
        .wrap(Wrap { trim: false });
    frame.render_widget(details, right_chunks[1]);

    if app.focus == Focus::Conflicts {
        let conflict_block = theme.panel("Conflicts");
        let conflict_inner = conflict_block.inner(right_chunks[2]);
        let conflict_lines = build_conflict_lines(app, &theme, conflict_inner.height as usize);
        let conflicts = Paragraph::new(conflict_lines)
            .style(Style::default().fg(theme.text))
            .block(conflict_block);
        frame.render_widget(conflicts, right_chunks[2]);
    } else {
        let overrides = Paragraph::new(build_override_lines(app, &theme))
            .style(Style::default().fg(theme.text))
            .block(theme.panel("Overrides"));
        frame.render_widget(overrides, right_chunks[2]);
    }

    let footer_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Length(7)])
        .split(chunks[2]);

    let status_block = theme.panel("Status");
    let status_inner = status_block.inner(footer_chunks[0]);
    let footer = Paragraph::new(status_bar_line(app, status_inner.width))
        .style(Style::default().fg(theme.text))
        .block(status_block);
    frame.render_widget(footer, footer_chunks[0]);

    let log_area = footer_chunks[1];
    let log_block = theme.panel("Log").style(Style::default().bg(theme.log_bg));
    let log_inner = log_block.inner(log_area);
    let log_lines = build_log_lines(app, &theme, log_inner.height as usize);
    let log = Paragraph::new(log_lines)
        .style(Style::default().fg(theme.text).bg(theme.log_bg))
        .block(log_block);
    frame.render_widget(log, log_area);

    if app.dialog.is_some() {
        draw_dialog(frame, app, &theme);
    }
    draw_toast(frame, app, &theme, chunks[1]);
}

fn status_bar_line(app: &App, width: u16) -> String {
    let width = width as usize;
    let (left, right) = match &app.input_mode {
        InputMode::Normal => (format!("Status: {}", app.status), app.hint().to_string()),
        InputMode::Editing {
            prompt,
            buffer,
            auto_submit,
            ..
        } => {
            let right = if *auto_submit {
                "Auto import: pause to accept | Esc cancel"
            } else {
                "Enter confirm | Esc cancel"
            };
            (format!("{prompt}: {buffer}"), right.to_string())
        }
    };

    if width == 0 {
        return String::new();
    }

    if left.len() + right.len() + 1 > width {
        let available = width.saturating_sub(left.len() + 1);
        let mut trimmed_right = right;
        if trimmed_right.len() > available {
            trimmed_right.truncate(available);
        }
        return format!("{left} {}", trimmed_right);
    }

    let spaces = width - left.len() - right.len();
    format!("{left}{}{}", " ".repeat(spaces), right)
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

fn build_conflict_lines(app: &App, theme: &Theme, height: usize) -> Vec<Line<'static>> {
    if height == 0 {
        return Vec::new();
    }
    if app.conflicts_scanning() {
        return vec![Line::from(Span::styled(
            "Scanning conflicts...",
            Style::default().fg(theme.muted),
        ))];
    }
    if app.conflicts_pending() {
        return vec![Line::from(Span::styled(
            "Conflict scan queued...",
            Style::default().fg(theme.muted),
        ))];
    }
    if app.conflicts.is_empty() {
        return vec![Line::from(Span::styled(
            "No conflicts detected.",
            Style::default().fg(theme.muted),
        ))];
    }

    let total = app.conflicts.len();
    let view = height.max(1);
    let selected = app.conflict_selected.min(total.saturating_sub(1));
    let start = if selected + 1 > view {
        selected + 1 - view
    } else {
        0
    };
    let end = (start + view).min(total);

    app.conflicts[start..end]
        .iter()
        .enumerate()
        .map(|(offset, conflict)| {
            let index = start + offset;
            let selected_line = index == selected && app.focus == Focus::Conflicts;
            let marker = if conflict.overridden { "*" } else { " " };
            let kind = conflict_short_label(conflict.target);
            let mut label = format!(
                "{marker}[{kind}] {} -> {}",
                conflict.relative_path.to_string_lossy(),
                conflict.winner_name
            );
            let others = conflict.candidates.len().saturating_sub(1);
            if others > 0 {
                label.push_str(&format!(" (+{others})"));
            }
            let style = if selected_line {
                Style::default()
                    .bg(theme.accent_soft)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text)
            };
            Line::from(Span::styled(label, style))
        })
        .collect()
}

fn draw_dialog(frame: &mut Frame<'_>, app: &App, theme: &Theme) {
    let Some(dialog) = &app.dialog else {
        return;
    };

    let area = frame.size();
    let message_lines: Vec<Line> = dialog
        .message
        .lines()
        .map(|line| Line::from(line.to_string()))
        .collect();
    let content_height = message_lines.len().max(1) as u16;
    let mut height = content_height + 6;
    if height < 7 {
        height = 7;
    }
    if height > area.height.saturating_sub(2) {
        height = area.height.saturating_sub(2);
    }
    let width = area.width.saturating_mul(2) / 3;
    let width = width.clamp(34, area.width.saturating_sub(2).max(34));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let dialog_area = Rect::new(x, y, width, height);

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
    lines.push(Line::from(""));
    lines.push(buttons);

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
                InputPurpose::SetupGameRoot => {
                    let path = value("<path>");
                    format!("Set game root: {path} | {hint}")
                }
                InputPurpose::SetupLarianDir => {
                    let path = value("<path>");
                    format!("Set user dir: {path} | {hint}")
                }
            };
            Some((message, ToastLevel::Info))
        }
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
    let max_width = body_area.width.saturating_sub(4).max(24);
    let max_text = max_width.saturating_sub(4) as usize;
    if message.len() > max_text {
        message.truncate(max_text.saturating_sub(3));
        message.push_str("...");
    }
    let width = (message.len() as u16 + 4).clamp(24, max_width);
    let height = 3u16;
    let x = body_area.x + (body_area.width.saturating_sub(width)) / 2;
    let y = body_area.y + 1;
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
        .style(Style::default().bg(theme.header_bg));
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
            out.push_str("| ");
        } else {
            out.push_str("  ");
        }
    }

    let branch = if has_next_at_depth(items, index, depth) {
        "|- "
    } else {
        "+- "
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
            let prefix = if item.expanded { "[-]" } else { "[+]" };
            spans.push(Span::styled(prefix, muted));
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
            let prefix = if item.expanded { "[-]" } else { "[+]" };
            spans.push(Span::styled(prefix, muted));
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

fn build_rows(app: &App, theme: &Theme) -> (Vec<Row<'static>>, usize, usize) {
    let mut rows = Vec::new();
    let mut enabled_count = 0;
    let profile_entries = app.profile_entries();
    let mod_map = app.library.index_by_id();

    for (index, entry) in profile_entries.iter().enumerate() {
        let Some(mod_entry) = mod_map.get(&entry.id) else {
            continue;
        };
        if entry.enabled {
            enabled_count += 1;
        }
        rows.push(row_for_entry(index, entry.enabled, mod_entry, theme));
    }

    let total = rows.len();
    (rows, total, enabled_count)
}

fn row_for_entry(index: usize, enabled: bool, mod_entry: &ModEntry, theme: &Theme) -> Row<'static> {
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
    let (state_label, state_style) = mod_path_label(mod_entry, theme);
    Row::new(vec![
        Cell::from(enabled_text.to_string()).style(enabled_style),
        Cell::from((index + 1).to_string()),
        Cell::from(kind.to_string()).style(kind_style),
        Cell::from(state_label.to_string()).style(state_style),
        Cell::from(mod_entry.display_name()),
    ])
}

fn wrap_text(value: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let mut current = String::new();

    for word in value.split_whitespace() {
        let word_len = word.chars().count();
        if word_len > max_width {
            if !current.is_empty() {
                lines.push(current);
                current = String::new();
            }
            let mut chunk = String::new();
            let mut count = 0;
            for ch in word.chars() {
                if count == max_width {
                    lines.push(chunk);
                    chunk = String::new();
                    count = 0;
                }
                chunk.push(ch);
                count += 1;
            }
            if !chunk.is_empty() {
                lines.push(chunk);
            }
        } else {
            let current_len = current.chars().count();
            let next_len = if current.is_empty() {
                word_len
            } else {
                current_len + 1 + word_len
            };
            if next_len > max_width {
                lines.push(current);
                current = word.to_string();
            } else {
                if !current.is_empty() {
                    current.push(' ');
                }
                current.push_str(word);
            }
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    if lines.is_empty() && !value.is_empty() {
        let mut chunk = String::new();
        let mut count = 0;
        for ch in value.chars() {
            if count == max_width {
                lines.push(chunk);
                chunk = String::new();
                count = 0;
            }
            chunk.push(ch);
            count += 1;
        }
        if !chunk.is_empty() {
            lines.push(chunk);
        }
    }

    lines
}

fn push_wrapped_kv(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    value: &str,
    label_style: Style,
    value_style: Style,
    max_width: usize,
) {
    if max_width == 0 {
        return;
    }

    let label_text = format!("{label}: ");
    let label_len = label_text.len();

    if max_width <= label_len + 1 {
        lines.push(Line::from(Span::styled(label_text, label_style)));
        for part in wrap_text(value, max_width) {
            lines.push(Line::from(Span::styled(part, value_style)));
        }
        return;
    }

    let wrapped = wrap_text(value, max_width.saturating_sub(label_len));
    if wrapped.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(label_text, label_style),
            Span::styled(String::new(), value_style),
        ]));
        return;
    }

    lines.push(Line::from(vec![
        Span::styled(label_text.clone(), label_style),
        Span::styled(wrapped[0].clone(), value_style),
    ]));

    let indent = " ".repeat(label_len);
    for part in wrapped.iter().skip(1) {
        lines.push(Line::from(vec![
            Span::raw(indent.clone()),
            Span::styled(part.clone(), value_style),
        ]));
    }
}

fn push_wrapped_prefixed(
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
    let available = max_width.saturating_sub(prefix_len).max(1);
    let wrapped = wrap_text(value, available);
    if wrapped.is_empty() {
        lines.push(Line::from(Span::styled(prefix.to_string(), prefix_style)));
        return;
    }

    lines.push(Line::from(vec![
        Span::styled(prefix.to_string(), prefix_style),
        Span::styled(wrapped[0].clone(), value_style),
    ]));

    let indent = " ".repeat(prefix_len);
    for part in wrapped.iter().skip(1) {
        lines.push(Line::from(vec![
            Span::raw(indent.clone()),
            Span::styled(part.clone(), value_style),
        ]));
    }
}

fn build_details(app: &App, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    if app.focus == Focus::Explorer {
        return build_explorer_details(app, theme, width);
    }
    if app.focus == Focus::Conflicts {
        return build_conflict_details(app, theme, width);
    }

    let profile_entries = app.profile_entries();
    let mod_map = app.library.index_by_id();

    let Some(entry) = profile_entries.get(app.selected) else {
        return vec![Line::from("No mod selected.")];
    };
    let Some(mod_entry) = mod_map.get(&entry.id) else {
        return vec![Line::from("No mod selected.")];
    };

    let mut lines = Vec::new();
    let label_style = Style::default().fg(theme.muted);
    let value_style = Style::default().fg(theme.text);
    let display_name = mod_entry.display_name();
    push_wrapped_kv(
        &mut lines,
        "Name",
        &display_name,
        label_style,
        value_style,
        width,
    );
    if display_name != mod_entry.name {
        push_wrapped_kv(
            &mut lines,
            "Internal",
            &mod_entry.name,
            label_style,
            value_style,
            width,
        );
    }
    if let Some(source_label) = mod_entry.source_label() {
        if source_label != display_name {
            push_wrapped_kv(
                &mut lines,
                "Source",
                source_label,
                label_style,
                value_style,
                width,
            );
        }
    }
    let enabled_label = if entry.enabled { "Yes" } else { "No" };
    let enabled_style =
        Style::default().fg(if entry.enabled { theme.success } else { theme.muted });
    push_wrapped_kv(
        &mut lines,
        "Enabled",
        enabled_label,
        label_style,
        enabled_style,
        width,
    );
    let order_label = (app.selected + 1).to_string();
    push_wrapped_kv(&mut lines, "Order", &order_label, label_style, value_style, width);
    let type_label = mod_entry.display_type();
    push_wrapped_kv(
        &mut lines,
        "Type",
        &type_label,
        label_style,
        value_style,
        width,
    );
    let targets_label = targets_summary(mod_entry);
    push_wrapped_kv(
        &mut lines,
        "Targets",
        &targets_label,
        label_style,
        value_style,
        width,
    );
    let (path_label, path_style) = mod_path_label(mod_entry, theme);
    push_wrapped_kv(
        &mut lines,
        "Path",
        path_label,
        label_style,
        path_style,
        width,
    );
    let overrides_label = if mod_entry.target_overrides.is_empty() {
        "Auto"
    } else {
        "Custom"
    };
    push_wrapped_kv(
        &mut lines,
        "Overrides",
        overrides_label,
        label_style,
        value_style,
        width,
    );
    push_wrapped_kv(
        &mut lines,
        "ID",
        &mod_entry.id,
        label_style,
        value_style,
        width,
    );

    if let Some(info) = mod_entry.targets.iter().find_map(|target| match target {
        InstallTarget::Pak { info, .. } => Some(info),
        _ => None,
    }) {
        push_wrapped_kv(
            &mut lines,
            "Folder",
            &info.folder,
            label_style,
            value_style,
            width,
        );
        let version_label = info.version.to_string();
        push_wrapped_kv(
            &mut lines,
            "Version",
            &version_label,
            label_style,
            value_style,
            width,
        );
    }

    lines
}

fn build_explorer_details(app: &App, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let Some(item) = app.explorer_selected_item() else {
        return vec![Line::from("No selection.")];
    };

    match item.kind {
        ExplorerItemKind::Game(game_id) => {
            let mut lines = Vec::new();
            let label_style = Style::default().fg(theme.muted);
            let value_style = Style::default().fg(theme.text);
            push_wrapped_kv(
                &mut lines,
                "Game",
                game_id.display_name(),
                label_style,
                value_style,
                width,
            );
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
                push_wrapped_kv(
                    &mut lines,
                    "Root",
                    &root,
                    label_style,
                    value_style,
                    width,
                );
                push_wrapped_kv(
                    &mut lines,
                    "User dir",
                    &user_dir,
                    label_style,
                    value_style,
                    width,
                );
                let status_style =
                    Style::default().fg(if app.paths_ready() { theme.success } else { theme.warning });
                let status_label = if app.paths_ready() {
                    "Ready"
                } else {
                    "Setup required"
                };
                push_wrapped_kv(
                    &mut lines,
                    "Status",
                    status_label,
                    label_style,
                    status_style,
                    width,
                );
            } else {
                lines.push(Line::from(Span::styled(
                    "Select game to load profiles.",
                    Style::default().fg(theme.muted),
                )));
            }
            lines
        }
        ExplorerItemKind::Profile { name, .. } => {
            let mut lines = Vec::new();
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
            push_wrapped_kv(
                &mut lines,
                "Profile",
                &display_name,
                label_style,
                value_style,
                width,
            );
            if app.is_renaming_profile(&name) {
                lines.push(Line::from(Span::styled(
                    "Renaming...",
                    Style::default().fg(theme.warning),
                )));
            }
            if let Some(profile) = app
                .library
                .profiles
                .iter()
                .find(|profile| profile.name == name)
            {
                let enabled = profile.order.iter().filter(|entry| entry.enabled).count();
                let mods_label = profile.order.len().to_string();
                let enabled_label = enabled.to_string();
                push_wrapped_kv(
                    &mut lines,
                    "Mods",
                    &mods_label,
                    label_style,
                    value_style,
                    width,
                );
                let enabled_style = Style::default().fg(theme.success);
                push_wrapped_kv(
                    &mut lines,
                    "Enabled",
                    &enabled_label,
                    label_style,
                    enabled_style,
                    width,
                );
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
            "Profiles in this game.",
            Style::default().fg(theme.muted),
        ))],
        ExplorerItemKind::NewProfile(_) => vec![Line::from(Span::styled(
            "Press Enter to create a new profile.",
            Style::default().fg(theme.muted),
        ))],
        ExplorerItemKind::Info(_) => vec![Line::from(Span::styled(
            "Select the game to inspect profiles.",
            Style::default().fg(theme.muted),
        ))],
    }
}

fn build_conflict_details(app: &App, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    if app.conflicts_scanning() {
        return vec![Line::from(Span::styled(
            "Scanning conflicts...",
            Style::default().fg(theme.muted),
        ))];
    }
    if app.conflicts_pending() {
        return vec![Line::from(Span::styled(
            "Conflict scan queued...",
            Style::default().fg(theme.muted),
        ))];
    }
    let Some(conflict) = app.conflicts.get(app.conflict_selected) else {
        return vec![Line::from(Span::styled(
            "No conflicts detected.",
            Style::default().fg(theme.muted),
        ))];
    };

    let mut lines = Vec::new();
    let label_style = Style::default().fg(theme.muted);
    let value_style = Style::default().fg(theme.text);
    push_wrapped_kv(
        &mut lines,
        "Target",
        conflict_target_label(conflict.target),
        label_style,
        value_style,
        width,
    );
    let path_label = conflict.relative_path.to_string_lossy().to_string();
    push_wrapped_kv(
        &mut lines,
        "Path",
        &path_label,
        label_style,
        value_style,
        width,
    );
    let winner_style = Style::default().fg(theme.success);
    push_wrapped_kv(
        &mut lines,
        "Winner",
        &conflict.winner_name,
        label_style,
        winner_style,
        width,
    );
    if conflict.overridden {
        lines.push(Line::from(Span::styled(
            "Override active",
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
        push_wrapped_prefixed(&mut lines, &prefix, style, &candidate.mod_name, style, width);
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

fn mod_path_label(mod_entry: &ModEntry, theme: &Theme) -> (&'static str, Style) {
    if mod_entry.targets.is_empty() {
        return ("Not Valid", Style::default().fg(theme.warning));
    }

    ("Valid", Style::default().fg(theme.success))
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

fn conflict_target_label(target: TargetKind) -> &'static str {
    match target {
        TargetKind::Pak => "Pak",
        TargetKind::Generated => "Generated",
        TargetKind::Data => "Data",
        TargetKind::Bin => "Bin",
    }
}

fn conflict_short_label(target: TargetKind) -> &'static str {
    match target {
        TargetKind::Pak => "P",
        TargetKind::Generated => "G",
        TargetKind::Data => "D",
        TargetKind::Bin => "B",
    }
}

fn build_override_lines(app: &App, theme: &Theme) -> Vec<Line<'static>> {
    if app.focus == Focus::Explorer {
        return vec![
            Line::from(Span::styled(
                "Explorer actions",
                Style::default().fg(theme.accent),
            )),
            Line::from(Span::raw("Enter: select/expand")),
            Line::from(Span::raw("a: new profile")),
            Line::from(Span::raw("r/F2: rename profile")),
            Line::from(Span::raw("c: duplicate profile")),
            Line::from(Span::raw("e: export profile")),
            Line::from(Span::raw("p: import profile")),
            Line::from(Span::raw("Tab: cycle focus")),
        ];
    }
    if app.focus == Focus::Conflicts {
        return vec![
            Line::from(Span::styled(
                "Conflict actions",
                Style::default().fg(theme.accent),
            )),
            Line::from(Span::raw("Enter/Left/Right: cycle winner")),
            Line::from(Span::raw("Backspace: clear override")),
            Line::from(Span::raw("Up/Down: select conflict")),
            Line::from(Span::raw("Tab: cycle focus")),
        ];
    }

    let profile_entries = app.profile_entries();
    let mod_map = app.library.index_by_id();

    let Some(entry) = profile_entries.get(app.selected) else {
        return vec![Line::from("No mod selected.")];
    };
    let Some(mod_entry) = mod_map.get(&entry.id) else {
        return vec![Line::from("No mod selected.")];
    };

    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        "Toggle targets (1-4)",
        Style::default().fg(theme.accent),
    )));

    for (label, kind, key) in [
        ("Pak", TargetKind::Pak, '1'),
        ("Generated", TargetKind::Generated, '2'),
        ("Data", TargetKind::Data, '3'),
        ("Bin", TargetKind::Bin, '4'),
    ] {
        let line = if mod_entry.has_target_kind(kind) {
            let enabled = mod_entry.is_target_enabled(kind);
            let marker = if enabled { "[x]" } else { "[ ]" };
            let style = if enabled {
                Style::default().fg(theme.success)
            } else {
                Style::default().fg(theme.muted)
            };
            Line::from(vec![
                Span::styled(marker, style),
                Span::raw(format!(" {label} ({key})")),
            ])
        } else {
            Line::from(Span::styled(
                format!("- {label} ({key})"),
                Style::default().fg(theme.muted),
            ))
        };
        lines.push(line);
    }

    lines
}
