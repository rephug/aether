# Phase 1 - Stage 3.2: Robustness

## Purpose
Make inference behavior resilient so AETHER keeps serving usable SIR under transient failures.

## In scope
- Failure/staleness metadata in `crates/aether-store`
- Retry/backoff and throttling in `crates/aether-infer` and `crates/aetherd`
- Stale-state surfacing in `crates/aether-mcp` and `crates/aether-lsp`

## Out of scope
- Cost management policy and quota systems
- Verification runtime (Phase 3)

## Pass criteria
1. Failed/timeout inference does not remove last good SIR.
2. Stale metadata is stored and visible in MCP/LSP outputs.
3. Successful regeneration clears stale status consistently.
4. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase1-stage3-2-robustness off main.
3) Create worktree ../aether-phase1-stage3-2-robustness for that branch and switch into it.
4) Implement robustness in existing crates only:
   - preserve last-good SIR on inference failure
   - persist stale/error metadata in crates/aether-store
   - add bounded retry/backoff in crates/aether-infer or orchestration in crates/aetherd
   - surface stale status in crates/aether-mcp and crates/aether-lsp
5) Add tests for stale retention, stale clearing, and output status fields.
6) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
7) Commit with message: "Add stale-SIR handling and inference robustness".
```

## Compatibility Note For Stage 3.3
Stage 3.3 changes canonical SIR JSON storage location to SQLite, but it does **not** change
the meaning of Stage 3.2 metadata fields. Existing metadata remains valid and should not be
rewritten during mirror-file backfill.
