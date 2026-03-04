# AETHER Hardening Pass 7b — P2/P3 Performance & Async Fixes

You are working on the AETHER project at the repository root. This prompt contains
verified P2/P3 fixes from two independent Gemini deep code reviews, cross-validated
by Claude against the actual source. Apply ALL fixes below, then run the validation
gate.

**CRITICAL: Do NOT change any public API signatures, struct field names, or trait
method signatures. All fixes are internal implementation changes only.**

**Read `docs/hardening/hardening_pass7_session_context.md` before starting.**

**Prerequisite:** Hardening Pass 7a (P0/P1 fixes) MUST be merged to main first.

---

## Preflight

```bash
git status --porcelain
# Must be clean. If not, stop and report dirty files.

git fetch origin
git pull --ff-only origin main
```

## Branch + Worktree

```bash
git worktree add ../aether-hardening-pass7b feature/hardening-pass7b -b feature/hardening-pass7b
cd ../aether-hardening-pass7b
```

## Build Environment

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

---

## Fix B1: O(N) Full Graph Load for Single Edge Check (P2)

**The bug:** `has_dependency_between_files` loads the entire symbol table and all
dependency edges into memory, constructs HashMaps, and iterates them to answer a
single boolean question. This is called per file-pair in the coupling analysis loop
(`coupling.rs:386`), so a project with 10,000 symbols and 15,000 edges loads the
full graph hundreds of times.

**Why it matters:** OOM crashes and extreme latency during coupling and blast radius
analysis on medium-to-large codebases.

### File: `crates/aether-store/src/graph_surreal.rs`

Replace the body of `has_dependency_between_files` (around line 272). Keep the
existing signature and the empty/trim guards:

```rust
// BEFORE (around lines 282-298):
        let symbols = self.list_all_symbols().await?;
        let file_by_symbol = symbols
            .into_iter()
            .map(|row| (row.id, row.file_path))
            .collect::<HashMap<_, _>>();
        let edges = self.list_dependency_edges_raw().await?;
        Ok(edges.into_iter().any(|edge| {
            let Some(source_file) = file_by_symbol.get(edge.source_id.as_str()) else {
                return false;
            };
            let Some(target_file) = file_by_symbol.get(edge.target_id.as_str()) else {
                return false;
            };
            (source_file == file_a && target_file == file_b)
                || (source_file == file_b && target_file == file_a)
        }))

// AFTER:
        let mut response = self
            .db
            .query(
                r#"
                SELECT VALUE id
                FROM depends_on
                WHERE (in.file_path = $file_a AND out.file_path = $file_b)
                   OR (in.file_path = $file_b AND out.file_path = $file_a)
                LIMIT 1;
                "#,
            )
            .bind(("file_a", file_a.to_owned()))
            .bind(("file_b", file_b.to_owned()))
            .await
            .map_err(|err| {
                StoreError::Graph(format!("SurrealDB has_dependency query failed: {err}"))
            })?;

        let rows: Vec<serde_json::Value> = response.take(0).map_err(|err| {
            StoreError::Graph(format!("SurrealDB has_dependency decode failed: {err}"))
        })?;

        Ok(!rows.is_empty())
```

**NOTE ON SCHEMA:** Check the `ensure_schema` function in the same file to confirm
that `depends_on` uses record references where `in` and `out` are symbol records
with `file_path` fields accessible via dot notation (`in.file_path`). In SurrealDB,
`RELATE symbol:a->depends_on->symbol:b` stores `in = symbol:a, out = symbol:b`,
and record reference traversal (`in.file_path`) resolves the linked record's field.

If `in.file_path` does not resolve correctly (because symbols are stored in a
different table name, or the `depends_on` schema stores plain string IDs rather
than record links), fall back to a subquery approach:

```rust
        // FALLBACK (only if in.file_path doesn't resolve):
        let mut response = self
            .db
            .query(
                r#"
                SELECT VALUE id FROM depends_on
                WHERE ((SELECT VALUE file_path FROM symbol WHERE id = in)[0] = $file_a
                   AND (SELECT VALUE file_path FROM symbol WHERE id = out)[0] = $file_b)
                   OR ((SELECT VALUE file_path FROM symbol WHERE id = in)[0] = $file_b
                   AND (SELECT VALUE file_path FROM symbol WHERE id = out)[0] = $file_a)
                LIMIT 1;
                "#,
            )
            .bind(("file_a", file_a.to_owned()))
            .bind(("file_b", file_b.to_owned()))
            .await
            .map_err(|err| {
                StoreError::Graph(format!("SurrealDB has_dependency query failed: {err}"))
            })?;
```

### Verification

The existing test `has_dependency_between_files` (around line 1331) MUST still pass.
Run:
```bash
cargo test -p aether-store has_dependency_between_files
```

---

## Fix B2: Unified Query Post-Processing Blocks Tokio Thread (P2)

**The bug:** In `pub async fn ask()`, after the three `spawn_blocking` search
phases complete (lines 121-170), the merge/enrich/increment post-processing runs
synchronously on the Tokio worker thread. This includes `rank_coupling_candidates`
(opens CozoDB, runs Datalog), `enrich_symbol_snippets` (reads SIR blobs from
SQLite), and `increment_access_from_results` (writes to SQLite). Under concurrent
MCP load, this starves the Tokio executor.

