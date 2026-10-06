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

if ! command -v rpmbuild >/dev/null 2>&1; then
  echo "Missing rpmbuild. Install your distro's rpm-build (or rpm-tools) package."
  exit 1
fi

cargo deb --no-build
cp "$ROOT/target/debian"/*.deb "$DIST_DIR/"

# cargo-rpm only understands "RPM version 4.x", and RPM 6 writes v6 packages
# that older distros can't install. With a newer rpmbuild, run cargo rpm
# through a wrapper that reports 4.x and pins the v4 package format.
RPM_PATH="$PATH"
if ! rpmbuild --version | grep -q '^RPM version 4\.'; then
  RPM_WRAPPER_DIR="$(mktemp -d)"
  trap 'rm -rf "$RPM_WRAPPER_DIR"' EXIT
  cat > "$RPM_WRAPPER_DIR/rpmbuild" <<EOF
#!/bin/sh
if [ "\$1" = "--version" ]; then echo "RPM version 4.20.1"; exit 0; fi
exec "$(command -v rpmbuild)" --define "_rpmformat 4" "\$@"
EOF
  chmod +x "$RPM_WRAPPER_DIR/rpmbuild"
  RPM_PATH="$RPM_WRAPPER_DIR:$PATH"
fi
PATH="$RPM_PATH" cargo rpm build
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
