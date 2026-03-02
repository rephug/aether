# AETHER Hardening Pass 5a — P0/P1 Critical Fixes

You are working on the AETHER project at the repository root. This prompt contains
verified bug fixes from two independent code reviews (Gemini + Claude). Each fix has
been validated against the actual source at commit e74a0bb. Apply ALL fixes below,
then run the validation gate.

**CRITICAL: Do NOT change any public API signatures, struct field names, or trait
method signatures. All fixes are internal implementation changes only.**

**Read `docs/hardening/hardening_pass5_session_context.md` before starting.**

---

## Fix 1: SurrealDB edge direction — DELETE/RELATE mismatch (P0)

**The bug:** In `upsert_edge()`, `RELATE $src->depends_on->$dst` creates an edge
where SurrealDB sets `in=$src, out=$dst`. But the dedup DELETE on the line above
checks `WHERE out=$src AND in=$dst` — the reverse direction. The DELETE never
matches, so duplicate edges silently accumulate on every re-index.

**Why it matters:** Every re-index of a file doubles the edge count for that file's
symbols. Over time this corrupts PageRank, community detection, and dependency
traversal with inflated weights.

### File: `crates/aether-store/src/graph_surreal.rs`

At line 737, fix the DELETE to match RELATE's direction. In SurrealDB,
`RELATE $src->edge->$dst` stores `in=$src, out=$dst`:

```rust
// BEFORE (line 737):
DELETE depends_on WHERE out = $src AND in = $dst AND file_path = $file_path AND edge_kind = $edge_kind;

// AFTER:
DELETE depends_on WHERE in = $src AND out = $dst AND file_path = $file_path AND edge_kind = $edge_kind;
```

That's the only change needed — swap `out = $src AND in = $dst` to
`in = $src AND out = $dst`.

### Verification

Add a test to the existing `mod tests` block in the same file that proves the
dedup DELETE actually removes the old edge before re-creating:

```rust
#[tokio::test]
async fn upsert_edge_deduplicates_on_reindex() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".aether")).expect("mkdir");
    let graph = SurrealGraphStore::open(&workspace).await.expect("open");

    let sym_a = symbol("alpha", "alpha", "src/a.rs");
    let sym_b = symbol("beta", "beta", "src/b.rs");
    graph.upsert_symbol_node(&sym_a).await.expect("upsert a");
    graph.upsert_symbol_node(&sym_b).await.expect("upsert b");

    let edge = ResolvedEdge {
        source_id: "alpha".to_owned(),
        target_id: "beta".to_owned(),
        edge_kind: EdgeKind::Calls,
        file_path: "src/a.rs".to_owned(),
    };

    // Upsert the same edge three times (simulating three re-indexes)
    graph.upsert_edge(&edge).await.expect("upsert 1");
    graph.upsert_edge(&edge).await.expect("upsert 2");
    graph.upsert_edge(&edge).await.expect("upsert 3");

    // There should be exactly ONE edge, not three
    let edges = graph
        .list_dependency_edges_by_kind(&["calls"])
        .await
        .expect("list edges");
    let matching = edges
        .iter()
        .filter(|e| e.source_id == "alpha" && e.target_id == "beta")
        .count();
    assert_eq!(matching, 1, "expected exactly one edge after three upserts, got {matching}");
}
```

---

## Fix 2: Vector search limit truncation (P1)

**The bug:** `search_nearest()` in `vector.rs` passes `limit` directly to LanceDB's
`.limit(limit)`. Then `semantic_search()` in `search.rs` filters results by
threshold AFTER the limit was already applied. If 15 of 20 results are below
threshold, you get 5 results even though positions 21–35 might have passed.

**Why it matters:** Users get fewer semantic search results than they should,
especially with strict thresholds.

### File: `crates/aetherd/src/search.rs`

At line 310, where `limit` is passed to `search_nearest`, apply a 3× over-fetch
multiplier so threshold filtering has more candidates to work with. The final
limit is applied downstream in `fuse_hybrid_results`.

Find the call to `vector_store.search_nearest()` (around line 306):

```rust
// BEFORE:
let matches = runtime
    .block_on(vector_store.search_nearest(
        &query_embedding,
        &loaded.provider_name,
        &loaded.model_name,
        limit,
    ))
    .context("failed to run semantic symbol search")?;

// AFTER:
let semantic_fetch_limit = limit.saturating_mul(3).max(30);
let matches = runtime
    .block_on(vector_store.search_nearest(
        &query_embedding,
        &loaded.provider_name,
        &loaded.model_name,
        semantic_fetch_limit,
    ))
    .context("failed to run semantic symbol search")?;
```

---

## Fix 3: No HTTP timeouts on inference clients (P1)

**The bug:** Every `reqwest::Client` in `aether-infer` is constructed via
`reqwest::Client::new()` with no timeout. A stalled API endpoint hangs the
entire SIR pipeline thread indefinitely.

### File: `crates/aether-infer/src/lib.rs`

**Step 3a:** Add a helper function near the top of the file (after the existing
constants, around line 30):

```rust
/// Build a reqwest client with sensible timeouts for inference requests.
fn inference_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Build a reqwest client with shorter timeouts for management calls
/// (model listing, health checks).
fn management_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}
```

