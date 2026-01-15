# SigilSmith (BG3 Mod Loader for Linux)

## Short Description
A fast, native Linux TUI mod manager for Baldur's Gate 3. Drag-drop mods, set load
order, resolve overrides, and deploy via the SigiLink cache with confidence.

## Long Description
SigilSmith is a native Linux mod manager for Baldur's Gate 3 with a clean,
keyboard-first TUI. It focuses on reliable load order, correct loose-file overrides,
and predictable deployment. Drag and drop archives or folders, enable mods, reorder
them, and SigilSmith handles the rest.

SigiLink keeps deploys fast and safe using hardlinks or symlinks instead of full
copies. SigiLink Intelligent Ranking can auto-rank your list, keep pins, and show
diff previews when you want them. No game assets are included — SigilSmith only
manages files you provide.

Multi-game support is coming next via an open adapter template.

## Features
- Drag & drop `.zip/.7z/.pak` or Data/Generated/bin/Public folders
- Automatic mod type detection (Pak / Generated / Data / Bin)
- Load order with deterministic override rules (last wins)
- Overrides panel for manual conflict picks
- SigiLink Intelligent Ranking preview (diff + scroll) for safer ordering
- Mod list interop (JSON + modsettings.lsx)
- Missing mod placeholders with dependency prompts
- Native mod.io sync + manual mods in one list
- Auto-deploy on enable/disable and reorder
- Duplicate mod detection with overwrite prompts
- Detailed log + deploy manifest cleanup

## Requirements
- Linux
- Baldur's Gate 3 installed (Steam or Proton)
- Optional: `7z` for faster .7z extraction

## Install
1) Download the latest release binary from GitHub Releases.
2) Make it executable: `chmod +x sigilsmith`
3) Run: `./sigilsmith` (in a terminal)

## Usage (Quick)
- Drag & drop mod archives/folders into the window
- Use arrows to select
- Space: enable/disable
- m: move mode, then arrows to reorder (higher order overrides)
 - Esc: Settings (AI Smart Ranking + confirmations)

## Notes
- SigilSmith writes to your BG3 install `Data/Generated` for loose files and to Larian `Mods` for .pak files.
- SigiLink deploys via hardlinks/symlinks (no full-copy fallback).
- No telemetry. No online dependencies.

## Links
- GitHub: <ADD_GITHUB_URL>
- Releases: <ADD_RELEASES_URL>
- Issues: <ADD_ISSUES_URL>

## Disclaimer
This is a community tool and is not affiliated with Larian Studios, Valve, or ModForge.
SigilSmith is source-available and permission is required for reuse or redistribution.
