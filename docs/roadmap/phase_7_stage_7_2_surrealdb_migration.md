# Phase 7 — The Pathfinder

## Stage 7.2 — SurrealDB Graph Migration (Revised — replaces RocksDB Migration)

### Purpose

Replace CozoDB/sled with SurrealDB 3.0/SurrealKV as the graph storage engine. This is the core technology swap that eliminates the concurrent access problem, the `links` conflict, and the CozoDB maintenance risk — all in one stage. The `GraphStore` trait (built specifically as an exit path) makes this a clean implementation swap.

**This stage replaces the original 7.2 (RocksDB migration).** The original existed solely to work around sled's exclusive file lock. SurrealDB's SurrealKV backend has MVCC with concurrent readers and writers — no workaround needed.

### What This Replaces and Why

| Problem | Original 7.2 (RocksDB) | Revised 7.2 (SurrealDB) |
|---------|------------------------|------------------------|
| Concurrent access | RocksDB multi-reader | SurrealKV MVCC (readers + writers) |
| `links` conflict | Still present (CozoDB kept) | **Gone** (CozoDB removed) |
| Maintenance risk | Still present (CozoDB kept) | **Gone** (SurrealDB active) |
| C++ build dependency | Yes (rocksdb-sys) | **No** (pure Rust SurrealKV) |
| Two-backend testing | sled + RocksDB matrix | **None** — single backend |
| Feature flags for backends | Required | **Not needed** |
| Graph algorithms | CozoDB built-in | Must reimplement (~500 LOC) |

**Net assessment:** More work upfront (query rewrite + algorithm reimplementation), but eliminates three ongoing problems permanently. The graph algorithm gap is bounded and well-understood.

### Engineering Design

#### Dependency Changes

```toml
# Cargo.toml (workspace) — REMOVE
[workspace.dependencies]
cozo = { version = "0.7", default-features = false, features = ["storage-sled", "graph-algo"] }

# Cargo.toml (workspace) — ADD
[workspace.dependencies]
surrealdb = { version = "3", features = ["kv-surrealkv"] }
```

```toml
# crates/aether-store/Cargo.toml — REMOVE
cozo = { workspace = true }

# crates/aether-store/Cargo.toml — ADD
surrealdb = { workspace = true }
```

```toml
# crates/aether-analysis/Cargo.toml — ADD
petgraph = "0.6"
```

**Impact:** Removes C++ build dependency entirely (sled is pure Rust, SurrealKV is pure Rust). No more `links = "sqlite3"` conflict.

#### New File: `graph_surreal.rs`

