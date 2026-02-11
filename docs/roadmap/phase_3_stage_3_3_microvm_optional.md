# Phase 3 - Stage 3.3: MicroVM (Optional)

## Purpose
Add an optional high-isolation verification path for teams that can run MicroVM infrastructure.

## In scope
- Optional microVM execution adapter in `crates/aetherd`
- Feature/config gating in `crates/aether-config`
- Fallback behavior when microVM support is unavailable

## Out of scope
- Making microVM mandatory for all users
- Full hypervisor abstraction across every platform

## Pass criteria
1. MicroVM mode is explicitly opt-in and disabled by default.
2. Unsupported environments fail gracefully with clear guidance and fallback path.
3. Tests cover config parsing and runtime selection/fallback logic.
4. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase3-stage3-3-microvm-optional off main.
3) Create worktree ../aether-phase3-stage3-3-microvm for that branch and switch into it.
4) Implement optional microVM verification in existing crates only:
   - adapter + mode selection in crates/aetherd
   - opt-in config in crates/aether-config
   - explicit fallback result path through crates/aether-mcp
5) Add tests for opt-in behavior and unsupported-host fallback.
6) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
7) Commit with message: "Add optional microVM verification mode".
```
