# Claude Code Prompt: Phase 8.23 — Graph Read-Path Denormalization

## Preflight

```bash
# Ensure clean working tree
git status --porcelain
# Should be empty. If not, stash or commit first.

git pull --ff-only

# Create worktree
git worktree add -B feature/graph-denormalization /home/rephu/feature/graph-denormalization

cd /home/rephu/feature/graph-denormalization

export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=16
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

## Context

Read these files before writing any code:
- `docs/roadmap/phase_8_stage_8_23_graph_denormalization.md` — the spec
- `docs/roadmap/phase8_stage8_23_session_context.md` — session context with known gotchas
- `crates/aether-store/src/schema.rs` — current migrations (PRAGMA user_version = 14)
- `crates/aether-store/src/graph.rs` — `store_upsert_edges`, `store_delete_edges_for_file`, `store_get_callers`, `store_get_dependencies`
- `crates/aether-store/src/graph_sqlite.rs` — `SqliteGraphStore` implementation (GraphStore trait)
- `crates/aether-mcp/src/state.rs` — `open_shared_graph` (sync version always uses SqliteGraphStore), `open_shared_graph_async` (opens SurrealGraphStore when config says surreal — THIS is the lock contention path)
- `crates/aether-mcp/src/tools/search.rs` — `aether_dependencies_logic` (calls `graph_store.get_callers/get_dependencies`)
- `crates/aetherd/src/health_score.rs` — opens `SurrealGraphStore::open()` directly (line ~81)
- `crates/aetherd/src/fsck.rs` — opens `SurrealGraphStore::open()` directly (line ~137)
- `crates/aetherd/src/refactor_prep.rs` — opens `SurrealGraphStore::open()` directly (lines ~117, ~545)
- `crates/aetherd/src/indexer.rs` — where `store.upsert_edges()` is called during indexing

## Mandatory Source Inspection

Before writing any code, inspect these files and answer the questions:

1. Read `crates/aether-store/src/schema.rs`. Confirm the latest migration is `version < 14` creating `sir_quality` table. The new migration will be `version < 15`.

2. Read `crates/aether-store/src/graph.rs` method `store_upsert_edges`. Identify:
   - The `symbol_edges` table columns: `source_id`, `target_qualified_name`, `edge_kind`, `file_path`
   - The ON CONFLICT clause
   - The `store_delete_edges_for_file` implementation

3. Read `crates/aether-store/src/graph.rs` methods `store_get_callers` and `store_get_dependencies`. Identify:
   - `store_get_callers` queries by `target_qualified_name` with `edge_kind = 'calls'` only
   - `store_get_dependencies` queries by `source_id` with `edge_kind = 'depends_on'` only
   - Both return `Vec<SymbolEdge>` (raw edges, not resolved symbols)

4. Read `crates/aether-store/src/graph_sqlite.rs` methods `get_callers` and `get_dependencies`. Identify:
   - These JOIN `symbol_edges` with `symbols` to return resolved `Vec<SymbolRecord>`
   - What `edge_kind` values each one filters on (verify — `get_dependencies` may use `calls` not `depends_on`)
   - The exact SQL queries used

5. Read `crates/aether-mcp/src/state.rs` function `open_shared_graph` (sync version, ~line 340). Confirm it ALWAYS creates `SqliteGraphStore` regardless of configured backend.

6. Read `crates/aether-mcp/src/state.rs` function `open_shared_graph_async` (~line 360). Confirm that when `graph_backend == Surreal`, it opens `SurrealGraphStore::open()` which takes the fs2 file lock. This is the contention path we need to fix.

7. Read `crates/aetherd/src/indexer.rs`. Find where `store.upsert_edges()` is called during indexing. Identify the surrounding context — what file_path is available, whether it runs inside a per-file loop or a batch loop.

8. Check all `check_compatibility("core", 14)` calls:
   - `crates/aether-mcp/src/state.rs` (should be 4 occurrences)
   - `crates/aether-dashboard/src/state.rs` (should be 1 occurrence)
   All must be bumped to `check_compatibility("core", 15)`.

9. Check test assertions on schema version:
   - `grep -rn "schema_version\|user_version" crates/aether-store/src/tests/`
   Update any that assert a specific version number.

## Implementation

### Step 1: Schema Migration (v15)

In `crates/aether-store/src/schema.rs`, after the `version < 14` block and before the `schema_version` table creation, add:

```rust
if version < 15 {
    conn.execute_batch(
        r#"
    CREATE TABLE IF NOT EXISTS symbol_neighbors (
        symbol_id     TEXT NOT NULL,
        neighbor_id   TEXT NOT NULL,
        edge_type     TEXT NOT NULL,
        neighbor_name TEXT NOT NULL,
        neighbor_file TEXT NOT NULL,
        PRIMARY KEY (symbol_id, neighbor_id, edge_type)
    );
    CREATE INDEX IF NOT EXISTS idx_neighbors_symbol ON symbol_neighbors(symbol_id);
    CREATE INDEX IF NOT EXISTS idx_neighbors_file ON symbol_neighbors(neighbor_file);
    "#,
    )?;
    conn.execute("PRAGMA user_version = 15", [])?;
}
```

### Step 2: Add `SymbolNeighborRecord` Struct

In `crates/aether-store/src/lib.rs` (or a new `crates/aether-store/src/neighbors.rs` module if you prefer), add:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolNeighborRecord {
    pub symbol_id: String,
    pub neighbor_id: String,
    pub edge_type: String,
    pub neighbor_name: String,
    pub neighbor_file: String,
}
```

