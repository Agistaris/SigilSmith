# What SigilSmith touches

Every folder SigilSmith reads or writes, every network call it makes, and how
to turn the network calls off. This page describes SigilSmith 0.9.11 and is
updated with each release.

In short:

- It writes to its own data folder, the game's mod folders and the game's
  load order file.
- It **never writes Steam's files** or other launchers' settings.
- It talks only to GitHub, for update checks and downloads you choose.
- It sends **no telemetry**.

Paths below use the defaults. `~/.local/share` is `$XDG_DATA_HOME` if you set
it, and `~/.cache` is `$XDG_CACHE_HOME`.

## SigilSmith's own data

Everything SigilSmith keeps is in `~/.local/share/sigilsmith/`. Deleting this
folder resets SigilSmith completely (take your mods out of the game first:
turn them all off and deploy).

| Path | What it holds |
| --- | --- |
| `config.json` | App settings: updates, confirmations, the downloads folder |
| `Baldur's Gate 3/config.json` | Game settings: game folder, Larian folder, active profile, mod store location |
| `Baldur's Gate 3/library.json` | Your mods, profiles, load orders and override choices |
| `Baldur's Gate 3/deploy_manifest.json` | Every link the last deploy made, so later deploys only remove SigilSmith's own |
| `Baldur's Gate 3/mods/<id>/` | SigilSmith's copy of each mod you imported (the files you imported from are left alone) |
| `Baldur's Gate 3/sigillink/<id>.json` | The file list of each mod |
| `Baldur's Gate 3/backups/backup-<time>/` | One per deploy: library, deploy record, `modsettings.lsx` and settings. The newest 50 are kept. |
| `Baldur's Gate 3/set-aside/` | Files that were in the way of a deploy, kept here and put back later |
| `Baldur's Gate 3/move-journal.json` | Only while a move between native and Proton is unfinished |
| `Baldur's Gate 3/smart_rank_cache.json` | SigiLink ranking cache |
| `Baldur's Gate 3/sigilsmith.log` | The log (it isn't trimmed; delete it whenever you like) |
| `Baldur's Gate 3/tmp/`, `trash/` | Imports in progress, and removed mods waiting to be deleted |

You can move the mod store (`mods/` and `sigillink/`) to another drive in
Settings; it then lives there instead.

Other places it writes only when you ask:

- `~/.cache/sigilsmith/updates/`: update downloads.
- `~/Downloads/SigilSmith/exports/`: mod list exports
  (`modlist-bg3-<profile>-<time>.json`, `modsettings-bg3-<profile>-<time>.lsx`).
- A log export goes to the folder you pick.

## The game

| Where | What SigilSmith does |
| --- | --- |
| `Data/`, `Data/Generated/`, `bin/` in the BG3 install folder | Links loose mod files into place |
| `Mods/` in the Larian data folder | Links `.pak` mods into place |
| `PlayerProfiles/Public/modsettings.lsx` in the Larian data folder | Reads it at start; rewrites it on every deploy with your load order |
| `ModCrashSanityCheck` in the Larian data folder | Removes it (only when it's empty and BG3 isn't running), because BG3 turns every mod off on the next launch while it's there |

How deploys treat your files:

- **Links, not copies.** A hardlink when the mod store is on the same drive,
  otherwise a symlink.
- **It only removes its own links.** A deploy removes a file only if the
  last deploy recorded it and it's still the same link. Anything changed
  since is left in place, with a warning in the log.
- **Nothing in the way is deleted.** A file already where a mod's file goes
  is moved to `set-aside/` and put back when SigilSmith's link goes away.
- **In-game mods.** A mod's `.pak` in `Mods/` is only deleted when you remove
  that mod and confirm that its files should be deleted too (off by default).

Only when you confirm it:

- **Moving between native and Proton:** saves that only the old Larian folder
  has can be copied to the new one. Existing saves are never overwritten.
  Links SigilSmith left in the old folder are removed.
- **Script Extender setup** (Proton): puts `DWrite.dll` in the game's `bin`
  folder and turns on the `DWrite` override in BG3's Proton prefix
  (`compatdata/1086940/pfx/user.reg`, backed up first to
  `user.reg.sigilsmith-backup`). It won't run while BG3 is running.

If your Larian folder is inside a Lutris, Heroic, Faugus or Bottles prefix,
the `Mods/` and `modsettings.lsx` steps above happen there.

## Read only

SigilSmith reads these to find the game and how it runs. It **never writes**
them.

- **Steam:** `steamapps/libraryfolders.vdf`, `config/config.vdf` (whether BG3
  runs through Proton), `config/loginusers.vdf` and
  `userdata/<id>/config/localconfig.vdf` (BG3's launch options), in
  `~/.local/share/Steam`, `~/.steam/steam`, and the Flatpak and Snap Steam
  folders. When the Script Extender needs a launch option, SigilSmith copies
  it for you to paste into Steam yourself.
- **Other launchers:** the settings and prefixes of Lutris, Heroic, Faugus and
  Bottles, native and Flatpak.
- **BG3LE:** its folder in `~/.local/share/bg3le`, to detect it.
- **Running programs:** `/proc`, to check whether BG3 is running before steps
  that need it closed.
- **How SigilSmith was installed:** the dpkg, rpm and pacman databases, to pick
  the right way to update.

## Other programs and the desktop

- **`7z`**, if installed, to unpack archives (otherwise a built-in unpacker).
- **`tar`**, to unpack a `.tar.gz` update.
- **Your browser** (`xdg-open`), only for links you choose: a Nexus Mods search
  for a missing mod, or a DuckDuckGo search when only its ID is known.
  SigilSmith doesn't fetch these pages itself.
- **The clipboard**, only when you copy something: the log, a mod list, a
  launch option, a link or a mod ID.
- **Updating itself:** the AppImage and `.tar.gz` replace their own file with
  the new version, through a temporary `.sigilsmith-update` next to it. For
  the `.deb` and `.rpm` SigilSmith shows the command instead.

## Network

SigilSmith makes three kinds of request, all to GitHub. Each sends only the
header `User-Agent: SigilSmith`: no IDs, no tokens, nothing about your mods.
GitHub sees your IP address, as with any download.

| Request | When | Turn it off |
| --- | --- | --- |
| `https://api.github.com/repos/Agistaris/SigilSmith/releases/latest` (is there a newer SigilSmith?) | At start and when Settings opens, at most every 10 minutes. Command-line commands (except `--help` and `--version`) check too. **Check For Updates** always checks. | **Esc → Updates → Off** stops the automatic checks |
| The release's package and `SHA256SUMS.txt` from GitHub | When you choose **Update now**, or by itself in **Automatic** mode (AppImage and `.tar.gz` only) | Use **Ask** or **Off** |
| `https://api.github.com/repos/Norbyte/bg3se/releases/latest` and its updater `.zip` (Norbyte's Script Extender) | Only when you choose **Set up for me** in the Script Extender checklist | Don't use it; set up the Script Extender yourself |

Downloads are checked against their published SHA-256 and refused if they
don't match.

**No telemetry.** SigilSmith has no analytics, crash reporting or usage
tracking, and nothing needs an account. If optional usage reporting is ever
added, it will be off unless you turn it on, and this page will document it
before the release that adds it.
