# Stage 7.6 — Web Dashboard: HTTP API + Visualization (Revised)

**Phase:** 7 — The Pathfinder
**Prerequisites:** Stage 7.1 (Store Pooling)
**Feature Flag:** `--features dashboard`
**Estimated Codex Runs:** 2–3

---

## Purpose

Provide a browser-based read-only visualization of AETHER's intelligence: dependency graphs, drift trends, coupling maps, health metrics, and project memory. The dashboard is the first human-friendly interface beyond CLI and IDE integration.

### Why HTMX + D3 (Decision #41)

The MVP dashboard has 5 read-only pages. React would add:
- Node.js 18+ as a build-time dependency
- `node_modules/` management
- A JavaScript bundler (webpack/vite/esbuild)
- A separate `npm run build` step before Rust compilation

For 5 pages of charts and graphs, this is unjustifiable. The dashboard uses:
- **HTMX** (CDN) for server-driven interactivity — partial page updates without writing JavaScript fetch/render logic
- **D3.js** (CDN) for graph visualization — force-directed graphs, heatmaps, radar charts
- **Tailwind CSS** (CDN) for styling
- **Axum** for serving HTML fragments (HTMX) + JSON (D3) + static files

No build step. No Node.js. No client-side routing framework. HTMX handles navigation and partial updates; D3 handles data visualization. Upgrade to a SPA framework if dashboard complexity warrants it (>15 interactive pages).

### HTMX Architecture Pattern

```
Browser click → HTMX sends GET /dashboard/fragment/overview → Axum returns HTML fragment
                HTMX swaps innerHTML of target div → page updates without full reload

D3 charts:    → JS fetches GET /api/v1/graph → returns JSON → D3 renders SVG
```