**Why it matters:** Server deadlock under concurrent `aether_ask` calls.

### File: `crates/aether-memory/src/unified_query.rs`

Find the post-processing section starting at `let coupling_candidates =` (around
line 270). Wrap everything from there through `increment_access_from_results` in
`tokio::task::spawn_blocking`:

```rust
// BEFORE (around lines 270-293):
        let coupling_candidates = if include.contains(&AskInclude::Coupling) {
            rank_coupling_candidates(self.workspace(), symbol_candidates.as_slice())?
        } else {
            Vec::new()
        };
        let symbol_candidates_for_results = if include.contains(&AskInclude::Symbols) {
            symbol_candidates
        } else {
            Vec::new()
        };

        let mut result = merge_candidates(
            now_ms,
            limit,
            symbol_candidates_for_results,
            note_candidates,
            test_candidates,
            coupling_candidates,
        );
        result.query = query.to_owned();

        enrich_symbol_snippets(&store, result.results.as_mut_slice())?;
        increment_access_from_results(&store, result.results.as_mut_slice(), now_ms)?;

        Ok(result)

// AFTER:
        let workspace = self.workspace().to_path_buf();
        let store_clone = store.clone();
        let query_owned = query.to_owned();

        let result = tokio::task::spawn_blocking(move || -> Result<AskQueryResult, MemoryError> {
            let coupling_candidates = if include.contains(&AskInclude::Coupling) {
                rank_coupling_candidates(&workspace, symbol_candidates.as_slice())?
            } else {
                Vec::new()
            };
            let symbol_candidates_for_results = if include.contains(&AskInclude::Symbols) {
                symbol_candidates
            } else {
                Vec::new()
            };

            let mut res = merge_candidates(
                now_ms,
                limit,
                symbol_candidates_for_results,
                note_candidates,
                test_candidates,
                coupling_candidates,
            );
            res.query = query_owned;

            enrich_symbol_snippets(&store_clone, res.results.as_mut_slice())?;
            increment_access_from_results(&store_clone, res.results.as_mut_slice(), now_ms)?;

            Ok(res)
        })
        .await
        .map_err(|err| MemoryError::InvalidInput(format!("spawn_blocking failure: {err}")))??;

        Ok(result)
```

**KEY DETAILS:**
- `self.workspace()` returns `&Path` — clone to `PathBuf` for the move closure
- `store` is `Arc<SqliteStore>` — clone the Arc (cheap)
- `symbol_candidates`, `note_candidates`, `test_candidates` are `Vec`s — they move in
- `include` is a `BTreeSet` — moves in
- `now_ms` and `limit` are `i64`/`usize` — Copy types, move freely
- Double `?` on `.await`: outer `?` unwraps the JoinError, inner `?` unwraps the MemoryError

**Verify:** Check that `rank_coupling_candidates` takes `&Path` (not `&Self`). If
it takes `self.workspace()` as `&Path`, the `PathBuf` deref will satisfy it. If it
takes `&Self`, you'll need to restructure — report what you find.

---

## Fix B3: Dashboard Search API Sync Blocking on Async Thread (P2)

**The bug:** `load_search_data` (async function) runs `search_symbols`,
`load_dependency_algo_edges`, `latest_drift_score_by_symbol`, `test_count_by_symbol`,
`symbols_with_sir`, and `load_symbols` synchronously on the Axum async worker thread.
Pass 6a fixed `run_async_with_timeout` to not create a new runtime, but the callers
still block the worker.

### File: `crates/aether-dashboard/src/api/search.rs`

Find `load_search_data` (around line 76). Wrap the synchronous SQLite operations
in `tokio::task::spawn_blocking`:

```rust
// BEFORE (around lines 96-110):
    let results = shared
        .store
        .search_symbols(&q, limit)
        .map_err(|err| err.to_string())?;

    let edges = common::load_dependency_algo_edges(shared)?;
    let pagerank = common::pagerank_map(shared, &edges).await;
    let drift_map = common::latest_drift_score_by_symbol(shared).unwrap_or_default();
    let test_map = common::test_count_by_symbol(shared).unwrap_or_default();
    let sir_symbols = common::symbols_with_sir(shared).unwrap_or_default();

// AFTER:
    let shared_sync = shared.clone();
    let q_sync = q.clone();
    let (results, edges, drift_map, test_map, sir_symbols) =
        tokio::task::spawn_blocking(move || {
            let results = shared_sync
                .store
                .search_symbols(&q_sync, limit)
                .map_err(|err| err.to_string())?;
            let edges = common::load_dependency_algo_edges(&shared_sync)?;
            let drift_map =
                common::latest_drift_score_by_symbol(&shared_sync).unwrap_or_default();
            let test_map = common::test_count_by_symbol(&shared_sync).unwrap_or_default();
            let sir_symbols = common::symbols_with_sir(&shared_sync).unwrap_or_default();
            Ok::<_, String>((results, edges, drift_map, test_map, sir_symbols))
        })
        .await
        .map_err(|err| err.to_string())??;

    let pagerank = common::pagerank_map(shared, &edges).await;
```

