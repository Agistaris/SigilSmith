# Release Checklist

This repo uses Cargo for versioning and `packaging/` for Linux artifacts.

## 1) Version + Changelog

- Update `Cargo.toml` version.
- Update `CHANGELOG.md` with release notes.
- Ensure screenshots in `docs/` are up to date (see README).

## 2) Build + Verify

```bash
cargo check -q
VERSION=$(rg -m1 '^version\s*=\s*"' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')
DIST_DIR="dist/v$VERSION" ./packaging/build-packages.sh

# Nexus upload zips (requires 7z/7zz)
rm -rf "dist/current build zips"
mkdir -p "dist/current build zips"
artifacts=(
  "dist/v$VERSION/sigilsmith-${VERSION}-linux-x86_64.tar.gz"
  "dist/v$VERSION/sigilsmith-${VERSION}-x86_64.AppImage"
  "dist/v$VERSION/sigilsmith_${VERSION}-1_amd64.deb"
  "dist/v$VERSION/sigilsmith-${VERSION}-1.x86_64.rpm"
)
for artifact in "${artifacts[@]}"; do
  7z a -t7z "dist/current build zips/$(basename "$artifact").7z" \
    "$artifact" "dist/v$VERSION/SHA256SUMS.txt"
done
```

Artifacts land in:
- `dist/v<version>/` (release assets + `SHA256SUMS.txt`)
- `dist/current build zips/` (only the current release, each `.7z` includes the artifact + `SHA256SUMS.txt`)

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

## 4) GitHub Release (Source + Assets)

1) Push your commits and tag:

```bash
git push
git tag vX.Y.Z
git push --tags
```

2) Create a GitHub Release for the tag.
3) Upload assets from `dist/vX.Y.Z/`.
4) Edit the GitHub Release notes if needed (optional).


## 6) Publish to Mod Sites

This checkout has no maintained mod-site publishing runbook. Prepare the
site-specific steps and obtain explicit user authorization before publication;
the workflow boundaries are in [WORKFLOW.md](WORKFLOW.md).
