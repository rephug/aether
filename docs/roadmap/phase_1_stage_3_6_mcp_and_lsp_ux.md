# Phase 1 - Stage 3.6: MCP and LSP UX

## Purpose
Improve how AETHER presents stored intelligence to agents and editors.

## In scope
- Response schema and tool UX in `crates/aether-mcp`
- Hover formatting and stale indicators in `crates/aether-lsp`
- Shared model wiring from existing store/core crates

## Out of scope
- VS Code extension command palette/status bar work (Stage 3.7)
- New verification runtime features (Phase 3)

## Pass criteria
1. MCP outputs include stable fields for status and lookup/search responses.
2. LSP hover output includes clear sections and stale warning when applicable.
3. Unit/integration tests cover JSON shape and hover formatting.
4. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase1-stage3-6-mcp-lsp-ux off main.
3) Create worktree ../aether-phase1-stage3-6-mcp-lsp for that branch and switch into it.
4) Implement in existing crates only:
   - extend MCP response fields and tools in crates/aether-mcp
   - improve hover rendering + stale warning in crates/aether-lsp
   - keep backward compatibility for existing tool names/fields where possible
5) Add tests for MCP JSON shapes and hover text formatting.
6) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
7) Commit with message: "Improve MCP and LSP UX outputs".
```
