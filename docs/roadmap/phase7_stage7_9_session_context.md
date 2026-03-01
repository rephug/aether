# Phase 7.9 Session Context — Dashboard Visual Intelligence

## What You Need to Know

I'm building AETHER, a Rust multi-crate workspace that creates persistent semantic intelligence for codebases. We're in Phase 7 (The Pathfinder). This stage transforms the existing web dashboard from functional data display into a polished visual intelligence surface.

**Repo:** `https://github.com/rephug/aether` at `/home/rephu/projects/aether`
**Dev environment:** WSL2 Ubuntu, mold linker, all builds from `/home/rephu/`

## What Already Exists (Stage 7.6)

The `aether-dashboard` crate exists with:
- Axum router mounted in `aetherd` behind `--features dashboard`
- HTMX navigation (sidebar swaps fragments into `#main-content`)
- 5 pages: Overview, Dependency Graph, Drift Report, Coupling Map, Health
- D3 charts: force-directed graph, line chart, heatmap, radar chart
- JSON API at `/api/v1/*` (graph, drift, coupling, health, search)
- HTMX fragments at `/dashboard/frag/*`
- Static files embedded via `rust-embed` or `include_dir!`
- CDN: HTMX 2.0.4, D3 7.9.0, Tailwind CSS (CDN script)
- Templating: maud (macro-based, compile-time checked)

## What This Stage Adds

6 new pages + design system + upgrades to existing pages. No new crates. All changes in `aether-dashboard` (new API endpoints + new embedded static files).

**Key constraint:** All data already exists from Phases 2-6. No new analysis algorithms. This is API wrappers + D3 visualization only.

## Build Environment

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=1
export PROTOC=$(which protoc)
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

Do NOT use `/tmp/` for build artifacts (RAM-backed tmpfs in WSL2).

## Files to Read First

- `docs/roadmap/phase_7_stage_7_9_dashboard_visual_intelligence.md` — full spec
- `crates/aether-dashboard/` — existing dashboard crate structure
- `crates/aether-dashboard/static/` — existing JS/CSS/HTML files
- `crates/aether-mcp/src/state.rs` — SharedState (data access layer)
- `crates/aether-analysis/src/` — coupling, drift, health, causal chain modules
- `crates/aether-store/src/lib.rs` — SqliteStore query methods

## Crate Test Order (OOM-safe for WSL2)

```bash
cargo fmt --all --check
cargo clippy --workspace --features dashboard -- -D warnings
cargo test -p aether-dashboard
cargo test -p aether-store
cargo test -p aether-mcp
cargo test -p aetherd --features dashboard
```

---

## Run 1 of 3: Foundation + X-Ray + Existing Page Upgrades

### Implementation Prompt

