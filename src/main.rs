mod app;
mod backup;
mod bg3;
mod cli;
mod config;
mod deploy;
mod game;
mod importer;
mod library;
mod metadata;
mod native_pak;
mod sigillink;
mod smart_rank;
mod ui;
mod update;

use anyhow::Result;

fn main() -> Result<()> {
    cli::run()
}
