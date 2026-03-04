# Phase 8 Session Context — The Crucible (Hardening & Scale)

## What You Need to Know

I'm building AETHER, a Rust multi-crate workspace that creates persistent semantic intelligence for codebases. We're in Phase 8 (The Crucible). I use OpenAI Codex CLI for implementation and need you to produce verified Codex prompts.

**Repo:** `https://github.com/rephug/aether` at `/home/rephu/projects/aether`
**Dev environment:** WSL2 Ubuntu, mold linker, sccache, all builds from `/home/rephu/`

**Required build environment (must be set before any cargo command):**
```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

## What Just Happened

- **Phase 7 complete** — All stages 7.1–7.9 merged:
  - 7.1: Store pooling + SharedState refactor
  - 7.2: SurrealDB 3.0 migration (replaces CozoDB/sled)
  - 7.3: aether-query read-only MCP-over-HTTP server
  - 7.4: Document abstraction layer (`aether-document` crate)
  - 7.5: AETHER Legal (clause parser + CIR)
  - 7.6: Web dashboard (HTMX + D3 + Tailwind, Decision #41)
  - 7.8: OpenAI-compatible inference provider
  - 7.9: Dashboard visual intelligence (X-Ray, Blast Radius, Architecture Map, Time Machine, Causal Explorer, Smart Search)
- **Hardening passes 1–6** — 60+ fixes across security, performance, correctness, concurrency (all merged)
- **Inference fix** — Updated default Gemini model to `gemini-flash-latest`, fixed Ollama model name, added SIR pipeline tracing
- **Post-7.4 cleanup** — `aether-graph-algo` leaf crate, MCP verification feature-gating
- **Dashboard revision** — Consolidating into 5 Codex runs for non-technical user accessibility

## Phase 8 Mission

Phase 8 halts lateral expansion (Legal, Finance verticals deferred) to focus entirely on **reliability, synchronization, and scale**. AETHER must survive crashes without state corruption and deliver value on large repositories without blocking on multi-hour indexing times.

## Current Architecture — Key Components

### Three-Database Stack
- **SQLite** (`crates/aether-store/src/sqlite.rs`): Relational metadata, SIR storage, symbol table, project notes. Schema version 2 with `document_units` + `semantic_records` tables.
- **LanceDB** (`crates/aether-store/src/vector.rs`): Vector embeddings for semantic search. Disk-backed HNSW. Table naming: `{provider}_{model}` (sanitized).
- **SurrealDB** (`crates/aether-store/src/graph_surreal.rs`): Dependency graph with SurrealKV embedded backend. SCHEMAFULL tables: `symbol`, `depends_on`, `co_change`, `tested_by`, `community_snapshot`, `document_node`, `document_edge`.

**Critical gap:** No cross-database write coordination. A crash mid-pipeline can leave symbols in SQLite without corresponding vectors in LanceDB or graph nodes in SurrealDB.

### Indexing Pipeline (`crates/aetherd/src/indexer.rs`)
Current flow is monolithic — for each changed file:
1. Parse AST (tree-sitter) → extract symbols
2. Generate SIR (LLM inference) → store in SQLite
3. Generate embeddings → store in LanceDB
4. Update graph edges → store in SurrealDB

All four steps happen synchronously per symbol. No tiered processing, no priority queue.

### SharedState (`crates/aether-mcp/src/state.rs`)
Holds `Arc<SqliteStore>`, `Arc<dyn GraphStore>`, vector store, config. Constructors: `open_readwrite()`, `open_readonly()`, async variants.

### SIR Pipeline (`crates/aetherd/src/sir_pipeline.rs`)
Processes symbol change events. Calls inference provider, validates JSON, stores SIR + embeddings. Uses `SirQualityMonitor` for confidence tracking.

### Graph Algorithms (`crates/aether-graph-algo/`)
Leaf crate with zero internal deps. Exports: PageRank, Louvain, SCC, connected components, BFS shortest path, cross-community edges.

### Inference Providers (`crates/aether-infer/src/lib.rs`)

**Provider table (Phase 8 removes Mock, adds Tiered):**

| Provider | Config value | Use case | Rate limited? |
|----------|-------------|----------|---------------|
| Gemini | `gemini` | Google Gemini Flash cloud API | Yes (15 req/min free) |
| OpenAI-Compat | `openai_compat` | NVIDIA NIM, OpenRouter, NanoGPT, any OpenAI-compatible API | Depends on service (NIM: 40 req/min free) |
| Ollama | `qwen3_local` | Local Ollama or remote Ollama server, fully offline capable | No — unlimited |
| Tiered | `tiered` | Cloud primary + Ollama fallback (NEW in Phase 8) | Cloud portion only |
| Auto | `auto` | Gemini if key present, else error (was: else Mock — now errors clearly) | Same as Gemini |

**Mock provider is removed in Phase 8** (Decision #44). It produced fake data, operated via a different code path (file-level instead of per-symbol), and masked real inference pipeline bugs.

**Default local model:** `qwen3.5:9b` (replaces `qwen2.5-coder:7b-instruct-q4_K_M` — Decision #45)
**NVIDIA NIM model:** `qwen3.5-397b-a17b` (full flagship quality via NIM free tier)

All providers have HTTP timeouts (10s connect, 120s request) from hardening pass 5.

**How to switch providers — one line in config.toml:**
```toml
# ── Gemini ──
[inference]
provider = "gemini"
model = "gemini-flash-latest"
api_key_env = "GEMINI_API_KEY"

