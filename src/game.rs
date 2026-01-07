use crate::bg3;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameId {
    Bg3,
}

impl Default for GameId {
    fn default() -> Self {
        GameId::Bg3
    }
}

impl GameId {
    pub fn display_name(self) -> &'static str {
        match self {
            GameId::Bg3 => bg3::GAME_NAME,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            GameId::Bg3 => "bg3",
        }
    }

    pub fn data_dir_name(self) -> &'static str {
        match self {
            GameId::Bg3 => bg3::GAME_NAME,
        }
    }

    // Setup prompts are handled by the path browser UI.
}

pub fn supported_games() -> Vec<GameId> {
    vec![GameId::Bg3]
}

pub fn detect_paths(
    game: GameId,
    game_root_override: Option<&Path>,
    user_dir_override: Option<&Path>,
) -> Result<bg3::GamePaths> {
    match game {
        GameId::Bg3 => bg3::detect_paths(game_root_override, user_dir_override),
    }
}

pub fn looks_like_game_root(game: GameId, path: &Path) -> bool {
    match game {
        GameId::Bg3 => bg3::looks_like_game_root(path),
    }
}

pub fn looks_like_user_dir(game: GameId, path: &Path) -> bool {
    match game {
        GameId::Bg3 => bg3::looks_like_larian_dir(path),
    }
}
