# Phase 7 Revision Punch List — Final Pre-Implementation Audit (v2)

Consolidated from three independent reviews cross-referenced against all ten Phase 7 specification files. Items categorized by action type and priority. PDF fallback decision resolved: Option A (lopdf).

---

## Category A: Already Addressed in Specs (No Action Needed)

These items were flagged in the reviews but are already handled in the current specs. Noting them here for auditability.

### A1. GenericUnit `parent_id` for hierarchical documents
- **Review 1 Item #4** recommended adding `parent_id: Option<String>` to GenericUnit.
- **Status: Already present.** Stage 7.4 `unit.rs` trait definition includes `fn parent_id(&self) -> Option<&str>`, and the `GenericUnit` struct has `pub parent_id: Option<String>`. The SQLite schema has `parent_id TEXT` with a self-referencing column.

### A2. SQLite read-only flags in aether-query
- **Review 1 Item #2** recommended `SQLITE_OPEN_READ_ONLY` in Stage 7.3.
- **Status: Already present.** Stage 7.1 defines `SqliteStore::open_readonly()` with `OpenFlags::SQLITE_OPEN_READ_ONLY`. Stage 7.3 architecture section states "SQLite: opened with SQLITE_OPEN_READONLY". The Codex prompt references `SharedState::open_readonly()`.

### A3. Entity resolution MVP dictionary-based approach
- **Review 1 Item #5** recommended a deterministic alias lookup before LLM.
- **Status: Already present.** Stage 7.7 `entity.rs` shows the `EntityResolver` using `HashMap<String, String>` aliases loaded from an `entity_aliases` SQLite table. LLM disambiguation is explicitly secondary via `disambiguate()`. The Codex prompt says "Do NOT implement graph-based entity resolution."

---

## Category B: Prompt Patches (Spec Is Correct, Codex Prompt Needs Update)

The specification describes the correct behavior, but the Codex prompt doesn't explicitly instruct it. LLMs may skip implicit requirements.

### B1. `Mutex<Connection>` in SqliteStore — HARD COMPILER ERROR

**Source:** Review 2, Landmine A
**File:** `phase_7_stage_7_1_store_pooling_v2.md`
**Problem:** Stage 7.1 spec discusses `Mutex<Connection>` in the "Thread Safety Analysis" section and recommends Option 1. However, the **Codex prompt** never explicitly instructs this. If Codex reads only the prompt (likely), it will try `Arc<SqliteStore>` with a raw `Connection`, triggering `error[E0277]: *mut sqlite3 cannot be shared between threads safely`.

**Required Codex prompt patch (insert after Step 3, before Step 4):**
```
CRITICAL THREAD SAFETY: SqliteStore currently holds a raw rusqlite::Connection,
which is !Send + !Sync. Wrapping SqliteStore in Arc requires the connection to be
safe for shared access. In crates/aether-store/src/lib.rs, change the conn field
in SqliteStore from Connection to Mutex<Connection>. Ensure ALL methods acquire
the lock via self.conn.lock().unwrap() before executing queries. This is required
for Arc<SqliteStore> to compile. Do NOT use a connection pool (r2d2) for this stage.
```

### B2. `spawn_blocking` for Graph Algorithms — ASYNC EXECUTOR DEADLOCK

**Source:** Review 2, Landmine B
**File:** `phase_7_stage_7_2_surrealdb_migration.md`
**Problem:** Stage 7.2 spec says graph algorithms run "application-level" in Rust, and notes they take <500ms. But the Codex prompt doesn't instruct wrapping them in `spawn_blocking`. If PageRank or Louvain runs directly inside an `async fn`, it blocks the Tokio worker thread. Multiple concurrent requests → deadlock.

**Required Codex prompt patch (insert into Step 7):**
```
CRITICAL ASYNC SAFETY: All mathematical graph algorithms (page_rank,
louvain_communities, bfs_shortest_path) perform CPU-bound computation (iterative
loops over adjacency matrices). They MUST execute inside tokio::task::spawn_blocking
to avoid starving the async reactor. The pattern is:

  pub async fn page_rank(db: &Surreal<Db>, ...) -> Result<HashMap<String, f64>> {
      let edges = fetch_all_edges(db).await?;  // async: fetch from SurrealDB
      tokio::task::spawn_blocking(move || {
          compute_page_rank_sync(&edges, damping, iterations)  // sync: CPU math
      }).await.map_err(|e| StoreError::Graph(format!("spawn_blocking: {e}")))?
  }
```

