# Phase 1: Observer

## Purpose
Deliver reliable local indexing and retrieval so AETHER continuously produces and serves symbol-level understanding.

## In scope
- Incremental indexing for Rust and TS/JS via `crates/aetherd`, `crates/aether-parse`, and `crates/aether-core`
- SIR generation and persistence via `crates/aether-infer`, `crates/aether-sir`, and `crates/aether-store`
- Retrieval surfaces in `crates/aether-mcp` and `crates/aether-lsp`

## Out of scope
- Historical attribution of why symbols changed (Phase 2)
- Execution verification/sandboxing of patches (Phase 3)

## Pass criteria
1. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` all pass.
2. In a temp workspace with Rust and TS/JS files, indexing and SIR generation produce retrievable entries under `.aether/`.
3. Stage 1 through Stage 3.8 docs are completed with implemented behavior and tests.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and report the dirty files.
2) Create branch feature/phase-1-observer-rollup off main.
3) Create worktree ../aether-phase-1-observer for that branch and switch into it.
4) Implement Phase 1 by completing stage docs in this order:
   - docs/roadmap/phase_1_stage_1_indexing.md
   - docs/roadmap/phase_1_stage_2_sir_generation.md
   - docs/roadmap/phase_1_stage_3_1_search_lexical.md
   - docs/roadmap/phase_1_stage_3_2_robustness.md
   - docs/roadmap/phase_1_stage_3_3_sir_sqlite_source_of_truth.md
   - docs/roadmap/phase_1_stage_3_4_semantic_search_embeddings.md
   - docs/roadmap/phase_1_stage_3_5_cli_ux_and_config.md
   - docs/roadmap/phase_1_stage_3_6_mcp_and_lsp_ux.md
   - docs/roadmap/phase_1_stage_3_7_vscode_extension_polish.md
   - docs/roadmap/phase_1_stage_3_8_ci_release_packaging.md
5) Run tests and checks:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
6) Commit with message: "Complete Phase 1 Observer rollout".
```
