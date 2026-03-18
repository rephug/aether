# Phase 10 — The Conductor

## Stage 10.4 — The Seismograph (Semantic Change Monitoring)

### Purpose

Turn the fingerprint history from 10.1-10.2 into a live monitoring surface that answers: "How is meaning flowing through this codebase, and where is it unstable?" The Seismograph aggregates per-symbol regeneration events into codebase-wide metrics, traces semantic cascades to their epicenters, and predicts which symbols will shift meaning next.

### Prerequisites

- Stage 10.1 merged (fingerprint history table, prompt hashing)
- Stage 10.2 merged (staleness scoring, Δ_sem computation, drift monitor)
- **Data requirement:** At least 2-4 weeks of fingerprint history from nightly batch runs. The Seismograph produces empty/meaningless output without historical data.

### What Problem This Solves

The drift monitor (10.2) answers "which symbols are stale right now?" The Seismograph answers bigger questions:

- "Is the codebase stabilizing or destabilizing?" (semantic velocity)
- "Which module is an active fault line?" (community stability)
- "Yesterday's PR changed 5 files — which other symbols actually shifted meaning as a result?" (epicenter tracing)
- "Symbol B just drifted significantly — what's likely to drift next?" (aftershock prediction)

### In scope

#### Semantic velocity (Deep Think finding A1)

A single codebase-wide metric: how fast is meaning changing? Uses PageRank-weighted Exponential Moving Average with a noise floor to ignore LLM phrasing jitter.

**Per-batch codebase shift:**

```
S_t = Σ(PR_i × max(0, Δ_sem_i - τ)) / Σ(PR_i)
    for all symbols i regenerated in batch t
    where τ = 0.15 (noise floor, configurable)
```

**Semantic velocity (EMA):**

```
V_t = α × S_t + (1 - α) × V_{t-1}
    where α = 0.2 (smoothing factor, configurable)
```

**Model upgrade spike mitigation:** Filter out any fingerprint row where `config_changed == true AND source_changed == false` from the S_t calculation. The composite prompt hash decomposition from 10.1 makes this a simple WHERE clause.

Stored in a new `metrics_seismograph` SQLite table:
```sql
CREATE TABLE IF NOT EXISTS metrics_seismograph (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    batch_timestamp INTEGER NOT NULL,
    codebase_shift REAL NOT NULL,
    semantic_velocity REAL NOT NULL,
    symbols_regenerated INTEGER NOT NULL,
    symbols_above_noise INTEGER NOT NULL
);
```

#### Community stability scoring (Deep Think finding A3)

Per-community metric: what fraction of the community's importance is shifting?

```
Stability_C = 1.0 - Σ(PR_i × 𝟙(Δ_sem_i > τ)) / Σ(PR_i)
    for all symbols i in community C over a 30-day window
```

Score of 1.0 = stable bedrock. Score near 0.0 = active fault line. PageRank weighting means a shifting leaf node barely moves the score, but a shifting hub drops it hard.

Stored in `metrics_community_stability`:
```sql
CREATE TABLE IF NOT EXISTS metrics_community_stability (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    community_id TEXT NOT NULL,
    computed_at INTEGER NOT NULL,
    stability REAL NOT NULL,
    symbol_count INTEGER NOT NULL,
    breach_count INTEGER NOT NULL
);
```

#### Epicenter tracing (Deep Think finding A4)

When a cascade of SIR changes propagates through the graph, trace it backward to the root cause.

**Algorithm — Time-respecting reverse BFS via prompt hash decomposition:**

1. Start at affected symbol S_n at batch timestamp t_n
2. Check `sir_fingerprint_history.trigger` for S_n. If `source_changed == true` → this is an epicenter. Stop.
3. If `neighbor_changed == true` → query `symbol_edges` (SQLite) for its DEPENDS_ON/CALLS neighbors
4. Among those neighbors, find the one with highest `coupling × Δ_sem` that registered a Δ_sem > τ at timestamp t_{n-1} where `(t_n - batch_window) ≤ t_{n-1} ≤ t_n`
5. Recurse from step 2 with S_{n-1}. Strict temporal monotonicity (t_{n-1} ≤ t_n) prevents infinite loops in cyclic graphs.

