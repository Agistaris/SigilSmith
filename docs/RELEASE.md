# Release Checklist

This repo uses Cargo for versioning and `packaging/` for Linux artifacts.

## 1) Version + Changelog

- Update `Cargo.toml` version.
- Update `CHANGELOG.md` with release notes.
- Ensure screenshots in `docs/` are up to date (see README).

## 2) Build + Verify

```bash
cargo check -q
./packaging/build-packages.sh
```

Artifacts land in `dist/`:
- `sigilsmith-<version>-linux-x86_64.tar.gz`
- `sigilsmith-<version>-x86_64.AppImage`
- `.deb` and `.rpm`
- `SHA256SUMS.txt`

## 3) Git Tag + Push

```bash
git status
git add Cargo.toml CHANGELOG.md
# add other updated files as needed
git commit -m "Release vX.Y.Z"
git tag vX.Y.Z
git push
git push --tags
```

## 4) GitHub Release (Source + CI)

The public repo includes the full source and a GitHub Actions workflow that builds
release artifacts and publishes them when you push a version tag.

1) Push your commits and tag:

```bash
git push
git tag vX.Y.Z
git push --tags
```

2) The `release` workflow builds and uploads artifacts in `dist/`.
3) Edit the GitHub Release notes if needed (optional).


## 6) Publish to Mod Sites

Follow `docs/PUBLISH.md` to post the same release on Nexus Mods and other BG3 channels.
