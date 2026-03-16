# Phase 9 — The Beacon

## Stage 9.1 — Tauri Shell + System Tray

### Purpose

First, bring the dashboard current with all intelligence shipped in Phases 10.1–10.6 and Repo R.1–R.4 — adding operational pages for the batch pipeline, continuous monitor, task context, context export, preset management, and fingerprint history. Then, wrap the completed dashboard in a Tauri 2.x native desktop application with a system tray and OS notifications. This is the foundation stage — every subsequent Phase 9 stage builds on the Tauri shell established here.

### What Problem This Solves

The dashboard was built in Phase 7.6 and has grown to 27+ pages with rich visualizations. However, six major subsystems shipped since then with CLI-only interfaces — no dashboard representation at all:

| Subsystem | Shipped in | CLI commands | Dashboard page |
|-----------|-----------|-------------|----------------|
| Batch pipeline | 10.1 | `batch extract/build/ingest/run` | ❌ None |
| Continuous monitor | 10.2 | `continuous run-once/status` | ❌ None |
| Task context | 10.6 | `context --mode task`, `task-history`, `task-relevance` | ❌ None |
| Context export | R.1 | `context --target <file>` | ❌ None |
| Presets | R.3 | `preset list/show/create/delete` | ❌ None |
| Fingerprint history | 10.1 | Stored in `sir_fingerprint_history` | ❌ None |

A desktop user can't run CLI commands. These subsystems must be visible in the dashboard before Tauri wraps it.

Additionally, AETHER currently requires users to:
1. Download a tarball from GitHub Releases
2. Extract the binary and add it to PATH
3. Edit `.aether/config.toml` by hand
4. Run `aetherd --features dashboard` from a terminal
5. Open `http://localhost:3847/dashboard/` in a browser
6. Keep the terminal session alive

A desktop app reduces this to: install → launch → point at workspace → done.

### Architecture

```
┌─────────────────────────────────────────────────────┐
│  Tauri Native Window (OS Webview)                    │
│  ┌─────────────────────────────────────────────────┐ │
│  │  HTMX Dashboard (27+ pages, completed in this stage) │ │
│  │  Rendered from internal HTTP server              │ │
│  └─────────────────────────────────────────────────┘ │
├─────────────────────────────────────────────────────┤
│  Tauri Rust Backend                                  │
│  ┌──────────┐  ┌──────────┐  ┌───────────────────┐  │
│  │SharedState│  │ Tray Mgr │  │ Notification Mgr  │  │
│  └────┬─────┘  └────┬─────┘  └────────┬──────────┘  │
│       │              │                  │             │
│  ┌────┴─────────────┴──────────────────┴──────────┐  │
│  │  Background Tasks (tokio)                       │  │
│  │  • Indexer / File Watcher (git-aware, 10.1)     │  │
│  │  • Dashboard HTTP Server (internal, for webview)│  │
│  │  • MCP Server (stdio or HTTP/SSE)               │  │
│  │  • LSP Server                                   │  │
│  │  • Continuous Monitor (drift + staleness, 10.2) │  │
│  └─────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────┘
         │
    System Tray Icon
    • Status: idle / indexing / error
    • Menu: Open / Settings / Pause / Quit
```

The Tauri app does NOT spawn `aetherd` as a subprocess. It links the same crate code (`aether-mcp`, `aether-store`, `aether-analysis`, etc.) directly and initializes `SharedState` in-process. The dashboard HTTP server runs on `127.0.0.1` with an ephemeral port, and the webview loads from it.

### In scope

#### Part A: Dashboard completion (in `aether-dashboard` crate)

Add operational pages for the six subsystems that shipped with CLI-only interfaces. Each page follows the existing pattern: API route in `src/api/`, fragment template in `src/fragments/`, and navigation sidebar entry.

**1. Batch Pipeline page**

| Component | What it shows |
|-----------|--------------|
| API: `GET /api/v1/batch-status` | Current batch job state (idle/extracting/building/submitting/polling/ingesting), queue depth, last run timestamp |
| API: `GET /api/v1/batch-history` | Last 20 batch runs with symbol count, duration, cost estimate, prompt hash skip rate |
| Fragment: `batch.rs` | Job status card, history table, "Run Batch Now" button (POST), prompt hash stats |

**2. Continuous Monitor page**

| Component | What it shows |
|-----------|--------------|
| API: `GET /api/v1/continuous-status` | Last scan time, symbols scanned, requeued count, overall staleness distribution |
| API: `GET /api/v1/staleness-summary` | Per-crate staleness scores (noisy-OR), worst symbols, semantic gate activity |
| Fragment: `continuous.rs` | Status card with last scan time, staleness distribution bar, requeue log, "Scan Now" button |

**3. Task Context page**