```rust
// crates/aether-store/src/graph_surreal.rs (NEW — replaces graph_cozo.rs)

use surrealdb::engine::local::{Db, SurrealKV};
use surrealdb::Surreal;
use std::sync::Arc;

pub struct SurrealGraphStore {
    db: Surreal<Db>,
}

impl SurrealGraphStore {
    pub async fn open(workspace_root: &Path) -> Result<Self, StoreError> {
        let graph_dir = workspace_root.join(".aether").join("graph");
        fs::create_dir_all(&graph_dir)?;
        let db = Surreal::new::<SurrealKV>(&graph_dir).await
            .map_err(|e| StoreError::Graph(format!("SurrealDB open failed: {e}")))?;
        db.use_ns("aether").use_db("graph").await
            .map_err(|e| StoreError::Graph(format!("SurrealDB namespace setup: {e}")))?;
        let store = Self { db };
        store.ensure_schema().await?;
        Ok(store)
    }

    pub async fn open_readonly(workspace_root: &Path) -> Result<Self, StoreError> {
        // SurrealKV supports concurrent access — open normally.
        // Read-only enforcement is at the SharedState level (require_writable guard).
        Self::open(workspace_root).await
    }

    async fn ensure_schema(&self) -> Result<(), StoreError> {
        // Define tables and fields — SurrealDB schema definitions
        self.db.query("
            -- Symbol nodes
            DEFINE TABLE IF NOT EXISTS symbol SCHEMAFULL;
            DEFINE FIELD IF NOT EXISTS symbol_id ON symbol TYPE string;
            DEFINE FIELD IF NOT EXISTS name ON symbol TYPE string;
            DEFINE FIELD IF NOT EXISTS kind ON symbol TYPE string;
            DEFINE FIELD IF NOT EXISTS file_path ON symbol TYPE string;
            DEFINE FIELD IF NOT EXISTS language ON symbol TYPE string;
            DEFINE FIELD IF NOT EXISTS updated_at ON symbol TYPE datetime DEFAULT time::now();
            DEFINE INDEX IF NOT EXISTS idx_symbol_id ON symbol FIELDS symbol_id UNIQUE;

            -- Dependency edges (CALLS, DEPENDS_ON, IMPORTS)
            DEFINE TABLE IF NOT EXISTS depends_on SCHEMAFULL TYPE RELATION
                FROM symbol TO symbol;
            DEFINE FIELD IF NOT EXISTS kind ON depends_on TYPE string;
            DEFINE FIELD IF NOT EXISTS weight ON depends_on TYPE float DEFAULT 1.0;

            -- Co-change coupling edges (Phase 6.2)
            DEFINE TABLE IF NOT EXISTS co_change SCHEMAFULL TYPE RELATION
                FROM symbol TO symbol;
            DEFINE FIELD IF NOT EXISTS coupling_score ON co_change TYPE float;
            DEFINE FIELD IF NOT EXISTS shared_commits ON co_change TYPE int;
            DEFINE FIELD IF NOT EXISTS last_observed ON co_change TYPE datetime;

            -- Test coverage edges (Phase 6.3)
            DEFINE TABLE IF NOT EXISTS tested_by SCHEMAFULL TYPE RELATION
                FROM symbol TO symbol;
            DEFINE FIELD IF NOT EXISTS confidence ON tested_by TYPE float;

            -- Community detection snapshots (Phase 6.6)
            DEFINE TABLE IF NOT EXISTS community_snapshot SCHEMAFULL;
            DEFINE FIELD IF NOT EXISTS snapshot_id ON community_snapshot TYPE string;
            DEFINE FIELD IF NOT EXISTS community_id ON community_snapshot TYPE int;
            DEFINE FIELD IF NOT EXISTS members ON community_snapshot TYPE array;
            DEFINE FIELD IF NOT EXISTS created_at ON community_snapshot TYPE datetime DEFAULT time::now();

            -- Record References for bidirectional traversal (Decision #42)
            DEFINE FIELD IF NOT EXISTS in ON depends_on TYPE record<symbol> REFERENCE;
            DEFINE FIELD IF NOT EXISTS out ON depends_on TYPE record<symbol> REFERENCE;

            -- Computed fields for derived properties (Decision #43)
            DEFINE FIELD IF NOT EXISTS callers ON symbol COMPUTED <~depends_on;
            DEFINE FIELD IF NOT EXISTS dependees ON symbol COMPUTED ->depends_on->symbol;
        ").await.map_err(|e| StoreError::Graph(format!("Schema setup: {e}")))?;

        Ok(())
    }
}
```

#### GraphStore Trait Implementation

The existing `GraphStore` trait methods map to SurrealQL queries:

