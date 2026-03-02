# AETHER Hardening Pass 5b — P2/P3 Performance & Correctness Fixes

You are working on the AETHER project at the repository root. This prompt contains
verified P2/P3 fixes from two independent code reviews (Gemini + Claude). Apply ALL
fixes below, then run the validation gate.

**CRITICAL: Do NOT change any public API signatures, struct field names, or trait
method signatures. All fixes are internal implementation changes only.**

**Read `docs/hardening/hardening_pass5_session_context.md` before starting.**

**Prerequisite:** Hardening Pass 5a (P0/P1 fixes) MUST be merged to main first.

---

## Fix 1: Reranker creates a new Tokio runtime per search query (P2)

**The bug:** In `rerank_rows_with_provider()`, every search query that triggers
reranking constructs and destroys a full Tokio runtime via
`tokio::runtime::Builder::new_current_thread()`. Runtime construction involves
thread-local setup, timer infrastructure, and I/O driver initialization.

### File: `crates/aetherd/src/search.rs`

Replace the per-call runtime construction (around lines 563–568) with a static
shared runtime using the same `OnceLock` pattern as `CozoGraphStore`:

```rust
// Add this import at the top of the file:
use std::sync::OnceLock;

// Add this function near the top of the file, after imports:
fn reranker_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("reranker tokio runtime should initialize")
    })
}
```

Then replace the inline runtime construction in `rerank_rows_with_provider`:

```rust
// BEFORE (around lines 563–568):
let runtime = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()
    .context("failed to build runtime for reranker")?;
let reranked = runtime
    .block_on(provider.rerank(query, &rerank_candidates, limit))
    .context("reranker request failed")?;

// AFTER:
let reranked = reranker_runtime()
    .block_on(provider.rerank(query, &rerank_candidates, limit))
    .context("reranker request failed")?;
```

---

## Fix 2: Dashboard async blocking — wrap sync analyzers in spawn_blocking (P2)

**The bug:** Multiple dashboard API handlers call synchronous analyzer methods
directly on Axum async worker threads. The `CozoGraphStore` compat shim internally
spawns an OS thread via `std::thread::scope` and blocks the Axum worker until it
returns. Under concurrent load this starves the Axum worker pool.

### File: `crates/aether-dashboard/src/api/architecture.rs`

Wrap the synchronous `DriftAnalyzer` call in `spawn_blocking`. Find the handler
function (around line 62) where `DriftAnalyzer::new()` and `.communities()` are
called:

```rust
// BEFORE (conceptual pattern):
let analyzer = DriftAnalyzer::new(&workspace, &store, &graph)?;
let communities = analyzer.communities()?;

// AFTER — wrap the sync work in spawn_blocking:
let (analyzer_result, communities_result) = tokio::task::spawn_blocking(move || {
    let analyzer = DriftAnalyzer::new(&workspace, &store, &graph)?;
    let communities = analyzer.communities()?;
    Ok::<_, anyhow::Error>((analyzer, communities))
})
.await
.context("architecture analysis task panicked")?
.context("architecture analysis failed")?;
```

Adapt the exact variable names and error types to match what the handler uses.
The key constraint is: anything that touches `CozoGraphStore` (the sync compat
shim) MUST run inside `spawn_blocking`, not directly on the Axum worker.

### File: `crates/aether-dashboard/src/api/causal_chain.rs`

Same pattern. Find the handler (around line 103) where `CausalAnalyzer` is used:

```rust
// Wrap the CausalAnalyzer construction and trace_cause call in spawn_blocking:
let trace_result = tokio::task::spawn_blocking(move || {
    let analyzer = CausalAnalyzer::new(&workspace, &store, &graph)?;
    analyzer.trace_cause(&symbol_id, lookback)
})
.await
.context("causal chain analysis task panicked")?
.context("causal chain analysis failed")?;
```

**Important:** You will need to clone any `Arc` or owned values before the
`move` closure. The workspace path, store, and graph handles typically need
cloning.

---

## Fix 3: `get_call_chain` loads entire symbol table (P2)

**The bug:** After the BFS traversal in `get_call_chain()` discovers which symbols
are in the chain, it calls `self.list_all_symbols()` to load the ENTIRE symbol
table into a HashMap, then looks up only the handful of BFS results. For a 10K
symbol codebase, this loads 10K records to use ~50.

### File: `crates/aether-store/src/graph_surreal.rs`

Find `get_call_chain` (around line 920). After the BFS loop that populates
`min_depth: HashMap<String, u32>`, replace the `list_all_symbols()` call with
a targeted query:

