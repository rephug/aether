# Phase 4 - Stage 4.4: Dependency Edge Extraction

## Purpose
Extract CALLS and DEPENDS_ON relationships from tree-sitter ASTs. Currently AETHER knows *what* symbols exist but not *how they relate*. This stage adds edge extraction so the system can answer "what calls this function?" and "what does this module depend on?" — prerequisite data for Stage 4.5 (graph storage).

## Current implementation (what's missing)
- `aether-parse` extracts symbols: functions, structs, enums, traits, classes, interfaces, type aliases
- Each symbol gets a stable BLAKE3 ID
- Zero relationship data exists between symbols
- No call graph, no import graph, no dependency edges

## Target implementation
- Tree-sitter AST walker extracts two edge types per file:
  - **CALLS**: function A contains a call expression to function B
  - **DEPENDS_ON**: file/module A imports or uses symbols from file/module B
- Edges stored in a `symbol_edges` SQLite table (lightweight, no graph DB yet)
- Edges rebuilt incrementally per-file (same granularity as symbol extraction)
- MCP tool `aether_dependencies` exposes edge queries

## In scope
- Extend `crates/aether-parse` with edge extraction for Rust and TypeScript:
  - Rust: `call_expression` nodes → resolve callee name, `use_declaration` → imports
  - TypeScript: `call_expression`, `new_expression`, `import_declaration`
- Add `SymbolEdge` type to `crates/aether-core`:
  ```rust
  pub struct SymbolEdge {
      pub source_id: String,       // caller / importer
      pub target_qualified_name: String,  // callee / imported name
      pub edge_kind: EdgeKind,     // Calls | DependsOn
      pub file_path: String,
  }
  pub enum EdgeKind { Calls, DependsOn }
  ```
- Add `symbol_edges` table to `crates/aether-store` SQLite schema
- Add `Store` trait methods: `upsert_edges`, `get_callers`, `get_dependencies`, `delete_edges_for_file`
- Add MCP tool `aether_dependencies` in `crates/aether-mcp`
- Update `crates/aetherd/src/sir_pipeline.rs` to extract and store edges during indexing

## Out of scope
- Cross-file call resolution (matching callee names to actual symbol IDs across files) — that's a graph DB query in Stage 4.5
- Dynamic dispatch / trait resolution
- Full import path resolution (just capture the import string for now)
- KuzuDB or any graph database

## Implementation notes

### Edge extraction approach
Edges are "best-effort unresolved" in this stage. We capture:
- The source symbol ID (the function containing the call)
- The target qualified name as a string (what's being called, as written in source)
- Edge kind

Resolution (matching `target_qualified_name` to an actual `symbol_id`) happens in Stage 4.5 via graph queries or SQLite joins.

### Rust call extraction
```
tree-sitter node type: call_expression
  child[0] = identifier | field_expression | scoped_identifier
  → extract the callee name text
```

### TypeScript call extraction
```
tree-sitter node type: call_expression | new_expression
  child "function" = identifier | member_expression
  → extract the callee name text

tree-sitter node type: import_declaration
  child "source" = string literal
  → extract module path
```

### SQLite schema
```sql
CREATE TABLE IF NOT EXISTS symbol_edges (
    source_id TEXT NOT NULL,
    target_qualified_name TEXT NOT NULL,
    edge_kind TEXT NOT NULL CHECK (edge_kind IN ('calls', 'depends_on')),
    file_path TEXT NOT NULL,
    PRIMARY KEY (source_id, target_qualified_name, edge_kind)
);
CREATE INDEX idx_edges_target ON symbol_edges(target_qualified_name);
CREATE INDEX idx_edges_file ON symbol_edges(file_path);
```

## Pass criteria
1. Indexing a Rust file with function calls produces CALLS edges in `symbol_edges`.
2. Indexing a TypeScript file with imports produces DEPENDS_ON edges.
3. `get_callers("function_name")` returns correct source symbol IDs.
4. Re-indexing a file replaces old edges for that file (no stale edges).
5. MCP tool `aether_dependencies` returns edges for a given symbol.
6. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_4_stage_4_4_dependency_extraction.md for full spec.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase4-stage4-4-edges off main.
3) Create worktree ../aether-phase4-stage4-4-edges for that branch and switch into it.
4) Add SymbolEdge and EdgeKind types to crates/aether-core.
5) Extend crates/aether-parse to extract CALLS and DEPENDS_ON edges from:
   - Rust: call_expression, use_declaration
   - TypeScript: call_expression, new_expression, import_declaration
6) Add symbol_edges table to crates/aether-store with upsert/query/delete methods.
7) Update crates/aetherd/src/sir_pipeline.rs to store edges during indexing.
8) Add aether_dependencies MCP tool in crates/aether-mcp.
9) Add tests:
   - Rust file with fn calls → correct CALLS edges
   - TS file with imports → correct DEPENDS_ON edges
   - Re-index replaces old edges
   - MCP tool returns structured edge data
10) Run:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test --workspace
11) Commit with message: "Extract dependency edges from tree-sitter AST".
```

## Expected commit
`Extract dependency edges from tree-sitter AST`
