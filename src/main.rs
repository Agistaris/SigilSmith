mod app;
mod backup;
mod bg3;
mod config;
mod deploy;
mod game;
mod importer;
mod library;
mod metadata;
mod native_pak;
mod smart_rank;
mod update;
mod ui;

use anyhow::Result;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1).peekable();
    let mut import_paths = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--import" | "-i" => {
                if let Some(path) = args.next() {
                    import_paths.push(path);
                } else {
                    eprintln!("--import requires a path");
                }
            }
            "--help" | "-h" => {
                println!("SigilSmith");
                println!("  --import <path>   Import a mod archive/folder without the TUI");
                return Ok(());
            }
            _ => {}
        }
    }

    if !import_paths.is_empty() {
        let mut app = app::App::initialize()?;
        for path in import_paths {
            let count = app.import_mod_blocking(path.clone())?;
            if count == 0 {
                println!("No mods imported from {path}");
            } else {
                println!("Imported {count} mod(s) from {path}");
            }
        }
        return Ok(());
    }

    let mut app = app::App::initialize()?;
    ui::run(&mut app)
}
