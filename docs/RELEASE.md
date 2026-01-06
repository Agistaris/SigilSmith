# Release Checklist

This repo uses Cargo for versioning and `packaging/` for Linux artifacts.

## 1) Version + Changelog

- Update `Cargo.toml` version.
- Update `CHANGELOG.md` with release notes.

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

## 4) GitHub Release (Exact Steps)

1) Create the repo on GitHub (if not already created).
2) Add the remote and push:

```bash
git remote add origin git@github.com:<user>/sigilsmith.git
git branch -M main
git push -u origin main
```

3) In GitHub, go to Releases → “Draft a new release”.
4) Tag: `vX.Y.Z` (create tag on publish).
5) Title: `SigilSmith vX.Y.Z`.
6) Paste the `CHANGELOG.md` entry into the release notes.
7) Upload all files from `dist/`.
8) Publish.

## 4a) Release-only Public Repo

If your public repo is release-only, generate notes and upload artifacts without pushing source:

```bash
./packaging/build-packages.sh
./scripts/release_notes.sh
```

Then in GitHub Releases:
1) Tag: `vX.Y.Z` (create tag on publish).
2) Title: `SigilSmith vX.Y.Z`.
3) Paste `dist/RELEASE_NOTES.md` into the description.
4) Upload all files from `dist/` (including `SHA256SUMS.txt`).
5) Publish.

## 5) Funding Links

When your handles are ready, update `.github/FUNDING.yml`:

```yaml
github: [yourhandle]
ko_fi: yourhandle
```

## 6) Publish to Mod Sites

Follow `docs/PUBLISH.md` to post the same release on Nexus Mods and other BG3 channels.
