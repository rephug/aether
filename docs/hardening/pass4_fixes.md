# AETHER Hardening Pass 4 — Codex Fix Specification

**Source:** Gemini deep code review via repomix export (Feb 28, 2026)
**Triage:** Claude + Gemini cross-validation (Claude validated findings, Gemini pushed back with source-level evidence, Claude adjudicated)
**Status:** 📋 Ready for Codex

---

## Summary

Gemini performed a full codebase scan (via repomix export) and identified 13+ findings. After two rounds of validation — Claude's initial triage and Gemini's pushback with source-level evidence — the definitive fix list is **12 fixes** across security, concurrency, performance, and correctness categories.

| Priority | Fix | Impact |
|----------|-----|--------|
| 🔴 Security | CORS permissive policy on localhost | Codebase intelligence exfiltration |
| 🔴 Correctness | `open_readwrite_async` SurrealDB bypass | Split-brain: daemon writes SQLite, query reads SurrealDB |
| 🟠 Concurrency | LSP hover blocks Tokio thread | Serialized hover requests under load |
| 🟠 Concurrency | MCP search blocks Tokio thread | Async starvation in search handler |
| 🟠 Concurrency | Semaphore permit dropped before SSE stream completes | Rate limit bypass under load |
| 🟠 Performance | Full graph loaded into memory for call chain | O(all edges) instead of O(reachable) |
| 🟡 Correctness | `try_join!` aborts all queries on single failure | No partial results returned to agents |
| 🟡 Correctness | `duration_since` panic risk in observer | Daemon crash on clock drift |
| 🟡 Performance | Unbounded symbol text sent to inference | Context window errors, excessive API cost |
| 🟡 Performance | Inotify exhaustion on large monorepos | File watcher fails silently |
| 🟡 Tech Debt | Git subprocess shell-out in coupling.rs | Process spawn overhead, fragile |
| ⚪ Cleanup | Port conflict + duplicate static_assets | Developer experience |

---

## Fix 1: Remove CORS Permissive Policy (SECURITY)

**Files:** `crates/aether-dashboard/src/lib.rs`, `crates/aether-query/src/server.rs`

**Problem:** `.layer(CorsLayer::permissive())` on localhost servers allows any website to make cross-origin requests to `127.0.0.1:9720`. A malicious site can silently `fetch('http://127.0.0.1:9720/mcp')`, query your codebase, and exfiltrate SIR summaries, project notes, and dependency graphs. This is especially dangerous for AETHER's regulated-industry target market.

**Fix:** Remove `.layer(CorsLayer::permissive())` entirely from both files. The standard Same-Origin Policy will then block cross-origin requests. If the dashboard serves from a different port and needs CORS, replace with a restrictive policy:

```rust
// INSTEAD OF:
.layer(CorsLayer::permissive())

// USE (only if dashboard needs cross-origin):
use tower_http::cors::{CorsLayer, AllowOrigin};
.layer(
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin, _| {
            origin.as_bytes().starts_with(b"http://127.0.0.1:")
                || origin.as_bytes().starts_with(b"http://localhost:")
        }))
        .allow_methods([Method::GET, Method::POST])
        .allow_headers(Any)
)
```

If both dashboard and MCP run on the same axum server (same origin), just remove CORS entirely — no layer needed.

---

## Fix 2: Fix `open_readwrite_async` SurrealDB Bypass (CORRECTNESS)

**File:** `crates/aether-mcp/src/state.rs`

**Problem:** `open_readwrite_async()` calls the synchronous `open_shared_graph()` instead of `open_shared_graph_async()`. This causes it to fall back to `SqliteGraphStore` even when config requests SurrealDB. Result: `aetherd` writes graph data to SQLite while `aether-query` (using `open_readonly_async()` which correctly uses the async variant) reads from SurrealDB. Split-brain state — the query server sees an empty graph.

**Background:** This was intentionally deferred because the async variant caused aether-mcp tests to hang. The hang needs to be investigated and resolved.

**Fix:**
```rust
// BEFORE (in open_readwrite_async):
let (graph, surreal_graph_opt) = open_shared_graph(workspace, &config, false)?;

// AFTER:
let (graph, surreal_graph_opt) = open_shared_graph_async(workspace, &config, false).await?;
```

