use crate::{
    app::{App, CliImportOptions, CliVerbosity, StartupMode},
    bg3::GamePaths,
    game,
    library::{library_mod_root, InstallTarget, Library, ModEntry, Profile},
    metadata, native_pak, ui,
};
use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Text,
    Json,
}

impl OutputFormat {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "json" => Some(OutputFormat::Json),
            "text" => Some(OutputFormat::Text),
            _ => None,
        }
    }
}

struct GlobalOptions {
    format: OutputFormat,
    profile: Option<String>,
}

enum CliAction {
    Ui,
    Import {
        paths: Vec<String>,
        options: CliImportOptions,
    },
    Command {
        command: CliCommand,
        format: OutputFormat,
        profile: Option<String>,
    },
}

enum CliCommand {
    ModsList(ModsListOptions),
    ProfilesList,
    DepsList,
    DepsMissing,
    DepsDebug(String),
    Paths,
    Help,
    Version,
}

struct ModsListOptions {
    sort: ModSortKey,
    reverse: bool,
    filter: Option<String>,
}

#[derive(Clone, Copy)]
enum ModSortKey {
    Order,
    Name,
    Created,
    Added,
    Kind,
}

pub fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let action = parse_args(&args)?;
    match action {
        CliAction::Ui => {
            let mut app = App::initialize(StartupMode::Ui)?;
            ui::run(&mut app)
        }
        CliAction::Import { paths, options } => {
            let mut app = App::initialize(StartupMode::Cli)?;
            app.import_mods_cli(paths, options)
        }
        CliAction::Command {
            command,
            format,
            profile,
        } => match command {
            CliCommand::Help => {
                print_help();
                Ok(())
            }
            CliCommand::Version => {
                println!("SigilSmith v{}", env!("CARGO_PKG_VERSION"));
                Ok(())
            }
            _ => {
                let mut app = App::initialize(StartupMode::Cli)?;
                run_command(&mut app, command, format, profile)
            }
        },
    }
}

fn parse_args(args: &[String]) -> Result<CliAction> {
    if args.is_empty() {
        return Ok(CliAction::Ui);
    }

    if matches!(args.first().map(|s| s.as_str()), Some("--help" | "-h" | "help")) {
        return Ok(CliAction::Command {
            command: CliCommand::Help,
            format: OutputFormat::Text,
            profile: None,
        });
    }
    if matches!(args.first().map(|s| s.as_str()), Some("--version" | "-V" | "version")) {
        return Ok(CliAction::Command {
            command: CliCommand::Version,
            format: OutputFormat::Text,
            profile: None,
        });
    }

    let (global, tokens) = parse_global_options(args);
    if let Some(action) = parse_subcommand(&tokens, &global)? {
        return Ok(action);
    }

    if let Some(action) = parse_legacy_import(args) {
        return Ok(action);
    }

    Ok(CliAction::Command {
        command: CliCommand::Help,
        format: OutputFormat::Text,
        profile: None,
    })
}

fn parse_global_options(args: &[String]) -> (GlobalOptions, Vec<String>) {
    let mut format = OutputFormat::Text;
    let mut profile = None;
    let mut tokens = Vec::new();
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--format=") {
            if let Some(parsed) = OutputFormat::parse(value) {
                format = parsed;
            }
            continue;
        }
        if arg == "--format" {
            if let Some(value) = iter.next() {
                if let Some(parsed) = OutputFormat::parse(value) {
                    format = parsed;
                }
            }
            continue;
        }
        if let Some(value) = arg.strip_prefix("--profile=") {
            profile = Some(value.to_string());
            continue;
        }
        if arg == "--profile" {
            if let Some(value) = iter.next() {
                profile = Some(value.to_string());
            }
            continue;
        }
        tokens.push(arg.to_string());
    }

    (GlobalOptions { format, profile }, tokens)
}

