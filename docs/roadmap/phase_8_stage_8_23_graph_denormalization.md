# Phase 8 — Stage 8.23: Graph Read-Path Denormalization

**Status:** Spec Ready  
**Decision:** #103 (Dual-Write Graph Architecture — SQLite Hot Path + SurrealDB Complex Queries)  
**Prerequisite:** None (independent of PR #121/#122 turbo-index work)  
**Estimated effort:** 1 Claude Code session

---

## Problem Statement

SurrealKV takes a process-level exclusive file lock (`fs2`) on `.aether/graph/`. This means:

- The daemon (`aetherd`) holds the lock while running
- CLI commands (`health-score`, `fsck`, `refactor-prep`) open `SurrealGraphStore` directly and fail
- The MCP server's async initialization path (`open_shared_graph_async`) opens `SurrealGraphStore` when `graph_backend = "surreal"`, blocking MCP use during daemon scans/triages
- Current workaround: `pkill -f aetherd` before running CLI commands

The MCP sync path (`open_shared_graph`) already maps all backends to `SqliteGraphStore`, so MCP tools using that path work fine. But two of the four `SharedState` constructors use the async path, which hits the lock.

Deep Think recommended removing SurrealDB entirely (twice). Decision #58 locked a compromise: keep SurrealDB for complex graph queries, add a SQLite denormalized table for the read-hot-path, and ensure all primary read paths go through SQLite.

---

## Solution

### Part A: `symbol_neighbors` Denormalized Table

**Write path (daemon only):**  
During indexing, after the daemon writes edges to `symbol_edges`, it ALSO populates a new `symbol_neighbors` table with pre-resolved neighbor names and file paths, including reverse edges.

**Read path (all consumers):**  
CLI commands, MCP tools, LSP context assembly query `symbol_neighbors` in SQLite. No JOINs needed — names and file paths are denormalized. SQLite WAL mode handles concurrent readers natively.

### Part B: Fix MCP Async Graph Initialization

Modify `open_shared_graph_async` to always return `SqliteGraphStore` as the primary `Arc<dyn GraphStore>` (matching the sync version's behavior). `SurrealGraphStore` becomes an optional secondary that fails gracefully with a warning if the daemon holds the lock.

This means: **MCP tools work while the daemon is scanning or triaging.**

### Schema (Migration v14 → v15)

```sql
CREATE TABLE IF NOT EXISTS symbol_neighbors (
    symbol_id     TEXT NOT NULL,
    neighbor_id   TEXT NOT NULL,
    edge_type     TEXT NOT NULL,      -- 'calls', 'called_by', 'depends_on', 'depended_on_by', etc.
    neighbor_name TEXT NOT NULL,      -- denormalized qualified_name for display
    neighbor_file TEXT NOT NULL,      -- denormalized file_path for display
    PRIMARY KEY (symbol_id, neighbor_id, edge_type)
);

CREATE INDEX IF NOT EXISTS idx_neighbors_symbol ON symbol_neighbors(symbol_id);
CREATE INDEX IF NOT EXISTS idx_neighbors_file ON symbol_neighbors(neighbor_file);
```

### Write-Path Integration

After `upsert_edges` writes to `symbol_edges`, a new function `populate_symbol_neighbors` runs in a single transaction:

1. Cleans stale entries for the file being indexed
2. For each edge, resolves `target_qualified_name` → target symbol via `symbols` table
3. Writes forward edge: `(source_id, target_id, edge_kind, target_name, target_file)`
4. Writes reverse edge: `(target_id, source_id, reverse_kind, source_name, source_file)`

Reverse kind mapping:
- `calls` → `called_by`
- `depends_on` → `depended_on_by`
- `implements` → `implemented_by`
- `type_ref` → `type_ref_by`

### SurrealDB Retained For

- Multi-hop call chains (`aether_call_chain`)
- Community detection
- Coupling analysis
- Dashboard visualizations (daemon process, no contention)

---

## Pass Criteria

1. New `symbol_neighbors` table created via schema migration v15
2. Table populated during indexing after edge upsert (forward + reverse edges)
3. `open_shared_graph_async` always returns `SqliteGraphStore` as primary GraphStore
4. SurrealDB opens as optional secondary, fails gracefully with warning if locked
5. MCP tools work while daemon is running a scan or triage pass
6. `check_compatibility("core", 15)` updated in all 5 locations
7. `cargo fmt --all --check`, `cargo clippy -p aetherd --features dashboard -- -D warnings`, `cargo test -p aetherd`, `cargo test -p aether-store`, `cargo test -p aether-mcp` pass
8. No regression in existing SurrealDB-backed queries (dashboard, call chains)

---

## Non-Goals

- Full SurrealDB removal (out of scope — Decision #58 explicitly retains it)
- Migrating `aether_dependencies` MCP tool to use denormalized table (follow-up PR)
- Migrating `health-score`, `fsck`, `refactor-prep` CLI commands (follow-up PR)
- Multi-hop query migration (call chains stay on SurrealDB)
- Community detection migration (stays on SurrealDB)
