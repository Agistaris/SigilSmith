#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="${DIST_DIR:-$ROOT/dist}"
VERSION=$(rg -m1 '^version\s*=\s*"' "$ROOT/Cargo.toml" | sed -E 's/.*"([^"]+)".*/\1/')
if [ -z "$VERSION" ]; then
  VERSION="0.0.0"
fi

mkdir -p "$DIST_DIR"

cargo build --release

tar -C "$ROOT/target/release" -czf "$DIST_DIR/sigilsmith-${VERSION}-linux-x86_64.tar.gz" sigilsmith

if ! command -v cargo-deb >/dev/null 2>&1; then
  echo "Missing cargo-deb. Install with: cargo install cargo-deb"
  exit 1
fi

if ! command -v cargo-rpm >/dev/null 2>&1; then
  echo "Missing cargo-rpm. Install with: cargo install cargo-rpm"
  exit 1
fi

cargo deb --no-build
cp "$ROOT/target/debian"/*.deb "$DIST_DIR/"

cargo rpm build
cp "$ROOT/target/release/rpmbuild/RPMS"/*/sigilsmith-"$VERSION"-*.rpm "$DIST_DIR/"

"$ROOT/packaging/build-appimage.sh"

# Hash only this build's packages: DIST_DIR can also hold older releases,
# subdirectories and a previous SHA256SUMS.txt.
(
  cd "$DIST_DIR"
  sha256sum -- \
    "sigilsmith-${VERSION}-linux-x86_64.tar.gz" \
    "sigilsmith_${VERSION}"-*_amd64.deb \
    "sigilsmith-${VERSION}"-*.rpm \
    "sigilsmith-${VERSION}"-*.AppImage \
    > SHA256SUMS.txt.tmp
  mv SHA256SUMS.txt.tmp SHA256SUMS.txt
)
