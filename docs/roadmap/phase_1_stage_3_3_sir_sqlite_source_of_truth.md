# Phase 1 - Stage 3.3: SQLite SIR Source of Truth

## Purpose
Make SQLite the primary SIR store so reads/writes are consistent and resilient.

## In scope
- Schema/migration updates in `crates/aether-store`
- Read/write order changes in `crates/aether-store` and callers in `crates/aetherd`
- Backfill path from existing `.aether/sir/*.json` mirror files

## Out of scope
- Historical multi-version timelines (Phase 2)
- Non-SQLite database backends

## Pass criteria
1. Canonical SIR JSON is persisted in SQLite and used as the first read path.
2. If JSON mirror files exist but DB field is missing, read path backfills SQLite.
3. Optional file mirror remains compatible for debugging.
4. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase1-stage3-3-sir-sqlite off main.
3) Create worktree ../aether-phase1-stage3-3-sqlite for that branch and switch into it.
4) Implement in existing crates only:
   - migrate SQLite schema in crates/aether-store to store canonical SIR JSON
   - make SQLite the primary read/write path
   - keep .aether/sir/*.json as optional mirror/backfill source
5) Add tests for DB-first read, mirror fallback backfill, and restart persistence.
6) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
7) Commit with message: "Use SQLite as SIR source of truth".
```
