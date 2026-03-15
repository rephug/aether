# AETHER — Codex Context

## What this project is

Semantic intelligence engine for codebases. Indexes symbols into Semantic Intent
Records (SIRs), stores relationships across SQLite + SurrealDB + vector store,
and surfaces intelligence via LSP, MCP tools, CLI, and a web dashboard.

Single developer, ~90K+ lines of Rust across a multi-crate workspace.

## Repo structure

Rust workspace with 16+ crates under `crates/`.

Key crates and their roles:

- `aetherd` — CLI binary, daemon, command dispatch
- `aether-core` — symbol model, stable BLAKE3 IDs, diffing
- `aether-config` — TOML configuration loading and validation (13 modules after refactor)
- `aether-parse` — tree-sitter AST parsing (Rust, TypeScript, Python)
- `aether-sir` — SIR schema and validation
- `aether-infer` — inference provider abstraction (Gemini native, OpenAI-compat, Ollama)
- `aether-store` — SQLite + SurrealDB + vector storage (13 modules after refactor)
- `aether-mcp` — MCP tools for AI agents (~4800 lines, refactor pending)
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

Per-crate validation (use during iterative development):

```bash
cargo fmt --all --check
cargo clippy -p <crate> -- -D warnings
cargo test -p <crate>
```

Full validation (before committing / PR):

```bash
cargo fmt --all --check
cargo clippy -p <crate> -- -D warnings
cargo test -p <crate>
```

Never run `cargo test --workspace` or `cargo clippy --workspace` — run per-crate.

## Architecture decisions (locked — do not suggest alternatives)

- **Embeddings:** Gemini Embedding 2 (`gemini-embedding-2-preview`, 3072-dim)
- **Triage inference:** `gemini-3.1-flash-lite` (cloud), Claude Sonnet (premium deep pass)
- **Pipeline:** three-pass named scan → triage → deep
- **Databases:** SQLite denormalized hot path + SurrealDB 3.0/SurrealKV for graph queries
- **Vector store:** SQLite (active), LanceDB (intended backend, deferred due to perf bug)
- **MCP transport:** HTTP/SSE
- **Dashboard:** HTMX + D3.js + Tailwind CSS
- **Build tools:** mold linker, sccache, cargo-nextest

## Build environment

Build settings for WSL2 development environment (relevant if Codex needs to compile):

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

Do NOT use `/tmp/` for build artifacts (RAM-backed tmpfs in WSL2, causes OOM).
Do NOT use `/mnt/` paths for build targets (Windows 9P filesystem, too slow).

## Current state

Phase 8 ("The Synthesizer") is active development. Recent work includes:
- God File refactors of `aether-store` and `aether-config` (both split into 13 modules)
- Embedding model validation (Gemini Embedding 2 locked at 3072-dim)
- Community detection quality overhaul
- TYPE_REF and IMPLEMENTS edge extraction
- Triage concurrency fix (14x speedup)

Pending work: `aether-mcp` refactor, Boundary Leaker false positive fix, health score formula adjustment.
