# Phase 2: Historian

## Purpose
Track how symbol meaning evolves over time and expose history for "why changed" workflows.

## In scope
- SIR version history storage and retrieval in `crates/aether-store`
- Git metadata linkage from indexed symbols in existing crates (`aetherd`, `aether-store`, optional `aether-core` helpers)
- History-oriented responses in `crates/aether-mcp` and optional hover metadata in `crates/aether-lsp`

## Out of scope
- Perfect blame fidelity for every git edge case in first release
- Verification execution runtime (Phase 3)

## Pass criteria
1. Symbol SIR history records multiple versions when SIR hashes change.
2. History queries return deterministic ordered timelines after restart.
3. Git commit linkage is present for timeline entries where data is available.
4. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase-2-historian-rollup off main.
3) Create worktree ../aether-phase-2-historian for that branch and switch into it.
4) Implement Phase 2 by completing stage docs in this order:
   - docs/roadmap/phase_2_stage_2_1_sir_versioning.md
   - docs/roadmap/phase_2_stage_2_2_git_linkage.md
   - docs/roadmap/phase_2_stage_2_3_why_queries.md
5) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
6) Commit with message: "Complete Phase 2 Historian rollout".
```
