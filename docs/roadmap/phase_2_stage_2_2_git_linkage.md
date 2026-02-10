# Phase 2 - Stage 2.2: Git Linkage

## Purpose
Attach SIR history entries to relevant git commits so symbol timelines can answer "what changed when".

## In scope
- Commit metadata capture in `crates/aetherd` pipeline
- Storage/query fields in `crates/aether-store`
- MCP-accessible linkage fields in `crates/aether-mcp`

## Out of scope
- Full PR/provider integrations
- Cross-repo monorepo federation

## Pass criteria
1. Timeline records include commit hash for version entries when git data is available.
2. Fixture tests with multiple commits return expected commit order/hash values.
3. Missing git data is handled explicitly (null/unknown), not as silent failure.
4. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase2-stage2-2-git-linkage off main.
3) Create worktree ../aether-phase2-stage2-2-git-linkage for that branch and switch into it.
4) Implement git linkage in existing crates only:
   - capture commit context in crates/aetherd
   - store/query linkage in crates/aether-store
   - expose fields in crates/aether-mcp
5) Add tests on a temp git repo fixture with multiple commits.
6) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
7) Commit with message: "Link symbol history to git commits".
```
