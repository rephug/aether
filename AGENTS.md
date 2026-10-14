# AETHER — Codex Context

## What this project is

Semantic intelligence engine for codebases. Indexes symbols into Semantic Intent
Records (SIRs), stores relationships across SQLite + SurrealDB + vector store,
and surfaces intelligence via LSP, MCP tools, CLI, and a web dashboard.

Single developer, ~160K lines of Rust across a multi-crate workspace.

## Repo structure

Rust workspace with 17 crates under `crates/`.

Key crates and their roles:

- `aetherd` — CLI binary, daemon, command dispatch
- `aether-core` — symbol model, stable BLAKE3 IDs, diffing
- `aether-config` — TOML configuration loading and validation (13 modules after refactor)
- `aether-parse` — tree-sitter AST parsing (Rust, TypeScript, Python)
- `aether-sir` — SIR schema and validation
- `aether-infer` — inference provider abstraction (Gemini native, OpenAI-compat, Ollama)
- `aether-store` — SQLite + SurrealDB + vector storage (13 modules after refactor)
- `aether-mcp` — stdio MCP server for AI agents (one module per tool family under `src/tools/`)
- `aether-analysis` — drift detection, causal chains, graph health, community detection
- `aether-memory` — project notes, session context, hybrid search
- `aether-lsp` — Language Server Protocol implementation
- `aether-dashboard` — HTMX + D3.js + Tailwind CSS web dashboard

## Code conventions

### Must follow

- No `unwrap()` or `expect()` in library crate code — use `anyhow::Result` or `thiserror`
- SIR is canonical JSON with sorted keys, BLAKE3 hashed
- `Store` trait in `aether-store` abstracts all persistence
- `InferenceProvider` and `EmbeddingProvider` traits abstract AI backends
- Config loaded from `.aether/config.toml` via `aether-config`
- Error types use `thiserror` in libraries, `anyhow` in binaries
- Per-crate cargo commands only — never `--workspace` (OOM risk on constrained build machines)
- Missing error context is a bug: prefer `.context("what failed")` over bare `?`

### Patterns to watch for in reviews

- **God files:** flag any single file over 1500 lines as a refactor candidate
- **Blocking in async:** no `std::thread::sleep` or blocking I/O in async functions
- **Clone proliferation:** prefer borrowing; flag unnecessary `.clone()` calls
- **Feature gate awareness:** some code is behind feature flags (`legacy-cozo`, `verification`)
- **Concurrency:** `aether-store` operations may be called from multiple async tasks — watch for lock contention
- **Semantic drift:** if a function's name no longer describes what it does after a change, flag it

### Do NOT suggest

- Adding new crates without explicit instruction
- Replacing SurrealDB with another graph database
- Replacing tree-sitter with another parser
- Using `unwrap()` or `expect()` in library crate code
- Global workspace builds or tests (`--workspace` flag)
- Model or provider recommendations (developer handles model selection)

## Validation commands

For every crate you change, before committing:

```bash
cargo fmt --all --check
cargo clippy -p <crate> --all-targets -- -D warnings
cargo test -p <crate>
```

For `aetherd`, also run clippy and tests with `--features dashboard`, as CI does.

Never run `cargo test --workspace` or `cargo clippy --workspace` — run per-crate, one
cargo job at a time when worktrees share a target directory.

## Architecture decisions (locked — do not suggest alternatives)

- **Embeddings:** Gemini Embedding 2 (`gemini-embedding-2-preview`, 3072-dim)
- **Triage inference:** `gemini-3.1-flash-lite` (cloud), Claude Sonnet (premium deep pass)
- **Pipeline:** three-pass named scan → triage → deep
- **Databases:** SQLite denormalized hot path + SurrealDB 3.0/SurrealKV for graph queries
- **Vector store:** SQLite (active), LanceDB (intended backend, deferred due to perf bug)
- **MCP transport:** stdio (`aether-mcp`); `aether-query` serves read-only MCP over streamable HTTP (`POST /mcp`)
- **Dashboard:** HTMX + D3.js + Tailwind CSS
- **Build tools:** mold linker, sccache, cargo-nextest

## Build environment

Suggested build settings (relevant if Codex needs to compile):

```bash
export CARGO_TARGET_DIR="$HOME/aether-target"   # outside the repository
export CARGO_BUILD_JOBS=2                       # raise it on machines with more memory
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR="$CARGO_TARGET_DIR/tmp"
mkdir -p "$TMPDIR"
```

Do NOT use `/tmp/` for build artifacts (RAM-backed tmpfs in WSL2, causes OOM).
Do NOT use `/mnt/` paths for build targets (Windows 9P filesystem, too slow).

## Current state

Core schema 21; agent template schema 4; `aether-mcp` exposes 39 tools (40 with the
`verification` feature).

Shipped: Phases 1–8 (apart from the deferred stages, the 7.5/7.7 vertical crates and
8.18), Phase 9 stages 9.1–9.5 (Tauri desktop app), Phase 10 stages 10.1–10.7 (batch,
continuous intelligence, agent hooks, Seismograph, intent contracts, task context,
batch providers), the audit and contract MCP tools, the omp inference provider (#149),
and Phase WF's first stage, the `/scan` zero-key onboarding command (#152).

Planned, with specs in `docs/roadmap/`: the rest of Phase WF
(`phase_wf_the_companion.md`), 5.6b, 8.18 (Boundary Leaker fix), 9.6, 10.1b, 10.5b
(implicit contracts, part of the 10.5 spec) and Phase 11.
