# Repository Guidelines

## Project Structure & Module Organization
- `src/`: Rust source code. Entry point is `src/main.rs`, app flow in `src/app.rs`, UI in `src/ui.rs`, BG3 logic in `src/bg3.rs`, deployment in `src/deploy.rs`, smart ranking in `src/smart_rank.rs`, updates in `src/update.rs`.
- `docs/`: Release notes, publish guides, and support docs (`docs/RELEASE.md`, `docs/PUBLISH.md`).
- `packaging/`: Build scripts and desktop assets (`build-packages.sh`, `build-appimage.sh`, icons, `.desktop`).
- `dist/`: Built artifacts and checksums (ignored).
- `test-fixtures/`: Sample archives for manual validation.

## Build, Test, and Development Commands
- `cargo run`: run the TUI locally.
- `cargo build --release`: build `target/release/sigilsmith`.
- `./packaging/build-packages.sh`: build `.tar.gz`, `.deb`, `.rpm` into `dist/`.
- `./packaging/build-appimage.sh`: build AppImage into `dist/`.

## Coding Style & Naming Conventions
- Rust 2021 edition; use standard `rustfmt` defaults (`cargo fmt`).
- Naming: `snake_case` for functions/vars, `PascalCase` for types, `SCREAMING_SNAKE_CASE` for constants.
- Keep UI strings in UI-facing modules; avoid hard-coded paths outside config logic.

## TUI Layout & Spacing Guardrails (prevent alignment regressions)

When the user requests a UI spacing/alignment tweak (e.g., “move X left by 1 space”, “align subtitle with list”, “fix padding”):

1) Target resolution (required; cheap)
- `rg -n` the exact label text AND `rg -n` a nearby unique anchor (icon like 🔗, section header like “Legend”, or neighbor label).
- If the label appears multiple times, do NOT “fix all”.
  - Default: change only the single occurrence in the described context (e.g., the Legend row next to 🔗).

2) Minimal blast radius
- Prefer adjusting layout/padding/column widths at the smallest scope that achieves the request.
- Avoid changing shared spacing constants unless you enumerate all call sites that use them.
- Prefer layout/padding over adding/removing manual leading spaces in strings (unless explicitly requested).
- If emojis/icons are involved: assume display width can vary across terminals; keep icons as a dedicated column where feasible.

3) Automatic validation (preferred)
- Add/update a low-cost UI render/snapshot/buffer test for the affected view:
  - Render the panel to a test backend buffer and assert the left-margin/column starts for:
    - panel title/subtext/body alignment
    - legend rows (icon column + label column)
  - The test should fail if alignment regresses.
- If snapshot tests are not currently feasible, explicitly mark: “UI manual check needed” and specify the exact screen/action to confirm.

4) Diff discipline
- UI spacing request => diff should be localized (ideally 1–2 small edits).
- If the diff touches unrelated UI strings/behavior, stop and reassess before proceeding.

5) Reporting (cheap confidence)
- In the final output, include a 1-line “UI Target” statement:
  - `<view/panel> / <anchor> / <exact string> / <which occurrence> / <expected change>`
  - Example: `Legend row next to 🔗: “SigilLink Ranking” padding -1 (only this occurrence).`

## Testing Guidelines
- No automated test suite yet. Use manual checks:
  - Launch with `cargo run`.
  - Verify import, profiles, overrides, deploy, and update flows.
  - QA pass: validate `modsettings.lsx` load-order parsing (BG3MM and in-game) and confirm SigilSmith starts from that order.
- Dependency debugging: you can open a `.pak` in a text viewer and search for `Dependencies` or `ModName_UUID` strings (e.g., `ImpUI_...`) to confirm embedded dependency labels when metadata parsing is unclear.
- When verifying dependency names, raw `.pak` bytes often contain readable `ModName_UUID` tokens; use those as a fallback hint for search labels.
- If adding tests, prefer `#[cfg(test)]` unit tests or integration tests under `tests/`.
  - For TUI spacing regressions, prefer buffer/snapshot-style tests targeting the specific view.

## BG3 Patch 7/8 Compatibility (modsettings + link safety)
- Patch 7/8 load order source: the ordered `ModuleShortDesc` list under `Mods`. `ModOrder` may be missing or ignored; treat `Mods` order as authoritative and keep `ModOrder` in sync only for interop.
- Preserve base modules (e.g., Gustav/GustavX/GustavDev/Honour/HonourX) in the `Mods` list; do not strip them.
- Validation: confirm `modsettings.lsx` reports version `major="4" minor="8"` for Patch 8, and that exported files keep the `Mods` list order stable.
- Symlink safety: treat symlinked Larian data dirs as valid; never recommend `rm -rf` on symlink paths. To remove a symlink, use `rm` or `unlink` without `-r`, or a reversible tool like `trash-cli`. Avoid trailing slashes or subpaths that follow the link.
- When implementing destructive operations, avoid recursion into symlink targets; prefer `remove_file` on specific files and add explicit warnings when user paths are symlinked.

## Commit & Pull Request Guidelines
- Commit messages are short and imperative (e.g., “Update lockfile”).
- PRs should include: summary, testing notes, and screenshots for UI changes.
- Update `CHANGELOG.md` and relevant docs for user-facing changes.
- Always create local commits for work; only push remotes when explicitly requested.
- Bump the version (e.g., `Cargo.toml` + `CHANGELOG.md`) when changes warrant a new release.

## Security & Configuration Notes
- Do not commit secrets or tokens. Use user config paths (e.g., `~/.config/sigilsmith/`).
- Keep build outputs out of git (`dist/`, `target/`).
- Release prep: move `AGENTS.md` one directory up and remove it from `.gitignore` before pushing, then restore it and re-add the ignore after the push.
- Header banner policy: keep `docs/header-nexus-1280x360.png` and `docs/header-nexus-1920x480.png` labeled `v0.5.x` (x placeholder).
- Release artifacts: store only the release files in `dist/vX.Y.Z/` (AppImage, tar.gz, deb, rpm, headers, `SHA256SUMS.txt`).
- Nexus packaging: after GitHub release builds, create `dist/current build zips/` with only the current release archives (`AppImage`, `tar.gz`, `.deb`, `.rpm`) and include `SHA256SUMS.txt` inside each `.7z`; do not include images or older versions.