Update the struct initialization to handle the async result:
```rust
surreal_graph: Arc::new(tokio::sync::Mutex::new(surreal_graph_opt)),
```

**Test hang investigation:** After applying this change, run `cargo test -p aether-mcp`. If tests hang:
1. Check if the test setup creates a SurrealDB instance that blocks on `.await` in a sync test context
2. Verify tests use `#[tokio::test]` not `#[test]` for async tests
3. If the hang is in SurrealKV file lock contention during parallel tests, add `--test-threads=1` for aether-mcp tests
4. Report exactly what hangs and at which test

If the hang cannot be resolved, document what you found and skip this fix.

---

## Fix 3: LSP Hover — Remove Unnecessary Async Mutex (CONCURRENCY)

**File:** `crates/aether-lsp/src/lib.rs`

**Problem:** `AetherLspBackend` wraps `SqliteStore` in `Arc<tokio::sync::Mutex<SqliteStore>>`. The hover handler acquires the async lock then calls the fully synchronous `resolve_hover_markdown_for_source()` which does heavy SQLite I/O + AST parsing. This:
1. Blocks the Tokio worker thread during the synchronous work
2. Serializes ALL hover requests through one async lock

`SqliteStore` already has an internal `std::sync::Mutex<Connection>` for thread safety.

**Fix:** Change the store field type and move blocking work to spawn_blocking:

```rust
// BEFORE (struct field):
store: Arc<tokio::sync::Mutex<SqliteStore>>,

// AFTER:
store: Arc<SqliteStore>,

// BEFORE (in hover handler):
let resolution = {
    let guard = self.store.lock().await;
    resolve_hover_markdown_for_source(..., &guard, ...)
};

// AFTER:
let store = Arc::clone(&self.store);
let resolution = tokio::task::spawn_blocking(move || {
    resolve_hover_markdown_for_source(..., &store, ...)
}).await.map_err(|e| {
    tracing::error!("Hover task panicked: {e}");
    tower_lsp::jsonrpc::Error::internal_error()
})?;
```

