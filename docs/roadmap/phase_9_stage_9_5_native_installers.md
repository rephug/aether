# Phase 9 — The Beacon

## Stage 9.5 — Native Installers + Auto-Update

### Purpose

Produce platform-specific installers (`.msi`, `.dmg`, `.AppImage`, `.deb`) via CI and enable automatic update checking so users stay current without manually downloading new releases. This is the final step in making AETHER a product that non-technical users can install and maintain.

### What Problem This Solves

The current release pipeline (Stage 3.8) produces `.tar.gz` and `.zip` archives containing raw binaries. Users must:
1. Download the correct archive for their platform
2. Extract it
3. Move the binary to a suitable location
4. Optionally add it to PATH

This is fine for developers installing CLI tools but doesn't work for:
- Legal/finance professionals who expect a standard installer
- IT departments deploying to managed workstations
- Anyone who expects "click to install, auto-updates after that"

Native installers with auto-update complete the product distribution story.

### Architecture

```
GitHub Release
    │
    ├── aether-desktop-{version}-x86_64.msi         (Windows installer)
    ├── aether-desktop-{version}-x86_64.dmg          (macOS disk image)
    ├── aether-desktop-{version}-x86_64.AppImage     (Linux universal)
    ├── aether-desktop-{version}-amd64.deb            (Debian/Ubuntu)
    │
    ├── aether-{version}-x86_64-unknown-linux-gnu.tar.gz    (CLI, existing)
    ├── aether-{version}-x86_64-pc-windows-msvc.zip         (CLI, existing)
    └── aether-{version}-x86_64-apple-darwin.tar.gz         (CLI, existing)
    │
    └── latest.json                                   (Tauri updater manifest)
```

Desktop installers and CLI archives coexist in the same GitHub Release. The Tauri updater checks `latest.json` to determine if a new version is available.

### In scope

#### 1. Tauri build configuration

`tauri.conf.json` bundle settings:

```json
{
  "bundle": {
    "active": true,
    "targets": ["msi", "dmg", "appimage", "deb"],
    "identifier": "com.aether.desktop",
    "icon": [
      "icons/32x32.png",
      "icons/128x128.png",
      "icons/128x128@2x.png",
      "icons/icon.icns",
      "icons/icon.ico"
    ],
    "windows": {
      "wix": {
        "language": "en-US"
      }
    },
    "macOS": {
      "minimumSystemVersion": "10.15"
    },
    "linux": {
      "desktop": {
        "categories": ["Development", "Utility"],
        "comment": "Semantic Intelligence for Code and Documents"
      }
    }
  }
}
```

#### 2. CI workflow: desktop release

New workflow: `.github/workflows/release-desktop.yml`

```yaml
name: Release Desktop
on:
  workflow_dispatch:
    inputs:
      tag:
        description: 'Release tag (e.g., v0.10.0)'
        required: true

jobs:
  build-desktop:
    strategy:
      matrix:
        include:
          - os: ubuntu-22.04
            target: x86_64-unknown-linux-gnu
            formats: appimage,deb
          - os: macos-13
            target: x86_64-apple-darwin
            formats: dmg
          - os: windows-latest
            target: x86_64-pc-windows-msvc
            formats: msi

    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: tauri-apps/tauri-action@v0
        with:
          tagName: ${{ inputs.tag }}
          releaseName: "AETHER Desktop ${{ inputs.tag }}"
          releaseBody: "See CHANGELOG.md for details."
          releaseDraft: true
          includeUpdaterJson: true
```

The `tauri-action` handles:
- Installing platform-specific build dependencies (WebView2 SDK on Windows, WebKitGTK on Linux)
- Running `cargo tauri build`
- Uploading installer artifacts to the GitHub Release
- Generating `latest.json` for the updater

#### 3. Auto-update configuration

`tauri.conf.json` updater settings:

```json
{
  "plugins": {
    "updater": {
      "active": true,
      "dialog": true,
      "pubkey": "dW50cnVzdGVkIGNvbW1lbnQ...",
      "endpoints": [
        "https://github.com/rephug/aether/releases/latest/download/latest.json"
      ]
    }
  }
}
```

Update flow:
1. On app launch, Tauri checks the endpoint for `latest.json`
2. If a newer version exists, shows a native dialog: "Update available: v0.X.Y"
3. Options: "Install Now" (downloads + restarts), "Later" (dismisses), "Skip This Version" (suppresses this version)
4. Skipped versions stored in Tauri app data directory
5. Updates are signed with the Tauri updater key pair (generated once, private key stored as GitHub secret)

