# docs/roadmap/phase_1_observer.md

# Phase 1 — Observer (Understanding + Retrieval)

## What Phase 1 delivers
AETHER runs locally and continuously maintains a structured summary (“SIR”) per code symbol, updating it incrementally as code changes. It exposes that data through:
- LSP: editor hover & commands
- MCP: tool calls for agents

## Scope
IN SCOPE:
- Stable symbol IDs and incremental updates
- SIR generation (cloud or mock providers)
- Local storage in .aether/
- LSP + MCP access paths
- Search (lexical first, then semantic)

OUT OF SCOPE (for Phase 1):
- Full git-history reasoning and “why changed” attribution (Phase 2)
- Verified execution/sandboxing (Phase 3)

## Phase 1 pass criteria (revised)
A Phase 1 stage “passes” only if ALL are true:

### A) Build & quality gates
- cargo fmt --check passes
- cargo clippy -- -D warnings passes
- cargo test passes workspace-wide

### B) Hermetic behavior (no-network tests)
- All tests pass without any API keys
- Tests do not require network access
- Tests do not write outside a temp workspace

### C) Minimal functional E2E (mock-only)
Using a temp workspace containing both Rust + TS files:
- Indexing runs and produces symbols
- SIR exists for those symbols and can be retrieved later
- After editing one function, only the affected symbol(s) are updated

### D) Interfaces are stable
- CLI flags added by a stage do not break existing CLI behavior
- MCP tool additions are backward compatible

## Stage map inside Phase 1
- Stage 1: Indexing + stable IDs (DONE)
- Stage 2: SIR generation + storage (DONE)
- Stage 3: Retrieval & Search (NEXT)
  - Stage 3.1: Search Stage 1 (Lexical search)
  - Stage 3.2: Robustness polish (stale SIR + rate limiting)

See:
- docs/roadmap/phase_1_stage_3_1_search_lexical.md
- docs/roadmap/phase_1_stage_3_2_robustness.md
