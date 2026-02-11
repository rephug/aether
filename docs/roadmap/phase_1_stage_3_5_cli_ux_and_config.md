# Phase 1 - Stage 3.5: CLI UX and Config

## Purpose
Make CLI flows predictable for humans and automation, with stable config behavior.

## In scope
- Command/flag behavior and output consistency in `crates/aetherd`
- Config defaults and schema evolution in `crates/aether-config`
- Documentation updates in root `README.md`

## Out of scope
- New editor UI features (Stage 3.7)
- Deep protocol changes to LSP/MCP (Stage 3.6)

## Pass criteria
1. One-shot commands exit deterministically (`--index-once`, search modes, status output).
2. Missing `.aether/config.toml` is created without overwriting existing user values.
3. JSON output mode is stable for scripting.
4. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Implementation Notes
- `aetherd` exposes `--index-once` for deterministic one-shot indexing.
- `aetherd --search` supports `--output table|json`; JSON output uses a stable envelope:
  - `mode_requested`
  - `mode_used`
  - `fallback_reason`
  - `matches`
- Semantic/hybrid fallback reasons are aligned across CLI and MCP.
- `aether-config` keeps schema backward compatibility and adds non-fatal `validate_config` warnings.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase1-stage3-5-cli-ux-config off main.
3) Create worktree ../aether-phase1-stage3-5-cli for that branch and switch into it.
4) Implement in existing crates only:
   - tighten CLI command/exit behavior in crates/aetherd
   - harden config defaults/migration in crates/aether-config
   - document usage updates in README.md
5) Add tests for exit behavior, config bootstrapping, and JSON output shape.
6) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
7) Commit with message: "Harden CLI UX and config behavior".
```
