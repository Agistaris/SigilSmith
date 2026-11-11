# Install SigilSmith

SigilSmith runs on 64-bit x86 Linux with glibc 2.34 or newer: Ubuntu 22.04,
Debian 12, Linux Mint 21, Fedora, RHEL or AlmaLinux 9, openSUSE Leap 15.6 and
anything newer, Arch, plus current SteamOS on the Steam Deck. (On NixOS, see
[NixOS](#nixos).)

Running `sigilsmith` opens the desktop app. `sigilsmith --tui` opens the
terminal app instead, in Konsole, GNOME Terminal or any other terminal; so
does a plain `sigilsmith` when there's no display (over SSH, on a text
console), and it says so.

Every package is on the
[latest release](https://github.com/Agistaris/SigilSmith/releases/latest),
with a SHA-256 checksum in `SHA256SUMS.txt`. The
commands below use 1.0.1; change the version to the one you downloaded.

## AppImage (works almost anywhere)

```bash
chmod +x sigilsmith-1.0.1-x86_64.AppImage
./sigilsmith-1.0.1-x86_64.AppImage
```

Double-clicking it in a file manager opens the desktop app. For the terminal
app, add `--tui` (`./sigilsmith-1.0.1-x86_64.AppImage --tui`). An AppImage
needs FUSE 2 and some system libraries; see [System libraries](#system-libraries).

## .deb (Debian, Ubuntu, Mint, Pop!_OS)

```bash
sudo apt install ./sigilsmith_1.0.1-1_amd64.deb
```

This installs `sigilsmith` in `/usr/bin` and adds SigilSmith to your app menu;
the menu entry opens the desktop app. Run `sigilsmith --tui` in a terminal for
the terminal app. `apt` installs the libraries the program needs; the window
libraries and a GPU driver come as recommended packages, so don't turn
recommends off.

## .rpm (Fedora, openSUSE)

```bash
sudo dnf install ./sigilsmith-1.0.1-1.x86_64.rpm
```

On openSUSE, use
`sudo zypper install --allow-unsigned-rpm ./sigilsmith-1.0.1-1.x86_64.rpm`: the
`.rpm` itself has no signature (the signed `SHA256SUMS.txt` covers it), so without
the option zypper stops and asks first. Like the `.deb`, it installs `/usr/bin/sigilsmith` and an app menu entry.

## .tar.gz (any distro)

The archive holds one file, the `sigilsmith` program.

```bash
tar -xzf sigilsmith-1.0.1-linux-x86_64.tar.gz
./sigilsmith
```

To run it as `sigilsmith` from anywhere, move it to a folder on your `PATH`,
such as `~/.local/bin`. It needs some system libraries; see
[System libraries](#system-libraries).

## Steam Deck

Switch to Desktop Mode and use the AppImage or the `.tar.gz` (the `.deb` and
`.rpm` are for other distros). SigilSmith opens as a window. Its Controller
layout, with big rows for the Deck's screen and buttons, is in **Settings →
Appearance → Layout**; **Automatic** picks it when Steam's Game Mode runs
SigilSmith.

## System libraries

The `.deb` and `.rpm` install what SigilSmith needs. The AppImage and the
`.tar.gz` are only the program, so the system has to provide:

- **To run at all:** glibc 2.34 or newer, `libgcc_s`, `libstdc++`, `libudev`
  (controllers), `libfontconfig` and `libfreetype`. Every desktop distro has
  these.
- **To open the window:** Vulkan (`libvulkan`) or OpenGL/EGL, `libxkbcommon`,
  the Wayland or X11 client libraries, and a GPU driver. Without them, or with
  no display, SigilSmith starts the terminal app and says why on stderr.

Package names, in case you want to install them by hand:

| Distro | To run | For the window |
| --- | --- | --- |
| Debian, Ubuntu | `libudev1 libfontconfig1 libfreetype6 libstdc++6` | `libvulkan1 libegl1 libgl1 libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 libwayland-egl1 libx11-6 libx11-xcb1 libxcb1 libxi6 libxrender1`, plus your GPU's driver |
| Fedora | `systemd-libs fontconfig freetype libstdc++` | `vulkan-loader libglvnd-egl libglvnd-glx libxkbcommon libxkbcommon-x11 libwayland-client libwayland-egl libX11 libX11-xcb libxcb libXi libXrender` |
| Arch | `systemd-libs fontconfig freetype2 gcc-libs` | `vulkan-icd-loader libglvnd libxkbcommon libxkbcommon-x11 wayland libx11 libxcb libxi libxrender` |

Archives (`.zip`, `.7z`, `.rar`, `.tar.*`, bzip2, xz and more) are opened by
code inside the program, so no archive tools or `liblzma` are needed.

## NixOS

We haven't tested SigilSmith on NixOS yet. These are the usual ways to run a
program built for other distros; please tell us what you find.

- **AppImage:** `appimage-run ./sigilsmith-1.0.1-x86_64.AppImage`
- **`.tar.gz`:** `steam-run ./sigilsmith`
- **Either, without a wrapper:** turn on `programs.nix-ld` and add the
  libraries from [System libraries](#system-libraries) (the
  [nixpkgs](https://search.nixos.org/packages) names for them include
  `systemd`, `fontconfig`, `freetype`, `stdenv.cc.cc.lib`, `vulkan-loader`,
  `libxkbcommon` and `wayland`; check the names against your nixpkgs).

For Wayland (on X11, add its client libraries too):

```nix
programs.nix-ld.enable = true;
programs.nix-ld.libraries = with pkgs; [
  systemd fontconfig freetype stdenv.cc.cc.lib
  vulkan-loader libxkbcommon wayland libGL
];
```

If the window can't open, `sigilsmith --tui` still gives you the terminal app.

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

`SHA256SUMS.txt` is signed. To check the signature too, see
[Check a download yourself](../SECURITY.md#check-a-download-yourself).

## First launch

On Steam, SigilSmith finds BG3 and its Larian data folder by itself, whether
the game runs natively or through Proton. If it misses something (GOG, Heroic,
Lutris, Faugus, Bottles or a custom location), open **Settings → Game** in the
desktop app, or **Esc → Folders** in the terminal app. After the game folder,
it lists the Larian folders it finds, including those in Lutris, Heroic,
Faugus and Bottles prefixes.

In the terminal app's folder browser: arrow keys move, Enter opens a folder,
Backspace goes up, Tab lets you type a path, and `S` selects the current
folder. Typing a name jumps to it.

Your existing load order is read from the game's `modsettings.lsx` at first
launch, so an order from BG3 Mod Manager or the in-game mod manager comes
along.

## Updates

SigilSmith asks whether to check for updates once your game folder is set up.
Until you choose, nothing is checked, and **Ask me later** asks again at the next
start. Once you agree, it checks GitHub when it starts and when you open Settings
(at most every 10 minutes). When a new release is out,
the window shows an **Update available** button (the last row of the rail; on
the Deck in the footer bar, on R3), and the terminal app says so in its status
line.

**Settings → Updates** in the window, or **Esc → Check for updates at start** in
the terminal app, picks:

- **Check at start** (terminal app: **On**): shows what's new, with Update now,
  Skip this version or Later. Nothing is downloaded until you choose
  **Update now**.
- **Check and install by itself** (terminal app: **On, installs by itself**):
  AppImage and `.tar.gz` installs update themselves. The `.deb` and `.rpm` still
  ask first, because this setting never opens a password prompt.
- **Don't check** (terminal app: **Off**): nothing is checked until you choose
  **Check for updates**.

Every download must match our signature and its published SHA-256, or the update
is refused. Then each install type does its own thing:

- **AppImage and `.tar.gz`:** they replace themselves in place, with no password.
  In a folder only root can write to, like `/opt`, SigilSmith shows the command
  to run instead.
- **`.deb` and `.rpm`:** your system asks for your password with its own prompt
  (polkit), and the package is installed with apt, dnf, zypper or rpm. SigilSmith
  never sees your password. If you close the prompt, nothing changes. If your
  system has no prompt, SigilSmith shows the command to run instead. Your normal
  package updater can update these packages too.
- **pacman (AUR):** your system updater looks after it, and SigilSmith shows a
  command you can copy.

Restart SigilSmith to start using the new version; it offers to do that for you.

## Uninstall

- AppImage or `.tar.gz`: delete the file.
- `.deb`: `sudo apt remove sigilsmith`
- `.rpm`: `sudo dnf remove sigilsmith` (or `sudo zypper remove sigilsmith`)

Removing the program leaves your library, profiles, settings and backups in
place, and the mods it deployed stay in the game. To take the mods out of the
game first, use **Undeploy mods** (**Settings → Game** in the desktop app,
**Esc → Undeploy mods** in the terminal app, or `sigilsmith undeploy`). Then,
if you no longer want them, delete SigilSmith's data folder `~/.local/share/sigilsmith/`
(`$XDG_DATA_HOME/sigilsmith/` if you set it) and its update downloads in
`~/.cache/sigilsmith/`. Mod list exports stay in
`~/Downloads/SigilSmith/exports/`. [What SigilSmith touches](what-sigilsmith-touches.md)
lists every folder it uses.