Update all other methods that previously called `self.store.lock().await` to either:
- Use `Arc::clone(&self.store)` directly (SqliteStore's internal mutex handles thread safety)
- Wrap in `spawn_blocking` if the operation is heavy

---

## Fix 4: MCP Search — spawn_blocking for Lexical Search (CONCURRENCY)

**File:** `crates/aether-mcp/src/lib.rs`

**Problem:** `aether_search_logic` (or equivalent) calls `lexical_search_matches()` which performs blocking SQLite queries directly on the Tokio worker thread.

**Fix:** Wrap the specific lexical search call in `spawn_blocking`:

```rust
// BEFORE:
let lexical_matches = self.lexical_search_matches(&request.query, limit)?;

// AFTER:
let query = request.query.clone();
let store = Arc::clone(&self.state.store);
let lexical_matches = tokio::task::spawn_blocking(move || {
    store.search_symbols(&query, limit)
}).await
.map_err(|e| AetherMcpError::Internal(format!("search join: {e}")))??;
```

Adapt exact method name and error type to match the code. The key: no blocking SQLite on the Tokio worker.

---

## Fix 5: Semaphore Permit Lifecycle in aether-query (CONCURRENCY)

**File:** `crates/aether-query/src/server.rs`

**Problem:** In the MCP handler, the semaphore `_permit` is dropped when the handler function returns. But `rmcp::StreamableHttpService` returns a streaming HTTP response body (HTTP/SSE per Decision #40). Axum sends the headers, then the payload streams asynchronously AFTER the handler returns. Dropping the permit frees the concurrency slot while the response is still streaming, defeating the `max_concurrent_queries` limit under load.

**Fix:** Attach the permit to the response so it lives as long as the response body:

```rust
// BEFORE:
let _permit = semaphore.acquire().await?;
let mcp_response = /* ... */;
Ok(mcp_response.into_response())

// AFTER:
let permit = semaphore.acquire().await?;
let mcp_response = /* ... */;
let mut res = mcp_response.into_response();
res.extensions_mut().insert(permit);
Ok(res)
```

If Axum doesn't keep extensions alive for the response body lifetime, an alternative approach: wrap the permit in an `Arc` captured by the response body stream. Verify which approach works and report.

---

## Fix 6: Fix `duration_since` Panic Risk (CORRECTNESS)

**File:** `crates/aetherd/src/observer.rs`

**Problem:** In the `drain_due` function (around line 144), `now.duration_since(*last_seen)` will panic if `last_seen` is ahead of `now` due to thread scheduling or OS clock adjustments. This crashes the file watcher daemon.

**Fix:** Use a guard before the subtraction:

```rust
// BEFORE:
if now.duration_since(*last_seen) >= debounce

// AFTER (option A — guard):
if *last_seen <= now && now.duration_since(*last_seen) >= debounce

// AFTER (option B — saturating, preferred):
if now.saturating_duration_since(*last_seen) >= debounce
```

Also grep the entire `observer.rs` and `lib.rs` files for any other `duration_since` calls without `saturating` and fix them all. A previous hardening pass may have fixed SOME call sites but missed this one.

---

## Fix 7: Unified Query — Connection Reuse + Partial Degradation (PERFORMANCE + CORRECTNESS)

**File:** `crates/aether-memory/src/unified_query.rs` and `crates/aether-memory/src/lib.rs` (ProjectMemoryService)

**Two sub-fixes combined because they touch the same code:**

### 7a: Inject shared stores instead of opening new connections

**Problem:** `ask()` opens a new `SqliteStore` inside each query, bypassing SharedState's connection pooling.

**Fix:** Add optional shared store fields to `ProjectMemoryService`:

```rust
// In ProjectMemoryService struct:
pub struct ProjectMemoryService {
    workspace: PathBuf,
    shared_store: Option<Arc<SqliteStore>>,
    shared_vector_store: Option<Arc<dyn VectorStore>>,
}

// Add new constructor:
impl ProjectMemoryService {
    pub fn new(workspace: impl AsRef<Path>) -> Self {
        Self {
            workspace: workspace.as_ref().to_path_buf(),
            shared_store: None,
            shared_vector_store: None,
        }
    }

    pub fn with_shared(
        workspace: impl AsRef<Path>,
        store: Arc<SqliteStore>,
        vector_store: Option<Arc<dyn VectorStore>>,
    ) -> Self {
        Self {
            workspace: workspace.as_ref().to_path_buf(),
            shared_store: Some(store),
            shared_vector_store: vector_store,
        }
    }
}
```

In `ask()`, use the shared store if available, otherwise fall back to opening a new one:

```rust
let store = match &self.shared_store {
    Some(s) => Arc::clone(s),
    None => Arc::new(SqliteStore::open(&self.workspace)?),
};
```

In `crates/aether-mcp/src/lib.rs`, update the call site to use `with_shared`:

```rust
// BEFORE:
let memory = ProjectMemoryService::new(&self.state.workspace);

// AFTER:
let memory = ProjectMemoryService::with_shared(
    &self.state.workspace,
    Arc::clone(&self.state.store),
    self.state.vector_store.clone(),
);
```

### 7b: Replace try_join! with join! for partial degradation

**Problem:** `tokio::try_join!` aborts all sub-queries if any one fails. A transient SQLite busy timeout kills the entire query instead of returning partial results.

**Fix:**
```rust
// BEFORE:
let (symbols, notes, tests) = tokio::try_join!(symbol_task, note_task, test_task)?;

// AFTER:
let (symbol_result, note_result, test_result) = tokio::join!(symbol_task, note_task, test_task);

let symbols = match symbol_result {
    Ok(Ok(s)) => s,
    Ok(Err(e)) => { tracing::warn!("Symbol search failed: {e}"); vec![] }
    Err(e) => { tracing::warn!("Symbol task panicked: {e}"); vec![] }
};
let notes = match note_result {
    Ok(Ok(n)) => n,
    Ok(Err(e)) => { tracing::warn!("Note search failed: {e}"); vec![] }
    Err(e) => { tracing::warn!("Note task panicked: {e}"); vec![] }
};
let tests = match test_result {
    Ok(Ok(t)) => t,
    Ok(Err(e)) => { tracing::warn!("Test search failed: {e}"); vec![] }
    Err(e) => { tracing::warn!("Test task panicked: {e}"); vec![] }
};
```

---

## Fix 8: SurrealQL Graph Traversal for Call Chain (PERFORMANCE)

**File:** `crates/aether-store/src/graph_surreal.rs`

**Problem:** `get_call_chain()` (and likely `list_upstream_dependency_traversal`) fetch ALL edges via `list_dependency_edges_by_kind(&["calls"])`, build an adjacency map in Rust memory, then BFS. For large monorepos this loads the entire graph just to traverse 3 hops.

**Fix:** Replace with depth-bounded iterative queries:

```rust
// Instead of fetching ALL edges:
// let edges = self.list_dependency_edges_by_kind(&["calls"]).await?;

// Use iterative frontier expansion:
let mut visited = HashSet::new();
let mut frontier = vec![start_symbol_id.to_string()];
let mut result_edges = Vec::new();

for _depth in 0..max_depth {
    if frontier.is_empty() { break; }
    // Query only neighbors of the current frontier
    let query = "SELECT * FROM depends_on WHERE source_symbol_id INSIDE $frontier AND kind = $kind";
    let mut resp = self.db.query(query)
        .bind(("frontier", &frontier))
        .bind(("kind", "calls"))
        .await?;
    let edges: Vec<DependencyEdge> = resp.take(0)?;

    let mut next_frontier = Vec::new();
    for edge in edges {
        if visited.insert(edge.target_symbol_id.clone()) {
            next_frontier.push(edge.target_symbol_id.clone());
            result_edges.push(edge);
        }
    }
    frontier = next_frontier;
}
```

Adapt the exact SurrealQL syntax to what's in `graph_surreal.rs`. The key: do NOT fetch all edges for localized queries.

Apply the same pattern to `list_upstream_dependency_traversal` if it has the same "fetch all then BFS" pattern.

Keep existing Rust BFS for global algorithms (PageRank, Louvain) in `aether-graph-algo` where loading the full graph is necessary.

---

## Fix 9: Truncate Symbol Text Before Inference (PERFORMANCE)

**File:** `crates/aetherd/src/sir_pipeline.rs`

**Problem:** `extract_symbol_source_text()` can return 50k+ characters for auto-generated arrays. This is sent directly to Gemini/Ollama without truncation, causing context window errors or massive API bills.

**Fix:**
```rust
const MAX_SYMBOL_TEXT_CHARS: usize = 10_000;

// After extracting symbol text, before passing to inference:
if symbol_text.len() > MAX_SYMBOL_TEXT_CHARS {
    let truncated = symbol_text.char_indices()
        .take_while(|(i, _)| *i < MAX_SYMBOL_TEXT_CHARS)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    tracing::warn!(
        symbol = %symbol.name,
        original_len = symbol_text.len(),
        truncated_len = truncated,
        "Symbol text truncated for inference"
    );
    symbol_text = symbol_text[..truncated].to_string();
}
```

Use `char_indices()` for safe UTF-8 truncation — direct byte slicing panics on multi-byte characters.

---

## Fix 10: Inotify Non-Recursive Initial Watch (PERFORMANCE)

**File:** `crates/aetherd/src/indexer.rs`

**Problem:** `watcher.watch(&config.workspace, RecursiveMode::Recursive)` registers inotify watches on every subdirectory including `node_modules`, `.git`, and `.aether`. This exhausts `fs.inotify.max_user_watches` on large monorepos and causes infinite index loops when AETHER writes to its own `.aether/` database.

**Fix:**
```rust
// BEFORE:
watcher.watch(&config.workspace, RecursiveMode::Recursive)?;

// AFTER:
use ignore::WalkBuilder;
for entry in WalkBuilder::new(&config.workspace)
    .hidden(true)
    .git_ignore(true)
    .build()
    .filter_map(|e| e.ok())
    .filter(|e| e.file_type().map_or(false, |ft| ft.is_dir()))
    .filter(|e| !crate::observer::is_ignored_path(e.path()))
{
    watcher.watch(entry.path(), RecursiveMode::NonRecursive)?;
}
```

**Critical:** The `is_ignored_path` filter is essential — it prevents watching `.aether/`, `.aether-snapshot/`, and any other AETHER-internal directories. Without it, writing to the database triggers inotify events that trigger re-indexing in an infinite loop.

Keep the existing dynamic directory registration code (for runtime-created directories) — it already uses `NonRecursive`.

---

## Fix 11: Remove Git Shell-out in Coupling Analysis (TECH DEBT)

**File:** `crates/aether-analysis/src/coupling.rs`

**Problem:** `changed_files_for_commit()` (around line 464) uses `Command::new("git").arg("-C")...args(["diff-tree"...])`. Phase 4.3 migrated HEAD resolution to `gix`, but this function was added in Phase 6.2 and reintroduced the shell-out pattern.

**Fix:** Replace with `gix` native equivalent:

```rust
// BEFORE:
let output = Command::new("git")
    .arg("-C").arg(&repo_path)
    .args(["diff-tree", "--no-commit-id", "--name-only", "-r", commit_hash])
    .output()?;

// AFTER:
use gix;
let repo = gix::discover(&repo_path)?;
let commit = repo.rev_parse_single(commit_hash)?.object()?.into_commit();
let tree = commit.tree()?;
let parent_tree = commit.parent_ids().next()
    .and_then(|id| id.object().ok()?.into_commit().tree().ok());

let mut changed_files = Vec::new();
// Use gix diff-tree equivalent
let changes = repo.diff_tree_to_tree(
    parent_tree.as_ref(),
    Some(&tree),
    None,
)?;
for change in changes {
    if let Some(path) = change.location() {
        changed_files.push(path.to_string());
    }
}
```

The exact `gix` API depends on the version in the workspace. Read `gix`'s actual API from the installed version. The key requirement: remove `Command::new("git")` and use `gix` instead. If the `gix` diff-tree API is too complex, an acceptable fallback is to use `gix` to get both trees and compare their entries manually.

---

## Fix 12: Cleanup — Port Conflict + Duplicate Assets (TECH DEBT)

### 12a: Change aether-query default port

**File:** `crates/aether-query/src/server.rs` (or config.rs, wherever `DEFAULT_PORT` or default bind address is defined)

```rust
// BEFORE:
const DEFAULT_PORT: u16 = 9720;
// or in config:
bind_address = "127.0.0.1:9720"

// AFTER:
const DEFAULT_PORT: u16 = 9721;
// or:
bind_address = "127.0.0.1:9721"
```

Grep for `9720` across the entire `aether-query` crate to catch all occurrences.

### 12b: Delete duplicate static_assets

```bash
rm -rf static_assets/
```

The live copy used by rust-embed is at `crates/aether-dashboard/src/static/`.

---

## Setup

Before pasting the prompt, commit the fix specification into the repo:

```bash
cd /home/rephu/projects/aether
mkdir -p docs/hardening
# Copy this file to docs/hardening/pass4_fixes.md
git add docs/hardening/pass4_fixes.md
git commit -m "Add hardening pass 4 fix specification"
git push origin main
```

Set build environment:

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=1
export PROTOC=$(which protoc)
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

---

## Codex Prompt

Copy everything between the `=====` markers into Codex:

==========BEGIN CODEX PROMPT==========

You are working in the repo root of https://github.com/rephug/aether.

This is Hardening Pass 4 implementing 12 verified fixes from a deep code review.
All fixes are surgical — no new crates, no schema changes (except removing CORS).

Read the fix specification in docs/hardening/pass4_fixes.md for full context.
The fixes are numbered 1-12. Apply them in order.

PREFLIGHT

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Fetch origin and confirm local main SHA equals origin/main SHA. If not, stop and report.

BRANCH + WORKTREE

3) Create branch feature/hardening-pass4 off main.
4) Create worktree ../aether-hardening-pass4 for that branch and switch into it.
5) Set build environment:
   - export CARGO_TARGET_DIR=/home/rephu/aether-target
   - export CARGO_BUILD_JOBS=1
   - export PROTOC=$(which protoc)
   - export TMPDIR=/home/rephu/aether-target/tmp
   - mkdir -p $TMPDIR

