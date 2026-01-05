# Packaging SigilSmith

## Tools
- Rust toolchain (stable)
- `cargo-deb` (`cargo install cargo-deb`)
- `cargo-rpm` (`cargo install cargo-rpm`)
- `appimagetool` (auto-downloaded by the script)

On Ubuntu, you may need:
- `sudo apt-get install rpm libfuse2`

## Build All Packages
```bash
./packaging/build-packages.sh
```

Outputs are placed in `dist/`:
- `sigilsmith-<version>-linux-x86_64.tar.gz`
- `sigilsmith-<version>-x86_64.AppImage`
- `.deb` and `.rpm`
- `SHA256SUMS.txt`

## AppImage Only
```bash
./packaging/build-appimage.sh
```

## Notes
- Update maintainer info in `Cargo.toml` under `[package.metadata.deb]`.
- Replace the icon if you want a custom brand image.