Ensure it is publicly exported from the crate.

### Step 3: Add `populate_symbol_neighbors` to `SqliteStore`

In `crates/aether-store/src/graph.rs`, add a new method on `SqliteStore`:

```rust
pub(crate) fn populate_symbol_neighbors(&self, file_path: &str) -> Result<(), StoreError>
```

This method must, in a single transaction:

1. Get all symbol IDs in the file: `SELECT id FROM symbols WHERE file_path = ?1`
2. Delete existing forward entries: `DELETE FROM symbol_neighbors WHERE symbol_id IN (SELECT id FROM symbols WHERE file_path = ?1)`
3. Delete existing reverse entries: `DELETE FROM symbol_neighbors WHERE neighbor_id IN (SELECT id FROM symbols WHERE file_path = ?1)`
4. For each edge in `symbol_edges` where the source symbol is in this file, resolve the target and insert both directions:

```sql
-- Forward edges: source_id → resolved target
INSERT OR REPLACE INTO symbol_neighbors (symbol_id, neighbor_id, edge_type, neighbor_name, neighbor_file)
SELECT
    e.source_id,
    s_target.id,
    e.edge_kind,
    s_target.qualified_name,
    s_target.file_path
FROM symbol_edges e
JOIN symbols s_source ON s_source.id = e.source_id
JOIN symbols s_target ON s_target.qualified_name = e.target_qualified_name
WHERE s_source.file_path = ?1;

-- Reverse edges: resolved target → source_id
INSERT OR REPLACE INTO symbol_neighbors (symbol_id, neighbor_id, edge_type, neighbor_name, neighbor_file)
SELECT
    s_target.id,
    e.source_id,
    CASE e.edge_kind
        WHEN 'calls' THEN 'called_by'
        WHEN 'depends_on' THEN 'depended_on_by'
        WHEN 'implements' THEN 'implemented_by'
        WHEN 'type_ref' THEN 'type_ref_by'
        ELSE e.edge_kind || '_reverse'
    END,
    s_source.qualified_name,
    s_source.file_path
FROM symbol_edges e
JOIN symbols s_source ON s_source.id = e.source_id
JOIN symbols s_target ON s_target.qualified_name = e.target_qualified_name
WHERE s_source.file_path = ?1;
```

### Step 4: Add Read Methods

In `crates/aether-store/src/graph.rs`, add public read methods on `SqliteStore`:

```rust
/// Get all neighbors for a symbol from the denormalized table.
pub fn get_symbol_neighbors(&self, symbol_id: &str) -> Result<Vec<SymbolNeighborRecord>, StoreError>

/// Get neighbors filtered by edge type.
pub fn get_symbol_neighbors_by_type(&self, symbol_id: &str, edge_type: &str) -> Result<Vec<SymbolNeighborRecord>, StoreError>
```

These are simple `SELECT * FROM symbol_neighbors WHERE symbol_id = ?1` queries (with optional `AND edge_type = ?2`).

### Step 5: Hook Into Indexing Write Path

Find where `store.upsert_edges()` is called in `crates/aetherd/src/indexer.rs`. After edges are upserted for a file, call:

