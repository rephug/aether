# AETHER Hardening Pass 6a — P0/P1 Critical Fixes

You are working on the AETHER project at the repository root. This prompt contains
verified bug fixes from a Gemini deep code review. Each fix has been validated
against the actual source on main. Apply ALL fixes below, then run the validation gate.

**CRITICAL: Do NOT change any public API signatures, struct field names, or trait
method signatures. All fixes are internal implementation changes only.**

**Read `docs/hardening/hardening_pass6_session_context.md` before starting.**

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
git worktree add ../aether-hardening-pass6a feature/hardening-pass6a -b feature/hardening-pass6a
cd ../aether-hardening-pass6a
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

## Fix A1: SIR Regeneration Loop — Stale Timestamp (P0)

**The bug:** When the LLM generates the same SIR hash as the latest version,
`record_sir_version_if_changed` returns `updated_at: latest_created_at` (the old
DB timestamp) instead of `created_at` (the current attempt time). The pipeline
writes this old timestamp to `sir_meta.updated_at`. On next restart, the freshness
check at `sir_pipeline.rs:457` compares `source_modified_at_ms < meta.updated_at * 1000`
— the file mtime is still newer than the old timestamp, so the symbol is sent to
the LLM again. This loops on every daemon restart, silently burning API tokens.

**Why it matters:** Every unchanged symbol triggers an LLM call on every restart.
On a 500-symbol project, this is 500 wasted API calls per daemon start.

### File: `crates/aether-store/src/lib.rs`

Find the `record_sir_version_if_changed` function (around line 1562). In the
branch where `latest_hash == sir_hash` (around line 1600), change:

```rust
// BEFORE (around line 1600-1604):
if latest_hash == sir_hash {
    SirVersionWriteResult {
        version: latest_version,
        updated_at: latest_created_at,
        changed: false,
    }
}

// AFTER:
if latest_hash == sir_hash {
    SirVersionWriteResult {
        version: latest_version,
        updated_at: created_at,  // Use current attempt time, not old DB time
        changed: false,
    }
}
```

That's a one-field change: `latest_created_at` → `created_at`.

**NOTE:** The `created_at` variable is the function parameter (the current attempt
timestamp). It is already in scope — it's declared on line 1572 as
`let created_at = created_at.max(0);`.

### Verification

Add a unit test to the existing `#[cfg(test)] mod tests` block at the bottom
of the same file:

```rust
#[test]
fn record_sir_version_unchanged_hash_advances_timestamp() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open");
    let symbol_id = "test::unchanged_ts";
    let sir_hash = "abc123";
    let sir_json = r#"{"intent":"test"}"#;

    // First write at time=1000
    let first = store
        .record_sir_version_if_changed(symbol_id, sir_hash, "test", "test", sir_json, 1000, None)
        .expect("first write");
    assert!(first.changed);
    assert_eq!(first.updated_at, 1000);

    // Second write with SAME hash at time=2000
    let second = store
        .record_sir_version_if_changed(symbol_id, sir_hash, "test", "test", sir_json, 2000, None)
        .expect("second write");
    assert!(!second.changed);
    // Critical assertion: updated_at must advance to 2000, not stay at 1000
    assert_eq!(second.updated_at, 2000);
}
```

---

## Fix A2: Whole-File Hallucination Fallback (P0)

**The bug:** When tree-sitter byte range extraction fails for a symbol, the code
falls back to passing the *entire file source* to the LLM. The LLM generates a
merged SIR for everything in the file, permanently corrupting that symbol's entry.

### File: `crates/aetherd/src/sir_pipeline.rs`

Find the `build_job` function (around line 866). Change the fallback from the
entire file to an error that skips the symbol:

```rust
// BEFORE (around line 871-873):
let mut symbol_text = extract_symbol_source_text(&source, symbol.range)
    .filter(|text| !text.trim().is_empty())
    .unwrap_or(source);

// AFTER:
let mut symbol_text = extract_symbol_source_text(&source, symbol.range)
    .filter(|text| !text.trim().is_empty())
    .ok_or_else(|| {
        anyhow::anyhow!(
            "tree-sitter extraction failed for {} in {} — skipping to avoid whole-file hallucination",
            symbol.qualified_name,
            symbol.file_path,
        )
    })?;
```