#### 4. Settings: update preferences

Add to the Configuration UI (Stage 9.2):

| Setting | Control | Default |
|---------|---------|---------|
| Auto-check for updates | Toggle | On |
| Check frequency | Dropdown: On launch / Daily / Weekly | On launch |
| Include pre-release versions | Toggle | Off |

#### 5. Icon set

Generate the required icon set from a source SVG/PNG:

| File | Size | Use |
|------|------|-----|
| `32x32.png` | 32×32 | Taskbar (Windows) |
| `128x128.png` | 128×128 | App list |
| `128x128@2x.png` | 256×256 | Retina app list |
| `icon.ico` | Multi-size | Windows installer/exe |
| `icon.icns` | Multi-size | macOS app bundle |
| `icon.png` | 512×512 | Linux .desktop file |
| `tray-idle.png` | 22×22 | System tray (idle) |
| `tray-indexing.png` | 22×22 | System tray (indexing) |
| `tray-error.png` | 22×22 | System tray (error) |

Use `cargo tauri icon` to generate the platform-specific icon sets from a source image.

#### 6. Signing (optional for initial release)

| Platform | Signing | Notes |
|----------|---------|-------|
| Windows | Optional: EV code signing cert | Without it: SmartScreen warning on first install. Users click "More info → Run anyway." |
| macOS | Optional: Apple Developer ID | Without it: Gatekeeper blocks. Users must right-click → Open. |
| Linux | N/A | AppImage and .deb don't require signing |

For MVP, ship unsigned. Document the workaround in installation instructions. Add signing when/if revenue justifies the ~$200-400/year cost for certificates.

### Out of scope

- ARM64 (aarch64) builds — add later when Apple Silicon demand warrants it
- Microsoft Store / Mac App Store / Snapcraft / Flatpak distribution
- Homebrew cask formula
- Delta updates (full binary replacement for now)
- Rollback to previous version (user can manually install old release)

### Implementation Notes

#### Existing CLI releases continue unchanged

The existing `release.yml` workflow continues producing `.tar.gz` / `.zip` CLI archives. The new `release-desktop.yml` is a separate workflow that produces desktop installers. Both attach to the same GitHub Release tag.

This means a single release contains both:
- `aether-v0.10.0-x86_64-unknown-linux-gnu.tar.gz` (headless CLI)
- `aether-desktop-v0.10.0-x86_64.AppImage` (desktop app)

Users choose based on their use case.

#### Tauri updater key management

```bash
# One-time setup (run locally):
cargo tauri signer generate -w ~/.tauri/aether.key

# This produces:
# ~/.tauri/aether.key       (PRIVATE — add to GitHub secrets as TAURI_SIGNING_PRIVATE_KEY)
# ~/.tauri/aether.key.pub   (PUBLIC  — goes in tauri.conf.json as updater.pubkey)
```

The private key password is stored as `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` in GitHub secrets.

#### Linux dependencies

The `.deb` package should declare dependencies:
- `libwebkit2gtk-4.1-0` (or `libwebkit2gtk-4.0-0` for older Ubuntu)
- `libgtk-3-0`
- `libayatana-appindicator3-1` (for system tray)

The AppImage bundles these, so no external deps.

### Pass criteria

1. `cargo tauri build` produces a working `.msi` on Windows, `.dmg` on macOS, `.AppImage` + `.deb` on Linux.
2. Windows: `.msi` installs to Program Files, adds Start Menu shortcut, app launches.
3. macOS: `.dmg` mounts, drag-to-Applications works, app launches from Launchpad.
4. Linux: `.AppImage` runs without installation. `.deb` installs via `dpkg -i` and launches from application menu.
5. CI workflow produces all platform artifacts as draft release.
6. `latest.json` is generated and attached to the release.
7. Auto-update check fires on launch (verified in dev mode with a mock update endpoint).
8. Update dialog shows version info and all three options (Install / Later / Skip).
9. "Skip This Version" suppresses the dialog for that version on subsequent launches.
10. Update settings (toggle, frequency) are saved and respected.
11. Existing CLI release workflow is unaffected — `.tar.gz` and `.zip` still produced.
12. `cargo fmt --all --check`, `cargo clippy -p aether-desktop -- -D warnings` pass.

### Estimated Claude Code sessions: 1–2