# ── NVIDIA NIM (or any OpenAI-compatible service) ──
[inference]
provider = "openai_compat"
model = "qwen3.5-397b-a17b"
endpoint = "https://integrate.api.nvidia.com/v1"
api_key_env = "NVIDIA_NIM_API_KEY"

# ── Ollama (local) ──
[inference]
provider = "qwen3_local"
model = "qwen3.5:9b"
endpoint = "http://127.0.0.1:11434"

# ── Tiered: NIM for important symbols, Ollama for the rest ──
[inference]
provider = "tiered"
[inference.tiered]
primary = "openai_compat"
primary_model = "qwen3.5-397b-a17b"
primary_endpoint = "https://integrate.api.nvidia.com/v1"
primary_api_key_env = "NVIDIA_NIM_API_KEY"
primary_threshold = 0.8
fallback_model = "qwen3.5:9b"
fallback_endpoint = "http://127.0.0.1:11434"
retry_with_fallback = true

# ── Tiered: Gemini for important symbols, Ollama for the rest ──
[inference]
provider = "tiered"
[inference.tiered]
primary = "gemini"
primary_model = "gemini-flash-latest"
primary_api_key_env = "GEMINI_API_KEY"
primary_threshold = 0.8
fallback_model = "qwen3.5:9b"
fallback_endpoint = "http://127.0.0.1:11434"
retry_with_fallback = true
```

Or override from CLI: `aetherd --inference-provider gemini` (etc.)

### Dashboard (`crates/aether-dashboard/`)
HTMX + D3 + Tailwind. 11 pages. Feature-gated behind `--features dashboard`. Axum router mounted in aetherd.

### aether-query (`crates/aether-query/`)
Read-only MCP-over-HTTP server. Separate binary from aetherd. Uses SharedState in readonly mode.

## Crate Architecture (Current)

```
SHARED ENGINE (domain-agnostic)
├── aether-core         # Symbol/document model, stable IDs, diffing
├── aether-config       # Config loader
├── aether-store        # SQLite + LanceDB + SurrealDB
├── aether-infer        # Provider traits (Gemini, Ollama, OpenAI-compat, Tiered)
├── aether-memory       # Project notes, session context
├── aether-analysis     # Drift, coupling, health analysis
├── aether-document     # Domain-agnostic document traits + embedding pipeline
├── aether-graph-algo   # Graph algorithms (leaf crate, zero internal deps)
├── aether-mcp          # MCP tool definitions (shared by aetherd + aether-query)
└── aether-dashboard    # Web dashboard (feature-gated)

