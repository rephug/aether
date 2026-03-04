# Phase 8 — Deferred Stages Reference

**Purpose:** Capture design intent for Phase 8 stages that are planned but not yet prioritized for implementation. Each section contains enough detail to draft a full Codex prompt when the time comes.

**Active stages (in progress or complete):**
- 8.1 — State Reconciliation Engine ✅ (spec ready)
- 8.2 — Progressive Indexing ✅ (spec ready)
- 8.7 — Stress Test Harness ✅ (spec ready)

**Deferred stages (this document):**
- 8.3 — Team Tier Hardening
- 8.4 — Interactive State Management (UI Write APIs)
- 8.5 — Operational Telemetry & Health Dashboard
- 8.6 — CI Index Publisher
- 8.8 — SIR Feedback Loop & Regeneration Triggers

---

## Stage 8.3 — Team Tier Hardening

**Priority:** High (run after 8.7 validates 8.1 + 8.2)
**Depends on:** 8.1, 8.2
**Estimated effort:** 1 Codex run

### Problem
`aether-query` allows read-only HTTP access to the shared index, but as multiple developers hit it simultaneously, cache invalidation and connection pooling will bottleneck. Currently there is no rate limiting, no cache invalidation signal from `aetherd` to `aether-query`, and no connection pool tuning for SurrealDB/LanceDB under concurrent load.

### Key Implementation Items

1. **Index Hot-Swap Notification:**
   - `aetherd` writes a monotonically increasing counter to a known SQLite row (`last_write_epoch`) after each completed write intent
   - `aether-query` polls this counter on a configurable interval (default: 5s)
   - When the counter advances, `aether-query` invalidates its in-memory caches (SIR cache, graph traversal cache, search result cache)
   - Lightweight, no IPC required — SQLite WAL mode supports concurrent readers

2. **Token-Bucket Rate Limiting:**
   - Per-IP and per-Bearer-token rate limiting on `aether-query` endpoints
   - Configurable in `config.toml`: `[query_server] rate_limit_per_minute = 120`
   - Returns HTTP 429 with `Retry-After` header when exceeded
   - Protects against infinite-loop AI agents consuming all capacity

3. **Connection Pool Sizing:**
   - Expose configurable pool sizes for SurrealDB and LanceDB connections
   - Default: 4 concurrent SurrealDB connections, 2 LanceDB readers
   - Add connection pool metrics (active, idle, waiting) to status endpoint

4. **Load Testing Step:**
   - Before implementing fixes, run the stress test harness (8.7) with concurrent query load
   - Identify actual bottlenecks rather than assumed ones
   - Document findings to prioritize the implementation order

### Files to Modify
- `crates/aether-query/src/server.rs` — Rate limiting middleware, cache invalidation
- `crates/aether-store/src/sqlite.rs` — `last_write_epoch` row + update method
- `crates/aether-store/src/graph_surreal.rs` — Connection pool configuration
- `crates/aether-config/src/lib.rs` — Rate limit and pool size config fields
- `crates/aetherd/src/sir_pipeline.rs` — Write epoch increment after intent completion

---

## Stage 8.4 — Interactive State Management (The HTMX Ceiling)

**Priority:** Low (defer to Phase 9)
**Depends on:** 8.1, 8.2
**Estimated effort:** 2 Codex runs

### Problem
HTMX + D3 was the right choice for a read-only dashboard. But manual SIR editing, graph annotation, and project memory curation require rich client-side state management that server-rendered HTML fragments handle poorly.

### Key Implementation Items

1. **Alpine.js Integration:**
   - Add Alpine.js (CDN) for complex client-side interactions
   - Use for: multi-select graph nodes, live-editing SIR JSON forms, drag-and-drop
   - HTMX continues to handle page navigation and data fetching
   - Alpine handles local UI state within those fragments

2. **Write-API Endpoints:**
   - `POST /api/v1/sir/{symbol_id}` — Override LLM-generated SIR with human-curated version
   - `DELETE /api/v1/graph/edge/{edge_id}` — Remove false-positive dependency edges
   - `PUT /api/v1/notes/{note_id}` — Edit project memory notes
   - `POST /api/v1/symbols/{symbol_id}/flag` — Flag symbols for re-indexing
   - All write endpoints require authentication (bearer token from config)