fn parse_subcommand(tokens: &[String], global: &GlobalOptions) -> Result<Option<CliAction>> {
    let Some(head) = tokens.first() else {
        return Ok(None);
    };
    match head.as_str() {
        "mods" => {
            let options = parse_mods_list(tokens.get(1..).unwrap_or(&[]))?;
            Ok(Some(CliAction::Command {
                command: CliCommand::ModsList(options),
                format: global.format,
                profile: global.profile.clone(),
            }))
        }
        "profiles" => Ok(Some(CliAction::Command {
            command: CliCommand::ProfilesList,
            format: global.format,
            profile: global.profile.clone(),
        })),
        "deps" => {
            let sub = tokens.get(1).map(|value| value.as_str()).unwrap_or("missing");
            let command = match sub {
                "list" => CliCommand::DepsList,
                "missing" => CliCommand::DepsMissing,
                "debug" => {
                    let query = tokens.get(2).ok_or_else(|| {
                        anyhow::anyhow!("deps debug requires a mod id or name")
                    })?;
                    CliCommand::DepsDebug(query.to_string())
                }
                _ => {
                    bail!("Unknown deps command: {sub} (use 'list', 'missing', or 'debug')");
                }
            };
            Ok(Some(CliAction::Command {
                command,
                format: global.format,
                profile: global.profile.clone(),
            }))
        }
        "paths" => Ok(Some(CliAction::Command {
            command: CliCommand::Paths,
            format: global.format,
            profile: global.profile.clone(),
        })),
        _ => Ok(None),
    }
}

fn parse_mods_list(args: &[String]) -> Result<ModsListOptions> {
    let mut sort = ModSortKey::Order;
    let mut reverse = false;
    let mut filter = None;
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "list" => {}
            "--sort" => {
                if let Some(value) = iter.next() {
                    sort = parse_sort_key(value)?;
                } else {
                    bail!("--sort requires a value");
                }
            }
            value if value.starts_with("--sort=") => {
                let key = value.trim_start_matches("--sort=");
                sort = parse_sort_key(key)?;
            }
            "--reverse" | "-r" => reverse = true,
            "--filter" => {
                if let Some(value) = iter.next() {
                    filter = Some(value.to_string());
                } else {
                    bail!("--filter requires a value");
                }
            }
            value if value.starts_with("--filter=") => {
                filter = Some(value.trim_start_matches("--filter=").to_string());
            }
            _ => {}
        }
    }

    Ok(ModsListOptions {
        sort,
        reverse,
        filter,
    })
}

fn parse_sort_key(value: &str) -> Result<ModSortKey> {
    match value {
        "order" => Ok(ModSortKey::Order),
        "name" => Ok(ModSortKey::Name),
        "created" => Ok(ModSortKey::Created),
        "added" => Ok(ModSortKey::Added),
        "kind" => Ok(ModSortKey::Kind),
        _ => bail!("Unknown sort key: {value}"),
    }
}

fn parse_legacy_import(args: &[String]) -> Option<CliAction> {
    let mut import_paths = Vec::new();
    let mut deploy = None;
    let mut verbosity = CliVerbosity::Normal;
    let mut stop_parsing = false;
    let mut iter = args.iter().peekable();

    while let Some(arg) = iter.next() {
        if stop_parsing {
            import_paths.push(arg.to_string());
            continue;
        }

        match arg.as_str() {
            "--" => stop_parsing = true,
            "--import" | "-i" => {
                let mut pushed = false;
                while let Some(next) = iter.peek() {
                    if next.as_str() == "--" || next.starts_with('-') {
                        break;
                    }
                    if let Some(path) = iter.next() {
                        import_paths.push(path.to_string());
                        pushed = true;
                    }
                }
                if !pushed {
                    eprintln!("--import requires one or more paths");
                }
            }
            "--deploy" => deploy = Some(true),
            "--no-deploy" => deploy = Some(false),
            "-q" | "--quiet" => verbosity = CliVerbosity::Quiet,
            "--verbose" => verbosity = CliVerbosity::Verbose,
            "--verbosity" => {
                if let Some(level) = iter.next() {
                    verbosity = match level.as_str() {
                        "quiet" | "minimal" => CliVerbosity::Quiet,
                        "normal" | "info" => CliVerbosity::Normal,
                        "verbose" => CliVerbosity::Verbose,
                        "debug" | "trace" => CliVerbosity::Debug,
                        _ => {
                            eprintln!("Unknown verbosity: {level}");
                            CliVerbosity::Normal
                        }
                    };
                } else {
                    eprintln!("--verbosity requires a level");
                }
            }
            _ if arg.starts_with("-v") && !arg.starts_with("--") => {
                let count = arg.chars().filter(|ch| *ch == 'v').count();
                verbosity = if count >= 2 {
                    CliVerbosity::Debug
                } else {
                    CliVerbosity::Verbose
                };
            }
            _ => {}
        }
    }

    if import_paths.is_empty() {
        return None;
    }

    Some(CliAction::Import {
        paths: import_paths,
        options: CliImportOptions {
            deploy: deploy.unwrap_or(false),
            verbosity,
        },
    })
}

