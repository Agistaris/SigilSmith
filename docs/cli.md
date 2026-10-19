# Command-line reference

Run `sigilsmith` with no arguments to open the app. The commands below list
your mods, profiles and dependencies, as text or JSON, and import mods
without opening the app. This page describes SigilSmith 0.9.11.

```text
sigilsmith                          Open the app
sigilsmith mods [list]              List mods
sigilsmith profiles [list]          List profiles
sigilsmith deps [missing]           List missing dependencies (the default)
sigilsmith deps list                List every mod's dependencies
sigilsmith deps resolved            List dependencies matched to installed mods
sigilsmith deps debug <mod>         Show how one mod's dependencies were matched
sigilsmith paths                    Show the game paths in use
sigilsmith --import <paths...>      Import mods without opening the app
sigilsmith --help                   Show help (also -h, help)
sigilsmith --version                Show the version (also -V, version)
```

Every command except `--help` and `--version` starts the way the app does: it
reads your library, picks up changes made in the game's own mod manager and,
unless **Updates** is **Off**, checks GitHub for a new release
([details](what-sigilsmith-touches.md#network)). Close the app before
importing from the command line, so the two don't both change your library.

## Global options

| Option | Meaning |
| --- | --- |
| `--format text` | Plain text for people (the default) |
| `--format json` | JSON for scripts (`mods`, `profiles`, `deps list`, `deps missing`, `deps resolved`, `paths`) |
| `--profile <name>` | Use this profile instead of the active one (`mods` and `deps`). The name must match exactly. |

Options can go anywhere on the line, and `--format=json` and
`--profile=<name>` work too.

## mods

```bash
sigilsmith mods
sigilsmith mods --sort name --filter ui
sigilsmith --format json mods
```

Lists every mod in your library, with its place in the profile's load order.

| Option | Meaning |
| --- | --- |
| `--sort <key>` | `order` (default), `name`, `created`, `added` or `kind` |
| `--reverse`, `-r` | Reverse the order |
| `--filter <text>` | Only mods whose name contains this text (not case-sensitive) |

Mods that aren't in the profile come last when sorted by order.

Text output, one mod per line: position (`-` if not in the profile), `[x]` if
it's on, the kind, `SE` if it uses the Script Extender, `Os` if it ships
Osiris story scripts, its own date, the date you added it, and the name.

```text
  1 [x] Pak          SE    05-02-2026 10-01-2026 Example Settings Menu
  2 [x] Pak             Os 03-11-2025 10-01-2026 Example Spells
  - [ ] Generated          -- -- ---- 12-09-2026 Example Dyes
```

Dates follow your locale (`LC_TIME`, then `LANG`).

JSON output is an array:

```json
[
  {
    "id": "11111111-2222-4333-8444-555555555555",
    "name": "Example Settings Menu",
    "display_name": "Example Settings Menu",
    "kind": "Pak",
    "created_at": 1770285600,
    "added_at": 1791100000,
    "enabled": true,
    "order": 1,
    "scripts": { "script_extender": { "required_version": 20 } }
  }
]
```

| Field | Notes |
| --- | --- |
| `id` | The mod's ID ([file formats](file-formats.md#mod-list-export-sigilsmith-json)) |
| `name`, `display_name` | The stored name, and the name SigilSmith shows |
| `kind` | `Pak`, `Override Pak`, or the folders a loose mod uses, joined with `+` (such as `Generated` or `Data+Bin`) |
| `created_at`, `added_at` | Unix seconds. `created_at` is `null` when the mod has no date. |
| `enabled` | On in this profile |
| `order` | Position in the profile, from 1, or `null` if it isn't in the profile |
| `scripts` | Left out when the mod uses neither the Script Extender nor Osiris scripts |

## profiles

```bash
sigilsmith profiles
sigilsmith --format json profiles
```

Text output lists the names, with `*` before the active one. JSON:
`[{ "name": "Default", "active": true }]`.

## deps

`sigilsmith deps` on its own is `sigilsmith deps missing`.

**`deps missing`** lists what the profile's enabled mods need but don't have:
a mod that isn't installed, or one that's turned off.

```json
[
  {
    "required_by": "Example Spells",
    "required_by_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
    "dependency": "Example Settings Menu",
    "dependency_id": "11111111-2222-4333-8444-555555555555",
    "reason": "disabled"
  }
]
```

`reason` is `"not installed"` or `"disabled"`.

**`deps list`** lists each mod's dependencies:

```json
[
  {
    "mod_name": "Example Spells",
    "mod_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
    "dependencies": [
      { "id": "11111111-2222-4333-8444-555555555555", "name": "Example Settings Menu", "enabled": true }
    ]
  }
]
```

`name` is `null` when the dependency isn't installed.

**`deps resolved`** (also `deps aliases`) shows which installed mods each
dependency was matched to:
`{ "required_by", "required_by_id", "dependency", "resolved": [{ "id", "name", "enabled" }] }`.

**`deps debug <mod>`** shows, as text, how one mod's dependencies were found
and matched. `<mod>` is its ID or name.

## paths

```bash
sigilsmith paths
```

```text
Game root: /home/you/.local/share/Steam/steamapps/common/Baldurs Gate 3
Data dir: /home/you/.local/share/Steam/steamapps/common/Baldurs Gate 3/Data
Larian dir: /home/you/.local/share/Larian Studios/Baldur's Gate 3
Larian mods: /home/you/.local/share/Larian Studios/Baldur's Gate 3/Mods
Modsettings: /home/you/.local/share/Larian Studios/Baldur's Gate 3/PlayerProfiles/Public/modsettings.lsx
```

JSON has `game_root`, `data_dir`, `larian_dir`, `larian_mods_dir`,
`modsettings_path` and `error` (`null`, or why the paths couldn't be checked;
then the configured paths are shown).

## Importing mods

```bash
sigilsmith --import ~/Downloads/SomeMod.zip ~/Downloads/OtherMod.pak
sigilsmith -i ~/Downloads/mods/ --deploy
```

Imports `.zip`, `.7z` and `.pak` files and folders into the library, the same
as dropping them on the app. `-i` is short for `--import`. Everything after
`--` is taken as a path, for names that start with `-`.

| Option | Meaning |
| --- | --- |
| `--deploy` | Deploy after importing (with a backup, as in the app) |
| `--no-deploy` | Don't deploy (the default) |
| `-q`, `--quiet` | Errors only |
| `-v`, `-vv` | More detail; `-vv` is the most |
| `--verbosity <level>` | `quiet`, `normal`, `verbose` or `debug` |
| `--verbose` | Same as `--verbosity verbose` |

When a mod is already in the library, SigilSmith asks:

```text
Choose [o]verwrite, [s]kip, keep [b]oth, overwrite [a]ll, skip all [k] (Enter = default):
```

The default is to overwrite when the new copy is newer and skip otherwise.
**Keep both** is offered when the two have different IDs. With no input (for
example `</dev/null`), every question takes its default.

The command ends with a summary (`Import complete: 3 mod(s) imported, 1
failure(s)`) and lists up to eight failures.

## Exit status

`0` when the command ran, even if some imports failed (check the summary).
Non-zero when SigilSmith couldn't start, a deploy failed, or the command was
wrong: an unknown profile, sort key or `deps` command. Unrecognised arguments
show the help. (`sigilsmith debug …` commands exist only in development
builds.)
