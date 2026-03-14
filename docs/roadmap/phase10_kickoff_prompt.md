# Phase 10 — The Conductor: Chat Session Kickoff

## Who I Am

I'm Robert (GitHub: rephug), sole developer of AETHER — a Rust multi-crate workspace (~90K LOC, 16 crates) that creates persistent semantic intelligence for codebases. Symbols are indexed into Semantic Intent Records (SIRs) capturing purpose/behavior/edge-cases. Data lives across SurrealDB (graph edges), LanceDB (vectors), and SQLite (metadata + SIR records). Served via LSP, MCP, and a web dashboard with 27+ pages.

## My Setup

- **Dev machine:** WSL2 Ubuntu 24.04, MSI with RTX 2070 (8GB VRAM)
- **Repo:** `github.com/rephug/aether` at `/home/rephu/projects/aether`
- **Servers:** 2x netcup (64GB DDR5, no GPU) — for batch/nightly jobs
- **Implementation agent:** OpenAI Codex CLI (plan mode first, then implementation)
- **Planning/verification:** Claude (you), with Gemini Deep Think for mathematical/algorithmic review
- **Build env:** Always set before any cargo command:
  ```bash
  export CARGO_TARGET_DIR=/home/rephu/aether-target
  export CARGO_BUILD_JOBS=2
  export PROTOC=$(which protoc)
  export RUSTC_WRAPPER=sccache
  export TMPDIR=/home/rephu/aether-target/tmp
  mkdir -p $TMPDIR
  ```
- **Tests:** Per-crate only (`cargo test -p <crate>`), never `--workspace` (OOM)
- **SurrealKV:** Exclusive lock — `pkill -f aetherd` before CLI commands if dashboard running

## Current State

Phase 8 ("The Crucible") is complete. Key deliverables:
- Three-pass SIR pipeline: scan (flash-lite) → triage (flash-lite enriched) → deep (Sonnet)
- 3748 symbols, 100% SIR coverage, 3623 triage-enriched, 46 deep
- Community detection with component-bounded semantic rescue at threshold 0.90
- Gemini Embedding 2 (3072-dim) locked as production embedding model
- Structural + semantic health scoring with archetypes
- TYPE_REF + IMPLEMENTS edge extraction
- Dashboard with 27+ pages including blast radius, architecture map, time machine, X-ray
- 5 God File refactors completed (aether-store, aether-infer, aether-mcp, aether-config, sir_pipeline)
- Decision register through #89, with Phase 9 decisions renumbered to #90-96

**Known issues still open:**
- Triage concurrency not actually parallelizing despite config setting
- `--suggest-splits` crashes with SurrealKV lifetime bug
- Health scores regressed after refactoring due to git blame distortion
- 11/16 Boundary Leaker false positives (Phase 8.18 fix specced but may not be merged yet)

## What Phase 10 Is

"The Conductor" — three stages making the intelligence layer autonomous:

**10.1 — Batch Index Pipeline + Watcher Intelligence**
- `aetherd batch extract/build/ingest/run` subcommands for Gemini Batch API (50% off)
- Prompt hashing via BLAKE3 — skip unchanged symbols on rebuild (biggest cost optimization)
- Pre-fetch dictionary for neighbor context (kills N+1 query problem)
- `[watcher]` config: separate premium model for file-save events, git triggers via gix
- 3s settling debounce, file event suppression during git ops
- `sir_fingerprint_history` SQLite table for change tracking

**10.2 — Continuous Intelligence**
- Noisy-OR staleness scoring with hard gates + logistic sigmoid time decay
- Semantic-gated neighbor propagation (Δ_sem × γ decay, BFS with cutoff)
- Cold-start volatility prior from git churn
- Predictive staleness from temporal coupling matrix
- Log-dampened PageRank as priority tiebreaker
- `aetherd continuous run-once` for nightly cron on netcup servers
- Fingerprint history consumption for volatility detection

**10.3 — Agent Integration Hooks**
- `aetherd sir context` — token-budgeted context assembly (greedy knapsack, 9-tier priority)
- `aetherd sir inject` — direct SIR update with synchronous re-embedding
- `aetherd sir diff` — structural comparison via tree-sitter (no inference)

## Documents in Project Knowledge

The following Phase 10 documents should be in this project's knowledge base. Search for them:
- `phase_10_conductor_v2.md` — Phase overview
- `phase_10_stage_10_1_batch_index_v2.md` — Stage 10.1 spec
- `phase_10_stage_10_2_continuous_intelligence_v2.md` — Stage 10.2 spec
- `phase_10_stage_10_3_agent_hooks_v2.md` — Stage 10.3 spec
- `phase10_stage10_1_session_context.md` — 10.1 session context
- `phase10_stage10_1_codex_prompt.md` — 10.1 Codex prompt
- `phase10_stage10_2_session_context.md` — 10.2 session context
- `phase10_stage10_2_codex_prompt.md` — 10.2 Codex prompt
- `phase10_stage10_3_session_context.md` — 10.3 session context
- `phase10_stage10_3_codex_prompt.md` — 10.3 Codex prompt

## CRITICAL: Clone the repo

The project knowledge docs reference specific file paths, function names, and module structures. These may have drifted since the docs were written. **Always clone the repo and verify against actual source code before making claims:**

```bash
git clone https://github.com/rephug/aether.git /home/claude/aether
```

Key files to inspect before answering any implementation question:
- `crates/aether-config/src/root.rs` — AetherConfig struct
- `crates/aetherd/src/cli.rs` — Commands enum (~line 444)
- `crates/aetherd/src/main.rs` — run_subcommand dispatch (~line 302)
- `crates/aetherd/src/sir_pipeline/infer.rs` — build_job, prompt construction
- `crates/aetherd/src/sir_pipeline/persist.rs` — SIR upsert
- `crates/aetherd/src/indexer.rs` — file watcher
- `Cargo.toml` — workspace members and dependencies

## Key Principles

- **Read actual source before making claims.** Specs go stale. Always inspect live code.
- **Distinguish proven facts from hypotheses.** Label claims explicitly.
- **Codex prompts must be narrowly scoped** with explicit source inspection sections and per-crate cargo commands (never `--workspace`).
- **Scripts with heredocs inside loops** must be written to files, not pasted interactively.
- **Per-crate cargo commands always** — never `--workspace` due to OOM risk.
- **Health checks after each stage** to track progression.
- **When finishing a stage**, always provide the complete git cleanup sequence AND a descriptive PR title and body for the GitHub PR.

## Decision Register

Decisions #44-51 (Phase 8 addendum), #52-69 (Reflex/Library/Oracle architecture), #83-89 (Phase 8.15/8.16), #90-96 (Phase 9 Tauri). Phase 10 decisions have not been assigned numbers yet — they'll start after the last used number.

## What I Need From You

I'm starting Phase 10 implementation. Help me:
1. Review and validate the specs against actual repo state before I run Codex
2. Answer implementation questions as they come up
3. Adjudicate if Codex or other AI tools disagree on approach
4. Produce updated Codex prompts if the specs need adjustment after repo inspection
5. Track what ships and what's still pending

Start by cloning the repo and confirming the session context for Stage 10.1 matches reality. Then we'll decide if 10.1 is ready to hand to Codex or if the prompt needs adjustments first.