**Output:** A cascade chain: `[epicenter] → sym_A (Δ=0.7) → sym_B (Δ=0.4) → sym_C (Δ=0.2)` with timestamps, coupling strengths, and change sources at each hop.

Store cascade records in `metrics_cascade`:
```sql
CREATE TABLE IF NOT EXISTS metrics_cascade (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    epicenter_symbol_id TEXT NOT NULL,
    chain_json TEXT NOT NULL,
    total_hops INTEGER NOT NULL,
    max_delta_sem REAL NOT NULL,
    detected_at INTEGER NOT NULL
);
```

#### Aftershock prediction (Deep Think finding A2)

Given a high-Δ_sem event on symbol B, predict the probability (not magnitude) that downstream symbol A will breach the noise floor on its next regeneration.

**Calibrated logistic model:**

```
P(Δ_sem_A > τ) = σ(w₀ + w₁·Δ_sem_B + w₂·C_AB + w₃·γ + w₄·PR_A)
```

Where C_AB is coupling score, γ is graph distance, and σ is the logistic sigmoid. Weights w_n trained via lightweight logistic regression (Rust crate `linfa` or `smartcore`) over `sir_fingerprint_history` joined with coupling data.

**Calibration:** Run a periodic background fit (e.g., weekly or on `aetherd continuous run-once`) using the last 30 days of fingerprint data as training signal. Store weights in a `metrics_aftershock_model` table.

**Prediction output:** For each high-Δ_sem event, emit a list of at-risk symbols with probability scores. Surface in dashboard and structured logs.

#### Dashboard pages

Three new pages added to `aether-dashboard`:

1. **Seismograph Timeline** — Line chart of semantic velocity over time (like the USGS earthquake timeline). Highlight individual high-Δ_sem events as markers. Click a marker to see the cascade chain.
2. **Tectonic Plates** — Louvain communities colored by stability score. Green = bedrock, amber = shifting, red = fault line. Click a community to drill into its symbol-level Δ_sem distribution.
3. **Velocity Gauge** — Single-number display with trending arrow (↑ accelerating, → stable, ↓ decelerating). The thing a tech lead looks at every morning.

#### CLI commands

- `aetherd seismograph status` — Print current semantic velocity, top 5 most unstable communities, and any active cascades
- `aetherd seismograph trace <symbol_id>` — Run epicenter trace for a specific symbol's most recent shift

### New config: `[seismograph]`

```toml
[seismograph]
enabled = false
noise_floor = 0.15
ema_alpha = 0.2
community_window_days = 30
cascade_max_depth = 10
aftershock_retrain_interval = "weekly"
```

### Out of scope

- Real-time streaming (Seismograph runs after each batch, not per-keystroke)
- Automated remediation (alerts only, no auto-fix)
- Cross-repository seismograph (single workspace only)

### Pass criteria

1. `aetherd seismograph status` prints semantic velocity, top unstable communities, and active cascades.
2. Semantic velocity uses PageRank-weighted EMA. Model upgrade batches (config_changed=true, source_changed=false) are filtered out.
3. Community stability normalizes by PageRank weight — a single shifted leaf doesn't tank a large community's score.
4. Epicenter tracing follows strict temporal monotonicity — no infinite loops on cyclic graphs.
5. Aftershock prediction trains on fingerprint history and produces probability scores.
6. `metrics_seismograph`, `metrics_community_stability`, `metrics_cascade` tables populated after batch runs.
7. Dashboard pages render with D3 visualizations (or placeholders if data insufficient).
8. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings` pass.
9. `cargo test -p aetherd` passes.

### Estimated Codex runs: 2-3
