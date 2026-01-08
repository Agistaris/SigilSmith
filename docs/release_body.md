# SigilSmith v0.4.4

SigilSmith is a Linux-first TUI mod manager for Baldur's Gate 3. Drag-drop mods, manage profiles, resolve overrides, and deploy with confidence. Multi-game support is coming next via an open adapter template.

## Highlights

- Auto-update check on startup (AppImage self-update, deb/tar downloads with instructions).
- Settings menu shows version + update status with manual check action.
- Guided path browser for BG3 install + Larian data directories.
- Path browser supports manual path entry with list/path focus switching.
- Settings menu includes Configure Paths and shows current config locations.
- Public repo now includes full source under Apache-2.0 with CI-built releases.
- Override Actions panel with debounced swap overlay for manual conflict picks.
- AI Smart Ranking preview (diff + scroll) for safer order suggestions.

## Screenshots

![Overview](docs/01-hero-overview.png)
![Profiles](docs/02-explorer-profiles.png)
![Overrides](docs/03-overrides-mode.png)
![Smart Ranking](docs/04-smart-ranking.png)
![Settings](docs/05-settings-menu.png)
![Import Toast](docs/06-import-toast.png)
![Directory Select](docs/07_directory_select.png)

## Install

Prebuilt Linux packages are attached to this release (AppImage, `.deb`, `.rpm`, `.tar.gz`).

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
- Loose files deploy to `Data/Generated` (or `Data`), and `.pak` files deploy to the Larian `Mods` directory.
- Support links (Ko-fi + GitHub Sponsors) are coming next update.

## Changelog

See `CHANGELOG.md` for the full list of changes.