```rust
// BEFORE (around line 958):
let symbols = self.list_all_symbols().await?;
let by_id = symbols
    .into_iter()
    .map(|row| (row.id.clone(), row))
    .collect::<HashMap<_, _>>();

// AFTER — batch-fetch only the symbols we found in BFS:
let found_ids: Vec<String> = min_depth.keys().cloned().collect();
let mut response = self
    .db
    .query(
        r#"
        SELECT VALUE {
            id: symbol_id,
            file_path: file_path,
            language: language,
            kind: kind,
            qualified_name: qualified_name,
            signature_fingerprint: signature_fingerprint,
            last_seen_at: last_seen_at
        }
        FROM symbol
        WHERE symbol_id INSIDE $found_ids;
        "#,
    )
    .bind(("found_ids", found_ids))
    .await
    .map_err(|err| StoreError::Graph(format!("SurrealDB get_call_chain symbol fetch failed: {err}")))?;
let rows: Vec<serde_json::Value> = response.take(0).map_err(|err| {
    StoreError::Graph(format!("SurrealDB get_call_chain symbol decode failed: {err}"))
})?;
let fetched = decode_symbol_records(rows)?;
let by_id: HashMap<String, SymbolRecord> = fetched
    .into_iter()
    .map(|row| (row.id.clone(), row))
    .collect();
```

The rest of the function (building the depth-layered `Vec<Vec<SymbolRecord>>`)
stays the same — it already indexes into `by_id` by the keys from `min_depth`.

---

## Fix 4: `open_graph_store_readonly` routes Surreal through wrong type (P2)

**The bug:** `open_graph_store_readonly()` has identical code for both the `Surreal`
and `Cozo` backend arms — both call `CozoGraphStore::open_readonly()`. This works
accidentally because `CozoGraphStore` is a compat shim wrapping `SurrealGraphStore`,
but adds unnecessary overhead (sync thread-spawn) and will break if the shim is
ever removed.

### File: `crates/aether-store/src/lib.rs`

Find `open_graph_store_readonly` (around line 563). The function is sync, so it
needs the compat shim. Fix the comment to make the intent explicit, and use the
shim only for the Cozo arm:

```rust
// BEFORE (line 569):
GraphBackend::Surreal => Ok(Box::new(CozoGraphStore::open_readonly(workspace_root)?)),

// AFTER — explicit comment that we're using the sync compat shim intentionally:
GraphBackend::Surreal => {
    // Uses CozoGraphStore (sync compat shim) because this function is sync.
    // The shim wraps SurrealGraphStore internally.
    Ok(Box::new(CozoGraphStore::open_readonly(workspace_root)?))
},
```

This is a documentation-only fix since the behavior is identical. If you want to
make this function async to avoid the shim, that would be a larger refactor that
touches all callers — defer to a future pass.

---

## Fix 5: Python nested scope qualified name truncation (P2)

**The bug:** `nearest_ancestor_name()` walks up the tree and returns the FIRST
matching ancestor. For deeply nested Python classes
(`class Outer: class Inner: def method()`), it returns `Inner` and stops,
producing `module::Inner::method` instead of `module::Outer::Inner::method`.

### File: `crates/aether-parse/src/languages/python.rs`

Replace `nearest_ancestor_name` (at line 540) with a version that collects ALL
matching ancestors:

```rust
// BEFORE:
fn nearest_ancestor_name(node: Node<'_>, source: &[u8], kinds: &[&str]) -> Option<String> {
    let mut current = node.parent();
    while let Some(parent) = current {
        if kinds.iter().any(|kind| *kind == parent.kind()) {
            return named_child_text(parent, "name", source);
        }
        current = parent.parent();
    }
    None
}

// AFTER:
fn nearest_ancestor_name(node: Node<'_>, source: &[u8], kinds: &[&str]) -> Option<String> {
    let mut parts = Vec::new();
    let mut current = node.parent();
    while let Some(parent) = current {
        if kinds.iter().any(|kind| *kind == parent.kind()) {
            if let Some(name) = named_child_text(parent, "name", source) {
                parts.push(name);
            }
        }
        current = parent.parent();
    }
    if parts.is_empty() {
        return None;
    }
    // Ancestors are collected inner-to-outer; reverse for outer-to-inner
    parts.reverse();
    Some(parts.join("::"))
}
```

### Verification

Add a test that validates nested class qualified names:

```rust
#[test]
fn nested_class_method_qualified_name() {
    let source = r#"
class Outer:
    class Inner:
        def method(self):
            pass
"#;
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("set language");
    let tree = parser.parse(source, None).expect("parse");
    let root = tree.root_node();

    // Find the method node
    let outer = root.named_child(0).expect("Outer class");
    let body = outer.child_by_field_name("body").expect("outer body");
    let inner = body.named_children(&mut body.walk())
        .find(|n| n.kind() == "class_definition")
        .expect("Inner class");
    let inner_body = inner.child_by_field_name("body").expect("inner body");
    let method = inner_body.named_children(&mut inner_body.walk())
        .find(|n| n.kind() == "function_definition")
        .expect("method");

    let result = nearest_ancestor_name(method, source.as_bytes(), &["class_definition"]);
    assert_eq!(result, Some("Outer::Inner".to_owned()));
}
```

---

## Fix 6: Schema DDL runs on every SurrealGraphStore::open() (P3)

**The bug:** Even though the SurrealDB handle is cached via `cached_surreal_handle`,
every `open()` call re-executes `ensure_schema()` (~40 DDL statements). This runs
on every file change event via the SIR pipeline.

### File: `crates/aether-store/src/graph_surreal.rs`

