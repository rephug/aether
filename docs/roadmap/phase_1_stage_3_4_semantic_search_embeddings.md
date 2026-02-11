# Phase 1 - Stage 3.4: Semantic Search (Embeddings)

## Purpose
Add optional semantic retrieval over SIR content while keeping lexical search as the reliable baseline.

## In scope
- Embedding provider hooks in `crates/aether-infer`
- Embedding storage/query paths in `crates/aether-store`
- CLI and MCP access in `crates/aetherd` and `crates/aether-mcp`
- Config switches in `crates/aether-config`

## Out of scope
- External vector database integration
- Heavy reranking pipelines

## Pass criteria
1. Mock embedding mode works fully offline and deterministic in tests.
2. Embeddings update when SIR hash changes and delete on symbol removal.
3. Semantic query returns expected top matches in fixture tests.
4. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase1-stage3-4-semantic-search off main.
3) Create worktree ../aether-phase1-stage3-4-semantic for that branch and switch into it.
4) Implement semantic search in existing crates only:
   - embedding provider + mock in crates/aether-infer
   - embedding table and query logic in crates/aether-store
   - CLI/MCP entrypoints in crates/aetherd and crates/aether-mcp
   - config flags in crates/aether-config
5) Add tests for deterministic offline semantic search and incremental updates.
6) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
7) Commit with message: "Add optional semantic search with embeddings".
```
