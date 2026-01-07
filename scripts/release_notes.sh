#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="$ROOT/dist"

version_line="$(rg -n "^version" "$ROOT/Cargo.toml" | head -n 1)"
VERSION="$(printf "%s" "$version_line" | sed -E 's/.*"([^"]+)".*/\1/')"

CHANGELOG="$ROOT/CHANGELOG.md"
OUT="$DIST_DIR/RELEASE_NOTES.md"

mkdir -p "$DIST_DIR"

notes="$(awk -v ver="$VERSION" '
  BEGIN {found=0}
  /^## /{
    if (found) { exit }
    if ($0 ~ "^## "ver"$") { found=1 }
  }
  { if (found) print }
' "$CHANGELOG")"

if [ -z "$notes" ]; then
  echo "No changelog entry found for version $VERSION" >&2
  exit 1
fi

if [ -f "$DIST_DIR/SHA256SUMS.txt" ]; then
  checksums="$(cat "$DIST_DIR/SHA256SUMS.txt")"
else
  checksums="$(cd "$DIST_DIR" && sha256sum sigilsmith-"$VERSION"-* 2>/dev/null || true)"
fi

{
  echo "# SigilSmith v$VERSION"
  echo
  echo "## Release Notes"
  echo
  echo "$notes"
  echo
  echo "## Downloads"
  echo
  found=0
  if ls "$DIST_DIR"/sigilsmith-"$VERSION"-* >/dev/null 2>&1; then
    for file in "$DIST_DIR"/sigilsmith-"$VERSION"-*; do
      echo "- $(basename "$file")"
    done
    found=1
  fi
  if ls "$DIST_DIR"/sigilsmith_"$VERSION"-*.deb >/dev/null 2>&1; then
    for file in "$DIST_DIR"/sigilsmith_"$VERSION"-*.deb; do
      echo "- $(basename "$file")"
    done
    found=1
  fi
  if [ "$found" -eq 0 ]; then
    echo "- (build artifacts not found yet)"
  fi
  echo
  echo "## Checksums"
  echo
  if [ -n "$checksums" ]; then
    echo '```text'
    echo "$checksums"
    echo '```'
  else
    echo "_No checksums generated yet._"
  fi
} > "$OUT"

echo "Wrote $OUT"