| Component | What it shows |
|-----------|--------------|
| API: `GET /api/v1/task-history` | Recent task context resolutions — task description, resolved symbols, token budget used, timestamp |
| API: `GET /api/v1/task-relevance?task=<desc>` | Live task-to-symbol ranking (RRF + PPR scores) without full assembly |
| Fragment: `task_context.rs` | Task history table, "Try a task" input field with live relevance preview |

**4. Context Export page**

| Component | What it shows |
|-----------|--------------|
| API: `GET /api/v1/context-preview?target=<file>&budget=32000` | Assembled context document preview with layer breakdown (source, SIR, graph, tests, coupling, memory, health, drift) and token usage per layer |
| Fragment: `context_export.rs` | Target selector (file/symbol/overview), budget slider, format selector (markdown/xml/compact), live preview panel, "Copy to Clipboard" button |

**5. Preset Management page**

| Component | What it shows |
|-----------|--------------|
| API: `GET /api/v1/presets` | List all presets from `.aether/presets/*.toml` with name, description, target count, budget |
| API: `POST /api/v1/presets` | Create new preset |
| API: `DELETE /api/v1/presets/{name}` | Delete preset |
| Fragment: `presets.rs` | Preset list with create/edit/delete controls, TOML preview, "Run Preset" button that navigates to context export with preset applied |

**6. Fingerprint History page**

| Component | What it shows |
|-----------|--------------|
| API: `GET /api/v1/fingerprint-history?symbol_id=<id>` | Per-symbol prompt hash timeline: timestamp, change_source (source/neighbor/config), delta_sem, old/new prompt hash |
| API: `GET /api/v1/fingerprint-summary` | Workspace-wide fingerprint churn: symbols changed this week, top changed symbols, most common change sources |
| Fragment: `fingerprint.rs` | Symbol-level timeline view, workspace summary with churn metrics |

**Dashboard navigation update:**

```
Dashboard
├── Overview (existing)
├── Dependency Graph (existing)
├── Architecture Map (existing)
├── Anatomy (existing)
├── X-Ray (existing)
├── Blast Radius (existing)
├── Causal Explorer (existing)
├── Time Machine (existing)
├── Context Export (NEW — R.1)
├── Task Context (NEW — 10.6)
├── Presets (NEW — R.3)
├── Batch Pipeline (NEW — 10.1)
├── Continuous Monitor (NEW — 10.2)
├── Fingerprint History (NEW — 10.1)
├── Search (existing)
├── Ask (existing)
├── Tour / Glossary (existing)
└── Health / Health Score (existing)
```

#### Part B: Tauri shell (new `aether-desktop` crate)

- Create `crates/aether-desktop/` with Tauri 2.x project structure
- `main.rs`: Tauri `Builder` setup with `SharedState` initialization
- Internal HTTP server serving dashboard routes (reuse `aether-dashboard` router)
- Webview loads `http://127.0.0.1:{ephemeral_port}/dashboard/`
- System tray plugin (`tauri-plugin-shell` + tray API):
  - Icon states: idle (gray), indexing (blue pulse), error (red)
  - Tooltip: "AETHER — {workspace_name} — {symbol_count} symbols"
  - Context menu: Open Dashboard, Pause Indexing, Resume Indexing, Quit
- Window close → minimize to tray (not quit). Quit only from tray menu or Cmd+Q/Alt+F4.
- OS notifications via `tauri-plugin-notification`:
  - "Indexing complete: {N} symbols, {M} SIRs generated"
  - "High drift detected in {module}" (if drift score > threshold)
  - "Batch pipeline complete: {N} symbols regenerated" (10.1)
  - "Staleness threshold exceeded: {N} symbols need refresh" (10.2)
  - "Task context assembled: {N} symbols for {task}" (10.6)
- Tauri commands exposed to JS frontend:
  - `get_status()` → returns engine state (indexing, idle, error, symbol count, etc.)
  - `get_workspace_path()` → returns current workspace
  - `pause_indexing()` / `resume_indexing()` → toggle file watcher
- Workspace Cargo.toml: add `aether-desktop` to members, `desktop` feature flag
- `tauri.conf.json` with window size, title, CSP policy, and plugin configuration

### Out of scope

- Settings/configuration UI (Stage 9.2)
- Onboarding wizard (Stage 9.3)
- New D3 analytical visualizations — drift timeline, coupling chord, staleness heatmap, etc. (Stage 9.4)
- Platform installers and auto-update (Stage 9.5)
- Multi-window management (single main window for MVP)
- Custom window chrome / frameless window (use default OS title bar)
- Inline editing of presets via the dashboard (create/delete only — editing is done via TOML files or the settings UI in 9.2)

### Implementation Notes

#### Tauri + Axum integration pattern

