# Install SigilSmith

SigilSmith ships Linux builds as AppImage, .deb, .rpm, and .tar.gz on GitHub Releases.

## AppImage (Recommended)

1) Download the latest AppImage from the release page.
2) Make it executable:

```bash
chmod +x SigilSmith-*.AppImage
```

3) Run it:

```bash
./SigilSmith-*.AppImage
```

Tip: This is a terminal UI. If you launch from a file manager, choose “Run in Konsole” or launch from a terminal.

## .deb

```bash
sudo dpkg -i sigilsmith-*.deb
# If needed:
sudo apt-get -f install
```

## .rpm

```bash
sudo rpm -i sigilsmith-*.rpm
# or
sudo dnf install sigilsmith-*.rpm
```

## .tar.gz

```bash
tar -xzf sigilsmith-*.tar.gz
cd sigilsmith-*
./sigilsmith
```

## From Source

```bash
cargo build --release
./target/release/sigilsmith
```

## Uninstall

- AppImage/tarball: delete the binary you downloaded.
- .deb/.rpm: uninstall via your package manager.

Config and library data live under:

```
~/.local/share/sigilsmith/
```

See `README.md` for full paths.
