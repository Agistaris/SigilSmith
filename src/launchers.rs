//! BG3 Larian folders inside other launchers' Wine prefixes (Lutris, Heroic, Faugus, Bottles),
//! offered as suggestions when picking a folder. SigilSmith only reads these launchers'
//! settings: it never writes them or the prefixes' Wine settings.

use crate::bg3;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherFolder {
    pub launcher: &'static str,
    pub larian_dir: PathBuf,
}

pub fn larian_dirs(home: &Path) -> Vec<LauncherFolder> {
    let mut found: Vec<LauncherFolder> = Vec::new();
    for (launcher, prefix) in prefixes(home) {
        for larian_dir in larian_dirs_in_prefix(&prefix) {
            if !found
                .iter()
                .any(|item| same_dir(&item.larian_dir, &larian_dir))
            {
                found.push(LauncherFolder {
                    launcher,
                    larian_dir,
                });
            }
        }
    }
    found
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Wine prefixes the launchers know about, read from their settings and default folders.
fn prefixes(home: &Path) -> Vec<(&'static str, PathBuf)> {
    let mut out = Vec::new();

    for dir in [
        home.join(".local/share/lutris/games"),
        home.join(".config/lutris/games"),
        home.join(".var/app/net.lutris.Lutris/data/lutris/games"),
        home.join(".var/app/net.lutris.Lutris/config/lutris/games"),
    ] {
        for file in files_with_extension(&dir, "yml") {
            for prefix in lutris_prefixes(&file) {
                out.push(("Lutris", expand_home(home, &prefix)));
            }
        }
    }

    for root in [
        home.join(".config/heroic"),
        home.join(".var/app/com.heroicgameslauncher.hgl/config/heroic"),
    ] {
        let mut files = files_with_extension(&root.join("GamesConfig"), "json");
        files.push(root.join("config.json"));
        for file in files {
            for prefix in json_strings(&file, &["winePrefix", "defaultWinePrefix"]) {
                let prefix = expand_home(home, &prefix);
                // The default is a folder of per-game prefixes.
                if file.ends_with("config.json") {
                    out.extend(subdirs(&prefix).into_iter().map(|dir| ("Heroic", dir)));
                }
                out.push(("Heroic", prefix));
            }
        }
    }
    for dir in [
        home.join("Games/Heroic/Prefixes/default"),
        home.join("Games/Heroic/Prefixes"),
    ] {
        out.extend(subdirs(&dir).into_iter().map(|dir| ("Heroic", dir)));
    }

    for root in [
        home.join(".config/faugus-launcher"),
        home.join(".var/app/io.github.Faugus.faugus-launcher/config/faugus-launcher"),
    ] {
        for prefix in json_strings(&root.join("games.json"), &["prefix"]) {
            out.push(("Faugus", expand_home(home, &prefix)));
        }
        for default in json_strings(&root.join("config.json"), &["default-prefix"]) {
            let default = expand_home(home, &default);
            out.extend(subdirs(&default).into_iter().map(|dir| ("Faugus", dir)));
        }
    }
    out.extend(
        subdirs(&home.join("Faugus"))
            .into_iter()
            .map(|dir| ("Faugus", dir)),
    );

    for dir in [
        home.join(".local/share/bottles/bottles"),
        home.join(".var/app/com.usebottles.bottles/data/bottles/bottles"),
    ] {
        out.extend(subdirs(&dir).into_iter().map(|dir| ("Bottles", dir)));
    }

    out
}

/// `<prefix>/drive_c/users/<user>/AppData/Local/Larian Studios/Baldur's Gate 3`, also under
/// `pfx/` as Proton lays a prefix out.
fn larian_dirs_in_prefix(prefix: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for base in [prefix.to_path_buf(), prefix.join("pfx")] {
        for user in subdirs(&base.join("drive_c/users")) {
            let dir = user.join("AppData/Local/Larian Studios/Baldur's Gate 3");
            if bg3::looks_like_larian_dir(&dir) {
                found.push(dir);
            }
        }
    }
    found
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

fn files_with_extension(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == extension))
        .collect();
    files.sort();
    files
}

fn expand_home(home: &Path, value: &str) -> PathBuf {
    match value.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(value),
    }
}

