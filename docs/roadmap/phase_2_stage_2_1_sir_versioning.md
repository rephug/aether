# Phase 2 - Stage 2.1: SIR Versioning

## Purpose
Persist symbol-level SIR revisions so timeline queries can compare intent changes over time.

## In scope
- Version tables/migrations in `crates/aether-store`
- Update triggers from indexing/inference pipeline in `crates/aetherd`
- History retrieval API surfaces used by MCP/LSP callers

## Out of scope
- Commit/PR linkage details (Stage 2.2)
- Human-readable "why" synthesis (Stage 2.3)

## Pass criteria
1. When a symbol’s SIR hash changes, a new version row is inserted with timestamp.
2. Re-index without content change does not create duplicate versions.
3. History retrieval for a symbol returns chronological versions after restart.
4. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase2-stage2-1-sir-versioning off main.
3) Create worktree ../aether-phase2-stage2-1-versioning for that branch and switch into it.
4) Implement SIR versioning in existing crates only:
   - schema + APIs in crates/aether-store
   - pipeline hooks in crates/aetherd
5) Add tests for: new-version-on-hash-change, no-duplicate-on-no-change, persistent history retrieval.
6) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
7) Commit with message: "Add SIR version history storage".
```
