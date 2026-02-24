# Phase 7 — The Pathfinder

## Stage 7.1 — Store Pooling + Shared State Refactor (Revised)

### Purpose

Refactor `AetherMcpServer` to hold database connections as shared state (`Arc`-wrapped) instead of reopening them on every MCP tool call. This fixes the ARCH-1 connection thrashing bug from the hardening scan and establishes the `SharedState` pattern that the query server (Stage 7.3), web dashboard (Stage 7.6), and all future consumers will use.

**This is the most important infrastructure stage in Phase 7.** Every subsequent stage depends on the `SharedState` abstraction. Without it, the query server can't hold read-only connections, the web API can't share a store with the MCP server, and every MCP call continues paying unnecessary connection setup cost.

### What Problem This Solves

The hardening scan (ARCH-1) identified:

> Nearly every MCP tool request (`aether_search_logic`, `aether_blast_radius_logic`, etc.) instantiates a new database connection: `SqliteStore::open(workspace)`, `open_vector_store()`, and re-reads the TOML config. Opening LanceDB and running SQLite PRAGMAs on every single query creates massive, unnecessary latency.

Currently in `aether-mcp/src/lib.rs`, each tool handler does something like:
```rust
fn aether_some_tool_logic(&self, request: Request) -> Result<Response> {
    let store = SqliteStore::open(&self.workspace)?;  // Opens new connection!
    let graph = open_graph_store(&self.workspace)?;    // Opens new connection!
    let config = load_workspace_config(&self.workspace)?; // Re-parses TOML!
    // ... use store, graph, config ...
}
```

This means:
- SQLite PRAGMA execution on every request (~1ms each)
- CozoDB/sled initialization on every request (~5-10ms) — **will become SurrealDB init in 7.2**
- LanceDB table scanning on every request (~2-5ms)
- TOML parsing on every request (~0.1ms)
- No connection sharing between concurrent MCP requests
- The query server (7.3) can't hold persistent read-only handles

### Architecture: SharedState

```rust
// crates/aether-mcp/src/state.rs (NEW)

use std::sync::Arc;

/// Shared state held for the lifetime of the server process.
/// Opened once at startup. All tool handlers borrow from this.
pub struct SharedState {
    pub store: Arc<SqliteStore>,
    pub graph: Arc<dyn GraphStore>,
    pub config: Arc<AetherConfig>,
    pub vector_store: Option<Arc<dyn VectorStore>>,
    pub read_only: bool,
    pub schema_version: SchemaVersion,
}

impl SharedState {
    /// Create shared state for the full daemon (read-write).
    pub fn open_readwrite(workspace: &Path) -> Result<Self, AetherMcpError> {
        let config = Arc::new(load_workspace_config(workspace)?);
        let store = Arc::new(SqliteStore::open(workspace)?);
        let graph = Arc::from(open_graph_store(workspace)?);
        let vector_store = open_vector_store_optional(workspace, &config)?
            .map(|v| Arc::from(v) as Arc<dyn VectorStore>);
        let schema_version = store.get_schema_version()?;

        Ok(Self {
            store,
            graph,
            config,
            vector_store,
            read_only: false,
            schema_version,
        })
    }

    /// Create shared state for the query server (read-only).
    /// Opens databases with read-only flags where supported.
    pub fn open_readonly(index_path: &Path) -> Result<Self, AetherMcpError> {
        let config = Arc::new(load_workspace_config(index_path)?);
        let store = Arc::new(SqliteStore::open_readonly(index_path)?);
        let graph = Arc::from(open_graph_store_readonly(index_path)?);
        let vector_store = open_vector_store_optional(index_path, &config)?
            .map(|v| Arc::from(v) as Arc<dyn VectorStore>);
        let schema_version = store.get_schema_version()?;

        Ok(Self {
            store,
            graph,
            config,
            vector_store,
            read_only: true,
            schema_version,
        })
    }

    /// Guard: returns error if server is read-only and a write was attempted.
    pub fn require_writable(&self) -> Result<(), AetherMcpError> {
        if self.read_only {
            Err(AetherMcpError::ReadOnly(
                "This is a read-only query server. Use the full aetherd daemon for write operations."
            ))
        } else {
            Ok(())
        }
    }
}
```