```rust
if let Err(err) = store.populate_symbol_neighbors(file_path) {
    tracing::warn!(
        file = %file_path,
        error = %err,
        "failed to populate symbol_neighbors for file"
    );
}
```

Use `warn` not `error` — neighbor population failure should not block indexing.

Also find where `store.delete_edges_for_file()` is called. After it, add:

```rust
// Clean symbol_neighbors for deleted file edges
// (populate_symbol_neighbors handles cleanup when called for the new edges,
// but if edges are deleted without re-population, we need explicit cleanup)
```

The delete cleanup can be handled by `populate_symbol_neighbors` when it runs for the same file, but verify there are no code paths that delete edges without subsequently calling populate.

### Step 6: Fix MCP Async Graph Path

In `crates/aether-mcp/src/state.rs`, modify `open_shared_graph_async` so that when `graph_backend == Surreal`, it STILL creates a `SqliteGraphStore` for the `Arc<dyn GraphStore>` return value (same as the sync version does), and only opens `SurrealGraphStore` for the optional second return value used by tools that explicitly need SurrealDB (like `aether_call_chain` for multi-hop):

```rust
async fn open_shared_graph_async(
    workspace: &Path,
    config: &AetherConfig,
    read_only: bool,
) -> Result<(Arc<dyn GraphStore>, Option<Arc<SurrealGraphStore>>), AetherMcpError> {
    // Always use SqliteGraphStore for the primary GraphStore interface.
    // This avoids SurrealKV file lock contention when the daemon is running.
    let graph: Arc<dyn GraphStore> = if read_only {
        Arc::new(SqliteGraphStore::open_readonly(workspace)?)
    } else {
        Arc::new(SqliteGraphStore::open(workspace)?)
    };

    // Only open SurrealDB when explicitly configured AND the caller may need
    // multi-hop traversal (call chains, community detection).
    let surreal = match config.storage.graph_backend {
        GraphBackend::Surreal => {
            match if read_only {
                SurrealGraphStore::open_readonly(workspace).await
            } else {
                SurrealGraphStore::open(workspace).await
            } {
                Ok(store) => Some(Arc::new(store)),
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "SurrealDB graph unavailable (daemon may hold lock), using SQLite only"
                    );
                    None
                }
            }
        }
        _ => None,
    };

    Ok((graph, surreal))
}
```

This is the critical fix: the primary `GraphStore` used by MCP tools like `aether_dependencies` will always be SQLite. SurrealDB becomes optional and fails gracefully if the daemon holds the lock.

### Step 7: Bump `check_compatibility` Calls

Update ALL of these from `check_compatibility("core", 14)` to `check_compatibility("core", 15)`:
- `crates/aether-mcp/src/state.rs` (4 occurrences)
- `crates/aether-dashboard/src/state.rs` (1 occurrence)

### Step 8: Update Test Assertions

Update any tests that assert `schema_version.version == 14` to assert `== 15`.
Update the `run_migrations_sets_user_version_and_is_idempotent` test if it checks a specific version number.

### Step 9: Add Tests

In `crates/aether-store/src/tests/` (new file `neighbors.rs` or add to `basic.rs`):

1. **Test `populate_symbol_neighbors` populates forward and reverse edges:**
   - Create two symbols (alpha in `src/a.rs`, beta in `src/b.rs`)
   - Upsert a `calls` edge from alpha → beta
   - Call `populate_symbol_neighbors("src/a.rs")`
   - Assert `get_symbol_neighbors(alpha.id)` contains forward entry with edge_type `calls`
   - Assert `get_symbol_neighbors(beta.id)` contains reverse entry with edge_type `called_by`

2. **Test `populate_symbol_neighbors` cleans stale entries on re-population:**
   - Populate neighbors for a file
   - Delete the edge, re-upsert a different edge, re-populate
   - Assert old neighbor is gone, new one present

3. **Test `get_symbol_neighbors_by_type` filters correctly:**
   - Create edges of different types
   - Assert filtering by `calls` returns only call edges

4. **Test schema migration v15 is idempotent:**
   - Run migrations, verify `symbol_neighbors` table exists
   - Run migrations again, verify no error

## Scope Guard

