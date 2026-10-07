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


## 5) Publish to Nexus Mods

Publish the GitHub release first. Everything below goes out under the
maintainer's Nexus account: show the exact files or text and get explicit user
authorization before publication of each upload, changelog, description edit
or reply.

- Mod page: <https://www.nexusmods.com/baldursgate3/mods/20561> (v3 API mod id
  `14920716406865`).
- Each release adds a new version to the same four main files:

  | File id | Archive from `dist/current build zips/` |
  | --- | --- |
  | 2858152 | `sigilsmith-X.Y.Z-x86_64.AppImage.7z` |
  | 2858143 | `sigilsmith_X.Y.Z-1_amd64.deb.7z` |
  | 2858146 | `sigilsmith-X.Y.Z-1.x86_64.rpm.7z` |
  | 2858149 | `sigilsmith-X.Y.Z-linux-x86_64.tar.gz.7z` |

  Files 2858134, 2858137 and 2858140 are archived v0.4.7 builds; leave them.

1) Check the packages and archives: `sha256sum -c SHA256SUMS.txt` in
   `dist/vX.Y.Z/`, and `7z t` on each `.7z`.
2) Upload through the Nexus v3 API. The maintainer uses a local helper,
   `nexus-upload`, that is not part of this repo; the API key stays in the
   maintainer's home directory and never goes in this repo, logs or command
   lines. Dry run first, then repeat without `--dry-run`:

   ```bash
   cd "dist/current build zips"
   V=X.Y.Z
   up() { nexus-upload upload "$1.7z" --file-id "$2" --name "$1" --version "$V" \
            --category main --archive-existing $3 --dry-run; }
   up "sigilsmith-$V-x86_64.AppImage" 2858152 --update-mod-version
   up "sigilsmith_$V-1_amd64.deb" 2858143
   up "sigilsmith-$V-1.x86_64.rpm" 2858146
   up "sigilsmith-$V-linux-x86_64.tar.gz" 2858149
   ```

   `--archive-existing` moves the previous version to Archived.
   `--update-mod-version` sets the version on the mod page; use it on one file
   only.
3) Check that `nexus-upload files baldursgate3 20561` lists all four at the new
   version as `[main]`.
4) Add the changelog (plain `- ` bullets; it appends, so run it once):
   `nexus-upload changelog --mod-id 14920716406865 --version "$V" --text-file changelog.txt`.
5) Update the description on the mod's edit page: version in the heading,
   "What's New", install commands and FAQ. The API can't edit descriptions or
   post comments, so this and replies are done in the browser. Browser
   automation must set field values directly: on Nexus pages `/` focuses site
   search and Enter can lock the page behind a "Leave site?" prompt. The
   editor's Save button stays disabled while its tab is in the background.
6) Reply to comments the release answers once the Files tab shows the new
   files, a few minutes apart.

## 6) Other Mod Sites

Nexus Mods is the only mod site covered here. For any other site this checkout
has no maintained mod-site publishing runbook. Prepare the site-specific steps
and obtain explicit user authorization before publication; the workflow
boundaries are in [WORKFLOW.md](WORKFLOW.md).