**Step 3b:** Replace all `reqwest::Client::new()` calls in provider constructors
with `inference_http_client()`. There are four:

1. `GeminiProvider::from_env_key` / `GeminiProvider::new` (around line 203):
   ```rust
   // BEFORE:  client: reqwest::Client::new(),
   // AFTER:   client: inference_http_client(),
   ```

2. `Qwen3LocalProvider::new` (around line 274):
   ```rust
   // BEFORE:  client: reqwest::Client::new(),
   // AFTER:   client: inference_http_client(),
   ```

3. `OpenAiCompatProvider::new` (around line 368):
   ```rust
   // BEFORE:  client: reqwest::Client::new(),
   // AFTER:   client: inference_http_client(),
   ```

4. `Qwen3LocalEmbeddingProvider::new` (around line 495):
   ```rust
   // BEFORE:  client: reqwest::Client::new(),
   // AFTER:   client: inference_http_client(),
   ```

**Step 3c:** Replace `reqwest::Client::new()` in management/one-shot calls with
`management_http_client()`. There are three:

1. `fetch_ollama_tags` (around line 563):
   ```rust
   // BEFORE:  let response_value = reqwest::Client::new()
   // AFTER:   let response_value = management_http_client()
   ```

2. `pull_ollama_model_with_progress` (around line 581):
   ```rust
   // BEFORE:  let mut response = reqwest::Client::new()
   // AFTER:   let mut response = management_http_client()
   ```

3–4. The Gemini embedding calls `embed_with_gemini` and `embed_with_gemini_batch`
(around lines 1164 and 1186) — replace both:
   ```rust
   // BEFORE:  let response_value: Value = reqwest::Client::new()
   // AFTER:   let response_value: Value = inference_http_client()
   ```

**Step 3d:** In `crates/aether-infer/src/reranker/cohere.rs`, replace the client
in `CohereReranker::new` (around line 28):
```rust
// BEFORE:  client: reqwest::Client::new(),
// AFTER:   client: crate::inference_http_client(),
```

Make sure `inference_http_client` is `pub(crate)` so the reranker submodule can
access it. Change the function visibility:

```rust
// BEFORE:
fn inference_http_client() -> reqwest::Client {

// AFTER:
pub(crate) fn inference_http_client() -> reqwest::Client {
```

And `management_http_client` as well:
```rust
pub(crate) fn management_http_client() -> reqwest::Client {
```

**Step 3e:** In `crates/aether-query/src/main.rs` (around line 100), replace the
health-check client:
```rust
// BEFORE:  let client = reqwest::Client::new();
// AFTER:   let client = reqwest::Client::builder()
//              .connect_timeout(std::time::Duration::from_secs(5))
//              .timeout(std::time::Duration::from_secs(10))
//              .build()
//              .unwrap_or_else(|_| reqwest::Client::new());
```

---

## Fix 4: README documentation — remove fictional CLI flags and fix MCP tool list (P1)

### File: `README.md`

**Step 4a:** Remove ALL references to `--mcp` flag. There are four occurrences
(around lines 50, 59, 267, 268). The MCP server is the separate `aether-mcp`
binary started via `aether-query`, not a flag on `aetherd`.

Replace quickstart MCP references with the correct invocation. For example, if
the README says:
```
aetherd --workspace /path/to/project --mcp
```
Replace with:
```
aether-query --workspace /path/to/project
```

**Step 4b:** Remove or correct references to these nonexistent CLI flags:
- `--index` → remove or replace with the bare `aetherd` command (indexing is the default mode)
- `--from` / `--to` → remove (drift-report uses `--lookback` and `--min-drift`)
- `--scope` → remove (blast-radius uses positional `<file>` argument)
- `--all` / `--check` → remove (health uses `--limit` and `--min-risk`)
- `--format json|text` → remove (no format flag exists)
- `--regenerate-sir` → remove (no such flag)
- `--transport` → remove (no such flag; aether-query always uses HTTP/SSE)

**Step 4c:** Fix the MCP tools table. Remove these rows (tools don't exist):
- `aether_snapshot_intent`
- `aether_verify_intent`

Add these rows (tools ARE implemented but undocumented):

| Tool | Description |
|------|-------------|
| `aether_call_chain` | Trace outgoing call chains from a symbol up to N depth levels |
| `aether_dependencies` | List callers and dependencies for a symbol |
| `aether_status` | Report workspace indexing status, SIR coverage, and provider info |
| `aether_symbol_timeline` | Show SIR version history for a symbol across commits |

**Step 4d:** Rename `aether_verify_intent` to `aether_verify` in the table if it
exists, since the implementation is `aether_verify_logic` (not `verify_intent`).

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

All tests MUST pass. The new `upsert_edge_deduplicates_on_reindex` test in Fix 1
MUST pass — it proves the edge direction fix works.

If `cargo clippy` warns about unused imports after adding timeout helpers,
remove the unused imports.

---

## Commit Message

```
fix: P0/P1 hardening — edge dedup, vector search, HTTP timeouts, README

- Fix SurrealDB edge DELETE/RELATE direction mismatch (P0 data corruption)
- Over-fetch vector search results before threshold filtering (P1)
- Add HTTP connect/request timeouts to all inference clients (P1)
- Remove fictional CLI flags and fix MCP tool list in README (P1)
```
