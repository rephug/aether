# Phase 8.13 — Symbol Reconciliation + Orphan Cleanup — Session Context

**Date:** 2026-03-10
**Branch:** `feature/phase8-stage8-13-reconciliation` (to be created)
**Worktree:** `/home/rephu/projects/aether-phase8-reconciliation` (to be created)
**Starting commit:** HEAD of main (after 8.12.2 merge)

## CRITICAL: Read actual source, not this document

```bash
# Clone the repo to inspect real code:
git clone https://github.com/rephug/aether.git /tmp/aether

# The live repo is at:
/home/rephu/projects/aether

# Always grep/read actual source before making claims about what exists
```

## Project overview

AETHER is a Rust multi-crate workspace (~55K+ LOC, 15+ crates) that creates persistent
semantic intelligence for codebases. It parses code via tree-sitter, generates Semantic
Intent Records (SIRs) per symbol using LLM inference, stores metadata in SQLite, vectors
in LanceDB/SQLite, and graph relationships in SurrealDB.

Robert is the sole developer. He uses OpenAI Codex CLI as the primary implementation agent,
with Claude for planning, verification, and prompt production.

## Build environment (MUST be set for ALL cargo commands)

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

**Never run `cargo test --workspace`** — OOM risk. Always per-crate.

## The problem being solved

When AETHER parses a file, it generates a BLAKE3 content hash as the symbol ID. If file
content changes between runs (even whitespace or comment changes), the same logical symbol
gets a different ID. The old ID stays in the store with its SIR but the new parse snapshot
doesn't include it. On `--full` re-index, old SIRs are stranded and coverage permanently
gets stuck.

Observed: `WARN Scan pass skipped symbols missing from initial snapshot unresolved=8146`
with coverage stuck at 46.71%. After nuking `.aether/` and re-indexing fresh: 3361 total
symbols, 100% coverage. The 15289 was inflated by stale IDs from multiple partial runs.

## What just merged

- **Phase 8.12** — Community detection pipeline (type-anchor, semantic rescue, etc.)
- **Phase 8.12.1** — OpenAI-compatible embedding provider + `--embeddings-only` flag
- **Phase 8.12.2** — Large anchor intra-partitioning (first-token bucketing, empty-stem
  guard, per-step diagnostics). aether-store went from 2/155 to 14/91/10.

## Key files to inspect before writing code

```
crates/aetherd/src/sir_pipeline.rs         — SIR generation pipeline, scan/triage/deep passes
crates/aether-store/src/lib.rs             — SqliteStore: symbols, sir, sir_history tables
crates/aether-store/src/graph_surreal.rs   — SurrealDB graph edge storage
crates/aetherd/src/cli.rs                  — CLI argument parsing
```

## Scope guard (must NOT be modified in 8.13)

- SIR generation logic or quality pipeline
- Symbol ID hashing algorithm (BLAKE3 content hash)
- Normal incremental scan behavior (only `--full` gets reconciliation)
- Health scoring, community detection, or planner
- Coupling, drift, or dashboard code
- Edge extraction in aether-parse

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
