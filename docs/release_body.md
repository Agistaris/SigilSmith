# SigilSmith v0.7.9

SigilSmith is a Linux-first TUI mod manager for Baldur's Gate 3. Drag-drop mods,
manage profiles, resolve overrides, and deploy with confidence. Multi-game support
is coming next via an open adapter template.

## Highlights

- SigiLink cache with hardlink/symlink deploys (no full-copy fallback).
- Transactional imports with clear stage progress and safe cancel.
- SigiLink Intelligent Ranking with onboarding, pins, and deterministic diffs.
- Mod list interop: JSON export/import and modsettings.lsx export/import.
- Missing mod placeholders with safe enable/disable dependency prompts.
- Overrides panel redesigned for fast selection and scrollable lists.

## What's New Since 0.5.0

- SigiLink cache and index (fast, safe deploys with recovery tools).
- Ranking preview + auto-ranking with pins and restore hotkeys.
- Mod list import preview with missing/ambiguous handling.
- Dependency dialogs for enable/disable cascades and missing files.
- Dep counts (missing vs disabled) in the mod stack at a glance.
- Refined UI: aligned panels, wider settings, richer help, and better overlays.

## Creator Note

I worked tirelessly day and night on the new SigiLink cache and ranking system,
polishing the UX, correcting bugs, and testing edge cases. This release is ready,
and I have more planned snapshots (and a few secrets) waiting for the next one.

## Screenshots

![Overview](docs/01-hero-overview.png)
![Profiles](docs/02-explorer-profiles.png)
![Search](docs/02.5-search-names.png)
![Sort](docs/02.8_sort_by_name.png)
![Overrides](docs/03-overrides-mode.png)
![SigiLink Ranking](docs/04-smart-ranking.png)
![Settings](docs/05-settings-menu.png)
![Directory Select](docs/07_directory_select.png)

## Install

Prebuilt Linux packages are attached to this release (AppImage, `.deb`, `.rpm`,
`.tar.gz`).

Quick start:

```bash
chmod +x SigilSmith-*.AppImage
./SigilSmith-*.AppImage
```

From source:

```bash
cargo build --release
./target/release/sigilsmith
```

Checksums are included in `SHA256SUMS.txt`.

## Notes

- SigilSmith only manages files you provide; no game assets are bundled.
- Deploy uses the SigiLink cache with hardlink/symlink targets (no full-copy fallback).
- Loose files deploy to `Data/Generated` (or `Data`), and `.pak` files deploy to the Larian `Mods` directory.

## Changelog

See `CHANGELOG.md` for the full list of changes.
