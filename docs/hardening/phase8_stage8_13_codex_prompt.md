# Codex Prompt — Phase 8.13: Symbol Reconciliation + Orphan Cleanup

CRITICAL BUILD SETTINGS — use these for ALL cargo commands:
```
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

Read these files before writing any code:
- `docs/roadmap/phase_8_stage_8_13_reconciliation.md` (the full spec)
- `docs/hardening/phase8_stage8_13_session_context.md` (session context)
- `crates/aetherd/src/sir_pipeline.rs` (current SIR pipeline)
- `crates/aether-store/src/lib.rs` (SqliteStore implementation)
- `crates/aether-store/src/graph_surreal.rs` (graph edge storage)
- `crates/aetherd/src/cli.rs` (CLI arguments)

Also inspect these to understand the vector store and indexer architecture:
- `crates/aether-store/src/vector.rs` (VectorStore trait, backends)
- `crates/aetherd/src/indexer.rs` (Pass 1 and Pass 2 loops, if it exists)

## PREFLIGHT

```bash
cd /home/rephu/projects/aether
git status --porcelain
# Must be clean. If dirty, STOP and report.
git pull --ff-only
```

Create branch and worktree:
```bash
git worktree add ../aether-phase8-reconciliation -b feature/phase8-stage8-13-reconciliation
cd /home/rephu/projects/aether-phase8-reconciliation
```

## SOURCE INSPECTION (do this BEFORE writing code)

Before implementing, inspect the actual source to answer these questions.
Report your findings. Do not assume anything from the spec alone.

1. **Schema check:** Does the `symbols` table have `access_count` and
   `last_accessed_at` columns? Check `crates/aether-store/src/lib.rs`
   for the CREATE TABLE statement or migration that defines the schema.

2. **Graph cascade check:** In `crates/aether-store/src/graph_surreal.rs`,
   does deleting a symbol node automatically cascade to delete connected
   edges? Or do edges need explicit deletion? Check the SurrealDB schema
   definitions and any existing delete methods.

3. **Vector backend check:** What vector store backends exist? Check the
   `VectorStore` trait and its implementations. The active backend may be
   SQLite or LanceDB depending on config — cleanup must target whichever
   backend is active, not hardcode one.

4. **Indexer location:** Is the main indexing/SIR pipeline entry point in
   `sir_pipeline.rs`, `indexer.rs`, or somewhere else? Find where `--full`
   is handled and where the parse snapshot is built.

5. **Matching key:** Confirm that `qualified_name` (not just `symbol_name`)
   is available on `SymbolRecord`. This is critical — matching by leaf name
   alone (`new`, `build`, `process`) will cause collisions across impl blocks.

## IMPLEMENTATION

### 1. Add new Store methods to `crates/aether-store/src/lib.rs`

**a) `list_stale_symbols(snapshot_ids: &HashSet<String>) -> Result<Vec<SymbolRecord>>`**

CRITICAL: Do NOT use `WHERE id NOT IN (?, ?, ...)` SQL clause. SQLite has
parameter limits that will crash on large repos. Instead: query all symbols
from the table and filter in Rust using the HashSet. This is O(N) and takes
milliseconds.

**b) `reconcile_and_prune(migrations: &[(String, String)], prunes: &[String]) -> Result<(usize, usize)>`**

Execute ALL mutations inside a SINGLE SQLite transaction.

For each `(old_id, new_id)` in migrations:
- Check if `new_id` already has a SIR record:
  - If yes: compare `updated_at` timestamps. Keep the newer SIR. If old is
    newer, migrate it to new_id (overwrite). If new is newer, skip the SIR
    migration but still clean up the old record. Log the conflict resolution
    at WARN level.
  - If no: copy the SIR row from old_id to new_id.
- Migrate `sir_history` rows from old_id to new_id.
- If `symbols` table tracks access metadata (`access_count`, `last_accessed_at`),
  transfer it to the new symbol record (add old counts to new counts, keep
  the more recent `last_accessed_at`).
- Delete the old `sir`, `sir_history`, and `symbols` rows for old_id.

For prunes, chunk the orphan_ids into batches of 500 to avoid SQLite
parameter limits:
- Delete from `symbols`, `sir`, `sir_history` where id/symbol_id matches.
- Also clean up `write_intents` and `sir_requests` if those tables reference
  symbol IDs.

Return `(migrated_count, pruned_count)`.

### 2. Add vector store cleanup

Add a `delete_embeddings(symbol_ids: &[String])` method to the `VectorStore`
trait (or equivalent). Implement it for ALL active backends:
- If SQLite vector backend: chunked DELETE.
- If LanceDB vector backend: chunked `table.delete(predicate)`.

The cleanup must target whichever backend is active — do not hardcode one.
Chunk IDs into batches of 500 to prevent parser/parameter limits.

This is best-effort: log errors but do not fail the reconciliation if
vector cleanup fails (the next `--embeddings-only` run will regenerate).

### 3. Add graph store cleanup

Inspect `graph_surreal.rs` to determine whether symbol node deletion
cascades to edges automatically.

- If cascade: add `delete_symbols_batch(symbol_ids: &[String])` that deletes
  the symbol nodes. Chunk into batches of 500.
- If no cascade: add explicit edge deletion for source/target matches,
  then delete the symbol nodes. Chunk into batches of 500.

### 4. Add reconciliation logic to the SIR pipeline

Add a `reconcile_stale_symbols` function that runs AFTER the parse snapshot
is built but BEFORE SIR generation, only when `--full` is set.

Logic:
1. Build a `HashSet` of current parse snapshot symbol IDs.
2. Call `list_stale_symbols` to find symbols not in the current snapshot.
3. For each stale symbol, match by `(file_path, qualified_name, kind)` tuple
   against new symbols in the snapshot.
   CRITICAL: Use `qualified_name`, not `symbol_name`. Leaf names like `new`
   collide across impl blocks.
   - Exactly one match → push to `migrations` as `(old_id, new_id)`
   - Multiple old IDs match same new symbol → pick by most recent
     `updated_at`, migrate that one, push the rest to `prunes`
   - No match → push to `prunes`
4. Call `reconcile_and_prune(&migrations, &prunes)` on SQLite.
5. Collect ALL old_ids (both migrated and pruned) into `cleanup_ids`.
6. Call graph store cleanup with `cleanup_ids` (SurrealDB).
7. Call vector store cleanup with `cleanup_ids` (best-effort, log on error).
   Delete embeddings for BOTH pruned AND reconciled old IDs — reconciled
   old IDs leak ghost vectors that cause search failures if not cleaned up.
   The new ID will get embedded naturally during the next pipeline pass.
8. Log: "Reconciliation complete: {migrated} migrated, {pruned} pruned"

### 5. Add `--dry-run` flag to CLI

In `crates/aetherd/src/cli.rs`, add `--dry-run` alongside `--full`.

When set:
- Run the matching logic (steps 1-3 above)
- Print what WOULD be migrated and pruned (with symbol names and paths)
- Do NOT call any mutation methods
- Exit after the report

### 6. Tests

**aether-store:**
- `reconcile_migrates_sir_to_new_id` — old symbol with SIR, new symbol with
  same (file, qualified_name, kind) but different ID → SIR moved to new ID
- `reconcile_picks_most_recent_on_ambiguity` — two old IDs match same new
  symbol → most recent SIR wins, other pruned
- `prune_removes_orphans_not_in_snapshot` — old symbols with no match →
  deleted from symbols, sir, sir_history
- `reconcile_preserves_sir_history` — sir_history rows migrated with SIR
- `reconcile_handles_new_id_already_has_sir` — if new_id already has SIR,
  keep the newer one by updated_at, log the conflict

**aetherd:**
- `dry_run_reports_without_mutating` — with --dry-run, store unchanged
- `coverage_reaches_100_after_reconcile` — simulate partial run → content
  change → --full re-index → coverage 100%
- `full_reconcile_is_idempotent` — run reconciliation twice on an
  already-clean state: zero migrations, zero prunes, no corruption

## SCOPE GUARD

Do NOT modify:
- SIR generation logic or the scan/triage/deep pipeline
- Symbol ID hashing (BLAKE3)
- Normal incremental scan behavior
- Health scoring, community detection, planner
- Coupling, drift, dashboard code
- Edge extraction in aether-parse
- Any public API signatures (add new methods, don't change existing ones)

## VALIDATION GATE

```bash
cargo fmt --check
cargo clippy -p aether-store -p aetherd -- -D warnings
cargo test -p aether-store
cargo test -p aetherd
```

## COMMIT

```bash
git add -A
git commit -m "Add symbol reconciliation and orphan cleanup for --full re-index

- Match stale symbols by (file_path, qualified_name, kind) tuple to preserve
  SIR across content-hash ID changes between re-index runs
- reconcile_and_prune() migrates SIR, history, and access metadata inside a
  single SQLite transaction
- Prune orphaned symbols not in current snapshot
- Vector store cleanup targets active backend (SQLite or LanceDB), chunked
- SurrealDB graph cleanup for reconciled and pruned symbol IDs
- Only runs on --full flag, supports --dry-run for safe preview
- Idempotent: running reconciliation on clean state produces no changes"
```

Do NOT push. Robert will review.