**Note on graph store:** Stage 7.1 still uses the current CozoDB graph store. Stage 7.2 replaces the `GraphStore` implementation with SurrealDB. The `Arc<dyn GraphStore>` abstraction means 7.1 is unaffected by the 7.2 swap — the `SharedState` works with either implementation.

### Changes to AetherMcpServer

**Before (current):**
```rust
pub struct AetherMcpServer {
    workspace: PathBuf,
    verbose: bool,
}

impl AetherMcpServer {
    pub fn new(workspace: &Path, verbose: bool) -> Result<Self, anyhow::Error> {
        Ok(Self {
            workspace: workspace.to_path_buf(),
            verbose,
        })
    }

    fn lock_store(&self) -> Result<SqliteStore, AetherMcpError> {
        SqliteStore::open(&self.workspace).map_err(...)  // Opens new connection every time!
    }
}
```

**After (this stage):**
```rust
pub struct AetherMcpServer {
    state: Arc<SharedState>,
    verbose: bool,
}

impl AetherMcpServer {
    pub fn new(workspace: &Path, verbose: bool) -> Result<Self, anyhow::Error> {
        let state = Arc::new(SharedState::open_readwrite(workspace)?);
        Ok(Self { state, verbose })
    }

    /// New: create from pre-built shared state (used by aether-query)
    pub fn from_state(state: Arc<SharedState>, verbose: bool) -> Self {
        Self { state, verbose }
    }

    // Remove lock_store() entirely. Tool handlers use self.state.store directly.
}
```

### Changes to SqliteStore

Add read-only opening mode:

```rust
// crates/aether-store/src/lib.rs

impl SqliteStore {
    /// Existing: open for read-write
    pub fn open(workspace_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::open_with_flags(workspace_root, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE)
    }

    /// New: open for read-only (no writes, no migrations)
    pub fn open_readonly(workspace_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::open_with_flags(workspace_root, OpenFlags::SQLITE_OPEN_READ_ONLY)
    }

    fn open_with_flags(workspace_root: impl AsRef<Path>, flags: OpenFlags) -> Result<Self, StoreError> {
        // ... shared initialization logic ...
        // Skip migrations if read-only
        // Skip WAL mode if read-only (WAL is read-compatible without setting)
    }
}
```

### Schema Version Table

Added during `run_migrations()`:

```sql
CREATE TABLE IF NOT EXISTS schema_version (
    component TEXT PRIMARY KEY,
    version INTEGER NOT NULL,
    migrated_at INTEGER NOT NULL
);
INSERT OR IGNORE INTO schema_version (component, version, migrated_at) VALUES ('core', 1, strftime('%s', 'now'));
```

Query methods:
```rust
impl SqliteStore {
    pub fn get_schema_version(&self, component: &str) -> Result<u32, StoreError>;
    pub fn check_compatibility(&self, component: &str, max_supported: u32) -> Result<(), StoreError>;
}
```

### Tool Handler Migration Pattern

**Read tools (all 17+ existing tools):**
```rust
pub async fn aether_search_logic(&self, request: AetherSearchRequest) -> Result<AetherSearchResponse, AetherMcpError> {
    let store = &self.state.store;           // Arc<SqliteStore> — no new connection
    let graph_store = &self.state.graph;     // Arc<dyn GraphStore> — no new connection
    let config = &self.state.config;         // Arc<AetherConfig> — no re-parse
    // ... search logic using store, graph_store, config (same logic, different access) ...
}
```

**Write tools (add guard):**
```rust
pub async fn aether_remember_logic(&self, request: AetherRememberRequest) -> Result<AetherRememberResponse, AetherMcpError> {
    self.state.require_writable()?;
    let store = &self.state.store;
    // ... write logic ...
}
```

### File Paths (new/modified)

| Path | Action | Description |
|------|--------|-------------|
| `crates/aether-mcp/src/state.rs` | **Create** | SharedState struct with open_readwrite/open_readonly |
| `crates/aether-mcp/src/lib.rs` | **Modify** | Replace workspace field with Arc<SharedState>. Remove lock_store(). Update all tool handlers. |
| `crates/aether-mcp/src/errors.rs` | **Modify** | Add ReadOnly error variant |
| `crates/aether-store/src/lib.rs` | **Modify** | Add SqliteStore::open_readonly(), open_with_flags(), schema_version table + queries |
| `crates/aether-store/src/graph_cozo.rs` | **Modify** | Add CozoGraphStore::open_readonly(), open_graph_store_readonly() |
| `crates/aether-store/src/graph_sqlite.rs` | **Modify** | Add SqliteGraphStore::open_readonly() |
| `crates/aether-mcp/tests/mcp_tools.rs` | **Modify** | Update test setup to use SharedState |
| `crates/aetherd/src/main.rs` | **Modify** | Create SharedState at startup, pass to AetherMcpServer |