FIXES

Read docs/hardening/pass4_fixes.md for the detailed before/after code.

   Fix 1:  SECURITY — Remove CorsLayer::permissive() from aether-dashboard and aether-query
   Fix 2:  CORRECTNESS — Fix open_readwrite_async to use open_shared_graph_async (state.rs)
   Fix 3:  CONCURRENCY — LSP hover: Arc<SqliteStore> + spawn_blocking (aether-lsp/lib.rs)
   Fix 4:  CONCURRENCY — MCP search: spawn_blocking for lexical_search_matches (aether-mcp/lib.rs)
   Fix 5:  CONCURRENCY — Semaphore permit lifecycle in aether-query handler (server.rs)
   Fix 6:  CORRECTNESS — duration_since → saturating_duration_since in observer (observer.rs)
   Fix 7a: PERFORMANCE — Unified query: ProjectMemoryService::with_shared() constructor
   Fix 7b: CORRECTNESS — Unified query: tokio::join! partial degradation (unified_query.rs)
   Fix 8:  PERFORMANCE — SurrealQL iterative graph traversal (graph_surreal.rs)
   Fix 9:  PERFORMANCE — Truncate symbol text before inference (sir_pipeline.rs)
   Fix 10: PERFORMANCE — Inotify non-recursive watch with is_ignored_path filter (indexer.rs)
   Fix 11: TECH DEBT — Remove git shell-out, use gix (coupling.rs)
   Fix 12: CLEANUP — Port 9720→9721 in aether-query, delete static_assets/

   For each fix:
   - Read the file at the specified path first
   - Apply the BEFORE → AFTER change as described in the spec file
   - If the BEFORE code doesn't match exactly (structure may have shifted by a few
     lines), find the equivalent code and apply the same transformation
   - If a fix cannot be applied because the code structure is fundamentally different
     from what's described, SKIP that fix and report exactly what you found

