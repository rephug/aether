# Phase 8 — Stage 8.13: Symbol Reconciliation + Orphan Cleanup

## Purpose

Fix the `--full` re-index bug where orphaned symbol IDs from prior
parse snapshots cannot be regenerated or cleaned up, permanently
capping SIR coverage until the user manually nukes `.aether/`.

## Problem

When AETHER parses a file, it generates a BLAKE3 content hash as
the symbol ID. If the file content changes between runs (even
whitespace or comment changes), the same logical symbol gets a
different ID. The old ID remains in the store with its SIR, but
the new parse snapshot doesn't include it. On `--full` re-index:

1. The scan pass builds a fresh tree-sitter snapshot with new IDs.
2. It looks up each new ID in the store to check for existing SIRs.
3. Old IDs are not in the snapshot → "missing from initial snapshot."
4. New IDs have no SIRs yet → but `--full` without `force` skips
   symbols that already have SIRs (and new IDs don't).
5. Result: old SIRs are stranded, new symbols may or may not get
   generated depending on the `force` flag behavior.

The user sees: `WARN Scan pass skipped symbols missing from initial
snapshot unresolved=8146` and coverage permanently stuck.

## Observed behavior

```
2026-03-09T09:56:51.224380Z  INFO Scan pass: generating SIR for 0 symbols
2026-03-09T09:56:51.224390Z  WARN Scan pass skipped symbols missing from
  initial snapshot unresolved=8146
2026-03-09T09:56:51.238464Z  INFO Quality pipeline complete: SIR coverage
  symbols_with_sir=7143 total_symbols=15289 coverage_pct=46.71
```

After nuking `.aether/` and re-indexing fresh: 3361 total symbols,
100% coverage. The 15289 was inflated by stale IDs accumulated
across multiple partial runs.

## Fix

Two changes:

### 1. Symbol reconciliation on re-index

When `--full` encounters stored symbols whose IDs don't match the
current snapshot, attempt to match them by the tuple
`(file_path, symbol_name, symbol_kind)`.

For each match:
- Migrate the existing SIR from the old symbol ID to the new ID
  (copy `sir` row, update `symbol_id`, preserve all SIR fields)
- Migrate `sir_history` rows to the new ID
- Delete the old symbol record and its edges
- Log at INFO: "Reconciled symbol {name} in {file}: {old_id} → {new_id}"

For ambiguous matches (multiple old IDs match the same new symbol):
- Pick the one with the most recent SIR (highest `updated_at`)
- Log a WARN for the discarded duplicates

### 2. Orphan pruning

After reconciliation, any remaining stored symbols that:
- Are not in the current parse snapshot, AND
- Could not be reconciled to a new ID

...are orphans from deleted/renamed code. Prune them:
- Delete from `symbols` table
- Delete from `sir` table
- Delete from `sir_history` table
- Delete edges where source_id or target_id matches
- Delete from `sir_embeddings` (LanceDB)
- Log at INFO: "Pruned {count} orphaned symbols"

Pruning only runs when `--full` is set. Normal incremental scans
do NOT prune — they only process changed files.

## Safety

- Reconciliation is best-effort. If the tuple match is ambiguous
  or missing, the old SIR is pruned, not silently kept.
- A `--dry-run` flag on `--full` shows what would be reconciled
  and pruned without making changes. Useful for debugging.
- All mutations happen inside a single SQLite transaction so a
  crash mid-reconciliation doesn't leave the store inconsistent.
- SurrealDB graph edge cleanup runs after SQLite reconciliation.
- LanceDB embedding cleanup runs last (LanceDB doesn't support
  transactions, but losing an embedding is recoverable — next
  scan regenerates it).

## Files to modify

```
crates/aetherd/src/sir_pipeline.rs    — reconciliation + pruning logic
crates/aether-store/src/lib.rs        — new methods: reconcile_symbol_id(),
                                        prune_orphaned_symbols(),
                                        list_symbols_not_in_snapshot()
crates/aether-store/src/graph_surreal.rs — delete edges for pruned symbols
crates/aetherd/src/cli.rs             — add --dry-run flag (optional)
```

## Scope guard

- Do NOT change SIR generation logic or quality pipeline
- Do NOT change symbol ID hashing algorithm (BLAKE3 content hash)
- Do NOT change normal incremental scan behavior
- Do NOT change health scoring, community detection, or planner
- Do NOT touch coupling, drift, or dashboard code

## Tests

- `reconcile_migrates_sir_to_new_id`
  Old symbol with SIR, new symbol with same (file, name, kind)
  but different ID → SIR moved to new ID, old ID deleted.
- `reconcile_picks_most_recent_on_ambiguity`
  Two old IDs match same new symbol → most recent SIR wins.
- `prune_removes_orphans_not_in_snapshot`
  Old symbols with no match in current snapshot → deleted.
- `prune_only_runs_on_full_flag`
  Normal incremental scan does not prune anything.
- `reconcile_preserves_sir_history`
  `sir_history` rows migrated alongside the `sir` row.
- `dry_run_reports_without_mutating`
  With --dry-run, reconciliation and pruning are logged but
  store is unchanged.
- `coverage_reaches_100_after_reconcile`
  Simulate partial run → content change → --full re-index →
  coverage should be 100%, not stuck at old percentage.

## Validation

```bash
cargo fmt --check
cargo clippy -p aether-store -p aetherd -- -D warnings
cargo test -p aether-store
cargo test -p aetherd
```

Manual verification:
```bash
# Simulate the bug: index, modify a file, re-index with --full
# Verify no "missing from initial snapshot" warnings
# Verify coverage_pct = 100%
```

## Decisions

- **#69**: Reconciliation by (file_path, symbol_name, symbol_kind)
  tuple. Ambiguous matches resolved by most recent SIR.
- **#70**: Orphan pruning only on `--full`. Incremental scans
  never prune.
- **#71**: All SQLite mutations in single transaction. LanceDB
  cleanup is best-effort (recoverable on next scan).

## Future work

**Stage 8.14 — Global community quality:**
- Apply selective rescue + resolution tuning to global Louvain
- Fixes Boundary Leaker counts and orphaned subgraph counts
- Depends on 8.12 ablation results

**Stage 8.15 or 9.x — Enhanced edge extraction:**
- IMPLEMENTS, TYPE_REF, FIELD_ACCESS in aether-parse
- Reduces structural orphans at the source

## End-of-stage git sequence

```bash
cd /home/rephu/projects/aether-phase8-reconciliation
git push origin feature/phase8-stage8-13-reconciliation

# Create PR via GitHub web UI, then after merge:
cd /home/rephu/projects/aether
git switch main
git pull --ff-only
git worktree remove ../aether-phase8-reconciliation
git branch -D feature/phase8-stage8-13-reconciliation
git worktree prune
```