This changes the return type behavior: when extraction fails, `build_job` returns
`Err`, the caller logs a warning, and the symbol is skipped. This is safe because
the caller (`process_symbols_for_file`, around line 230) already handles `Err`
from `build_job` gracefully — it logs the error and continues with remaining symbols.

**Verify:** Check that the caller of `build_job` handles `Err` results without
panicking (it should already use `match` or `?` propagation with logging).

---

## Fix A3: Time Machine 1970 — Timestamp Unit Mismatch (P0)

**The bug:** `sir_history.created_at` stores timestamps in **seconds** (set via
`unix_timestamp_secs()` in `sir_pipeline.rs`). The Time Machine UI passes `at_ms`
in **milliseconds**. Three queries compare seconds directly against milliseconds.
Because milliseconds are 1000× larger, conditions like `MIN(created_at) <= at_ms`
are always true, and the Time Machine just shows the current state at all times.

### File: `crates/aether-dashboard/src/api/time_machine.rs`

**Step A3a:** Fix `read_time_range()` (around line 236).

The `sir_history` path returns raw seconds. The `symbols` fallback already does
`* 1000`. Fix the `sir_history` path to match:

```rust
// BEFORE (around line 238):
"SELECT MIN(created_at), MAX(created_at) FROM sir_history",

// AFTER:
"SELECT MIN(created_at) * 1000, MAX(created_at) * 1000 FROM sir_history",
```

**Step A3b:** Fix the symbol existence query (around line 127).

```rust
// BEFORE (around line 130):
HAVING MIN(created_at) <= ?1

// AFTER:
HAVING (MIN(created_at) * 1000) <= ?1
```

**Step A3c:** Fix `events_around_timestamp` — the `sir_history` subquery (around line 331).

The `drift_results` query (lines 300-317) is correct — `detected_at` already
stores milliseconds. Only the `sir_history` query needs fixing:

```rust
// BEFORE (around line 335):
HAVING first_at BETWEEN ?1 AND ?2

// AFTER:
HAVING (first_at * 1000) BETWEEN ?1 AND ?2
```

Do NOT change the `drift_results` query — it is already correct.

---

## Fix A4: Unbounded Dashboard Caches — Memory Leak (P1)

**The bug:** `DashboardCaches` uses `HashMap<i64, String>` and
`HashMap<i64, LayerAssignmentsCache>` keyed by `sir_count`. As the indexer runs
and `sir_count` increments, new entries are inserted but old entries are never
removed. On a long-running daemon, this grows indefinitely.

### File: `crates/aether-dashboard/src/state.rs`

**Step A4a:** Change the struct definition (around line 15):

```rust
// BEFORE:
#[derive(Debug, Default)]
pub struct DashboardCaches {
    pub project_summary_by_sir: Mutex<HashMap<i64, String>>,
    pub layer_assignments_by_sir: Mutex<HashMap<i64, LayerAssignmentsCache>>,
}

// AFTER:
#[derive(Debug, Default)]
pub struct DashboardCaches {
    pub project_summary: Mutex<Option<(i64, String)>>,
    pub layer_assignments: Mutex<Option<(i64, LayerAssignmentsCache)>>,
}
```

Remove the `use std::collections::HashMap;` import if it becomes unused after
this change (check if HashMap is used elsewhere in the file first).

**Step A4b:** Update the cache consumer in `crates/aether-dashboard/src/api/anatomy.rs`.

Find the `project_summary_cached` function (around line 348) and the
`layer_assignments` function (around line 370):

```rust
// BEFORE — project_summary_cached (around line 356):
let mut cache = shared.caches.project_summary_by_sir.lock()
    .map_err(|err| format!("project summary cache lock poisoned: {err}"))?;
if let Some(cached) = cache.get(&sir_count) {
    return Ok(cached.clone());
}
let summary = compose_project_summary(sir_intents, lang, deps);
cache.insert(sir_count, summary.clone());
Ok(summary)

// AFTER:
let mut cache = shared.caches.project_summary.lock()
    .map_err(|err| format!("project summary cache lock poisoned: {err}"))?;
if let Some((cached_count, ref cached)) = *cache {
    if cached_count == sir_count {
        return Ok(cached.clone());
    }
}
let summary = compose_project_summary(sir_intents, lang, deps);
*cache = Some((sir_count, summary.clone()));
Ok(summary)
```

