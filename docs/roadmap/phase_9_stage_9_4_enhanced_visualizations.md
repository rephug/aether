# Phase 9 — The Beacon

## Stage 9.4 — Enhanced Visualizations

### Purpose

Elevate the dashboard from functional data display to genuinely compelling visual intelligence. This stage adds five new interactive D3 visualization pages that make AETHER's graph data tangible: blast radius explorer, drift timeline, coupling chord diagram, project memory narrative, and health scorecard with sparklines. These visualizations are the "demo moments" — the screens that make someone watching over your shoulder say "what is that?"

### What Problem This Solves

The dashboard has grown to 27+ pages with 29 API routes, 28 fragment templates, and 9 existing chart modules (`anatomy.js`, `architecture.js`, `blast-radius.js`, `causal-explorer.js`, `smart-search.js`, `symbol-search.js`, `time-machine.js`, `xray-cards.js`, `xray-hotspots.js`). The existing visualizations provide functional data display and some interactive exploration, but several new intelligence surfaces from Phases 10.1–10.6 and Repo R.1–R.4 have no visual representation yet:

- **Staleness data** (10.2) exists but isn't presented as a heatmap — you can't glance at which areas are rotting
- **Drift data** has per-symbol scores but no temporal view — you can't see trends over time
- **Coupling analysis** produces numbers but not an intuitive visual of module relationships
- **Project memory entries** are a list, not a narrative timeline
- **Task context history** (10.6) has no visual — you can't see which tasks touched which symbols
- **Fingerprint history** (10.1) is stored but not surfaced — prompt hash changes over time are invisible

Each new visualization transforms raw AETHER data into an interactive experience that delivers immediate insight.

### In scope

#### 1. Blast Radius Explorer (UPGRADE — existing `blast-radius.js`)

**What it shows:** Select any symbol → see a radial tree of everything that would be affected if it changed, with distance rings for 1-hop, 2-hop, 3-hop dependencies.

**Current state:** `blast-radius.js` exists with basic functionality and an API endpoint at `/api/v1/blast-radius`. This stage upgrades it to a radial tree layout with staleness coloring from 10.2's continuous monitor data.

**Interaction:**
- Click any symbol in the dashboard (search results, graph nodes, hover cards) → "Show Blast Radius" button
- Radial tree layout with the selected symbol at center
- Rings at each hop distance (concentric circles)
- Node size = PageRank importance score
- Node color = staleness (green → yellow → red)
- Click any downstream node to re-center the blast radius on it
- Hover shows: symbol name, SIR intent summary, staleness age, file path

**Data source:** `GET /api/v1/blast-radius?symbol_id={id}&depth=3` — already implemented as MCP tool `aether_blast_radius_logic`, needs a JSON API wrapper.

**D3 approach:** `d3.tree()` with radial projection (`d3.linkRadial()`). Node radius mapped to PageRank. Color scale via `d3.scaleSequential(d3.interpolateRdYlGn)`.

#### 2. Drift Timeline

**What it shows:** Semantic drift scores over time, per module or per file, as a scrollable multi-line chart with brushable time selection.

**Interaction:**
- X-axis: time (SIR version timestamps)
- Y-axis: drift score (0.0–1.0)
- One line per module (top 10 driftiest by default)
- Brush selection on X-axis to zoom into a time range
- Hover line → tooltip with module name, drift score, and triggering commit
- Click a point → jump to the SIR diff view for that version transition
- Toggle: show individual files vs. module aggregates

**Data source:** `GET /api/v1/drift-timeline?modules=top10&since={date}` — needs new API endpoint that queries `sir_versions` + drift analysis per time window.

**D3 approach:** `d3.line()` with `d3.scaleTime()` X-axis. `d3.brush()` for time selection. `d3.voronoi()` for hover detection on dense line charts.

#### 3. Coupling Chord Diagram

**What it shows:** Module-to-module coupling strength as an interactive chord diagram. Thick chords = strong coupling. Colors represent coupling type (structural, semantic, temporal).

**Interaction:**
- Outer ring: modules (or configurable: files, directories)
- Chords between modules: width proportional to coupling score
- Chord color: blend of coupling signal types
  - Blue = structural (import/dependency edges)
  - Green = semantic (embedding similarity)
  - Orange = temporal (co-change frequency)
- Hover a module → highlight all its chords, fade others
- Click a chord → detail panel showing the coupling breakdown (which signals, specific symbols involved)
- Slider: coupling threshold filter (0.0–1.0) — hide weak couplings to reduce visual noise

