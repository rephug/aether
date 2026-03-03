# Hardening Pass 6 Session Context — Gemini Deep Review Fixes

## What You Need to Know

I'm building AETHER, a Rust multi-crate workspace that creates persistent semantic intelligence for codebases. We've completed through Phase 7 (The Pathfinder) plus dashboard revision runs. This hardening pass addresses bugs found by a Gemini deep review of the full codebase (via repomix XML export).

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
- **Hardening passes 1–4** — 40+ fixes across security, performance, correctness (all merged)
- **Hardening pass 5** — Codex prompts drafted (P0/P1 and P2/P3), fixes HTTP timeouts, reranker runtime, vector over-fetch, SurrealDB edge dedup, README cleanup — **partially applied** (HTTP timeouts, OnceLock reranker runtime, and vector over-fetch are live in main; other 5a/5b fixes may not be merged yet)
- **Dashboard revision** — Run 5 merged (LLM-powered Ask AETHER, guided tours, glossary)
- **Gemini deep review** — New review against the full codebase found 13 issues, several overlapping with pass 5 findings, several net-new

## What's Already Fixed (Skip These)

These were addressed in pass 5 and are confirmed present in current main:

| Fix | Status |
|-----|--------|
| HTTP timeouts on all inference providers | ✅ `inference_http_client()` / `management_http_client()` in use |
| Reranker per-call Tokio runtime → `OnceLock<Runtime>` | ✅ `search.rs:28` |
| Vector search over-fetch (3× multiplier) | ✅ `search.rs:316` |

## What This Pass Fixes

### Run A — P0/P1 Critical (7 fixes, ~120 lines)

| ID | Fix | File(s) | Risk |
|----|-----|---------|------|
| A1 | SIR regeneration loop — stale timestamp on unchanged hash | `aether-store/src/lib.rs:1602` | Burns API tokens every restart |
| A2 | Whole-file hallucination — extraction fallback sends entire file to LLM | `aetherd/src/sir_pipeline.rs:873` | Permanently corrupts SIR DB |
| A3 | Time Machine 1970 — seconds vs milliseconds in 3 queries | `aether-dashboard/src/api/time_machine.rs` | Time Machine always shows current state |
| A4 | Unbounded dashboard caches — HashMap keyed by sir_count grows forever | `aether-dashboard/src/state.rs:17-18` | OOM on long-running daemon |
| A5 | MCP hybrid search starves reranker — passes `limit` not `retrieval_limit` | `aether-mcp/src/lib.rs:1348,1389` | Poor MCP search quality |
| A6 | `run_async_with_timeout` creates new Tokio runtime per request | `aether-dashboard/src/support.rs:367` | Massive latency, potential deadlock |
| A7 | Semantic records duplication — stale records accumulate on edit | `aether-store/src/document_store.rs:192` | SQLite bloat, search pollution |

### Run B — P2/P3 Polish (6 fixes, ~80 lines)

| ID | Fix | File(s) | Risk |
|----|-----|---------|------|
| B1 | Analyzers bypass SharedState — open fresh RW connections | `aether-dashboard/src/api/{architecture,causal_chain,health}.rs`, `aether-analysis/src/drift.rs` | Crash in read-only mode |
| B2 | Unfixed `normalize_rename_path` in drift.rs | `aether-analysis/src/drift.rs:1211` | Drift drops renamed symbols |
| B3 | SSE semaphore permit drops early | `aether-query/src/server.rs:161` | Rate limit bypass under load |
| B4 | LSP UTF-16 → UTF-8 column mismatch | `aether-lsp/src/lib.rs:201` | Hover fails on multi-byte lines |
| B5 | Double-mutex in SqliteVectorStore | `aether-store/src/vector.rs:117` | Unnecessary lock contention |
| B6 | WET `percent_encode` (6 copies) | `aether-dashboard/src/fragments/*.rs` | Tech debt |

## Current Architecture — Key Files

