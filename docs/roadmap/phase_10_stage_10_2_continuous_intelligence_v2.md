# Phase 10 — The Conductor

## Stage 10.2 — Continuous Intelligence

### Purpose

Turn AETHER from a system that generates SIRs once and serves them into a system that continuously monitors whether those SIRs are still accurate — and automatically corrects them when they're not. Phase 10.1 gave AETHER cheap batch inference with prompt hashing. Phase 10.2 uses that capability as a continuous background signal with mathematically rigorous staleness scoring and semantic-aware drift propagation.

### What Problem This Solves

SIRs go stale in three ways:

1. **Direct edit staleness:** Someone edits a function but the SIR still describes the old behavior. The watcher catches this on file save (Stage 10.1), but only if the edit triggers a re-parse. Renames, moved code, and refactors across multiple files can leave SIRs inconsistent.

2. **Indirect semantic drift:** A function's *callers* or *dependencies* change meaning, which changes what the function effectively does — even though its source code hasn't changed. No file-save event fires. This is the hardest to catch and the most dangerous for AI context.

3. **Temporal decay:** SIRs generated months ago with an older model may be lower quality than what the current model would produce. Periodic re-generation raises the floor.

---

### Staleness Scoring (Deep Think finding A1)

A linear weighted sum of signals suffers from dilution — if source code changed, the SIR is stale regardless of how recent the generation was. The staleness formula uses a **bounded max over Noisy-OR** combiner with hard gates:

```
S_total = max(S_source, S_model, 1 - (1 - S_time)(1 - S_neighbor))
```

Where:
- `S_source ∈ {0, 1}` — hard gate. 1 if source code changed since SIR generation, 0 otherwise.
- `S_model ∈ {0, 1}` — hard gate. 1 if the model that generated this SIR is now deprecated (lookup table from benchmark data), 0 otherwise.
- `S_time` — logistic sigmoid: `1 / (1 + e^(-k(t - t_half)))` where `t` = days since generation, `t_half` = 15 days (configurable), `k` = 0.3. Provides a grace period then rots quickly.
- `S_neighbor` — semantic-gated neighbor drift (see below).

This ensures any single critical signal (source changed, model deprecated) immediately marks the SIR as stale without being dampened by other signals.

#### Cold-start volatility prior (Deep Think finding A3)

On day 1, all SIRs are fresh, so `S_time ≈ 0` for everything — useless for queue prioritization. Inject a volatility prior from existing `git_churn_30d` data:

```
t_effective = t × (1 + log₂(1 + git_churn_30d))
```

High-churn files age faster in the queue during the first few weeks. This data is already computed by `aether-health` — no new calculation needed.

---

### Neighbor Drift Propagation (Deep Think findings A2 + C2)

If symbol A depends on B, and B's SIR changed, A's staleness should increase — but proportional to how much B's meaning actually shifted, not just that it changed at all.

**Semantic gate:** When a SIR is regenerated, compute the cosine distance between old and new embeddings:

```
Δ_sem(B) = 1 - cos(embedding_old(B), embedding_new(B))
```

This is a single dot product on stored vectors — no inference, no API call.

**Discounted reverse-BFS propagation:**

```
S_indirect(A) = max(S_A, S_B × γ × Δ_sem(B))
```

Where `γ = 0.5` (configurable decay factor). Propagation terminates when `S_indirect < 0.1`, which mathematically bounds cascade to ~4 hops (0.5⁴ = 0.0625).

This eliminates false positives: if B's SIR changed from "validates email" to "validates email and trims whitespace" (Δ_sem ≈ 0.05), A receives nearly zero drift. If B's SIR changed fundamentally (Δ_sem ≈ 0.8), A gets properly flagged.

**Implementation:** Load `symbol_edges` into `petgraph::DiGraph` in-memory once per drift monitor run. Memory: ~5MB for 50K nodes. Traversal: <15ms. Already have `petgraph` via `aether-graph-algo`.