**NOTE:** Keep `pagerank_map` OUTSIDE the `spawn_blocking` block — it has its own
async SurrealDB path and must run on the async thread. Only the synchronous SQLite
operations go inside.

Also find `load_symbols` (around line 107, may be a few lines below the block above):

```rust
    let symbol_records = common::load_symbols(shared)
```

If this is also synchronous (calls SQLite), include it inside the same
`spawn_blocking` block. Check whether `shared` needs to be re-cloned for this.

---

## Fix B4: Dashboard Common Graph Algos Block Tokio (P2)

**The bug:** In `common.rs`, `louvain_map`, `pagerank_map`, and
`connected_components_vec` have SurrealDB-first paths (async, correct) with
CozoDB/fallback paths that run CPU-heavy graph algorithms synchronously on the
async thread. `louvain_communities` and `page_rank` are O(V+E) iterative
algorithms that block the worker for hundreds of milliseconds on large graphs.

### File: `crates/aether-dashboard/src/api/common.rs`

**Step B4a:** Wrap the `louvain_map` fallback (around line 148):

```rust
// BEFORE (around line 148-151):
    louvain_communities(fallback_edges)
        .into_iter()
        .map(|(id, community)| (id, community as i64))
        .collect()

// AFTER:
    let edges = fallback_edges.to_vec();
    tokio::task::spawn_blocking(move || {
        louvain_communities(&edges)
            .into_iter()
            .map(|(id, community)| (id, community as i64))
            .collect()
    })
    .await
    .unwrap_or_default()
```

**Step B4b:** Wrap the `pagerank_map` fallback. Find the fallback path (around
line 131):

```rust
// BEFORE:
    page_rank(fallback_edges, 0.85, 20)
        .into_iter()
        .collect()

// AFTER:
    let edges = fallback_edges.to_vec();
    tokio::task::spawn_blocking(move || {
        page_rank(&edges, 0.85, 20)
            .into_iter()
            .collect()
    })
    .await
    .unwrap_or_default()
```

**Step B4c:** Wrap the `connected_components_vec` fallback (around line 166):

```rust
// BEFORE:
    connected_components(fallback_edges)

// AFTER:
    let edges = fallback_edges.to_vec();
    tokio::task::spawn_blocking(move || connected_components(&edges))
        .await
        .unwrap_or_default()
```

**NOTE:** All three functions take `&[GraphAlgorithmEdge]`. The `.to_vec()` clone
is necessary because `spawn_blocking` requires `'static`. `GraphAlgorithmEdge`
must implement `Clone` — verify this. If it doesn't implement `Clone`, check if
you can derive it, or report and skip.

**Verify:** Check that the SurrealDB-first paths (the `if` branches above each
fallback) are async and correct — they should already use `.await` and not need
`spawn_blocking`.

---

## Scope Guard

- Do NOT modify any MCP tool schemas, CLI argument shapes, or public API contracts.
- Do NOT add new crates or new workspace dependencies.
- Do NOT touch SQLite schema migrations, SurrealDB schema definitions, or LanceDB table schemas.
- Do NOT rename any public functions or types.
- Do NOT change any SurrealDB schema DDL in `ensure_schema`.
- If any fix cannot be applied because the code structure differs from what's
  described, report exactly what you found and skip that fix.

---

## Validation Gate

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

All tests MUST pass. The existing `has_dependency_between_files` test in Fix B1
MUST still pass after the SurrealQL rewrite.

If `cargo clippy` warns about unused imports (e.g., `HashMap` after removing the
in-memory approach in B1), remove them.

If Fix B1's SurrealQL query using `in.file_path` doesn't work with the current
schema, try the subquery fallback from the spec. If neither works, report exactly
what error SurrealDB returns.

---

## Commit Message

```
fix: P2/P3 hardening pass 7b — SurrealQL edge query, spawn_blocking for unified query + dashboard

- Replace O(N) has_dependency_between_files with targeted SurrealQL query (P2 — OOM fix)
- Wrap unified query post-processing in spawn_blocking (P2 — deadlock fix)
- Wrap dashboard search SQLite ops in spawn_blocking (P2 — latency fix)
- Wrap graph algo fallbacks (louvain, pagerank, components) in spawn_blocking (P2)
```

---

## Post-Fix Commands

```bash
git add -A
git commit -m "fix: P2/P3 hardening pass 7b — SurrealQL edge query, spawn_blocking for unified query + dashboard"
git push origin feature/hardening-pass7b
gh pr create --base main --head feature/hardening-pass7b \
  --title "Hardening pass 7b: P2/P3 performance fixes from Gemini deep review" \
  --body "4 fixes: O(N) graph load replaced with SurrealQL, unified query deadlock, dashboard search blocking, graph algo worker starvation."
```

After merge:

```bash
cd /home/rephu/projects/aether
git switch main
git pull --ff-only
git worktree remove ../aether-hardening-pass7b
git branch -d feature/hardening-pass7b
git worktree prune
```
