SIGILSMITH - 0.4.5
Linux-first TUI mod manager for Baldur's Gate 3

SigilSmith is a keyboard-first terminal UI for managing BG3 mods on Linux.
Profiles, overrides, smart ordering, and clean deploys in one place.

:: REQUIREMENTS ::
• Baldur's Gate 3 (PC/Linux)
• Linux terminal (Konsole, GNOME Terminal, etc.)

:: FEATURES ::
• Fast, readable TUI layout with clear focus states and striping
• Profile explorer with rename/duplicate/import/export
• Override Actions panel for manual conflict picks
• AI Smart Ranking preview before applying order changes
• Created/Added dates pulled from mod metadata
• Native mod.io entries shown inline with manual installs
• Auto-deploy on enable/disable and reorder

:: INSTALL ::
Recommended (AppImage)
• chmod +x sigilsmith-0.4.5-x86_64.AppImage
• ./sigilsmith-0.4.5-x86_64.AppImage

Alternate formats
• sigilsmith-0.4.5-linux-x86_64.tar.gz
• sigilsmith_0.4.5-1_amd64.deb

:: GITHUB RELEASE / CHECKSUMS ::
https://github.com/Agistaris/SigilSmith/releases/tag/v0.4.5

:: NOTES ::
• SigilSmith only manages files you provide; no game assets are bundled.
• Loose files deploy to Data/Generated (or Data).
• .pak files deploy to the Larian Mods directory.
• If paths are not detected, open the menu to configure (browser opens automatically).
  Use Tab to edit the path directly.
• Full source is available on GitHub (Apache-2.0), with CI-built releases and checksums.

:: ROADMAP ::
• Multi-game support via open adapter templates (coming next).

:: KNOWN ISSUES ::
• None known. If you hit anything, report it on GitHub.

:: CREDITS ::
♦ Larian Studios for Baldur's Gate 3
♦ BG3 modding community for inspiration and testing
♦ saghm for the larian_formats crate used for BG3 LSPK + modsettings parsing