**Data source:** `GET /api/v1/coupling-matrix?granularity=module&threshold=0.3` — wraps existing `aether-analysis` coupling computation.

**D3 approach:** `d3.chord()` with `d3.ribbon()`. Custom color interpolation for multi-signal coupling. `d3.arc()` for the outer ring segments.

#### 4. Project Memory Timeline

**What it shows:** A visual narrative of the project's evolution — decision points, major changes, ownership shifts, and SIR generation milestones laid out on a horizontal timeline.

**Interaction:**
- Horizontal scrollable timeline with zoom (weeks / months / all-time)
- Event nodes categorized by type:
  - 🔵 Structural: new modules added, major refactors detected
  - 🟢 Semantic: significant SIR drift events, intent changes
  - 🟡 Memory: project memory entries (decisions, conventions, context)
  - 🔴 Health: graph health events (orphan spikes, connectivity drops)
- Click an event → expand detail card with:
  - Memory text / drift summary / health snapshot
  - Affected symbols or modules
  - Git commit link (if available)
- Filter by event type (checkboxes)
- Search within timeline events

**Data source:** `GET /api/v1/memory-timeline?since={date}&types=all` — aggregates from project memory store, SIR version history, health snapshots, and drift events.

**D3 approach:** Custom timeline layout with `d3.scaleTime()` X-axis. `d3.zoom()` for pan/zoom. Event nodes as SVG circles/icons positioned along the timeline. Detail cards as HTML overlays.

#### 5. Health Scorecard

**What it shows:** At-a-glance project health with 6-8 metric cards, each showing current value, trend sparkline, and status indicator.

**Metrics:**
| Metric | Source | Good | Warning | Critical |
|--------|--------|------|---------|----------|
| Stale SIR % | `sir_meta.sir_status` counts | < 10% | 10-30% | > 30% |
| Orphan symbols | Graph connectivity | < 5 | 5-20 | > 20 |
| Avg drift score | Drift analysis | < 0.15 | 0.15-0.30 | > 0.30 |
| Graph connectivity | Connected components ratio | > 0.9 | 0.7-0.9 | < 0.7 |
| High coupling pairs | Coupling analysis | < 3 | 3-8 | > 8 |
| Coverage (SIR/symbols) | Symbol count vs SIR count | > 90% | 70-90% | < 70% |
| Index freshness | Time since last full index | < 1hr | 1-24hr | > 24hr |
| Avg SIR confidence | SIR metadata | > 0.8 | 0.6-0.8 | < 0.6 |
| Staleness score (10.2) | Continuous monitor noisy-OR | < 0.3 | 0.3-0.6 | > 0.6 |
| Batch queue depth (10.1) | Pending symbols for batch | < 50 | 50-200 | > 200 |
| Fingerprint churn (10.1) | Prompt hash changes / week | < 5% | 5-15% | > 15% |

**Interaction:**
- Each metric is a card with: current value (large), sparkline (last 30 data points), status color (green/yellow/red)
- Click a card → navigate to the relevant detailed view (e.g., click Stale SIR → drift timeline filtered to stale modules)
- Auto-refresh via HTMX `hx-trigger="every 30s"` (matches existing dashboard pattern)

**Data source:** `GET /api/v1/health-scorecard` — aggregates from multiple existing endpoints.

**D3 approach:** Sparklines via `d3.line()` with `d3.area()` fill. Small multiples pattern. Color thresholds via CSS classes driven by server-computed status.

#### 6. Staleness Heatmap (NEW — Phase 10.2 data)

**What it shows:** A grid heatmap showing staleness scores across modules (Y-axis) over time (X-axis). Hot cells indicate areas where SIR meaning is drifting from source code reality — the continuous monitor's noisy-OR scores visualized spatially.

**Interaction:**
- Y-axis: modules or crates (sorted by worst staleness)
- X-axis: time windows (daily buckets)
- Cell color: green (fresh, staleness < 0.3) → yellow (aging, 0.3-0.6) → red (stale, > 0.6)
- Hover a cell → tooltip with module name, staleness score, top contributing symbols, and prompt hash changes
- Click a cell → navigate to continuous monitor status for that module at that time
- Toggle: show all modules vs. only modules above a staleness threshold

**Data source:** `GET /api/v1/staleness-heatmap?since={date}&granularity=daily` — queries `sir_fingerprint_history` (10.1) and staleness scores (10.2).

**D3 approach:** `d3.scaleSequential(d3.interpolateRdYlGn).domain([1, 0])` (reversed — low staleness = green). Grid cells via `d3.selectAll('rect')`. Responsive row/column sizing.

### Dashboard navigation update

