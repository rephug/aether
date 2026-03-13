# Phase 8.19: MCP Refactoring Intelligence

## Purpose

Three targeted improvements to AETHER's MCP tooling, driven by gaps
discovered during the Store trait A/B refactoring experiment. These
address the highest-impact gaps that would have changed the quality
of the AETHER-informed refactoring plan.

## Prerequisites

- Phase 8.17 merged (Gemini native embedding provider)
- Store trait refactor merged (or at least the 11-subtrait plan finalized)
- MCP server functional (`aether_status` returns data via Codex)

## Problem

During the Store trait decomposition experiment (blind vs AETHER-informed),
three gaps reduced the value of AETHER's semantic intelligence:

1. **No consumer × method usage matrix.** The core question for trait
   decomposition is "which consumers call which methods?" AETHER has
   all the data (CALLS edges + method symbols) but no tool aggregates
   it into a matrix view. Codex had to infer groupings from SIR
   dependency lists instead.

2. **`aether_dependencies` returns nothing for structs/traits.** SqliteStore
   is the most important symbol in the crate and AETHER can't report its
   dependency relationships. Edges are method-level, not type-level.
   There's no aggregation from methods up to their parent type.

3. **MCP server silently fails on semantic search when API key is missing.**
   `aether_search` with hybrid mode returned a raw error about missing
   `GEMINI_API_KEY`. The server should validate embedding provider
   availability at startup and warn, and tools should degrade gracefully
   to lexical mode instead of erroring.

## New Tool: `aether_usage_matrix`

### Request

```json
{
  "symbol": "Store",
  "file": "crates/aether-store/src/lib.rs",
  "kind": "trait"
}
```

`symbol` is required. `file` and `kind` help disambiguate. The tool
resolves the target symbol, finds all methods defined on it (for traits)
or all methods with `self` receivers (for structs), then queries CALLS
edges to build the matrix.

### Algorithm

```
1. Resolve target symbol by name (+ optional file/kind filter)
2. Find all child method symbols:
   - For traits: symbols where qualified_name starts with "{TraitName}::"
     AND file_path matches AND kind is "function" or "method"
   - For structs/enums: symbols in the same file where qualified_name
     starts with "{TypeName}::" (impl block methods)
3. For each method, query symbol_edges WHERE target_qualified_name
   matches the method's qualified_name AND edge_kind = 'calls'
4. Resolve each caller's source_id back to a SymbolRecord to get
   its file_path
5. Build matrix: group callers by file_path, columns by method name
6. Return structured JSON
```

### Response

```json
{
  "schema_version": "1.0",
  "target": "Store",
  "target_file": "crates/aether-store/src/lib.rs",
  "method_count": 52,
  "consumer_count": 23,
  "matrix": [
    {
      "consumer_file": "crates/aetherd/src/indexer.rs",
      "methods_used": ["upsert_symbol", "read_sir_blob", "upsert_sir_meta", "list_sir_history"],
      "method_count": 4
    },
    {
      "consumer_file": "crates/aetherd/src/fsck.rs",
      "methods_used": ["read_sir_blob", "get_sir_meta"],
      "method_count": 2
    }
  ],
  "method_consumers": [
    {
      "method": "upsert_symbol",
      "consumer_files": ["crates/aetherd/src/indexer.rs", "crates/aetherd/src/sir_pipeline/mod.rs"],
      "consumer_count": 2
    }
  ],
  "uncalled_methods": ["increment_symbol_access_debounced"],
  "suggested_clusters": [
    {
      "cluster_name": "sir_state",
      "methods": ["write_sir_blob", "read_sir_blob", "upsert_sir_meta", "get_sir_meta"],
      "reason": "Always co-consumed by the same files"
    }
  ]
}
```

The `suggested_clusters` field groups methods that are always consumed
together by the same set of files. This is computed by treating each
method's consumer set as a bitvector and grouping methods with identical
(or >80% overlapping) consumer sets. This directly answers "which
methods belong in the same sub-trait?" with data, not judgment.

### Implementation

New file: `crates/aether-mcp/src/tools/usage_matrix.rs`

The tool uses only SQLite queries (symbol_edges table + symbols table).
No graph store, no vector store, no inference. It should work without
any API keys.

## Enhancement: `aether_dependencies` Type-Level Aggregation

When `aether_dependencies` is called with a symbol_id that resolves to
a struct, trait, or enum (not a function/method), aggregate dependency
edges across all child methods.

### Current behavior

