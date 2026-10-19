# Install SigilSmith

SigilSmith runs on 64-bit x86 Linux with glibc 2.34 or newer: Ubuntu 22.04,
Debian 12, Linux Mint 21 and anything newer, Arch, plus current SteamOS on the
Steam Deck. It's a terminal app, so it opens in Konsole, GNOME Terminal or any
other terminal.

Fedora and openSUSE: 0.9.11 doesn't start there, from any of its packages. It
looks for the bzip2 library under the file name Debian, Ubuntu and Arch use
(`libbz2.so.1.0`), and Fedora and openSUSE name it `libbz2.so.1`. SigilSmith
1.0.0 fixes this; please wait for it.

Every package is on the
[latest release](https://github.com/Agistaris/SigilSmith/releases/latest),
with a SHA-256 checksum in `SHA256SUMS.txt`. The
commands below use 0.9.11; change the version to the one you downloaded.

## AppImage (works almost anywhere)

```bash
chmod +x sigilsmith-0.9.11-x86_64.AppImage
./sigilsmith-0.9.11-x86_64.AppImage
```

If you start it from a file manager, choose "Run in terminal" (or "Run in
Konsole").

## .deb (Debian, Ubuntu, Mint, Pop!_OS)

```bash
sudo apt install ./sigilsmith_0.9.11-1_amd64.deb
```

This installs `sigilsmith` in `/usr/bin` and adds SigilSmith to your app menu;
the menu entry opens it in a terminal. Run `sigilsmith` in a terminal to start
it from there.

## .rpm (Fedora, openSUSE)

The 0.9.11 `.rpm` can't be installed (see the note at the top); the `.rpm`
returns with SigilSmith 1.0.0.

## .tar.gz (any distro)

The archive holds one file, the `sigilsmith` program.

```bash
tar -xzf sigilsmith-0.9.11-linux-x86_64.tar.gz
./sigilsmith
```

To run it as `sigilsmith` from anywhere, move it to a folder on your `PATH`,
such as `~/.local/bin`.

## Steam Deck

Switch to Desktop Mode and use the AppImage or the `.tar.gz` from Konsole
(the `.deb` and `.rpm` are for other distros).

<!-- Arch Linux: add the AUR package here once sigilsmith-bin is settled
     with its maintainer (public issue #3). Until then the AppImage and
     .tar.gz work on Arch. -->

## Check the download

Download `SHA256SUMS.txt` from the same release into the same folder, then:

```bash
sha256sum -c --ignore-missing SHA256SUMS.txt
```

It prints `OK` next to each file you downloaded. Anything else means the file
is incomplete or isn't the one we published: download it again.

## First launch

On Steam, SigilSmith finds BG3 and its Larian data folder by itself, whether
the game runs natively or through Proton. If it misses something (GOG, Heroic,
Lutris, Faugus, Bottles or a custom location), press **Esc → Configure Game
Paths**. After the game folder, it lists the Larian folders it finds,
including those in Lutris, Heroic, Faugus and Bottles prefixes.

In the folder browser: arrow keys move, Enter opens a folder, Backspace goes
up, Tab lets you type a path, and `S` selects the current folder. Typing a
name jumps to it.

Your existing load order is read from the game's `modsettings.lsx` at first
launch, so an order from BG3 Mod Manager or the in-game mod manager comes
along.

## Updates

SigilSmith checks GitHub for a new release when it starts and when you open
Settings (at most every 10 minutes), and asks before doing anything: nothing
is downloaded until you choose **Update now**.
**Esc → Updates** picks:

- **Ask** (the default): shows what's new, with Update now, Skip this version
  or Later.
- **Automatic**: AppImage and `.tar.gz` installs update themselves.
- **Off**: no automatic checks. **Esc → Check For Updates** still checks
  when you ask.

The AppImage and `.tar.gz` replace themselves in place. For the `.deb` and
`.rpm`, SigilSmith shows the command to install the new package. Every
download must match its published SHA-256, or the update is refused.

## Uninstall

- AppImage or `.tar.gz`: delete the file.
- `.deb`: `sudo apt remove sigilsmith`
- `.rpm`: `sudo dnf remove sigilsmith` (or `sudo zypper remove sigilsmith`)

Removing the program leaves your library, profiles, settings and backups in
place, and the mods it deployed stay in the game. To take the mods out of the
game first, turn them all off in SigilSmith and deploy. Then, if you no
longer want them, delete SigilSmith's data folder `~/.local/share/sigilsmith/`
(`$XDG_DATA_HOME/sigilsmith/` if you set it) and its update downloads in
`~/.cache/sigilsmith/`. Mod list exports stay in
`~/Downloads/SigilSmith/exports/`. [What SigilSmith touches](what-sigilsmith-touches.md)
lists every folder it uses.