### Thread Safety Analysis

**SqliteStore:** Currently uses `Connection` which is `!Send + !Sync`. Wrapping in `Arc` requires the connection to be safe for shared access.

Options:
1. **`Mutex<Connection>`** — serialize all database access. Simple but limits concurrency.
2. **`r2d2::Pool<SqliteConnectionManager>`** — connection pool. Each handler gets its own connection from the pool. Best concurrency.
3. **Single connection with `Mutex`, read pool with separate connections** — hybrid.

**Recommendation for this stage:** Option 1 (`Arc<Mutex<Connection>>`) or restructure `SqliteStore` to hold a `Mutex<Connection>`. This matches the existing single-process model. Connection pooling (Option 2) can be added in a future optimization stage if contention becomes measurable.

**Note:** The existing `SqliteStore` already uses `Connection` (not `Send`), so the current code must be running all tool handlers on a single thread or using `spawn_blocking`. Verify this in the implementation — the `#[tool_router]` macro generates async handlers, and the existing code uses `tokio::task::spawn_blocking(move || server.some_logic())` which moves the cloned server into a blocking task. With `Arc<SharedState>`, the `spawn_blocking` pattern continues to work: clone the Arc, move into blocking task, access the Mutex-wrapped connection.

### Edge Cases

| Scenario | Behavior |
|----------|----------|
| SharedState fails to open (corrupt SQLite) | AetherMcpServer::new() returns error. Server doesn't start. Clear error message. |
| Read-only server receives write MCP call | Returns `{"error": "read_only_server", "message": "..."}`. HTTP 400. |
| Multiple concurrent MCP requests | Mutex serializes SQLite access. LanceDB and CozoDB handle their own concurrency. |
| Config file changes while server is running | Config is loaded once at startup. Restart required for config changes. (Same as current behavior.) |
| Database migration needed but server is read-only | open_readonly() skips migrations. If schema is too old, compatibility check fails at startup with clear error. |
| Vector store not configured (embeddings disabled) | SharedState.vector_store = None. Tools that need vectors return fallback_reason: "embeddings_disabled". (Same as current behavior.) |

### Pass Criteria

1. **All 17+ existing MCP tools produce identical responses before and after the refactor.** This is the critical regression test. Run the full `mcp_tools.rs` integration test suite — every assertion must pass unchanged.
2. `AetherMcpServer` no longer calls `SqliteStore::open()`, `open_graph_store()`, or `load_workspace_config()` in any tool handler. Grep confirms zero occurrences.
3. `SharedState::open_readwrite()` successfully opens all databases and holds them.
4. `SharedState::open_readonly()` opens databases in read-only mode. Verify: `INSERT` on read-only SQLite returns error.
5. `SharedState::require_writable()` returns error when `read_only = true`.
6. `schema_version` table exists with `core` component entry.
7. Validation gates pass:
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

CONTEXT: This stage fixes the ARCH-1 connection thrashing bug identified in the
hardening scan. Currently, every MCP tool handler opens new SqliteStore, GraphStore,
and config connections. This stage refactors to hold shared state once at startup.

THIS IS A PURE REFACTOR. No new features. No new MCP tools. No new CLI commands.
The ONLY user-visible change is improved latency on MCP tool calls. Every existing
test must pass with identical results.

NOTE: Stage 7.2 (next) will replace CozoDB with SurrealDB. This stage still uses
CozoDB — the Arc<dyn GraphStore> abstraction means the SharedState pattern works
identically with either backend. Do NOT anticipate or implement any SurrealDB changes.

READ THESE FILES FIRST (critical for understanding the current architecture):
- crates/aether-mcp/src/lib.rs (current AetherMcpServer + all tool handlers)
- crates/aether-store/src/lib.rs (SqliteStore, Store trait, GraphStore trait, open_graph_store)
- crates/aether-store/src/graph_cozo.rs (CozoGraphStore::open)
- crates/aether-mcp/tests/mcp_tools.rs (integration tests — these MUST all pass unchanged)
- crates/aetherd/src/main.rs (where AetherMcpServer is created)
- docs/roadmap/phase_7_stage_7_1_store_pooling.md (this spec)

