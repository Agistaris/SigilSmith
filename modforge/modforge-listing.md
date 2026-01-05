# SigilSmith (BG3 Mod Loader for Linux)

## Short Description
A fast, native Linux TUI mod loader for Baldur's Gate 3. Drag-drop mods, set load order, and deploy loose files or .pak mods with automatic overrides.

## Long Description
SigilSmith is a native Linux mod manager for Baldur's Gate 3 with a clean TUI interface. It focuses on reliable load order, correct loose-file overrides, and simple, predictable deployment. Drag and drop archives or folders, enable mods, reorder them, and SigilSmith handles the rest.

No game assets are included. SigilSmith only manages files you provide.

## Features
- Drag & drop .zip/.7z/.pak or Data/Generated/bin/Public folders
- Automatic mod type detection (Pak / Generated / Data / Bin)
- Load order with deterministic override rules (higher order wins)
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
3) Run: `./sigilsmith`

## Usage (Quick)
- Drag & drop mod archives/folders into the window
- Use arrows to select
- Space: enable/disable
- m: move mode, then arrows to reorder (higher order overrides)

## Notes
- SigilSmith writes to your BG3 install `Data/Generated` for loose files and to Larian `Mods` for .pak files.
- No telemetry. No online dependencies.

## Links
- GitHub: <ADD_GITHUB_URL>
- Releases: <ADD_RELEASES_URL>
- Issues: <ADD_ISSUES_URL>

## Disclaimer
This is a community tool and is not affiliated with Larian Studios, Valve, or ModForge.
