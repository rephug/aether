# Phase 3 - Stage 3.2: Container Verification

## Purpose
Run verification in containers for reproducible dependency/runtime boundaries.

## In scope
- Container execution adapter in `crates/aetherd`
- Container-related config fields in `crates/aether-config`
- Structured result propagation through `crates/aether-mcp`

## Out of scope
- MicroVM acceleration and snapshot pooling (Stage 3.3)
- Full remote build farm support

## Pass criteria
1. Verification mode can run checks in a configured container image.
2. Command outputs and exit status are captured and returned consistently.
3. Tests cover mode selection and graceful handling when container runtime is unavailable.
4. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase3-stage3-2-container-verification off main.
3) Create worktree ../aether-phase3-stage3-2-container for that branch and switch into it.
4) Implement container verification in existing crates only:
   - container adapter in crates/aetherd
   - config in crates/aether-config
   - result output in crates/aether-mcp
5) Add tests for mode selection and unavailable-runtime fallback.
6) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
7) Commit with message: "Add container-based verification mode".
```