FIX 1 SPECIAL NOTES (CORS)

   Search for CorsLayer::permissive in both crates. If found, remove the layer entirely.
   If the dashboard needs cross-origin access to a different-port API, replace with a
   restrictive policy allowing only 127.0.0.1/localhost origins (see spec for code).

FIX 2 SPECIAL NOTES (SurrealDB bypass)

   This fix was previously deferred because the async variant caused aether-mcp tests to
   hang. After applying the change, run cargo test -p aether-mcp. If tests hang:
   - Check if test functions use #[tokio::test] (not #[test]) for async
   - Try --test-threads=1 for SurrealKV lock contention
   - Report what hangs and at which test
   If the hang cannot be resolved, revert this fix and document exactly what happened.

FIX 3 SPECIAL NOTES (LSP hover)

   Changing store from Arc<tokio::sync::Mutex<SqliteStore>> to Arc<SqliteStore> affects
   all methods that previously called self.store.lock().await. Update EVERY call site —
   not just hover. Grep for .lock().await on the store field.

FIX 5 SPECIAL NOTES (Semaphore)

   After attaching the permit to response extensions, verify that the permit is NOT
   dropped early by running a test: the semaphore count should decrease during an
   active response and increase only after the response body completes. If Axum
   drops extensions before the body completes, report this and try wrapping the
   permit in an Arc captured by the response body stream instead.

