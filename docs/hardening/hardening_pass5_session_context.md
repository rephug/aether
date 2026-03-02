# Hardening Pass 5 Session Context — Combined Code Review Fixes

## What You Need to Know

I'm building AETHER, a Rust multi-crate workspace that creates persistent semantic intelligence for codebases. We've completed through Phase 7 (The Pathfinder). This hardening pass addresses bugs found by two independent code reviews: a Gemini review of the full codebase, and a Claude scan of the cloned repo.

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

- **Phase 7 complete** — All stages 7.1–7.8 merged (store pooling, SurrealDB migration, aether-query, document abstraction, AETHER Legal, web dashboard, dashboard viz, OpenAI-compat provider)
- **Hardening passes 1–4** — 40+ fixes across security, performance, correctness (all merged)
- **Stage 7.9** — Dashboard visual intelligence with six new visualization pages (merged)
- **Inference fix** — Updated default Gemini model, fixed Ollama model name, added SIR pipeline tracing (merged)
- **Two independent code reviews** — Gemini (full codebase) and Claude (cloned repo scan) found overlapping and distinct issues

## Current Architecture — Key Files for This Pass

### Graph Store (`crates/aether-store/src/graph_surreal.rs`)
SurrealDB 3.0 with SurrealKV backend. Edge creation uses `RELATE $src->depends_on->$dst`, which in SurrealDB sets `in=$src, out=$dst`. Queries use explicit `source_symbol_id`/`target_symbol_id` fields (not `in`/`out`), so caller/dependency results are correct despite the edge direction bug.

### Vector Store (`crates/aether-store/src/vector.rs`)
LanceDB vector backend. `search_nearest()` passes `limit` to LanceDB, then threshold filtering happens in `semantic_search()` (in `search.rs`) AFTER the limit has already been applied.

### Inference Providers (`crates/aether-infer/src/lib.rs`)
Four provider structs: `GeminiProvider`, `Qwen3LocalProvider`, `OpenAiCompatProvider`, `Qwen3LocalEmbeddingProvider`. All construct HTTP clients via `reqwest::Client::new()` with no timeout configuration.

### Search Pipeline (`crates/aetherd/src/search.rs`)
`rerank_rows_with_provider()` creates a new Tokio runtime per call via `tokio::runtime::Builder::new_current_thread()`.

### MCP Tools (`crates/aether-mcp/src/lib.rs`)
4,312-line file with all tool logic functions. README advertises `aether_snapshot_intent` and `aether_verify_intent` which don't exist. Four implemented tools (`aether_call_chain`, `aether_dependencies`, `aether_status`, `aether_symbol_timeline`) are NOT in the README.

### Dashboard API (`crates/aether-dashboard/src/api/`)
Multiple handlers call synchronous analyzers directly on async Axum worker threads: `architecture.rs` (DriftAnalyzer), `causal_chain.rs` (CausalAnalyzer).

### Python Parser (`crates/aether-parse/src/languages/python.rs`)
`nearest_ancestor_name()` returns only the first matching ancestor, so `class Outer: class Inner: def method()` produces `module::Inner::method` instead of `module::Outer::Inner::method`.

### Graph Store Readonly (`crates/aether-store/src/lib.rs` line 569)
`open_graph_store_readonly()` routes the `Surreal` backend arm through `CozoGraphStore::open_readonly()` — identical to the `Cozo` arm. Works accidentally via the compat shim but is semantically wrong.

### Call Chain (`crates/aether-store/src/graph_surreal.rs` line 958)
`get_call_chain()` calls `self.list_all_symbols().await?` after BFS traversal, loading the entire symbol table into a HashMap just to resolve a few results.

## Decisions Made

- **README will be updated** to remove fictional CLI flags and MCP tools, and add undocumented but working tools
- **HTTP timeouts** will use 10s connect + 120s request for inference, 30s for Ollama management calls
- **Vector search over-fetch** will use a 3× multiplier on the limit to compensate for threshold filtering
- **SurrealDB edge dedup DELETE** will be fixed to match `RELATE` direction
- **Reranker runtime** will use a static `OnceLock<Runtime>` like the CozoGraphStore compat shim
- **Dashboard sync calls** will be wrapped in `tokio::task::spawn_blocking`
- **Python qualified names** will collect all ancestor names, not just the nearest one

---

*Last updated: 2026-03-01 — Hardening pass 5 (combined Gemini + Claude code review fixes)*
