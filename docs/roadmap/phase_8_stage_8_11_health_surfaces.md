# Phase 8 — Stage 8.11: Health Surfaces + Split Planner

**Codename:** Pressure Gauge
**Depends on:** Stage 8.10 (Git + Semantic Health Signals) merged
**New crates:** None
**Modified crates:** `aether-dashboard`, `aether-mcp`, `aether-lsp`, `aether-health` (split planner module)

---

## Purpose

Stages 8.9 and 8.10 built the health scoring engine. This stage makes it visible everywhere developers and agents work:

1. **Dashboard** — hotspot leaderboard panel, archetype distribution, trend sparklines
2. **MCP tools** — `aether_health_hotspots` and `aether_health_explain` for agent-driven refactor planning
3. **LSP** — code lens showing health score + archetype on file hover
4. **Split planner** — lightweight file-level split recommendations using intra-file community analysis

This is the UX payoff for the health scoring infrastructure.

---

## Prerequisites

- Stage 8.10 merged — `aether-health` has structural + git + semantic scoring
- Dashboard operational (`aetherd --features dashboard`)
- Workspace indexed for semantic signals

---

## In scope

- Dashboard: hotspot leaderboard API endpoint + HTMX fragment
- Dashboard: archetype distribution summary (count per archetype)
- Dashboard: score trend sparkline (last 10 runs from history table)
- MCP: `aether_health_hotspots` tool — top N crates by score with archetypes
- MCP: `aether_health_explain` tool — full violation list + signals for one crate
- LSP: health score in hover card when file belongs to a scored crate
- Split planner: file-level split suggestions using intra-file symbol clustering from community snapshots
- Before/after comparison: `health-score --compare <commit>` CLI flag

## Out of scope

- Symbol-level split recommendations (requires embedding clustering, future work)
- Auto-refactoring or code modification
- PR integration or CI bot
- Git churn visualization in dashboard (can be added later)
- Per-file dashboard drilldown

---

## 1. Dashboard Integration

### API endpoint

```
GET /api/v1/health-score?limit=10&min_score=25
```

Response shape (JSON, consumed by HTMX fragment):

```json
{
  "workspace_score": 58,
  "severity": "moderate",
  "delta": -4,
  "crates": [
    {
      "name": "aether-store",
      "score": 78,
      "severity": "high",
      "archetypes": ["God File", "Boundary Leaker"],
      "top_violation": "Store trait has 52 methods"
    }
  ],
  "archetype_distribution": {
    "God File": 2,
    "Brittle Hub": 2,
    "Churn Magnet": 1,
    "Legacy Residue": 1,
    "Boundary Leaker": 1
  },
  "trend": [62, 60, 58, 58, 62, 65, 67, 62, 60, 58]
}
```

### HTMX fragment

New file: `crates/aether-dashboard/src/fragments/health_score.rs`

The fragment renders:

- **Workspace score badge** — large number with severity color and delta arrow
- **Hotspot table** — crate name, score bar (colored by severity), archetype pills, top violation
- **Archetype distribution** — small horizontal bar chart showing count per archetype
- **Trend sparkline** — D3 mini line chart from last 10 history entries

Follow existing dashboard patterns: maud template, Tailwind classes, dark theme, IBM Plex Sans. The health score panel is added to the dashboard overview page alongside the existing health panel.

### API handler

New file: `crates/aether-dashboard/src/api/health_score.rs`

The handler:
1. Opens `GitContext` for the workspace
2. Runs `aether_health::compute_workspace_score()` with structural metrics
3. If indexed workspace is available, runs semantic scoring via the bridge pattern from 8.10
4. Reads score history from `.aether/meta.sqlite` for trend data
5. Returns JSON for the fragment

Register route in `api/mod.rs`:
```rust
.route("/api/v1/health-score", get(health_score::health_score_handler))
```

### Dashboard page registration

Add the health-score fragment to the overview page (`fragments/mod.rs` or wherever the overview composition lives) as a new panel section. It should appear after the existing health panel.

---

## 2. MCP Tools

### `aether_health_hotspots`

Returns the top N crates by health score, with archetypes and top violation.

**Input schema:**
```json
{
  "limit": 5,
  "min_score": 25,
  "semantic": true
}
```

**Output:** Same shape as the dashboard API but formatted as MCP tool response text.

**Implementation:** Call `aether_health::compute_workspace_score()` in the MCP handler, filter and format.

### `aether_health_explain`

Returns the full health breakdown for a single crate: all metrics, all signals, all violations, archetype explanation.

**Input schema:**
```json
{
  "crate_name": "aether-store",
  "semantic": true
}
```

**Output:** Formatted text with metric values, signal values, violations with reasons, and archetype labels. Designed to give an agent enough context to plan a refactor.