The dashboard is already an Axum router. In the Tauri app, instead of binding to a fixed port, bind to `127.0.0.1:0` (ephemeral) and pass the actual port to the webview URL:

```rust
// main.rs
use tauri::Manager;

#[tokio::main]
async fn main() {
    let shared_state = SharedState::open_readwrite(&workspace_path)
        .expect("Failed to initialize AETHER");

    let state = Arc::new(shared_state);

    // Start dashboard HTTP server on ephemeral port
    let dashboard_router = build_dashboard_router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        axum::serve(listener, dashboard_router).await.unwrap();
    });

    // Start background tasks (indexer, MCP, LSP, continuous monitor)
    spawn_background_tasks(state.clone());

    // Launch Tauri app
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .setup(move |app| {
            let window = app.get_webview_window("main").unwrap();
            window.eval(&format!(
                "window.location.href = 'http://127.0.0.1:{}/dashboard/'",
                port
            ))?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            get_workspace_path,
            pause_indexing,
            resume_indexing,
        ])
        .run(tauri::generate_context!())
        .expect("error while running AETHER Desktop");
}
```

#### System tray state machine

```
App Launch → [Initializing] → workspace loaded → [Idle]
                                                    │
                                          file change detected
                                                    ▼
                                              [Indexing]
                                                    │
                                          indexing complete
                                                    ▼
                                                 [Idle]
                                                    │
                                            error encountered
                                                    ▼
                                                [Error]
                                                    │
                                             user retry / auto-recovery
                                                    ▼
                                                 [Idle]
```

Each state maps to a tray icon and tooltip. Transitions update both via Tauri's `tray.set_icon()` and `tray.set_tooltip()`.

#### Notification throttling

Drift and staleness notifications should not fire on every reindex. Implement a cooldown:
- Max 1 "high drift" notification per module per hour
- "Indexing complete" fires at most once per 5 minutes (batch rapid file changes)
- Store last-notification timestamps in memory (not persisted)

### Dependencies

```toml
# crates/aether-desktop/Cargo.toml
[dependencies]
tauri = { version = "2", features = ["tray-icon"] }
tauri-plugin-notification = "2"
tauri-plugin-shell = "2"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Internal deps — link the engine directly
aether-core = { path = "../aether-core" }
aether-config = { path = "../aether-config" }
aether-store = { path = "../aether-store" }
aether-mcp = { path = "../aether-mcp" }
aether-dashboard = { path = "../aether-dashboard" }
aether-analysis = { path = "../aether-analysis" }
aether-health = { path = "../aether-health" }
aether-memory = { path = "../aether-memory" }
aether-infer = { path = "../aether-infer" }
aether-parse = { path = "../aether-parse" }
aether-lsp = { path = "../aether-lsp" }
aether-query = { path = "../aether-query" }
aether-graph-algo = { path = "../aether-graph-algo" }
```

### Pass criteria

**Part A — Dashboard completion:**

1. Batch Pipeline page renders: status card shows idle/running, history table shows last runs (or "No batch runs yet" for empty state).
2. Continuous Monitor page renders: last scan time, staleness distribution, "Scan Now" button triggers `continuous run-once` and updates status.
3. Task Context page renders: task history table with recent resolutions. "Try a task" input field returns live relevance ranking.
4. Context Export page renders: target selector, budget slider, format selector, and preview panel. "Copy to Clipboard" copies the assembled export.
5. Presets page renders: list of presets from `.aether/presets/`. Create and delete work. Selecting a preset navigates to context export with the preset applied.
6. Fingerprint History page renders: workspace churn summary. Selecting a symbol shows its prompt hash change timeline.
7. All 6 new pages are linked in the dashboard navigation sidebar.
8. All 6 new pages handle empty data gracefully (no errors, shows appropriate empty state messages).
9. `cargo clippy -p aether-dashboard -- -D warnings` passes.
10. `cargo test -p aether-dashboard` passes (API endpoint tests with mock data for new routes).

**Part B — Tauri shell:**

11. `cargo tauri dev` launches a native window displaying the complete AETHER dashboard (including new pages).
12. System tray icon appears with correct status (idle after init, indexing during file changes).
13. Closing the window minimizes to tray. Reopening from tray menu restores the window.
14. "Indexing complete" native OS notification fires after initial index.
15. `get_status` Tauri command returns valid JSON with workspace path, symbol count, and engine state.
16. `pause_indexing` / `resume_indexing` toggle the file watcher (verified by modifying a file while paused → no reindex).
17. `cargo fmt --all --check`, `cargo clippy -p aether-desktop -- -D warnings` pass.
18. `cargo test -p aether-desktop` passes (unit tests for commands and tray state machine).

### Estimated Claude Code sessions: 3–4