fn run_command(
    app: &mut App,
    command: CliCommand,
    format: OutputFormat,
    profile: Option<String>,
) -> Result<()> {
    match command {
        CliCommand::ModsList(options) => {
            let profile = resolve_profile(&app.library, profile.as_deref())?;
            list_mods(app, profile, options, format)
        }
        CliCommand::ProfilesList => list_profiles(&app.library, format),
        CliCommand::DepsList => {
            let profile = resolve_profile(&app.library, profile.as_deref())?;
            list_dependencies(app, profile, format)
        }
        CliCommand::DepsMissing => {
            let profile = resolve_profile(&app.library, profile.as_deref())?;
            list_missing_dependencies(app, profile, format)
        }
        CliCommand::DepsDebug(query) => debug_dependencies(app, &query),
        CliCommand::Paths => list_paths(app, format),
        CliCommand::Help | CliCommand::Version => Ok(()),
    }
}

fn resolve_profile<'a>(library: &'a Library, override_name: Option<&str>) -> Result<&'a Profile> {
    if let Some(name) = override_name {
        return library
            .profiles
            .iter()
            .find(|profile| profile.name == name)
            .ok_or_else(|| anyhow::anyhow!("Unknown profile: {name}"));
    }
    library
        .active_profile()
        .ok_or_else(|| anyhow::anyhow!("No active profile"))
}

#[derive(Serialize)]
struct ModListItem {
    id: String,
    name: String,
    display_name: String,
    kind: String,
    created_at: Option<i64>,
    added_at: i64,
    enabled: bool,
    order: Option<usize>,
}

fn list_mods(
    app: &App,
    profile: &Profile,
    options: ModsListOptions,
    format: OutputFormat,
) -> Result<()> {
    let mut order_map = HashMap::new();
    for (index, entry) in profile.order.iter().enumerate() {
        order_map.insert(entry.id.clone(), (index + 1, entry.enabled));
    }

    let mut items: Vec<ModListItem> = app
        .library
        .mods
        .iter()
        .map(|mod_entry| {
            let (order, enabled) = order_map
                .get(&mod_entry.id)
                .copied()
                .unwrap_or((0, false));
            ModListItem {
                id: mod_entry.id.clone(),
                name: mod_entry.name.clone(),
                display_name: mod_entry.display_name(),
                kind: mod_entry.display_type(),
                created_at: mod_entry.created_at,
                added_at: mod_entry.added_at,
                enabled,
                order: if order == 0 { None } else { Some(order) },
            }
        })
        .collect();

    if let Some(filter) = &options.filter {
        let needle = filter.to_ascii_lowercase();
        items.retain(|item| item.display_name.to_ascii_lowercase().contains(&needle));
    }

    match options.sort {
        ModSortKey::Order => items.sort_by_key(|item| item.order.unwrap_or(usize::MAX)),
        ModSortKey::Name => items.sort_by(|a, b| a.display_name.cmp(&b.display_name)),
        ModSortKey::Created => items.sort_by_key(|item| item.created_at.unwrap_or(0)),
        ModSortKey::Added => items.sort_by_key(|item| item.added_at),
        ModSortKey::Kind => items.sort_by(|a, b| a.kind.cmp(&b.kind)),
    }

    if options.reverse {
        items.reverse();
    }

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&items)?);
        }
        OutputFormat::Text => {
            for item in items {
                let order = item
                    .order
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string());
                let enabled = if item.enabled { "x" } else { " " };
                let created = format_date_cell(item.created_at);
                let added = format_date_cell(Some(item.added_at));
                println!(
                    "{order:>3} [{enabled}] {kind:<10} {created} {added} {name}",
                    kind = item.kind,
                    name = item.display_name
                );
            }
        }
    }

    Ok(())
}

#[derive(Serialize)]
struct ProfileListItem {
    name: String,
    active: bool,
}

fn list_profiles(library: &Library, format: OutputFormat) -> Result<()> {
    let items: Vec<ProfileListItem> = library
        .profiles
        .iter()
        .map(|profile| ProfileListItem {
            name: profile.name.clone(),
            active: profile.name == library.active_profile,
        })
        .collect();

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&items)?);
        }
        OutputFormat::Text => {
            for item in items {
                if item.active {
                    println!("* {}", item.name);
                } else {
                    println!("  {}", item.name);
                }
            }
        }
    }

    Ok(())
}

#[derive(Serialize)]
struct MissingDependencyItem {
    required_by: String,
    required_by_id: String,
    dependency: String,
    dependency_id: String,
    reason: String,
}

#[derive(Serialize)]
struct DependencyListItem {
    mod_name: String,
    mod_id: String,
    dependencies: Vec<DependencyRef>,
}