**Example output:**
```
Health Score: aether-store — 78/100 (High)
Archetypes: God File, Boundary Leaker

Structural signals:
  max_file_loc: 6,493 (fail threshold: 1,500) — lib.rs is 6,493 lines
  trait_method_max: 52 (fail threshold: 35) — Store trait has 52 methods
  internal_dep_count: 5 (below threshold)
  dead_feature_flags: 4 (warn threshold: 1)

Git signals:
  churn_30d: 0.47 — 7 commits in last 30 days
  author_count: 0.33 — 2 distinct authors

Semantic signals:
  boundary_leakage: 0.72 — symbols span 4 communities
  drift_density: 0.18 — 18% of symbols show active drift
  test_gap: 0.35 — 35% of high-centrality symbols lack test intents

Recommended first action:
  Split Store trait into domain-specific sub-traits (symbol ops, SIR ops, notes ops, embedding ops)
```

### MCP registration

Add both tools to the tool router in `aether-mcp/src/lib.rs` following the existing pattern (tool attribute definition, logic method, handler wiring).

**Dependency note:** `aether-mcp` already depends on `aether-analysis` and `aether-store`. It will additionally need `aether-health` as a dependency. Since `aether-health` only depends on `aether-config` and `aether-core`, this adds no circular dependency risk.

---

## 3. LSP Integration

### Health score in hover card

When the LSP resolves a hover for a symbol, and the symbol's file belongs to a crate that has been scored, append a health line to the hover markdown:

```markdown
---
**Health:** aether-store — 78/100 (High) · God File
```

This is a single line appended to the existing SIR hover content.

### Implementation

In `aether-lsp/src/lib.rs`, after resolving the SIR hover content:

1. Determine which crate the file belongs to (from file path relative to workspace root)
2. Check if a cached `ScoreReport` exists (see caching below)
3. If yes, find the crate's score and format the one-line summary
4. Append to hover markdown

### Score caching

Health score computation takes < 1 second for structural-only, but we don't want to recompute on every hover. Cache the `ScoreReport` in the LSP backend state with a 5-minute TTL. Recompute on first hover after TTL expires.

Semantic scoring is skipped in LSP hover — structural-only is sufficient for the one-line summary, and semantic scoring requires async Store queries that don't fit the hover latency budget.

**LSP dependency:** `aether-lsp` will need `aether-health` as a dependency. Same as MCP — no circular risk.

---

## 4. Split Planner

### What it does

Given a file path, the split planner suggests how to break the file into smaller files based on:

1. **Intra-file community clustering** — symbols in the file that belong to different graph communities (from `list_latest_community_snapshot()`) are natural split candidates
2. **Trait method grouping** — for large traits, group methods by the types they operate on (symbol ops, SIR ops, note ops, etc.)
3. **Public vs private surface** — suggest extracting private helper functions that cluster together

### Scope

This is a **lightweight heuristic planner**, not an AI-powered refactoring engine. It uses data AETHER already has. No LLM calls.

### API

New module in `aether-health/src/planner.rs`:

```rust
pub struct SplitSuggestion {
    pub target_file: String,
    pub suggested_modules: Vec<SuggestedModule>,
    pub expected_score_impact: String,  // "Likely reduces score by ~15-25 points"
    pub confidence: SplitConfidence,
}

pub struct SuggestedModule {
    pub name: String,              // e.g., "sir_ops" or "note_queries"
    pub symbols: Vec<String>,      // symbol IDs to extract
    pub reason: String,            // "These 12 symbols all belong to community 3 (SIR management)"
}

pub enum SplitConfidence {
    High,      // clear community separation
    Medium,    // some overlap but reasonable cut
    Low,       // heuristic guess, review carefully
}

pub fn suggest_split(
    file_path: &str,
    community_assignments: &[CommunitySummary],
    symbol_records: &[SymbolSummary],
) -> Option<SplitSuggestion> {
    // Only suggest for files with score >= 50
    // Only suggest when symbols span >= 2 communities
    // Group symbols by community, name each group by dominant concept
}
```

### CLI

Add `--suggest-splits` flag to `health-score`:

```
aetherd health-score --workspace . --semantic --suggest-splits
```

Appends split suggestions after the score table for any crate scoring ≥ 50:

```
Split suggestions:

  aether-store/src/lib.rs (score: 78, God File)
    Confidence: High — symbols span 4 communities
    1. Extract SIR operations → store/sir_ops.rs (14 methods)
    2. Extract note operations → store/note_ops.rs (8 methods)
    3. Extract embedding operations → store/embedding_ops.rs (6 methods)
    Expected impact: reduces crate score by ~20-30 points

  aether-mcp/src/lib.rs (score: 68, Brittle Hub)
    Confidence: Medium — symbols span 3 communities
    1. Extract search tools → mcp/search_tools.rs (4 tool handlers)
    2. Extract memory tools → mcp/memory_tools.rs (3 tool handlers)
    Expected impact: reduces crate score by ~10-15 points
```

