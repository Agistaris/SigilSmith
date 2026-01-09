# Changelog

## Unreleased

## 0.5.0

- Native mod entries resolve .pak filenames (including spaces) and stop overwriting Created dates when metadata is missing.
- Mod stack search bar with `/` or `Ctrl+F`, debounced preview, and inline clear hints; global shortcuts focus the mod stack.
- Mod stack sorting with visible indicator, Ctrl+arrow cycling, and a guard dialog when moving while sorted/filtered.
- Help overlay and context panels refined with stable legend/hotkeys layout and clearer headers.
- PageUp/PageDown now page through the mod list when focused.
- Import pipeline improvements for script extender-style archives and override `.pak` files without `meta.lsx`.
- Ignore `.git` and `.vscode` folders when importing/scanning/deploying loose files.
- Update checks fixed from the settings menu.
- Refresh release headers and screenshots for the 0.5.x series.

## 0.4.8

- Reconcile modsettings entries without managed storage to resolve native .pak filenames (incl. spaces) and timestamps.
- Add a mod search bar with `/` or `Ctrl+F`, debounced preview, Enter to apply, and inline clear hint while active.
- Add mod stack sorting with a highlighted sort indicator, column cycling, and a guard dialog when trying to move while sorted/filtered.
- Add a `?` help overlay with full hotkeys, padded modal layout, and a visible scroll indicator.
- Separate legend symbols from hotkeys with full-width headers, softer styling, and debounced fade-in; global hotkeys stay fixed at the top with an N/A legend placeholder and pastel text headers.
- Keep legend height stable across views and avoid overwriting native mod Created dates with filesystem timestamps when metadata is missing.
- Keep context label alignment consistent across views and format blank dates using the locale's date order.
- PageUp/PageDown now page through the mod list when focused.
- Add Ctrl+Alt+V clipboard paste support in input fields.
- Remove implicit import mode activation on unassigned keys.
- Import override `.pak` files without `meta.lsx` by installing them to `Data/`.
- Detect script extender-style bin-root archives (e.g. `DWrite.dll`) during import.
- Ignore `.vscode` folders alongside `.git` when importing/scanning/deploying loose files.
- Fix update checks triggering from the settings menu.

## 0.4.7

- Auto-restart after applying updates (AppImage/tarball).

## 0.4.6

- Allow applying ready updates from the settings menu.
- Auto-apply tarball updates when the install directory is writable.

## 0.4.5

- Fix update check state persisting after completion/timeout.

## 0.4.4

- Settings menu now shows app version + update status and supports manual update checks.
- Update availability surfaces in settings with clear status messaging.

## 0.4.3

- Auto-update check on startup with AppImage self-update and deb/tar instructions.
- Update downloads are verified with SHA256SUMS before applying.

## 0.4.2

- Added guided path browser for BG3 install + Larian data directories.
- Path browser now supports manual path entry and list/path focus switching.
- Removed accidental path hotkey; configure paths from the menu.
- Setup flow now shows clear status + auto-detect toast on success.
- Public repo now includes full source under Apache-2.0 with CI-built releases.
- Startup now syncs enabled state for all pak mods from modsettings.

## 0.4.1

- Setup onboarding improvements with clearer status and path auto-detect retry.
- Settings menu now includes a "Configure game paths" action plus path display.
- Context + Explorer details show config path and setup hints.
- Added Nexus readme + header banners for release pages.

## 0.4.0

- Major UI refresh with refined header/footer, full-width striping, and cohesive panel layout.
- Overrides workflow polish: dedicated Override Actions panel, debounced swap overlay, clearer legends.
- AI Smart Ranking preview with scrollable diff view, dates, conflict stats, and warnings.
- Metadata accuracy: Created dates now read from meta.lsx/info.json (incl. Zstd paks) with background refresh.
- Native mod sync improvements + self-heal for missing pak files before deploy.
- Import pipeline updates (zip/7z detection + loose metadata persistence).

## 0.3.3

- Adds AI Smart Ranking preview (pak + loose scan) with apply/cancel.
- Adds a release notes helper script for release-only publishing.

## 0.3.2

- Adds a swap overlay in Override Actions during deploy.
- Adjusts scrollbar positioning to align with viewport edges.
- Tightens the settings menu layout for a more compact modal.

## 0.3.1

- Syncs native BG3 mod.io entries from modsettings into the library.
- Adds a Native column in the mod stack and source info in Details.
- Native mod removal can optionally delete the local .pak file.

## 0.3.0

- Overrides focus no longer tints the details/log panels; focus is shown via border + banner.
- Overrides banner gains a subtle inline highlight behind its text.

## 0.2.9

- Left panels now match the dark blue log tone.
- Mod stack background uses a neutral dark grey.

## 0.2.8

- Left panels now use a cohesive dark-blue background.
- Mod stack background is darkened for stronger contrast.

## 0.2.7

- Override focus now tints the details + log pair instead of the bottom bar.
- Bottom overrides banner uses readable accent styling without the highlight band.

## 0.2.6

- Details and log now split evenly for a balanced lower row.
- Details panel background matches log, with override focus highlight.

## 0.2.5

- Details/log row now splits horizontally to reclaim vertical space.
- Override actions border uses the focused highlight color.

## 0.2.4

- Override actions border now uses the light-blue highlight when focused.
- Context panel includes a spacer line before the legend block.
- Explorer tree uses unicode branch/expand glyphs for a cleaner look.

## 0.2.3

- Details panel now uses the same border style as the log.
- Target column width is computed from content and the header is renamed.
- Context values align with legend key widths for cleaner scanning.

## 0.2.2

- Overrides focus highlighting and banner readability updates.
- Details panel header now reflects override actions when focused.
- Bottom status area shares overrides highlight for a cohesive focus band.
- Mod stack column spacing tuned for shorter path labels.

## 0.2.1

- Overrides UX refresh: summary banner, legends, and context counts.
- Details panel repositioned under main panels with unified styling.
- Confirmation dialogs and settings menu refinements.
- Mod stack path labels shortened for clarity.
- Status bar progress indicator and centered text.

## 0.2.0

- UI refresh with cleaner header/footer layout and full-width row striping.
- Explorer profile actions and rename flow improvements.
- Mod stack table and override presentation refinements.
- Legend and details formatting updates for quick scanning.
- Toast and status messaging tuned for clarity.
