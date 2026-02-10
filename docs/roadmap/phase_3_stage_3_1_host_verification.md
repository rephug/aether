# Phase 3 - Stage 3.1: Host Verification

## Purpose
Ship the fastest verification baseline by executing checks directly on the host workspace.

## In scope
- Verification command orchestration in `crates/aetherd`
- Config for allowed host commands in `crates/aether-config`
- Reporting pass/fail and logs via `crates/aether-mcp`

## Out of scope
- Container/microVM isolation (later stages)
- Cross-machine reproducibility guarantees

## Pass criteria
1. A verification request can run configured commands and return structured status + logs.
2. Failed commands propagate exit codes and captured output.
3. Tests validate command allowlist behavior and status mapping.
4. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase3-stage3-1-host-verification off main.
3) Create worktree ../aether-phase3-stage3-1-host for that branch and switch into it.
4) Implement host verification in existing crates only:
   - command orchestration in crates/aetherd
   - config knobs in crates/aether-config
   - result tool output in crates/aether-mcp
5) Add tests for command success/failure and output capture.
6) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
7) Commit with message: "Add host-based verification mode".
```