```rust
#[async_trait]
impl GraphStore for SurrealGraphStore {
    async fn upsert_symbol(&self, symbol: &GraphSymbol) -> Result<(), StoreError> {
        self.db.query("
            UPSERT symbol SET
                symbol_id = $symbol_id,
                name = $name,
                kind = $kind,
                file_path = $file_path,
                language = $language,
                updated_at = time::now()
            WHERE symbol_id = $symbol_id
        ")
        .bind(("symbol_id", &symbol.symbol_id))
        .bind(("name", &symbol.name))
        .bind(("kind", &symbol.kind))
        .bind(("file_path", &symbol.file_path))
        .bind(("language", &symbol.language))
        .await?;
        Ok(())
    }

    async fn add_edge(&self, from: &str, to: &str, kind: EdgeKind) -> Result<(), StoreError> {
        // Use RELATE for graph edges
        self.db.query("
            LET $from = (SELECT id FROM symbol WHERE symbol_id = $from_id)[0].id;
            LET $to = (SELECT id FROM symbol WHERE symbol_id = $to_id)[0].id;
            RELATE $from->depends_on->$to SET kind = $kind, weight = 1.0;
        ")
        .bind(("from_id", from))
        .bind(("to_id", to))
        .bind(("kind", kind.as_str()))
        .await?;
        Ok(())
    }

    async fn get_callers(&self, symbol_id: &str) -> Result<Vec<GraphSymbol>, StoreError> {
        // Reverse traversal: who depends on this symbol?
        let results: Vec<GraphSymbol> = self.db.query("
            SELECT <-depends_on<-symbol.* AS callers
            FROM symbol WHERE symbol_id = $symbol_id
        ")
        .bind(("symbol_id", symbol_id))
        .await?
        .take("callers")?;
        Ok(results)
    }

    async fn get_dependencies(&self, symbol_id: &str) -> Result<Vec<GraphSymbol>, StoreError> {
        // Forward traversal: what does this symbol depend on?
        let results: Vec<GraphSymbol> = self.db.query("
            SELECT ->depends_on->symbol.* AS deps
            FROM symbol WHERE symbol_id = $symbol_id
        ")
        .bind(("symbol_id", symbol_id))
        .await?
        .take("deps")?;
        Ok(results)
    }

    async fn shortest_path(&self, from: &str, to: &str) -> Result<Vec<String>, StoreError> {
        // Application-level BFS — see Graph Algorithms section
        graph_algorithms::bfs_shortest_path(&self.db, from, to).await
    }

    // ... remaining trait methods follow same pattern
}
```

#### Datalog → SurrealQL Query Migration

Every CozoDB Datalog query has a SurrealQL equivalent. Key translations:

| CozoDB Datalog | SurrealQL |
|---|---|
| `?[from, to] := edges[from, to, "CALLS"]` | `SELECT in, out FROM depends_on WHERE kind = "CALLS"` |
| `?[sym] := symbols[sym, name, kind, _], kind == "function"` | `SELECT * FROM symbol WHERE kind = "function"` |
| `?[from, to] := edges[from, mid, _], edges[mid, to, _]` | `SELECT ->depends_on->symbol->depends_on->symbol FROM symbol:x` |
| `?[node, degree] := edges[node, _, _], degree = count(node)` | `SELECT symbol_id, count(->depends_on) AS degree FROM symbol` |
| `shortest_path(from, to, edges)` | Application-level BFS (see below) |
| `page_rank(edges)` | Application-level PageRank (see below) |
| `community_detection(edges)` | Application-level Louvain (see below) |

**Full query mapping file:** Create `docs/migration/datalog_to_surql.md` listing every CozoDB query in the codebase and its SurrealQL equivalent. This serves as both a migration guide and a test oracle.

#### Graph Algorithms (Rust Application-Level)

CozoDB provides built-in `page_rank`, `community_detection`, and `shortest_path`. SurrealDB does not. These are reimplemented in Rust:

```rust
// crates/aether-analysis/src/graph_algorithms.rs (NEW)

/// BFS shortest path between two symbols
/// ~100 LOC
pub async fn bfs_shortest_path(
    db: &Surreal<Db>,
    from_id: &str,
    to_id: &str,
) -> Result<Vec<String>, StoreError> {
    // 1. Fetch adjacency from SurrealDB: SELECT ->depends_on->symbol.symbol_id FROM symbol:from
    // 2. Standard BFS with visited set
    // 3. Return path as vec of symbol_ids
    // Early termination if to_id found
    // Max depth limit (configurable, default 10) to prevent runaway on cyclic graphs
}

/// PageRank computation over the dependency graph
/// ~100 LOC
pub async fn page_rank(
    db: &Surreal<Db>,
    damping: f64,       // default 0.85
    iterations: usize,  // default 20
) -> Result<HashMap<String, f64>, StoreError> {
    // 1. Fetch all edges: SELECT in.symbol_id, out.symbol_id FROM depends_on
    // 2. Build adjacency map in memory
    // 3. Standard iterative PageRank
    // 4. Return symbol_id → rank score
}

/// Louvain community detection
/// ~200 LOC
pub async fn louvain_communities(
    db: &Surreal<Db>,
) -> Result<HashMap<String, usize>, StoreError> {
    // 1. Fetch all edges with weights: SELECT in.symbol_id, out.symbol_id, weight FROM depends_on
    // 2. Build weighted adjacency map
    // 3. Standard Louvain modularity optimization
    // 4. Return symbol_id → community_id
}
```