```
==========BEGIN IMPLEMENTATION PROMPT==========

CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=1
- PROTOC=$(which protoc)
- TMPDIR=/home/rephu/aether-target/tmp (mkdir -p $TMPDIR)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

TECHNOLOGY CONSTRAINTS (Decision #41):
- HTMX + D3.js + Tailwind CSS only. NO React, NO Node.js, NO build step.
- CDN URLs (already in dashboard shell):
  HTMX: https://cdnjs.cloudflare.com/ajax/libs/htmx/2.0.4/htmx.min.js
  D3: https://cdnjs.cloudflare.com/ajax/libs/d3/7.9.0/d3.min.js
  Tailwind: https://cdn.tailwindcss.com
- ZERO new CDN dependencies. D3 v7 includes force, treemap, zoom, brush, arc.
- HTML templating: maud (compile-time, no template files).
- Static files: rust-embed or include_dir! (same pattern as existing dashboard).

You are working in the repo root of https://github.com/rephug/aether.

Read these files first:
- docs/roadmap/phase_7_stage_7_9_dashboard_visual_intelligence.md (full spec)
- crates/aether-dashboard/ (existing structure — understand what exists before modifying)
- crates/aether-analysis/src/ (available analysis functions for data)
- crates/aether-store/src/lib.rs (SqliteStore methods for querying symbols, SIR, etc.)

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase7-stage7-9-dashboard-viz off main.
3) Create worktree ../aether-phase7-stage7-9 for that branch and switch into it.

--- PART A: Design System Foundation ---

4) Create shared D3 utility modules in static/js/:

   a) aether-theme.js:
      - Dark/light toggle: reads/writes localStorage.theme, toggles
        document.documentElement.classList 'dark'
      - Respects prefers-color-scheme as default
      - Exports color scale functions that adapt to current theme:
        statusColor(value, thresholds) → emerald/amber/rose hex
        riskColor(score) → continuous emerald-to-rose scale
        communityColor(index) → d3.schemeTableau10[index]
      - Exports isDark() helper

   b) aether-tooltip.js:
      - Singleton tooltip div, absolutely positioned
      - show(event, htmlContent) — positions near cursor, themed bg
      - hide() — removes
      - Handles edge cases: viewport boundaries, scroll offset
      - Dark mode aware (dark bg in light mode, light bg in dark mode)

   c) aether-responsive.js:
      - initResponsive(containerId, renderFn) — sets up ResizeObserver
      - On resize: clears SVG, calls renderFn(width, height)
      - Debounced to 200ms to avoid thrashing

   d) aether-animate.js:
      - enterTransition(selection) — fade in + scale from 0.8
      - exitTransition(selection) — fade out + scale to 0.8
      - pulseNode(selection) — amber glow pulse for drift events
      - Standard duration: 300ms for UI transitions, 600ms for data transitions

5) Create static/css/aether-dashboard.css:
   - Minimal custom CSS beyond Tailwind:
     - SVG pattern for "misplaced" diagonal stripes
     - Sparkline area gradient definitions
     - Keyframe for pulse animation
     - Scrollbar styling for dark mode
   - Keep this file small — Tailwind handles 90% of styling

6) Update dashboard shell HTML (the main index.html equivalent):
   - Add theme initialization script in <head> (before paint):
     if (localStorage.theme === 'dark' || (!localStorage.theme &&
         window.matchMedia('(prefers-color-scheme: dark)').matches)) {
       document.documentElement.classList.add('dark');
     }
   - Add dark: Tailwind classes to body/main containers
   - Add <link> to aether-dashboard.css
   - Add <script> tags for the 4 shared modules
   - Add theme toggle button in sidebar footer (☀/🌙 icon)

7) Update sidebar navigation to new structure:
   - X-Ray (new landing page, replaces Overview)
   - Search (upgraded)
   - ────── separator
   - Blast Radius (new — placeholder page for now)
   - Architecture Map (new — placeholder page for now)
   - Time Machine (new — placeholder page for now)
   - Causal Explorer (new — placeholder page for now)
   - ────── separator
   - Dependency Graph (existing, upgraded)
   - Drift Report (existing, upgraded)
   - Coupling Map (existing, upgraded)
   - ────── separator
   - Theme toggle
   - AETHER version

   Placeholder pages: return a simple HTML fragment with the page name
   and "Coming soon" message. These get built in Runs 2 and 3.

--- PART B: X-Ray Page ---

8) Add API endpoint GET /api/v1/xray?window=7d

   Returns JSON:
   {
     "data": {
       "metrics": {
         "sir_coverage": { "value": 0.87, "trend": 0.03, "sparkline": [0.82, 0.83, ...] },
         "orphan_count": { "value": 4, "trend": -2, "sparkline": [...] },
         "avg_drift": { "value": 0.12, "trend": -0.03, "sparkline": [...] },
         "graph_connectivity": { "value": 0.94, "trend": 0.0, "sparkline": [...] },
         "high_coupling_pairs": { "value": 2, "trend": 1, "sparkline": [...] },
         "sir_coverage_pct": { "value": 0.91, "trend": 0.05, "sparkline": [...] },
         "index_freshness_secs": { "value": 120, "trend": 0, "sparkline": [...] },
         "risk_grade": { "value": "B+", "trend": "up", "sparkline": [...] }
       },
       "hotspots": [
         {
           "symbol_id": "...",
           "qualified_name": "parse_document",
           "file_path": "src/parser.rs",
           "risk_score": 0.89,
           "pagerank": 0.94,
           "drift_score": 0.34,
           "test_count": 0,
           "has_sir": true,
           "risk_factors": ["High centrality", "drifting", "no tests"]
         }
       ]
     },
     "meta": { "timestamp": "...", "stale": false, "window": "7d" }
   }

   Implementation:
   - sir_coverage: query SqliteStore for total symbols vs symbols with SIR
   - orphan_count: query GraphStore connected components, count single-node components
   - avg_drift: query drift_results from aether-analysis
   - graph_connectivity: largest connected component / total nodes
   - high_coupling_pairs: count module pairs with fused_score > 0.7
   - index_freshness: time since last indexing event
   - risk_grade: composite weighted score → letter grade mapping
   - hotspots: top 10 symbols by composite risk (same formula as Stage 6.8)
   - sparkline: collect historical data points (daily aggregation) for the window

   IMPORTANT: If data is not available (e.g., drift hasn't been computed),
   return null for that metric with a "not_computed" flag. Don't error.

9) Add HTMX fragment GET /dashboard/frag/xray

   Returns maud-rendered HTML fragment containing:
   - 8 metric cards in a responsive grid (4 columns on wide, 2 on narrow)
   - Each card: large value, trend arrow (↑↓─), color-coded border
   - D3 sparkline container div per card (id="sparkline-{metric}")
   - Time range selector (7d/30d/90d/All) that triggers HTMX re-fetch
   - Hotspot table below the cards

10) Create static/js/charts/xray-cards.js:
    - Fetch /api/v1/xray
    - For each metric card, render d3.area() sparkline in the container
    - Color sparkline gradient: emerald if healthy, amber if warning, rose if critical
    - Sparkline is small (80x24px), no axes, just the shape
    - Apply status color to card border based on thresholds from the spec

11) Create static/js/charts/xray-hotspots.js:
    - Render hotspot table rows
    - Risk score gets a colored badge (emerald/amber/rose)
    - Click row → triggers HTMX navigation to blast radius page
      (hx-get="/dashboard/frag/blast-radius?symbol_id={id}")
    - Sortable columns (click header to sort by risk, pagerank, drift)

--- PART C: Existing Page Design Upgrades ---

12) Apply design system to existing pages:
    - Add dark: Tailwind classes to all existing maud templates
    - Dependency Graph: add node color = risk score, node size = PageRank
      (enhance existing /api/v1/graph response to include pagerank + risk_score per node)
    - Drift Report: add color-coded table rows by severity
    - Coupling Map: update colors to use signal-type encoding
      (blue=structural, green=semantic, orange=temporal)
    - Health page: keep as-is but apply theme classes
      (it will be merged into X-Ray conceptually, but keep the page working)

    DO NOT break existing D3 chart functionality. The upgrades are additive:
    new CSS classes, enhanced API response fields, updated color scales.

--- VALIDATION ---

13) Run validation:
    - cargo fmt --all --check
    - cargo clippy --workspace --features dashboard -- -D warnings
    - cargo test -p aether-dashboard
    - cargo test -p aether-store
    - cargo test -p aether-mcp
    - cargo test -p aetherd --features dashboard
    Do NOT use cargo test --workspace (OOM risk on WSL2).

14) Report:
    - Which steps were applied vs. skipped (with reason)
    - Validation command outcomes (pass/fail per crate)
    - Any files modified outside scope (should be zero)
    - Total lines changed

--- COMMIT ---

15) Commit with message:
    Add dashboard design system, X-Ray page, and existing page upgrades

--- OUTPUT ---

16) Report commit SHA.
17) Provide push command:
    git -C ../aether-phase7-stage7-9 push -u origin feature/phase7-stage7-9-dashboard-viz

==========END IMPLEMENTATION PROMPT==========
```

