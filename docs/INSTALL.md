# Installing AETHER

AETHER is available as both a headless CLI and a native desktop application.

## Desktop App

Download the latest installer for your platform from [GitHub Releases](https://github.com/rephug/aether/releases).

### Windows (.msi)

1. Download `aether-desktop-{version}-x86_64.msi`
2. Double-click to run the installer
3. If Windows SmartScreen shows "Windows protected your PC":
   - Click **More info**
   - Click **Run anyway**
4. Follow the installation wizard
5. Launch AETHER from the Start Menu

The app installs to `C:\Program Files\AETHER\` and adds a Start Menu shortcut.

### macOS (.dmg)

1. Download `aether-desktop-{version}-x86_64.dmg`
2. Open the disk image
3. Drag **AETHER** to your Applications folder
4. On first launch, macOS Gatekeeper may block the app (unsigned):
   - Right-click the app in Finder
   - Select **Open**
   - Click **Open** in the dialog
5. Alternatively: `System Settings > Privacy & Security > Open Anyway`

### Linux

**AppImage (universal):**

```bash
chmod +x aether-desktop-{version}-x86_64.AppImage
./aether-desktop-{version}-x86_64.AppImage
```

No installation required. The AppImage bundles all dependencies.

**Debian/Ubuntu (.deb):**

```bash
sudo dpkg -i aether-desktop-{version}-amd64.deb
sudo apt-get install -f  # resolve any missing dependencies
```

Launch from your application menu or run `aether-desktop` from the terminal.

Dependencies: `libwebkit2gtk-4.1-0`, `libgtk-3-0`, `libayatana-appindicator3-1`

## CLI (Headless)

For server environments or users who prefer the command line.

### Linux

```bash
tar xzf aether-{version}-x86_64-unknown-linux-gnu.tar.gz
sudo mv aetherd aether-mcp /usr/local/bin/
```

### macOS

```bash
tar xzf aether-{version}-x86_64-apple-darwin.tar.gz
sudo mv aetherd aether-mcp /usr/local/bin/
```

### Windows

Extract `aether-{version}-x86_64-pc-windows-msvc.zip` and add the directory to your PATH.

## Auto-Updates

The desktop app checks for updates automatically on launch. You can configure this in **Settings > Updates**:

- **Auto-check for updates** — enabled by default
- **Check frequency** — On launch, Daily, or Weekly
- **Include pre-release versions** — opt in to beta releases

Manual check: **Settings > Updates > Check Now**

## Signing

Desktop installers are currently unsigned. This means:
- **Windows:** SmartScreen warning on first install (see workaround above)
- **macOS:** Gatekeeper blocks the app (see workaround above)
- **Linux:** No signing required

Code signing certificates will be added in a future release.

## Building from Source

Requires: Rust 1.77+, protobuf-compiler, mold (Linux), sccache

```bash
git clone https://github.com/rephug/aether.git
cd aether

# CLI only
cargo build -p aetherd --release
cargo build -p aether-mcp --release

# Desktop app (requires Tauri CLI + platform WebView SDK)
cargo install tauri-cli
cargo tauri build -p aether-desktop
```

See the project README for full build instructions and system dependencies.