**Previous embedding storage:** To compute Δ_sem, store `previous_embedding_hash` (or the raw vector) alongside the current embedding. On SIR regeneration, copy current → previous before writing the new one.

---

### Predictive Staleness via Coupling (Deep Think finding G2)

If file A is edited and `coupling(A, B) > threshold`, bump B's staleness immediately — before waiting for any structural change in B. The intuition: high co-change probability means B *should* have been edited alongside A. If it wasn't, its SIR may be inconsistent with A's new reality.

```
If edit(A) and coupling(A, B) > 0.85:
    S_B = max(S_B, coupling(A, B) × 0.5)
```

This is a soft bump (not a hard gate) — it moves B up the re-queue priority without forcing immediate re-generation. The coupling matrix is already computed from Phase 6.2 temporal data.

---

### Re-queue Priority (Deep Think finding B1)

Raw `Staleness × PageRank` lets hyper-central nodes dominate even when barely stale. Use log-dampened PageRank as a tiebreaker:

```
Priority = S_total + α × log₁₀(1 + PR) / log₁₀(1 + PR_max)
```

With `α = 0.2`: a fully stale leaf (S=1.0, PR=0) scores 1.0. A barely stale hub (S=0.1, PR=max) scores 0.3. Staleness dominates; PageRank breaks ties among equally stale symbols.

---

### In scope

#### Background drift monitor

A tokio task that runs on a configurable schedule (default: nightly). On each run:

1. Load `symbol_edges` into in-memory `petgraph::DiGraph`
2. Compute `S_source` for all symbols (compare file mtime or content hash against `sir_generated_at`)
3. Compute `S_time` with volatility prior for all symbols
4. Compute `S_model` from model deprecation lookup table
5. Run discounted reverse-BFS with semantic gate for `S_neighbor`
6. Apply predictive staleness from coupling matrix
7. Combine via Noisy-OR formula → `S_total` per symbol
8. Compute priority = `S_total + α × log-dampened PageRank`
9. Select top N candidates by priority
10. Queue for re-generation via batch pipeline (10.1)

#### Staleness score storage

New columns in `sir` SQLite table (added via `ensure_sir_column` helper in schema migration — do NOT use raw ALTER TABLE):
- `staleness_score REAL` — the computed S_total (0.0-1.0)
- `staleness_computed_at INTEGER` — Unix timestamp of last computation
- `previous_embedding_hash TEXT` — for Δ_sem computation on next regeneration

Also update `SirMetaRecord` in `sir_meta.rs` to include `pub staleness_score: Option<f64>`.

Staleness score is recomputed on each drift monitor run and available to dashboard and MCP queries.

#### Automatic re-queue

