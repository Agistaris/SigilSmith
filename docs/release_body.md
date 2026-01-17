# SigilSmith v0.9.0

Linux-first TUI mod manager for Baldur's Gate 3.

SigilSmith is a keyboard-first terminal UI for managing BG3 mods on Linux.
Drag-drop imports, profiles, overrides, SigiLink cache deploys, and intelligent ordering in one place.

## Requirements

- Baldur's Gate 3 installed (Steam native or Proton)
- Linux terminal (Konsole, GNOME Terminal, etc.)

## Features

- Fast, readable TUI layout with clear focus states and full-width striping
- Drag & drop import: .zip/.7z/.pak (and folders), with automatic target detection
- Profile explorer: create/rename/duplicate + import/export mod lists
- Overrides panel redesigned for conflict resolution: scrollable lists, fast winner selection
- SigiLink cache: hardlink/symlink deploys for fast updates (no full-copy fallback)
- Deploy control: debounced auto-deploy toggle + manual deploy on demand
- SigiLink Intelligent Ranking: onboarding, diff preview, "unlinked" pins, restore/reset hotkeys
- Mod list interop: SigilSmith JSON (full fidelity) + modsettings.lsx import/export (interop)
- Missing mod placeholders ("ghost" entries) to preserve intended order + safe dependency prompts
- Native mod.io entries shown inline alongside manual installs
- Auto-update checks with clear release notes
- Per-deploy backups with rollback support

## Install

Recommended (AppImage)
- chmod +x sigilsmith-0.9.0-x86_64.AppImage
- ./sigilsmith-0.9.0-x86_64.AppImage

Alternate formats
- sigilsmith-0.9.0-linux-x86_64.tar.gz
- sigilsmith_0.9.0-1_amd64.deb
- sigilsmith-0.9.0-1.x86_64.rpm

From source
- cargo build --release
- ./target/release/sigilsmith

## GitHub Release / Checksums

https://github.com/Agistaris/SigilSmith/releases/tag/v0.9.0

Every release includes SHA256SUMS.txt.

## Notes

- SigilSmith only manages files you provide; no game assets are bundled.
- Deploy uses the SigiLink cache and links files into the game directories (hardlink same-drive, symlink cross-drive).
- Loose files deploy to Data/Generated (or Data).
- .pak files deploy to the Larian Mods directory.
- Bin overrides deploy to BG3/bin when applicable.
- If paths are not detected: press Esc -> Configure game paths (browser opens).
  Use arrows to navigate, Enter to open/select, Backspace to go up, Tab to edit the path directly, S to select current folder.
- Full source is available on GitHub under the SigilSmith Community License v1.0
  (non-commercial use only; no redistribution or hosted services without permission).

## Roadmap

- Multi-game support via an open adapter template (coming next).

## Known Issues

- None known. If you hit anything, report it on GitHub.

## Credits

- Larian Studios for Baldur's Gate 3
- BG3 modding community for inspiration and testing
- saghm for the larian_formats crate used for BG3 LSPK + modsettings parsing
