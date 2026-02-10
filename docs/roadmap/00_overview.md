# docs/roadmap/00_overview.md

# AETHER Roadmap Overview (Phase + Stage)

## Definitions
- **Phase** = a major product milestone that changes what AETHER *is* (Observer → Historian → Ghost).
- **Stage** = a shippable chunk inside a Phase, sized for 1–3 Codex CLI runs.

## Current state (assumed)
Phase 1 (Observer) is already in place:
- Watch repo → parse Rust/TS via tree-sitter → stable symbol IDs
- Generate SIR per changed symbol and store locally
- Serve via LSP/MCP

## Phase plan
### Phase 1 — Observer (Understanding + Retrieval)
Purpose: keep a continuously-updated “intent layer” for the repo and expose it to tools.

Stages:
- Stage 1: Indexing + stable symbol identity (DONE)
- Stage 2: Generate + persist SIR + offline tests (DONE)
- Stage 3: Retrieval & search (YOU ARE HERE)
  - Stage 3.1: Search Stage 1 (Lexical search via CLI + MCP)
  - Stage 3.2: Robustness polish (stale SIR + rate limiting)

### Phase 2 — Historian (Why changed)
Purpose: store and query the evolution of meaning over time (versions, commits, PRs).

Stages:
- Stage 2.1: SIR versioning (store revisions when sir_hash changes)
- Stage 2.2: Git linkage (commit/PR mapping per symbol)
- Stage 2.3: “why” queries surfaced in MCP/LSP

### Phase 3 — Ghost (Verification)
Purpose: verify changes/suggestions in an isolated execution environment before marking them “safe”.

Stages:
- Stage 3.1: Host-based verification (fast checks, no sandbox)
- Stage 3.2: Containerized verification (Docker baseline)
- Stage 3.3: MicroVM acceleration (Firecracker/Hyper-V optional)

## How to use this roadmap
Each Stage has its own doc under docs/roadmap/ with:
- Goal + non-goals
- “Pass” criteria (tests + behavior)
- Exact Codex prompts to run
- Expected commit message(s)
