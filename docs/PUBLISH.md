# Publish to BG3 Mod Sites

Use GitHub Releases as the canonical download source, then mirror to mod sites.

## Nexus Mods (Recommended)

1) Create a new mod entry under BG3 “Tools/Utilities”.
2) Title: SigilSmith (Linux TUI mod manager).
3) Add a short description + key features + requirements (Linux + BG3 install).
4) Upload the AppImage or tar.gz from the latest release.
5) Link back to the GitHub Release page for full artifacts and checksums.
6) Paste the release notes from `CHANGELOG.md`.

## mod.io (If Tools Are Allowed)

If the BG3 mod.io category allows tools/utility uploads:

1) Create a new entry and mark it as a tool/utility.
2) Upload the same AppImage or tar.gz.
3) Include version + changelog text.
4) Link back to GitHub for alternate formats.

If tools are not allowed, post a short “announcement” page and link to GitHub Releases.

## Other Channels (Optional)

- Larian forums: post a release thread and link to GitHub/Nexus.
- BG3 modding Discord: announce new version with changelog + link.

## Consistency Checklist

- Version number matches `Cargo.toml` and `CHANGELOG.md`.
- Same artifacts across all sites.
- Checksums posted on GitHub.