**Files modified:**
- `crates/aether-store/src/schema.rs` — add v15 migration
- `crates/aether-store/src/graph.rs` — add `populate_symbol_neighbors`, `get_symbol_neighbors`, `get_symbol_neighbors_by_type`
- `crates/aether-store/src/lib.rs` — expose `SymbolNeighborRecord` struct
- `crates/aether-store/src/graph_sqlite.rs` — optionally add denormalized query wrappers on `SqliteGraphStore`
- `crates/aetherd/src/indexer.rs` — hook `populate_symbol_neighbors` after edge upsert
- `crates/aether-mcp/src/state.rs` — fix `open_shared_graph_async` to always use SQLite for primary GraphStore, bump `check_compatibility` to 15
- `crates/aether-dashboard/src/state.rs` — bump `check_compatibility` to 15
- `crates/aether-store/src/tests/` — add neighbor tests, update schema version assertions

**Files NOT modified:**
- `crates/aether-store/src/graph_surreal.rs` — NOT touched
- `crates/aether-mcp/src/tools/search.rs` — NOT migrated yet (future follow-up to use denormalized table)
- `crates/aetherd/src/health_score.rs` — NOT migrated yet (future follow-up)
- `crates/aetherd/src/fsck.rs` — NOT migrated yet
- `crates/aetherd/src/refactor_prep.rs` — NOT migrated yet
- No changes to inference providers, config crate, or CLI args
- No changes to batch pipeline

## Validation

```bash
# Format check
cargo fmt --all --check

# Clippy for all affected crates
cargo clippy -p aether-store -- -D warnings
cargo clippy -p aetherd --features dashboard -- -D warnings
cargo clippy -p aether-mcp -- -D warnings
cargo clippy -p aether-dashboard -- -D warnings

# Per-crate tests
cargo test -p aether-store
cargo test -p aetherd
cargo test -p aether-mcp
```

Do NOT run `cargo test --workspace` — OOM risk on servers and duplicates CI coverage.

## Commit

```
feat(store): add symbol_neighbors denormalized table for contention-free graph reads

Add schema migration v15 creating the symbol_neighbors table with
pre-resolved neighbor names and file paths. Populate the table during
indexing after edge upsert, including both forward and reverse edges
(calls/called_by, depends_on/depended_on_by, implements/implemented_by).

Fix MCP async graph initialization to always use SqliteGraphStore for
the primary GraphStore interface, with SurrealDB as an optional fallback
that fails gracefully when the daemon holds the file lock.

This implements Decision #58 (Dual Database Architecture): the daemon
writes to both SurrealDB and SQLite during indexing, enabling all
read-path consumers (MCP, CLI, LSP) to query SQLite without SurrealKV
lock contention.

Migration: automatic on first startup (PRAGMA user_version 14 → 15).
```

## Post-fix Cleanup

```bash
git push origin feature/graph-denormalization
```

Create PR via GitHub web UI with title and body below.

After merge:
```bash
git switch main && git pull --ff-only
git worktree remove /home/rephu/feature/graph-denormalization
git branch -D feature/graph-denormalization
```

## PR Title

`feat(store): denormalize symbol neighbors to SQLite for contention-free graph reads`

## PR Body

Implements Decision #58 — dual-write graph architecture. During indexing,
after edges are written to SurrealDB, the daemon also populates a
`symbol_neighbors` table in `meta.sqlite` with pre-resolved neighbor
names and file paths, including reverse edges.

Fixes MCP async graph initialization to always use `SqliteGraphStore`
for the primary `GraphStore` interface. SurrealDB opens as an optional
secondary handle that fails gracefully when the daemon holds the lock.
This means MCP tools work while the daemon is scanning/triaging.

**Schema:** Migration v14 → v15 adds `symbol_neighbors` table.

**Write path:** `populate_symbol_neighbors(file_path)` called after
`upsert_edges` during indexing. Single transaction per file.

**Read path:** New `get_symbol_neighbors` / `get_symbol_neighbors_by_type`
methods on `SqliteStore`.

**MCP fix:** `open_shared_graph_async` now always returns `SqliteGraphStore`
as the primary `Arc<dyn GraphStore>`, with `SurrealGraphStore` as optional
secondary (fails gracefully with warning if daemon holds lock).

**Future follow-up:** Migrate `aether_dependencies` MCP tool,
`health-score`, `fsck`, and `refactor-prep` CLI commands to use the
denormalized table for even faster reads.