FIX 6 SPECIAL NOTES (duration_since)

   Grep ALL of observer.rs AND lib.rs for `duration_since` calls without `saturating`.
   Fix every instance, not just the one at line 144. A previous hardening pass fixed
   some call sites but may have missed others.

FIX 7a SPECIAL NOTES (Unified query injection)

   This changes ProjectMemoryService to optionally hold Arc<SqliteStore>. The existing
   new() constructor must continue to work (for CLI/test use). Add with_shared() as a
   second constructor. Update the MCP call site to use with_shared().

FIX 11 SPECIAL NOTES (Git shell-out)

   The gix API for diff-tree varies by version. Check the gix version in workspace
   Cargo.toml. If the diff-tree API is too complex, an acceptable fallback is using
   gix to resolve both commit trees and comparing entries manually.

SCOPE GUARDS

- Do NOT modify any MCP tool schemas, CLI argument shapes, or public API contracts
  (except ProjectMemoryService gaining with_shared(), and LSP store type change).
- Do NOT add new crates or new workspace dependencies beyond what's needed for fixes.
- Do NOT touch SurrealDB schema definitions, SQLite schema migrations, or LanceDB table schemas.
- Do NOT rename any public functions or types (except as required by Fixes 3, 7a).
- Do NOT restructure any module layouts.
- If any fix cannot be applied because the code differs from what's described,
  report exactly what you found and skip that fix.

