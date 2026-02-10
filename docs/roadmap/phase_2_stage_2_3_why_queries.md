# Phase 2 - Stage 2.3: Why Queries

## Purpose
Provide structured "why changed" responses by combining SIR history and commit linkage.

## In scope
- Query composition in `crates/aether-mcp`
- Optional concise hover/timeline hints in `crates/aether-lsp`
- Supporting retrieval logic in `crates/aether-store`

## Out of scope
- Large-language-model generated narratives requiring network calls
- Auto-remediation or code rewrite suggestions

## Pass criteria
1. MCP exposes a deterministic "why" query returning prior/current summary + change context.
2. Query handles symbols with no prior version gracefully.
3. Response schema is tested and backward compatible with existing tools.
4. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase2-stage2-3-why-queries off main.
3) Create worktree ../aether-phase2-stage2-3-why for that branch and switch into it.
4) Implement why-query support in existing crates only:
   - retrieval assembly in crates/aether-store
   - MCP tool/response in crates/aether-mcp
   - optional compact LSP hint in crates/aether-lsp
5) Add tests for schema shape and no-history fallback behavior.
6) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
7) Commit with message: "Add why-changed query support".
```
