# Phase 10.5 — Intent Contracts — Session Context

**Date:** 2026-03
**Branch:** `feature/phase10-stage10-5-contracts` (to be created)
**Worktree:** `/home/rephu/aether-phase10-contracts` (to be created)
**Starting commit:** HEAD of main after 10.4 merged (10.4 recommended, not strictly required)
**Prerequisites:** Stages 10.1 + 10.2 merged. Embedding provider configured and working.

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

## What this stage adds

### A. Contract storage
- `intent_contracts` SQLite table with embedded clause vectors
- `intent_violations` SQLite table with streak tracking and dismissal

### B. Two-stage verification cascade
- Embedding cosine pre-filter (>0.88 pass, <0.50 fail)
- LLM judge for ambiguous 0.50-0.88 range
- Runs synchronously inline in SIR pipeline, not as background job

### C. Leaky bucket violation handling
- First violation: silent (streak=1)
- Second consecutive: alert
- Dismissed violations become negative few-shot examples

### D. Cross-symbol propagation
- Inject caller contracts into callee SIR enrichment context
- Fits into existing triage/deep prompt enrichment

### E. CLI: contract add/list/remove/check

---

## Key files to understand

**SIR pipeline (where verification hooks in):**
- `crates/aetherd/src/sir_pipeline/persist.rs` — SIR upsert function. Contract verification runs AFTER upsert, BEFORE the function returns. Find the exact function signature.
- `crates/aetherd/src/sir_pipeline/infer.rs` — SIR enrichment context. Cross-symbol contract clauses inject here.
- `crates/aetherd/src/batch/ingest.rs` — batch ingest calls the persist function. Verification triggers here too.

**Embeddings (for clause embedding + cosine comparison):**
- Find how to embed a single text string using the configured provider
- Find how to compute cosine similarity between two f32 vectors
- The clause embedding is computed once at contract creation time and stored as BLOB

**Inference (for LLM judge):**
- `crates/aether-infer/src/` — construct a provider for the judge model
- The judge prompt is a simple structured query returning JSON `{violated: bool, reason: "..."}`
- Use flash-lite by default for cost efficiency

**Schema:**
- `crates/aether-store/src/schema.rs` — migration for new tables
- Current schema version: check after 10.1-10.4 migrations

**Dashboard:**
- Add one new page: Contract Health
- Follow existing pattern from `xray.rs` or `health_score.rs`

---

## Important architectural decisions

**Verification is synchronous, not async.** Contract checking happens inside the SIR persist path. This means:
- Batch ingest is slightly slower (one cosine comparison per contract clause per symbol)
- But violations are detected at generation time, not hours later
- Deep Think explicitly recommended this: "run synchronously inside the pipeline like SirQualityMonitor"

**Embedding pre-filter resolves ~90% of cases.** Only the ambiguous 0.50-0.88 range triggers LLM calls. Design the code path so the common case (>0.88 or <0.50) is fast and the LLM path is lazy-initialized.

**The SIR table is named `sir` (NOT `sir_meta`).** Primary key is `id`.

---

## Scope guard — do NOT modify

- SIR generation logic (only hook AFTER upsert)
- Existing fingerprint history write path
- Existing batch build/ingest behavior (only add post-upsert hook)
- Existing dashboard pages

---

## End-of-stage sequence

```bash
cd /home/rephu/aether-phase10-contracts
git push -u origin feature/phase10-stage10-5-contracts

# After PR merges:
cd /home/rephu/projects/aether
git switch main
git pull --ff-only
git worktree remove ../aether-phase10-contracts
git branch -d feature/phase10-stage10-5-contracts
```
