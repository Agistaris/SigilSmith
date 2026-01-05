# Publish Checklist

## ModForge Page
- [ ] Add license file in repo (MIT or Apache-2.0 recommended)
- [ ] Add screenshots (TUI + import + log)
- [ ] Add short + long description (see modforge-listing.md)
- [ ] Add install steps + requirements
- [ ] Declare that no game assets are included
- [ ] Add support/issue link (GitHub Issues)
- [ ] Add version number and changelog note

## GitHub Release (Linux)
- [ ] Bump version in Cargo.toml
- [ ] Build release: `cargo build --release`
- [ ] Strip binary (optional): `strip target/release/sigilsmith`
- [ ] Package: `tar -czf sigilsmith-vX.Y.Z-linux-x86_64.tar.gz -C target/release sigilsmith`
- [ ] Generate sha256: `sha256sum sigilsmith-vX.Y.Z-linux-x86_64.tar.gz`
- [ ] Draft release notes (features + fixes + known issues)

## Optional Packaging
- [ ] AppImage for zero-deps distribution
- [ ] .deb/.rpm for distro installs
- [ ] Provide a `sigilsmith.desktop` and icon

## Quality Gate
- [ ] Clean run on fresh BG3 install
- [ ] Import + enable + reorder works for .pak and loose files
- [ ] Duplicate mod overwrite prompt works
- [ ] Deploy manifest cleanup verified