VALIDATION

6) After applying ALL fixes, run validation in dependency order:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test -p aether-core
   - cargo test -p aether-config
   - cargo test -p aether-graph-algo
   - cargo test -p aether-document
   - cargo test -p aether-store
   - cargo test -p aether-parse
   - cargo test -p aether-sir
   - cargo test -p aether-infer
   - cargo test -p aether-lsp
   - cargo test -p aether-analysis
   - cargo test -p aether-memory
   - cargo test -p aether-mcp
   - cargo test -p aether-query
   - cargo test -p aetherd
   Do NOT use cargo test --workspace (OOM risk on WSL2 with 12GB RAM).

7) If cargo clippy warns about unused imports after changes (e.g., CorsLayer,
   tokio::sync::Mutex), remove those imports and re-run clippy.

8) If aether-mcp tests hang after Fix 2, revert Fix 2, re-run validation,
   and document the hang in the output report.

9) If Fix 5 (semaphore) doesn't compile because Axum's response extensions
   don't accept the permit type, try the Arc wrapper approach from the spec.
   If neither works, skip Fix 5 and report.

COMMIT

10) Commit with message:
    Hardening pass 4: remove CORS permissive, fix SurrealDB bypass, LSP spawn_blocking, MCP search spawn_blocking, semaphore lifecycle, duration_since safety, unified query injection + partial degradation, SurrealQL graph traversal, SIR text truncation, inotify non-recursive watch, remove git shell-out, port conflict + static_assets cleanup

OUTPUT

11) Report:
    - Which fixes were applied vs. already correct vs. skipped (with reason)
    - Validation command outcomes (pass/fail per crate)
    - Any files modified outside the fix scope (should be zero)
    - Total lines changed
    - For Fix 2: did aether-mcp tests hang? What happened?
    - For Fix 5: which permit attachment approach worked?

12) Report commit SHA.
13) Provide push command:
    git -C ../aether-hardening-pass4 push -u origin feature/hardening-pass4

==========END CODEX PROMPT==========

---

## Post-Merge Sequence

After the hardening pass PR merges, run:

```bash
# Update local main
cd /home/rephu/projects/aether
git switch main
git pull --ff-only origin main
git log --oneline -3  # confirm the merge commit

# Clean up worktree and branch
git worktree remove ../aether-hardening-pass4
git branch -d feature/hardening-pass4
git worktree prune

# Verify clean state
git status --porcelain
```

---

## Appendix: Validation Audit Trail

| Fix | Source | Validated By | Ground Truth |
|-----|--------|-------------|--------------|
| 1. CORS | Gemini pushback | Gemini (had source) | `CorsLayer::permissive()` in source |
| 2. SurrealDB bypass | Gemini original + pushback | Both | Known deferred issue, split-brain risk real |
| 3. LSP hover | Gemini pushback | Gemini (had source) | Sync work under async Mutex confirmed |
| 4. MCP search | Claude + Gemini | Both | Blocking SQLite on async thread confirmed |
| 5. Semaphore | Gemini pushback | Gemini (had source) + Decision #40 (SSE) | Permit dropped before stream completes |
| 6. duration_since | Gemini pushback | Gemini (had source) | Line 144 NOT saturating |
| 7a. Unified query | Claude + Gemini pattern | Both (Gemini improved pattern) | ProjectMemoryService holds only PathBuf |
| 7b. try_join! | Claude | Claude (confirmed in source) | Line 54 try_join! confirmed |
| 8. Graph traversal | Claude + Gemini | Both | fetch-all-then-BFS confirmed |
| 9. Text truncation | Claude | Claude (confirmed in source) | No size limit before inference |
| 10. Inotify | Claude + Gemini correction | Both | Recursive + Gemini added is_ignored_path |
| 11. Git shell-out | Gemini pushback | Gemini (had source) | Command::new("git") in coupling.rs |
| 12. Port + assets | Claude | Claude | Both on 9720, duplicate dir exists |