---

## Run 2 of 3: Blast Radius + Architecture Map

### Prerequisites
- Run 1 merged or committed on the same branch
- Design system files exist (aether-theme.js, aether-tooltip.js, etc.)

### Implementation Prompt

```
==========BEGIN IMPLEMENTATION PROMPT==========

CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=1
- PROTOC=$(which protoc)
- TMPDIR=/home/rephu/aether-target/tmp (mkdir -p $TMPDIR)
- Do NOT use /tmp/ for any build artifacts.

TECHNOLOGY: HTMX + D3.js + Tailwind CSS only. Zero new CDN deps.
Read the spec: docs/roadmap/phase_7_stage_7_9_dashboard_visual_intelligence.md

You are continuing work on branch feature/phase7-stage7-9-dashboard-viz
in worktree ../aether-phase7-stage7-9.

Read these files first:
- docs/roadmap/phase_7_stage_7_9_dashboard_visual_intelligence.md
- crates/aether-dashboard/ (understand current state after Run 1)
- crates/aether-analysis/src/coupling.rs (blast_radius function)
- crates/aether-analysis/src/drift.rs (community detection results)
- static/js/aether-theme.js, aether-tooltip.js, aether-responsive.js (use these)

--- PART A: Shared Symbol Search Component ---

1) Create static/js/charts/symbol-search.js:
   - Debounced input (300ms) that queries /api/v1/search?q={input}&limit=10
   - Dropdown results list with symbol name + file path
   - Click result → fires custom event 'aether:symbol-selected' with symbol_id
   - Used by both Blast Radius and Causal Explorer pages
   - Keyboard: ↑↓ to navigate results, Enter to select, Esc to close

--- PART B: Blast Radius Explorer ---

2) Add API endpoint GET /api/v1/blast-radius?symbol_id={id}&depth=3&min_coupling=0.2

   Wraps existing aether-analysis blast_radius() logic. Returns:
   {
     "data": {
       "center": {
         "symbol_id": "...",
         "qualified_name": "...",
         "file_path": "...",
         "sir_intent": "First sentence of SIR intent...",
         "pagerank": 0.94,
         "risk_score": 0.89,
         "has_tests": false,
         "is_drifting": true,
         "drift_score": 0.34
       },
       "rings": [
         {
           "distance": 1,
           "nodes": [
             {
               "symbol_id": "...",
               "qualified_name": "...",
               "file_path": "...",
               "sir_intent": "...",
               "pagerank": 0.71,
               "risk_score": 0.45,
               "has_tests": true,
               "is_drifting": false,
               "coupling_to_parent": {
                 "strength": 0.82,
                 "type": "structural",
                 "signals": { "structural": 0.9, "semantic": 0.3, "temporal": 0.6 }
               }
             }
           ]
         },
         { "distance": 2, "nodes": [...] },
         { "distance": 3, "nodes": [...] }
       ],
       "total_impacted": 47
     },
     "meta": { "timestamp": "...", "stale": false }
   }

   Implementation:
   - Call existing blast_radius() from aether-analysis
   - For each symbol in results: look up PageRank, risk score from GraphStore/SqliteStore
   - Fetch first sentence of SIR intent from SqliteStore
   - Check drift_results for is_drifting flag
   - Check tested_by edges for has_tests
   - Cap total nodes at 500 per depth level
   - If symbol_id not found, return 404 with message

3) Add HTMX fragment GET /dashboard/frag/blast-radius?symbol_id={id}

   Returns maud HTML:
   - Symbol search component at top (loads symbol-search.js)
   - SVG container for radial tree
   - Controls: depth slider (1-5), min coupling slider (0.0-1.0)
   - Side panel area for symbol detail (populated on Shift+click)
   - If no symbol_id param, show search prompt only

4) Create static/js/charts/blast-radius.js:

   Radial tree visualization:
   - Center node at SVG center
   - Concentric rings drawn as d3.arc() circles at fixed radii per hop distance
   - Ring labels: "1 hop (12 symbols)", "2 hops (31 symbols)", etc.
   - Nodes positioned on rings using d3.forceSimulation():
     * forceRadial: pulls nodes to their correct ring radius
     * forceCollide: prevents node overlap within a ring
     * forceLink: optional — shows connections (can be toggled off for clarity)

   Node encoding:
   - Size: d3.scaleSqrt() mapped to PageRank (min 6px, max 24px radius)
   - Fill: riskColor(risk_score) from aether-theme.js
   - Stroke: solid 2px if has_tests, dashed 2px if no tests
   - Opacity: 1.0 if fresh SIR, 0.5 if stale
   - Badge: small amber dot overlay if is_drifting

   Edge encoding:
   - Solid line = structural coupling
   - Dashed = semantic coupling
   - Dotted = temporal coupling
   - Width: d3.scaleLinear() on coupling strength (1-4px)

   Interactions:
   - Hover node → aether-tooltip with: name, intent, risk, pagerank, file
   - Click node → re-fetch /api/v1/blast-radius?symbol_id={clicked_id}
     and re-render (animated transition — old nodes exit, new nodes enter)
   - Shift+click → HTMX load symbol detail in side panel
   - d3.zoom() for pan/zoom
   - Depth slider → re-fetch with new depth param
   - Min coupling slider → client-side filter (hide edges below threshold,
     hide nodes that become disconnected)

   Performance: if total nodes > 200, skip force simulation — use
   deterministic angular positioning within each ring instead.

--- PART C: Architecture Map ---

5) Add API endpoint GET /api/v1/architecture?granularity=symbol

   Returns:
   {
     "data": {
       "communities": [
         {
           "community_id": 0,
           "label": "auth",
           "symbol_count": 12,
           "files": [
             {
               "file_path": "src/auth.rs",
               "symbols": [
                 {
                   "symbol_id": "...",
                   "qualified_name": "...",
                   "is_misplaced": false
                 }
               ]
             },
             {
               "file_path": "src/utils/helpers.rs",
               "symbols": [
                 {
                   "symbol_id": "...",
                   "qualified_name": "validate_token",
                   "is_misplaced": true
                 }
               ]
             }
           ],
           "misplaced_count": 3
         }
       ],
       "total_communities": 8,
       "total_misplaced": 7
     },
     "meta": { "timestamp": "...", "stale": false }
   }

   Implementation:
   - Query GraphStore for community detection results (Louvain from Stage 6.6)
   - For each symbol: compare its community assignment vs its directory path
   - "Misplaced" heuristic: symbol is in community X, but its file's directory
     is dominated by community Y (>60% of symbols in that directory belong to Y)
   - Community label: most common directory prefix among community members
   - If community detection hasn't been run, return empty with "not_computed" flag

6) Add HTMX fragment GET /dashboard/frag/architecture

   Returns maud HTML:
   - Stats bar: N communities, M misplaced symbols
   - Toggle: "Logical view" (default) / "Directory view"
   - Toggle: "Show misplaced only"
   - SVG container for treemap

7) Create static/js/charts/architecture.js:

   Zoomable treemap:
   - d3.treemap() with d3.treemapSquarify tiling
   - Data hierarchy: root → communities → files → symbols
   - Cell color: communityColor(community_id) from aether-theme.js
   - Misplaced symbols: SVG <pattern> fill with diagonal stripes over community color
   - Cell label: truncated name, full name in tooltip

   Interactions:
   - Click community cell → zoom in (d3.zoom transition to show files/symbols)
   - Click "back" or breadcrumb → zoom out
   - Hover cell → tooltip with symbol count, misplaced count
   - "Show misplaced only" toggle → fade non-misplaced cells to 0.15 opacity
   - "Directory view" toggle → re-hierarchy data as root → directories → files → symbols
     with community colors on symbols (shows color mixing within directories)

   Performance: at symbol level >1000 cells, aggregate to file level by default.
   Show symbol level only when zoomed into a community.

--- VALIDATION ---

8) Run validation:
   - cargo fmt --all --check
   - cargo clippy --workspace --features dashboard -- -D warnings
   - cargo test -p aether-dashboard
   - cargo test -p aether-store
   - cargo test -p aetherd --features dashboard

9) Report: steps applied vs skipped, validation outcomes, files modified.

--- COMMIT ---

10) Commit with message:
    Add Blast Radius Explorer and Architecture Map visualizations

==========END IMPLEMENTATION PROMPT==========
```