**Why application-level is acceptable:**
- AETHER's graph is modest (~5K-20K nodes for typical projects, ~50K for large monorepos)
- PageRank on 50K nodes with 20 iterations takes <100ms in Rust
- Louvain on 50K nodes takes <500ms
- BFS with max depth 10 is sub-millisecond
- These algorithms are called on-demand (not hot path), typically for analysis reports
- Future: port to Surrealism WASM extensions for near-data execution (optimization, not Phase 7)

#### Migration Script

```rust
// crates/aether-store/src/graph_migrate.rs (REVISED — CozoDB → SurrealDB)

pub struct MigrationResult {
    pub relations_migrated: Vec<String>,
    pub total_rows: u64,
    pub duration_ms: u64,
    pub source: String,       // "cozo/sled"
    pub target: String,       // "surreal/surrealkv"
    pub source_size_bytes: u64,
    pub target_size_bytes: u64,
}

pub async fn migrate_cozo_to_surreal(
    workspace_root: &Path,
    dry_run: bool,
) -> Result<MigrationResult, StoreError> {
    // 1. Check aetherd is not running (PID file check)
    // 2. Open source CozoDB/sled instance (read-only)
    // 3. List all CozoDB relations:
    //    - symbols, edges, co_change_edges, tested_by, community_snapshots
    // 4. Export each relation via CozoDB export_relations()
    // 5. If dry_run: print what would happen and return
    // 6. Create new SurrealDB instance at .aether/graph.migrating/
    // 7. Ensure SurrealDB schema (ensure_schema)
    // 8. Transform and import:
    //    - symbols → symbol (UPSERT)
    //    - edges → depends_on (RELATE)
    //    - co_change_edges → co_change (RELATE)
    //    - tested_by → tested_by (RELATE)
    //    - community_snapshots → community_snapshot (CREATE)
    // 9. Verify row counts match
    // 10. Atomic swap:
    //     rename .aether/graph.db/ → .aether/graph.db.backup.{timestamp}/
    //     rename .aether/graph.migrating/ → .aether/graph/
    // 11. Update config: storage.graph_backend = "surreal"
    //     Remove storage.graph_cozo_backend (no longer relevant)
    // 12. Print summary
}
```

**Data transformation notes:**
- CozoDB stores edges as tuples: `(from_id, to_id, kind, weight)` → SurrealDB: `RELATE symbol:from->depends_on->symbol:to SET kind = $kind`
- CozoDB symbol IDs are strings → SurrealDB record IDs: `symbol:⟨symbol_id⟩` (using record syntax with angle brackets for BLAKE3 hashes)
- CozoDB timestamps (epoch integers) → SurrealDB datetime type

#### Config Changes

```toml
# aether.toml — BEFORE (current)
[storage]
graph_backend = "cozo"
graph_cozo_backend = "sled"

# aether.toml — AFTER (this stage)
[storage]
graph_backend = "surreal"       # "surreal" (new default) | "cozo" (legacy, read-only for migration)
```

**Backward compatibility:** If config says `graph_backend = "cozo"`, aetherd prints:
```
Warning: CozoDB backend is deprecated. Run 'aether graph-migrate' to migrate to SurrealDB.
aetherd will continue with CozoDB for this session.
```

After migration, `graph_backend = "surreal"` is the default. New projects created after this stage use SurrealDB automatically.

### Storage Layout Change

```
# BEFORE (CozoDB/sled)
.aether/
├── aether.db           # SQLite
├── graph.db/           # CozoDB/sled directory
│   ├── db              # sled data
│   ├── conf            # sled config
│   └── blobs/          # sled blobs
└── lance/              # LanceDB

# AFTER (SurrealDB/SurrealKV)
.aether/
├── aether.db           # SQLite (unchanged)
├── graph/              # SurrealDB/SurrealKV directory
│   └── ...             # SurrealKV internal files
└── lance/              # LanceDB (unchanged)
```