```rust
// BEFORE — layer_assignments (around line 378):
let mut cache = shared.caches.layer_assignments_by_sir.lock()
    .map_err(|err| format!("layer assignment cache lock poisoned: {err}"))?;
if let Some(cached) = cache.get(&sir_count) {
    return Ok(cached.clone());
}
// ... build assignments ...
cache.insert(sir_count, assignments.clone());

// AFTER:
let mut cache = shared.caches.layer_assignments.lock()
    .map_err(|err| format!("layer assignment cache lock poisoned: {err}"))?;
if let Some((cached_count, ref cached)) = *cache {
    if cached_count == sir_count {
        return Ok(cached.clone());
    }
}
// ... build assignments (unchanged) ...
*cache = Some((sir_count, assignments.clone()));
```

**Step A4c:** Update the cache consumer in `crates/aether-dashboard/src/api/catalog.rs`.

Find `load_or_build_layer_assignments` (around line 465). Apply the same pattern:

```rust
// BEFORE (around line 471):
let mut cache = shared.caches.layer_assignments_by_sir.lock()
    .map_err(|err| format!("layer assignment cache lock poisoned: {err}"))?;
if let Some(cached) = cache.get(&sir_count) {
    return Ok(cached.clone());
}
// ... build ...
cache.insert(sir_count, assignments.clone());

// AFTER:
let mut cache = shared.caches.layer_assignments.lock()
    .map_err(|err| format!("layer assignment cache lock poisoned: {err}"))?;
if let Some((cached_count, ref cached)) = *cache {
    if cached_count == sir_count {
        return Ok(cached.clone());
    }
}
// ... build (unchanged) ...
*cache = Some((sir_count, assignments.clone()));
```

---

## Fix A5: MCP Hybrid Search Starves the Reranker (P1)

**The bug:** In `aether_search_logic`, the lexical search (line 1348) and semantic
search (line 1389) both receive `limit` (e.g., 10) as their candidate cap. The
fuse/rerank stage then has at most 20 candidates to work with. With a
`rerank_window` of 50, the reranker is starved. The CLI search was already fixed
in `aetherd/src/search.rs` with `limit * 3`.

### File: `crates/aether-mcp/src/lib.rs`

**Step A5a:** Calculate retrieval limit before the searches.

Find `aether_search_logic` (around line 1332). After `let limit = effective_limit(request.limit);`
(around line 1337), add the retrieval limit calculation:

```rust
// ADD after line 1337 (after `let limit = effective_limit(request.limit);`):
let search_config_ref = self.state.config.as_ref();
let retrieval_limit = {
    let reranker = search_config_ref.search.reranker;
    let window = search_config_ref.search.rerank_window;
    if !matches!(reranker, SearchRerankerKind::None) {
        window.max(limit as u32).clamp(1, 200) as usize
    } else {
        limit
    }
};
```

**Step A5b:** Pass `retrieval_limit` to the lexical search.

Find the `store.search_symbols(query.as_str(), limit)` call (around line 1349):

```rust
// BEFORE:
let matches = store.search_symbols(query.as_str(), limit)?;

// AFTER:
let matches = store.search_symbols(query.as_str(), retrieval_limit)?;
```

**Step A5c:** Pass `retrieval_limit` to the semantic search.

Find the two calls to `self.semantic_search_matches(&request.query, limit)`:
one around line 1370 (Semantic mode) and one around line 1389 (Hybrid mode).
Change **both** to use `retrieval_limit`:

```rust
// BEFORE:
self.semantic_search_matches(&request.query, limit).await?

// AFTER:
self.semantic_search_matches(&request.query, retrieval_limit).await?
```

**Note:** `SearchRerankerKind` should already be imported. If not, add the import.

---

## Fix A6: `run_async_with_timeout` Creates New Tokio Runtime (P1)

**The bug:** `run_async_with_timeout` wraps an async future in `spawn_blocking`,
inside of which it creates a `new_current_thread()` Tokio runtime to execute the
future. This adds massive latency overhead and can cause deadlocks if the future
accesses clients (like SurrealDB) bound to the main Axum runtime.

### File: `crates/aether-dashboard/src/support.rs`

Replace the `run_async_with_timeout` function (around line 360):