```json
// aether_dependencies({ symbol_id: "<SqliteStore id>" })
{
  "found": true,
  "dependency_count": 0,
  "caller_count": 0,
  "dependencies": [],
  "callers": []
}
```

### Target behavior

```json
// aether_dependencies({ symbol_id: "<SqliteStore id>" })
{
  "found": true,
  "aggregated": true,
  "child_method_count": 52,
  "dependency_count": 15,
  "caller_count": 47,
  "dependencies": [
    { "qualified_name": "SirMetaRecord", "edge_kind": "type_ref", "referencing_methods": 4 }
  ],
  "callers": [
    { "qualified_name": "run_full_index_once_inner", "file_path": "crates/aetherd/src/indexer.rs", "methods_called": 6 }
  ]
}
```

When `aggregated: true`, the response includes `child_method_count` and
each caller/dependency entry includes the count of methods involved.
This gives the "how coupled is this consumer to this type?" signal.

### Implementation

Modify `crates/aether-mcp/src/tools/sir.rs` in `aether_dependencies_logic`.
After resolving the symbol, check `symbol.kind`. If it's `struct`, `trait`,
`enum`, or `type_alias`, find child methods (same qualified_name prefix +
file_path logic as usage_matrix), aggregate their edges, and set
`aggregated: true` in the response.

## Enhancement: Embedding Provider Validation on MCP Startup

### Current behavior

MCP server starts silently. First `aether_search` with hybrid mode
returns a raw error: `missing inference API key: GEMINI_API_KEY`.

### Target behavior

On startup, `run_stdio_server` in `crates/aether-mcp/src/lib.rs`:

1. Read `.aether/config.toml` (already done — config is loaded).
2. Check if embeddings are enabled in config.
3. If enabled, check if the required API key env var is set.
4. If not set, log a warning via `tracing::warn!`:
   `"Embedding provider requires {key_env} but it is not set. Semantic search will be unavailable. Register the MCP server with --env {key_env}=<value> to enable it."`
5. Store a `semantic_search_available: bool` flag on `SharedState`.

When `aether_search` is called with hybrid or semantic mode and
`semantic_search_available` is false:

- Fall back to lexical mode automatically.
- Include `fallback_reason: "Embedding API key not configured"` in
  the response (this field already exists in the search response schema).
- Do NOT return an error.

### Implementation

Modify `crates/aether-mcp/src/state.rs` to add validation in
`SharedState::open_readwrite` and `SharedState::open_readonly`.
Modify `crates/aether-mcp/src/tools/search.rs` to check the flag
and degrade gracefully.

## Subsequent Stages

- **8.20:** Method-attributed SIR dependencies — adds `method_dependencies`
  field to SIR for per-method type mappings on traits/structs.
- **8.21:** Trait split planner — uses usage_matrix + method_dependencies
  to suggest trait decompositions via consumer-bitvector clustering.

## Files to Modify

| File | Change |
|------|--------|
| `crates/aether-mcp/src/tools/usage_matrix.rs` | **NEW** — usage matrix tool |
| `crates/aether-mcp/src/tools/mod.rs` | Add `pub mod usage_matrix;` |
| `crates/aether-mcp/src/tools/router.rs` | Register `aether_usage_matrix` tool |
| `crates/aether-mcp/src/tools/sir.rs` | Type-level aggregation in `aether_dependencies_logic` |
| `crates/aether-mcp/src/state.rs` | Add `semantic_search_available` flag + validation |
| `crates/aether-mcp/src/lib.rs` | Thread validation into server startup |
| `crates/aether-mcp/src/tools/search.rs` | Graceful fallback when semantic unavailable |
| `crates/aether-mcp/tests/mcp_tools.rs` | Tests for all three changes |

## Pass Criteria

1. `aether_usage_matrix` for Store returns a matrix with 52 methods and 20+ consumer files.
2. `suggested_clusters` in the matrix response groups co-consumed methods.
3. `aether_dependencies` for SqliteStore returns aggregated edges with `aggregated: true`.
4. `aether_dependencies` for a regular function still returns non-aggregated edges (no regression).
5. MCP server logs a warning on startup when embedding API key is missing.
6. `aether_search` with hybrid mode falls back to lexical when API key is missing, with `fallback_reason` set.
7. `aether_search` with hybrid mode still works normally when API key IS set.
8. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, per-crate tests pass.

## Estimated Effort

1–2 Codex runs. The usage_matrix tool is the largest piece (~200 lines).
The dependency aggregation and search fallback are smaller modifications
to existing code.
