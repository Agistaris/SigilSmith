# Troubleshooting and FAQ

Answers to the questions that come up most. This page describes SigilSmith
1.0.1. The keys and panel names are the terminal app's (`sigilsmith --tui`),
where **Esc** opens the menu. The desktop app has the same settings under
**Settings**; where its names differ, they are given too. If your problem
isn't here, see [Report a bug](#report-a-bug).

- [My mods don't show up in the game](#my-mods-dont-show-up-in-the-game)
- [BG3 turned every mod off after a crash](#bg3-turned-every-mod-off-after-a-crash)
- [Which Larian folder does SigilSmith use?](#which-larian-folder-does-sigilsmith-use)
- [What do SE and Os mean?](#what-do-se-and-os-mean)
- [A mod has a ghost icon or says missing](#a-mod-has-a-ghost-icon-or-says-missing)
- [Dependencies are missing](#dependencies-are-missing)
- [Load order, SigiLink ranking and pins](#load-order-sigilink-ranking-and-pins)
- [Two mods change the same file](#two-mods-change-the-same-file)
- [Where is the log, and how do I share it?](#where-is-the-log-and-how-do-i-share-it)
- [Restore a backup](#restore-a-backup)
- [The AppImage won't start](#the-appimage-wont-start)
- [Fedora and openSUSE](#fedora-and-opensuse)
- [The title bar or window size looks wrong](#the-title-bar-or-window-size-looks-wrong)
- [Steam Deck](#steam-deck)
- [Update checks](#update-checks)
- [Reset SigilSmith's settings](#reset-sigilsmiths-settings)
- [Report a bug](#report-a-bug)

## My mods don't show up in the game

Go down this list. Most problems are one of the first three.

1. **Is the mod on?** The first column of the Mod Stack shows `[x]` for on
   and `[ ]` for off. In the Context panel, **Profile** reads
   `off / on | total`.
2. **Did it deploy?** Mods only reach the game when SigilSmith deploys. With
   **Deploy automatically** on (the default; Context shows `Auto-Deploy` as
   `ON`) it deploys after your changes. Press `d` to deploy by hand. When it
   worked, the Log says `Deploy complete`. A warning or error line next to it
   says what went wrong.
3. **Is the game reading the folder SigilSmith writes to?** Steam can run BG3
   natively or through Proton, and each has its own Larian folder. See
   [Which Larian folder does SigilSmith use?](#which-larian-folder-does-sigilsmith-use)
4. **Restart the game.** BG3 reads its mod list when it starts, so changes
   made while it's open need a restart.
5. **Does the mod show SE?** It needs the Script Extender. See
   [What do SE and Os mean?](#what-do-se-and-os-mean)
6. **Does the mod show a number under Dep?** Something it needs is missing or
   off. See [Dependencies are missing](#dependencies-are-missing).
7. **Does the mod show a ghost?** SigilSmith can't find its file. See
   [A mod has a ghost icon or says missing](#a-mod-has-a-ghost-icon-or-says-missing).

To check what the game sees, open the in-game mod manager. A mod that's listed
there was deployed. If it's listed but its changes don't show up, check steps
5 and 6.

## BG3 turned every mod off after a crash

After a crash or a killed game, BG3 can turn every mod off the next time it
starts. It does this when it finds an empty `ModCrashSanityCheck` folder in
the Larian folder, which it leaves behind when it doesn't exit cleanly.

SigilSmith removes that folder when it starts and on every deploy, as long as
BG3 isn't running. The log then says `Removed ...ModCrashSanityCheck`. So
after a crash: close BG3, open SigilSmith, then start the game again.

## Which Larian folder does SigilSmith use?

BG3 keeps its mods, saves and load order in the Larian folder. Where that is
depends on how Steam runs the game:

| How BG3 runs | Larian folder |
| --- | --- |
| Steam, native Linux | `~/.local/share/Larian Studios/Baldur's Gate 3` |
| Steam, Proton | `<Steam library>/steamapps/compatdata/1086940/pfx/drive_c/users/steamuser/AppData/Local/Larian Studios/Baldur's Gate 3`, in the library that holds the game |

Lutris, Heroic, Faugus and Bottles keep it inside their own prefixes.

SigilSmith follows Steam's compatibility setting. To see what it uses, run
`sigilsmith paths` or open **Esc → Folders** (in the desktop app, **Settings →
Game**), which lists the game folder, the mods folder (BG3's Larian folder), the
mod library and the rest. To change one, select it and press **Enter**; the
Larian folder list shows the folders it finds
([first launch](install.md#first-launch)).

**If you switch between native and Proton in Steam,** SigilSmith notices and
asks **Move your mod setup?**:

- **Move my setup** makes a backup, moves your mods and load order to the
  other folder, and removes the links it left in the old one. Each mod stays
  one file in SigilSmith's library.
- **Not now** leaves things as they are. On the startup question you can also
  choose **Don't ask again for this folder**. To move later, pick the other
  folder in **Esc → Folders**.
- A second question, **Copy your saves?**, offers saves that only the old
  folder has. Saves already in the new folder are never overwritten, and the
  old folder keeps its copies.
- In-game mods the new folder hasn't downloaded yet stay in your list; they
  join the load order once the game downloads them.
- If a move is interrupted, the next start asks **Finish moving your setup?**
  (**Finish the move** or **Go back**).

What it writes in each place is in
[What SigilSmith touches](what-sigilsmith-touches.md#the-game).

## What do SE and Os mean?

In the Mod Stack's script column, **SE** marks a mod that needs the BG3
Script Extender, and **Os** a mod that ships Osiris story scripts. SigilSmith
reads both from the mod's own files
([details](file-formats.md#what-sigilsmith-reads-from-mods)). Only SE needs
something installed.

Without the Script Extender, an SE mod's scripts don't run, so the mod can
seem to do nothing. The Context panel adds a **Script Ext.** line when an
enabled mod needs it and it isn't set up. When you turn on an SE mod,
SigilSmith checks your setup and shows a checklist, unless **Esc → Remind me
about the Script Extender** is off. **Esc → Set up the Script Extender** opens
the checklist at any time (in the desktop app, **Settings → Script Extender**
has **Check the setup** and **Set up…**).

The Script Extender is a separate project by Norbyte. For how to install it,
follow its own pages: [github.com/Norbyte/bg3se](https://github.com/Norbyte/bg3se).
SigilSmith can help on some setups:

- **Proton:** the checklist's **Set up for me** downloads the Script Extender
  from that project, puts `DWrite.dll` in the game's `bin` folder and turns on
  the `DWrite` override in BG3's Proton prefix. It asks first and won't run
  while BG3 is running
  ([what it changes](what-sigilsmith-touches.md#the-game)).
- **Native Linux:** SigilSmith detects BG3LE, the Script Extender for the
  native build (the checklist names Nexus mod 25431).
- **Lutris, Heroic, Bottles and other launchers:** the checklist says what to
  set there. SigilSmith never changes their settings, and it never writes
  Steam's. When a launch option is needed, it copies it for you to paste into
  Steam.

## A mod has a ghost icon or says missing

These are two different things.

**An empty-set mark (∅) in the link column.** The mod is in your library, but
SigilSmith can't find its `.pak` file. It happens when the file was deleted or
moved, when a mod store you moved to another drive isn't mounted, or when it's
an in-game mod (marked **N**) that the game hasn't downloaded yet.

- SigilSmith won't turn it on. A **SigiLink missing mod files** list opens;
  **Enter** there opens a Nexus search for the mod.
- A deploy leaves it out of the load order until its file is back, and the
  log warns about it.
- To fix it, import the mod again and choose **Overwrite** when SigilSmith
  says the mod already exists. It returns to its place in the list and turns
  on (unless **Esc → Turn on new mods** is off).
- For an in-game mod, start BG3: its mod manager downloads the mod.

**`(missing)` after a grey name.** This is a placeholder: a slot in your list
for a mod SigilSmith doesn't have. Details shows `Status: Missing mod`. You get
one when you import a mod list that names a mod you don't have. Import the mod
and it takes the slot back.

To get rid of a placeholder, press **Del** (or **Backspace**) on it and choose
**Remove the placeholder** in **Remove the placeholder for X?** Only its
place in the list goes, in every profile; no files change, and a mod you add
later won't take that place. With **Esc → Ask before removing a mod** off,
it goes without asking. Removing a mod that is in your library leaves no
placeholder behind.

## Dependencies are missing

The **Dep** column shows a number when a mod needs mods that are missing or
turned off. `sigilsmith deps` lists them by name
([CLI reference](cli.md#deps)).

- **A needed mod is in your library but off.** Turning the mod on asks
  **X needs other mods**. **Turn those on too** turns on what it needs;
  **Only this mod** turns on just this one, and it may not work.
- **A needed mod isn't in your library.** The mod stays off and a **Missing
  dependencies** list opens. **Enter** opens the mod's page or a Nexus search,
  and **Ctrl+C** copies the link. Import the missing mod, then turn the first
  one on again.
- **At start,** SigilSmith turns off enabled mods whose dependencies are
  missing and lists them under **Some mods were turned off**. They stay in
  your list. Install what they need and turn them back on. Tick **Don't tell
  me about this again** in that list, or turn off **Esc → Tell me about
  turned-off mods**, to stop the popup.
- **Removing a mod that others need** asks first (**Remove X?** names the
  mods that need it) and turns those mods off.

If the number looks wrong, for example a dependency matched to the wrong mod,
`sigilsmith deps debug "<mod>"` shows how each one was matched.
**Esc → Clear saved mod info** (in the desktop app, **Settings → Mods**) makes
SigilSmith read every mod's dependency and script information again; your
mods, profiles and order stay as they are.

## Load order, SigiLink ranking and pins

The `#` column is the load order. A mod lower in the list loads later, so when
two mods change the same thing, the lower one normally wins.

- **Move a mod:** `m` starts move mode, the arrow keys (or `u` and `n`) move
  it, and `m` or **Enter** places it (**Esc** cancels). Moving only works in
  the Order view; if you're sorted another way, SigilSmith asks **Move
  requires Order** and offers to switch.
- **The list looks out of order.** The list starts in load order. If you
  sort by another column, the `#` column still shows each mod's real place.
  The sort is in the Mod Stack's top right corner, and **Ctrl+←/→** changes
  the column for now. To choose the column the list starts with, use **Esc →
  Sort the mod list by**; **Auto** means the load order.
- **SigiLink ranking** is off until you answer its first-start question
  (**Use SigiLink** or **I like mod chaos**). Turn it on or off in **Esc →
  Rank automatically**. When it's on, SigilSmith ranks after you add or turn
  on mods. Mods that conflict with or need each other are put in an order
  that works (what they need first, then by how much they conflict, patch
  tags in the name, size, file count, how many mods need them and date); the
  other mods keep their places. With
  **Apply rankings without a preview** on (the default) it applies the new
  order at once; with it off, it shows the changes first (**Enter** or `Y`
  applies, **Esc** or `N` cancels). **Esc → Rank now** always shows them
  first.
- **Pins.** If you move a mod by hand while ranking is on, it's pinned
  (⇥ in the link column) and ranking leaves it where you put it. ⇄ marks a mod
  that ranking placed. The Context panel's `Unlinked: N` counts your pins.
  **Ctrl+R** unpins the selected mod; **Esc → Unpin all mods** (or **F12**)
  unpins them all.
- **Undo a reorder.** Each deploy saves a backup, so you can go back
  ([Restore a backup](#restore-a-backup)). To stop SigilSmith reordering for
  good, turn **Rank automatically** off.

## Two mods change the same file

When two or more enabled mods contain a file at the same path, the
**Conflicts** panel lists it (press **Tab** until it's active; the desktop app
has a **Conflicts** screen). By default the mod lower in the list wins. To pick
another winner:

- `↑` and `↓` choose a file, `←` and `→` change its winner, `1`-`9` picks one by
  number, and `p` picks from a list.
- **Enter** applies it now; otherwise it applies after a short pause.
- `c`, **Backspace** or **Del** clears your choice.

Your choices are saved with the profile and take effect when SigilSmith
deploys. The Context panel's **Conflicts** line counts the files left on the
load order (`Auto`) and the ones you chose (`Manual`).

In the desktop app, the **Conflicts** screen lists the files with the mods
that include them. Select a file, choose **Pick the winner…** and pick a mod
(it is marked **Your pick**); **Let load order decide** takes your pick back.
**Rescan** looks again.

Two other things use the word "override". The Kind `Override` in the Mod Stack
is a `.pak` with no mod ID of its own
([details](file-formats.md#what-sigilsmith-reads-from-mods)), and the **Target**
column (`1`-`5`) sets which game folder a mod's files go to. Leave Target on
`Auto` unless a mod's page says otherwise.

## Where is the log, and how do I share it?

The log is a plain text file:

```text
~/.local/share/sigilsmith/Baldur's Gate 3/sigilsmith.log
```

(`$XDG_DATA_HOME/sigilsmith/...` if you set it.) Each line starts with
`[INFO]`, `[WARN]` or `[ERROR]`. SigilSmith adds to it, so the newest lines
are at the end. Once the file passes 2 MiB, SigilSmith cuts it down to the
newest 1 MiB. The Log panel in the terminal app and the **Log** screen in the
desktop app show the lines from this session.

In the terminal app, at the bottom of the **Esc** menu:

- **Copy the last 200 log lines** is usually what a bug report needs.
- **Copy the log (this session)** copies everything since SigilSmith started,
  which can be long.
- **Save the log to a file…** asks for a folder and saves the whole log as
  `sigilsmith-log-<time>.txt` there.
- **Open the log folder** opens the folder the log is in.

The **Log** screen in the desktop app has the same actions: **Copy Last 200 Log
Lines**, **Copy Log (This Session)**, **Export Log File…** and **Open Log
Folder**.

If a copy fails, SigilSmith says so; save the log to a file, or take the file
itself.

**Before you post it:** the log contains folder paths, so it can show your user
name. Edit them out if you'd rather not share them. For a short log, quit
SigilSmith, delete the file, repeat the problem, and share the new one.

## Restore a backup

Every deploy saves a backup, and SigilSmith keeps the newest 100 (**Esc →
Backups to keep** offers 50, 100, 250 and 500). A backup you keep or make with
**Back up now** stays until you delete it. Press `b` (or **Esc → Backups:
restore or undo**) to list them, newest first; in the desktop app, open
**Settings → Backups**. Each one shows what restoring it would change.

1. Select a backup and press **Enter**. SigilSmith asks first, and the answer
   starts on **Cancel**.
2. Choose **Restore**. SigilSmith saves your current setup as **Before
   restore**, puts back the mod list, profiles and load order from that
   backup, and deploys.
3. To undo the restore, open the list again and restore **Before restore**.

Backups hold SigilSmith's own files, not the mods' files
([what's in a backup](file-formats.md#backups)). A mod whose files are gone
comes back marked missing and stays off. A restore waits while a deploy or
import is running, and says "Same as now: nothing to restore" when the backup
matches your setup.

## The AppImage won't start

If it prints this:

```text
dlopen(): error loading libfuse.so.2

AppImages require FUSE to run.
```

your system doesn't have FUSE 2, which AppImages use to open themselves. Pick
one:

- **Run it without FUSE.** Add `--appimage-extract-and-run`:

  ```bash
  ./sigilsmith-1.0.1-x86_64.AppImage --appimage-extract-and-run
  ```

  Setting `APPIMAGE_EXTRACT_AND_RUN=1` does the same. It unpacks the app to a
  temporary folder each time you start it.
- **Use the `.tar.gz` instead.** It's one program file and needs no FUSE
  ([install guide](install.md#targz-any-distro)).
- **Install FUSE 2.** The package name depends on your distro; the AppImage
  project lists them:
  [AppImage and FUSE](https://github.com/AppImage/AppImageKit/wiki/FUSE).

Double-clicking the AppImage opens the desktop app. If nothing appears,
start it from a terminal to see what it says. A window that can't open falls
back to the terminal app and says why; some system libraries may be missing
([System libraries](install.md#system-libraries)). For the terminal app, add
`--tui` ([install guide](install.md#appimage-works-almost-anywhere)).

## Fedora and openSUSE

SigilSmith 0.9.11 doesn't start on Fedora or openSUSE, from any of its
packages: it looks for the bzip2 library under a file name those distros don't
use (`libbz2.so.1.0`). SigilSmith 1.0.0 and later have bzip2 built in, so they start there;
install the latest release ([install guide](install.md)). The
[changelog](../CHANGELOG.md) has the details.

## The title bar or window size looks wrong

On KDE, and on GNOME 44 or newer under X11 or XWayland, your desktop draws the title bar, so it follows your desktop's
light or dark setting rather than SigilSmith's look.

SigilSmith's first window is 1440x900, or as much of your screen as it can use if the screen is smaller. On a small
screen GNOME may maximize it straight away, and on KDE Maximize and Restore can look like they do nothing because both
sizes are nearly the same. The smallest the window goes is 800x500.

## Steam Deck

Use Desktop Mode, and run the AppImage or the `.tar.gz`. SigilSmith opens as
a window. Its Controller layout, with big rows for the Deck's screen and
buttons, is in **Settings → Appearance → Layout**; **Automatic** picks it when
Steam's Game Mode runs SigilSmith. The steps are in the
[install guide](install.md#steam-deck). If mods don't show up in the game, see
[Which Larian folder does SigilSmith use?](#which-larian-folder-does-sigilsmith-use).

## Update checks

Once you've agreed at the **Check for updates?** question, SigilSmith checks GitHub for a new release
when it starts and when you open Settings. **Settings → Updates** in the window,
or **Esc → Check for updates at start** in the terminal app, switches between:

- **Check at start** (terminal app: **On**): it asks before it downloads anything.
- **Check and install by itself** (terminal app: **On, installs by itself**): it
  installs a new release by itself where it can (the AppImage and the `.tar.gz`),
  and asks otherwise.
- **Don't check** (terminal app: **Off**): nothing is checked until you choose
  **Check for updates**.

**Skip this version** hides one release until a newer one comes out. **Check for
updates** still offers it.

### The password prompt didn't appear or the update stopped

A `.deb` or `.rpm` update asks for your password through your system's own prompt
(polkit), not through SigilSmith.

- **No prompt came up:** your session has no polkit agent, or `pkexec` isn't
  installed. SigilSmith then shows the install command to run yourself.
- **Over ssh or on a bare console, your password was refused:** on some systems
  (we've seen it on Ubuntu and AlmaLinux) the text password prompt fails there even
  with the right password, and nothing is installed. Run the install command
  SigilSmith shows you, or update from a terminal on your desktop.
- **You closed the prompt:** nothing was changed. You can try again whenever you
  like.
- **"The install started but didn't finish":** something stopped the package tool
  partway. Check your package manager before you try again (on Ubuntu and Debian,
  `sudo dpkg --audit`; on Fedora, `sudo dnf check`). If Fedora lists SigilSmith
  twice, `sudo dnf reinstall` with the file from the card's command puts it right;
  `dnf install` says there's nothing to do.

The password goes to your system, never to SigilSmith.

- What each check sends, and what it doesn't:
  [What SigilSmith touches](what-sigilsmith-touches.md#network).
- How each install type updates:
  [install guide](install.md#updates).

## Reset SigilSmith's settings

Start with the smallest step.

1. **Game paths are wrong:** **Esc → Folders** (in the desktop app,
   **Settings → Game**).
2. **Badges or dependency counts look stale:** **Esc → Clear saved mod info**.
3. **Settings back to how SigilSmith starts out:** **Esc → Reset settings…**
   (in the desktop app, **Reset…** on a Settings page; choose **Reset this
   page** or **Reset all settings**). It lists what would change first. Your
   mods, load order, profiles, backups and folders stay. If SigilSmith won't
   start at all, quit it and delete `~/.local/share/sigilsmith/config.json`;
   the first-start questions (SigiLink, What's New) come back.
4. **Everything:** turn all your mods off and deploy, then delete
   `~/.local/share/sigilsmith/`
   ([what's in it](what-sigilsmith-touches.md#sigilsmiths-own-data)).

Don't delete the game's `Baldur's Gate 3/config.json` to fix paths. It also
forgets where you moved the mod store, and SigilSmith then looks for your mods
in the default place and shows them as missing. Use step 1.

**If SigilSmith stops at start with `Error: parse config.json`** (or `parse
library.json`), that file was damaged, and the line under it says what's wrong.
There are two `config.json` files: the app's, in `~/.local/share/sigilsmith/`,
and the game's, in `~/.local/share/sigilsmith/Baldur's Gate 3/`. For the app's,
delete it as in step 3. For the game's `config.json` or for `library.json`,
don't delete it: take the copy from your newest folder in
`~/.local/share/sigilsmith/Baldur's Gate 3/backups/` and put it over the file.
A backup keeps the library as `library.json.zst`; open it with `zstd -d`
([what's in a backup](file-formats.md#backups)).

**If it says a file is in a newer format** ("newer than this version of
SigilSmith reads"), an older SigilSmith met files that a newer one wrote.
Nothing was changed. Use the newer SigilSmith, or copy back the copy the
message names to go back to the older one
([format numbers](file-formats.md#format-numbers-and-upgrades)).

## Report a bug

Open an [issue](https://github.com/Agistaris/SigilSmith/issues) or comment on
the [Nexus Mods page](https://www.nexusmods.com/baldursgate3/mods/20561). The
more of this you include, the faster it gets fixed:

- Your SigilSmith version (`sigilsmith --version`), how you installed it
  (AppImage, `.deb`, `.rpm`, `.tar.gz` or on NixOS), and whether you used the
  desktop app or the terminal app.
- Your distro, and how you run BG3: Steam native, Steam with Proton, GOG,
  Heroic and so on.
- What you did, what you expected and what happened instead. The exact text of
  any error helps.
- The last 200 log lines (**Copy the last 200 log lines**), or the log file
  ([how](#where-is-the-log-and-how-do-i-share-it)). Check it for paths you'd
  rather not share.
- The output of `sigilsmith paths`, if the problem is about the game or Larian
  folder.