### File Paths

| Path | Action | Description |
|------|--------|-------------|
| `Cargo.toml` (workspace) | **Modify** | Remove cozo, add surrealdb |
| `crates/aether-store/Cargo.toml` | **Modify** | Remove cozo, add surrealdb |
| `crates/aether-store/src/graph_surreal.rs` | **Create** | SurrealGraphStore implementation |
| `crates/aether-store/src/graph_cozo.rs` | **Deprecate** | Keep for migration, gate behind `legacy-cozo` feature |
| `crates/aether-store/src/graph_migrate.rs` | **Rewrite** | CozoDB → SurrealDB migration |
| `crates/aether-store/src/lib.rs` | **Modify** | Update open_graph_store() to return SurrealGraphStore |
| `crates/aether-analysis/src/graph_algorithms.rs` | **Create** | PageRank, Louvain, BFS |
| `crates/aether-analysis/src/lib.rs` | **Modify** | Wire graph algorithms |
| `crates/aether-config/src/lib.rs` | **Modify** | Add "surreal" as graph_backend option |
| `crates/aetherd/src/cli.rs` | **Modify** | Update graph-migrate command for CozoDB→SurrealDB |
| `docs/migration/datalog_to_surql.md` | **Create** | Query mapping reference |

### SurrealDB 3.0 Features Used

