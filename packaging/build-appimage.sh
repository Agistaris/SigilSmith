#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="${DIST_DIR:-$ROOT/dist}"
APPDIR="$ROOT/target/appimage/SigilSmith.AppDir"

VERSION=$(rg -m1 '^version\s*=\s*"' "$ROOT/Cargo.toml" | sed -E 's/.*"([^"]+)".*/\1/')
if [ -z "$VERSION" ]; then
  VERSION="0.0.0"
fi

if [ ! -x "$ROOT/target/release/sigilsmith" ]; then
  cargo build --release
fi

rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons/hicolor/scalable/apps"
cp "$ROOT/target/release/sigilsmith" "$APPDIR/usr/bin/"
cp "$ROOT/packaging/sigilsmith.desktop" "$APPDIR/usr/share/applications/"
cp "$ROOT/packaging/icons/sigilsmith.svg" "$APPDIR/usr/share/icons/hicolor/scalable/apps/"
cp "$ROOT/packaging/sigilsmith.desktop" "$APPDIR/sigilsmith.desktop"
cp "$ROOT/packaging/icons/sigilsmith.svg" "$APPDIR/sigilsmith.svg"

cat <<'APP' > "$APPDIR/AppRun"
#!/bin/sh
exec "$APPDIR/usr/bin/sigilsmith" "$@"
APP
chmod +x "$APPDIR/AppRun"

APPIMAGETOOL="${APPIMAGETOOL:-}"
if [ -z "$APPIMAGETOOL" ]; then
  if command -v appimagetool >/dev/null 2>&1; then
    APPIMAGETOOL="$(command -v appimagetool)"
  else
    APPIMAGETOOL="$ROOT/target/appimagetool"
  fi
fi

if [ ! -x "$APPIMAGETOOL" ]; then
  echo "Downloading appimagetool..."
  mkdir -p "$(dirname "$APPIMAGETOOL")"
  curl -L -o "$APPIMAGETOOL" \
    "https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-x86_64.AppImage"
  chmod +x "$APPIMAGETOOL"
fi

mkdir -p "$DIST_DIR"
"$APPIMAGETOOL" "$APPDIR" "$DIST_DIR/sigilsmith-${VERSION}-x86_64.AppImage"
