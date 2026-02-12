# Decision Register v2 — Phase 4 Build Packet

Extends the original Phase 1 Decision Register. These decisions are "locked" for Phase 4 to reduce scope churn and help Codex implement confidently.

---

## Inherited decisions (unchanged from Phase 1)

| # | Decision | Status |
|---|----------|--------|
| 1 | Phase 1 focus = Observer | ✅ Complete |
| 2 | Local-first architecture (.aether/) | ✅ Active |
| 3 | LSP-first integration | ✅ Active |
| 4 | Cloud-first inference (Gemini Flash) | ✅ Active |
| 5 | API embeddings by default | ✅ Active |
| 6 | Reranking not enabled by default | ✅ Active |
| 7 | tree-sitter for symbol extraction | ✅ Active |
| 8 | Stable BLAKE3 symbol IDs | ✅ Active |
| 9 | Incremental updates only | ✅ Active |
| 10 | SIR stored as JSON blobs + SQLite metadata | ✅ Active |
| 13 | Linux primary | ✅ Active |
| 14 | Windows supported, no Ghost sandbox | ✅ Active |
| 16 | Strict SIR schema validation | ✅ Active |
| 17 | Cost and rate limiting mandatory | ✅ Active |

---

## Updated decisions (changed for Phase 4)

### 11. Vector storage → LanceDB (updated)
**Original:** "Use LanceDB for vector embeddings and ANN search (local embedded)."
**Phase 4 update:** LanceDB replaces SQLite `sir_embeddings` table. The `VectorStore` trait abstracts the backend. SQLite brute-force remains as a fallback via config toggle.
- Crate: `lancedb = "0.23"`
- Storage: `.aether/vectors/`
- Config: `[embeddings] vector_backend = "lancedb" | "sqlite"` (default: `lancedb`)
- Migration: automatic on first run with lancedb backend

### 12. Graph storage → CozoDB (updated)
**Original:** "Not required in Phase 1 (Historian/graph engine is Phase 2)."
**Phase 4 update:** KuzuDB (Prospectus Decision #2) was archived Oct 2025. After evaluating 9 alternatives (SurrealDB, HelixDB, Cayley, Neo4j, ArangoDB, Cognee, Nebula, OrientDB, CozoDB), **CozoDB** was selected as the replacement.
- Crate: `cozo = { version = "0.7", features = ["storage-sqlite", "graph-algo"] }`
- Storage: `.aether/graph.db` (CozoDB with SQLite backend)
- Query: Datalog (recursive, with built-in graph algorithms)
- Config: `[storage] graph_backend = "cozo" | "sqlite"` (default: `cozo`)
- License: MPL 2.0
- SQLite fallback remains via `GraphStore` trait for environments where CozoDB is undesirable

### 15. Typed event bus → Deferred to Phase 5 (updated)
**Original:** "Engines communicate through typed events inside aetherd."
**Phase 4 update:** The synchronous pipeline (file change → parse → diff → SIR → store) works. An event bus adds complexity without clear benefit at current scale. Deferred to Phase 5 when additional engines require async coordination.

---

## New decisions (Phase 4)

### 18. Structured logging via `tracing`
All `eprintln!` calls are replaced with `tracing` macros. Subscriber initialized in `aetherd` main.
- Crates: `tracing = "0.1"`, `tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }`
- Config: `[general] log_level = "info"` (overridden by `RUST_LOG`)
- CLI: `--log-format human | json`

### 19. Native git via `gix`
All `std::process::Command("git")` calls are replaced with `gix` library.
- Crate: `gix = { version = "0.76", default-features = false, features = ["max-performance-safe"] }`
- API: `GitContext` struct in `aether-core/src/git.rs`
- Graceful degradation: non-git workspaces return `None`, not crash

### 20. Dependency edges extracted from AST
tree-sitter AST walking extracts CALLS and DEPENDS_ON edges alongside symbol extraction.
- Types: `SymbolEdge { source_id, target_qualified_name, edge_kind, file_path }`
- Storage: `symbol_edges` SQLite table (lightweight, no graph DB required)
- Resolution: edges are "unresolved" strings until Stage 4.5 resolves them against symbol IDs

### 21. SIR hierarchy levels
Three SIR levels: Leaf (existing), File (new), Module (new).
- File SIR: deterministic aggregation of leaf SIR for a file
- Module SIR: on-demand aggregation of file SIR for a directory
- Synthetic IDs: `BLAKE3("file:" + language + ":" + path)`, `BLAKE3("module:" + language + ":" + dir_path)`
- MCP: `aether_get_sir` gains optional `level` parameter

### 22. Trait-based backend abstraction
All new backends (`VectorStore`, `GraphStore`) are behind traits to allow swapping.
- Implementations selected via config at startup
- Tests run against all implementations
- Default: LanceDB for vectors, CozoDB for graphs; SQLite fallback for both

### 23. CozoDB replaces KuzuDB (Prospectus Decision #2 superseded)
KuzuDB (Prospectus Decision #2) was archived Oct 2025. CozoDB was selected after evaluating 9 alternatives:
- **Eliminated (architecture mismatch):** Neo4j (Java server), ArangoDB (C++ server), Nebula (C++ cluster), OrientDB (Java, EOL), Cayley (Go, abandoned), Cognee (Python framework)
- **Eliminated (license/maturity):** SurrealDB (BSL 1.1 license, massive binary), HelixDB (AGPL, server-only, too immature)
- **Selected:** CozoDB — Rust-native, embeddable, MPL 2.0, Datalog queries, built-in graph algorithms
- **Risk acknowledged:** CozoDB's maintenance velocity is low (v0.7.2 last tagged release, but PRs still merged). The `GraphStore` trait provides a clean exit path if needed.
- **Vector search remains with LanceDB** — CozoDB's built-in HNSW is a bonus but not used

---

## Decision principles for Phase 4

1. **Trait-first:** Every new backend goes behind a trait. No concrete dependency leaks into callers.
2. **Config-switchable:** Every backend choice is a config toggle, not a compile-time flag.
3. **Migration-safe:** Old data is always readable. New backends migrate on first run.
4. **Test-both:** When two backends exist for one trait, tests run against both.
5. **Scope-strict:** Codex prompts enumerate exactly which files to modify. No unrelated refactors.