HTMX handles page navigation and data tables. D3 handles visualizations that need raw data (graphs, charts, heatmaps). This hybrid avoids both "everything is a JSON API" (tedious) and "everything is server-rendered HTML" (can't do force-directed graphs).

---

## Architecture

```
Browser ──HTTP──▶ aetherd (with --features dashboard)
                    │
                    ├── GET /dashboard/         → full HTML shell (HTMX + D3 + Tailwind CDN links)
                    ├── GET /dashboard/frag/*   → HTMX HTML fragments (partial page updates)
                    ├── GET /api/v1/*           → JSON API (D3 visualization data)
                    └── POST /mcp              → existing MCP endpoint (unchanged)
```

The dashboard is an Axum router mounted inside `aetherd` when the `dashboard` feature is enabled. It reads from the same `SharedState` that MCP tools use — no additional database connections.

### Why Inside aetherd (Not aether-query)

The dashboard needs the same read access as `aether-query`, but:
- Solo developers run `aetherd` only — they shouldn't need a second binary for visualization
- The dashboard feature flag adds ~200KB to the binary (static files + API handlers)
- Team deployments can run `aether-query` + dashboard separately if needed (future)

---

## New Module: `aether-dashboard`

```
crates/aether-dashboard/
├── Cargo.toml
└── src/
    ├── lib.rs              # Router construction, mount point
    ├── api/
    │   ├── mod.rs
    │   ├── overview.rs     # Project stats (JSON for cards, HTMX for tables)
    │   ├── graph.rs        # Dependency graph data (JSON for D3)
    │   ├── drift.rs        # Drift report (JSON for D3 chart, HTMX for table)
    │   ├── coupling.rs     # Coupling data (JSON for D3 heatmap)
    │   ├── health.rs       # Health metrics (JSON for D3 radar)
    │   └── search.rs       # Search proxy (HTMX fragment response)
    ├── fragments/
    │   ├── mod.rs
    │   ├── overview.rs     # HTMX partial: overview stats + tables
    │   ├── drift_table.rs  # HTMX partial: drift entries table
    │   ├── search_results.rs # HTMX partial: search result list
    │   └── symbol_detail.rs  # HTMX partial: symbol SIR detail panel
    └── static/
        ├── index.html      # Dashboard shell: sidebar nav, HTMX/D3/Tailwind CDN links
        ├── charts.js       # D3 chart initialization (graph, drift, coupling, health)
        └── style.css       # Minimal custom styles (Tailwind handles most)
```

**Dependencies:** `aether-core`, `aether-store`, `aether-analysis`, `axum`, `tower-http` (static files, CORS), `serde`, `serde_json`, `askama` or `maud` (HTML templating for HTMX fragments)

Static files are embedded in the binary via `include_dir!` or `rust-embed` — no external file serving needed. HTMX fragments are rendered server-side using a lightweight template engine.

### Template Engine Choice

HTMX fragments are small HTML snippets returned by Axum handlers. Two options:
- **`maud`** (macro-based): HTML as Rust macros, compile-time checked, no separate template files
- **`askama`** (Jinja2-style): `.html` template files with `{{ }}` syntax, familiar to web developers

Either works. `maud` is preferred for small fragments (fewer files, compile-time safety). `askama` if templates get complex.

---

## JSON API (for D3 visualizations)

All endpoints are read-only GET requests. All return JSON with consistent envelope:

```json
{
  "data": { ... },
  "meta": {
    "generated_at": "2026-02-21T15:30:00Z",
    "index_age_seconds": 120,
    "stale": false
  }
}
```

### `GET /api/v1/overview`

Project-level statistics.

```json
{
  "data": {
    "project_name": "aether",
    "total_symbols": 6234,
    "total_files": 187,
    "total_edges": 14502,
    "sir_coverage_pct": 87.3,
    "languages": { "rust": 142, "typescript": 31, "python": 14 },
    "domains": { "code": 6234, "legal": 47, "finance": 0 },
    "last_indexed_at": "2026-02-21T15:28:00Z",
    "graph_backend": "surreal",
    "memory_notes": 23
  }
}
```

### `GET /api/v1/graph?root={symbol_id}&depth={n}&edge_types={types}`

Subgraph for D3 force-directed visualization.

```json
{
  "data": {
    "nodes": [
      { "id": "sym_abc", "label": "parse_document", "kind": "function", "file": "src/parser.rs", "sir_exists": true }
    ],
    "edges": [
      { "source": "sym_abc", "target": "sym_def", "type": "CALLS", "weight": 1.0 }
    ],
    "total_nodes": 42,
    "truncated": false
  }
}
```

Parameters:
- `root` (optional) — center the graph on this symbol
- `depth` (default 2) — traversal depth from root
- `edge_types` (default all) — comma-separated: `CALLS,DEPENDS_ON,TESTED_BY`
- `limit` (default 200) — max nodes returned (prevent browser meltdown)

### `GET /api/v1/drift?since={days}&threshold={score}`

Drift detection results for visualization as time-series.

```json
{
  "data": {
    "drift_entries": [
      {
        "symbol_id": "sym_abc",
        "symbol_name": "parse_document",
        "drift_score": 0.73,
        "detected_at": "2026-02-21T14:00:00Z",
        "reason": "Implementation changed significantly but SIR unchanged"
      }
    ],
    "total_checked": 6234,
    "drifted_count": 12,
    "threshold_used": 0.5
  }
}
```

### `GET /api/v1/coupling?min_score={score}&limit={n}`

Multi-signal coupling data for heatmap visualization.

```json
{
  "data": {
    "pairs": [
      {
        "symbol_a": "parse_document",
        "symbol_b": "validate_ast",
        "coupling_score": 0.89,
        "signals": {
          "co_change": 0.92,
          "structural": 0.85,
          "semantic": 0.90
        }
      }
    ],
    "total_pairs": 34
  }
}
```

### `GET /api/v1/health`

Graph health metrics for radar chart visualization.

```json
{
  "data": {
    "overall_score": 0.82,
    "dimensions": {
      "sir_coverage": 0.87,
      "test_coverage": 0.73,
      "coupling_health": 0.91,
      "drift_health": 0.78,
      "documentation": 0.82
    },
    "hotspots": [
      { "file": "src/legacy/compat.rs", "issues": ["low_sir_coverage", "high_coupling", "no_tests"] }
    ]
  }
}
```

### `GET /api/v1/search?q={query}&domain={domain}&limit={n}`

Proxy to AETHER's unified search. Returns results formatted for dashboard display.

---

## HTMX Fragment Endpoints

These return HTML snippets, not JSON. HTMX swaps them into the page.

### `GET /dashboard/frag/overview`

Returns HTML fragment with stats cards and recent activity table. Triggered by sidebar nav click:

```html
<!-- In index.html sidebar -->
<a hx-get="/dashboard/frag/overview" hx-target="#main-content" hx-push-url="/dashboard/">
  Overview
</a>
```

### `GET /dashboard/frag/drift-table?threshold={score}`

Returns HTML table rows for drift entries. Triggered by threshold slider:

```html
<input type="range" min="0" max="1" step="0.1" name="threshold"
       hx-get="/dashboard/frag/drift-table" hx-target="#drift-table-body"
       hx-trigger="change" hx-include="this" />
```

### `GET /dashboard/frag/symbol/{symbol_id}`

Returns HTML detail panel for a symbol. Triggered by clicking a node in D3 graph:

```javascript
// In charts.js, on D3 node click:
htmx.ajax('GET', `/dashboard/frag/symbol/${nodeId}`, '#detail-panel');
```

### `GET /dashboard/frag/search?q={query}`

Returns HTML search results list. Triggered by search input:

```html
<input type="search" name="q"
       hx-get="/dashboard/frag/search" hx-target="#search-results"
       hx-trigger="input changed delay:300ms" />
```

---

## Dashboard Pages

### 1. Overview (`/dashboard/`)

Project summary with key metrics:
- Symbol count, file count, language breakdown (bar chart via D3)
- SIR coverage gauge (radial via D3)
- Domain breakdown (if legal/finance verticals active)
- Index freshness indicator
- Quick links to other pages
- Recent activity table (HTMX fragment, server-rendered)

### 2. Dependency Graph (`/dashboard/graph`)

Interactive force-directed graph (D3):
- Nodes = symbols, colored by file/module
- Edges = dependency relationships
- Click node → HTMX loads SIR summary in side panel
- Search box (HTMX-powered, debounced) to find and center on a symbol
- Filter controls for edge types
- Zoom/pan with mouse

### 3. Drift Report (`/dashboard/drift`)

Time-series visualization:
- Line chart: drift score over time (D3)
- Table: currently drifted symbols sorted by score (HTMX fragment, filterable)
- Click row → show SIR vs current implementation diff summary
- Threshold slider triggers HTMX partial update of table

### 4. Coupling Map (`/dashboard/coupling`)

Heatmap visualization (D3):
- Axes = files or modules
- Cell color = coupling score
- Click cell → show signal breakdown (HTMX detail panel)
- Filter by minimum coupling score

### 5. Health Dashboard (`/dashboard/health`)

Radar chart (D3):
- Axes = health dimensions
- Current score polygon overlaid on full-score polygon
- Hotspot table below (HTMX fragment)
- Historical trend if data available

---

## Static File Embedding

```rust
// crates/aether-dashboard/src/lib.rs
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "src/static/"]
struct StaticFiles;

pub fn dashboard_router(state: Arc<SharedState>) -> Router {
    Router::new()
        .nest("/api/v1", api_router(state.clone()))
        .nest("/frag", fragment_router(state))
        .fallback(static_handler)  // Serves embedded HTML/JS/CSS
}

async fn static_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches("/dashboard/");
    let path = if path.is_empty() { "index.html" } else { path };
    match StaticFiles::get(path) {
        Some(content) => { /* serve with correct Content-Type */ }
        None => { /* serve index.html for HTMX navigation */ }
    }
}
```

---

## Integration with aetherd

When `--features dashboard` is enabled:

```rust
// crates/aetherd/src/main.rs (simplified)
#[cfg(feature = "dashboard")]
{
    let dashboard = aether_dashboard::dashboard_router(state.clone());
    app = app.nest("/dashboard", dashboard);
    tracing::info!("Dashboard available at http://127.0.0.1:9730/dashboard/");
}
```

The dashboard shares the same HTTP port as the MCP endpoint. No additional port needed.

---

## CDN Dependencies

```html
<!-- In index.html <head> -->
<!-- HTMX: server-driven interactivity -->
<script src="https://cdnjs.cloudflare.com/ajax/libs/htmx/2.0.4/htmx.min.js"></script>

<!-- D3: data visualization -->
<script src="https://cdnjs.cloudflare.com/ajax/libs/d3/7.9.0/d3.min.js"></script>

<!-- Tailwind CSS: styling -->
<script src="https://cdn.tailwindcss.com"></script>
```

All loaded from CDN. No local node_modules, no bundler. For air-gapped environments, embed these files alongside static assets (adds ~300KB).

---

## File Paths (new/modified)

| Path | Action |
|---|---|
| `crates/aether-dashboard/Cargo.toml` | Create |
| `crates/aether-dashboard/src/lib.rs` | Create |
| `crates/aether-dashboard/src/api/mod.rs` | Create |
| `crates/aether-dashboard/src/api/overview.rs` | Create |
| `crates/aether-dashboard/src/api/graph.rs` | Create |
| `crates/aether-dashboard/src/api/drift.rs` | Create |
| `crates/aether-dashboard/src/api/coupling.rs` | Create |
| `crates/aether-dashboard/src/api/health.rs` | Create |
| `crates/aether-dashboard/src/api/search.rs` | Create |
| `crates/aether-dashboard/src/fragments/mod.rs` | Create |
| `crates/aether-dashboard/src/fragments/overview.rs` | Create |
| `crates/aether-dashboard/src/fragments/drift_table.rs` | Create |
| `crates/aether-dashboard/src/fragments/search_results.rs` | Create |
| `crates/aether-dashboard/src/fragments/symbol_detail.rs` | Create |
| `crates/aether-dashboard/src/static/index.html` | Create |
| `crates/aether-dashboard/src/static/charts.js` | Create (D3 init for all chart types) |
| `crates/aether-dashboard/src/static/style.css` | Create |
| `crates/aetherd/src/main.rs` | Modify — mount dashboard router (behind feature) |
| `Cargo.toml` (workspace) | Modify — add aether-dashboard, add `dashboard` feature |
| `.github/workflows/ci.yml` | Modify — add dashboard to feature matrix |

---

## Edge Cases

| Scenario | Behavior |
|---|---|
| Graph too large (>10K nodes requested) | Enforce `limit` parameter; return `truncated: true` |
| No drift data yet (never ran drift detection) | Return empty `drift_entries` with `total_checked: 0` |
| No SurrealDB graph (fresh install, never indexed) | Overview shows 0 counts; graph page shows "Run `aether index` to get started" |
| Dashboard feature not compiled | `/dashboard/*` returns 404; no static files embedded |
| Browser has no JavaScript | HTMX fragments degrade to full page loads; D3 charts won't render (acceptable) |
| Concurrent API requests | SharedState is Arc — concurrent reads safe |
| Very old index (stale) | All responses include `meta.stale: true`, UI shows warning banner |
| CDN unavailable (air-gapped) | Provide embedded fallback option; document in README |
| HTMX fragment returns error | Return HTML error fragment with retry button |

---

## Pass Criteria

1. `cargo build --features dashboard` compiles aetherd with embedded static files.
2. `cargo build` (no features) does NOT include dashboard (404 on `/dashboard/`).
3. Dashboard loads at `http://127.0.0.1:9730/dashboard/` when feature enabled.
4. HTMX navigation between pages works without full page reloads.
5. All 5 JSON API endpoints return valid data from SharedState.
6. HTMX fragment endpoints return valid HTML snippets.
7. Overview page displays project statistics (D3 bar chart + HTMX stats).
8. Graph page renders D3 force-directed visualization; node click loads HTMX detail panel.
9. Drift page displays D3 time-series chart; threshold slider updates HTMX table.
10. Coupling page displays D3 heatmap with coupling data.
11. Health page displays D3 radar chart with health dimensions.
12. `limit` parameter prevents browser-crashing payloads on graph endpoint.
13. Staleness warning appears in UI when index is old.
14. Existing MCP endpoint (`/mcp`) and CLI commands completely unaffected.
15. Validation gates pass:
    ```
    cargo fmt --all --check
    cargo clippy --workspace --features dashboard -- -D warnings
    cargo test -p aether-dashboard
    cargo test -p aether-store
    cargo test -p aether-mcp
    cargo test -p aetherd --features dashboard
    ```

---

## Codex Prompt

```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- export CARGO_TARGET_DIR=/home/rephu/aether-target
- export CARGO_BUILD_JOBS=1
- export PROTOC=$(which protoc)
- export TMPDIR=/home/rephu/aether-target/tmp
- mkdir -p $TMPDIR
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.
- The repo uses mold linker via .cargo/config.toml — ensure mold and clang are installed.

NOTE ON TECHNOLOGY (Decision #41): HTMX + D3.js + Tailwind CSS (all from CDN).
NO React, NO Node.js, NO build step, NO client-side routing framework.
- HTMX handles page navigation (sidebar links swap main content area) and interactive
  updates (search results, filtered tables, detail panels). Server returns HTML fragments.
- D3 handles data visualizations (force graph, line chart, heatmap, radar chart).
  These fetch JSON from /api/v1/* endpoints.
- Tailwind handles styling via CDN script tag.
CDN URLs:
  HTMX:     https://cdnjs.cloudflare.com/ajax/libs/htmx/2.0.4/htmx.min.js
  D3:       https://cdnjs.cloudflare.com/ajax/libs/d3/7.9.0/d3.min.js
  Tailwind: https://cdn.tailwindcss.com

NOTE ON HTMX PATTERN: The dashboard shell (index.html) loads once. Sidebar navigation
links use hx-get="/dashboard/frag/{page}" hx-target="#main-content" to swap page content.
D3 charts initialize after HTMX swaps in their container divs (use htmx:afterSwap event).
This gives SPA-like navigation with zero client-side JavaScript routing.

NOTE ON HTML TEMPLATING: Use maud (macro-based) or askama (Jinja2-style) for server-side
HTML fragment rendering. Prefer maud for small fragments (compile-time checked, no
separate template files). The HTMX fragment handlers in src/fragments/ return HTML
directly from Axum handlers.

NOTE ON ARCHITECTURE: The dashboard is an Axum router mounted inside aetherd at
/dashboard/* when --features dashboard is enabled. It shares the same HTTP port
as the MCP endpoint. JSON API at /api/v1/* reads from SharedState (Stage 7.1).
HTMX fragments at /dashboard/frag/* also read from SharedState.
No additional database connections.

NOTE ON FEATURE FLAG: Entire crate gated behind workspace feature "dashboard".
  cargo build                      → no dashboard
  cargo build --features dashboard → dashboard available

NOTE ON STATIC FILE EMBEDDING: Use rust-embed or include_dir! to embed HTML/JS/CSS
into the binary. No external file serving. The binary is self-contained.

NOTE ON PRIOR STAGES:
- Stage 7.1: SharedState with Arc<SqliteStore> + Arc<dyn GraphStore> + Arc<dyn VectorStore>
- Stage 7.2: SurrealDB/SurrealKV for graph storage (replaces CozoDB)
- Phase 6: All graph analytics available (drift, coupling, health, causal chains)

NOTE ON D3 VISUALIZATION GUIDELINES:
- Graph page: D3 force-directed layout. Nodes colored by module. Click → HTMX detail panel.
- Drift page: D3 line chart (time-series). HTMX table below with threshold slider.
- Coupling page: D3 heatmap matrix. Cell click for signal breakdown.
- Health page: D3 radar/spider chart.
- Keep visualizations functional and clean. No complex animations.
- D3 charts re-initialize on htmx:afterSwap when their container appears.

You are working in the repo root at /home/rephu/projects/aether.

Read docs/roadmap/phase_7_stage_7_6_web_dashboard.md for the full specification.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase7-stage7-6-web-dashboard off main.
3) Create worktree ../aether-phase7-stage7-6 for that branch and switch into it.
4) Create new crate crates/aether-dashboard with:
   - Cargo.toml depending on aether-core, aether-store, aether-analysis, axum,
     tower-http, rust-embed, serde, serde_json, mime_guess, maud (or askama)
   - src/lib.rs — dashboard_router() function, static file handler, fragment router
   - src/api/mod.rs — JSON API router construction
   - src/api/overview.rs — GET /api/v1/overview (project stats from SharedState)
   - src/api/graph.rs — GET /api/v1/graph (subgraph for D3, with root/depth/limit params)
   - src/api/drift.rs — GET /api/v1/drift (drift entries for D3 time-series)
   - src/api/coupling.rs — GET /api/v1/coupling (pairs for D3 heatmap)
   - src/api/health.rs — GET /api/v1/health (health dimensions for D3 radar)
   - src/api/search.rs — GET /api/v1/search (proxy to unified search)
   - src/fragments/mod.rs — HTMX fragment router
   - src/fragments/overview.rs — HTMX partial: stats cards + tables
   - src/fragments/drift_table.rs — HTMX partial: filtered drift table
   - src/fragments/search_results.rs — HTMX partial: search result list
   - src/fragments/symbol_detail.rs — HTMX partial: symbol SIR detail panel
5) Create static files in src/static/:
   - index.html — dashboard shell: sidebar with hx-get nav links, #main-content target,
     HTMX/D3/Tailwind CDN script tags, htmx:afterSwap handler for D3 chart init
   - charts.js — D3 chart definitions (initGraph, initDriftChart, initHeatmap, initRadar)
   - style.css — minimal custom styles
6) Mount dashboard router in crates/aetherd/src/main.rs behind #[cfg(feature = "dashboard")].
   CRITICAL: The Axum HTTP server for the dashboard MUST be spawned on a dedicated
   background Tokio task so it does not block the LSP stdio loop or the file watcher.
   After building the dashboard router, spawn it as:
     let dashboard_handle = tokio::spawn(async move {
         let listener = tokio::net::TcpListener::bind(&bind_addr).await
             .expect("dashboard: failed to bind");
         tracing::info!("Dashboard listening on http://{bind_addr}/dashboard/");
         axum::serve(listener, router).await
             .expect("dashboard: server error");
     });
   The LSP stdio loop and file watcher continue running on their own tasks. The
   dashboard server runs independently. Do NOT .await the dashboard handle in the
   main function — let it run in the background.
7) Add workspace feature "dashboard" in Cargo.toml. Add aether-dashboard to members.
8) Add tests:
    - Unit tests for each JSON API endpoint (mock SharedState, verify JSON structure)
    - Unit tests for HTMX fragment endpoints (verify HTML contains expected elements)
    - Integration test: start aetherd --features dashboard, GET /dashboard/, verify 200
    - Integration test: GET /api/v1/overview returns valid JSON with expected fields
    - Integration test: GET /api/v1/graph?limit=10 returns nodes and edges
    - Integration test: GET /dashboard/frag/overview returns HTML fragment
9) Run validation:
    - cargo fmt --all --check
    - cargo clippy --workspace --features dashboard -- -D warnings
    - cargo test -p aether-dashboard
    - cargo test -p aether-store
    - cargo test -p aether-mcp
    - cargo test -p aetherd --features dashboard
10) Commit with message: "Add web dashboard with HTMX navigation and D3 visualizations"

SCOPE GUARD: Do NOT use React or any JavaScript framework. Do NOT add Node.js as a
dependency. Do NOT implement write operations via dashboard (read-only). Do NOT add
WebSocket for live updates (HTMX polling is fine for MVP). Do NOT add user authentication
to the dashboard (it binds to localhost by default).
```
