# File formats

The files SigilSmith reads and writes, so you can back them up, script
against them or move to another tool. This page describes SigilSmith 0.9.11.

| File | Status |
| --- | --- |
| [Mod list export (SigilSmith JSON)](#mod-list-export-sigilsmith-json) | **Stable.** The format for sharing and moving a load order. |
| [`modsettings.lsx`](#modsettingslsx) | BG3's own format. What SigilSmith writes and reads is listed below. |
| [`library.json`](#libraryjson), [`config.json`](#configjson), [`deploy_manifest.json`](#deploy_manifestjson), [backups](#backups) | Documented for reading and recovery. They may gain fields; fields marked *internal* may change without notice. |

All of them are JSON or XML in UTF-8. Times are Unix seconds unless noted.
Unknown fields are ignored when SigilSmith reads a file and dropped the next
time it saves it. If you edit these files by hand, close SigilSmith first.

Where the files are: [What SigilSmith touches](what-sigilsmith-touches.md).

## Mod list export (SigilSmith JSON)

Written by **Export mod list** (`e` in the Explorer, or `Ctrl+E`) to
`~/Downloads/SigilSmith/exports/modlist-bg3-<profile>-<YYYYMMDD-HHMMSS>.json`.
It carries one profile's order, on/off state and override choices.

```json
{
  "schema_version": 1,
  "exported_at": "2026-10-17T14:02:11Z",
  "sigilsmith_version": "0.9.11",
  "game_id": "bg3",
  "game_name": "Baldur's Gate 3",
  "profile_name": "Default",
  "entries": [
    { "id": "11111111-2222-4333-8444-555555555555", "name": "Example Settings Menu", "enabled": true },
    { "id": "loose-6c1f0e…", "name": "Example Dyes", "enabled": false }
  ],
  "file_overrides": [
    { "kind": "data", "relative_path": "Public/Shared/Assets/x.dds", "mod_id": "loose-6c1f0e…" }
  ]
}
```

| Field | Type | Notes |
| --- | --- | --- |
| `schema_version` | number | `1`. Optional on import. |
| `exported_at` | string | UTC, `YYYY-MM-DDTHH:MM:SSZ`. Optional. |
| `sigilsmith_version` | string | The version that wrote it. Optional. |
| `game_id` | string | `"bg3"`. Importing a list for another game gives a warning. |
| `game_name` | string | `"Baldur's Gate 3"` |
| `profile_name` | string | The profile it came from; used to name the new profile on import. |
| `entries` | array | The load order, first to last. Each entry has `id`, `name` and `enabled`. |
| `file_overrides` | array | Optional. Your Overrides choices: `kind` (`generated`, `data` or `bin`), `relative_path` inside that folder, and the winning `mod_id`. |

**Mod IDs.** A `.pak` mod's ID is its module UUID from `meta.lsx`, the same on
every machine. Loose mods and override paks get `loose-<hash>` and
`pak-<hash>` IDs that differ between machines, so on import those are matched
by name.

**How import matches entries:** by `id` first, then by name (case-insensitive,
against the mod's display name and its own name), then by a simplified name
(letters and digits only). If a name matches more than one mod, the preview
marks it as ambiguous and the list can't be applied; giving that entry the
mod's `id` fixes it. Entries that match nothing become
placeholders in the list, so the order stays intact until you add the mod.
Override `mod_id`s are used as they are.

**Import options:** into a new profile (the default) or the active one, and
**Merge** (the default: keeps your other override choices) or **Strict**
(replaces them, and turns off mods that aren't in the list). Every import
shows a preview first.

The importer reads a file as SigilSmith JSON when its content starts with `{`,
and as `modsettings.lsx` when the name ends in `.lsx` or the content starts
with `<`. You can also paste either one.

## modsettings.lsx

BG3's load order file, `PlayerProfiles/Public/modsettings.lsx` in the Larian
data folder. SigilSmith rewrites it on every deploy and can export a copy
(`modsettings-bg3-<profile>-<time>.lsx`).

**What SigilSmith writes:**

- It starts from the existing file and keeps its version and any other nodes.
  If there's no file, it writes version 4.8.0.500.
- The `Mods` node gets the base game's entries, then every **enabled** `.pak`
  mod in your order, each as a `ModuleShortDesc` with `Folder`, `MD5`, `Name`,
  `PublishHandle`, `UUID` and `Version64`.
- The same order goes into `ModOrder` (as `Module` nodes with a `UUID`).
- Turned-off mods aren't written, so this file can't record them. Loose mods
  and override paks never appear in it (they're files in the game folder,
  not modules).

**What SigilSmith reads** (at start, and on import):

- The order comes from the `Mods` list, or from `ModOrder` if `Mods` is
  missing.
- From each module: `UUID`, `Name`, `Folder`, `Version64`, `PublishHandle`,
  `MD5`, `Created` (or `CreatedOn`) and `Enabled` (`1` or `true`). Without an
  `Enabled` attribute, every mod listed is treated as on.

## library.json

`~/.local/share/sigilsmith/Baldur's Gate 3/library.json`: your mods, profiles
and choices. A copy is in every backup.

Top level:

| Field | Notes |
| --- | --- |
| `mods` | Every mod in the library (see below) |
| `profiles` | Your profiles (see below) |
| `active_profile` | The name of the active profile |
| `renamed_ids` | Old ID → new ID, for mods whose ID SigilSmith corrected. Left out when empty. |
| `modsettings_sync_enabled`, `dependency_blocks`, `metadata_cache_version`, `metadata_cache_key`, `modsettings_hash`, `repair_version` | *Internal* |

Each mod:

| Field | Notes |
| --- | --- |
| `id` | The mod's ID (see [Mod IDs](#mod-list-export-sigilsmith-json)) |
| `name` | The mod's name |
| `added_at`, `created_at`, `modified_at` | When it was added to SigilSmith, and the mod's own dates if known |
| `targets` | What the mod installs, each with a `kind`: `{"kind": "pak", "file": …, "info": {…}}` for a `.pak`, or `{"kind": "generated" \| "data" \| "bin", "dir": …}` for loose files |
| `target_overrides` | Targets you turned off for this mod: `{ "kind", "enabled" }`. A target without an entry is on. |
| `source` | `"managed"` (imported into SigilSmith) or `"native"` (installed by the game's own mod manager) |
| `source_label` | What it was imported from |
| `dependencies` | The mods it needs, as UUIDs or names |
| `scripts` | Left out when empty. `{ "script_extender": { "required_version", "features" }, "osiris": true }` |

A `.pak` target's `info` has `uuid`, `name`, `folder`, `version`, and, when
the mod's `meta.lsx` has them, `md5`, `publish_handle`, `author`,
`description` and `module_type`.

Each profile:

| Field | Notes |
| --- | --- |
| `name` | The profile's name |
| `order` | The load order, first to last: `{ "id", "enabled", "missing_label" }`. `missing_label` marks a placeholder for a mod that isn't in the library. |
| `file_overrides` | Your Overrides choices, as in the export. Only choices that differ from the default (the last mod in the order wins) are stored. |
| `sigillink_pins`, `sigillink_meta` | *Internal* (SigiLink ranking) |

A profile named `__sigillink_ranking__` is *internal*; SigilSmith hides it.

## config.json

**App settings,** `~/.local/share/sigilsmith/config.json`:

| Field | Values |
| --- | --- |
| `active_game` | `"bg3"` |
| `update_mode` | `"ask"` (default), `"automatic"` or `"off"` |
| `skipped_update_version` | The version you chose to skip, if any |
| `downloads_dir` | Where exports go (default: your Downloads folder) |
| `auto_deploy_enabled`, `enable_mods_after_import`, `confirm_profile_delete`, `confirm_mod_delete`, `delete_mod_files_on_remove`, `warn_missing_dependencies`, `offer_dependency_downloads`, `show_startup_dependency_notice`, `show_script_extender_notice` | Settings toggles (`true` or `false`) |
| `sigillink_ranking_enabled`, `sigillink_auto_preview`, `sigillink_onboarded`, `sigillink_pin_notice_dismissed` | SigiLink settings |
| `last_browser_dir`, `last_whats_new_version`, `default_sort_column`, `dependency_search_copy_preference` | Remembered choices |

**Game settings,** `~/.local/share/sigilsmith/Baldur's Gate 3/config.json`:

| Field | Values |
| --- | --- |
| `game_root` | The BG3 install folder |
| `larian_dir` | The Larian data folder (with `Mods/` and `PlayerProfiles/`) |
| `active_profile` | The active profile's name |
| `sigillink_cache_dir` | Where the mod store is, if you moved it |
| `declined_move` | A Larian folder you chose not to move to ("Don't ask again for this folder") |
| `keep_links_in` | Larian folders where you chose to keep SigilSmith's leftover links, so it stops offering to remove them |
| `game_id`, `game_name`, `data_dir` | Set by SigilSmith at every start |

## deploy_manifest.json

`~/.local/share/sigilsmith/Baldur's Gate 3/deploy_manifest.json`: what the
last deploy put in the game, so the next one only removes SigilSmith's own
files.

| Field | Notes |
| --- | --- |
| `files` | Loose files: `{ "target", "path", "source_mod", "source_id", "source_kind" }` |
| `pak_files` | The `.pak` links in the Larian `Mods/` folder |
| `links` | Destination path → `{ "source", "mode": "hardlink" \| "symlink", "dev", "ino", "size" }` (`dev` and `ino` for hardlinks only). A file counts as SigilSmith's only while it still matches this record. |
| `set_aside` | Files moved out of the way: `{ "original", "saved" }` |

## Backups

Each deploy saves a backup in
`~/.local/share/sigilsmith/Baldur's Gate 3/backups/backup-<unix time>/`. The
newest 50 are kept; `b` in SigilSmith lists them and restores one (and can
undo the restore).

| File | What |
| --- | --- |
| `library.json` | The library as it was |
| `deploy_manifest.json`, `modsettings.lsx`, `config.json` | Copies, when they existed |
| `meta.json` | `{ "timestamp", "reason", "game", "profile" }`. `reason` says what made the backup, such as `manual deploy`, `cli import` or `before restore`. |
| `modsettings-new-folder.lsx` | Only for a move between native and Proton: the new folder's own load order |

`backups/last.json` points to the newest: `{ "path", "timestamp" }`.

## What SigilSmith reads from mods

- **`meta.lsx`** inside a `.pak`: `UUID`, `Folder`, `Name`, `Version64`,
  `MD5`, `Author`, `Description`, `PublishHandle`, `Type`, `Tags`, `Created`
  and the module's dependencies. A `.pak` without a UUID is an override pak.
- **`info.json`** (or `mod.json`, `modinfo.json`) next to a mod, as written by
  mod tools: `UUID`, `Folder`, `Name`, `Created` and dependencies
  (`Dependencies` or `RequiredMods`).
- **Script Extender:** `Mods/<Folder>/ScriptExtender/Config.json`
  (`RequiredVersion` and `FeatureFlags`). Osiris story scripts are detected
  from `Mods/<Folder>/Story/RawFiles/Goals/`.
- **Pak versions:** LSPK v15 and v16 (the Early Access format some tools still
  write) and v18 and newer.