Add a schema-initialized flag alongside the cached handle. Find the cache
infrastructure (the `OnceLock` or `static` used by `cached_surreal_handle`)
and add an `AtomicBool`:

```rust
use std::sync::atomic::{AtomicBool, Ordering};

// Add near the cache infrastructure:
static SCHEMA_INITIALIZED: AtomicBool = AtomicBool::new(false);
```

Then in `open()` (around lines 74–76), gate the schema call:

```rust
// BEFORE:
let store = Self { db };
store.ensure_schema().await?;
Ok(store)

// AFTER:
let store = Self { db };
if !SCHEMA_INITIALIZED.load(Ordering::Relaxed) {
    store.ensure_schema().await?;
    SCHEMA_INITIALIZED.store(true, Ordering::Relaxed);
}
Ok(store)
```

**Note:** `Relaxed` ordering is fine here — worst case we run `ensure_schema()`
twice on startup from two concurrent calls, which is harmless (all statements are
idempotent `IF NOT EXISTS`).

---

## Fix 7: Semaphore permit leak in SSE streaming (P3)

**The bug:** In `aether-query/src/server.rs` line 155, the concurrency semaphore
permit is stashed in `response.extensions_mut()`. For SSE/streaming responses,
extensions may be dropped when headers are sent, not when the stream completes.
This would release the permit prematurely.

### File: `crates/aether-query/src/server.rs`

Wrap the permit in an `Arc` and clone it into a response body wrapper that holds
the permit until the body is fully consumed:

```rust
// BEFORE (line 155):
let mut response = mcp_response.into_response();
response.extensions_mut().insert(Arc::new(permit));
response

// AFTER:
let response = mcp_response.into_response();
// Hold the permit in an Arc that lives as long as the response.
// For SSE, dropping the Arc (and thus the permit) happens when the
// response body is fully consumed or the connection closes.
let permit = Arc::new(permit);
let _hold = permit.clone();
let mut response = response;
response.extensions_mut().insert(permit);
response
```

This is a minimal fix. A more robust approach would use a custom `Body` wrapper
or Axum middleware, but that's a larger refactor.

---

## Fix 8: N+1 query in reranking — batch SIR blob fetch (P3)

**The bug:** `rerank_rows_with_provider` loops over candidate_rows calling
`rerank_candidate_text` which calls `store.read_sir_blob(&row.symbol_id)`
individually. For 50 candidates = 50 separate SQLite queries.

### File: `crates/aetherd/src/search.rs`

This fix depends on the complexity of refactoring `rerank_candidate_text`. If a
`read_sir_blobs_batch` method already exists on the store, use it. If not, add one.

In `crates/aether-store/src/lib.rs`, add a batch method to the `Store` trait and
`SqliteStore` implementation:

```rust
// In the Store trait:
fn read_sir_blobs_batch(&self, symbol_ids: &[&str]) -> Result<HashMap<String, Vec<u8>>, StoreError>;

// In SqliteStore:
fn read_sir_blobs_batch(&self, symbol_ids: &[&str]) -> Result<HashMap<String, Vec<u8>>, StoreError> {
    if symbol_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut results = HashMap::new();
    for id in symbol_ids {
        if let Ok(Some(blob)) = self.read_sir_blob(id) {
            results.insert(id.to_string(), blob);
        }
    }
    Ok(results)
}
```

Then in `search.rs`, pre-fetch all SIR blobs before the reranking loop and pass
them into `rerank_candidate_text` instead of fetching one at a time.

**If this refactor is too invasive**, skip it — the N+1 pattern is a performance
concern, not a correctness bug.

---

## Validation Gate

Before committing, run the full validation:

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR

cargo fmt --all --check
cargo clippy --workspace -- -D warnings
cargo test -p aether-core
cargo test -p aether-config
cargo test -p aether-store
cargo test -p aether-parse
cargo test -p aether-sir
cargo test -p aether-infer
cargo test -p aether-lsp
cargo test -p aether-analysis
cargo test -p aether-mcp
cargo test -p aether-query
cargo test -p aetherd
cargo test -p aether-dashboard
cargo test -p aether-document
cargo test -p aether-memory
cargo test -p aether-graph-algo
```

All tests MUST pass. The new `nested_class_method_qualified_name` test in Fix 5
MUST pass.

If `cargo clippy` warns about unused imports after changes, remove them.

If Fix 8 (batch SIR blob) causes trait incompatibilities or requires touching
too many call sites, SKIP it and note it in the commit message as deferred.

---

## Commit Message

```
fix: P2/P3 hardening — reranker runtime, spawn_blocking, call chain, Python scopes

- Reuse static Tokio runtime for reranker instead of creating per-call (P2)
- Wrap dashboard sync analyzers in spawn_blocking (P2)
- Batch-fetch symbols in get_call_chain instead of loading all (P2)
- Document open_graph_store_readonly Surreal/Cozo identity (P2)
- Fix Python nested class qualified name truncation (P2)
- Gate SurrealDB schema DDL to run once per process (P3)
- Hold semaphore permit through SSE response lifetime (P3)
- Batch SIR blob fetch in reranking [if applied] (P3)
```
