# Phase 1 - Stage 2: SIR Generation

## Purpose
Generate deterministic, structured SIR for indexed symbols and persist it for later retrieval.

## In scope
- Provider integration in `crates/aether-infer`
- SIR schema + validation + canonicalization in `crates/aether-sir`
- Persistence and lookup in `crates/aether-store`
- Index-to-inference orchestration in `crates/aetherd`

## Out of scope
- Search ranking and query UX (Stage 3.x)
- Historical SIR version timeline (Phase 2)

## Pass criteria
1. Mock provider produces SIR without network access.
2. SIR artifacts are persisted and retrievable after process restart.
3. Re-indexing changed symbols refreshes SIR only for affected symbols.
4. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase1-stage2-sir-generation off main.
3) Create worktree ../aether-phase1-stage2-sir for that branch and switch into it.
4) Implement Stage 2 SIR generation in existing crates:
   - crates/aether-infer
   - crates/aether-sir
   - crates/aether-store
   - crates/aetherd
5) Add tests for:
   - mock-provider SIR generation
   - persisted retrieval after restart
   - selective refresh on symbol change
6) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
7) Commit with message: "Implement Phase 1 Stage 2 SIR generation and storage".
```