| Feature | AETHER Usage |
|---------|-------------|
| SurrealKV embedded | Local `.aether/graph/` storage — no server process |
| SCHEMAFULL tables | Strict schemas for symbols, edges, snapshots |
| TYPE RELATION | First-class edge tables (depends_on, co_change, tested_by) |
| RELATE statement | Creating typed edges between symbols |
| `→` / `←` traversal | Forward/reverse graph queries |
| REFERENCE keyword | Bidirectional links (Decision #42) — `<~` reverse lookup |
| COMPUTED fields | Derived properties (Decision #43) — callers, dependees |
| UPSERT | Idempotent symbol updates |
| Parameterized queries | All queries use `$bindings` — no SQL injection |
| MVCC (SurrealKV) | Concurrent reads from aether-query while aetherd writes |

### SurrealDB 3.0 Features NOT Used (Phase 7)

| Feature | Why Deferred |
|---------|-------------|
| DEFINE API | Useful for Team Tier but adds complexity; Axum API is already planned |
| Surrealism WASM | Future path for graph algorithms; Rust app-level is sufficient for Phase 7 |
| File support (DEFINE BUCKET) | Future vertical optimization; legal/finance docs stay in SQLite for now |
| HNSW vector search | LanceDB handles vectors with disk-backed HNSW; SurrealDB HNSW is in-memory only |
| Time-travel (VERSION) | Future Phase 8+ for temporal queries; current approach uses explicit version tables |
| Built-in auth | Future Team Tier; Phase 7 uses bearer token at Axum level |
| GraphQL | MCP is AETHER's primary interface; GraphQL adds no value for AI agent consumers |
| Client-side transactions | Single-process embedded mode doesn't need external transaction control |
| Surqlize ORM | Rust SDK is the interface; TypeScript ORM not relevant |

### Edge Cases

| Scenario | Behavior |
|----------|----------|
| Config says "cozo" (not migrated) | Warning at startup, continues with CozoDB. `aether graph-migrate` to switch. |
| Migration while aetherd running | Check PID file. Refuse with error: "Stop aetherd before migrating." |
| Migration interrupted | `.aether/graph.migrating/` exists → detect on next attempt, clean up, retry. |
| Row count mismatch after migration | Abort. Keep backup at `.aether/graph.db.backup.{timestamp}/`. Report discrepancy. |
| Empty graph (no data yet) | Migration trivially succeeds. New projects use SurrealDB by default. |
| CozoDB relation has data not in schema | Log warning, skip unknown relations. Migration continues for known relations. |
| SurrealDB fails to open (corrupt SurrealKV) | Clear error. Backup exists for recovery. |
| User wants to revert to CozoDB | `.aether/graph.db.backup.{timestamp}/` preserved. Manual revert documented. |

### Pass Criteria

1. **SurrealGraphStore implements all GraphStore trait methods** with correct SurrealQL queries.
2. All existing MCP tools produce identical results with SurrealDB backend as they did with CozoDB.
3. `aether graph-migrate` successfully migrates AETHER's own graph from CozoDB to SurrealDB with zero data loss.
4. **Concurrent access verified:** Open two `SurrealGraphStore` instances against the same `.aether/graph/` directory — both succeed (one writes, one reads).
5. Graph algorithms produce equivalent results to CozoDB built-ins:
   - PageRank scores rank symbols in same relative order
   - Community detection produces clusters of similar quality (exact membership may differ — non-deterministic algorithm)
   - BFS shortest path finds same-length paths
6. `docs/migration/datalog_to_surql.md` covers every CozoDB query in the codebase.
7. New projects (`aether init`) create SurrealDB graph by default.
8. `graph_backend = "cozo"` in config still works (backward compatibility with deprecation warning).
9. Validation gates:
   ```
   cargo fmt --all --check
   cargo clippy --workspace -- -D warnings
   cargo test -p aether-core
   cargo test -p aether-config
   cargo test -p aether-store
   cargo test -p aether-memory
   cargo test -p aether-analysis
   cargo test -p aether-mcp
   cargo test -p aetherd
   ```

### Exact Codex Prompt

```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- export CARGO_TARGET_DIR=/home/rephu/aether-target
- export CARGO_BUILD_JOBS=1
- export PROTOC=$(which protoc)
- export TMPDIR=/home/rephu/aether-target/tmp
- mkdir -p $TMPDIR
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.
- The repo uses mold linker via .cargo/config.toml — ensure mold and clang are installed.

CONTEXT: This stage replaces CozoDB/sled with SurrealDB 3.0/SurrealKV as the graph
storage engine. CozoDB is being replaced because: (1) sled exclusive lock prevents
multi-process access needed for aether-query, (2) CozoDB's storage-sqlite conflicts
with rusqlite via links="sqlite3", (3) CozoDB maintenance is sparse.

SurrealDB 3.0 provides: embedded SurrealKV backend (pure Rust, MVCC, concurrent
readers+writers), first-class graph relations (RELATE, →/← traversal), Record
References (bidirectional links), Computed Fields, and SCHEMAFULL tables.

The GraphStore trait was designed as an exit path for exactly this scenario.

CRITICAL: CozoDB provides built-in PageRank, community detection, and shortest path.
SurrealDB does NOT. You must reimplement these in Rust application code:
- PageRank: ~100 LOC iterative algorithm
- Louvain community detection: ~200 LOC modularity optimization
- BFS shortest path: ~100 LOC with max depth limit

READ THESE FILES FIRST:
- crates/aether-store/src/lib.rs (GraphStore trait — THIS IS YOUR CONTRACT)
- crates/aether-store/src/graph_cozo.rs (current implementation — port every method)
- crates/aether-analysis/src/lib.rs (where graph algorithms are called)
- crates/aether-mcp/src/lib.rs (MCP tools that use GraphStore)
- crates/aether-config/src/lib.rs (config schema)
- docs/roadmap/phase_7_stage_7_2_surrealdb_migration.md (this spec)
- DECISIONS_v4.md (decisions #38, #42, #43)

You are working in the repo root at /home/rephu/projects/aether.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase7-stage7-2-surrealdb-migration off main.
3) Create worktree ../aether-phase7-stage7-2 for that branch and switch into it.

4) Dependency changes:
   a) In workspace Cargo.toml: remove cozo dependency, add surrealdb = { version = "3", features = ["kv-surrealkv"] }
   b) In crates/aether-store/Cargo.toml: remove cozo, add surrealdb workspace dep
   c) Keep cozo behind optional feature "legacy-cozo" for migration only

5) Create crates/aether-store/src/graph_surreal.rs:
   a) SurrealGraphStore struct wrapping Surreal<Db>
   b) open() and open_readonly() constructors
   c) ensure_schema() — define all tables, fields, indexes, references, computed fields
   d) Implement EVERY method of the GraphStore trait using SurrealQL queries
   e) Use parameterized queries everywhere ($bindings, no string interpolation)
   f) RELATE for edges, → and ← for traversal, UPSERT for symbols

6) Create docs/migration/datalog_to_surql.md:
   a) List EVERY CozoDB Datalog query in graph_cozo.rs
   b) Provide the equivalent SurrealQL query
   c) This is both a migration guide and a test oracle

7) Create crates/aether-analysis/src/graph_algorithms.rs:
   a) Add petgraph = "0.6" to crates/aether-analysis/Cargo.toml
   b) bfs_shortest_path(db, from_id, to_id) → Vec<String>
   c) page_rank(db, damping, iterations) → HashMap<String, f64>
   d) louvain_communities(db) → HashMap<String, usize>
   e) All algorithms: first fetch edges from SurrealDB via async queries, then
      dump into a petgraph::DiGraph for in-memory computation. Use petgraph's
      node indexing and adjacency iteration — do NOT hand-roll adjacency lists.
      petgraph does NOT include PageRank or Louvain, so implement those on top
      of petgraph's data structures.
   f) CRITICAL ASYNC SAFETY: All CPU-bound graph computation (the iterative
      PageRank loop, Louvain modularity optimization, BFS traversal) MUST
      execute inside tokio::task::spawn_blocking to avoid starving the async
      reactor. Pattern:
        pub async fn page_rank(db: &Surreal<Db>, ...) -> Result<...> {
            let edges = fetch_all_edges(db).await?;  // async: SurrealDB query
            tokio::task::spawn_blocking(move || {
                compute_page_rank_sync(&edges, damping, iterations)  // sync: CPU math
            }).await.map_err(|e| StoreError::Graph(format!("spawn_blocking: {e}")))?
        }

8) Modify crates/aether-store/src/graph_migrate.rs:
   a) Rewrite migrate function: CozoDB → SurrealDB (not sled→RocksDB)
   b) PID file check, dry_run support, atomic swap with backup
   c) Transform CozoDB tuples to SurrealDB RELATE/UPSERT statements

9) Update open_graph_store() in lib.rs to return SurrealGraphStore by default.
   Keep open_graph_store_cozo() behind "legacy-cozo" feature for migration.

10) Update config: add "surreal" as graph_backend option, make it default.
    "cozo" produces deprecation warning.

11) Update crates/aetherd/src/cli.rs: graph-migrate command now does CozoDB→SurrealDB.

12) Gate graph_cozo.rs behind #[cfg(feature = "legacy-cozo")] — only compiled for migration.

13) Tests:
    - All existing GraphStore tests pass with SurrealGraphStore (same trait, same assertions)
    - New: concurrent access test (two SurrealGraphStore instances, one writes, one reads)
    - New: graph algorithm tests (PageRank relative ordering, BFS path length, community count)
    - New: migration test (create CozoDB with test data, migrate, verify in SurrealDB)
    - All MCP tool integration tests pass unchanged

14) Run validation:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test -p aether-core
    - cargo test -p aether-config
    - cargo test -p aether-store
    - cargo test -p aether-memory
    - cargo test -p aether-analysis
    - cargo test -p aether-mcp
    - cargo test -p aetherd

15) Commit with message: "Replace CozoDB with SurrealDB 3.0 for graph storage (Decision #38)"

SCOPE GUARD:
- Do NOT implement DEFINE API endpoints (Team Tier is future).
- Do NOT implement Surrealism WASM extensions.
- Do NOT implement HNSW vector search in SurrealDB (LanceDB handles vectors).
- Do NOT implement time-travel queries (VERSION clause).
- Do NOT implement SurrealDB auth (bearer token stays at Axum level).
- Do NOT remove the legacy-cozo feature gate — it's needed for migration.
- Do NOT modify SQLite schemas or LanceDB schemas.
- Do NOT change any MCP tool request/response schemas.
- If a GraphStore trait method is unclear, check graph_cozo.rs for the exact behavior contract.
```
