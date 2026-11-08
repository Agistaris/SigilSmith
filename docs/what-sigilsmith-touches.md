# What SigilSmith touches

Every folder SigilSmith reads or writes, every network call it makes, and how
to turn the network calls off. This page describes SigilSmith 1.0.0 and is
updated with each release.

In short:

- It writes to its own data folder, the game's mod folders and the game's
  load order file.
- It **never writes Steam's files** or other launchers' settings.
- It talks only to GitHub, for update checks and downloads you agreed to, and
  to Steam's image servers for game pictures, if you allow it.
- It sends **no telemetry**.

Paths below use the defaults. `~/.local/share` is `$XDG_DATA_HOME` if you set
it, and `~/.cache` is `$XDG_CACHE_HOME`.

## SigilSmith's own data

Everything SigilSmith keeps is in `~/.local/share/sigilsmith/`. Deleting this
folder resets SigilSmith completely (take your mods out of the game first:
turn them all off and deploy).

| Path | What it holds |
| --- | --- |
| `config.json` | App settings: updates, game pictures, window layout, confirmations, the downloads folder |
| `Baldur's Gate 3/config.json` | Game settings: game folder, Larian folder, active profile, mod store location |
| `Baldur's Gate 3/library.json` | Your mods, profiles, load orders and conflict picks |
| `Baldur's Gate 3/deploy_manifest.json` | Every link the last deploy made, so later deploys only remove SigilSmith's own |
| `Baldur's Gate 3/mods/<id>/` | SigilSmith's copy of each mod you imported (the files you imported from are left alone) |
| `Baldur's Gate 3/sigillink/<id>.json` | The file list of each mod |
| `Baldur's Gate 3/backups/backup-<time>/` | One per deploy: library, `modsettings.lsx` and settings. The newest 100 are kept (you can choose 50, 100, 250 or 500), plus any you save. |
| `Baldur's Gate 3/set-aside/` | Files that were in the way of a deploy, kept here and put back later |
| `Baldur's Gate 3/move-journal.json` | Only while a move between native and Proton is unfinished |
| `Baldur's Gate 3/smart_rank_cache.json` | SigiLink ranking cache |
| `Baldur's Gate 3/gui-state.json` | The desktop app's own choices: icon-only rail, Details and side panels shown or hidden, which rail folds are open |
| `Baldur's Gate 3/sigilsmith.log` | The log. When it passes 2 MiB, SigilSmith cuts it to the last 1 MiB when it starts. Delete it whenever you like. |
| `Baldur's Gate 3/tmp/`, `trash/` | Imports in progress, and removed mods waiting to be deleted |

You can move the mod store (`mods/` and `sigillink/`) to another drive in
Settings; it then lives there instead.

Other places it writes only when you ask:

- `~/.cache/sigilsmith/updates/`: update downloads.
- `~/Downloads/SigilSmith/exports/`: mod list exports from the terminal app
  (`modlist-bg3-<profile>-<time>.json`, `modsettings-bg3-<profile>-<time>.lsx`).
  The window asks which folder to export to.
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

- **Archives:** every archive is read by SigilSmith itself, in a child copy of
  SigilSmith with limits on time, size and memory. On Linux that copy is also
  locked down: it can read only the archive and write only its own output
  folder.
- **`tar`**, to unpack a `.tar.gz` update.
- **Your browser** (`xdg-open`), only for links you choose: a Nexus Mods search
  for a missing mod, or a DuckDuckGo search when only its ID is known.
  SigilSmith doesn't fetch these pages itself.
- **The clipboard**, only when you copy something: the log, a mod list, a
  launch option, a link or a mod ID.
- **Updating itself:** the AppImage and `.tar.gz` replace their own file with
  the new version, through a temporary `.sigilsmith-update` next to it. No
  password is involved.
- **The system's password prompt** (`pkexec`, through polkit), only for a
  `.deb` or `.rpm` update you started with **Update now**. Your system draws the
  prompt, and SigilSmith never sees the password. The packages add one polkit
  policy, `/usr/share/polkit-1/actions/com.sigilmods.sigilsmith.update.policy`,
  which asks for an administrator's password every time. After you authorize it,
  a small part of SigilSmith runs as root: it holds the checked package in
  `/var/cache/sigilsmith-updates`, checks the signature again and installs it
  with apt, dnf, zypper or rpm. That is the only thing SigilSmith ever runs as
  root. If there's no prompt, SigilSmith shows the command instead.

## Network

SigilSmith makes the requests below. Each sends only the header
`User-Agent: SigilSmith`: no account IDs, tokens or information about your
mods. The host sees your IP address, as with any download.

| Request | When | Turn it off |
| --- | --- | --- |
| `https://api.github.com/repos/Agistaris/SigilSmith/releases/latest` (is there a newer SigilSmith?) | Only once you've agreed to automatic checks (the **Check for updates?** question once your game folder is set up; until you choose, nothing is checked). Then at start and when Settings opens, at most every 10 minutes. **Check for updates** always checks when you choose it. Command-line commands never check. | Window: **Settings → Updates → Don't check**. Terminal app: **Esc → Check for updates at start → Off**. |
| The release's package, `SHA256SUMS.txt` and its signature `SHA256SUMS.txt.sig` from GitHub | When you choose **Update now**, or by itself with **Check and install by itself** (AppImage and `.tar.gz` only) | Don't use **Check and install by itself**, or choose **Don't check** |
| `https://api.github.com/repos/Norbyte/bg3se/releases/latest` and its updater `.zip` (Norbyte's Script Extender) | Only when you choose **Set up for me** in the Script Extender checklist | Don't use it; set up the Script Extender yourself |
| Official game cover and hero JPEGs from `shared.steamstatic.com`, falling back to `cdn.cloudflare.steamstatic.com` when a file is missing or moved | In the background when a supported game's launcher and SigilSmith's cache have no copy. This is on by default, but requests wait until you have answered the picture choice once: on the first-start **Check for updates?** question (Ask me later counts), or by changing its switch. The TUI and command line never fetch pictures. The files stay in `~/.cache/sigilsmith/art/` (or `$XDG_CACHE_HOME/sigilsmith/art/`). No Steam install or account is needed. | Untick the picture box on the first-start question, or turn off **Game pictures from Steam** in GUI **Settings → Privacy** (`download_game_art`). In the TUI, use **Esc → What goes online → Game pictures from Steam**. |

Downloads that publish SHA-256 hashes are checked against them and refused if
they don't match. Steam art has no published hashes; we instead cap its size
and dimensions and validate its JPEG structure before keeping it.

**No telemetry.** SigilSmith has no analytics, crash reporting or usage
tracking, and nothing needs an account. If optional usage reporting is ever
added, it will be off unless you turn it on, and this page will document it
before the release that adds it.
