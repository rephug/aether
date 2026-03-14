# Phase 8.22 — Refactor-Prep Deep Scan — Session Context

**Date:** 2026-03-14
**Branch:** `feature/phase8-stage8-22-refactor-prep` (to be created)
**Worktree:** `/home/rephu/aether-phase8-refactor-prep` (to be created)
**Starting commit:** HEAD of main after 8.21 merges (or current HEAD if running before 8.21)

## CRITICAL: Read actual source, not this document

```bash
/home/rephu/projects/aether
```

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

## What this stage does

Adds two CLI subcommands and two MCP tools:

1. `aether refactor-prep` — Identifies highest-risk symbols via health
   reports, runs targeted deep SIR passes on them, saves an intent snapshot.
   Outputs a refactoring brief.

2. `aether verify-intent` — Compares current SIR state against a saved
   snapshot. Reports per-symbol semantic drift.

3. `aether_refactor_prep` MCP tool — same as CLI, returns JSON.

4. `aether_verify_intent` MCP tool — same as CLI, returns JSON.

## Key architectural decisions

- Symbol selection uses EXISTING health report output (HealthReport struct).
  Do not recompute health — just load the most recent report or compute
  if none exists.
- Deep pass uses EXISTING `generate_sir_with_retries` from sir_pipeline.
  Do not build a new inference path.
- Intent snapshots go in meta.sqlite (new tables, new migration).
- The refactoring brief is a display concern — the core logic returns
  a `RefactorPrepResult` struct that both CLI and MCP format differently.

## Files to read before implementation

**Health infrastructure (selection logic consumes these):**
- `crates/aether-health/src/scoring.rs` — health score computation
- `crates/aether-analysis/src/health.rs` — HealthAnalyzer, HealthReport struct
- `crates/aether-health/src/planner.rs` — existing planner (follow patterns)

**Deep pass infrastructure (reuse, don't rebuild):**
- `crates/aetherd/src/sir_pipeline.rs` — three-pass pipeline, generate_sir_with_retries
- `crates/aetherd/src/indexer.rs` — how deep pass is triggered today

**Store layer (add snapshot tables):**
- `crates/aether-store/src/schema.rs` — migration pattern
- `crates/aether-store/src/lib.rs` — Store sub-traits (add SnapshotStore)
- `crates/aether-store/src/sir_meta.rs` — SIR metadata queries (check freshness)

**MCP layer (add tools):**
- `crates/aether-mcp/src/tools/router.rs` — tool registration
- `crates/aether-mcp/src/tools/health.rs` — health tools (follow pattern)
- `crates/aether-mcp/src/tools/mod.rs` — module declarations

**CLI layer (add subcommands):**
- `crates/aetherd/src/main.rs` — CLI arg parsing, subcommand dispatch
- `crates/aetherd/src/health_score.rs` — existing health CLI (follow pattern)

## Scope guards

- Do NOT modify sir_pipeline.rs (only call into it)
- Do NOT modify health scoring formulas
- Do NOT modify existing Store sub-traits (add new SnapshotStore)
- Do NOT modify existing MCP tools
- Do NOT modify community detection or planner logic
- Do NOT modify the three-pass pipeline selection logic
- The deep provider config comes from existing SirQualityConfig.deep_provider

## How deep pass should work in this context

The existing pipeline runs deep pass as part of a full index run. Here,
we need to run deep pass on individual symbols on-demand. The key function
is `generate_sir_with_retries` which takes a provider, prompt text, and
SirContext. The refactor-prep command needs to:

1. Build the enriched prompt (source text + neighbor intents + existing SIR)
   — same enrichment as the pipeline's deep pass
2. Call `generate_sir_with_retries` with the deep provider
3. Store the result with `generation_pass = "deep"`

Inspect `sir_pipeline.rs` to find exactly how the deep pass builds its
prompt. Extract or reuse that logic — do not duplicate it.

## Testing approach

Unit tests in each new module:
- `refactor.rs`: test symbol selection with mock health data
- `snapshots.rs`: test snapshot round-trip persistence
- `refactor_prep.rs`: integration test with mock provider

Manual validation:
```bash
# After building:
cargo build -p aetherd

# Run against AETHER itself:
aetherd --workspace /home/rephu/projects/aether refactor-prep \
  --crate aether-mcp --top-n 10 --local

# Verify snapshot was saved:
sqlite3 /home/rephu/projects/aether/.aether/meta.sqlite \
  "SELECT snapshot_id, scope, symbol_count, deep_count FROM intent_snapshots;"
```

## After this stage merges

```bash
git push -u origin feature/phase8-stage8-22-refactor-prep
# Create PR via GitHub web UI with title and body from spec
# After merge:
git switch main
git pull --ff-only
git worktree remove /home/rephu/aether-phase8-refactor-prep
git branch -d feature/phase8-stage8-22-refactor-prep
```