### MCP

The `aether_health_explain` tool includes split suggestions when the crate scores ≥ 50 and community data is available.

---

## 5. Before/After Comparison

### CLI

Add `--compare <commit-or-run-id>` flag:

```
aetherd health-score --workspace . --compare a1b2c3d
```

Loads the historical score for the specified git commit (or the most recent run if `--compare last`), computes the current score, and displays a diff:

```
AETHER Health Score — Before/After Comparison
Before: commit a1b2c3d (2026-03-10) — Score: 62/100
After:  commit f4e5d6c (2026-03-15) — Score: 54/100
Delta:  -8 points (improvement)

Crate              Before  After  Delta
─────────────────────────────────────────
aether-store         78     72     -6  ↓
aether-mcp           71     65     -6  ↓
aetherd              54     50     -4  ↓
aether-config        44     44      0  —
aether-analysis      38     34     -4  ↓

Improvements:
  aether-store: trait_method_max 52 → 38 (Store trait split)
  aether-mcp: stale_backend_refs 14 → 6 (cozo cleanup)

Regressions:
  (none)
```

This is the refactor validation story — run `health-score` before a refactor, do the work, run again, see the delta.

---

## Implementation Notes

### Dashboard state

The dashboard handler needs access to `aether-health` scoring. Since `aether-dashboard` cannot depend on `aether-mcp` (existing circular dependency constraint), and the health scoring lives in `aether-health` (which has no such constraint), the dashboard depends on `aether-health` directly.

Add to `aether-dashboard/Cargo.toml`:
```toml
aether-health = { path = "../aether-health" }
```

### MCP dependency

Add to `aether-mcp/Cargo.toml`:
```toml
aether-health = { path = "../aether-health" }
```

### LSP dependency

Add to `aether-lsp/Cargo.toml`:
```toml
aether-health = { path = "../aether-health" }
```

### Score caching in dashboard and LSP

Both the dashboard and LSP benefit from caching the `ScoreReport`. Use a simple `Arc<RwLock<Option<(Instant, ScoreReport)>>>` with a 5-minute TTL. Recompute on first access after expiry.

The MCP tools recompute on every call — agent interactions are infrequent enough that caching adds complexity without benefit.

---

## Tests

### Unit tests

| Test | Description |
|------|-------------|
| `split_planner_no_suggestion_for_healthy_file` | File with score < 50 → None |
| `split_planner_groups_by_community` | Known community assignments → correct module groupings |
| `split_planner_names_modules_from_symbols` | Symbols named `sir_*` grouped → module named `sir_ops` |
| `compare_report_computes_delta` | Two score reports → correct per-crate delta |
| `compare_identifies_improvements` | Lower metric values → listed as improvements |

### Integration tests

| Test | Description |
|------|-------------|
| `dashboard_health_score_endpoint` | GET `/api/v1/health-score` returns valid JSON with expected fields |
| `mcp_health_hotspots_tool` | Call tool with limit=5 → returns ≤ 5 crates sorted by score |
| `mcp_health_explain_tool` | Call tool with crate_name → returns violations and signals |
| `compare_with_last` | `--compare last` shows delta vs most recent history entry |
| `lsp_hover_includes_health` | Hover on a symbol in aether-store → markdown contains health line |

---

## Pass Criteria

1. Dashboard at `http://127.0.0.1:9730/dashboard/` shows health score panel with hotspot table
2. Archetype distribution renders as colored pills or bar segments
3. Trend sparkline shows data from score history (or "no history" placeholder if first run)
4. `aether_health_hotspots` MCP tool returns correct data when called via MCP client
5. `aether_health_explain` MCP tool returns full breakdown with reasons for a specific crate
6. LSP hover on a file in `aether-store` includes the health score line
7. `--suggest-splits` shows split recommendations for high-scoring crates when community data exists
8. `--compare last` shows a valid before/after comparison
9. No regression in existing dashboard, MCP, or LSP functionality
10. `cargo fmt --check` and `cargo clippy -- -D warnings` pass
11. All tests pass

---

## Codex Prompt