/// `prefix:` values from a Lutris game file. Only that key is read, so a plain line scan is
/// enough and no YAML parser is needed.
fn lutris_prefixes(file: &Path) -> Vec<String> {
    let Ok(raw) = fs::read_to_string(file) else {
        return Vec::new();
    };
    raw.lines()
        .filter_map(|line| line.trim_start().strip_prefix("prefix:"))
        .map(|value| {
            value
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .to_string()
        })
        .filter(|value| !value.is_empty())
        .collect()
}

/// String values for any of `keys`, anywhere in a JSON file.
fn json_strings(file: &Path, keys: &[&str]) -> Vec<String> {
    let Some(value) = fs::read_to_string(file)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    collect_strings(&value, keys, &mut out);
    out
}

fn collect_strings(value: &Value, keys: &[&str], out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                match item {
                    Value::String(text) if keys.contains(&key.as_str()) && !text.is_empty() => {
                        out.push(text.clone());
                    }
                    _ => collect_strings(item, keys, out),
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_strings(item, keys, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn larian_in(prefix: &Path, user: &str) -> PathBuf {
        let dir = prefix
            .join("drive_c/users")
            .join(user)
            .join("AppData/Local/Larian Studios/Baldur's Gate 3");
        fs::create_dir_all(dir.join("PlayerProfiles/Public")).unwrap();
        dir
    }

    /// Fake settings for every launcher, each pointing at a prefix with a BG3 Larian folder.
    pub(crate) fn write_launcher_configs(home: &Path) -> Vec<PathBuf> {
        let lutris = home.join("Games/baldurs-gate-3");
        fs::create_dir_all(home.join(".local/share/lutris/games")).unwrap();
        fs::write(
            home.join(".local/share/lutris/games/baldurs-gate-3-1700000000.yml"),
            "game:\n  exe: drive_c/GOG Games/Baldurs Gate 3/bin/bg3.exe\n  prefix: ~/Games/baldurs-gate-3\nsystem: {}\n",
        )
        .unwrap();

        let heroic = home.join("Games/Heroic/Prefixes/default/Baldurs Gate 3");
        fs::create_dir_all(home.join(".config/heroic/GamesConfig")).unwrap();
        fs::write(
            home.join(".config/heroic/GamesConfig/1456460669.json"),
            format!(
                r#"{{"1456460669": {{"winePrefix": "{}", "wineVersion": {{"type": "proton"}}}}, "version": "v0"}}"#,
                heroic.display()
            ),
        )
        .unwrap();

        let faugus = home.join("Faugus/bg3");
        fs::create_dir_all(home.join(".config/faugus-launcher")).unwrap();
        fs::write(
            home.join(".config/faugus-launcher/games.json"),
            format!(
                r#"[{{"title": "Baldur's Gate 3", "prefix": "{}"}}]"#,
                faugus.display()
            ),
        )
        .unwrap();
        fs::write(
            home.join(".config/faugus-launcher/config.json"),
            format!(
                r#"{{"default-prefix": "{}"}}"#,
                home.join("Faugus").display()
            ),
        )
        .unwrap();

        let bottles = home.join(".local/share/bottles/bottles/Gaming");
        vec![
            larian_in(&lutris, "ryan"),
            larian_in(&heroic.join("pfx"), "steamuser"),
            larian_in(&faugus, "steamuser"),
            larian_in(&bottles, "ryan"),
        ]
    }

    #[test]
    fn finds_bg3_in_every_launchers_prefixes() {
        let home =
            std::env::temp_dir().join(format!("sigilsmith-launchers-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let expected = write_launcher_configs(&home);
        // A prefix without BG3 is not suggested.
        fs::create_dir_all(home.join("Faugus/other/drive_c/users/steamuser")).unwrap();

        let found = larian_dirs(&home);
        let pairs: Vec<(&str, &Path)> = found
            .iter()
            .map(|item| (item.launcher, item.larian_dir.as_path()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("Lutris", expected[0].as_path()),
                ("Heroic", expected[1].as_path()),
                ("Faugus", expected[2].as_path()),
                ("Bottles", expected[3].as_path()),
            ]
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn unreadable_settings_are_skipped() {
        let home =
            std::env::temp_dir().join(format!("sigilsmith-launchers-bad-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(home.join(".config/heroic/GamesConfig")).unwrap();
        fs::write(home.join(".config/heroic/GamesConfig/x.json"), "{not json").unwrap();
        fs::create_dir_all(home.join(".config/faugus-launcher")).unwrap();
        fs::write(home.join(".config/faugus-launcher/games.json"), "").unwrap();
        assert!(larian_dirs(&home).is_empty());
        let _ = fs::remove_dir_all(&home);
    }
}
