use crate::game::{self, GameId};
use anyhow::{Context, Result};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub active_game: GameId,
    #[serde(default = "default_true")]
    pub confirm_profile_delete: bool,
    #[serde(default = "default_true")]
    pub confirm_mod_delete: bool,
}

impl AppConfig {
    pub fn load_or_create() -> Result<Self> {
        let base_dir = base_data_dir()?;
        fs::create_dir_all(&base_dir).context("create app data dir")?;
        let path = base_dir.join("config.json");
        if path.exists() {
            let raw = fs::read_to_string(&path).context("read app config")?;
            let mut config: AppConfig = serde_json::from_str(&raw).context("parse app config")?;
            if !game::supported_games().contains(&config.active_game) {
                config.active_game = GameId::default();
                config.save()?;
            }
            return Ok(config);
        }

        let config = AppConfig {
            active_game: GameId::default(),
            confirm_profile_delete: true,
            confirm_mod_delete: true,
        };
        config.save()?;
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let base_dir = base_data_dir()?;
        fs::create_dir_all(&base_dir).context("create app data dir")?;
        let path = base_dir.join("config.json");
        let raw = serde_json::to_string_pretty(self).context("serialize app config")?;
        fs::write(path, raw).context("write app config")?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameConfig {
    #[serde(default)]
    pub game_id: GameId,
    pub game_name: String,
    pub data_dir: PathBuf,
    pub game_root: PathBuf,
    pub larian_dir: PathBuf,
    pub active_profile: String,
}

impl GameConfig {
    pub fn load_or_create(game: GameId) -> Result<Self> {
        let data_dir = data_dir_for_game(game)?;
        fs::create_dir_all(&data_dir).context("create data dir")?;

        let config_path = data_dir.join("config.json");
        if config_path.exists() {
            let raw = fs::read_to_string(&config_path).context("read config")?;
            let mut config: GameConfig = serde_json::from_str(&raw).context("parse config")?;
            config.game_id = game;
            config.game_name = game.display_name().to_string();
            config.data_dir = data_dir;
            config.save()?;
            return Ok(config);
        }

        let (game_root, larian_dir) = match game::detect_paths(game, None, None) {
            Ok(paths) => (paths.game_root, paths.larian_dir),
            Err(_) => (PathBuf::new(), PathBuf::new()),
        };

        let config = GameConfig {
            game_id: game,
            game_name: game.display_name().to_string(),
            data_dir,
            game_root,
            larian_dir,
            active_profile: "Default".to_string(),
        };

        config.save()?;
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let config_path = self.data_dir.join("config.json");
        let raw = serde_json::to_string_pretty(self).context("serialize config")?;
        fs::write(config_path, raw).context("write config")?;
        Ok(())
    }
}

pub fn data_dir_for_game(game: GameId) -> Result<PathBuf> {
    let base = base_data_dir()?;
    Ok(base.join(game.data_dir_name()))
}

fn default_true() -> bool {
    true
}

fn base_data_dir() -> Result<PathBuf> {
    let base = BaseDirs::new().context("resolve home dir")?;
    Ok(base.data_local_dir().join("sigilsmith"))
}
