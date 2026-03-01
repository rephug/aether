# Phase 7 — The Pathfinder

## Stage 7.9 — Dashboard Visual Intelligence

**Prerequisites:** Stage 7.6 (Web Dashboard), Stage 7.8 (OpenAI-Compat Provider)
**Feature Flag:** `--features dashboard`
**Estimated Codex/Claude Code Runs:** 3–4

---

## Purpose

Transform the Stage 7.6 dashboard from functional data display into a visual intelligence surface that showcases what only AETHER can do. This stage adds six new interactive pages, a dark/light theme system, polished typography, and the kind of visualization depth that makes someone watching a demo say "wait, go back — what was that?"

The goal is twofold: **daily utility** (screens you open every morning) and **competitive moat** (screens no other tool can produce because no other tool has AETHER's cross-layer data).

---

## What Changes vs. 7.6

Stage 7.6 built the plumbing — Axum router, HTMX navigation, JSON APIs, D3 chart containers, `rust-embed` static file embedding, SharedState integration. All of that stays. Stage 7.9 adds:

1. **Design system** — dark/light theme, consistent color palettes, typography, spacing
2. **6 new pages** — each built on data that already exists in AETHER's stores
3. **Upgraded existing pages** — existing charts get the new design system treatment
4. **No new Rust crates** — all changes are in `aether-dashboard` (new API endpoints + new embedded static files)

---

## Design System

### Theme

Toggle between light and dark mode. Persist choice in `localStorage`. Respect `prefers-color-scheme` as default. Tailwind's `dark:` prefix handles all styling.

```html
<!-- In dashboard shell, before </head> -->
<script>
  if (localStorage.theme === 'dark' || (!localStorage.theme && 
      window.matchMedia('(prefers-color-scheme: dark)').matches)) {
    document.documentElement.classList.add('dark');
  }
</script>
```

Toggle button in the sidebar footer. No additional JS framework.

### Color Palettes

Each page type gets a distinct accent palette so the dashboard feels like a curated collection, not a single template stamped six times:

| Page | Primary Accent | Purpose |
|------|---------------|---------|
| X-Ray (Health) | Emerald → Amber → Rose | Status traffic light |
| Blast Radius | Concentric rings: Indigo → Violet → Rose | Distance = danger |
| Architecture Map | Community colors from `d3.schemeTableau10` | Module identity |
| Time Machine | Blue cool → Warm amber | Past → Present gradient |
| Causal Explorer | Amber chain links on dark slate | Investigation trail |
| Search | Neutral with colored badges per result type | Non-distracting |

### Typography

- Headers: `font-sans` (system stack) at `text-2xl` / `text-lg`
- Body: `text-sm` for data density, `text-base` for descriptions
- Monospace: `font-mono` only for symbol names, file paths, code snippets
- No custom font imports (CDN Tailwind handles the stack)

### Shared D3 Utilities

```
static/js/
├── aether-theme.js      # Color scales, palette access, dark mode detection
├── aether-tooltip.js    # Shared tooltip component (positioned, themed, HTML content)
├── aether-responsive.js # ResizeObserver wrapper for SVG container auto-resize
└── aether-animate.js    # Shared transition helpers (enter/exit/update pattern)
```

These are imported by each page's chart module. Keeps individual chart files focused on data, not plumbing.

---

## New Pages

### 1. Codebase X-Ray (`/dashboard/xray`)

**What it is:** The screen you open every morning. A single-glance health dashboard with metric cards, sparkline trends, and a risk hotspot table.

**Why it matters:** No other tool combines graph centrality, semantic drift, test coverage, SIR quality, and access patterns into a composite risk score per symbol. SonarQube counts cyclomatic complexity. AETHER tells you "this function is the 3rd most critical in your codebase, has drifted semantically twice this month, has zero test guards, and three modules depend on it."

**Layout:**
```
┌──────────────────────────────────────────────────────────┐
│  CODEBASE X-RAY                               [7d ▾]    │
├──────┬──────┬──────┬──────┬──────┬──────┬──────┬────────┤
│ SIR  │Orphan│ Avg  │Graph │Couple│Cover │Fresh │ Risk   │
│Cover │Count │Drift │Conn. │Pairs │ age  │ness  │ Score  │
│ 87%  │  4   │ 0.12 │ 0.94 │  2   │ 91%  │ 2m   │  B+   │
│  ↑3% │ ↓2   │ ↓.03 │  ─   │ ↑1   │ ↑5%  │  ─   │  ↑    │
│ ████ │ ▃▅▂▁ │ ▅▃▂▁ │ ▇▇▇▇ │ ▁▂▃▅ │ ▃▅▇█ │ ████ │ ▃▅▇█ │
└──────┴──────┴──────┴──────┴──────┴──────┴──────┴────────┘
┌──────────────────────────────────────────────────────────┐
│  RISK HOTSPOTS                          [▾ All modules]  │
├──────────────────────────────────────────────────────────┤
│  ⚠ parse_document()    Risk: 0.89  PageRank: 0.94      │
│    High centrality + drifting + no tests                 │
│  ⚠ resolve_imports()   Risk: 0.76  PageRank: 0.71      │
│    Recent drift + 3 dependents + stale SIR               │
│  ● validate_config()   Risk: 0.45  PageRank: 0.52      │
│    Moderate centrality, well tested                      │
└──────────────────────────────────────────────────────────┘
```

**Metrics (8 cards):**

| Metric | Computation | Good | Warn | Critical |
|--------|------------|------|------|----------|
| SIR Coverage | symbols_with_sir / total_symbols | >90% | 70-90% | <70% |
| Orphan Count | symbols not in any connected component with main graph | <5 | 5-20 | >20 |
| Avg Drift | mean(drift_scores) across all tracked symbols | <0.15 | 0.15-0.30 | >0.30 |
| Graph Connectivity | largest_component_size / total_nodes | >0.9 | 0.7-0.9 | <0.7 |
| High-Coupling Pairs | count of module pairs with fused_score > 0.7 | <3 | 3-8 | >8 |
| SIR Coverage | symbols_with_sir / total_symbols | >90% | 70-90% | <70% |
| Index Freshness | time since last index completion | <1hr | 1-24hr | >24hr |
| Overall Risk Score | weighted composite → letter grade A+ through F | A/B | C | D/F |

Each card: large value, trend arrow (↑↓─), sparkline (last 30 data points via `d3.area()`), color-coded border (emerald/amber/rose).

**Time range selector:** 7d / 30d / 90d / All — changes sparkline window and trend calculations.

**Hotspot table:** Top 10 symbols by composite risk score. Click row → navigates to Blast Radius page centered on that symbol.

**API endpoint:** `GET /api/v1/xray?window=7d`

---

### 2. Blast Radius Explorer (`/dashboard/blast-radius`)

**What it is:** Select any symbol → see everything that would break if you changed it, visualized as concentric impact rings with importance sizing and risk coloring.

**Why it matters:** This is AETHER's signature visualization. No other tool can produce it because no other tool has the combination of AST dependency edges + semantic similarity coupling + temporal co-change data + PageRank importance scores + SIR-level understanding of what each symbol actually does.

**Interaction:**
- Search box at top (HTMX-powered, debounced) to find a symbol
- Or: arrive from X-Ray hotspot click, or from Graph page node context menu
- Selected symbol appears at center
- Concentric rings at 1-hop, 2-hop, 3-hop distances
- Ring labels show hop count and total symbol count at that distance

**Node encoding:**
- **Size** = PageRank importance (bigger = more critical)
- **Color** = composite risk score (emerald → amber → rose)
- **Border** = has tests (solid) vs no tests (dashed)
- **Opacity** = SIR freshness (bright = fresh, faded = stale)
- **Icon badge** = ⚠ if currently drifting

**Edge encoding:**
- **Style** = coupling type:
  - Solid = structural (import/call dependency)
  - Dashed = semantic (embedding similarity, no direct import)
  - Dotted = temporal (co-change frequency, no structural or semantic link)
- **Width** = coupling strength
- **Color** = inherits from coupling type (blue/green/orange matching Coupling page)

**Interaction:**
- Hover node → tooltip with: qualified name, SIR intent summary (first sentence), risk score, PageRank, file path
- Click node → re-center blast radius on that symbol (animated transition)
- Click node + hold Shift → open symbol detail panel (HTMX side panel)
- Mouse wheel → zoom. Drag background → pan.
- "Depth" control: slider 1–5 hops (default 3)
- "Min coupling" threshold slider: hide weak edges to reduce noise

**D3 approach:** Custom radial layout using `d3.tree()` with radial projection. Nodes positioned on concentric `d3.arc()` rings. Force simulation within each ring for collision avoidance. `d3.zoom()` for pan/zoom.

**API endpoint:** `GET /api/v1/blast-radius?symbol_id={id}&depth=3&min_coupling=0.2`

This wraps the existing `aether_blast_radius_logic` MCP tool but enriches the response with PageRank scores, SIR summaries, and test guard presence.

---

### 3. Architecture Map (`/dashboard/architecture`)

**What it is:** A zoomable treemap showing your codebase's actual module boundaries (from community detection) overlaid on the directory structure. Highlights where the logical architecture diverges from the physical layout.

**Why it matters:** Most teams think they know their architecture. AETHER's community detection reveals the *actual* architecture — which symbols cluster together by dependency, co-change, and semantic similarity, regardless of what directory they're in. A function in `utils/` that is logically part of the `auth` module shows up in the `auth` community, not the `utils` community. This reveals misplaced code, hidden coupling, and architectural erosion.

**Layout:**

Top level: treemap cells = communities (from Louvain/SCC detection). Cell size proportional to symbol count. Cell color from `d3.schemeTableau10`.

Within each cell: nested treemap of files → symbols. Symbol color matches community. Symbols in a community that don't match their file's directory → highlighted with a "misplaced" indicator (diagonal stripes pattern).

```
┌─────────────────────┬──────────────┬──────────┐
│  AUTH MODULE         │  PARSER      │  STORE   │
│  ┌────┬────┬────┐   │  ┌────┬───┐  │  ┌────┐  │
│  │auth│auth│////│   │  │pars│prs│  │  │sto │  │
│  │.rs │_mw │util│   │  │e.rs│_ts│  │  │re  │  │
│  │    │.rs │.rs │   │  │    │.rs│  │  │.rs │  │
│  └────┴────┴────┘   │  └────┴───┘  │  └────┘  │
│  12 symbols         │  8 symbols   │  5 sym.  │
│  3 misplaced ⚠      │  1 misplaced │  0       │
└─────────────────────┴──────────────┴──────────┘
```

**Interaction:**
- Click community cell → zoom into that community showing all symbols
- Click symbol → tooltip with SIR intent, file path, community assignment
- Toggle "Show misplaced only" → fades out correctly-placed symbols
- Toggle "Show directory view" → switches to treemap by directory (physical layout) with community colors overlaid. Makes it immediately obvious when a directory contains symbols from 4 different communities.
- "Community count" indicator in corner

**D3 approach:** `d3.treemap()` with squarified tiling. Nested data hierarchy: community → file → symbol. `d3.zoom()` with animated transitions between levels. Diagonal stripe pattern via SVG `<pattern>` for misplaced indicators.

**API endpoint:** `GET /api/v1/architecture?granularity=symbol`

Returns: communities with symbol assignments, directory mapping, misplacement flags.

---

### 4. Time Machine (`/dashboard/time-machine`)

**What it is:** A temporal graph explorer that lets you scrub through time and watch the dependency graph evolve. See modules appear, dependencies form, drift events propagate, and coupling patterns emerge.

**Why it matters:** AETHER is the only tool that stores versioned semantic understanding (SIR history) alongside structural changes (dependency edges over time). The Time Machine makes this temporal data tangible. It answers questions no static analysis tool can: "When did these two modules become coupled?" "What changed in the architecture after the big refactor?" "Is the codebase getting more or less modular over time?"

**Layout:**
```
┌──────────────────────────────────────────────────────────┐
│  ◄ ▶  ████████████░░░░░░░░  Feb 15                      │
│       Jan 1                            Feb 28            │
├──────────────────────────────────────────────────────────┤
│                                                          │
│            ┌───┐     ┌───┐                               │
│     ┌───┐──│ B │─────│ C │──┌───┐                        │
│     │ A │  └───┘     └───┘  │ D │                        │
│     └───┘       ╲           └───┘                        │
│                  ╲  ┌───┐                                │
│                   ╲─│ E │ (appeared Feb 3)               │
│                     └───┘                                │
│                                                          │
├──────────────────────────────────────────────────────────┤
│  ▸ Play  │  Speed: 1x ▾  │  Layers: [✓] Deps [✓] Drift │
│  Events: +E (Feb 3)  ~B→C drift (Feb 10)  +D→E (Feb 12)│
└──────────────────────────────────────────────────────────┘
```

**Timeline scrubber:**
- Horizontal range slider spanning the project's SIR version history
- Drag to any point → graph redraws to show the dependency state at that time
- Play button: auto-advance through time with configurable speed (1x, 2x, 5x, 10x)
- Event markers on the timeline: dots for drift events (amber), squares for new symbols (blue), diamonds for removed symbols (red)

**Graph state at time T:**
- Show only symbols and edges that existed at time T (based on SIR version timestamps)
- Newly appeared nodes since previous frame: animate in with a glow/pulse
- Removed nodes: fade out
- Drift events: affected node flashes amber
- Edge changes: new edges animate in (green), removed edges fade (red)

**Layer toggles:**
- Dependencies (structural edges)
- Drift events (amber flashes on drifted symbols)
- Coupling (edge thickness modulated by temporal coupling at that point in time)
- Communities (node colors shift as community detection results change)

**Event log panel (bottom):**
- Scrolling list of events that occurred at the current time point
- Click event → center graph on the affected symbol

**D3 approach:** `d3.forceSimulation()` for the graph layout (same engine as 7.6 Graph page). `d3.scaleTime()` for the timeline. Custom interpolation between graph states using D3's enter/update/exit pattern. `d3.transition()` for smooth node position changes between time steps.

**Performance:** Pre-compute graph snapshots at configurable intervals (daily for large repos, per-commit for small ones). The API returns the full snapshot for the requested time point. Interpolation between snapshots is handled client-side.

**API endpoint:** `GET /api/v1/time-machine?at={iso_timestamp}&layers=deps,drift`

Returns: graph snapshot (nodes + edges) at the requested time, plus events in a surrounding window.

---

### 5. Causal Explorer (`/dashboard/causal`)

**What it is:** Pick a symbol that changed → trace backwards through the dependency and co-change graph to find *why* it changed. Animated trail visualization that tells the story of a change propagating through the codebase.

**Why it matters:** This is AETHER's causal chain tracing (Stage 6.7) made visual. When something breaks, the first question is always "what changed?" The second question — which no other tool answers — is "why did it change?" AETHER traces through semantic drift, dependency edges, co-change patterns, and SIR diff history to reconstruct the causal narrative.

**Layout:**
```
┌──────────────────────────────────────────────────────────┐
│  CAUSAL EXPLORER                                         │
│  Target: parse_document()  [Change search...]            │
├──────────────────────────────────────────────────────────┤
│                                                          │
│  [Feb 20]        [Feb 18]         [Feb 15]               │
│  parse_doc() ←── validate() ←──── Config change          │
│  Drift: 0.34     Drift: 0.21      Drift: 0.45           │
│  "intent         "added new       "added TLS             │
│   shifted to      validation       field, changed        │
│   handle PDF"     rule"            all validators"        │
│                                                          │
│          ╲                                               │
│           ╲─── [Feb 19]                                  │
│                extract_text() ←── pdf_parser update       │
│                Drift: 0.18        (external dep)         │
│                "signature changed"                        │
│                                                          │
├──────────────────────────────────────────────────────────┤
│  Chain depth: 3    Confidence: 0.82    Lookback: 30d     │
│  [Expand depth]    [Export timeline]                      │
└──────────────────────────────────────────────────────────┘
```

**Visualization:** Horizontal left-to-right timeline of causation. The target symbol is on the right (most recent). Causes flow from left to right. Branching shows multiple causal paths converging.

**Node encoding:**
- Each node is a (symbol, timestamp) pair showing a specific change event
- Node card contains: symbol name, date, drift score, and the SIR diff summary (one sentence from the LLM-generated SIR explaining what changed)
- Node border color: confidence that this node is actually causal (green = high, amber = medium, gray = speculative)

**Edge encoding:**
- Solid line = direct dependency (A calls B, B changed → A affected)
- Dashed line = co-change correlation (A and B always change together, likely coupled)
- Width = causal confidence score

**Interaction:**
- Search box to select target symbol
- Click any node in the chain → expand to show its own causes (drill deeper)
- "Lookback" slider: how far back in time to search (7d / 30d / 90d)
- "Depth" control: max chain length (1–5, default 3)
- "Animate" button: replay the causal chain as a step-by-step animation, highlighting each link in sequence
- Hover node → full SIR diff popup (before/after intent comparison)

**D3 approach:** Custom DAG layout using `d3.dagStratify()` from `d3-dag` (CDN) or manual topological sort with `d3.tree()`. Horizontal orientation. `d3.transition()` for animation playback. Cards rendered as `<foreignObject>` in SVG for rich HTML content.

**API endpoint:** `GET /api/v1/causal-chain?symbol_id={id}&depth=3&lookback=30d`

Wraps existing `aether_trace_cause` MCP tool with enriched response (SIR diffs, confidence scores).

---

### 6. Smart Search (`/dashboard/search`)

**What it is:** The existing search page rebuilt with rich result cards showing SIR intelligence inline — not just "file:line" matches but semantic context, risk badges, drift status, and related symbols.

**Why it matters:** Search is the most frequently used feature. Upgrading it from a plain results list to rich intelligence cards demonstrates AETHER's depth on every query. A developer searching for "parse" doesn't just find functions with "parse" in the name — they see each result's intent, risk level, test coverage, and related symbols, all without clicking through.

**Result card layout:**
```
┌──────────────────────────────────────────────────────────┐
│  fn parse_document()                    Risk: ⚠ HIGH     │
│  crates/aether-parse/src/parser.rs:47                    │
│                                                          │
│  "Parses source files using tree-sitter grammars,        │
│   extracting symbol boundaries and dependency edges.      │
│   Returns a ParseResult with symbols and diagnostics."    │
│                                                          │
│  PageRank: 0.94  │  Drift: 0.12 ↓  │  Tests: 3  │  Py  │
│  ───────────────────────────────────────────────────────  │
│  Related: extract_symbols() · resolve_imports() · +4     │
└──────────────────────────────────────────────────────────┘
```

**Features:**
- **SIR summary** displayed directly in result card (first 2-3 sentences of `sir.intent`)
- **Badges:** Risk level (colored), language icon, domain tag (code/legal/finance)
- **Inline metrics:** PageRank, drift trend, test count
- **Related symbols:** top 3 most-coupled symbols shown as clickable links
- **Search mode toggle:** Lexical / Semantic / Hybrid (with visual indicator of which mode is active)
- **Filters sidebar:** Language, module, risk level, drift status, has-tests
- **Keyboard navigation:** ↑↓ to move between results, Enter to open detail, / to focus search box

**API endpoint:** Enhances existing `GET /api/v1/search?q={query}` with additional fields in response: `sir_summary`, `risk_score`, `pagerank`, `drift_score`, `test_count`, `related_symbols`.

---

## Upgrades to Existing Pages

### Overview (`/dashboard/`)
- Apply new design system (theme, typography, spacing)
- Replace plain stats with mini X-Ray cards (same component, fewer metrics)
- Add "Quick Actions" section: links to common workflows (search, blast radius for top-risk symbol, recent drift events)

### Dependency Graph (`/dashboard/graph`)
- Apply node encoding from Blast Radius (size=PageRank, color=risk, border=tests)
- Add right-click context menu: "Show Blast Radius", "Trace Causes", "View in Architecture Map"
- Node tooltip: show SIR intent summary (first sentence)

### Drift Report (`/dashboard/drift`)
- Upgrade line chart to multi-line with module selector (top N driftiest)
- Add brush selection for time range zoom
- Color-code table rows by severity

### Coupling Map (`/dashboard/coupling`)
- Add signal-type color encoding (blue=structural, green=semantic, orange=temporal)
- Click cell → slide-in panel showing: specific symbol pairs driving the coupling, co-change commits, shared dependencies

### Health (`/dashboard/health`)
- Merge into X-Ray page as the primary health view (avoid two separate "health" pages)
- Keep radar chart as a visualization option within X-Ray for users who prefer it

---

## Sidebar Navigation (Updated)

```
AETHER Dashboard
├── X-Ray              ← NEW (replaces Overview as landing page)
├── Search             ← UPGRADED
├── ──────────────
├── Blast Radius       ← NEW
├── Architecture Map   ← NEW
├── Time Machine       ← NEW
├── Causal Explorer    ← NEW
├── ──────────────
├── Dependency Graph   ← UPGRADED
├── Drift Report       ← UPGRADED
├── Coupling Map       ← UPGRADED
├── ──────────────
├── [☀/🌙] Theme
└── AETHER v0.X.Y
```

X-Ray is the new landing page (most useful at-a-glance). Search is second because it's the most used. The four new exploration pages are grouped together. The three upgraded original pages are at the bottom.

---

## New API Endpoints

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `GET /api/v1/xray` | GET | Composite health metrics + sparkline data + risk hotspots |
| `GET /api/v1/blast-radius` | GET | Symbol impact tree with PageRank, risk, coupling types |
| `GET /api/v1/architecture` | GET | Community assignments, directory mapping, misplacement flags |
| `GET /api/v1/time-machine` | GET | Graph snapshot at timestamp + event list |
| `GET /api/v1/causal-chain` | GET | Causal chain for a symbol with SIR diffs |

Existing endpoints enhanced:
| Endpoint | Enhancement |
|----------|-------------|
| `GET /api/v1/search` | Add: sir_summary, risk_score, pagerank, drift_score, test_count, related_symbols |
| `GET /api/v1/graph` | Add: pagerank per node, risk_score per node, community assignment |
| `GET /api/v1/drift` | Add: per-module aggregation, time-series with configurable granularity |
| `GET /api/v1/coupling` | Add: signal-type breakdown per cell, symbol-level detail |

All endpoints follow the existing JSON envelope: `{ "data": {...}, "meta": { "timestamp": ..., "stale": bool } }`.

---

## New Static Files

```
static/
├── js/
│   ├── aether-theme.js         # Theme toggle, color scale access, dark mode
│   ├── aether-tooltip.js       # Shared tooltip (positioned, themed, rich HTML)
│   ├── aether-responsive.js    # ResizeObserver auto-resize for SVG containers
│   ├── aether-animate.js       # Shared transition helpers
│   ├── charts/
│   │   ├── xray-cards.js       # Sparkline metric cards
│   │   ├── xray-hotspots.js    # Risk hotspot table
│   │   ├── blast-radius.js     # Radial impact tree
│   │   ├── architecture.js     # Zoomable treemap
│   │   ├── time-machine.js     # Temporal graph with scrubber
│   │   ├── causal-explorer.js  # DAG causal chain
│   │   └── smart-search.js     # Rich result cards
│   └── upgrades/
│       ├── graph-enhanced.js   # Enhanced force graph (context menu, risk encoding)
│       ├── drift-enhanced.js   # Multi-line with brush
│       └── coupling-enhanced.js # Signal-type colors + detail panel
├── css/
│   └── aether-dashboard.css    # Minimal custom CSS beyond Tailwind (animations, SVG patterns)
└── icons/
    └── risk-badges.svg         # Inline SVG sprites for risk/language/domain badges
```

All files embedded via `rust-embed` / `include_dir!` — same pattern as 7.6. No build step.

CDN additions (added to dashboard shell `<head>`):
```html
<!-- Existing -->
<script src="https://cdnjs.cloudflare.com/ajax/libs/htmx/2.0.4/htmx.min.js"></script>
<script src="https://cdnjs.cloudflare.com/ajax/libs/d3/7.9.0/d3.min.js"></script>
<script src="https://cdn.tailwindcss.com"></script>

<!-- New for 7.9 -->
<!-- None. D3 v7 includes everything needed. No additional libraries. -->
```

Zero new CDN dependencies. D3 v7 includes the force simulation, treemap, zoom, brush, chord, and arc modules. The `d3-dag` layout is implemented manually with topological sort (20 lines) rather than adding a CDN dependency.

---

## Out of Scope

- React/Vue/Svelte migration (Decision #41 holds — HTMX + D3 is sufficient for 11 pages)
- WebSocket/SSE real-time updates (HTMX polling at 30s intervals is sufficient)
- PDF/image export of visualizations (future enhancement)
- Custom dashboard layouts or drag-and-drop widget arrangement
- User accounts or saved views
- Mobile responsive layout (desktop-first, minimum 1024px width)
- Legal/Finance vertical-specific visualizations (those come with their respective phases)

---

## Implementation Notes

### Splitting the work

This stage is larger than typical stages. Recommended split for Claude Code sessions:

**Run 1: Foundation + X-Ray**
- Design system files (theme, tooltip, responsive, animate)
- Sidebar navigation update
- X-Ray page (API + fragment + D3 sparklines + hotspot table)
- Theme toggle
- Existing page style upgrades (apply design system to Overview, Graph, Drift, Coupling, Health)

**Run 2: Blast Radius + Architecture Map**
- Blast radius API endpoint + fragment + radial tree visualization
- Architecture map API endpoint + fragment + zoomable treemap
- Symbol search component (shared between Blast Radius and Causal Explorer)

**Run 3: Time Machine + Causal Explorer + Smart Search**
- Time machine API endpoint + fragment + temporal graph + scrubber
- Causal explorer API endpoint + fragment + DAG visualization
- Smart search upgrade (enhanced API response + rich result cards + filters)

### Data availability

All data for these visualizations already exists:

| Visualization | Data Source | Already Computed? |
|---------------|------------|-------------------|
| X-Ray metrics | SqliteStore (SIR counts, symbols), GraphStore (PageRank, connectivity), aether-analysis (drift, coupling) | ✅ Yes — Stage 6.6, 6.8 |
| Blast radius | aether-analysis::CouplingAnalyzer::blast_radius() | ✅ Yes — Stage 6.2 |
| Architecture | GraphStore community detection (Louvain) | ✅ Yes — Stage 6.6 |
| Time machine | sir_versions table (temporal SIR history), dependency_edges with timestamps | ✅ Yes — Stage 2.1, 4.4 |
| Causal chain | aether-analysis causal chain tracing | ✅ Yes — Stage 6.7 |
| Smart search | Unified search + SIR metadata + risk scores | ✅ Yes — Stage 6.5, 6.8 |

No new analysis algorithms needed. Stage 7.9 is purely API wrappers + visualization.

### Performance guardrails

| Page | Concern | Mitigation |
|------|---------|------------|
| Blast Radius | Large graphs (10K+ symbols at depth 5) | Default depth=3, max depth=5, cap at 500 nodes, paginate |
| Architecture | Many communities with many symbols | Aggregate to file level by default, expand to symbols on zoom |
| Time Machine | Hundreds of time points | Pre-compute daily snapshots server-side, interpolate client-side |
| Causal Explorer | Deep chains (depth 5+) | Default depth=3, lazy-load deeper chains on "Expand" click |
| Smart Search | Large result sets | Paginate at 20 results, lazy-load cards as user scrolls |

---

## Pass Criteria

1. Dark/light theme toggle works across all pages. Persists on refresh.
2. X-Ray page displays all 8 metric cards with sparklines and correct color coding. Hotspot table shows top 10 risk symbols.
3. Blast Radius: selecting a symbol renders radial tree. Re-centering on a downstream node works. Depth slider changes ring count.
4. Architecture Map: treemap renders communities with distinct colors. Zoom into a community shows files and symbols. Misplaced symbols are visually distinct.
5. Time Machine: scrubber changes graph state. Play button auto-advances. New nodes animate in, removed nodes fade out.
6. Causal Explorer: selecting a target symbol renders the causal DAG. Click a node expands deeper causes. Animation replays chain step by step.
7. Smart Search: results show SIR summaries, risk badges, metrics, and related symbols. Mode toggle switches search type. Filters narrow results.
8. Existing pages (Graph, Drift, Coupling) reflect design system updates (theme, node encoding, context menus).
9. All pages handle empty data gracefully ("No data — run indexing first" message, not blank page or JS error).
10. All visualizations redraw on window resize. No overflow, no truncation.
11. Page load time < 3 seconds on a workspace with 5,000 symbols (API response + D3 render).
12. Zero new CDN dependencies beyond what 7.6 established.
13. `cargo fmt --all --check`, `cargo clippy --workspace --features dashboard -- -D warnings` pass.
14. `cargo test -p aether-dashboard` passes (API endpoint tests with mock data).
15. Existing MCP tools and CLI commands completely unaffected.

---

## Estimated Effort

3–4 Claude Code runs, split as described in Implementation Notes. The hardest visualization is Time Machine (temporal state management + animation). Blast Radius is the highest-value demo moment.
