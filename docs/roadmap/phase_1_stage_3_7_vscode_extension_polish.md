# Phase 1 - Stage 3.7: VS Code Extension Polish

## Purpose
Polish the extension UX so local indexing/search is discoverable and useful in daily workflows.

## In scope
- Extension activation/runtime behavior in `vscode-extension/src`
- Status bar and command palette wiring in `vscode-extension`
- Build/test scripts and README updates for extension workflows

## Out of scope
- Core Rust protocol redesign
- Marketplace release operations beyond repo docs/scripts

## Pass criteria
1. Status bar reflects index state (`indexing`, `idle`, and stale/error hints when available).
2. Commands exist for index-once and symbol search, and open target locations.
3. Extension build succeeds and smoke tests (or equivalent script) pass.
4. If Rust crates are touched, `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` also pass.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase1-stage3-7-vscode-polish off main.
3) Create worktree ../aether-phase1-stage3-7-vscode for that branch and switch into it.
4) Implement in existing paths only:
   - add status bar + command palette actions in vscode-extension/src
   - wire command flows to existing AETHER CLI/LSP behavior
   - update vscode-extension/README.md if usage changes
5) Run extension checks:
   - cd vscode-extension && npm install
   - cd vscode-extension && npm run build
   - run extension tests/smoke script if present
6) If Rust code changes, also run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
7) Commit with message: "Polish VS Code extension UX".
```