---

## Run 3 of 3: Time Machine + Causal Explorer + Smart Search

### Prerequisites
- Runs 1 and 2 committed on the same branch
- All design system + X-Ray + Blast Radius + Architecture pages working

### Implementation Prompt

```
==========BEGIN IMPLEMENTATION PROMPT==========

CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=1
- PROTOC=$(which protoc)
- TMPDIR=/home/rephu/aether-target/tmp (mkdir -p $TMPDIR)
- Do NOT use /tmp/ for any build artifacts.

TECHNOLOGY: HTMX + D3.js + Tailwind CSS only. Zero new CDN deps.
Read the spec: docs/roadmap/phase_7_stage_7_9_dashboard_visual_intelligence.md

You are continuing work on branch feature/phase7-stage7-9-dashboard-viz
in worktree ../aether-phase7-stage7-9.

Read these files first:
- docs/roadmap/phase_7_stage_7_9_dashboard_visual_intelligence.md
- crates/aether-dashboard/ (understand current state after Runs 1-2)
- crates/aether-analysis/src/causal.rs (causal chain tracing)
- crates/aether-store/src/lib.rs (sir_versions queries, temporal data)
- static/js/ (reuse aether-theme, tooltip, responsive, animate modules)

--- PART A: Time Machine ---

1) Add API endpoint GET /api/v1/time-machine?at={iso_timestamp}&layers=deps,drift

   Returns:
   {
     "data": {
       "snapshot": {
         "timestamp": "2026-02-15T00:00:00Z",
         "nodes": [
           {
             "symbol_id": "...",
             "qualified_name": "...",
             "file_path": "...",
             "first_seen": "2026-01-10T...",
             "community": 2,
             "drift_score_at_time": 0.12
           }
         ],
         "edges": [
           {
             "source": "symbol_id_a",
             "target": "symbol_id_b",
             "edge_type": "structural",
             "strength": 0.8
           }
         ]
       },
       "events": [
         {
           "timestamp": "2026-02-14T...",
           "type": "drift",
           "symbol_id": "...",
           "qualified_name": "...",
           "description": "Drift score increased to 0.34"
         },
         {
           "timestamp": "2026-02-13T...",
           "type": "symbol_added",
           "symbol_id": "...",
           "qualified_name": "new_function",
           "description": "First indexed"
         }
       ],
       "time_range": {
         "earliest": "2026-01-01T...",
         "latest": "2026-02-28T..."
       }
     },
     "meta": { "timestamp": "...", "stale": false }
   }

   Implementation:
   - Query sir_versions table for symbols that existed at time T
     (first_version_timestamp <= T)
   - Query dependency_edges that existed at time T
   - Query drift_results for drift events near time T
   - Collect events in a ±24hr window around the requested timestamp
   - Performance: if symbol count > 500, aggregate to file-level nodes

2) Add HTMX fragment GET /dashboard/frag/time-machine

   Returns maud HTML:
   - Timeline scrubber: range input spanning time_range.earliest to latest
   - Play/pause button, speed selector (1x, 2x, 5x)
   - Layer toggles: Dependencies, Drift events, Communities
   - SVG container for graph
   - Event log panel at bottom (scrollable list)

3) Create static/js/charts/time-machine.js:

   Temporal graph with scrubber:
   - d3.forceSimulation() for graph layout (same physics as Dependency Graph page)
   - d3.scaleTime() for the timeline scrubber
   - Timeline: custom SVG drawn below the graph:
     * Horizontal axis with date labels
     * Event markers: circles (drift=amber, added=blue, removed=red)
     * Draggable handle for current position

   Scrubber interaction:
   - Drag handle → fetch /api/v1/time-machine?at={new_timestamp}
   - Debounce fetches to 500ms while dragging
   - On response: D3 enter/update/exit pattern:
     * New nodes: animate in with enterTransition() from aether-animate.js
     * Removed nodes: animate out with exitTransition()
     * Existing nodes: smooth position transition (600ms)
     * New edges: fade in (green tint briefly)
     * Removed edges: fade out (red tint briefly)

   Play mode:
   - setInterval advances timestamp by 1 day per tick (at 1x speed)
   - Each tick fetches new snapshot and animates transition
   - Speed multiplier changes interval duration

   Layer toggles:
   - Dependencies: show/hide edges
   - Drift events: when enabled, nodes that have drift at current time pulse amber
   - Communities: color nodes by community assignment at current time

   Event log:
   - Populated from events array in API response
   - Click event → center graph on that symbol + flash it
   - Auto-scrolls to current time position

   Performance: pre-fetch adjacent time snapshots (T-1, T+1) for smoother scrubbing.
   If >300 nodes at any snapshot, use file-level aggregation.

--- PART B: Causal Explorer ---

4) Add API endpoint GET /api/v1/causal-chain?symbol_id={id}&depth=3&lookback=30d

   Wraps existing aether-analysis causal chain tracing. Returns:
   {
     "data": {
       "target": {
         "symbol_id": "...",
         "qualified_name": "...",
         "timestamp": "2026-02-20T...",
         "drift_score": 0.34,
         "sir_diff_summary": "Intent shifted from XML parsing to PDF handling"
       },
       "chain": [
         {
           "symbol_id": "...",
           "qualified_name": "...",
           "timestamp": "2026-02-18T...",
           "drift_score": 0.21,
           "sir_diff_summary": "Added new validation rule for document format",
           "causal_confidence": 0.82,
           "link_type": "dependency",
           "caused": ["target_symbol_id"]
         },
         {
           "symbol_id": "...",
           "qualified_name": "...",
           "timestamp": "2026-02-15T...",
           "drift_score": 0.45,
           "sir_diff_summary": "Added TLS configuration field",
           "causal_confidence": 0.71,
           "link_type": "co_change",
           "caused": ["previous_symbol_id"]
         }
       ],
       "chain_depth": 3,
       "overall_confidence": 0.76
     },
     "meta": { "timestamp": "...", "stale": false }
   }

   Implementation:
   - Call existing causal chain tracer from aether-analysis
   - Enrich each chain node with SIR diff summary (compare SIR versions)
   - Compute causal_confidence based on coupling strength + temporal proximity
   - If SIR diff not available, use "No SIR diff available" as summary

5) Add HTMX fragment GET /dashboard/frag/causal

   Returns maud HTML:
   - Symbol search component at top (reuse symbol-search.js)
   - Controls: depth slider (1-5), lookback selector (7d/30d/90d)
   - SVG container for DAG
   - "Animate" button
   - Overall confidence indicator

6) Create static/js/charts/causal-explorer.js:

   Horizontal DAG visualization:
   - Layout: manual topological sort + layered positioning
     * X position: based on timestamp (right = most recent = target)
     * Y position: layered to avoid edge crossings (Sugiyama-style simple version)
   - DO NOT import d3-dag. Implement simple layered layout manually:
     * Assign layers by depth from target
     * Within each layer, order by timestamp
     * Position: x = layer * layerWidth, y = index * nodeHeight

   Node rendering:
   - Each node is a card (SVG <foreignObject> with HTML content):
     * Symbol name (bold)
     * Date (small, muted)
     * Drift score with colored indicator
     * SIR diff summary (1-2 lines, truncated)
   - Border color: green (high confidence), amber (medium), gray (low)
   - Target node has a distinct highlight (thicker border, subtle glow)

   Edge encoding:
   - Solid arrow = dependency link
   - Dashed arrow = co-change correlation
   - Arrow direction: cause → effect (left to right)
   - Width scaled by causal_confidence
   - Drawn with d3.linkHorizontal()

   Interactions:
   - Hover node → full tooltip with complete SIR diff
   - Click node → re-fetch causal chain centered on that symbol
   - "Animate" button:
     * Dims entire graph to 0.2 opacity
     * Highlights chain nodes one at a time from leftmost (root cause) to rightmost (target)
     * Each step: node brightens to full opacity, edge to it animates in
     * 800ms per step
   - Depth slider → re-fetch with new depth
   - Lookback selector → re-fetch with new lookback window

--- PART C: Smart Search Upgrade ---

7) Enhance existing GET /api/v1/search endpoint:

   Add fields to each search result:
   - sir_summary: first 2-3 sentences of SIR intent (from SqliteStore)
   - risk_score: composite risk score (from health metrics)
   - pagerank: PageRank value (from GraphStore)
   - drift_score: current drift score (from drift_results)
   - test_count: number of test guards (from tested_by edges)
   - related_symbols: top 3 most-coupled symbols by fused_score
     (from coupling analysis), each with { symbol_id, qualified_name }

   These are all lookups on existing data. No new computation needed.
   If any field is unavailable, return null (don't error).

8) Replace existing search HTMX fragment with rich result cards:

   GET /dashboard/frag/search?q={query}&mode=hybrid&lang=&risk=&drift=

   Returns maud HTML:
   - Search input with mode toggle (Lexical / Semantic / Hybrid buttons)
   - Filter sidebar: language checkboxes, risk level, drift status, has-tests
   - Results area with rich cards:
     * Symbol name (large, monospace) + file path
     * SIR summary (2-3 lines, normal text)
     * Metric badges: Risk (colored), PageRank, Drift trend, Test count
     * Related symbols as clickable links
     * Click card → navigate to blast radius for that symbol

9) Create static/js/charts/smart-search.js:
   - Handle mode toggle: re-fetch with mode param
   - Handle filter changes: re-fetch with filter params (HTMX hx-include)
   - Keyboard navigation: ↑↓ moves highlight between results, Enter opens
   - Result count indicator: "47 results for 'parse' (semantic mode)"
   - Empty state: "No results found. Try a different search mode."
   - Loading state: skeleton cards while fetching

--- PART D: Remove Placeholder Pages ---

10) Replace placeholder fragments for Blast Radius, Architecture Map,
    Time Machine, and Causal Explorer with actual fragments (if Run 1
    created placeholders that haven't been replaced yet by Run 2).

--- VALIDATION ---

11) Run validation:
    - cargo fmt --all --check
    - cargo clippy --workspace --features dashboard -- -D warnings
    - cargo test -p aether-dashboard
    - cargo test -p aether-store
    - cargo test -p aether-mcp
    - cargo test -p aetherd --features dashboard

12) Verify all pages load:
    - /dashboard/ → X-Ray (from Run 1)
    - /dashboard/blast-radius → Blast Radius (from Run 2)
    - /dashboard/architecture → Architecture Map (from Run 2)
    - /dashboard/time-machine → Time Machine (this run)
    - /dashboard/causal → Causal Explorer (this run)
    - /dashboard/search → Smart Search (this run)
    - /dashboard/graph → Enhanced Dependency Graph (from Run 1)
    - /dashboard/drift → Enhanced Drift Report (from Run 1)
    - /dashboard/coupling → Enhanced Coupling Map (from Run 1)

13) Report: steps applied vs skipped, validation outcomes, files modified.

--- COMMIT ---

14) Commit with message:
    Add Time Machine, Causal Explorer, and Smart Search visualizations

--- OUTPUT ---

15) Report commit SHA.

==========END IMPLEMENTATION PROMPT==========
```

