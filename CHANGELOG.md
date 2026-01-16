# Changelog

## 0.8.5

- Add auto-deploy onboarding + settings toggle and reflect it in the context panel.
- Expand README/release docs with deeper feature explanations and updated screenshots.

## 0.8.4

- Replace the log status bar animation with a compact braille spinner.
- Expand context hotkeys and align the missing-mod legend row.
- Refine the export modal footer styling and spacing.

## 0.8.3

- Clarify export options (SigilSmith JSON recommended vs. interop modsettings.lsx) and group help text.
- Reduce mod list preview modal height to better fit content.
- Keep the log status badge compact without a full-width footer band.

## 0.8.2

- Surface the missing mod file legend row and expand the context panel to keep hotkeys visible.
- Improve modsettings.lsx imports when Enabled flags are absent (fallback to ModOrder).
- Style the mod list preview hotkeys and slim the log status badge.

## 0.8.1

- Update path browser UX (list-first focus, Enter/Space select, dynamic tab label, better paste handling).
- Respect Enabled flags in modsettings imports and native sync.
- Move the status badge into the log footer with a subtle separator.

## 0.8.0

- Adjust mod stack spacing and header status layout polish.

## 0.7.9

- Move status panel to the header and reclaim vertical space for the main layout.
- Keep SigiLink ranking backups aligned with file overrides.
- Reduce trailing animation banding on mod-name loading overlays.
- Refresh public docs and release notes for the SigiLink era.

## 0.7.8

- Add additional spacing between the On and order columns in the mod stack.
- Keep Context values aligned with Legend/Hotkeys and lock the Legend section to 4 rows.
- Extend the overrides panel height and add a scrollbar + “↓ more” indicator.
- Darken Help section header text while keeping the blue highlight.

## 0.7.7

- Refine loading overlay spacing for mod names.

## 0.7.6

- Update license to permission-required source-available terms.

## 0.7.5

- Align context labels independently of hotkey widths.
- Soften Help section headers and stripe Help rows across full width.
- Keep Esc menu open after closing submenus.
- Remove now leaves a ghost entry (non-delete) and clarifies delete wording.
- Ignore base dependency labels with .pak suffixes and animate loading gaps in names.

## 0.7.4

- Title-case the Context/Legend/Hotkeys panels and the Help/Settings menus.
- Add SigiLink Debug log actions (copy last 200 lines, copy full log, export log file).
- Skip missing (ghost) entries for enable/disable/invert visible hotkeys.

## 0.7.3

- Default SigiLink auto-accept diffs and keep auto ranking running after enable/disable and reorder changes.
- Auto-apply override winner selection after a 5s debounce.
- Move missing-mod markers to the link column and keep names clean.

## 0.7.2

- Redesign overrides panel with fast scrolling, inline winner selection, and a candidate picker.

## 0.7.1

- Nudge left-side panel widths for improved layout balance.

## 0.7.0

- Treat missing .pak dependencies as missing (not disabled) to block auto-enabling ghost requirements.
- Only auto-prompt missing .pak files for enabled mods; missing entries still render with a strike/marker.

## 0.6.9

- Treat missing .pak mods as disabled in the mod stack (toggles + counts) and surface a missing-files queue with Nexus links.
- Improve native .pak filename matching to resolve truncated Mod.io filenames.

## 0.6.8

- Polish the settings menu layout (centered header, ON/OFF toggles, selected action highlight).
- Keep SigiLink status visible in the context panel and reduce spurious SigiLink relocation prompts.

## 0.6.7

- Add SigilLink Intelligent Ranking toggle with onboarding, per-mod pins, and diagnostics, plus ranking UI cues and hotkeys.

## 0.6.6

- Add mod list preview with destination/mode options plus missing/ambiguous summaries; unresolved entries render as grey "(missing)" placeholders that are ignored by deploy and auto-resolve on sync.
- Export mod list JSON + clipboard and add modsettings.lsx interop export with atomic writes.
- Clean up loose-file display names by stripping common Nexus suffixes and refresh settings menu cache actions/order.

## 0.6.5

- Label the Dep column in the header and legend.
- Hide leading zeroes in the Dep column and color missing/disabled counts separately.
- Remove mod dialog adds Cancel and defaults to Remove in the center option.

## 0.6.4

- Show missing/disabled dependency counts in the D column.
- Confirm before disabling dependents or enabling required dependencies.
- Remove mod now offers Remove vs Remove + delete file(s) with safe-path checks.

## 0.6.3

- Rename profile list wording to mod list and surface Ctrl+E/Ctrl+P mod list import/export hotkeys.
- Add a path browser with manual path entry for mod list import/export and a default export path.
- Settings menu adds Clear System Caches plus enable-after-import and delete-files-on-remove toggles.
- Align legend/hotkey panel layout and use uppercase A/S/X actions for visibility.
- Show a friendly error when importing unsupported files instead of crashing.

## 0.6.2

- Fix native .pak filename resolution for truncated UUID names so dependency metadata is detected.
- Preserve native mod dependencies during native sync updates and adoption.
- Fix Cancel Import dialog focus so the dependency queue no longer steals input.
- Refresh dependency cache after native sync dependency updates.
- Vary mod stack loading dot animation speed and pauses during cache/metadata sync.
- Parse ModuleShortDesc dependency entries so dependency names show alongside UUIDs.
- Filter base-game dependency UUIDs from fallback scans so Gustav/Honour modules do not appear as missing.
- Dependency queue now scrolls fully and includes a selectable override row with confirmation.
- Dependency queue hotkeys updated: Ctrl+C copies link, C copies UUID.
- Ignore base-game/system dependency UUIDs (Gustav, Shared, CrossplayUI, etc.) in dependency parsing.
- Strip ignored dependencies even when cached to avoid false missing warnings.
- Self-heal missing pak checks now resolve truncated filenames before disabling targets.
- Enabled marker uses warning color when overrides or missing dependencies are present.
- Import no longer pauses for dependency prompts; missing dependency counts update after import.
- Ignore UUID-only/underscore dependency tokens to reduce false missing warnings for cosmetic mods.
- CLI adds `deps resolved` report and aligns missing-deps detection with alias matching.
- Deploy now hardlinks managed .pak files (Vortex-style) with copy fallback.

## 0.6.1

- Smart-rank cache updates incrementally on toggles, moves, imports, and removals with per-mod scan reuse, plus debug cache validation and simulation helpers.

## 0.5.1

- CLI import now accepts multiple paths per `--import`, with `--deploy`/`--no-deploy` and standard verbosity flags (`-v/-vv/-vvv`, `--verbosity`).
- Batch import support for directories/archives containing multiple mods, including nested archives and top-level mod folders.
- Import progress overlay with hazy background and center gauge, plus a post-import failure summary dialog.
- Duplicate import prompts now default to newer releases and offer an “apply to all” toggle.
- Dependency queue for missing requirements, with downloads folder setup and optional warnings.

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
- Public repo now includes full source with a permission-required license.
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
