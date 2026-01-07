# SigilSmith v0.4.1

SigilSmith is a Linux-first TUI mod manager for Baldur's Gate 3. Drag-drop mods, manage profiles, resolve overrides, and deploy with confidence. Multi-game support is coming next via an open adapter template.

## Highlights

- Setup onboarding improvements with clearer status and path auto-detect retry.
- Settings menu now includes a Configure Paths action plus config path display.
- Context + Explorer details show config path and setup hints.
- Override Actions panel with debounced swap overlay for manual conflict picks.
- AI Smart Ranking preview (diff + scroll) for safer order suggestions.

## Screenshots

![Overview](docs/01-hero-overview.png)
![Profiles](docs/02-explorer-profiles.png)
![Overrides](docs/03-overrides-mode.png)
![Smart Ranking](docs/04-smart-ranking.png)
![Settings](docs/05-settings-menu.png)
![Import Toast](docs/06-import-toast.png)

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
