# Phase 3: Ghost

## Purpose
Verify patch safety before acceptance by running checks in isolated execution modes.

## In scope
- Verification orchestration in existing runtime paths (`crates/aetherd`, `crates/aether-config`)
- Result reporting to `crates/aether-mcp` and optional `crates/aether-lsp`
- Host, container, and optional microVM strategies

## Out of scope
- Guaranteeing identical determinism across every OS/kernel setup
- Replacing existing CI pipelines

## Pass criteria
1. Verification request produces clear pass/fail status with captured logs.
2. Host and container modes can be selected/configured and tested.
3. Optional microVM path degrades cleanly when unsupported.
4. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase-3-ghost-rollup off main.
3) Create worktree ../aether-phase-3-ghost for that branch and switch into it.
4) Implement Phase 3 by completing stage docs in this order:
   - docs/roadmap/phase_3_stage_3_1_host_verification.md
   - docs/roadmap/phase_3_stage_3_2_container_verification.md
   - docs/roadmap/phase_3_stage_3_3_microvm_optional.md
5) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
6) Commit with message: "Complete Phase 3 Ghost rollout".
```