### B3. `petgraph` for Graph Algorithm Implementation

**Source:** Review 1, Item #1
**File:** `phase_7_stage_7_2_surrealdb_migration.md`
**Problem:** The spec asks Codex to "reimplement" PageRank and Louvain (~500 LOC). Hand-writing Louvain modularity optimization is non-trivial and error-prone for an LLM. The `petgraph` crate provides battle-tested graph data structures; the `petgraph::algo` module does NOT include PageRank or Louvain, but `petgraph::Graph` is ideal for building the in-memory adjacency representation.

**Required Codex prompt patch (insert into Step 7 dependency list):**
```
Add petgraph = "0.6" to crates/aether-analysis/Cargo.toml.
When implementing graph algorithms, dump SurrealDB edges into a petgraph::DiGraph
for the in-memory computation. This gives you correct adjacency iteration, node
indexing, and BFS traversal (petgraph::algo::dijkstra, petgraph::visit::Bfs).
PageRank and Louvain must still be hand-implemented on top of petgraph's data
structures (petgraph does not include these algorithms).
```

### B4. Axum Server Spawning in Dashboard — THREAD STARVATION

**Source:** Review 1, Item #6
**File:** `phase_7_stage_7_6_web_dashboard_v2.md`
**Problem:** `aetherd` runs a file watcher, debouncer, LSP server over stdio, and (with dashboard feature) an Axum HTTP server. The Codex prompt doesn't instruct how to spawn Axum relative to the LSP loop.

**Required Codex prompt patch (insert after Step 6):**
```
CRITICAL: The Axum HTTP server for the dashboard MUST be spawned on a background
Tokio task so it does not block the LSP stdio loop or the file watcher:

  tokio::spawn(async move {
      let listener = tokio::net::TcpListener::bind(&bind_addr).await.unwrap();
      axum::serve(listener, router).await.unwrap();
  });

The LSP stdio loop and file watcher continue on their own tasks.
```

---

## Category C: Specification Revisions (Spec Itself Needs Updating)

### C1. Drop `pdfium-render` — Use `lopdf` as Rust-Native Fallback

