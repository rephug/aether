# Phase 9 — The Beacon

## Thesis

Phases 8 and 10 gave AETHER a production-quality intelligence pipeline — three-pass SIR generation, batch indexing, continuous drift monitoring, task-aware context assembly, and clipboard-ready export for any AI chat. Phase 9 makes AETHER **visible** — wrapping the engine in a native desktop application that anyone can install, configure, and use without touching a terminal or editing TOML files. The Beacon phase transitions AETHER from a developer tool distributed as CLI binaries to a polished desktop product with native installers, a system tray presence, guided onboarding, and rich interactive visualizations.

**One-sentence summary:** "From tarball to app store — AETHER becomes a product you install, not a binary you configure."

---

## Why Now

Three forces make Phase 9 the right time for a desktop shell:

1. **The legal/finance verticals demand it.** Users in regulated industries are lawyers, analysts, and compliance officers — not developers. They will not download a tarball, edit `config.toml`, and run `aetherd --index`. They need a `.dmg` or `.msi` that installs in two clicks with a setup wizard that asks "Where are your documents?"

2. **The dashboard exists but has no home.** The HTMX + D3 web dashboard has grown to 27+ pages with extensive visualization (blast radius, architecture, anatomy, x-ray, time machine, causal explorer, and more), but launching it means running `aetherd --features dashboard` and opening `localhost:3847` in a browser. A Tauri shell wraps the same dashboard in a native window with OS integration — no separate browser tab, no remembering port numbers.

3. **Configuration is a barrier.** AETHER now has 15+ config sections: inference providers, embedding backends, vector stores, graph databases, batch pipeline, continuous monitor, watcher intelligence, health scoring weights, drift detection, coupling analysis, search, context export presets, and more. Managing all of this through TOML files is unsustainable. A settings UI makes the difference between "powerful but intimidating" and "powerful and approachable."

---

## Why Tauri (Not Electron)

| Criterion | Tauri | Electron |
|-----------|-------|----------|
| Language | Rust backend (native to AETHER) | Node.js backend |
| Binary size | ~5-10 MB (uses OS webview) | ~150+ MB (bundles Chromium) |
| Memory usage | ~30-50 MB | ~150-300 MB |
| System tray | Native support | Requires extra packages |
| Auto-update | Built-in updater plugin | electron-updater |
| Installer formats | MSI, DMG, AppImage, DEB, RPM | Same via electron-builder |
| IPC | Rust ↔ JS via `invoke()` | Node ↔ Renderer via IPC |
| Webview | WKWebView (macOS), WebView2 (Win), WebKitGTK (Linux) | Chromium |
| Build system | `cargo tauri build` | `electron-builder` |

Tauri is the natural choice because AETHER is already a Rust workspace. The `aetherd` daemon, `SharedState`, and all store/graph/analysis code link directly into the Tauri binary — no subprocess management, no IPC serialization overhead. The frontend is the existing HTMX + D3 dashboard (already HTML/JS), which renders perfectly in any webview.

---

## Decisions for Phase 9

### Decision #90: Tauri 2.x as desktop framework

**Status:** ✅ Active

Tauri 2.x (stable since late 2024) provides:
- Multi-window support (dashboard + settings as separate windows)
- Plugin system for system tray, updater, file dialogs, notifications
- `tauri::command` attribute macro for Rust → JS function exposure
- Sidecar bundling (not needed — we embed `aetherd` directly)

The Tauri app wraps `aetherd` internals — it does NOT spawn `aetherd` as a child process. The binary IS the daemon.

### Decision #91: Frontend stays HTMX + D3 (no framework migration)

**Status:** ✅ Active

The dashboard already works as HTML + HTMX + D3 + Tailwind across 27+ pages with 9 chart modules. Tauri renders it in a native webview. No migration to React/Vue/Svelte. This preserves Decision #41 and means no Node.js build step.

New UI pages (settings, onboarding wizard) are built with the same HTMX pattern — server-rendered HTML fragments driven by Tauri commands exposed as local HTTP endpoints or `invoke()` calls.

If the UI complexity eventually warrants a framework migration (>30 interactive pages with complex state), that becomes a future decision. For Phase 9's scope (settings + onboarding + enhanced charts), HTMX is sufficient.

### Decision #92: Single binary embeds daemon

**Status:** ✅ Active

The Tauri app compiles `aetherd` code directly into the binary via Cargo workspace dependencies. On launch, it:
1. Initializes `SharedState` (same as `aetherd` does today)
2. Spawns the indexer/watcher on a background tokio task (includes git-aware triggers from 10.1)
3. Spawns the dashboard HTTP server on a background task (for webview rendering)
4. Spawns the MCP server on a background task (for AI agent access)
5. Spawns the LSP server on a background task (for editor integration)
6. Spawns the continuous monitor on a background task (drift + staleness from 10.2)

