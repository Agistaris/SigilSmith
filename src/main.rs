mod app;
mod backup;
mod bg3;
mod cli;
mod config;
mod deploy;
mod game;
mod importer;
mod launchers;
mod library;
mod localtime;
mod metadata;
mod native_pak;
mod repair;
mod script_extender;
mod sigillink;
mod smart_rank;
mod switch;
mod ui;
mod update;

use anyhow::Result;

fn main() -> Result<()> {
    cli::run()
}