#[derive(Serialize)]
struct DependencyRef {
    id: String,
    name: Option<String>,
    enabled: bool,
}

fn list_dependencies(app: &App, profile: &Profile, format: OutputFormat) -> Result<()> {
    let mod_map = app.library.index_by_id();
    let enabled_ids: HashSet<&String> = profile
        .order
        .iter()
        .filter(|entry| entry.enabled)
        .map(|entry| &entry.id)
        .collect();
    let paths = game::detect_paths(
        app.game_id,
        Some(&app.config.game_root),
        Some(&app.config.larian_dir),
    )
    .ok();

    let mut list = Vec::new();
    for mod_entry in &app.library.mods {
        let deps = collect_dependencies(app, mod_entry, paths.as_ref());
        if deps.is_empty() {
            continue;
        }
        let mut refs = Vec::new();
        for dep_id in deps {
            let dep = mod_map.get(&dep_id);
            let name = dep.map(|entry| entry.display_name());
            let enabled = dep
                .map(|entry| enabled_ids.contains(&entry.id))
                .unwrap_or(false);
            refs.push(DependencyRef {
                id: dep_id,
                name,
                enabled,
            });
        }
        list.push(DependencyListItem {
            mod_name: mod_entry.display_name(),
            mod_id: mod_entry.id.clone(),
            dependencies: refs,
        });
    }

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&list)?);
        }
        OutputFormat::Text => {
            if list.is_empty() {
                println!("No dependencies detected.");
            } else {
                for item in list {
                    println!("{} ({})", item.mod_name, item.mod_id);
                    for dep in item.dependencies {
                        let name = dep.name.unwrap_or_else(|| "Unknown".to_string());
                        let status = if dep.enabled { "enabled" } else { "disabled" };
                        println!("  -> {} ({}) {}", dep.id, name, status);
                    }
                }
            }
        }
    }

    Ok(())
}

fn list_missing_dependencies(app: &App, profile: &Profile, format: OutputFormat) -> Result<()> {
    let mod_map = app.library.index_by_id();
    let enabled_ids: HashSet<&String> = profile
        .order
        .iter()
        .filter(|entry| entry.enabled)
        .map(|entry| &entry.id)
        .collect();
    let paths = game::detect_paths(
        app.game_id,
        Some(&app.config.game_root),
        Some(&app.config.larian_dir),
    )
    .ok();

    let mut missing = Vec::new();
    for mod_id in &enabled_ids {
        let Some(mod_entry) = mod_map.get(*mod_id) else {
            continue;
        };
        let deps = collect_dependencies(app, mod_entry, paths.as_ref());
        for dep_id in deps {
            let dependency = mod_map.get(&dep_id);
            let (dependency_name, reason) = match dependency {
                Some(dep_mod) => {
                    if enabled_ids.contains(&dep_mod.id) {
                        continue;
                    }
                    (dep_mod.display_name(), "disabled".to_string())
                }
                None => ("Unknown".to_string(), "not installed".to_string()),
            };
            missing.push(MissingDependencyItem {
                required_by: mod_entry.display_name(),
                required_by_id: mod_entry.id.clone(),
                dependency: dependency_name,
                dependency_id: dep_id,
                reason,
            });
        }
    }

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&missing)?);
        }
        OutputFormat::Text => {
            if missing.is_empty() {
                println!("No missing dependencies detected.");
            } else {
                for item in missing {
                    println!(
                        "{} -> {} ({})",
                        item.required_by, item.dependency_id, item.reason
                    );
                }
            }
        }
    }

    Ok(())
}

fn debug_dependencies(app: &App, query: &str) -> Result<()> {
    println!("{}", app.debug_dependency_report(query));
    Ok(())
}

