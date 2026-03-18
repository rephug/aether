# Phase 10.4 — The Seismograph — Session Context

**Date:** 2026-03
**Branch:** `feature/phase10-stage10-4-seismograph` (to be created)
**Worktree:** `/home/rephu/aether-phase10-seismograph` (to be created)
**Starting commit:** HEAD of main after 10.2 merged
**Prerequisites:** Stages 10.1 + 10.2 must be merged. Needs `sir_fingerprint_history` table with accumulated data, staleness scoring, PageRank computation, and community detection.

## CRITICAL: Read actual source, not this document

```bash
/home/rephu/projects/aether
# Always grep/read actual source before making claims
```

## Build environment (MUST be set for ALL cargo commands)

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

**Never run `cargo test --workspace`** — OOM risk. Always per-crate.

## What 10.1-10.2 added (verify against actual repo)

- `sir_fingerprint_history` SQLite table with `source_changed`, `neighbor_changed`, `config_changed`, `delta_sem`
- `sir.prompt_hash` column (composite: `source_hash|neighbor_hash|config_hash`)
- `sir.staleness_score` column
- `[batch]`, `[watcher]`, `[continuous]` config sections
- Staleness scoring via Noisy-OR formula
- Background drift monitor with in-memory petgraph
- Batch build/ingest/run pipeline with prompt hashing

## What this stage adds

### A. Semantic velocity
- PageRank-weighted EMA of Δ_sem with noise floor τ=0.15
- Model upgrade spike filtering via `config_changed` flag
- New `metrics_seismograph` SQLite table

### B. Community stability
- PageRank-weighted threshold breach frequency per Louvain community
- 30-day rolling window
- New `metrics_community_stability` SQLite table

### C. Epicenter tracing
- Time-respecting reverse BFS via prompt hash decomposition
- Strict temporal monotonicity prevents infinite loops
- New `metrics_cascade` SQLite table

### D. Aftershock prediction
- Calibrated logistic model: P(Δ_sem > τ) = σ(w₀ + w₁·Δ_sem_B + w₂·C_AB + w₃·γ + w₄·PR_A)
- Trained on fingerprint history via `linfa` or `smartcore`
- Retrained weekly

### E. Dashboard pages
- Seismograph Timeline, Tectonic Plates, Velocity Gauge
- Added to existing `aether-dashboard` crate

---

## Key files to understand

**Fingerprint history (data source):**
- `sir_fingerprint_history` SQLite table — the primary data source for all Seismograph metrics
- Query patterns: group by batch timestamp, filter by date range, join with symbols table for PageRank

**PageRank:**
- Find where PageRank is computed — likely `aether-analysis` or the health pipeline
- Need to load PageRank per symbol for weighting

**Community detection:**
- Louvain community assignments from the planner pipeline
- `community_snapshot` SQLite table or the file-scoped planner output
- Map symbols to communities for stability scoring

**Coupling data:**
- SurrealDB `co_change` edges for aftershock prediction
- For nightly cron: SurrealDB accessible (no daemon lock)

**Edges (for epicenter tracing):**
- SQLite `symbol_edges` table — use for reverse BFS to avoid SurrealKV lock
- `store.get_callers()` and `store.get_dependencies()`

**Dashboard (for new pages):**
- `crates/aether-dashboard/src/api/` — add new API endpoints
- `crates/aether-dashboard/src/fragments/` — add new page fragments
- `crates/aether-dashboard/src/static/js/charts/` — add new D3 visualizations
- `crates/aether-dashboard/src/static/index.html` — add sidebar navigation links

**Logistic regression (for aftershock model):**
- Add `linfa` or `smartcore` to workspace dependencies
- Keep it lightweight — logistic regression only, not a full ML framework

---

## Scope guard — do NOT modify

- Fingerprint history write path (10.1/10.2 — read only)
- Existing dashboard pages
- Existing CLI subcommands
- Staleness scoring formulas (10.2)

---

## End-of-stage sequence

```bash
cd /home/rephu/aether-phase10-seismograph
git push -u origin feature/phase10-stage10-4-seismograph

# After PR merges:
cd /home/rephu/projects/aether
git switch main
git pull --ff-only
git worktree remove ../aether-phase10-seismograph
git branch -d feature/phase10-stage10-4-seismograph
```
