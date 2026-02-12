# Phase 4: The Architect (Infrastructure Alignment)

## Purpose
Close the gap between the running codebase and the V3.0 Engineering Prospectus. Replace temporary scaffolding (brute-force vectors, `eprintln!`, shell-out git) with the production infrastructure the prospectus specifies.

## In scope
- Vector storage migration from SQLite brute-force to LanceDB in `crates/aether-store`
- Structured logging via `tracing` across all crates
- Native git operations via `gix` in `crates/aetherd`
- Dependency edge extraction (CALLS, DEPENDS_ON) from tree-sitter AST in `crates/aether-parse`
- Graph storage via CozoDB for symbol relationships (replaces archived KuzuDB)
- SIR file/module hierarchy rollup in `crates/aether-sir` and `crates/aether-store`

## Out of scope
- Ticket/PR API connectors (Phase 5)
- Candle local embeddings / reranker (Phase 5)
- Event bus refactor (Phase 5)
- New language support (Phase 5)

## Pass criteria
1. Semantic search uses LanceDB ANN index, not brute-force cosine similarity.
2. All `eprintln!` replaced with `tracing` macros; structured JSON log output available.
3. Git commit resolution uses `gix` library, not `std::process::Command("git")`.
4. Tree-sitter extracts function call edges and import/dependency edges.
5. Symbol relationships are queryable ("what calls X?", "what does X depend on?").
6. File-level SIR rollups aggregate leaf SIR annotations.
7. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase-4-architect-rollup off main.
3) Create worktree ../aether-phase-4-architect for that branch and switch into it.
4) Implement Phase 4 by completing stage docs in this order:
   - docs/roadmap/phase_4_stage_4_1_lancedb_vector_backend.md
   - docs/roadmap/phase_4_stage_4_2_structured_logging.md
   - docs/roadmap/phase_4_stage_4_3_native_git.md
   - docs/roadmap/phase_4_stage_4_4_dependency_extraction.md
   - docs/roadmap/phase_4_stage_4_5_graph_storage.md
   - docs/roadmap/phase_4_stage_4_6_sir_hierarchy.md
5) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
6) Commit with message: "Complete Phase 4 Architect rollout".
```