3. **SPA Migration Decision Document:**
   - Document the threshold at which AETHER's UI should transition to a dedicated SPA framework
   - Criteria: number of interactive forms, client-side state complexity, team size
   - Recommendation: defer SPA migration until >20 interactive views or >2 developers working on UI

### Recommendation
**Defer to Phase 9.** This is feature expansion, not hardening. The current HTMX dashboard serves its purpose for read-only intelligence visualization. Write APIs can wait until there's user demand for manual curation.

---

## Stage 8.5 — Operational Telemetry & Health Dashboard

**Priority:** Medium-High (run after 8.3)
**Depends on:** 8.1, 8.2
**Estimated effort:** 1 Codex run

### Problem
AETHER has `tracing::warn` and `SirQualityMonitor` but no structured metrics pipeline. Debugging production issues on large repos or during enterprise pilots requires real observability.

### Key Implementation Items

1. **Prometheus-Compatible Metrics Endpoint:**
   - Add `GET /metrics` endpoint to `aether-query` (and optionally to `aetherd`'s dashboard server)
   - Use the `metrics` crate (zero-cost when no exporter attached) + `metrics-exporter-prometheus`
   - Metrics to expose:
     - `aether_symbols_total` (gauge) — Total symbols in SQLite
     - `aether_sir_total` (gauge) — Symbols with SIR generated
     - `aether_sir_coverage_ratio` (gauge) — SIR coverage percentage
     - `aether_sir_generation_duration_seconds` (histogram) — Per-symbol SIR generation time
     - `aether_sir_generation_errors_total` (counter, by provider) — Failed SIR generations
     - `aether_sir_queue_depth` (gauge) — Priority queue size
     - `aether_inference_requests_total` (counter, by provider) — Inference API calls
     - `aether_inference_latency_seconds` (histogram, by provider) — Inference latency
     - `aether_vector_store_size_bytes` (gauge) — LanceDB storage size
     - `aether_graph_nodes_total` (gauge) — SurrealDB symbol count
     - `aether_graph_edges_total` (gauge) — SurrealDB edge count
     - `aether_query_latency_seconds` (histogram, by tool) — MCP query latency
     - `aether_write_intents_total` (counter, by status) — Intent log activity
     - `aether_fsck_inconsistencies_total` (gauge, by type) — Last fsck results

2. **Dashboard "System Health" Panel:**
   - New page on existing dashboard: `/dashboard/system`
   - Pulls from `/metrics` endpoint (or internal API)
   - Shows: SIR generation throughput graph, provider error rates, queue depth over time, memory usage
   - Uses existing D3 chart infrastructure (line charts, gauge charts)

3. **Structured Logging Enhancement:**
   - Ensure all key operations emit structured tracing spans with measurable fields
   - Add span for: index_pass_1, index_pass_2_symbol, sir_generation, vector_upsert, graph_upsert
   - Compatible with `tracing-subscriber` JSON output for log aggregation

### Files to Modify
- `crates/aether-query/src/server.rs` — `/metrics` endpoint
- `crates/aetherd/src/sir_pipeline.rs` — Instrument with metrics counters/histograms
- `crates/aether-infer/src/lib.rs` — Instrument provider calls
- `crates/aether-dashboard/` — New system health page
- `Cargo.toml` (workspace) — Add `metrics`, `metrics-exporter-prometheus` deps

### Dependencies
- `metrics = "0.22"` — Zero-cost metrics facade
- `metrics-exporter-prometheus = "0.13"` — Prometheus text format exporter

---

## Stage 8.6 — CI Index Publisher

**Priority:** Medium (run after 8.5)
**Depends on:** 8.1, 8.2
**Estimated effort:** 1 Codex run

### Problem
The progressive indexing from 8.2 solves cold start on a developer's machine, but team deployments still require each developer to run local inference. CI publishing offloads the expensive work to CI where time and compute are cheap.

### Key Implementation Items

1. **`aether index --ci --output <path>`:**
   - New CLI flag that runs a full index (Pass 1 + Pass 2) and exports the `.aether/` directory as a compressed archive
   - Format: `aether-index-{repo}-{commit-short}.tar.zst` (zstd compression for speed + ratio)
   - Contents: SQLite DB, LanceDB tables, SurrealDB data directory
   - Metadata file inside archive: `index-metadata.json` with commit SHA, symbol count, SIR coverage, timestamp, AETHER version

2. **`aether index --import <archive>`:**
   - Import a CI-produced index archive
   - Validates AETHER version compatibility (reject if schema version mismatch)
   - Extracts to `.aether/` directory
   - Runs fsck after import to verify integrity

3. **Delta Mode:**
   - `aether index --ci --delta --base-index <path>` — If a previous index archive exists, only re-index symbols in files changed between the base index's commit and HEAD
   - Uses `git diff --name-only <base-commit>..HEAD` to determine changed files
   - Re-runs Pass 1 on changed files only, then Pass 2 for new/modified symbols
   - Dramatically reduces CI time for incremental builds

4. **GitHub Actions Template:**
   - `docs/ci/github-actions-index.yml` — Example workflow that runs nightly indexing
   - Caches the previous index archive as a GitHub Actions artifact
   - Uses delta mode for incremental updates
   - Publishes the new archive as a downloadable artifact

### Files to Create/Modify
- `crates/aetherd/src/cli.rs` — `--ci`, `--output`, `--import`, `--delta`, `--base-index` flags
- `crates/aetherd/src/export.rs` — Archive creation and extraction logic
- `docs/ci/github-actions-index.yml` — Example CI workflow
- `docs/ci/gitlab-ci-index.yml` — Example GitLab CI workflow

### Dependencies
- `zstd = "0.13"` — Zstandard compression (fast, good ratio)
- `tar = "0.4"` — Archive creation/extraction

---

## Stage 8.8 — SIR Feedback Loop & Regeneration Triggers

**Priority:** Medium (run when model quality becomes the bottleneck)
**Depends on:** 8.1, 8.2
**Estimated effort:** 1 Codex run

### Problem
The `SirQualityMonitor` warns via `tracing::warn` when confidence drops, but nothing acts on it. Low-quality SIR from a fast local model is never upgraded when a better model becomes available. There's no way to bulk-improve SIR quality after a provider change.

### Key Implementation Items

1. **`aether regenerate` CLI Command:**
   - `aether regenerate --below-confidence 0.5` — Queue all symbols with SIR confidence below threshold for regeneration
   - `aether regenerate --provider gemini --from-provider qwen3_local` — Regenerate all SIR originally produced by one provider using a different provider
   - `aether regenerate --file <path>` — Regenerate SIR for all symbols in a specific file
   - Uses the priority queue from 8.2, so regeneration is interleaved with new-symbol generation

2. **Automatic Regeneration on Provider Change:**
   - When `config.toml` `[inference] provider` changes, detect on next daemon startup
   - Log: "Provider changed from qwen3_local to gemini. N symbols with confidence < 0.7 queued for regeneration."
   - Queue low-confidence symbols automatically (respects priority queue ordering)
   - Store `generated_by_provider` and `generated_by_model` in the SIR metadata

3. **SIR Quality Dashboard View:**
   - New panel on existing dashboard: confidence distribution histogram
   - Breakdown by provider: "Gemini: 4,521 symbols, avg confidence 0.89 | Qwen: 3,200 symbols, avg confidence 0.62"
   - "Upgrade Available" badge for symbols where a better provider is configured but SIR was generated by a weaker one

4. **Quality-Triggered Alerts:**
   - When SIR confidence average drops below a configurable floor (default: 0.5), emit a structured warning
   - If telemetry (8.5) is enabled, increment `aether_sir_quality_warnings_total` counter
   - Dashboard shows a banner: "⚠️ SIR quality degraded — consider switching to a stronger inference provider"

### Files to Modify
- `crates/aetherd/src/cli.rs` — `regenerate` subcommand
- `crates/aetherd/src/sir_pipeline.rs` — Regeneration queue logic
- `crates/aether-store/src/sqlite.rs` — `generated_by_provider`, `generated_by_model` columns on SIR table
- `crates/aether-config/src/lib.rs` — Quality floor config, regeneration settings
- `crates/aether-dashboard/` — Quality view panel (if 8.5 is done)

---

## Recommended Execution Order

After 8.1 → 8.2 → 8.7 are complete:

```
8.3 (Team Tier)     — validates multi-user before enterprise pilots
  ↓
8.5 (Telemetry)     — enables monitoring for all subsequent stages
  ↓
8.6 (CI Publisher)   — bridges solo → team adoption
  ↓
8.8 (SIR Feedback)  — self-improving quality loop

8.4 (UI Writes)     — defer to Phase 9 unless user demand arises
```

---

*Last updated: 2026-03-03 — Phase 8 planning*
