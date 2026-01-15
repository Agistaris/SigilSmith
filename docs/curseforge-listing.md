# SigilSmith (Linux BG3 Mod Manager)

## Summary
SigilSmith is a Linux-first TUI mod manager for Baldur's Gate 3. Drag-drop mods,
manage profiles, resolve overrides, and deploy with confidence via the SigiLink cache.

Multi-game support is coming next via an open adapter template.

## Features
- Drag & drop `.zip/.7z/.pak` or folders
- Automatic target detection (Pak / Generated / Data / Bin)
- SigiLink cache: hardlink/symlink deploys (no full-copy fallback)
- SigiLink Intelligent Ranking with pins and diff previews
- Mod list interop (JSON + modsettings.lsx)
- Overrides panel for manual conflict picks
- Native mod.io sync + manual mods in one list
- Auto-deploy on enable/disable and reorder

## Requirements
- Linux
- Baldur's Gate 3 installed (Steam or Proton)

## Install
Download the latest AppImage or tar.gz from GitHub Releases, then run:

```bash
chmod +x SigilSmith-*.AppImage
./SigilSmith-*.AppImage
```

## Links
- GitHub: <ADD_GITHUB_URL>
- Releases: <ADD_RELEASES_URL>
- Issues: <ADD_ISSUES_URL>

## Disclaimer
Community tool. Not affiliated with Larian Studios, Valve, or CurseForge.
SigilSmith is source-available and permission is required for reuse or redistribution.
