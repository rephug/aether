# Phase 8.19 — MCP Refactoring Intelligence — Session Context

**Date:** 2026-03-13
**Branch:** `feature/phase8-stage8-19-mcp-refactoring-intelligence` (to be created)
**Worktree:** `/home/rephu/aether-phase8-mcp-refactor-intel` (to be created)
**Starting commit:** HEAD of main after Store trait refactor lands

## CRITICAL: Read actual source, not this document

```bash
# The live repo is at:
/home/rephu/projects/aether

# Always grep/read actual source before making claims about what exists
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

## What just merged (recent history)

| Commit | What |
|--------|------|
| (pending) | Store trait decomposition into 11 sub-traits |
| 06003e5 | Revise Phase 8.18 spec |
| f5bd037 | Batch embedding meta lookup (ARCH-2 N+1 fix) |
| b9834e4 | Batch progress logging |
| f6150b8 | Triage/deep concurrency fix (14x speedup) |
| 4e0917c–ccc8ec3 | All 6 God File refactors |
| 9f07651 | Phase 8.17 — Gemini native embedding provider |

## The problems being solved

Three gaps discovered during the Store trait A/B refactoring experiment
(Codex blind vs Codex+AETHER). These are the highest-impact improvements
for making AETHER useful during interface decomposition tasks.

### Gap 1: No usage matrix tool

The core question for trait decomposition — "which consumers call which
methods?" — has no tool answer. AETHER has CALLS edges and method symbols
but doesn't aggregate them into a consumer × method matrix. During the
experiment, Codex had to infer groupings from SIR dependency lists
(flat, no method attribution) instead of from actual call patterns.

### Gap 2: Type-level dependencies return nothing

`aether_dependencies` for SqliteStore (a struct) returns zero edges
because edges are method-level. No aggregation exists from methods up
to their parent type. During the experiment, Codex acknowledged this
gap and fell back to `aether_blast_radius` (file-level, less precise).

### Gap 3: Semantic search fails silently without API key

`aether_search` with hybrid mode returned a raw error about missing
`GEMINI_API_KEY` because the MCP server doesn't validate embedding
provider availability at startup. Should warn and fall back to lexical.

## Key files to read

### For usage_matrix tool
- `crates/aether-mcp/src/tools/router.rs` — where tools are registered
- `crates/aether-mcp/src/tools/impact.rs` — blast_radius as reference for a complex query tool
- `crates/aether-mcp/src/tools/sir.rs` — where `aether_dependencies_logic` lives
- `crates/aether-store/src/graph.rs` — `store_get_callers`, `store_get_dependencies`
- `crates/aether-store/src/symbols.rs` — `list_symbols_for_file`, symbol queries
- `crates/aether-core/src/lib.rs` — `SymbolEdge`, `EdgeKind`

### For dependency aggregation
- `crates/aether-mcp/src/tools/sir.rs` — `aether_dependencies_logic` (the function to modify)
- `crates/aether-store/src/lib.rs` — Store trait (after refactor: the sub-traits)

### For search fallback
- `crates/aether-mcp/src/state.rs` — `SharedState` construction
- `crates/aether-mcp/src/tools/search.rs` — search tool implementation
- `crates/aether-config/src/embeddings.rs` — embedding config fields

## Schema reference

### symbol_edges table
```sql
CREATE TABLE IF NOT EXISTS symbol_edges (
    source_id TEXT NOT NULL,
    target_qualified_name TEXT NOT NULL,
    edge_kind TEXT NOT NULL CHECK (edge_kind IN ('calls', 'depends_on', 'type_ref', 'implements')),
    file_path TEXT NOT NULL,
    PRIMARY KEY (source_id, target_qualified_name, edge_kind)
);
```

### Key query pattern for usage matrix
```sql
-- Find all methods of a trait/struct by qualified_name prefix
SELECT id, qualified_name, file_path, kind
FROM symbols
WHERE qualified_name LIKE '{TypeName}::%'
  AND file_path = '{type_file_path}'
  AND kind IN ('function', 'method');

-- Find callers of a specific method
SELECT source_id, file_path
FROM symbol_edges
WHERE target_qualified_name = '{method_qualified_name}'
  AND edge_kind = 'calls';

-- Resolve caller source_id to file_path
SELECT file_path FROM symbols WHERE id = '{source_id}';
```

## Crate layout (post-refactor)

```
crates/aether-mcp/src/
    tools/
        common.rs
        drift.rs
        health.rs
        history.rs
        impact.rs          — blast_radius (reference for complex query tool)
        memory.rs
        mod.rs             — ADD: pub mod usage_matrix;
        router.rs          — ADD: aether_usage_matrix registration
        search.rs          — MODIFY: graceful fallback
        sir.rs             — MODIFY: type-level aggregation
        status.rs
        usage_matrix.rs    — NEW
        verification.rs
    error.rs
    lib.rs                 — MODIFY: thread validation
    main.rs
    state.rs               — MODIFY: add semantic_search_available
```

## Scope guards

- Do NOT change GraphStore or VectorStore
- Do NOT change the SIR generation pipeline or prompts
- Do NOT change the health score planner
- Do NOT add new dependencies to aether-mcp's Cargo.toml
- The usage_matrix tool uses SQLite queries only (Store trait methods)
- The dependency aggregation modifies only the MCP response, not the
  underlying edge storage
- The search fallback modifies only the MCP search tool, not the
  underlying search implementation in aetherd

## After this stage merges

End-of-stage git sequence:
```bash
git push -u origin feature/phase8-stage8-19-mcp-refactoring-intelligence
# Create PR via GitHub web UI
# After merge:
git switch main
git pull --ff-only
git worktree remove /home/rephu/aether-phase8-mcp-refactor-intel
git branch -d feature/phase8-stage8-19-mcp-refactoring-intelligence
```

Then re-run the Store trait experiment with `aether_usage_matrix`
available and compare the decomposition quality.