CODE-SPECIFIC
├── aether-parse        # tree-sitter parsing (Rust, TypeScript/JS, Python)
├── aether-sir          # SIR schema and validation
├── aether-lsp          # LSP hover server
├── aether-git          # Git coupling, commit linkage
└── aetherd             # Daemon binary

TEAM INFRASTRUCTURE
└── aether-query        # Read-only query server binary
```

## Key Files Per Stage

### Stage 8.1 (State Reconciliation)
- `crates/aether-store/src/sqlite.rs` — Add write intent log table + migration
- `crates/aether-store/src/lib.rs` — Coordinated write trait/methods
- `crates/aether-store/src/vector.rs` — Wrap vector writes
- `crates/aether-store/src/graph_surreal.rs` — Wrap graph writes
- `crates/aetherd/src/sir_pipeline.rs` — Use coordinated write path
- `crates/aetherd/src/cli.rs` — Add `fsck` subcommand
- New: `crates/aetherd/src/fsck.rs` — State verification logic

### Stage 8.2 (Progressive Indexing + Tiered Providers)
- `crates/aetherd/src/indexer.rs` — Tiered indexing (Pass 1 AST-only, Pass 2 background SIR)
- New: `crates/aetherd/src/priority_queue.rs` — Git-aware priority queue
- `crates/aetherd/src/sir_pipeline.rs` — Background worker + on-demand bump
- `crates/aether-infer/src/lib.rs` — Remove MockProvider, add TieredProvider, update default model
- `crates/aether-config/src/lib.rs` — Remove `Mock` variant, add `Tiered` variant + `[inference.tiered]` section
- `crates/aether-mcp/src/lib.rs` — On-demand SIR trigger from MCP queries
- `crates/aether-lsp/src/lib.rs` — On-demand SIR trigger from hover
- `crates/aether-dashboard/` — "SIR Pending" status display

### Stage 8.7 (Stress Test Harness)
- New: `tests/stress/` — Benchmark scripts
- New: `tests/stress/run_benchmark.sh` — Orchestrator
- New: `tests/stress/repos.toml` — Repo definitions

## Build + Test Commands

```bash
# Per-crate testing (OOM-safe for WSL2 with 12GB RAM):
cargo fmt --all --check
cargo clippy --workspace -- -D warnings
cargo test -p aether-core
cargo test -p aether-config
cargo test -p aether-graph-algo
cargo test -p aether-document
cargo test -p aether-store
cargo test -p aether-parse
cargo test -p aether-sir
cargo test -p aether-infer
cargo test -p aether-lsp
cargo test -p aether-analysis
cargo test -p aether-memory
cargo test -p aether-mcp
cargo test -p aether-query
cargo test -p aetherd

# With dashboard:
cargo clippy --workspace --features dashboard -- -D warnings
cargo test -p aether-dashboard
```

Do NOT use `cargo test --workspace` (OOM risk on WSL2).

## Decisions Reference

Key decisions for Phase 8 (see DECISIONS_v4.md for full register):
- **#38:** SurrealDB 3.0 with SurrealKV (replaces CozoDB/sled)
- **#39:** pdfium-render for PDF fallback
- **#40:** HTTP/SSE for MCP transport
- **#41:** HTMX + D3.js + Tailwind CSS for dashboard (no SPA framework)
- **#42:** SurrealDB Record References for bidirectional traversal
- **#43:** SurrealDB Computed Fields for derived properties
- **#44 (NEW):** Remove Mock provider — dead code that masked real pipeline bugs
- **#45 (NEW):** Default local model updated to `qwen3.5:9b` (from `qwen2.5-coder:7b-instruct-q4_K_M`)
- **#46 (NEW):** Tiered provider for cloud+local hybrid inference

## Git Workflow

```bash
# Standard stage workflow:
git status --porcelain          # must be clean
git pull --ff-only              # sync main

# Codex creates branch + worktree, implements, validates
# After merge:
git switch main
git pull --ff-only
git worktree remove <path>
git branch -d <branch>
```

---

*Last updated: 2026-03-03 — Phase 8 planning*