```rust
// BEFORE (around line 360-375):
pub(crate) async fn run_async_with_timeout<T, F, Fut>(operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, String>> + Send + 'static,
{
    run_blocking_with_timeout(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| format!("failed to build blocking query runtime: {err}"))?;
        runtime.block_on(operation())
    })
    .await
}

// AFTER:
pub(crate) async fn run_async_with_timeout<T, F, Fut>(operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, String>> + Send + 'static,
{
    tokio::time::timeout(
        Duration::from_secs(GRAPH_QUERY_TIMEOUT_SECS),
        operation(),
    )
    .await
    .map_err(|_| timeout_error_message(GRAPH_QUERY_TIMEOUT_MESSAGE))?
}
```

Make sure `Duration` and `GRAPH_QUERY_TIMEOUT_SECS` / `GRAPH_QUERY_TIMEOUT_MESSAGE`
are already in scope (they should be — they're used by `run_blocking_with_timeout`
just above).

**Verify:** Confirm that callers of `run_async_with_timeout` pass futures that
are `Send + 'static`. Search for `run_async_with_timeout` in the dashboard crate
to check all call sites still compile.

---

## Fix A7: Semantic Records Duplication on Content Edit (P1)

**The bug:** `insert_semantic_record` upserts on `ON CONFLICT(record_id)`. Since
`record_id = BLAKE3(unit_id + schema_version + content_hash)`, when a document is
edited (changing `content_hash`), a new `record_id` is generated. The old record
is never deleted. Over time, edited documents accumulate stale rows.

### File: `crates/aether-store/src/document_store.rs`

Find `insert_semantic_record` (around line 182). Add a DELETE before the INSERT:

```rust
// BEFORE (around line 190-191):
let conn = self.conn.lock().unwrap();
conn.execute(

// AFTER:
let conn = self.conn.lock().unwrap();
// Remove stale records for this unit+schema before inserting the new version.
// This prevents accumulation when content_hash changes (which changes record_id).
conn.execute(
    "DELETE FROM semantic_records WHERE unit_id = ?1 AND schema_name = ?2 AND record_id != ?3",
    params![
        record.unit_id.as_str(),
        record.schema_name.as_str(),
        record.record_id.as_str(),
    ],
)?;
conn.execute(
```

The rest of the INSERT statement remains unchanged.

---

## Scope Guard

- Do NOT modify any MCP tool schemas, CLI argument shapes, or public API contracts.
- Do NOT add new crates or new workspace dependencies.
- Do NOT touch SQLite schema migrations or LanceDB table schemas.
- Do NOT rename any public functions or types.
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

All tests MUST pass. The new `record_sir_version_unchanged_hash_advances_timestamp`
test in Fix A1 MUST pass.

If `cargo clippy` warns about unused imports after changes, remove them.

---

## Commit Message

```
fix: P0/P1 hardening pass 6a — SIR regen loop, hallucination fallback, Time Machine timestamps, cache leak, MCP search, async timeout, semantic dedup

- Fix SIR regeneration returning stale timestamp on unchanged hash (P0 — burns API tokens)
- Reject whole-file fallback in build_job to prevent SIR corruption (P0)
- Multiply sir_history.created_at by 1000 in Time Machine queries (P0)
- Replace unbounded HashMap caches with single-entry Option (P1 — memory leak)
- Apply retrieval_limit to MCP lexical and semantic search (P1 — search quality)
- Rewrite run_async_with_timeout to use tokio::time::timeout directly (P1)
- Delete stale semantic_records before insert on content change (P1)
```

---

## Post-Fix Commands

```bash
git add -A
git commit -m "fix: P0/P1 hardening pass 6a — SIR regen loop, hallucination fallback, Time Machine timestamps, cache leak, MCP search, async timeout, semantic dedup"
git push origin feature/hardening-pass6a
gh pr create --base main --head feature/hardening-pass6a \
  --title "Hardening pass 6a: P0/P1 critical fixes from Gemini deep review" \
  --body "7 fixes: SIR regeneration loop, whole-file hallucination, Time Machine timestamps, cache memory leak, MCP search quality, async timeout anti-pattern, semantic record duplication."
```

After merge:

```bash
cd /home/rephu/projects/aether
git switch main
git pull --ff-only
git worktree remove ../aether-hardening-pass6a
git branch -d feature/hardening-pass6a
git worktree prune
```