```
CRITICAL BUILD SETTINGS — use these for ALL cargo commands:
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR

You are working in the repo root of https://github.com/rephug/aether.

Read these files before writing any code:
- docs/roadmap/phase_8_stage_8_11_health_surfaces.md      (this spec)
- crates/aether-health/src/lib.rs                          (scoring API from 8.9/8.10)
- crates/aether-dashboard/src/api/mod.rs                   (route registration pattern)
- crates/aether-dashboard/src/api/health.rs                (existing health API handler)
- crates/aether-dashboard/src/fragments/mod.rs             (fragment registration)
- crates/aether-mcp/src/lib.rs                             (MCP tool registration pattern)
- crates/aether-lsp/src/lib.rs                             (hover resolution)
- crates/aether-store/src/lib.rs                           (community snapshot, test intent queries)
- crates/aetherd/src/cli.rs                                (HealthScoreArgs)

PREFLIGHT

1) Verify working tree is clean. If dirty, STOP.
2) Create branch: git checkout -b feature/phase8-stage8-11-health-surfaces
3) Create worktree: git worktree add ../aether-phase8-health-surfaces feature/phase8-stage8-11-health-surfaces
4) cd into the worktree.

IMPLEMENTATION

5) Add aether-health dependency to:
   - crates/aether-dashboard/Cargo.toml
   - crates/aether-mcp/Cargo.toml
   - crates/aether-lsp/Cargo.toml

6) Dashboard — API handler:
   - Create crates/aether-dashboard/src/api/health_score.rs
   - health_score_handler: compute workspace score, read history for trend, return JSON
   - Register route: /api/v1/health-score in api/mod.rs

7) Dashboard — HTMX fragment:
   - Create crates/aether-dashboard/src/fragments/health_score.rs
   - Render: workspace score badge, hotspot table, archetype pills, trend sparkline
   - Follow existing maud + Tailwind dark theme patterns
   - Register in fragments/mod.rs

8) MCP tools:
   a) Add aether_health_hotspots tool:
      - Input: limit, min_score, semantic (bool)
      - Output: formatted text with crate scores, archetypes, top violations
      - Register in tool router
   b) Add aether_health_explain tool:
      - Input: crate_name, semantic (bool)
      - Output: full metric/signal/violation breakdown with reasons
      - Include split suggestions if score >= 50 and community data available
      - Register in tool router

9) LSP hover extension:
   - In resolve_hover_markdown_for_source(), after SIR content:
   - Check cached ScoreReport (Arc<RwLock<Option<(Instant, ScoreReport)>>>)
   - If crate found, append: "---\n**Health:** {crate} — {score}/100 ({severity}) · {archetype}"
   - Cache TTL: 5 minutes
   - Structural-only scoring (no semantic in hover path)

10) Split planner:
    - Create crates/aether-health/src/planner.rs
    - suggest_split(): groups symbols by community, names modules, estimates impact
    - Only for files with score >= 50 and symbols spanning >= 2 communities
    - SplitSuggestion, SuggestedModule, SplitConfidence types

11) CLI additions to aetherd:
    a) Add --suggest-splits bool flag to HealthScoreArgs
    b) Add --compare <String> flag to HealthScoreArgs (commit hash or "last")
    c) Implement compare logic: load historical score, compute current, display delta
    d) Implement split display: after score table, show suggestions for qualifying crates

12) Tests per spec.

SCOPE GUARD — do NOT modify:
- Health scoring formulas from 8.9/8.10 (additive features only)
- Existing dashboard panels or API endpoints
- Existing MCP tools
- Existing LSP hover content (append only)
- Store trait or implementations

VALIDATION

13) Run:
    cargo fmt --check
    cargo clippy -p aether-health -p aether-dashboard -p aether-mcp -p aether-lsp -p aetherd -- -D warnings
    cargo test -p aether-health
    cargo test -p aether-dashboard
    cargo test -p aether-mcp
    cargo test -p aether-lsp
    cargo test -p aetherd

14) Start dashboard and verify health-score panel renders:
    cargo run -p aetherd --features dashboard -- --workspace . &
    curl http://127.0.0.1:9730/api/v1/health-score | python3 -m json.tool

15) Test CLI features:
    cargo run -p aetherd --bin aetherd -- health-score --workspace . --suggest-splits
    cargo run -p aetherd --bin aetherd -- health-score --workspace . --compare last

COMMIT

16) Commit: "feat(phase8): add health surfaces — dashboard panel, MCP tools, LSP hover, split planner"
```

---

## End-of-Stage Git Sequence

```bash
git push origin feature/phase8-stage8-11-health-surfaces
gh pr create \
  --title "Phase 8.11 — Health Surfaces + Split Planner" \
  --body "Exposes health scoring via dashboard (hotspot leaderboard + trend sparklines), MCP tools (aether_health_hotspots + aether_health_explain), LSP hover (score + archetype), and split planner. Adds --compare and --suggest-splits CLI flags."

# After merge:
git switch main
git pull --ff-only
git worktree remove ../aether-phase8-health-surfaces
git branch -D feature/phase8-stage8-11-health-surfaces
```