---

## Post-Merge Sequence

After the PR merges (run this after all 3 runs are committed and PR approved):

```bash
# Update local main
cd /home/rephu/projects/aether
git checkout main
git pull --ff-only origin main
git log --oneline -3

# Clean up worktree and branch
git worktree remove ../aether-phase7-stage7-9
git branch -d feature/phase7-stage7-9-dashboard-viz
git worktree prune

# Clean build cache if needed
rm -rf /home/rephu/aether-target/*
```

---

## Notes on Claude Code vs Codex

If running this with Claude Code instead of Codex:
- Claude Code can browse the repo natively — the "Read these files" step is guidance, not a requirement
- Claude Code handles git operations directly — worktree creation works the same way
- The validation and commit steps are identical
- Claude Code may ask implementation questions interactively — refer to the full spec for answers
- If Claude Code asks about D3 approach for any visualization, the spec describes the exact D3 modules to use
- If data doesn't exist in the stores for a particular metric, return null with a "not_computed" flag — don't skip the API endpoint

## What Comes Next

After 7.9 merges → Phase 8 (The Synthesizer) — self-verifying code generation. The dashboard visualizations from 7.9 become the primary way to observe Phase 8's impact on the codebase (generated code appears in the graph, its SIR is tracked, its blast radius is visible).
