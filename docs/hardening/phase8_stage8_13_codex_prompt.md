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

## IMPLEMENTATION

### 1. Add new Store methods to `crates/aether-store/src/lib.rs`

Add these methods to the `SqliteStore` impl:

**a) `list_symbols_not_in_snapshot(snapshot_ids: &[String]) -> Result<Vec<SymbolRecord>>`**
Returns all stored symbols whose IDs are NOT in the provided snapshot.
Use a temporary table or `NOT IN` clause. These are candidates for
reconciliation or pruning.

**b) `reconcile_symbol_id(old_id: &str, new_id: &str) -> Result<()>`**
Inside a single transaction:
- Copy the `sir` row from old_id to new_id (update symbol_id, keep all other fields)
- Copy `sir_history` rows from old_id to new_id
- Delete the old `sir` row
- Delete old `sir_history` rows
- Update the `symbols` table: delete the old record
- Log at INFO: "Reconciled symbol: {old_id} → {new_id}"

If old_id has no SIR, just delete the old symbol record.

**c) `prune_orphaned_symbols(orphan_ids: &[String]) -> Result<usize>`**
Inside a single transaction:
- Delete from `symbols` where id IN orphan_ids
- Delete from `sir` where id IN orphan_ids
- Delete from `sir_history` where symbol_id IN orphan_ids
- Return the count of deleted symbols
- Log at INFO: "Pruned {count} orphaned symbols"

### 2. Add graph edge cleanup to `crates/aether-store/src/graph_surreal.rs`

**`delete_edges_for_symbols(symbol_ids: &[String]) -> Result<()>`**
Delete all edges where source_symbol_id OR target_symbol_id is in the list.
This runs after SQLite reconciliation.

### 3. Add reconciliation logic to `crates/aetherd/src/sir_pipeline.rs`

Add a `reconcile_stale_symbols` function that runs AFTER the parse snapshot
is built but BEFORE SIR generation, when `--full` is set.

Logic:
1. Get the current parse snapshot's symbol IDs
2. Call `list_symbols_not_in_snapshot` to find stale symbols
3. For each stale symbol, attempt to match by `(file_path, symbol_name, symbol_kind)` tuple:
   - Look for a new symbol in the snapshot with the same tuple
   - If exactly one match: call `reconcile_symbol_id(old_id, new_id)`
   - If multiple old IDs match the same new symbol: pick the one with the
     most recent SIR (`updated_at`), reconcile it, mark the rest for pruning
   - If no match: mark for pruning
4. Call `prune_orphaned_symbols` with all unmatched old IDs
5. Call `delete_edges_for_symbols` on SurrealDB for all pruned + reconciled old IDs
6. Log summary: "Reconciliation complete: {reconciled} migrated, {pruned} pruned"

**Important:** Reconciliation only runs when `--full` is set. Normal incremental
scans do NOT reconcile or prune.

### 4. Add `--dry-run` flag to CLI

In `crates/aetherd/src/cli.rs`, add a `--dry-run` bool to the appropriate args struct
(probably alongside `--full`).

When `--dry-run` is set with `--full`:
- Run the reconciliation matching logic
- Print what WOULD be reconciled and pruned
- Do NOT actually modify the store
- Exit after the report

### 5. Embedding cleanup (best-effort)

After SQLite reconciliation and SurrealDB edge cleanup, attempt to delete
embeddings for pruned symbol IDs from the vector store. LanceDB doesn't
support transactions, so this is best-effort — if it fails, the next
`--embeddings-only` run will regenerate.

### 6. Tests

Add these tests to `crates/aether-store` and `crates/aetherd`:

**aether-store tests:**
- `reconcile_migrates_sir_to_new_id` — old symbol with SIR, new symbol
  with same (file, name, kind) but different ID → SIR moved to new ID
- `reconcile_picks_most_recent_on_ambiguity` — two old IDs match same
  new symbol → most recent SIR wins
- `prune_removes_orphans_not_in_snapshot` — old symbols with no match
  → deleted from symbols, sir, sir_history tables
- `reconcile_preserves_sir_history` — sir_history rows migrated

**aetherd tests:**
- `dry_run_reports_without_mutating` — with --dry-run, store unchanged
- `coverage_reaches_100_after_reconcile` — simulate partial run → content
  change → --full re-index → coverage 100%

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

- reconcile_symbol_id() migrates SIR and history when symbol ID changes
  due to content hash differences across re-index runs
- prune_orphaned_symbols() removes stale symbols not in current snapshot
- Reconciliation matches by (file_path, symbol_name, symbol_kind) tuple
- Only runs on --full, never on incremental scans
- --dry-run flag shows what would be reconciled/pruned without mutating
- All SQLite mutations in single transaction
- SurrealDB edge cleanup and LanceDB embedding cleanup (best-effort)"
```

Do NOT push. Robert will review.