The user sees an app window. Under the hood, the full AETHER engine is running.

### Decision #93: System tray as primary status surface

**Status:** ✅ Active

AETHER lives in the system tray when the window is closed. Tray shows:
- Current status icon (idle / indexing / batch processing / error)
- Tooltip with workspace name and symbol count
- Context menu: Open Dashboard, Settings, Pause Indexing, Quit
- Native OS notifications for: indexing complete, high drift detected, batch pipeline complete, staleness threshold exceeded

### Decision #94: Platform installers via `cargo tauri build`

**Status:** ✅ Active

| Platform | Format | Notes |
|----------|--------|-------|
| Windows | `.msi` via WiX | Adds to Start Menu, optional PATH entry |
| macOS | `.dmg` with drag-to-Applications | Code signed if Apple Developer account available |
| Linux | `.AppImage` + `.deb` | AppImage for universal, DEB for Ubuntu/Debian |

CI produces all three in the release workflow. The existing `.tar.gz` / `.zip` CLI releases continue for headless/server use.

### Decision #95: Auto-update via Tauri updater plugin

**Status:** ✅ Active

Uses `tauri-plugin-updater` with GitHub Releases as the update source. On app launch (or configurable interval), checks for new releases. Presents a non-blocking notification: "Update available: v0.X.Y — Install now / Later / Skip this version."

No forced updates. No telemetry. Update check can be disabled in settings.

---

## Stages

| Stage | Name | Description | Claude Code Sessions | Dependencies |
|-------|------|-------------|----------------------|--------------|
| 9.1 | Tauri Shell + System Tray | Part A: Dashboard pages for 10.x/R.x. Part B: Tauri native window + tray. Part C: Visual polish for product-ready look | 4–5 | Phase 8 + 10.1-10.3 + R.1 complete, Stage 7.6 (dashboard) |
| 9.2 | Configuration UI | Settings screens replacing TOML editing for all 15+ config sections | 2–3 | 9.1 |
| 9.3 | Onboarding Wizard | First-run experience: workspace picker, dependency detection, provider setup | 1–2 | 9.2 |
| 9.4 | Enhanced Visualizations | New D3 pages: drift timeline, coupling chord, memory timeline, health scorecard, staleness heatmap | 2–3 | 9.1 (can parallel with 9.2/9.3) |
| 9.5 | Native Installers + Auto-Update | Platform packages, code signing, CI integration, Tauri updater | 1–2 | 9.1 |

### Dependency Graph

```
9.1 (Tauri Shell) ─────────────────────────────────────┐
    │                                                   │
    ├── 9.2 (Configuration UI) ── 9.3 (Onboarding)     │
    │                                                   │
    ├── 9.4 (Enhanced Visualizations) ──────────────────┤
    │                                                   │
    └── 9.5 (Native Installers) ────────────────────────┘
```

**Parallelism opportunities:**
- After 9.1 merges: 9.2, 9.4, and 9.5 can all start in parallel
- 9.3 depends on 9.2 (wizard uses the same settings components)
- 9.4 is completely independent of 9.2/9.3 (visualization pages, not config)

**Estimated total: 10–15 Claude Code sessions.**

---

## New Crate

### `aether-desktop`

```
crates/aether-desktop/
├── Cargo.toml
├── tauri.conf.json
├── src/
│   ├── main.rs              # Tauri entry point, SharedState init
│   ├── commands.rs           # #[tauri::command] functions exposed to JS
│   ├── tray.rs               # System tray setup and event handling
│   ├── settings.rs           # Config read/write via Tauri commands
│   ├── onboarding.rs         # First-run detection and wizard state
│   └── notifications.rs      # OS notification dispatch
├── ui/
│   ├── index.html            # Main shell (loads dashboard)
│   ├── settings.html         # Settings page
│   ├── onboarding.html       # First-run wizard
│   └── static/
│       ├── charts/           # Enhanced D3 visualization modules
│       └── components/       # Shared HTMX fragments
└── icons/
    ├── icon.png              # App icon
    ├── tray-idle.png         # Tray: normal state
    ├── tray-indexing.png     # Tray: indexing in progress
    └── tray-error.png        # Tray: error state
```

Feature-gated: `--features desktop` in workspace Cargo.toml. The CLI binary (`aetherd`) is still built without Tauri for headless/server deployments.

---

## What Phase 9 Does NOT Do

- **No cloud sync or accounts.** AETHER remains local-first. No user accounts, no cloud dashboards, no telemetry.
- **No mobile app.** Desktop only (Windows, macOS, Linux).
- **No React/Vue/Svelte migration.** HTMX + D3 remains the frontend stack.
- **No marketplace or plugin UI.** That's a future phase if the marketplace concept from the Strategic Roadmap moves forward.
- **No multi-workspace management.** The app opens one workspace at a time. Multi-workspace support is a future enhancement.
