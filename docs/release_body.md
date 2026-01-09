# SigilSmith v0.5.0

SigilSmith is a Linux-first TUI mod manager for Baldur's Gate 3. Drag-drop mods, manage profiles, resolve overrides, and deploy with confidence. Multi-game support is coming next via an open adapter template.

## Highlights

- Native mod entries resolve .pak filenames (including spaces) and stop overwriting Created dates when metadata is missing.
- Mod stack search bar with `/` or `Ctrl+F`, debounced preview, and inline clear hints; shortcuts can jump to the mod stack.
- Mod stack sorting with a visible indicator, Ctrl+arrow cycling, and a guard dialog when moving while sorted/filtered.
- Help overlay with full hotkeys, refined context/legend styling, and stable panel heights.
- PageUp/PageDown now page through the mod list when focused.
- Import pipeline improvements for script extender-style archives and override `.pak` files without `meta.lsx`.
- Ignore `.git` and `.vscode` folders when importing/scanning/deploying loose files.
- Update checks fixed from the settings menu.

## Screenshots

![Overview](docs/01-hero-overview.png)
![Profiles](docs/02-explorer-profiles.png)
![Search](docs/02.5-search-names.png)
![Sort](docs/02.8_sort_by_name.png)
![Overrides](docs/03-overrides-mode.png)
![Smart Ranking](docs/04-smart-ranking.png)
![Settings](docs/05-settings-menu.png)
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