**Source:** Review 1 Item #3 + Review 2 Landmine C (independent convergence on the same issue)
**Decision:** Option A selected. `lopdf` replaces `pdfium-render`.
**Files:** `DECISIONS_v4.md` (Decision #39), `phase_7_stage_7_5_aether_legal_v2.md`, `phase_7_stage_7_7_aether_finance_v2.md`, `phase_7_pathfinder_v2.md`

**Problem:** `pdfium-render` is Rust FFI bindings to Google's C++ Pdfium library. It does **not** statically compile or bundle Pdfium. At runtime, it dynamically loads `libpdfium.so` / `pdfium.dll` / `libpdfium.dylib`. If the library isn't installed on the host system, the binary panics on the first legal/finance PDF ingest. This breaks AETHER's single-binary portability.

**Resolution:** `lopdf` (pure Rust, extracts raw text streams, zero C++ dependency) replaces `pdfium-render` as the fallback. `pdftotext` (Poppler) remains the primary extractor. `pdfium-render` can be reconsidered in a future phase with a proper distribution strategy.

### C2. Risk Register Update

**File:** `phase_7_pathfinder_v2.md`
**Action:** Add graph algorithm correctness risk row. Update PDF extraction row for lopdf.

---

## Category D: Codebase Bugs — Fix Before Phase 7

These are latent bugs in the current codebase that will cause compilation failures or runtime panics. They must be fixed before beginning Stage 7.1.

### D1. Arrow Version Conflict — HARD COMPILER ERROR

**Source:** Review 2, Codebase Bug #1; Review 3 Patch #1
**Problem:** Hardening pass 3 set workspace Arrow versions to `56.2`, but `lancedb v0.23.0` internally depends on `arrow v54.0.0`. When `aether-store` passes a v56.2 `RecordBatch` into LanceDB, the compiler throws an unresolvable trait mismatch.

**Fix (workspace Cargo.toml):**
```toml
# Change:
arrow-array = "56.2"
arrow-schema = "56.2"
# To:
arrow-array = "54"
arrow-schema = "54"
```

**Verify:** `cargo check -p aether-store` compiles cleanly after the change.

### D2. CozoDB Engine String vs Cargo Feature Mismatch

**Source:** Review 2, Codebase Bug #2; Review 3 Patch #2 (CORRECTED)
**Problem:** Hardening pass 3 changed the engine string in `graph_cozo.rs` to `"sqlite"`, but the Cargo.toml feature is `storage-sled`. CozoDB panics at runtime: "Unknown storage engine: sqlite".

**IMPORTANT — Review 3 recommended switching the Cargo feature to `storage-sqlite`. This is WRONG.** CozoDB's `storage-sqlite` backend declares `links = "sqlite3"` in its build script. `rusqlite` also declares `links = "sqlite3"`. Cargo enforces that only one crate can claim a given `links` value. Switching the feature would replace the runtime panic with a hard linker error.

**Correct fix — revert the code, not the feature:**

**Fix (crates/aether-store/src/graph_cozo.rs):**
```rust
// Change:
let db = DbInstance::new("sqlite", &graph_path_str, Default::default())
// To (revert to match the Cargo feature):
let db = DbInstance::new("sled", &graph_path_str, Default::default())
```

Leave the workspace Cargo.toml as `features = ["storage-sled", "graph-algo"]` — do NOT change it.

**Note:** Stage 7.2 removes CozoDB entirely. This fix restores consistency so the codebase compiles and runs for Stage 7.1 testing.

### D3. Thread Starvation in `unified_query.rs` — RUNTIME DEADLOCK

**Source:** Review 2, Codebase Bug #3; Review 3 Patch #4
**Problem:** `std::thread::spawn(...).join()` inside `async fn ask()` blocks the Tokio worker thread. With multiple concurrent queries, Tokio runs out of workers and the server freezes.

**Fix (crates/aether-memory/src/unified_query.rs):**
Replace all three occurrences of `std::thread::spawn(move || { ... }).join()` (for `symbol_lexical`, `note_lexical`, `test_lexical`) with:
```rust
tokio::task::spawn_blocking(move || {
    // ... same closure body ...
})
.await
.map_err(|err| {
    MemoryError::InvalidInput(format!("search task join failure: {err}"))
})??;
```

Full replacement pattern per block:
```rust
// Before:
let query = query_owned.clone();
let store_clone = store.clone();
let symbol_lexical = std::thread::spawn(move || {
    store_clone.search_symbols(query.as_str(), candidate_limit).map_err(Into::into)
})
.join()
.map_err(|err| MemoryError::InvalidInput(format!("symbol search task join failure: {err:?}")))??;

// After:
let q1 = query_owned.clone();
let store1 = store.clone();
let symbol_lexical = tokio::task::spawn_blocking(move || {
    store1.search_symbols(q1.as_str(), candidate_limit).map_err(Into::into)
})
.await
.map_err(|err| MemoryError::InvalidInput(format!("symbol search task join failure: {err}")))??;
```

Apply the same pattern for `note_lexical` and `test_lexical`.

### D4. Thread Starvation in Candle Embedding/Reranker — RUNTIME DEADLOCK

**Source:** Review 2, Codebase Bug #4; Review 3 Patch #5
**Problem:** CPU-bound neural network matrix multiplications in Candle block the async executor.

**Fix (crates/aether-infer/src/embedding/candle.rs):**
Wrap the `ensure_loaded()` + `embed_texts_with_loaded()` call chain in `tokio::task::spawn_blocking`:
```rust
// Before:
let loaded = Arc::clone(provider.ensure_loaded()?);
let mut output = Self::embed_texts_with_loaded(loaded.as_ref(), &input)
    .map_err(|err| InferError::ModelUnavailable(format!("candle embedding task failed: {err}")))?;
Ok(output.pop().unwrap_or_else(|| vec![0.0; CANDLE_EMBEDDING_DIM]))

// After:
let output = tokio::task::spawn_blocking(move || {
    let loaded = Arc::clone(provider.ensure_loaded()?);
    Self::embed_texts_with_loaded(loaded.as_ref(), &input)
        .map_err(|err| InferError::ModelUnavailable(format!("candle embedding task failed: {err}")))
})
.await
.map_err(|err| InferError::ModelUnavailable(format!("candle embedding join failed: {err}")))??;
Ok(output.pop().unwrap_or_else(|| vec![0.0; CANDLE_EMBEDDING_DIM]))
```

**Fix (crates/aether-infer/src/reranker/candle.rs):**
Same pattern for `ensure_loaded()` + `rerank_sync_with_loaded()`:
```rust
// Before:
let loaded = Arc::clone(provider.ensure_loaded()?);
Self::rerank_sync_with_loaded(loaded.as_ref(), query.as_str(), candidates.as_slice(), top_n)
    .map_err(|err| InferError::ModelUnavailable(format!("candle reranker task failed: {err}")))

// After:
tokio::task::spawn_blocking(move || {
    let loaded = Arc::clone(provider.ensure_loaded()?);
    Self::rerank_sync_with_loaded(loaded.as_ref(), query.as_str(), candidates.as_slice(), top_n)
        .map_err(|err| InferError::ModelUnavailable(format!("candle reranker task failed: {err}")))
})
.await
.map_err(|err| InferError::ModelUnavailable(format!("candle reranker join failed: {err}")))?
```

### D5. SqliteVectorStore Connection Churn — SEVERE PERFORMANCE BUG

**Source:** Review 3, Patch #3 (new item)
**Problem:** `SqliteVectorStore` does not hold a database connection. It calls `SqliteStore::open()` for every vector operation, re-running expensive SQLite schema migrations on every semantic search. This is the same class of bug as ARCH-1 but in the vector store path.

**Fix (crates/aether-store/src/vector.rs):**
```rust
// Before:
pub struct SqliteVectorStore {
    workspace_root: PathBuf,
}

impl SqliteVectorStore {
    pub fn new(workspace_root: impl AsRef<Path>) -> Self {
        Self {
            workspace_root: workspace_root.as_ref().to_path_buf(),
        }
    }

    fn store(&self) -> Result<SqliteStore, StoreError> {
        SqliteStore::open(&self.workspace_root)
    }
}

// After:
pub struct SqliteVectorStore {
    store: std::sync::Mutex<SqliteStore>,
}

impl SqliteVectorStore {
    pub fn new(workspace_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        Ok(Self {
            store: std::sync::Mutex::new(SqliteStore::open(workspace_root)?),
        })
    }
}
```

**Additional changes required:**
1. Update `open_vector_store` call site: `SqliteVectorStore::new(workspace_root)` → `SqliteVectorStore::new(workspace_root)?`
2. Update all methods in `impl VectorStore for SqliteVectorStore`: replace `self.store()?.method_name(...)` with `self.store.lock().unwrap().method_name(...)`

**Note:** Stage 7.1's SharedState refactor will further improve this, but fixing it now ensures stable performance during 7.1 testing.

### D6. Dynamic Inotify Registration for New Directories

**Source:** Review 3, Patch #6B (new item)
**Problem:** The file watcher correctly registers non-recursive watches on existing directories at startup (via `WalkBuilder`), but if a user creates a new directory while `aetherd` is running, the watcher misses all files in it until restart.

**Fix (crates/aetherd/src/indexer.rs):**
In both the `rx.recv_timeout` and `rx.try_recv` event handling blocks, insert this snippet immediately before the `enqueue_event_paths` call:
```rust
if let Ok(ref event) = result {
    for path in &event.paths {
        if path.is_dir() && !crate::observer::is_ignored_path(path) {
            let _ = watcher.watch(path, notify::RecursiveMode::NonRecursive);
        }
    }
}
```

This adds 4 lines in two locations. Low risk, high value for monorepo-style development where new modules are created frequently.

### D7. LSP File Read Outside Mutex — LATENCY OPTIMIZATION

**Source:** Review 3, Patch #6A (new item)
**Problem:** In `crates/aether-lsp/src/lib.rs`, the hover handler reads the file from disk while holding `self.store.lock().await`. File I/O under the store mutex means every hover request serializes behind both the file read AND the database query. On slow filesystems (network mounts, WSL2 cross-mount), this causes visible hover latency.

**Fix (crates/aether-lsp/src/lib.rs):**

Step 1 — Read the file BEFORE acquiring the lock:
```rust
// Before:
let guard = self.store.lock().await;
let markdown = resolve_hover_markdown_for_path(
    &self.workspace_root,
    &guard,
    &file_path,
    text_doc_pos.position.line as usize + 1,
    text_doc_pos.position.character as usize + 1,
);

// After:
let source = match std::fs::read_to_string(&file_path) {
    Ok(s) => s,
    Err(_) => return Ok(Some(no_sir_hover())),
};

let guard = self.store.lock().await;
let markdown = resolve_hover_markdown_for_source(
    &self.workspace_root,
    &guard,
    &file_path,
    &source,
    text_doc_pos.position.line as usize + 1,
    text_doc_pos.position.character as usize + 1,
);
```

Step 2 — Refactor `resolve_hover_markdown_for_path` into `resolve_hover_markdown_for_source`:

Create a new function that accepts `source: &str` instead of reading from disk internally. The existing `resolve_hover_markdown_for_path` can be reimplemented as a thin wrapper that reads the file and calls `resolve_hover_markdown_for_source`. Both function signatures should exist for backward compatibility:

```rust
/// New: accepts pre-read source text (no file I/O under lock)
pub fn resolve_hover_markdown_for_source(
    workspace_root: &Path,
    store: &SqliteStore,
    file_path: &Path,
    source: &str,
    line: usize,
    col: usize,
) -> String {
    // ... same logic as resolve_hover_markdown_for_path, but uses `source` instead of reading file
}

/// Original: reads file then delegates (kept for non-LSP callers)
pub fn resolve_hover_markdown_for_path(
    workspace_root: &Path,
    store: &SqliteStore,
    file_path: &Path,
    line: usize,
    col: usize,
) -> String {
    let source = match std::fs::read_to_string(file_path) {
        Ok(s) => s,
        Err(_) => return no_sir_markdown(),
    };
    resolve_hover_markdown_for_source(workspace_root, store, file_path, &source, line, col)
}
```

The key insight: file I/O happens outside the mutex, database queries happen inside it. This eliminates the file system as a latency contributor to mutex hold time.

---

## Execution Order

### Step 1: Apply Category D codebase fixes (manual, before Phase 7)
1. D1: Arrow version downgrade (`arrow-array`/`arrow-schema` 56.2 → 54)
2. D2: Revert `graph_cozo.rs` engine string to `"sled"` (do NOT touch Cargo.toml)
3. D3: `unified_query.rs` — replace `std::thread::spawn().join()` with `tokio::task::spawn_blocking().await`
4. D4: Candle embedding + reranker — wrap in `spawn_blocking`
5. D5: `SqliteVectorStore` — hold `Mutex<SqliteStore>` instead of reopening per operation
6. D6: `indexer.rs` — register inotify watches on newly created directories
7. D7: `aether-lsp` — read file outside mutex, refactor `resolve_hover_markdown_for_source`
8. Run full validation suite to confirm no regressions:
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
9. Commit: "Pre-Phase 7 codebase stabilization: fix Arrow conflict, thread starvation, vector store churn, LSP mutex, inotify gaps"

### Step 2: Apply Category B prompt patches to spec files
1. B1 → patch Stage 7.1 Codex prompt (Mutex<Connection>)
2. B2 → patch Stage 7.2 Codex prompt (spawn_blocking for graph algos)
3. B3 → patch Stage 7.2 Codex prompt (petgraph dependency)
4. B4 → patch Stage 7.6 Codex prompt (Axum spawn)

### Step 3: Apply Category C spec revisions
1. C1 → Decision #39 revision + Stage 7.5 + 7.7 updates (pdfium → lopdf)
2. C2 → Risk Register update

### Step 4: Begin Phase 7 implementation
Execute stages in dependency order: 7.1 → 7.2 → (7.3, 7.4, 7.6 parallel) → (7.5, 7.7 after 7.4)
