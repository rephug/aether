# Phase 4 - Stage 4.5: Graph Storage via CozoDB

## Purpose
Enable relationship queries over symbol edges from Stage 4.4. The V3.0 Prospectus specified KuzuDB (Decision #2, §3.2) for a Temporal Knowledge Graph, but KuzuDB's repository was **archived October 2025**. After evaluating all viable alternatives, **CozoDB** replaces KuzuDB as the graph storage engine.

## Why CozoDB

KuzuDB (archived) and eight other candidates were evaluated against AETHER's hard requirements: embeddable in-process, Rust-native, actively maintained, permissive license.

| Criterion | CozoDB | KuzuDB (archived) | SurrealDB | HelixDB |
|-----------|--------|--------------------|-----------|---------|
| Embeddable (in-process) | ✅ Same model as SQLite | ✅ | ✅ (heavy) | ❌ Server only |
| Language | Rust | C++ (via FFI) | Rust | Rust |
| Graph algorithms | ✅ Built-in (PageRank, shortest path, community) | ✅ Cypher | ❌ None built-in | ❌ None built-in |
| Vector search | ✅ HNSW (bonus, not used) | ❌ | ✅ | ✅ |
| License | MPL 2.0 | MIT | BSL 1.1 ⚠️ | AGPL ⚠️ |
| Maintenance | Low velocity but active (PRs merged Nov 2025) | ❌ Archived | ✅ Very active | ✅ Active but immature |
| Binary size impact | Moderate (SQLite backend) | Large (~25MB C++ compile) | Very large (full DB engine) | N/A |

**Also eliminated:** Neo4j (Java/JVM server), ArangoDB (C++ server), Nebula (C++ cluster), OrientDB (Java, EOL), Cayley (Go, abandoned), Cognee (Python framework, not a DB).

**Key advantages of CozoDB for AETHER:**
- **Datalog** query language is ideal for recursive graph traversal (call chains, transitive dependencies)
- **Built-in graph algorithms** — no manual implementation of multi-hop traversal via CTEs
- **SQLite backend option** — minimal resource footprint, single-file storage, backup-friendly
- **Rust-native API** — `DbInstance::new("sqlite", path, Default::default())`, no FFI
- **HNSW vector search included** — not used (LanceDB handles vectors) but available as future option

## Current implementation (what we're replacing)
- Stage 4.4 creates `symbol_edges` SQLite table with unresolved `target_qualified_name` strings
- No relationship queries exist — edges are stored but never traversed
- No graph database of any kind in the project

## Target implementation
- CozoDB instance at `.aether/graph.db` (SQLite backend)
- Symbol nodes and edges stored as CozoDB relations
- Datalog queries for callers, dependencies, and multi-hop call chains
- `GraphStore` trait abstracts the backend for future swaps
- MCP tools updated to return resolved symbol relationships

## In scope
- Add `cozo = { version = "0.7", features = ["storage-sqlite", "graph-algo"] }` to workspace deps
- Create `GraphStore` trait in `crates/aether-store`:
  ```rust
  pub trait GraphStore: Send + Sync {
      fn upsert_symbol_node(&self, symbol: &SymbolRecord) -> Result<()>;
      fn upsert_edge(&self, edge: &ResolvedEdge) -> Result<()>;
      fn get_callers(&self, qualified_name: &str) -> Result<Vec<SymbolRecord>>;
      fn get_dependencies(&self, symbol_id: &str) -> Result<Vec<SymbolRecord>>;
      fn get_call_chain(&self, symbol_id: &str, depth: u32) -> Result<Vec<Vec<SymbolRecord>>>;
      fn delete_edges_for_file(&self, file_path: &str) -> Result<()>;
  }
  ```
- Implement `CozoGraphStore` in `crates/aether-store/src/graph_cozo.rs`
- Implement `SqliteGraphStore` (simple JOINs on `symbol_edges` table) as fallback
- Edge resolution: match `target_qualified_name` against `symbols.qualified_name` to produce `ResolvedEdge` with both `source_id` and `target_id`
- Update MCP tool `aether_dependencies` to use resolved edges
- Add MCP tool `aether_call_chain` for transitive traversal
- Config field: `[storage] graph_backend = "cozo" | "sqlite"` (default: `"cozo"`)

## Out of scope
- Commit/Ticket/PR/Author nodes (Phase 5 — need API connectors first)
- Hot/warm/cold graph tiering (§3.2.2 — premature optimization)
- Using CozoDB's HNSW for vector search (LanceDB handles vectors)
- Async blame integration (Phase 5)

## Implementation notes

### CozoDB schema (Datalog relations)

```
# Create symbol node relation
:create symbols {
    symbol_id: String =>
    qualified_name: String,
    name: String,
    kind: String,
    file_path: String,
    language: String
}

# Create edge relation
:create edges {
    source_id: String,
    target_id: String,
    edge_kind: String =>
    file_path: String
}
```

### Key queries

**Get callers (who calls this function?):**
```
?[caller_id, name, kind, file_path] :=
    *edges{source_id: caller_id, target_id: target_id, edge_kind: "calls"},
    *symbols{symbol_id: target_id, qualified_name: $qname},
    *symbols{symbol_id: caller_id, name, kind, file_path}
```

**Get dependencies (what does this function call?):**
```
?[dep_id, name, kind, file_path] :=
    *edges{source_id: $source_id, target_id: dep_id, edge_kind: "calls"},
    *symbols{symbol_id: dep_id, name, kind, file_path}
```

**Get call chain (transitive, depth-limited):**
```
# Recursive Datalog — CozoDB's strength
reachable[node, 1] := *edges{source_id: $start, target_id: node, edge_kind: "calls"}
reachable[node, depth] :=
    reachable[prev, prev_depth],
    prev_depth < $max_depth,
    *edges{source_id: prev, target_id: node, edge_kind: "calls"},
    depth = prev_depth + 1

?[symbol_id, name, kind, file_path, depth] :=
    reachable[symbol_id, depth],
    *symbols{symbol_id, name, kind, file_path}

:order depth
```

### Edge resolution strategy

Stage 4.4 stores edges with `target_qualified_name` (string). This stage resolves them:

1. After Stage 4.4 populates `symbol_edges` in SQLite
2. This stage reads unresolved edges and matches `target_qualified_name` against `symbols.qualified_name`
3. Resolved edges (with actual `symbol_id` for both source and target) are written to CozoDB
4. Unresolved edges (no matching symbol found — likely external dependencies) are logged but not stored in CozoDB

### CozoDB initialization

```rust
use cozo::DbInstance;

let db = DbInstance::new("sqlite", graph_db_path.to_str().unwrap(), Default::default())
    .map_err(|e| anyhow::anyhow!("Failed to create CozoDB instance: {e}"))?;

// Create relations on first run (idempotent — :create errors if exists, use :ensure)
db.run_script(
    ":create symbols { symbol_id: String => qualified_name: String, name: String, kind: String, file_path: String, language: String }",
    Default::default(),
    ScriptMutability::Mutable,
)?;

db.run_script(
    ":create edges { source_id: String, target_id: String, edge_kind: String => file_path: String }",
    Default::default(),
    ScriptMutability::Mutable,
)?;
```

### SQLite fallback (SqliteGraphStore)

If CozoDB is not desired, the SQLite fallback uses:
- JOIN on `symbol_edges.target_qualified_name = symbols.qualified_name` for caller/dependency queries
- Recursive CTE (`WITH RECURSIVE`) for call chain traversal
- Same `GraphStore` trait interface, selected via config

## Pass criteria
1. `get_callers("function_name")` returns resolved `SymbolRecord` values, not just string names.
2. `get_call_chain(id, depth=3)` returns multi-hop traversal results using CozoDB recursive Datalog.
3. `aether_call_chain` MCP tool returns structured JSON with resolved symbols at each depth level.
4. Backend is behind `GraphStore` trait — `cozo` and `sqlite` are both selectable via config.
5. Edge resolution correctly matches `target_qualified_name` to `symbol_id`; unresolved edges are logged.
6. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

Read these files for context first:
- docs/roadmap/phase_4_stage_4_5_graph_storage.md (this file)
- crates/aether-store/src/lib.rs (current Store trait + SqliteStore)
- crates/aether-store/src/schema.rs (symbol_edges table from Stage 4.4)
- crates/aether-mcp/src/lib.rs (MCP tool handlers)
- crates/aether-config/src/lib.rs (config schema)
- Cargo.toml (workspace deps)

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase4-stage4-5-graph off main.
3) Create worktree ../aether-phase4-stage4-5-graph for that branch and switch into it.
4) Add workspace dependency:
   - cozo = { version = "0.7", features = ["storage-sqlite", "graph-algo"] }