Add the new/upgraded pages to the existing HTMX navigation sidebar:

```
Dashboard
├── Overview (existing)
├── Dependency Graph (existing)
├── Architecture Map (existing)
├── Anatomy (existing)
├── X-Ray (existing)
├── Blast Radius (existing → UPGRADE)
├── Causal Explorer (existing)
├── Time Machine (existing)
├── Drift Timeline (NEW)
├── Coupling Map (NEW)
├── Memory Timeline (NEW)
├── Health Scorecard (NEW)
├── Staleness Heatmap (NEW)
├── Search (existing)
└── Settings (Phase 9.2)
```

### New JSON API endpoints

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/api/v1/blast-radius` | GET | Symbol impact tree with depth parameter (existing — enhance response) |
| `/api/v1/drift-timeline` | GET | Time-series drift scores per module |
| `/api/v1/coupling-matrix` | GET | Module coupling matrix with signal breakdown |
| `/api/v1/memory-timeline` | GET | Aggregated project events timeline |
| `/api/v1/health-scorecard` | GET | Composite health metrics with trends |
| `/api/v1/staleness-heatmap` | GET | Module × time staleness grid from 10.2 data |

All endpoints follow the existing JSON envelope pattern: `{ "data": {...}, "meta": { "timestamp": ..., "stale": bool } }`.

### Out of scope

- Real-time WebSocket updates (HTMX polling is sufficient for MVP)
- 3D visualizations (stay in 2D SVG)
- Animated transitions between visualization states (keep it snappy, not fancy)
- Export visualizations as images/PDF (future enhancement)
- Custom dashboard layouts / drag-and-drop widget arrangement

### Implementation Notes

#### D3 module structure

Each visualization is a self-contained JavaScript module. Existing modules live in `crates/aether-dashboard/src/static/js/charts/`:

```
static/js/charts/
├── anatomy.js           # (existing)
├── architecture.js      # (existing)
├── blast-radius.js      # (existing → upgrade with radial tree + staleness colors)
├── causal-explorer.js   # (existing)
├── smart-search.js      # (existing)
├── symbol-search.js     # (existing)
├── time-machine.js      # (existing)
├── xray-cards.js        # (existing)
├── xray-hotspots.js     # (existing)
├── drift-timeline.js    # NEW — multi-line time series with brush
├── coupling-chord.js    # NEW — chord diagram with multi-signal colors
├── memory-timeline.js   # NEW — horizontal event timeline
├── health-sparkline.js  # NEW — reusable sparkline component
├── staleness-heatmap.js # NEW — grid heatmap of staleness by module × time
└── common.js            # Shared: color scales, tooltip, responsive resize
```

Each module exports an `init(containerId, dataUrl)` function. The HTMX fragment for each page includes a `<script>` tag that calls `init()` after the fragment loads.

#### Responsive design

All D3 visualizations must handle window resize:

```javascript
// common.js
export function onResize(svgId, renderFn) {
    const observer = new ResizeObserver(() => {
        const container = document.getElementById(svgId).parentElement;
        renderFn(container.clientWidth, container.clientHeight);
    });
    observer.observe(document.getElementById(svgId).parentElement);
}
```

This matters for the Tauri window where the user can resize freely.

#### Performance guardrails

For large codebases (10K+ symbols):
- Blast radius: cap at depth=3 by default, paginate at 200 nodes
- Coupling chord: aggregate to module level, not file level
- Drift timeline: downsample to daily aggregates beyond 90 days
- Graph API: continue to respect existing `limit` parameter

### Pass criteria

1. All 4 new visualization pages and 1 upgrade render correctly with sample data.
2. Blast radius upgrade: radial tree layout with staleness coloring. Re-centering on a downstream node works.
3. Drift timeline: brush selection zooms the time range. Hover shows tooltip with module name and score.
4. Coupling chord: hovering a module highlights its chords. Threshold slider filters weak couplings.
5. Memory timeline: events display chronologically. Filter checkboxes hide/show event types. Zoom works.
6. Health scorecard: all 11 metrics display with sparklines. Color coding matches thresholds. Staleness and batch metrics from 10.x included.
7. All visualizations handle empty data gracefully (show "No data available" message, not a blank page or JS error).
8. Window resize causes visualizations to redraw at correct size (no overflow, no truncation).
9. Page load time < 2 seconds for a workspace with 5,000 symbols.
10. `cargo fmt --all --check`, `cargo clippy -p aether-dashboard -- -D warnings` pass.
11. `cargo test -p aether-dashboard` passes (API endpoint tests with mock data).

### Estimated Claude Code sessions: 2–3
