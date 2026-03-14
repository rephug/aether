# Phase 10.3 — Agent Integration Hooks — Session Context

**Date:** 2026-03
**Branch:** `feature/phase10-stage10-3-agent-hooks` (to be created)
**Worktree:** `/home/rephu/aether-phase10-agent-hooks` (to be created)
**Starting commit:** HEAD of main after 10.1 merged (10.2 NOT required — 10.3 is parallel with 10.2)
**Prerequisite:** Stage 10.1 merged — needs `sir_fingerprint_history` table and `sir.prompt_hash` column (migration v8).

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

## What this stage adds

Three new CLI commands:

### `aetherd sir context <symbol>`
Token-budgeted context assembly. Greedy knapsack with 9-tier priority ordering. Default 16K tokens. Outputs markdown, JSON, or text. Pulls from SQLite (SIR, symbols, coupling, test intents, memory), SurrealDB (edges), and gix (recent commits).

### `aetherd sir inject <symbol> --intent "..."`
Direct SIR update without inference. Synchronous re-embedding if embeddings enabled. Writes fingerprint history row with `trigger = "inject"`.

### `aetherd sir diff <symbol>`
Structural comparison between current SIR and current source code. No inference — tree-sitter only.

---

## Key files to understand

**CLI:**
- `crates/aetherd/src/cli.rs` — `Commands` enum, add three new variants
- `crates/aetherd/src/main.rs` — `run_subcommand()` dispatch

**SIR storage:**
- `crates/aether-store/src/sir_meta.rs` — `SirMetaRecord` struct, `store_upsert_sir_meta`, `store_get_sir_meta`
- **The table is named `sir` (NOT `sir_meta`).** Primary key is `id` (NOT `symbol_id`). SIR JSON is in the `sir_json` column.

**Edges (for dependency/dependent lookup):**
- **Use SQLite `symbol_edges` table, NOT SurrealDB.** SurrealKV exclusive lock means querying SurrealDB while daemon runs crashes.
- `store.get_callers(qualified_name)` and `store.get_dependencies(symbol_id)` in `crates/aether-store/src/graph.rs`
- These read from SQLite `symbol_edges` table directly — lock-free

**Coupling:**
- **Coupling data is in SurrealDB, NOT SQLite.** There is NO `coupling_pairs` table.
- Access via `graph_store.list_co_change_edges_for_file(path, min_score)`
- For `sir context` running alongside daemon: SurrealDB may be locked. Handle gracefully — skip coupling section and note "coupling data unavailable (daemon holds lock)"

**Test intents:**
- `test_intents` SQLite table — exists, verify column names
- Check `crates/aetherd/src/test_intents.rs` for query pattern

**Project memory:**
- **Table is `project_notes` (NOT `project_memory`)**
- Check `crates/aetherd/src/memory.rs` for query pattern

**Embeddings:**
- Read embedding provider config from `[embeddings]` section
- For inject re-embed: construct a one-off embedding provider and call embed for the single symbol
- **CRITICAL for delta_sem:** Fetch the OLD embedding BEFORE upserting the new one. LanceDB overwrites by symbol_id.

**Fingerprint history (from 10.1):**
- `sir_fingerprint_history` SQLite table
- Reuse `write_fingerprint_row()` helper from 10.1's batch module

**Tree-sitter (for sir diff):**
- `crates/aether-parse/src/parser.rs` — `SymbolExtractor` extracts `Symbol` structs with `signature_fingerprint` and `content_hash`
- Do NOT attempt deep AST extraction (parameter names/types). Compare fingerprints only.

---

## Token estimation

Use `chars / 3.5` as the token estimate. This is approximate but consistent with the rest of the codebase. Do NOT add a `tiktoken` dependency — the approximation is sufficient for budget enforcement.

---

## Scope guard — do NOT modify

- Batch pipeline (10.1)
- Existing CLI subcommands
- Existing SIR pipeline
- Existing watcher behavior
- Existing MCP tools

---

## End-of-stage sequence

```bash
cd /home/rephu/aether-phase10-agent-hooks
git push -u origin feature/phase10-stage10-3-agent-hooks

# After PR merges:
cd /home/rephu/projects/aether
git switch main
git pull --ff-only
git worktree remove ../aether-phase10-agent-hooks
git branch -d feature/phase10-stage10-3-agent-hooks
```
