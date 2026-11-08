# Changelog

## 1.0.0

Released: 7 November 2026.

- New: the desktop app. Running `sigilsmith` now opens a window when your session has a display (Wayland or X11). The terminal app is `sigilsmith --tui`. With no display (over SSH, on a text console) a plain `sigilsmith` opens the terminal app by itself and says so on stderr, and it does the same if the window can't open. The app menu entry no longer opens a terminal. The window has Mods, Conflicts and Log screens, your profiles in the side rail, and Settings pages for Game, Mods, SigiLink, Dependencies, Script Extender, Updates, Privacy, Backups, Appearance and About.
- The window has two layouts: Desktop, and Controller, with big rows for the Steam Deck and a TV. Settings → Appearance → Layout picks one; Automatic follows Steam's Game Mode. The first time you press a controller button in the Desktop layout, SigilSmith asks whether to switch (Offer the controller layout turns that off).
- Overrides is now Conflicts in both apps (the screen, the panel and the Context line). Your picks of which mod wins a file are the same choices as before, and they are still saved with the profile.
- Your files get a version number. The first start of 1.0.0 reads your 0.9.x `library.json`, both `config.json` files and `deploy_manifest.json`, writes them in the new format and keeps each old file beside the new one as `<name>.format-0.bak`; the log lists them. A file from a newer SigilSmith is never read as if it were current, changed or replaced. SigilSmith 0.9.x can't read the new files: it stops with an error and leaves them alone. The mod list export keeps its format. File details: file-formats.md.
- Fix: 0.9.11 did not start on Fedora and openSUSE, from any of its packages, because it looked for the bzip2 library under a name those distros don't use. bzip2 is built in now.
- New commands: `sigilsmith deploy` puts the enabled mods into the game folders. `sigilsmith undeploy` takes the mods out of the game folders (it removes the links SigilSmith made and the empty folders it created, and leaves the game's load order file as it is). `sigilsmith licenses` shows the open-source licenses (also `--licenses`).
- `sigilsmith.log` is trimmed to its last 1 MiB at start, once it has grown past 2 MiB.
- Updates install from inside the app. When a new release is out, the window shows an **Update available** button (the last row of the rail; on the Deck in the footer bar, on R3) and the terminal app says so in its status line, with the new version number. **Update now** downloads the release and checks its signature and its checksum. The AppImage and `.tar.gz` then replace themselves. The `.deb` and `.rpm` ask for your password through your system's own prompt, never one of ours, and install with apt, dnf, zypper or rpm; with no prompt on your system they show the command instead. pacman installs keep their own updater. **Check and install by itself** covers only the AppImage and `.tar.gz`, and never opens a password prompt. Nothing is checked until you've made a choice at the **Check for updates?** question, and command-line commands never check for updates.
- Packages run on older systems: glibc 2.34 or newer (Ubuntu 22.04, RHEL/AlmaLinux 9, openSUSE Leap 15.6). 0.9.11's packages needed glibc 2.39, so they did not start on Ubuntu 22.04 (its `.deb` installed and then failed). The `.deb` now lists its dependencies, and the `.rpm` requires its libraries by name.
- The terminal app's Esc menu now matches the window's Settings words: sentence-case labels (Deploy automatically, Ask before removing a mod, Sort the mod list by, ...), ON/OFF values in one column, and sections for Settings, SigiLink, Backups, Game specific and Debug. "Rank now (last: ...)" and "Unpin all mods (N pinned)" replace the two status lines.
- A settings change that cannot be saved (a full disk, an unwritable settings folder) no longer stays in effect until you quit: the setting goes back to what it was. The mod library size in Folders and Settings no longer follows links, so a link loop cannot hang it. Folders' long paths are cut by screen columns, so wide characters keep the end of the path in view. Esc in a folder picker opened from Folders returns to Folders during first-run setup too.
- Esc → Folders replaces Configure Game Paths and Display SigilSmith Paths in the terminal app: the game, Larian, mod library, downloads, settings file and backups folders in one list. Enter changes (the mod library moves), o opens, c copies the path.
- Esc → What goes online shows what SigilSmith sends over the network, with the same words as the window's Privacy page, and turns game pictures from Steam on or off. Esc → Reset settings... asks first, then puts every switch and choice back to how SigilSmith starts out, the window's Motion, Layout and Offer the controller layout included, the same set as the window's Reset.
- Saved backups: Back up now asks for an optional name and makes a saved backup, and Keep this backup saves an automatic one. Saved backups show a pin and their name, never count toward Keep and stay until you delete them; Delete asks first. In the terminal UI's Backups list, `k` keeps a backup, `n` backs up now and `d` deletes a saved one (`k` no longer moves up there; use the arrows).
- Backups to keep is 50, 100, 250 or 500 (default 100, up from 50). Lowering it asks before deleting the oldest automatic backups, and deletes only the ones it named. Going back to 0.9.x trims to its fixed 50 automatic backups; saved backups stay safe.
- Switching profiles no longer makes a backup unless "Back up when switching profiles" is on (off by default).
- The desktop app has a Backups settings page: a table of backups by day, what each one changed, a panel showing what restoring would change, Find a mod, and on the Steam Deck the same with controller buttons and Steam's keyboard.

- Settings search in the desktop app: typing in Search settings turns the page title into "Search results" and lists the matching rows of every page except Backups under their page's name, still working in place. The count beside the box reads "3 settings on 3 pages", the matched words get a soft highlight, and × clears the search (on the Steam Deck the search stays in the group column).
- On the Steam Deck the footer no longer cuts its status mid-word when there are many hints: it drops the file count first, then the time ("Deployed · 1,969 files · 2 min ago", "Deployed · 2 min ago", "Deployed").
- Fix: on the desktop, Settings pages used only part of the window's width (a blank column the width of the Details panel stood at the right).
- Game pictures: the desktop app can request a supported game's official cover and hero from Steam when this PC has no local copy. It is on by default, asks once before the first request (including upgrades that already chose an update mode), caches pictures in `~/.cache/sigilsmith/art/` or `$XDG_CACHE_HOME/sigilsmith/art/`, and can be turned off in Settings → Privacy. The TUI and command line never fetch pictures.
- Password-protected archives (zip, 7z and RAR): before a drop is added, SigilSmith asks for each locked archive's password ("Archive password · 1 of 2"), checks it in the locked-down extraction worker and asks again if it is wrong; Skip leaves that archive out. In the GUI it is a card with a masked field and Show/Hide (Y on the Deck); in the TUI a dialog with Tab to show or hide. The password is used once and never saved or logged, and a message that would contain it is withheld. The command line skips locked archives and says to add them in the app.
- Archives now open in more compiled-in formats: 7z Deflate64, Brotli and LZ4; tar, tar.gz/tgz, tar.xz/txz, tar.bz2/tbz2 and tar.zst; single gz/xz/bz2/zst files; CAB; and LHA/LZH.
- Archive extraction no longer invokes outside programs. A compression method the compiled-in readers cannot decode is refused with plain repacking advice.
- RAR archives are now opened by SigilSmith itself through compiled-in UnRAR.
- New logo: the hammer-and-sigil mark replaces the chain link. The GUI's sidebar and First run screen show the small mark on its own (no blue tile), the About page's header shows the full logo at the left, with the name, the version line and the tagline beside it and no border (its background fades out to the right), and the window and the packages (.deb, .rpm, AppImage, AUR) use it as their icon.
- Archive safety: all compiled-in archive readers run in a locked-down child copy of SigilSmith, so a decoder crash, memory limit or timeout becomes a clean import error instead of taking down the app. Linux workers require kernel 5.13+ with fully enforced Landlock V1 and use resource limits plus an all-thread syscall allow-list; the parent independently audits, measures and safely removes hostile output, and workers die with their parent. Windows uses a memory-limited kill-on-close job without crash dialogs and macOS uses resource limits. Unsupported compression methods are refused whole; no outside extractor is attempted.
- Terminal safety: a mod's name or an archive's file name can no longer send your terminal commands or fake a line of our output. The terminal app and the command line clean everything they draw and print, and `--format json` writes those characters as escapes.
- Import safety: an import or a deploy now checks a mod's own details (its id and folder name) before touching any file. A name that points outside SigilSmith's and the game's folders is refused with a message, a replacement either finishes or leaves the old mod as it was, and deploy leaves your own files in the game's folders alone, keeping any it has to move aside so that undeploy can put them back.
- A command-line import that fails now exits with an error code. Replacing a mod from the command line without `--deploy` takes it out of the game until you run `sigilsmith deploy`, and says so. `deploy` and `undeploy` refuse options they don't know instead of ignoring them.
- The command line refuses what it can't do instead of guessing: a `--format` that isn't `json` or `text` (or has no value) is an error, and so is `--deploy` or `--no-deploy` without `--import` (the message points to `sigilsmith deploy`).
- Archive review fixes: Unix output traversal now saves bounded entry-name buffers instead of resuming closed directory streams, keeping descriptor use constant and attempting failed removals only once per pass. Live checks refuse unreadable subtrees; startup also sweeps abandoned extraction setup directories without touching locked runs or stopping at an individual entry error. Unverified unsupported methods, excessive folder depth and unavailable free-space readings now give the appropriate manual-extraction or drive-check advice.
- Fix: in the terminal UI the scrollbar of the mods list veered off its column in some terminals. The link column used chain and ghost emoji that those terminals draw two cells wide. It now uses plain one-cell marks: ⇄ (ranked by SigiLink), ⇥ (manual pin) and ∅ (the mod's .pak file is missing). The help, the legend and the troubleshooting page say the same.
- About SigilSmith in the TUI: Esc → About SigilSmith (it replaces Support SigilSmith) opens a full About page with the same words as the GUI: the name, version and tagline, a thank-you note, GitHub Sponsors, Ko-fi, PayPal and Nexus Mods donations, links, the licence and the open-source notices. Up/Down move, Enter opens a link in the browser, c copies it and q shows its QR code (beside the list in a window 100 columns wide or more, in a popup otherwise). The help's About section points there.
- Fix: in the Order view a "(missing)" placeholder sat at the bottom of the list (5, 7, 8, 9, 10, then the placeholder that is #6), in either direction. In Order every row, a placeholder too, now sits at its real place in the load order, ascending and descending. In the other sorts a placeholder still comes after the other rows.
- Fix: in a window under about 45 rows the Explorer box (the game and its profiles) was squeezed to its title line or not drawn at all, because the Context box took 36 rows first (it was cut to one row at 140 × 40 and gone at 120 × 36). The Explorer now gets the rows its tree needs first, down to the smallest window, and the Context box takes the rest; in a short window it is the Legend and Hotkeys at the Context box's foot that are cut, never its own rows.
- Fix: the sort is named the same way everywhere. The Mod Stack header showed "Sort: Mod Name ↑", but the status line and the "Move requires Order" question said "(asc)" or "(desc)"; both now show the arrow, such as "Sort: Mod Name ↓" and "Can't move while sorting by Mod Name ↓."
- Fix: the Remove Mod question opened with "Remove & update cache" selected, so pressing Enter right away deleted SigilSmith's stored copy of the mod. It now opens on Cancel, like the other remove questions; Esc still cancels. Its text no longer says Remove "leaves a !ghost entry" but "leaves a placeholder in the list" (the same for a native mod).
- Esc → Copy Log To Clipboard is now "Copy Log (This Session)": it copies only the lines SigilSmith wrote since it started (or since you switched games), not the whole log file, which can reach 2 MB. If the log file was trimmed or deleted in the meantime, it copies what is there and says so; with no lines yet it says that instead of copying nothing. Copy Last 200 Log Lines and Export Log File are unchanged.
- Fix: the terminal UI crashed in a narrow window (at 34 columns, for one) and could on others; drawing no longer panics at any window size, in any screen, dialog or the Esc menu. Below 80 columns by 18 rows, where the mod list no longer fits, SigilSmith shows "SigilSmith needs a bigger window" and, under it, "at least 80 × 18 (now W × H)" in the middle of the window and keeps running; it comes back as soon as the window is big enough. While it shows, keys do nothing (so nothing is answered or moved out of sight) except Ctrl+C, which quits.
- Esc → Default Sort Column: Auto now always means Order (the load order) and is shown as "Auto (Order)". Before, it meant Mod Name while SigiLink ranking was on, so switching SigiLink on or off in the Esc menu (or answering its first-start question) changed how the list was sorted. The sort no longer changes with SigiLink. A column you picked in Default Sort Column is kept, and Ctrl+←/→ still changes the sort for the session.
- Fix: Del on a "(missing)" placeholder row (a place kept for a mod SigilSmith doesn't have) did nothing. It now asks whether to remove the placeholder from the list and, on Remove, takes it out of every profile. No files change. Before, only "Remove & update cache" on the mod itself dropped the row.
- Fix: the Esc menu scrolls. On a short terminal the items below the bottom of the screen (the log items and What's New?) could be neither reached nor seen. The menu now scrolls to keep the selected item in view, and a scrollbar on its right edge shows that there is more above or below.
- Update checks are opt-in: the first time SigilSmith starts it asks whether to check for updates (check and ask before installing, check and install by itself, or don't check), and nothing is checked by itself until you say yes. Check For Updates still works at any time. Settings from earlier versions that chose Automatic or Off keep it; those on the default (Ask) are asked once.
- Fix: SigiLink ranking ignored the dependencies a pak lists in its meta.lsx, so a mod could end up loading before a mod it needs. Ranking now puts each pak after the installed mods it depends on, found by UUID, or by the mod's full name if its UUID changed (never by a single shared word). The ranking's Dependencies list shows mod names instead of raw IDs.

## 0.9.11

- Fix: some paks name other modules' UUIDs inside their meta.lsx (KaiLime UI is one), and SigilSmith imported those mods under the wrong ID. The in-game mod manager's copy then showed up as a second entry, and the load order could reset. SigilSmith now reads the mod's own UUID. On first start it fixes mods imported with a wrong ID: it backs up the library, renames them, merges the duplicate entry they caused (the mod stays on if either copy was on), and deploys again.
- Moving between the native Linux build and Proton: when Steam starts BG3 with a different Larian folder than SigilSmith's, SigilSmith asks to move your setup there instead of switching on its own. A move backs up the library, deploy record, both load orders and settings, then takes SigilSmith's links out of the old Mods folder and makes them in the new one, so each mod stays one file in SigilSmith's library. The load order comes along, in-game mods turned on only in the new folder join your list, and saves only the old folder has can be copied over (nothing is overwritten). In-game mods the new folder hasn't downloaded yet stay on your list and join its load order once the game downloads them. A move cut short is offered again at the next start, with Finish or Go back; Go back also puts back the new folder's own load order from the backup. "Don't ask again for this folder" stops the prompt, and Configure Game Paths uses the same move.
- Deploys only remove files that are still SigilSmith's own links, checked by the link's target or the hardlink's file identity; anything changed since is left in place with a warning. A file already in the way is moved to `set-aside/` in SigilSmith's data folder and put back when SigilSmith's link goes away, even if a deploy was cut short. Quitting during a deploy waits for it to finish, and deploys wait for the startup read of the game's load order.
- Startup checks the last deploy: links that went missing, or that are still in a folder SigilSmith isn't set to, are fixed by deploying again. Links to SigilSmith's library that no deploy recorded (left from an earlier setup) can be removed after asking.
- Enabled mods with no file in the Mods folder are left out of modsettings.lsx and listed in the log, instead of written as entries BG3 drops. The native mod sync no longer reads SigilSmith's own modsettings write as a change.
- After a crash, BG3 leaves `ModCrashSanityCheck` in its Larian folder and turns every mod off on the next launch. SigilSmith removes it at startup and on deploy, only while BG3 isn't running.
- Script Extender: BG3LE, the Script Extender for the native Linux build, is detected, and the checklist then looks for it in Steam's launch options instead of asking for Proton. For Lutris, Heroic, Bottles and other launchers, the checklist says which setting is up to the launcher and copies the DWrite override; SigilSmith never changes other launchers' settings.
- Configure Game Paths now always asks for the Larian folder after the game root, starting at the one Steam uses, and lists the folders it finds: Steam's native and Proton folders, and BG3 prefixes from Lutris, Heroic, Faugus and Bottles (read only). Folders inside a Wine prefix are shown by their prefix (such as `…/compatdata/1086940`), here and in the move dialogs, so long paths stay readable.
- In-game mods the new folder hasn't downloaded yet no longer open the "SigiLink missing mod files" popup after a move; they are listed in the log and marked missing until the game downloads them. Settings no longer reopens over a move's results, and the What's New banner no longer ends in "...".
- Backups also keep `config.json`, so each backup records which Larian folder it was for.

## 0.9.10

- Backups list: `b` (or Settings → Backups) lists the newest 50 backups with when each was made, what started that deploy, and a color-coded summary of what restoring it would change; a details pane lists the mods under labels (Moved, On, Off, Re-added, Removed, and so on). Restoring asks first, shows the full list, defaults to Cancel, and saves your current setup as "Before restore" so it can be undone from the same list. Before, `b` restored only the last backup, with no prompt.
- A restore takes the order, on/off state, targets, override choices and profiles from the backup, and keeps today's mod details (names, dependencies, dates). Mods whose files have since been deleted stay off. A restore deploys even with Auto-Deploy off.
- SigilSmith keeps the newest 50 backups and deletes older ones (each deploy writes one, so they could reach hundreds of megabytes). Two backups made in the same second no longer overwrite each other.
- Updates ask first: a popup lists the new version's highlights with Update now, Skip this version, or Later, and Restart now starts the new version. Before, every check downloaded the update in the background. Settings → Updates picks Ask (the default), Automatic (AppImage and .tar.gz installs update themselves), or Off.
- Update downloads must match their published SHA-256, or the update is refused; before, a release without SHA256SUMS.txt installed unchecked. AUR installs and source builds are left to pacman and git, and .deb and .rpm updates show the command to install them.
- The mod list marks mods that use the Script Extender (`SE`) or ship Osiris story scripts (`Os`), found from the same files BG3 Mod Manager checks. Details shows the Script Extender version and features a mod needs; `mods list` shows the markers, and its JSON gains `scripts`. Existing libraries rescan once on first start.
- Script Extender setup: SigilSmith checks that Steam runs BG3 through Proton, that `DWrite.dll` is in the game's `bin` folder, and that Proton loads it. Enabling a mod that needs it opens a checklist with "Set up for me", which downloads the latest Script Extender from Norbyte's GitHub releases (checked against GitHub's SHA-256), puts `DWrite.dll` in `bin` and turns on the DWrite override in the Proton prefix (backing up `user.reg` first), and "Copy launch option" for anyone who prefers `WINEDLLOVERRIDES="DWrite.dll=n,b" %command%`. SigilSmith never edits Steam's own settings and won't set up while BG3 is running. Settings gains "Script Extender Setup" and "Script Extender Notice".
- Fix: paks in the older LSPK v15/v16 format (KaiLime UI is one) import as Pak mods; before, they were imported as override paks in the Data folder and shown as Loose. A mod already imported that way gets a "Remove and import again" note in Details.
- Override paks (a .pak with no meta.lsx, deployed to the game's Data folder) show as "Override" instead of "Loose", and choosing a target a mod can't use says why instead of "Target not present for this mod".
- The mod list keeps names readable in narrow windows by hiding Created, Added, Target, then Kind first (Details still shows them).
- File pickers: type a name to jump to it, as in a desktop file manager. The same letter again cycles through matches, and `/` keeps the typed text until Esc.
- Clear target overrides is Shift+C, like the other bulk actions (A/S/X); Ctrl+C no longer clears them. Leaving move mode any other way places the mod as m/Enter does, and deploys and SigiLink auto-ranking wait until a move is finished.
- Dates and times show in your local time zone instead of UTC, "time ago" goes on to months and years, and dialogs size themselves to their text so long lines are no longer cut off.
- Help gains a "Mod List Markers" section explaining every icon, and Help and the side legend list the Overrides panel and file picker keys correctly.
- Fix: the startup native mod sync no longer marks an outdated metadata cache as current, which could skip filling in dependencies and dates.
- Packaging: the RPM states the SigilSmith Community License (it said MIT), installs the LICENSE file, and builds in the v4 format with rpmbuild 6.

## 0.9.8

- Enable/disable dependency prompts get an "Only this mod" button (`O`) that changes just the selected mod(s), leaving dependents or required dependencies as they are. The prompt warns that the mod or its dependents may not work; the default stays the safe choice.
- Duplicate and similar-mod import prompts get a "Keep both" button (`O`) that adds the new mod next to the existing one when their IDs differ; the command-line import prompt gains `[b]oth`.
- Treat DiceSet_05 and DiceSet_07 as built-in game modules, so mods that list them no longer report missing dependencies.
- Loose mods keep the dependencies listed in their meta.lsx or info.json when imported; before, they showed no dependencies at all.
- Cleaner mod names: when a pak file name is generated (a trailing UUID, or the in-game mod manager's `name_1af5b-i3zl` form), show the mod's own name instead, and drop trailing UUIDs from native mod names.
- Packaging: `SHA256SUMS.txt` lists only the packages from the current build.
- CI: the release workflow runs only when started by hand.

## 0.9.7

- Pick the Larian data dir from Steam's per-game compatibility tool: BG3 forced to Proton uses its compatdata prefix; native runs keep `~/.local/share/Larian Studios`.
- Search every Steam library, plus Flatpak and Snap Steam, for the game and its Proton prefix.
- Warn at startup when the configured Larian data dir is not the one Steam launches BG3 with.

## 0.9.6

- Warn when enabling duplicate-named mods and offer to disable other enabled copies.

## 0.9.5

- Write Patch 7/8 modsettings using the enabled mod order (Mods list reflects active mods).
- Skip native sync when modsettings is empty to avoid disabling all mods.
- Auto-disable native duplicates when a managed mod with the same name is enabled.

## 0.9.4

- Treat Patch 7/8 modsettings order as the `Mods` list order (ModOrder fallback only).
- Add a startup warning when the Larian data directory is symlinked.
- Expand debug cache output with modsettings version and ModOrder presence.

## 0.9.3

- Update toolchain/dependencies for release build compatibility.

## 0.9.2

- Remove raw dependency byte-scans that caused phantom UUID requirements.
- Improve native .pak resolution by UUID to avoid false missing-mod warnings.
- Preserve modsettings import order when ModOrder is missing.
- Label native mods as "Native (Larian Mods)" unless they are mod.io published.

## 0.9.1

- Update license to SigilSmith Community License v1.0 (non-commercial; no redistribution/hosting).

## 0.9.0

- Refresh release docs and screenshots for the 0.9.0 rollout.
- Update SigiLink reset-all hotkey to `F12` and sync the help text.
- Center the Native sort header and align the missing-mod legend row.

## 0.8.7

- Fix backspace handling in profile rename input so edits persist.

## 0.8.6

- Fix Ctrl+Shift+R to reset all SigilLink pins in the mod stack.
- Skip redundant native .pak meta reads when modsettings are unchanged to speed cold syncs.

## 0.8.5

- Add auto-deploy onboarding + settings toggle and reflect it in the context panel.
- Expand README/release docs with deeper feature explanations and updated screenshots.

## 0.8.4

- Replace the log status bar animation with a compact braille spinner.
- Expand context hotkeys and align the missing-mod legend row.
- Refine the export modal footer styling and spacing.

## 0.8.3

- Clarify export options (SigilSmith JSON recommended vs. interop modsettings.lsx) and group help text.
- Reduce mod list preview modal height to better fit content.
- Keep the log status badge compact without a full-width footer band.

## 0.8.2

- Surface the missing mod file legend row and expand the context panel to keep hotkeys visible.
- Improve modsettings.lsx imports when Enabled flags are absent (fallback to ModOrder).
- Style the mod list preview hotkeys and slim the log status badge.

## 0.8.1

- Update path browser UX (list-first focus, Enter/Space select, dynamic tab label, better paste handling).
- Respect Enabled flags in modsettings imports and native sync.
- Move the status badge into the log footer with a subtle separator.

## 0.8.0

- Adjust mod stack spacing and header status layout polish.

## 0.7.9

- Move status panel to the header and reclaim vertical space for the main layout.
- Keep SigiLink ranking backups aligned with file overrides.
- Reduce trailing animation banding on mod-name loading overlays.
- Refresh public docs and release notes for the SigiLink era.

## 0.7.8

- Add additional spacing between the On and order columns in the mod stack.
- Keep Context values aligned with Legend/Hotkeys and lock the Legend section to 4 rows.
- Extend the overrides panel height and add a scrollbar + “↓ more” indicator.
- Darken Help section header text while keeping the blue highlight.

## 0.7.7

- Refine loading overlay spacing for mod names.

## 0.7.6

- Update license to permission-required source-available terms.

## 0.7.5

- Align context labels independently of hotkey widths.
- Soften Help section headers and stripe Help rows across full width.
- Keep Esc menu open after closing submenus.
- Remove now leaves a ghost entry (non-delete) and clarifies delete wording.
- Ignore base dependency labels with .pak suffixes and animate loading gaps in names.

## 0.7.4

- Title-case the Context/Legend/Hotkeys panels and the Help/Settings menus.
- Add SigiLink Debug log actions (copy last 200 lines, copy full log, export log file).
- Skip missing (ghost) entries for enable/disable/invert visible hotkeys.

## 0.7.3

- Default SigiLink auto-accept diffs and keep auto ranking running after enable/disable and reorder changes.
- Auto-apply override winner selection after a 5s debounce.
- Move missing-mod markers to the link column and keep names clean.

## 0.7.2

- Redesign overrides panel with fast scrolling, inline winner selection, and a candidate picker.

## 0.7.1

- Nudge left-side panel widths for improved layout balance.

## 0.7.0

- Treat missing .pak dependencies as missing (not disabled) to block auto-enabling ghost requirements.
- Only auto-prompt missing .pak files for enabled mods; missing entries still render with a strike/marker.

## 0.6.9

- Treat missing .pak mods as disabled in the mod stack (toggles + counts) and surface a missing-files queue with Nexus links.
- Improve native .pak filename matching to resolve truncated Mod.io filenames.

## 0.6.8

- Polish the settings menu layout (centered header, ON/OFF toggles, selected action highlight).
- Keep SigiLink status visible in the context panel and reduce spurious SigiLink relocation prompts.

## 0.6.7

- Add SigilLink Intelligent Ranking toggle with onboarding, per-mod pins, and diagnostics, plus ranking UI cues and hotkeys.

## 0.6.6

- Add mod list preview with destination/mode options plus missing/ambiguous summaries; unresolved entries render as grey "(missing)" placeholders that are ignored by deploy and auto-resolve on sync.
- Export mod list JSON + clipboard and add modsettings.lsx interop export with atomic writes.
- Clean up loose-file display names by stripping common Nexus suffixes and refresh settings menu cache actions/order.

## 0.6.5

- Label the Dep column in the header and legend.
- Hide leading zeroes in the Dep column and color missing/disabled counts separately.
- Remove mod dialog adds Cancel and defaults to Remove in the center option.

## 0.6.4

- Show missing/disabled dependency counts in the D column.
- Confirm before disabling dependents or enabling required dependencies.
- Remove mod now offers Remove vs Remove + delete file(s) with safe-path checks.

## 0.6.3

- Rename profile list wording to mod list and surface Ctrl+E/Ctrl+P mod list import/export hotkeys.
- Add a path browser with manual path entry for mod list import/export and a default export path.
- Settings menu adds Clear System Caches plus enable-after-import and delete-files-on-remove toggles.
- Align legend/hotkey panel layout and use uppercase A/S/X actions for visibility.
- Show a friendly error when importing unsupported files instead of crashing.

## 0.6.2

- Fix native .pak filename resolution for truncated UUID names so dependency metadata is detected.
- Preserve native mod dependencies during native sync updates and adoption.
- Fix Cancel Import dialog focus so the dependency queue no longer steals input.
- Refresh dependency cache after native sync dependency updates.
- Vary mod stack loading dot animation speed and pauses during cache/metadata sync.
- Parse ModuleShortDesc dependency entries so dependency names show alongside UUIDs.
- Filter base-game dependency UUIDs from fallback scans so Gustav/Honour modules do not appear as missing.
- Dependency queue now scrolls fully and includes a selectable override row with confirmation.
- Dependency queue hotkeys updated: Ctrl+C copies link, C copies UUID.
- Ignore base-game/system dependency UUIDs (Gustav, Shared, CrossplayUI, etc.) in dependency parsing.
- Strip ignored dependencies even when cached to avoid false missing warnings.
- Self-heal missing pak checks now resolve truncated filenames before disabling targets.
- Enabled marker uses warning color when overrides or missing dependencies are present.
- Import no longer pauses for dependency prompts; missing dependency counts update after import.
- Ignore UUID-only/underscore dependency tokens to reduce false missing warnings for cosmetic mods.
- CLI adds `deps resolved` report and aligns missing-deps detection with alias matching.
- Deploy now hardlinks managed .pak files (Vortex-style) with copy fallback.

## 0.6.1

- Smart-rank cache updates incrementally on toggles, moves, imports, and removals with per-mod scan reuse, plus debug cache validation and simulation helpers.

## 0.5.1

- CLI import now accepts multiple paths per `--import`, with `--deploy`/`--no-deploy` and standard verbosity flags (`-v/-vv/-vvv`, `--verbosity`).
- Batch import support for directories/archives containing multiple mods, including nested archives and top-level mod folders.
- Import progress overlay with hazy background and center gauge, plus a post-import failure summary dialog.
- Duplicate import prompts now default to newer releases and offer an “apply to all” toggle.
- Dependency queue for missing requirements, with downloads folder setup and optional warnings.

## 0.5.0

- Native mod entries resolve .pak filenames (including spaces) and stop overwriting Created dates when metadata is missing.
- Mod stack search bar with `/` or `Ctrl+F`, debounced preview, and inline clear hints; global shortcuts focus the mod stack.
- Mod stack sorting with visible indicator, Ctrl+arrow cycling, and a guard dialog when moving while sorted/filtered.
- Help overlay and context panels refined with stable legend/hotkeys layout and clearer headers.
- PageUp/PageDown now page through the mod list when focused.
- Import pipeline improvements for script extender-style archives and override `.pak` files without `meta.lsx`.
- Ignore `.git` and `.vscode` folders when importing/scanning/deploying loose files.
- Update checks fixed from the settings menu.
- Refresh release headers and screenshots for the 0.5.x series.

## 0.4.8

- Reconcile modsettings entries without managed storage to resolve native .pak filenames (incl. spaces) and timestamps.
- Add a mod search bar with `/` or `Ctrl+F`, debounced preview, Enter to apply, and inline clear hint while active.
- Add mod stack sorting with a highlighted sort indicator, column cycling, and a guard dialog when trying to move while sorted/filtered.
- Add a `?` help overlay with full hotkeys, padded modal layout, and a visible scroll indicator.
- Separate legend symbols from hotkeys with full-width headers, softer styling, and debounced fade-in; global hotkeys stay fixed at the top with an N/A legend placeholder and pastel text headers.
- Keep legend height stable across views and avoid overwriting native mod Created dates with filesystem timestamps when metadata is missing.
- Keep context label alignment consistent across views and format blank dates using the locale's date order.
- PageUp/PageDown now page through the mod list when focused.
- Add Ctrl+Alt+V clipboard paste support in input fields.
- Remove implicit import mode activation on unassigned keys.
- Import override `.pak` files without `meta.lsx` by installing them to `Data/`.
- Detect script extender-style bin-root archives (e.g. `DWrite.dll`) during import.
- Ignore `.vscode` folders alongside `.git` when importing/scanning/deploying loose files.
- Fix update checks triggering from the settings menu.

## 0.4.7

- Auto-restart after applying updates (AppImage/tarball).

## 0.4.6

- Allow applying ready updates from the settings menu.
- Auto-apply tarball updates when the install directory is writable.

## 0.4.5

- Fix update check state persisting after completion/timeout.

## 0.4.4

- Settings menu now shows app version + update status and supports manual update checks.
- Update availability surfaces in settings with clear status messaging.

## 0.4.3

- Auto-update check on startup with AppImage self-update and deb/tar instructions.
- Update downloads are verified with SHA256SUMS before applying.

## 0.4.2

- Added guided path browser for BG3 install + Larian data directories.
- Path browser now supports manual path entry and list/path focus switching.
- Removed accidental path hotkey; configure paths from the menu.
- Setup flow now shows clear status + auto-detect toast on success.
- Public repo now includes full source with a permission-required license.
- Startup now syncs enabled state for all pak mods from modsettings.

## 0.4.1

- Setup onboarding improvements with clearer status and path auto-detect retry.
- Settings menu now includes a "Configure game paths" action plus path display.
- Context + Explorer details show config path and setup hints.
- Added Nexus readme + header banners for release pages.

## 0.4.0

- Major UI refresh with refined header/footer, full-width striping, and cohesive panel layout.
- Overrides workflow polish: dedicated Override Actions panel, debounced swap overlay, clearer legends.
- AI Smart Ranking preview with scrollable diff view, dates, conflict stats, and warnings.
- Metadata accuracy: Created dates now read from meta.lsx/info.json (incl. Zstd paks) with background refresh.
- Native mod sync improvements + self-heal for missing pak files before deploy.
- Import pipeline updates (zip/7z detection + loose metadata persistence).

## 0.3.3

- Adds AI Smart Ranking preview (pak + loose scan) with apply/cancel.
- Adds a release notes helper script for release-only publishing.

## 0.3.2

- Adds a swap overlay in Override Actions during deploy.
- Adjusts scrollbar positioning to align with viewport edges.
- Tightens the settings menu layout for a more compact modal.

## 0.3.1

- Syncs native BG3 mod.io entries from modsettings into the library.
- Adds a Native column in the mod stack and source info in Details.
- Native mod removal can optionally delete the local .pak file.

## 0.3.0

- Overrides focus no longer tints the details/log panels; focus is shown via border + banner.
- Overrides banner gains a subtle inline highlight behind its text.

## 0.2.9

- Left panels now match the dark blue log tone.
- Mod stack background uses a neutral dark grey.

## 0.2.8

- Left panels now use a cohesive dark-blue background.
- Mod stack background is darkened for stronger contrast.

## 0.2.7

- Override focus now tints the details + log pair instead of the bottom bar.
- Bottom overrides banner uses readable accent styling without the highlight band.

## 0.2.6

- Details and log now split evenly for a balanced lower row.
- Details panel background matches log, with override focus highlight.

## 0.2.5

- Details/log row now splits horizontally to reclaim vertical space.
- Override actions border uses the focused highlight color.

## 0.2.4

- Override actions border now uses the light-blue highlight when focused.
- Context panel includes a spacer line before the legend block.
- Explorer tree uses unicode branch/expand glyphs for a cleaner look.

## 0.2.3

- Details panel now uses the same border style as the log.
- Target column width is computed from content and the header is renamed.
- Context values align with legend key widths for cleaner scanning.

## 0.2.2

- Overrides focus highlighting and banner readability updates.
- Details panel header now reflects override actions when focused.
- Bottom status area shares overrides highlight for a cohesive focus band.
- Mod stack column spacing tuned for shorter path labels.

## 0.2.1

- Overrides UX refresh: summary banner, legends, and context counts.
- Details panel repositioned under main panels with unified styling.
- Confirmation dialogs and settings menu refinements.
- Mod stack path labels shortened for clarity.
- Status bar progress indicator and centered text.

## 0.2.0

- UI refresh with cleaner header/footer layout and full-width row striping.
- Explorer profile actions and rename flow improvements.
- Mod stack table and override presentation refinements.
- Legend and details formatting updates for quick scanning.
- Toast and status messaging tuned for clarity.
