# Phase 1 - Stage 1: Indexing

## Purpose
Build fast local indexing that tracks symbol identity across edits for Rust and TS/JS code.

## In scope
- File watching and one-shot indexing paths in `crates/aetherd`
- Symbol extraction in `crates/aether-parse`
- Stable symbol IDs and symbol diffing in `crates/aether-core`
- Persisting symbol metadata in `crates/aether-store`

## Out of scope
- SIR inference generation content quality (Stage 2)
- Search features (Stage 3.x)

## Pass criteria
1. Initial index produces symbol metadata for Rust and TS/JS fixtures.
2. Editing function bodies keeps stable symbol IDs when symbol identity is unchanged.
3. Renaming/removing symbols updates store state correctly.
4. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase1-stage1-indexing off main.
3) Create worktree ../aether-phase1-stage1-indexing for that branch and switch into it.
4) Implement Stage 1 indexing behavior in existing crates only:
   - crates/aetherd
   - crates/aether-parse
   - crates/aether-core
   - crates/aether-store
5) Add/adjust tests for:
   - first index
   - stable IDs after non-identity edits
   - symbol rename/delete handling
6) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
7) Commit with message: "Implement Phase 1 Stage 1 indexing".
```