### SIR Pipeline (`crates/aetherd/src/sir_pipeline.rs`)
Processes symbol change events → extracts symbol text → sends to LLM → records SIR version. Freshness check at line 457: `source_modified_at_ms < meta.updated_at * 1000`. If `updated_at` doesn't advance on unchanged hashes, the symbol stays perpetually "stale."

### SIR Version Recording (`crates/aether-store/src/lib.rs:1562`)
`record_sir_version_if_changed()` compares new SIR hash against latest. If unchanged, returns `SirVersionWriteResult { updated_at: latest_created_at, changed: false }`. This `updated_at` propagates to `sir_meta.updated_at` via the pipeline.

### Time Machine (`crates/aether-dashboard/src/api/time_machine.rs`)
Dashboard scrubber passes `at_ms` (milliseconds). Three query locations compare against `sir_history.created_at` (seconds) without conversion. The `symbols` fallback path correctly uses `* 1000` but the `sir_history` path does not.

### Dashboard Caches (`crates/aether-dashboard/src/state.rs`)
`DashboardCaches` uses `HashMap<i64, String>` and `HashMap<i64, LayerAssignmentsCache>` keyed by `sir_count`. Old entries never evicted. Only the current `sir_count` is ever useful.

### Dashboard Timeout Helper (`crates/aether-dashboard/src/support.rs`)
Two functions: `run_blocking_with_timeout` (correct — uses `spawn_blocking`) and `run_async_with_timeout` (broken — creates `new_current_thread()` runtime inside `spawn_blocking`).

### MCP Search (`crates/aether-mcp/src/lib.rs`)
`aether_search_logic()` fetches `limit` candidates from lexical (line 1348) and semantic (line 1389) searches. The `fuse_limit` calculation at line 1399 correctly uses `rerank_window.max(limit)`, but the upstream fetch pools are already capped at `limit`. The CLI path (`aetherd/src/search.rs:316`) was already fixed with `limit * 3`.

### Document Store (`crates/aether-store/src/document_store.rs`)
`insert_semantic_record` upserts on `ON CONFLICT(record_id)`. Since `record_id = BLAKE3(unit_id + schema_version + content_hash)`, content changes produce a new `record_id`, leaving the old record orphaned.

### Analyzers (`crates/aether-analysis/src/drift.rs`, `health.rs`, `coupling.rs`)
Constructors take a `workspace: &Path` and open fresh `SqliteStore::open()` (read-write) and `CozoGraphStore::open()` connections on each method call. Bypasses SharedState pooling.

## Decisions Made

- **SIR regeneration fix:** Return `created_at` (the current attempt time) instead of `latest_created_at` when hash unchanged. This advances the freshness watermark without creating a new version row.
- **Whole-file fallback:** Return `Err` from `build_job()` instead of falling back to the whole file. The symbol gets logged as failed but doesn't corrupt the DB. This is safer than using `qualified_name` (which would produce low-quality SIRs anyway).
- **Dashboard caches:** Replace `HashMap<i64, T>` with `Option<(i64, T)>`. Only the current `sir_count` tuple is needed.
- **`run_async_with_timeout`:** Rewrite to use `tokio::time::timeout` directly on the future. No `spawn_blocking`, no `new_current_thread()`.
- **Semantic records:** DELETE stale records for the same `(unit_id, schema_name)` before INSERT. This is the minimal change; a `UNIQUE(unit_id, schema_name)` constraint could also work but requires a migration.
- **Analyzer read-only:** For Run B, the most practical fix is wrapping the analyzer calls in `spawn_blocking` and passing `SqliteStore::open_readonly()`. Full refactor to accept `&SqliteStore` references is a larger change better suited for Phase 8.
- **SSE permit:** Tie the permit to the response body stream using a wrapper type, not extensions.

---

*Last updated: 2026-03-03 — Hardening pass 6 (Gemini deep review fixes)*
