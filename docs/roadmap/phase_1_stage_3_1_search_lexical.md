# Phase 1 - Stage 3.1: Lexical Search

## Purpose
Add practical symbol search over local indexed data, exposed via CLI and MCP.

## In scope
- Search query/storage logic in `crates/aether-store`
- CLI wiring in `crates/aetherd`
- MCP tool wiring in `crates/aether-mcp`
- Search result model reuse in existing crates

## Out of scope
- Embeddings and semantic ranking (Stage 3.4)
- UI styling or extension quick-pick UX (Stage 3.7)

## Pass criteria
1. Query by symbol name/qualified name/file path/language returns expected symbols.
2. Rename/remove operations are reflected in search results after re-index.
3. CLI and MCP both expose stable structured fields (`symbol_id`, `qualified_name`, `file_path`, `language`, optional summary).
4. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase1-stage3-1-lexical-search off main.
3) Create worktree ../aether-phase1-stage3-1-search for that branch and switch into it.
4) Implement lexical search using existing crates only:
   - add search query support in crates/aether-store
   - expose CLI search in crates/aetherd
   - add MCP tool aether_search in crates/aether-mcp
5) Add tests for: lookup success, rename updates, and removal cleanup.
6) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
7) Commit with message: "Add lexical search via CLI and MCP".
```