You are working in the repo root at /home/rephu/projects/aether.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase7-stage7-1-store-pooling off main.
3) Create worktree ../aether-phase7-stage7-1 for that branch and switch into it.

CRITICAL THREAD SAFETY: SqliteStore currently holds a raw rusqlite::Connection,
which is !Send + !Sync. Wrapping SqliteStore in Arc requires the connection to be
safe for shared access. In crates/aether-store/src/lib.rs, change the conn field
in SqliteStore from Connection to Mutex<Connection>. Ensure ALL methods in
SqliteStore acquire the lock via self.conn.lock().unwrap() before executing
queries. This is required for Arc<SqliteStore> to compile — without it, the Rust
compiler will reject Arc<SqliteStore> with:
  error[E0277]: *mut sqlite3 cannot be shared between threads safely
Do NOT use a connection pool (r2d2) for this stage — Mutex is sufficient for the
single-process model. Connection pooling is a future optimization.

4) In crates/aether-store/src/lib.rs:
   a) Add SqliteStore::open_readonly() that opens with SQLITE_OPEN_READ_ONLY flag.
      Skip migrations when read-only. Skip WAL pragma when read-only.
   b) Add schema_version table to run_migrations(). Insert ('core', 1, epoch) on creation.
   c) Add get_schema_version() and check_compatibility() methods.
   d) Add open_graph_store_readonly() helper function.

5) In crates/aether-store/src/graph_cozo.rs:
   a) Add CozoGraphStore::open_readonly() that passes read_only config to sled.
   b) Factor out shared init logic into open_internal(workspace, read_only).

6) In crates/aether-store/src/graph_sqlite.rs:
   a) Add SqliteGraphStore::open_readonly() (same pattern as CozoGraphStore).

7) In crates/aether-mcp:
   a) Create src/state.rs with SharedState struct:
      - Arc<SqliteStore>, Arc<dyn GraphStore>, Option<Arc<dyn VectorStore>>, Arc<AetherConfig>
      - read_only: bool, schema_version: SchemaVersion
      - open_readwrite(workspace) and open_readonly(workspace) constructors
      - require_writable() guard method
   b) Modify src/lib.rs:
      - Replace workspace: PathBuf with state: Arc<SharedState>
      - Add from_state() constructor (takes pre-built SharedState)
      - Update new() to create SharedState::open_readwrite() internally
      - Remove lock_store() method entirely
      - Update EVERY tool handler to use self.state.store, self.state.graph, self.state.config
        instead of opening new connections
      - Add self.state.require_writable()? guard to write tools: aether_remember,
        aether_session_note, aether_acknowledge_drift, aether_snapshot_intent
   c) Add ReadOnly variant to error types.

8) In crates/aetherd/src/main.rs:
   a) Update AetherMcpServer creation to use the new constructor pattern.
   b) No other changes needed — the constructor handles state creation.

9) Update crates/aether-mcp/tests/mcp_tools.rs:
   a) Update test setup to work with new constructor.
   b) ALL existing test assertions must pass UNCHANGED. If any test needs
      assertion changes, STOP and report — the refactor has a bug.

10) Add new tests:
    - Test SharedState::open_readonly() correctly opens in read-only mode
    - Test require_writable() returns error when read_only=true
    - Test schema_version table is populated
    - Test that read-only SqliteStore rejects INSERT operations

11) Run validation (per-crate to avoid OOM):
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test -p aether-core
    - cargo test -p aether-config
    - cargo test -p aether-store
    - cargo test -p aether-memory
    - cargo test -p aether-analysis
    - cargo test -p aether-mcp
    - cargo test -p aetherd

12) Commit with message: "Refactor MCP server to use shared state (fix ARCH-1 connection thrashing)"

SCOPE GUARD:
- Do NOT add new MCP tools.
- Do NOT add new CLI commands.
- Do NOT change any MCP tool request/response schemas.
- Do NOT modify CozoDB relation schemas.
- Do NOT modify SQLite table schemas (except adding schema_version table).
- Do NOT add read_only mode to VectorStore yet (LanceDB handles concurrent reads natively).
- Do NOT implement any SurrealDB changes — that is Stage 7.2.
- If ANY existing test assertion needs changing, the refactor has a regression — STOP and fix.
```
