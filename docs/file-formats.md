# File formats

The files SigilSmith reads and writes, so you can back them up, script
against them or move to another tool. This page describes SigilSmith 1.0.1.

| File | Status |
| --- | --- |
| [Mod list export (SigilSmith JSON)](#mod-list-export-sigilsmith-json) | **Stable.** The format for sharing and moving a load order. |
| [`modsettings.lsx`](#modsettingslsx) | BG3's own format. What SigilSmith writes and reads is listed below. |
| [`library.json`](#libraryjson), [`config.json`](#configjson), [`deploy_manifest.json`](#deploy_manifestjson), [backups](#backups) | Documented for reading and recovery. They may gain fields; fields marked *internal* may change without notice. Each carries a [format number](#format-numbers-and-upgrades). |

All of them are JSON or XML in UTF-8. Times are Unix seconds unless noted.
Unknown fields are ignored when SigilSmith reads a file and dropped the next
time it saves it. If you edit these files by hand, close SigilSmith first.

## Format numbers and upgrades

`library.json`, both `config.json` files and `deploy_manifest.json` begin with
`"schema_version": 1`. The files SigilSmith 0.9.x wrote have no
`schema_version`; that counts as format 0.

- **Upgrading.** The first time 1.0.0 starts, it reads each older file,
  writes it in format 1 and replaces it. Before that it keeps the old file
  beside the new one as `<name>.format-0.bak` (for example
  `library.json.format-0.bak`). It checks the copy, never overwrites a copy
  that holds other content (it adds `-2`, `-3` and so on), and leaves a file
  alone if it can't read it. The window and the terminal app say so in the
  status line and list each file in the log; command-line commands print it on
  standard error.
- **Going back to 0.9.x.** 0.9.x can't read format 1: it stops with an
  error and leaves the file alone. To go back, close SigilSmith and copy each
  `.format-0.bak` over the file it sits beside.
- **Newer files.** If a file has a format number higher than the SigilSmith
  you're running knows, SigilSmith doesn't read it as if it were its own, and
  never changes or replaces it. It says so and points to the copy kept from
  before an upgrade, if there is one.
- The [mod list export](#mod-list-export-sigilsmith-json) has its own
  `schema_version` and didn't change.

Where the files are: [What SigilSmith touches](what-sigilsmith-touches.md).

## Mod list export (SigilSmith JSON)

Written by **Export Mod List**. In the window, the profile menu's export list
(`Ctrl+E` opens it) asks which folder to use once you pick a file. In the
terminal app, `e` in the Explorer or `Ctrl+E` on the main screen opens the export
menu. Its JSON entry asks where to save, starting at
`SigilSmith/exports/modlist-bg3-<profile>-<YYYYMMDD-HHMMSS>.json` in your
downloads folder (`~/Downloads`, unless you set another one).
It carries one profile's order, on/off state and conflict picks.

```json
{
  "schema_version": 1,
  "exported_at": "2026-10-17T14:02:11Z",
  "sigilsmith_version": "1.0.1",
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
| `file_overrides` | array | Optional. Your conflict picks (the Conflicts screen's choices; the field keeps its older name): `kind` (`generated`, `data` or `bin`), `relative_path` inside that folder, and the winning `mod_id`. |

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
A conflict pick's `mod_id` is used as it is.

**Import options:** into a new profile (the default) or the active one, and
**Merge** (the default: keeps your other conflict picks) or **Strict**
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
and choices. A copy is in every backup. Format 1:

Top level:

| Field | Notes |
| --- | --- |
| `schema_version`, `game_id` | `1` and `"bg3"` |
| `mods` | Every mod in the library (see below) |
| `profiles` | Your profiles (see below) |
| `active_profile` | The name of the active profile |
| `renamed_ids` | Old ID → new ID, for mods whose ID SigilSmith corrected. Left out when empty. |
| `dependency_blocks`, `metadata_cache_version`, `metadata_cache_key`, `load_order_hash`, `in_game_sync_enabled`, `repair_version` | *Internal* (`load_order_hash` is the fingerprint of BG3's `modsettings.lsx`; `in_game_sync_enabled` is the switch for picking up changes made in the game's own mod manager) |

Each mod:

| Field | Notes |
| --- | --- |
| `id` | The mod's ID (see [Mod IDs](#mod-list-export-sigilsmith-json)) |
| `name` | The mod's name |
| `added_at`, `created_at`, `modified_at` | When it was added to SigilSmith, and the mod's own dates if known |
| `targets` | What the mod installs: `{ "target": "pak", "path": "Name.pak" }` for a `.pak` (`path` is the file name), or `{ "target": "generated" \| "data" \| "bin", "path": "Generated" }` for loose files (`path` is the folder) |
| `target_overrides` | Targets you turned off for this mod: `{ "target", "enabled" }`. A target without an entry is on. |
| `source` | `"managed"` (imported into SigilSmith) or `"native"` (installed by the game's own mod manager) |
| `source_label` | What it was imported from |
| `dependencies` | What the mod needs, left out when empty (see below) |
| `game` | What only the game's module understands, by game ID: `{ "bg3": { "packages": […], "data": {…} } }`. Left out when empty. |

A dependency is `{ "type", "label", "required", "min_version", "as_written" }`.
`type` is `"package"` (another mod, with its module UUID as `id`), `"file"`
(with a `name`), `"tool"` (with an `id`) or `"game"`. `label` is the name
shown, `required` is `true` unless the mod says otherwise, `min_version` is
left out when there isn't one, and `as_written` is the text the mod's own
files gave.

`game.bg3.packages` has one entry per `.pak` target: `{ "file", "info" }`,
where `file` matches the target's `path` and `info` has `id` (the module UUID),
`name`, `folder`, `version`, `author` and `description` (`null` when the mod
has none), and, when the mod's `meta.lsx` has them, `md5`, `publish_handle`
and `module_type`.
`game.bg3.data` is left out when empty, and otherwise holds
`{ "script_extender": { "required_version", "features" }, "osiris": true }`
(see [what SigilSmith reads from mods](#what-sigilsmith-reads-from-mods)).

Each profile:

| Field | Notes |
| --- | --- |
| `name` | The profile's name |
| `order` | The load order, first to last: `{ "id", "enabled", "missing_label" }`. `missing_label` marks a placeholder for a mod that isn't in the library. |
| `file_overrides` | Your conflict picks: `{ "target", "relative_path", "mod_id" }`, like the export's but with `target` where the export keeps `kind`. Only picks that differ from the default (the last mod in the order wins) are stored. Left out when empty. |
| `sigillink_pins`, `sigillink_meta` | *Internal* (SigiLink ranking) |
| `game` | *Internal.* What the game's module keeps for the profile, by game ID. Left out when empty. |

A profile named `__sigillink_ranking__` is *internal*; SigilSmith hides it.

## config.json

**App settings,** `~/.local/share/sigilsmith/config.json` (format 1):

| Field | Values |
| --- | --- |
| `schema_version`, `game_id` | `1` and `"bg3"` (0.9.x called the game `active_game`) |
| `update_mode` | `"ask"`, `"automatic"` or `"off"`. Left out until you have answered the first-start update question, and nothing checks for updates by itself while it is missing. A `"ask"` from 0.9.x is removed when the file is upgraded, so you are asked once. |
| `skipped_update_version` | The version you chose to skip, if any |
| `download_game_art`, `download_game_art_answered` | Whether game pictures may be fetched from Steam (default `true`), and whether you have answered that choice. Nothing is fetched until you have. |
| `downloads_dir` | Your downloads folder (default: your Downloads folder). The terminal app writes its exports under it. |
| `motion` | `true` (default) or `false` for the calm look: nothing animates |
| `layout` | `"automatic"` (default), `"desktop"` or `"controller"`: which layout the window draws |
| `offer_controller_layout` | Whether to offer the controller layout when a controller is used (default `true`) |
| `auto_deploy_enabled`, `enable_mods_after_import`, `confirm_profile_delete`, `confirm_mod_delete`, `delete_mod_files_on_remove`, `warn_missing_dependencies`, `offer_dependency_downloads`, `show_startup_dependency_notice` | Settings toggles (`true` or `false`) |
| `tool_settings` | Toggles that belong to a game's tools, by key (such as `bg3.script-extender.show-notice`). 0.9.x's `show_script_extender_notice` moves here when the file is upgraded. Left out when empty. |
| `sigillink_ranking_enabled`, `sigillink_auto_preview`, `sigillink_onboarded`, `sigillink_pin_notice_dismissed` | SigiLink settings |
| `last_browser_dir`, `last_whats_new_version`, `default_sort_column`, `dependency_search_copy_preference` | Remembered choices |

**Game settings,** `~/.local/share/sigilsmith/Baldur's Gate 3/config.json` (format 1):

| Field | Values |
| --- | --- |
| `schema_version`, `game_id`, `game_name`, `data_dir` | The format, the game and its data folder; set by SigilSmith at every start |
| `install` | `{ "game_root": …, "folders": { "larian": … } }`: the BG3 install folder, and the Larian data folder (with `Mods/` and `PlayerProfiles/`) under the ID `larian`. 0.9.x kept these as `game_root` and `larian_dir` at the top. |
| `active_profile` | The active profile's name |
| `sigillink_cache_dir` | Where the mod store is, if you moved it |
| `declined_move` | A Larian folder you chose not to move to ("Don't ask again for this folder"). Left out when none. |
| `keep_links_in` | Larian folders where you chose to keep SigilSmith's leftover links, so it stops offering to remove them. Left out when empty. |
| `setup_confirmed` | Written only as `false`, while you haven't yet confirmed the game folders SigilSmith found at first run. A file without it counts as confirmed. |
| `keep_backups` | How many automatic backups to keep: `50`, `100`, `250` or `500`. Left out at the default, `100`. |
| `backup_on_profile_switch` | Written only as `true`, when switching profiles makes a backup |

## deploy_manifest.json

`~/.local/share/sigilsmith/Baldur's Gate 3/deploy_manifest.json`: what the
last deploy put in the game, so the next one only removes SigilSmith's own
files. Format 1:

| Field | Notes |
| --- | --- |
| `schema_version` | `1` |
| `files` | Loose files: `{ "target", "dir", "path", "source_mod", "source_id" }`. `target` is the target's ID (`generated`, `data` or `bin`) and `dir` the folder it went to. (0.9.x had the folder as `target` and a label as `source_kind`.) |
| `packages` | The `.pak` links in the Larian `Mods/` folder (0.9.x called this `pak_files`) |
| `links` | Destination path → `{ "source", "mode": "hardlink" \| "symlink", "dev", "ino", "size" }` (`dev` and `ino` for hardlinks only). A file counts as SigilSmith's only while it still matches this record. |
| `set_aside` | Files moved out of the way: `{ "original", "saved" }` |
| `generated`, `kept`, `created_dirs` | *Internal.* Files a game module wrote, records of kept settings files, and folders deploys made (for **Undeploy mods** to clean up). Each is left out when empty. |

## Backups

Each deploy saves a backup in
`~/.local/share/sigilsmith/Baldur's Gate 3/backups/backup-<unix time>/`, except
a deploy that only follows a profile switch (unless the setting for backing up
on a switch is on). The newest 100 of these automatic backups are kept (50, 250
or 500 when chosen); **Settings → Backups** in the window, or `b` in the
terminal app, lists them and restores one (and can undo the restore).

A saved backup (Back up now, or Keep this backup on an automatic one) is a
`backups/saved-<unix time>/` folder with the same files. Saved backups are
never deleted to make room and don't count toward the number kept; only the
player deletes one. Keeping a backup renames its `backup-<unix time>` folder
to `saved-<unix time>`. Older versions of SigilSmith don't list saved backups
and never delete them.

| File | What |
| --- | --- |
| `library.json.zst` | The library as it was, zstd-compressed. Backups made by older versions hold a plain `library.json` instead; both are read. |
| `modsettings.lsx`, `config.json` | Copies, when they existed. Older backups also hold a copy of `deploy_manifest.json`, which a restore never read. |
| `meta.json` | `{ "timestamp", "reason", "game", "profile", "event", "saved", "name" }`. `reason` says what made the backup (`manual deploy`, `cli import`, `before restore`, ...); `event` is what changed since the backup before it; `name` is a saved backup's name, if any. Fields may be missing; unknown ones are ignored. |
| `modsettings-new-folder.lsx` | Only for a move between native and Proton: the new folder's own load order |

`backups/last.json` points to the newest backup, automatic or saved:
`{ "path", "timestamp" }`.

A backup is written to a hidden folder first and only appears once it is
complete.

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
