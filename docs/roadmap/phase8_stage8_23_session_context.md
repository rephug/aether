# Session Context: Phase 8.23 — Graph Read-Path Denormalization

## What This Stage Does

Adds a `symbol_neighbors` denormalized table to `meta.sqlite` so that MCP, CLI, and LSP consumers can read graph neighbor data without touching SurrealKV (which holds an exclusive process-level file lock). Also fixes the MCP async graph initialization path to always use SQLite for the primary GraphStore interface.

## Why

SurrealKV uses `fs2` file locking. Only one process can open `.aether/graph/` at a time. The daemon holds this lock while running. The MCP server's async initialization path (`open_shared_graph_async`) tries to open `SurrealGraphStore` when `graph_backend = "surreal"`, which fails with `"LOCK is already locked"` if the daemon is scanning or triaging. CLI commands like `health-score`, `fsck`, and `refactor-prep` also open `SurrealGraphStore` directly and hit the same issue.

The fix: dual-write during indexing (SurrealDB + SQLite neighbor table), always use SQLite for the primary read path, and make SurrealDB an optional secondary that fails gracefully. Decision #58.

## Current State (Post PR #122)

- Latest merged: PR #122 (turbo quality batch — batched embeddings in triage/deep passes)
- Schema: `PRAGMA user_version = 14` (last migration: `sir_quality` table)
- `check_compatibility("core", 14)` in 5 places: 4 in `aether-mcp/src/state.rs`, 1 in `aether-dashboard/src/state.rs`
- PR #121 added bulk concurrent scan pass (`process_bulk_scan`)
- PR #122 unified embedding pipeline across scan and quality batch (`process_pending_embeddings`)

## Key Finding From Repo Inspection

The MCP tools have TWO graph initialization paths:

1. **Sync `open_shared_graph`** (line ~340 of `aether-mcp/src/state.rs`) — ALWAYS maps to `SqliteGraphStore` regardless of config. MCP tools using this path already work while daemon runs.

2. **Async `open_shared_graph_async`** (line ~360) — Opens `SurrealGraphStore` when config says surreal. Two of the four `SharedState` constructors use this path. **THIS is the contention path that breaks MCP during scans.**

The fix in this stage: make `open_shared_graph_async` behave like the sync version — always return `SqliteGraphStore` as the primary `Arc<dyn GraphStore>`, with `SurrealGraphStore` as an optional secondary that fails gracefully.

## Key Files

### Schema & migrations
- `crates/aether-store/src/schema.rs` — `run_migrations()`, PRAGMA user_version checks. Latest is v14 (`sir_quality` table). New migration targets v15.

### Edge storage (write path)
- `crates/aether-store/src/graph.rs` — `store_upsert_edges()` (uses `symbol_edges` table: `source_id`, `target_qualified_name`, `edge_kind`, `file_path`), `store_delete_edges_for_file()`, `store_get_callers()`, `store_get_dependencies()`
- `crates/aetherd/src/indexer.rs` — where `upsert_edges` is called during indexing

### Read path (GraphStore trait)
- `crates/aether-store/src/graph_sqlite.rs` — `SqliteGraphStore` with `get_callers`, `get_dependencies`, `get_call_chain`. These JOIN `symbol_edges` with `symbols` to return resolved `SymbolRecord`.

### MCP state (contention fix needed here)
- `crates/aether-mcp/src/state.rs` — `open_shared_graph` (sync, always SQLite — fine), `open_shared_graph_async` (opens SurrealDB — needs fix)

### CLI commands that open SurrealDB directly (future follow-up)
- `crates/aetherd/src/health_score.rs` (line ~81)
- `crates/aetherd/src/fsck.rs` (line ~137)
- `crates/aetherd/src/refactor_prep.rs` (lines ~117, ~545)

### SurrealDB graph store (reference only — NOT modified)
- `crates/aether-store/src/graph_surreal.rs`

## Edge Types in the System

The `symbol_edges` table has `edge_kind` values:
- `calls` — function A calls function B
- `depends_on` — structural dependency
- `implements` — trait implementation
- `type_ref` — type reference

Note: `store_get_callers` only queries `edge_kind = 'calls'`. `store_get_dependencies` only queries `edge_kind = 'depends_on'`. The `SqliteGraphStore::get_dependencies` in `graph_sqlite.rs` may use `edge_kind = 'calls'` instead — verify during source inspection.

When denormalizing into `symbol_neighbors`, create both forward and reverse edges:
- `calls` → reverse: `called_by`
- `depends_on` → reverse: `depended_on_by`
- `implements` → reverse: `implemented_by`
- `type_ref` → reverse: `type_ref_by`

## What NOT to Change

- `crates/aether-store/src/graph_surreal.rs` — no modifications
- `GraphStore` trait definition — keep as-is
- Multi-hop queries (call chains, community detection, coupling) — stay on SurrealDB
- Inference providers, config crate, CLI args — not touched
- Batch pipeline (`batch/*.rs`) — not touched
- `aether_dependencies` MCP tool implementation — not migrated in this PR (future follow-up)

## Build Environment

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=16
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

## Validation

```bash
cargo fmt --all --check
cargo clippy -p aether-store -- -D warnings
cargo clippy -p aetherd --features dashboard -- -D warnings
cargo clippy -p aether-mcp -- -D warnings
cargo clippy -p aether-dashboard -- -D warnings
cargo test -p aether-store
cargo test -p aetherd
cargo test -p aether-mcp
```

Do NOT run `cargo test --workspace` — OOM risk.

## CRITICAL: Schema Migration Lesson

When bumping PRAGMA user_version, you MUST update ALL of these:
- `check_compatibility("core", N)` in `crates/aether-mcp/src/state.rs` (4 occurrences)
- `check_compatibility("core", N)` in `crates/aether-dashboard/src/state.rs` (1 occurrence)
- Any test assertions on `schema_version.version` in `crates/aether-store/src/tests/`

Missing these caused 3 CI round-trips on Phase 10.4. Do not repeat.

## PR Convention

- Branch: `feature/graph-denormalization`
- Worktree: `/home/rephu/feature/graph-denormalization`
- PR title: `feat(store): denormalize symbol neighbors to SQLite for contention-free graph reads`
- Always include descriptive PR body