When the drift monitor identifies stale symbols, it writes a JSONL file to `.aether/batch/auto_requeue_{timestamp}.jsonl` using the 10.1 `batch build` machinery (including prompt hashing — symbols whose prompt context hasn't actually changed are skipped even in the auto-requeue). Configurable behavior:

```toml
[continuous]
# Enable background drift monitoring
enabled = false

# Schedule: "nightly", "hourly", or cron-style "0 2 * * *"
schedule = "nightly"

# Staleness sigmoid half-life in days
staleness_half_life_days = 15

# Sigmoid steepness
staleness_sigmoid_k = 0.3

# Neighbor propagation decay factor (0.0 = no propagation, 1.0 = full propagation)
neighbor_decay = 0.5

# Neighbor propagation cutoff (stop BFS when induced staleness < this)
neighbor_cutoff = 0.1

# Coupling-based predictive staleness threshold
coupling_predict_threshold = 0.85

# PageRank tiebreaker weight (α)
priority_pagerank_alpha = 0.2

# Max symbols to re-queue per run
max_requeue_per_run = 500

# Auto-submit batch job (true) or just write JSONL for manual submission (false)
auto_submit = false

# Pass for re-generation
requeue_pass = "triage"
```

#### Post-build trigger (implements 10.1 stub)

When `trigger_on_build_success = true` in `[watcher]`:

1. Watch for cargo build completion (detect by monitoring `target/` directory or a configurable build artifact path)
2. On successful build: identify which source files were compiled
3. Re-queue those symbols for SIR refresh — not a full re-index, just the symbols in files that were part of the build

#### Nightly re-generation pipeline

A convenience command: `aetherd continuous run-once` that:

1. Runs the full drift monitor scoring pipeline
2. Selects top N stale symbols by priority
3. Builds a triage-pass JSONL with enriched neighbor context (via 10.1 batch build)
4. Optionally submits to Gemini Batch API
5. Polls for completion (if auto-submit)
6. Ingests results (if auto-submit)

Designed for cron on the netcup servers:
```bash
0 2 * * * rephu cd /home/rephu/projects/myrepo && aetherd continuous run-once --workspace . 2>&1 | tee .aether/logs/nightly.log
```

#### Coupling change alerts

When the drift monitor detects that a symbol's coupling graph changed significantly (new high-coupling edge appeared, or existing edge strengthened beyond threshold), emit a structured log event. Phase 9's Tauri app can surface these as native OS notifications.

#### Fingerprint history consumption

The drift monitor writes fingerprint history rows (from 10.1's `sir_fingerprint_history` table) during re-generation and also reads from it to improve scoring:

**Volatility detection:** Query the fingerprint history for each symbol's Δ_sem values over the last 30 days. Symbols with ≥3 regeneration events where Δ_sem > 0.2 are in volatile zones — their neighborhood keeps shifting meaning. Bump their staleness by a volatility factor:

```
volatility_factor = min(1.0, count(Δ_sem > 0.2 in last 30d) / 5)
S_time_adjusted = S_time × (1 + volatility_factor)
```

This ensures symbols in unstable parts of the codebase get re-checked more frequently.

**Neighbor-induced vs self-induced tracking:** The fingerprint history decomposes each change into `source_changed` and `neighbor_changed` flags. The drift monitor can report: "aether-store::Store::upsert_sir has shifted meaning 4 times this month, but 3 of those were neighbor-induced — the function itself is stable, its dependencies are not." This distinction matters for architectural decisions — it tells you whether to fix the symbol or fix its neighborhood.

### Out of scope

- Dashboard pages for continuous intelligence status (Phase 9 or separate dashboard stage)
- Custom drift detection models (uses existing SIR comparison + embedding cosine)
- Cross-workspace drift monitoring
- Real-time streaming drift (this is batch/scheduled, not per-keystroke)
- Risk integral adaptive trigger (noted as future enhancement for team-tier)

### Pass criteria

1. `aetherd continuous run-once` completes on a workspace with stale SIRs, re-queues the most stale symbols.
2. Staleness scores use the Noisy-OR formula — symbols with changed source code score 1.0 regardless of other signals.
3. Logistic time decay: SIRs at 7 days score < 0.3; SIRs at 30 days score > 0.8 (with default parameters).
4. Symbols with changed neighbors score higher staleness than symbols with unchanged neighbors, proportional to Δ_sem.
5. Neighbor propagation terminates within 4 hops (verify no full-graph cascade).
6. Predictive staleness: editing file A bumps coupled file B's staleness when coupling > 0.85.
7. Priority ordering: fully stale leaf outranks barely stale hub (verify with test data).
8. `max_requeue_per_run` is respected.
9. Prompt hashing from 10.1 applies to auto-requeue — unchanged symbols are skipped.
10. With `auto_submit = true`, batch job is submitted and ingested without manual intervention.
11. With `auto_submit = false`, only the JSONL file is written. No API calls.
12. `trigger_on_build_success = true` detects cargo build completion and re-queues affected symbols.
13. Fingerprint history rows are written for each drift-monitor re-generation with `trigger = "drift_monitor"`.
14. Volatility detection: symbols with ≥3 high-Δ_sem events in 30 days receive elevated staleness scores compared to equally-aged symbols with stable histories.
15. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings` pass.
16. `cargo test -p aether-config` and `cargo test -p aetherd` pass.

### Estimated Codex runs: 2–3
