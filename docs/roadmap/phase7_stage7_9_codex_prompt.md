# Stage 7.9 — Dashboard Visual Intelligence — Codex Prompt

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
- HTMX handles page navigation and interactive updates. Server returns HTML fragments.
- D3 handles data visualizations. These fetch JSON from /api/v1/* endpoints.
- Tailwind handles styling via CDN script tag.
- ZERO new CDN dependencies. D3 v7 includes force, treemap, zoom, brush, arc, chord.
CDN URLs (already in dashboard shell):
  HTMX:     https://cdnjs.cloudflare.com/ajax/libs/htmx/2.0.4/htmx.min.js
  D3:       https://cdnjs.cloudflare.com/ajax/libs/d3/7.9.0/d3.min.js
  Tailwind: https://cdn.tailwindcss.com

NOTE ON HTMX PATTERN: The dashboard shell loads once. Sidebar navigation links use
hx-get="/dashboard/frag/{page}" hx-target="#main-content" to swap page content.
D3 charts initialize after HTMX swaps in their container divs (use htmx:afterSwap event).

NOTE ON HTML TEMPLATING: Use maud (macro-based) for server-side HTML fragment rendering.
Compile-time checked, no separate template files. Same pattern as existing dashboard.

NOTE ON STATIC FILE EMBEDDING: Use rust-embed or include_dir! to embed all static
files into the binary. No external file serving. Same pattern as existing dashboard.

NOTE ON EXISTING DASHBOARD (Stage 7.6): The dashboard crate already exists at
crates/aether-dashboard/ with:
  - Axum router mounted in aetherd behind --features dashboard
  - 5 pages: Overview, Dependency Graph, Drift Report, Coupling Map, Health
  - JSON API at /api/v1/* (graph, drift, coupling, health, search)
  - HTMX fragments at /dashboard/frag/*
  - D3 charts in static/js/charts.js (force graph, line chart, heatmap, radar)
  - maud fragment rendering, rust-embed static file serving
This stage ADDS 6 new pages and a design system ON TOP of what exists.
Do NOT rewrite existing pages — enhance them.

NOTE ON PRIOR STAGES:
- Stage 7.1: SharedState with Arc<SqliteStore> + Arc<dyn GraphStore> + Arc<dyn VectorStore>
- Stage 7.2: SurrealDB/SurrealKV for graph storage
- Stage 7.6: Existing dashboard with 5 pages
- Phase 6: All graph analytics available — PageRank, Louvain community detection,
  drift analysis, coupling analysis, causal chains, health metrics, blast radius.
  These are in crates/aether-analysis/src/ and accessible via SharedState.

NOTE ON DATA AVAILABILITY: All visualization data already exists from Phases 2–6.
No new analysis algorithms are needed — this stage creates API wrappers and
D3 visualizations for existing data. If a specific metric is not available (e.g.,
drift hasn't been computed yet), return null for that field with a "not_computed"
flag. Do NOT error. Do NOT skip the endpoint.

You are working in the repo root at /home/rephu/projects/aether.

=== STEP 0: CODE INSPECTION (PLAN MODE) ===

Before writing ANY code, read these files to understand the codebase patterns:

a) docs/roadmap/phase_7_stage_7_9_dashboard_visual_intelligence.md — the full
   specification. Contains JSON shapes for all new API endpoints, D3 visualization
   details for all 6 new pages, design system spec, color palettes, interaction
   patterns, and performance guardrails. READ THIS ENTIRE FILE.

b) crates/aether-dashboard/ — the existing dashboard crate. Understand ALL
   existing files, routes, static files, maud fragment patterns, and how the
   Axum router is structured before modifying anything.

c) crates/aether-dashboard/static/ — existing JS, CSS, HTML files. Understand
   what charts.js does and how index.html is structured.

d) crates/aether-analysis/src/ — list all modules. Find the public functions for:
   - blast_radius (or equivalent subgraph extraction)
   - drift reports and drift_results queries
   - coupling analysis (CouplingEdge, SignalBreakdown, fused_score)
   - causal chain tracing (trace_cause or similar)
   - health metrics (composite scores, risk grades)
   - community detection results (Louvain)
   - PageRank computation or cached PageRank values

e) crates/aether-store/src/lib.rs — SqliteStore public methods. Find:
   - Symbol queries (count, list, by file, by language)
   - SIR queries (get_sir_for_symbol, sir coverage)
   - sir_versions table queries (temporal data)
   - drift_results queries
   - dependency_edges with timestamps

f) crates/aether-store/src/graph_surreal.rs — GraphStore trait methods. Find:
   - get_dependencies_for_symbol, get_callers_of_symbol
   - PageRank, community detection, connected components
   - Any existing blast radius or subgraph traversal methods

g) crates/aether-graph-algo/src/ — extracted graph algorithm library. Find:
   - PageRank, Louvain, BFS, connected components

After reading, note any discrepancies with the spec assumptions, then proceed.

=== STEP 1: BRANCH + WORKTREE ===

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase7-stage7-9-dashboard-viz off main.
3) Create worktree ../aether-phase7-stage7-9 for that branch and switch into it.

=== STEP 2: DESIGN SYSTEM ===

4) Create shared D3 utility modules in the static/js/ directory:

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
      - show(event, htmlContent) — positions near cursor, themed background
      - hide() — removes tooltip
      - Handles viewport boundaries, scroll offset
      - Dark mode aware (dark bg in light mode, light bg in dark mode)

   c) aether-responsive.js:
      - initResponsive(containerId, renderFn) — sets up ResizeObserver
      - On resize: clears SVG, calls renderFn(width, height)
      - Debounced to 200ms

   d) aether-animate.js:
      - enterTransition(selection) — fade in + scale from 0.8
      - exitTransition(selection) — fade out + scale to 0.8
      - pulseNode(selection) — amber glow pulse for drift events
      - Durations: 300ms for UI, 600ms for data transitions

5) Create static/css/aether-dashboard.css:
   - SVG <pattern> for diagonal stripes (misplaced symbol indicator)
   - Sparkline area gradient definitions
   - Pulse animation keyframe
   - Dark mode scrollbar styling
   Keep minimal — Tailwind handles 90% of styling.

6) Update dashboard shell HTML (the index.html or equivalent):
   - Add theme initialization script in <head> (before first paint):
     if (localStorage.theme === 'dark' || (!localStorage.theme &&
         window.matchMedia('(prefers-color-scheme: dark)').matches))
       document.documentElement.classList.add('dark');
   - Add dark: Tailwind classes to body/main containers
   - Add <link> to aether-dashboard.css
   - Add <script> tags for the 4 shared modules
   - Add theme toggle button in sidebar footer (☀/🌙 icon)

7) Update sidebar navigation to new structure:
   X-Ray (new landing page, replaces Overview as default) → Search → separator →
   Blast Radius → Architecture Map → Time Machine → Causal Explorer → separator →
   Dependency Graph → Drift Report → Coupling Map → separator →
   Theme toggle → AETHER version

=== STEP 3: X-RAY PAGE ===

8) Add API endpoint GET /api/v1/xray?window=7d

   Returns JSON envelope { "data": { "metrics": {...}, "hotspots": [...] }, "meta": {...} }
   with 8 metric cards: sir_coverage, orphan_count, avg_drift, graph_connectivity,
   high_coupling_pairs, sir_coverage_pct, index_freshness_secs, risk_grade.
   Each metric has: value, trend (numeric change), sparkline (array of ~30 data points).
   Hotspots: top 10 symbols by composite risk score with symbol_id, qualified_name,
   file_path, risk_score, pagerank, drift_score, test_count, has_sir, risk_factors array.

   Implementation: query SqliteStore for SIR/symbol counts, GraphStore for PageRank and
   connectivity, aether-analysis for drift/coupling. Sparkline data from daily aggregation
   over the window period. If any metric unavailable, return null with "not_computed" flag.

9) Add HTMX fragment GET /dashboard/frag/xray
   Maud-rendered HTML: 8 metric cards in responsive grid (4 columns wide, 2 narrow),
   each with large value display, trend arrow (↑↓─), color-coded border, and a
   sparkline container div (id="sparkline-{metric}"). Time range selector
   (7d/30d/90d/All) that triggers hx-get re-fetch. Hotspot table below cards.

10) Create static/js/charts/xray-cards.js:
    Fetch /api/v1/xray. For each metric card render d3.area() sparkline in its
    container (80×24px, no axes, just the shape). Color gradient: emerald if
    healthy, amber if warning, rose if critical. Apply status color to card border.

11) Create static/js/charts/xray-hotspots.js:
    Render hotspot table rows. Risk score gets colored badge (emerald/amber/rose).
    Click row → hx-get="/dashboard/frag/blast-radius?symbol_id={id}" to navigate.
    Sortable columns (click header toggles sort by risk, pagerank, drift).

=== STEP 4: BLAST RADIUS EXPLORER ===

12) Create static/js/charts/symbol-search.js:
    Reusable symbol search component. Debounced input (300ms) querying
    /api/v1/search?q={input}&limit=10. Dropdown results with symbol name + file path.
    Keyboard navigation: ↑↓ to navigate, Enter to select, Esc to close.
    Click or Enter fires custom event 'aether:symbol-selected' with symbol_id.
    Used by both Blast Radius and Causal Explorer pages.

13) Add API endpoint GET /api/v1/blast-radius?symbol_id={id}&depth=3&min_coupling=0.2

    Wraps existing aether-analysis blast radius logic. Returns center node + rings
    array (one per hop distance). Each node includes: symbol_id, qualified_name,
    file_path, sir_intent (first sentence of SIR), pagerank, risk_score, has_tests,
    is_drifting, drift_score, coupling_to_parent (strength, type, signal breakdown).
    Also total_impacted count. Cap at 500 nodes per depth level. 404 if symbol not found.

14) Add HTMX fragment GET /dashboard/frag/blast-radius?symbol_id={id}
    Symbol search input at top (loads symbol-search.js). SVG container for radial tree.
    Depth slider (1-5, default 3). Min coupling threshold slider (0.0-1.0, default 0.2).
    Side panel area for symbol detail (populated on Shift+click).
    If no symbol_id param, show search prompt only.

15) Create static/js/charts/blast-radius.js:
    Radial tree visualization:
    - Center node at SVG center
    - Concentric rings as d3.arc() circles at fixed radii per hop distance
    - Ring labels: "1 hop (N symbols)", "2 hops (M symbols)", etc.
    - Node placement: d3.forceSimulation with forceRadial (correct ring) +
      forceCollide (no overlap within ring). If >200 total nodes, skip force sim
      and use deterministic angular positioning within each ring instead.
    - Node encoding: size=d3.scaleSqrt(PageRank), fill=riskColor(risk_score),
      stroke=solid if has_tests / dashed if not, opacity=freshness, badge if drifting
    - Edge encoding: solid=structural, dashed=semantic, dotted=temporal,
      width=coupling strength (1-4px)
    - Interactions: hover→aether-tooltip, click→re-center (re-fetch + animated
      transition), Shift+click→load detail in side panel, d3.zoom() for pan/zoom,
      depth slider→re-fetch, coupling slider→client-side filter

=== STEP 5: ARCHITECTURE MAP ===

16) Add API endpoint GET /api/v1/architecture?granularity=symbol
    Returns communities with files and symbol assignments, directory mapping,
    misplacement flags. "Misplaced" heuristic: symbol is in community X but its
    directory is dominated by community Y (>60% of directory symbols are community Y).
    Community label: most common directory prefix among members.
    Queries GraphStore for Louvain community detection results.
    If community detection hasn't run, return empty with "not_computed" flag.

17) Add HTMX fragment GET /dashboard/frag/architecture
    Stats bar: N communities, M misplaced symbols. Toggles: "Logical view" (default)
    vs "Directory view", "Show misplaced only". SVG container for treemap.

18) Create static/js/charts/architecture.js:
    Zoomable treemap using d3.treemap() with d3.treemapSquarify tiling.
    Hierarchy: root → communities → files → symbols.
    Cell color: communityColor(community_id). Misplaced symbols get SVG <pattern>
    fill with diagonal stripes over community color.
    Click community → zoom in with d3.zoom animated transition. Breadcrumb for back.
    "Show misplaced only" → fade non-misplaced to 0.15 opacity.
    "Directory view" → re-hierarchy as root → directories → files → symbols with
    community colors on symbols.
    Performance: >1000 symbols → aggregate to file level, expand on zoom.

=== STEP 6: TIME MACHINE ===

19) Add API endpoint GET /api/v1/time-machine?at={iso_timestamp}&layers=deps,drift
    Returns graph snapshot at time T: nodes (symbols existing at T, with community
    and drift_score_at_time), edges (dependencies at T), events in ±24hr window
    (drift events, added symbols, removed symbols), full time_range (earliest/latest).
    Query sir_versions for symbols existing at T (first_version_timestamp <= T),
    dependency_edges with timestamps.
    Performance: >500 symbols → aggregate to file-level nodes.

20) Add HTMX fragment GET /dashboard/frag/time-machine
    Timeline scrubber (range input spanning earliest-to-latest). Play/pause button.
    Speed selector (1x/2x/5x). Layer toggles: Dependencies, Drift events, Communities.
    SVG container for graph. Event log panel at bottom (scrollable list).

21) Create static/js/charts/time-machine.js:
    d3.forceSimulation for graph layout. d3.scaleTime for timeline.
    Custom SVG timeline below graph: horizontal axis with date labels, event markers
    (circles=drift amber, squares=added blue, diamonds=removed red), draggable handle.
    Drag handle → debounced fetch (500ms) → D3 enter/update/exit:
      New nodes: enterTransition(). Removed: exitTransition(). Existing: smooth move.
      New edges: fade in green tint. Removed: fade out red tint.
    Play mode: setInterval advances 1 day/tick at 1x speed, each tick fetches and animates.
    Layer toggles: show/hide edges, pulse drifting nodes, color by community.
    Event log: click event → center graph on symbol + flash highlight.

=== STEP 7: CAUSAL EXPLORER ===

22) Add API endpoint GET /api/v1/causal-chain?symbol_id={id}&depth=3&lookback=30d
    Wraps existing aether-analysis causal chain tracer. Returns target node + chain
    array, each with: symbol_id, qualified_name, timestamp, drift_score,
    sir_diff_summary (one sentence describing SIR change), causal_confidence,
    link_type (dependency or co_change), caused list. Plus overall_confidence.
    If SIR diff unavailable, use "No SIR diff available".

23) Add HTMX fragment GET /dashboard/frag/causal
    Symbol search (reuse symbol-search.js). Depth slider (1-5, default 3).
    Lookback selector (7d/30d/90d). SVG container. "Animate" button.

24) Create static/js/charts/causal-explorer.js:
    Horizontal DAG: manual topological sort + layered positioning.
    X = layer * layerWidth (by depth from target, right = most recent).
    Y = index * nodeHeight within layer. DO NOT import d3-dag — implement
    simple layered layout manually.
    Nodes: SVG <foreignObject> with HTML cards (name, date, drift indicator,
    SIR diff summary 1-2 lines). Border color = confidence (green/amber/gray).
    Target node: thicker border, subtle glow.
    Edges: d3.linkHorizontal(), solid=dependency, dashed=co-change, width=confidence.
    Hover → full SIR diff in tooltip. Click → re-center on that symbol.
    "Animate" → dim graph to 0.2, then highlight chain nodes left-to-right, 800ms/step.

=== STEP 8: SMART SEARCH UPGRADE ===

25) Enhance existing GET /api/v1/search response to include per-result:
    sir_summary (first 2-3 sentences of SIR intent), risk_score, pagerank,
    drift_score, test_count, related_symbols (top 3 most-coupled, each with
    symbol_id + qualified_name). All lookups on existing data. Return null if unavailable.

26) Replace existing search HTMX fragment with rich result cards:
    GET /dashboard/frag/search?q={query}&mode=hybrid&lang=&risk=&drift=
    Search input with mode toggle buttons (Lexical / Semantic / Hybrid).
    Filter sidebar: language checkboxes, risk level, drift status, has-tests.
    Rich result cards: symbol name (monospace) + file path, SIR summary (2-3 lines),
    metric badges (risk colored, PageRank, drift trend, test count),
    related symbols as clickable links. Click card → navigate to blast radius.

27) Create static/js/charts/smart-search.js:
    Mode toggle → re-fetch with mode param. Filter changes → re-fetch via hx-include.
    Keyboard navigation: ↑↓ between results, Enter to open.
    Result count indicator. Empty state message. Loading skeleton cards while fetching.

=== STEP 9: EXISTING PAGE UPGRADES ===

28) Apply design system to existing pages:
    - Add dark: Tailwind classes to ALL existing maud templates
    - Dependency Graph: enhance /api/v1/graph response to include pagerank and
      risk_score per node. Update existing D3 to use node size=PageRank,
      node color=riskColor(risk_score). Add right-click context menu with
      "Show Blast Radius" and "Trace Causes" links.
    - Drift Report: add color-coded table rows by severity (emerald/amber/rose)
    - Coupling Map: add signal-type color encoding on heatmap cells
      (blue=structural, green=semantic, orange=temporal)
    - Health: apply theme classes. Keep functional as-is (X-Ray is the new
      primary health view but Health page still works).

    DO NOT break existing D3 chart functionality. Upgrades are additive only:
    new CSS classes, enhanced API response fields, updated color scales.

=== STEP 10: TESTS ===

29) Unit tests in crates/aether-dashboard/:
    - Test each new JSON API endpoint with mock/minimal SharedState
    - Verify JSON structure matches spec (data + meta envelope)
    - Verify new HTMX fragments return valid HTML
    - Test xray endpoint with empty data returns null metrics with "not_computed"
    - Test blast-radius with invalid symbol_id returns 404
    - Test architecture with no community data returns "not_computed"
    - Test search enhancements include new fields
    - Test theme toggle doesn't break existing pages

30) Integration test in crates/aether-dashboard/tests/:
    - Extend existing integration tests
    - GET /dashboard/ → 200, HTML includes theme initialization script
    - GET /api/v1/xray → 200, valid JSON with metrics and hotspots
    - GET /api/v1/blast-radius?symbol_id=nonexistent → 404
    - GET /api/v1/architecture → 200, valid JSON
    - GET /api/v1/time-machine?at=2026-01-01T00:00:00Z → 200, valid JSON
    - GET /api/v1/causal-chain?symbol_id=test → 200, valid JSON
    - GET /dashboard/frag/xray → 200, HTML fragment
    - Verify no regressions: existing endpoints still return expected shapes

=== STEP 11: VALIDATION ===

31) Run full validation:
    - cargo fmt --all --check
    - cargo clippy --workspace --features dashboard -- -D warnings
    - cargo test -p aether-core
    - cargo test -p aether-config
    - cargo test -p aether-store
    - cargo test -p aether-parse
    - cargo test -p aether-sir
    - cargo test -p aether-infer
    - cargo test -p aether-memory
    - cargo test -p aether-lsp
    - cargo test -p aether-analysis
    - cargo test -p aether-mcp
    - cargo test -p aether-query
    - cargo test -p aether-dashboard
    - cargo test -p aetherd --features dashboard
    Do NOT use cargo test --workspace (OOM risk on WSL2 with 12GB RAM).

32) Report:
    - Which steps were applied vs. skipped (with reason)
    - Validation command outcomes (pass/fail per crate)
    - Any files modified outside scope (should be zero)
    - Total lines changed

33) Commit with message:
    "Phase 7.9: Add dashboard visual intelligence — X-Ray, Blast Radius, Architecture Map, Time Machine, Causal Explorer, Smart Search"

SCOPE GUARD:
- Do NOT add new crates — all changes in aether-dashboard
- Do NOT add new CDN dependencies — D3 v7 has everything needed
- Do NOT migrate to React, Vue, Svelte, or any JS framework (Decision #41 holds)
- Do NOT add WebSocket/SSE — HTMX polling at 30s is sufficient
- Do NOT add Node.js as a build dependency
- Do NOT modify existing MCP tool schemas or CLI subcommands
- Do NOT add new SQLite tables or SurrealDB schema
- Do NOT add user accounts, saved views, or mobile layout
- Do NOT import d3-dag or any external D3 modules — use D3 v7 built-ins only
- If data not available for a metric, return null with "not_computed" — do NOT skip endpoint
- If a D3 visualization is too complex, simplify to a working version first.
  Working > pretty. Can be polished later.
- If SharedState doesn't expose a method you need, add a helper function in
  the dashboard crate that queries SqliteStore/GraphStore directly — do NOT
  modify SharedState or store traits for dashboard-only needs.
- If any step cannot be applied because the code structure differs from what's
  described here, report exactly what you found and skip that step.
```