5) Create GraphStore trait in crates/aether-store with:
   - upsert_symbol_node, upsert_edge, get_callers, get_dependencies, get_call_chain, delete_edges_for_file
6) Implement CozoGraphStore in crates/aether-store/src/graph_cozo.rs:
   - Initialize CozoDB with SQLite backend at .aether/graph.db
   - Create symbols and edges relations on first run (idempotent)
   - Use Datalog queries per the spec above for traversal
   - Recursive Datalog for call chain (NOT recursive CTE)
7) Implement SqliteGraphStore in crates/aether-store/src/graph_sqlite.rs:
   - Use JOINs on symbol_edges table for callers/dependencies
   - Use recursive CTE for call chain traversal
8) Add edge resolution: read symbol_edges, match target_qualified_name against symbols.qualified_name, write resolved edges to GraphStore.
9) Add config field: [storage] graph_backend = "cozo" | "sqlite" (default: "cozo")
10) Update aether_dependencies MCP tool to return resolved SymbolRecord data.
11) Add aether_call_chain MCP tool:
    - Input: symbol_id (or qualified_name), max_depth (default 3)
    - Output: array of arrays (one per depth level) of SymbolRecord
12) Add tests:
    - Callers query returns correct resolved symbols
    - Dependencies query returns correct resolved symbols
    - Call chain at depth 3 returns multi-hop results
    - Unresolved edges are not stored in graph (logged only)
    - Config toggle between cozo and sqlite backends
13) Run:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test --workspace
14) Commit with message: "Add CozoDB graph storage for symbol relationships".
```

## Expected commit
`Add CozoDB graph storage for symbol relationships`