fn collect_dependencies(
    app: &App,
    mod_entry: &ModEntry,
    paths: Option<&GamePaths>,
) -> Vec<String> {
    let mut out = Vec::new();
    let mod_root = library_mod_root(&app.config.data_dir).join(&mod_entry.id);
    let base_dir = if mod_entry.is_native() {
        paths
            .map(|paths| paths.larian_mods_dir.clone())
            .unwrap_or_else(|| mod_root.clone())
    } else {
        mod_root.clone()
    };
    let native_index = if mod_entry.is_native() {
        paths.map(|paths| native_pak::build_native_pak_index(&paths.larian_mods_dir))
    } else {
        None
    };

    for target in &mod_entry.targets {
        let InstallTarget::Pak { file, .. } = target else {
            continue;
        };
        let mut pak_path = base_dir.join(file);
        if mod_entry.is_native() && !pak_path.exists() {
            if let Some(info) = mod_entry.targets.iter().find_map(|target| {
                if let InstallTarget::Pak { info, .. } = target {
                    Some(info)
                } else {
                    None
                }
            }) {
                if let Some(index) = native_index.as_deref() {
                    if let Some(resolved) = native_pak::resolve_native_pak_path(info, index) {
                        pak_path = resolved;
                    }
                }
            }
        }
        if let Some(meta) = metadata::read_meta_lsx_from_pak(&pak_path) {
            out.extend(meta.dependencies);
        }
    }

    if out.is_empty() && mod_root.exists() {
        if let Some(meta_path) = metadata::find_meta_lsx(&mod_root) {
            if let Some(meta) = metadata::read_meta_lsx(&meta_path) {
                out.extend(meta.dependencies);
            }
        }
    }

    out.sort();
    out.dedup();
    out.retain(|dep| !dep.eq_ignore_ascii_case(&mod_entry.id));
    out
}

#[derive(Serialize)]
struct PathsOutput {
    game_root: String,
    data_dir: String,
    larian_dir: String,
    larian_mods_dir: String,
    modsettings_path: String,
    error: Option<String>,
}

fn list_paths(app: &App, format: OutputFormat) -> Result<()> {
    let detected = game::detect_paths(
        app.game_id,
        Some(&app.config.game_root),
        Some(&app.config.larian_dir),
    );
    let (paths, error) = match detected {
        Ok(paths) => (paths, None),
        Err(err) => (
            GamePaths {
                game_root: app.config.game_root.clone(),
                data_dir: app.config.game_root.join("Data"),
                larian_dir: app.config.larian_dir.clone(),
                larian_mods_dir: app.config.larian_dir.join("Mods"),
                modsettings_path: app
                    .config
                    .larian_dir
                    .join("PlayerProfiles")
                    .join("Public")
                    .join("modsettings.lsx"),
                profiles_dir: app.config.larian_dir.join("PlayerProfiles"),
            },
            Some(err.to_string()),
        ),
    };

    let output = PathsOutput {
        game_root: paths.game_root.display().to_string(),
        data_dir: paths.data_dir.display().to_string(),
        larian_dir: paths.larian_dir.display().to_string(),
        larian_mods_dir: paths.larian_mods_dir.display().to_string(),
        modsettings_path: paths.modsettings_path.display().to_string(),
        error,
    };

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Text => {
            println!("Game root: {}", output.game_root);
            println!("Data dir: {}", output.data_dir);
            println!("Larian dir: {}", output.larian_dir);
            println!("Larian mods: {}", output.larian_mods_dir);
            println!("Modsettings: {}", output.modsettings_path);
            if let Some(error) = output.error {
                println!("Warning: {error}");
            }
        }
    }

    Ok(())
}

fn print_help() {
    println!("SigilSmith v{}", env!("CARGO_PKG_VERSION"));
    println!("Usage:");
    println!("  sigilsmith                     Launch TUI");
    println!("  sigilsmith mods list            List mods");
    println!("  sigilsmith profiles list        List profiles");
    println!("  sigilsmith deps list            List dependencies for installed mods");
    println!("  sigilsmith deps missing         List missing dependencies");
    println!("  sigilsmith deps debug <mod>     Show dependency matching details");
    println!("  sigilsmith paths                Show detected paths");
    println!("  sigilsmith --import <paths...>  Import mods without the TUI");
    println!();
    println!("Global options:");
    println!("  --format <json|text>            Output format for list commands");
    println!("  --profile <name>                Profile name for list commands");
    println!("  -h, --help                      Show help");
    println!("  -V, --version                   Show version");
    println!();
    println!("Import options:");
    println!("  --deploy                         Deploy after import");
    println!("  --no-deploy                      Skip deploy after import (default)");
    println!("  -q, --quiet                      Errors only");
    println!("  -v, -vv, -vvv                    Increase verbosity");
    println!("  --verbosity <level>              quiet | normal | verbose | debug");
    println!("  --verbose                        Alias for --verbosity verbose");
}

fn format_date_cell(value: Option<i64>) -> String {
    if let Some(value) = value {
        if let Some(formatted) = format_short_date(value) {
            return formatted;
        }
    }
    format_blank_date()
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

fn format_blank_date() -> String {
    let locale = locale_hint();
    if prefers_ymd(&locale) {
        "---- -- --".to_string()
    } else {
        "-- -- ----".to_string()
    }
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
