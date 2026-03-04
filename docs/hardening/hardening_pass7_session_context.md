# Hardening Pass 7 Session Context — Gemini Deep Review Fixes (Second Scan)

## What You Need to Know

I'm building AETHER, a Rust multi-crate workspace that creates persistent semantic intelligence for codebases. We've completed through Phase 7 (The Pathfinder) plus dashboard revision and Phase 8 specs. This hardening pass addresses bugs found by two independent Gemini deep reviews of the full codebase (via repomix XML export), cross-validated by Claude against the cloned repo at commit `c5d84ed`.

**Repo:** `https://github.com/rephug/aether` at `/home/rephu/projects/aether`
**Dev environment:** WSL2 Ubuntu, mold linker, sccache, all builds from `/home/rephu/`

**Required build environment (must be set before any cargo command):**
```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

## What Just Happened

- **Phase 7 complete** — All stages 7.1–7.9 merged (store pooling, SurrealDB migration, aether-query, document abstraction, AETHER Legal, web dashboard, dashboard viz, OpenAI-compat provider, visual intelligence)
- **Hardening passes 1–6** — 55+ fixes across security, performance, correctness (all merged)
- **Hardening pass 6a** — SIR regen loop, hallucination fallback, Time Machine timestamps, cache leak, MCP search quality, async timeout, semantic dedup (merged)
- **Hardening pass 6b** — Analyzer read-only, rename path, SSE permit, LSP UTF-16, vector mutex, percent_encode dedup (merged)
- **Phase 8 specs** — Session context, overview, stages 8.1/8.2/8.7 committed
- **Two new Gemini deep reviews** — Full codebase scan found 9 issues, all verified against source by Claude

## What's Already Fixed (Skip These)

These were addressed in pass 6 and are confirmed present in current main:

| Fix | Status |
|-----|--------|
| `run_async_with_timeout` runtime-per-request → `tokio::time::timeout` | ✅ `support.rs:367` rewritten in 6a |
| Semantic records duplication on content edit → DELETE before INSERT | ✅ `document_store.rs` has cleanup DELETE in 6a |
| Analyzer `SqliteStore::open()` → `open_readonly()` | ✅ Drift/health/causal analyzers fixed in 6b |

## What This Pass Fixes

### Run A — P0/P1 Critical (5 fixes, ~60 lines)

| ID | Fix | File(s) | Risk |
|----|-----|---------|------|
| A1 | Concurrency panic — `duration_since` in debounce retain | `aether-store/src/lib.rs:1261` | Daemon crash under concurrent load |
| A2 | Read-only violations — increment functions + auto_mine | `aether-store/src/lib.rs`, `aether-mcp/src/lib.rs` | Crashes `aether-query` read-only server |
| A3 | Timestamp seconds→ms mismatch in causal ranking fallback | `aether-analysis/src/causal.rs:697` | All non-git SIR events get recency ≈ 0 |
| A4 | Missing transaction on semantic record upsert | `aether-store/src/document_store.rs:190` | Data loss on crash between DELETE and INSERT |
| A5 | `std::thread::sleep` blocking Tokio in SurrealDB init | `aether-store/src/graph_surreal.rs:50` | Tokio thread starvation on lock contention |

### Run B — P2/P3 Performance (4 fixes, ~200 lines)

| ID | Fix | File(s) | Risk |
|----|-----|---------|------|
| B1 | O(N) full graph load for single edge check | `aether-store/src/graph_surreal.rs:272` | OOM + extreme latency in coupling analysis |
| B2 | Unified query post-processing blocks Tokio thread | `aether-memory/src/unified_query.rs:270` | Server deadlock under concurrent MCP load |
| B3 | Dashboard search API sync blocking on async thread | `aether-dashboard/src/api/search.rs:98` | Dashboard latency / worker pool starvation |
| B4 | Dashboard common graph algos block Tokio (louvain, pagerank) | `aether-dashboard/src/api/common.rs` | All dashboard visualizations stall under load |

## Current Architecture — Key Files

### Symbol Access Debouncing (`crates/aether-store/src/lib.rs:1245`)
`increment_symbol_access_debounced` captures `Instant::now()` before acquiring a mutex, then calls `tracker.retain(|_, last_accessed| now.duration_since(*last_accessed) < window)`. The retain uses the panicking `duration_since`; the individual symbol check at line 1273 already uses `saturating_duration_since`. Inconsistent.

### Read-Only Mode (`crates/aether-store/src/lib.rs:596`)
`SqliteStore::open_readonly()` opens with `SQLITE_OPEN_READ_ONLY`. The `increment_symbol_access` (line 1203) and `increment_project_note_access` (line 2240) functions unconditionally start write transactions. No read-only guard.

### MCP Blast Radius (`crates/aether-mcp/src/lib.rs:1748`)
`aether_blast_radius_logic` passes `auto_mine: true` unconditionally, which triggers writes to the graph and SQLite stores. Crashes when invoked via the read-only `aether-query` server.

### Causal Ranking Fallback (`crates/aether-analysis/src/causal.rs:682`)
`resolve_change_metadata` falls back to `after.created_at` when no git commit metadata exists. `created_at` in `sir_history` is stored in **seconds** (via `current_unix_timestamp()` which calls `duration.as_secs()`). The `timestamp_ms` field is consumed by `now_ms.saturating_sub(timestamp_ms)` at line 320, where `now_ms` is milliseconds. The unit mismatch causes `days_since_change ≈ 19,675`, decaying recency to 0.

### Semantic Record Upsert (`crates/aether-store/src/document_store.rs:190`)
Pass 6a added a `DELETE FROM semantic_records` before the `INSERT`. However, the DELETE and INSERT are two separate `conn.execute()` calls with no transaction wrapper. A crash between them loses data.

### SurrealDB Init Retry (`crates/aether-store/src/graph_surreal.rs:50`)
Inside `pub async fn open()`, lock contention triggers `std::thread::sleep(Duration::from_millis(50))` — a blocking sleep on an async thread.

### SurrealDB `has_dependency_between_files` (`crates/aether-store/src/graph_surreal.rs:272`)
Loads the entire symbol table + all edges into memory, builds HashMaps, then iterates to answer a single boolean. Called per file-pair in coupling analysis loops.

### Unified Query Post-Processing (`crates/aether-memory/src/unified_query.rs:270`)
`pub async fn ask()` runs `rank_coupling_candidates`, `enrich_symbol_snippets`, and `increment_access_from_results` synchronously on the Tokio worker thread. The search phases (lines 121-170) already use `spawn_blocking` correctly — the post-processing phase was missed.

### Dashboard Search / Common (`crates/aether-dashboard/src/api/search.rs`, `common.rs`)
`load_search_data` calls `search_symbols`, `load_dependency_algo_edges`, `latest_drift_score_by_symbol` etc. directly on the async thread. `louvain_map` and `pagerank_map` in `common.rs` run CPU-heavy graph algorithms synchronously in their CozoDB fallback paths. Pass 6a fixed `run_async_with_timeout` to use `tokio::time::timeout`, but the *callers* still block.

## Decisions Made

- **Debounce panic fix:** Use `saturating_duration_since` to match the pattern already at line 1273
- **Read-only guards:** Add `conn.is_readonly()` check inside increment functions (no-op when read-only). Change `auto_mine: true` to `auto_mine: !self.state.read_only` in blast radius
- **Drift report NOT guarded:** Gemini claimed `drift_report_logic` needs `require_writable()` — this is wrong. `DriftAnalyzer::report()` opens its own read-only connection and does not write. Skip this sub-fix
- **Causal timestamp:** Multiply fallback `created_at` by 1000 via `saturating_mul(1000)` to convert seconds → milliseconds
- **Semantic record transaction:** Wrap the existing DELETE + INSERT in a proper SQLite transaction
- **SurrealDB sleep:** Replace `std::thread::sleep` with `tokio::time::sleep().await`
- **has_dependency_between_files:** Replace O(N) in-memory scan with a targeted SurrealQL query using `in.file_path` / `out.file_path` record reference traversal
- **Unified query spawn_blocking:** Wrap the post-processing block (coupling + enrich + increment) in `tokio::task::spawn_blocking`, matching the pattern of the search phases above it
- **Dashboard APIs:** Wrap synchronous SQLite loads in `spawn_blocking`. Wrap CPU-heavy graph algo fallbacks (`louvain_communities`, `page_rank`) in `spawn_blocking`

## Gemini Findings Rejected / Corrected

| Finding | Verdict | Reason |
|---------|---------|--------|
| Drift report needs `require_writable()` | ❌ Rejected | `DriftAnalyzer::report()` opens its own read-only store; does not write to DB |
| Dashboard `xray.rs` same issue as `search.rs` | ✅ Valid but omitted | Same pattern — will be addressed alongside search.rs if `xray.rs` has the same blocking pattern |
| `blast_radius.rs` `build_node` needs `spawn_blocking` | ✅ Valid but deferred | Per-node `spawn_blocking` adds overhead; better to pre-load symbol records into a HashMap before the loop (Phase 8 refactor) |

---

*Last updated: 2026-03-03 — Hardening pass 7 (two Gemini deep review scans, Claude-verified)*
