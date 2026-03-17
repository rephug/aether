# AETHER — Claude Code Context

## What this project is

Semantic intelligence engine for codebases. Indexes every symbol into a Semantic Intent Record (SIR), stores relationships across SQLite + SurrealDB + vector store, and surfaces intelligence via LSP, MCP tools, CLI (31 commands), a web dashboard (27+ HTMX pages), and clipboard-ready context export. Single developer, ~113K lines of Rust across a multi-crate workspace.

**Repo:** `github.com/rephug/aether` (public)
**Dev environment:** WSL2 Ubuntu 24.04 on MSI laptop, RTX 2070 8GB, 16GB RAM
**Schema version:** 11

## Crate map (16 crates)

```
crates/
├── aetherd/              # CLI binary + daemon + command dispatch (largest crate)
│   ├── src/main.rs       # Entry point
│   ├── src/cli.rs        # clap subcommand definitions (31 commands, ~2253 lines)
│   ├── src/batch/        # Phase 10.1: batch index pipeline (Gemini Batch API)
│   ├── src/continuous/   # Phase 10.2: drift monitor + staleness scoring
│   ├── src/sir_pipeline/ # Three-pass SIR generation: scan → triage → deep (~2063 lines)
│   ├── src/sir_context.rs    # Phase 10.3/R.1/10.6: shared export engine + task context (~3969 lines — GOD FILE)
│   ├── src/sir_inject.rs     # Phase 10.3: external SIR injection
│   ├── src/sir_agent_support.rs # Shared utilities for agent commands
│   ├── src/indexer.rs        # File watcher + git triggers from 10.1 (~2731 lines — GOD FILE)
│   ├── src/health_score.rs   # Health scoring CLI (~2389 lines — GOD FILE)
│   ├── src/templates/    # Phase 5.6: agent config generators (CLAUDE.md, etc.)
│   ├── src/init_agent.rs # `aether init-agent` command
│   └── src/refactor_prep.rs  # Phase 8.22: deep scan refactor preparation
├── aether-core/          # Symbol model, stable BLAKE3 IDs, GitContext, diffing
├── aether-config/        # TOML config loading (13 modules after God File refactor)
├── aether-parse/         # tree-sitter AST parsing (Rust, TypeScript, Python)
├── aether-sir/           # SIR schema, validation, canonical JSON
├── aether-infer/         # Inference provider abstraction
│   ├── src/providers/    # Gemini native, OpenAI-compat, Ollama
│   ├── src/embedding/    # Embedding providers (Gemini native, OpenAI-compat, candle)
│   └── src/reranker/     # Reranker abstraction
├── aether-store/         # SQLite + SurrealDB + vector storage (13 modules after refactor, 11 sub-traits)
├── aether-mcp/           # MCP tools for AI agents (~8K lines across tools/)
│   └── src/tools/        # 17+ individual tool modules (drift, coupling, search, etc.)
├── aether-analysis/      # Drift detection, causal chains, community detection, coupling
├── aether-health/        # Health scoring engine, split planner, trait clustering
├── aether-memory/        # Project notes, session context, hybrid search
├── aether-query/         # Unified query layer
├── aether-lsp/           # Language Server Protocol implementation (~2077 lines — single file)
├── aether-dashboard/     # HTMX + D3.js + Tailwind CSS web dashboard (27+ pages)
│   └── src/api/          # 27 route modules, served by axum at localhost:3847
├── aether-document/      # Document abstraction for verticals (legal, finance)
└── aether-graph-algo/    # Graph algorithms (Louvain community detection, etc.)
```

## Code conventions

### Must follow

- No `unwrap()` or `expect()` in library crate code — use `anyhow::Result` or `thiserror`
- SIR is canonical JSON with sorted keys, BLAKE3 hashed
- `Store` trait in `aether-store` abstracts all persistence (11 sub-traits after decomposition)
- `InferenceProvider` and `EmbeddingProvider` traits abstract AI backends
- Config loaded from `.aether/config.toml` via `aether-config`
- Error types use `thiserror` in libraries, `anyhow` in binaries
- **Per-crate cargo commands only — never `--workspace`** (OOM risk on constrained build machines)
- Missing error context is a bug: prefer `.context("what failed")` over bare `?`
- Concurrency-safe: `aether-store` operations may be called from multiple async tasks
- Feature gates: `dashboard` (for aether-dashboard dep), `verification`, `legacy-cozo` (dead)

### Patterns to watch for in reviews

- **God files:** flag any single file over 1500 lines as a refactor candidate
- **Blocking in async:** no `std::thread::sleep` or blocking I/O in async functions
- **Clone proliferation:** prefer borrowing; flag unnecessary `.clone()` calls
- **Semantic drift:** if a function's name no longer describes what it does after a change, flag it
- **SurrealKV lock contention:** `pkill -f aetherd` before running CLI commands if daemon is running

### Do NOT suggest

- Adding new crates without explicit instruction
- Replacing SurrealDB with another graph database
- Replacing tree-sitter with another parser
- Using `unwrap()` or `expect()` in library crate code
- Global workspace builds or tests (`--workspace` flag on clippy/test)
- Model or provider recommendations (developer handles model selection)
- xAI, Grok, or x.ai integrations

## Build environment (WSL2)

**MUST be set for ALL cargo commands:**

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

**NEVER** use `/tmp/` for build artifacts — RAM-backed tmpfs in WSL2, causes OOM.
**NEVER** use `/mnt/` paths for build targets — Windows 9P filesystem, too slow.

System dependencies: `protobuf-compiler`, `liblzma-dev`, `poppler-utils`, `mold`, `sccache`

## Validation gates

Run these before every commit:

```bash
cargo fmt --all --check
cargo clippy -p <crate> -- -D warnings
cargo test -p <crate>
```

All three must pass. No exceptions. Run per-crate, never `--workspace`.

## Architecture decisions (locked — do not suggest alternatives)

- **Embeddings:** Gemini Embedding 2 (`gemini-embedding-2-preview`, 3072-dim) — Decision #83
- **Semantic rescue threshold:** 0.90 — Decision #84
- **Triage inference:** `gemini-3.1-flash-lite` (cloud), Claude Sonnet (premium deep pass)
- **Pipeline:** three-pass named scan → triage → deep — Decision #46
- **Databases:** SQLite denormalized hot path + SurrealDB 3.0/SurrealKV for graph queries — Decision #38
- **Vector store:** SQLite (active), LanceDB (intended backend, deferred)
- **MCP transport:** HTTP/SSE — Decision #40
- **Dashboard:** HTMX + D3.js + Tailwind CSS, axum server — Decision #41
- **Build tools:** mold linker, sccache, cargo-nextest
- **Desktop framework:** Tauri 2.x — Decision #90
- **Desktop frontend:** HTMX + D3 stays, no React/Vue migration — Decision #91
- **Desktop binary model:** Single binary embeds daemon, no subprocess — Decision #92
- **System tray:** Native OS tray as primary status surface — Decision #93
- **Installers:** MSI/DMG/AppImage/DEB — Decision #94
- **Auto-updater:** Tauri built-in updater plugin — Decision #95

## Current state

### Completed phases

| Phase | Name | What it delivered |
|-------|------|-------------------|
| 1-7 | Observer → Pathfinder | Core indexing, SIR generation, search, graph, dashboard, SurrealDB migration, verticals |
| 8 | The Crucible | Three-pass quality pipeline, health scoring, community detection, embeddings, edge extraction, God File refactors, trait split planner, refactor prep, MCP refactoring tools |
| 10.1 | Batch Index | Gemini Batch API pipeline, prompt hashing (BLAKE3), fingerprint history, git-aware watcher (~3960 LOC) |
| 10.2 | Continuous Intelligence | Drift monitor, noisy-OR staleness scoring, semantic gate on propagation, auto re-queue (~1000 LOC) |
| 10.3 | Agent Hooks | `sir context` (token-budgeted assembly), `sir inject` (external SIR), `sir diff` (semantic comparison) (~2975 LOC) |
| R.1 | Context Export | `aether context` CLI — shared ExportDocument engine, clipboard-ready context assembly (~2050 LOC) |
| 10.6 | Task Context Engine | Task-to-symbol ranking via RRF + Personalized PageRank, branch diff mode (~1450 LOC) |
| R.2+R.3+R.4 | File Slicing + Presets + Formats | Symbol-range file slicing, preset library (`.aether/presets/`), XML/compact output formats (~2234 LOC) |

### Health score: 52/100 (Watch)

Dragged down by `aetherd` god files. The following are refactor candidates but are **not** part of Phase 9 scope:

| File | Lines | Notes |
|------|-------|-------|
| `aetherd/src/sir_context.rs` | 3,969 | R.1 shared engine + legacy sir-context + 10.6 branch mode |
| `aether-mcp/tests/mcp_tools.rs` | 2,935 | Test file — lower priority |
| `aetherd/src/indexer.rs` | 2,731 | Watcher + git triggers from 10.1 |
| `aetherd/src/health_score.rs` | 2,389 | Health scoring orchestration |
| `aetherd/src/cli.rs` | 2,253 | 31 commands of arg structs |
| `aether-analysis/src/drift.rs` | 2,094 | Drift analysis |
| `aether-lsp/src/lib.rs` | 2,077 | LSP server (single file) |
| `aetherd/src/sir_pipeline/mod.rs` | 2,063 | SIR generation pipeline |

### Known issues

- SurrealKV lock contention: requires `pkill -f aetherd` before CLI commands
- Boundary Leaker false positives on 11/16 crates (orphaned symbols)
- Health score `--suggest-splits` crash (sending into closed channel)
- Post-refactor health score git blame distortion on newly split files

### Pipeline state

- ~3750+ symbols, 100% SIR coverage, all embedded at 3072-dim via Gemini Embedding 2
- Triage concurrency verified at 14x speedup
- Community detection: Louvain with component-bounded semantic rescue (threshold 0.90)

## SharedState (singleton the app shares)

This is what Phase 9's Tauri shell will wrap directly:

- `Arc<SqliteStore>` — all SQLite operations
- `Arc<dyn GraphStore>` — SurrealDB graph
- `Arc<Mutex<Option<SurrealGraphStore>>>` — coupling data
- Vector store — embeddings
- Config — `Arc<AetherConfig>`
- Schema version tracking

## Config sections (current)

```toml
[inference]       # Provider, model, concurrency
[embeddings]      # Provider, model, dimensions, vector_backend
[planner]         # Semantic rescue threshold
[sir_quality]     # Triage/deep thresholds
[health]          # Health scoring weights
[health_score]    # Structural health config
[storage]         # Graph backend, mirror settings
[coupling]        # Co-change mining config
[drift]           # Drift detection config
[search]          # Search config
[analysis]        # Analysis config
[verification]    # Intent verification
[dashboard]       # Port, features
[batch]           # Batch API models, thinking levels, chunk size
[watcher]         # Git triggers, realtime model, debounce
[continuous]      # Staleness scoring, schedule, requeue config
```

This is the full surface the Configuration UI (Stage 9.2) must cover.

## CLI commands (31 total)

```
# Daemon modes
aetherd                          # Daemon: indexer + watcher + dashboard + MCP + LSP
aetherd --index-once [--full]    # One-shot indexing (--full for quality passes)
aetherd --lsp                    # LSP server mode

# Intelligence queries
ask, blast-radius, communities, coupling-report, drift-report, drift-ack,
health, health-score, test-intents, trace-cause, status

# Context export (Phase Repo + 10.3 + 10.6)
context                          # File/symbol/overview/branch context with budget + presets
sir-context                      # Legacy symbol context (compatibility alias)
sir-inject                       # Direct SIR update without inference
sir-diff                         # Structural drift detection
task-history                     # Recent task context resolutions
task-relevance                   # Task-to-symbol ranking without assembly

# Presets
preset list/show/create/delete   # Manage .aether/presets/*.toml

# Batch + continuous (10.1 + 10.2)
batch extract/build/ingest/run   # Gemini Batch API pipeline
continuous run-once/status       # Drift monitor + staleness scoring

# Maintenance
regenerate, fsck, setup-local, init-agent, remember, recall, notes,
mine-coupling, refactor-prep, verify-intent
```

## Git workflow

```bash
# Create worktree for a stage
git worktree add -b feature/phase9-stage9-1-tauri-shell /home/rephu/aether-phase9-tauri-shell

# Work, validate, commit
cargo fmt --all --check
cargo clippy -p <crate> -- -D warnings
cargo test -p <crate>
git add -A
git commit -m "feat(phase9): <descriptive message for semantic indexing>"

# Push and PR (descriptive title + body — PRs are indexed by AETHER)
git push origin feature/phase9-stage9-1-tauri-shell
gh pr create \
  --title "Phase 9.1 — Tauri Shell + System Tray" \
  --body "Embeds aetherd in Tauri 2.x native window, renders HTMX dashboard in webview, adds system tray with live status. Decision #90."

# After merge
git switch main
git pull --ff-only
git worktree remove /home/rephu/aether-phase9-tauri-shell
git branch -d feature/phase9-stage9-1-tauri-shell
```

**Worktree paths:** `/home/rephu/aether-phase9-*` (siblings of `projects/`, NOT inside it)
**The `-b` flag is required** — omitting it causes commits directly on main.

## Decision register

Decisions live in `docs/roadmap/DECISIONS_v4.md` and addendum files:

- #1-43: Phase 1-7 (in `DECISIONS_v4.md`)
- #44-51: Phase 8 core (in `DECISIONS_v4_phase8_addendum.md`)
- #83-89: Phase 8.12-8.17 embeddings (in addendum files)
- #89.1: Boundary Leaker fix
- #90-96: Phase 9 (Tauri=#90, HTMX=#91, binary=#92, tray=#93, installers=#94, updater=#95, tray alerts=#96)
- #97-100: Phase 10 session (batch transport, build_job max_chars, daemon scheduler deferred, build trigger deferred)
- **Next available: #101+**

## Phase 9 stages

**CRITICAL:** These spec files must be committed to the repo before starting implementation. They exist in project knowledge but not yet in `docs/roadmap/`.

Read the spec before starting each stage:

| Stage | Spec file | What it builds |
|-------|-----------|----------------|
| 9.1 | `phase_9_stage_9_1_tauri_shell.md` | Part A: Dashboard pages for batch, continuous, task context, export, presets, fingerprints. Part B: Tauri 2.x shell, embed aetherd, webview, system tray. Part C: Visual polish — consistent cards, unified D3 colors, dark mode audit, bundle CDN deps locally, responsive sidebar |
| 9.2 | `phase_9_stage_9_2_configuration_ui.md` | Settings UI replacing TOML editing for all 15+ config sections, hot-reload |
| 9.3 | `phase_9_stage_9_3_onboarding_wizard.md` | First-run wizard: workspace picker, dep detection, provider setup |
| 9.4 | `phase_9_stage_9_4_enhanced_visualizations.md` | D3 viz: drift timeline, coupling chord, memory timeline, health scorecard, staleness heatmap, blast radius upgrade |
| 9.5 | `phase_9_stage_9_5_native_installers.md` | MSI/DMG/AppImage, code signing, Tauri updater |

**Dependency chain:** 9.1 first, then 9.2/9.4/9.5 can parallel, 9.3 needs 9.2.

## New crate for Phase 9: `aether-desktop`

Phase 9 introduces `crates/aether-desktop/` — a Tauri 2.x app that embeds the full `aetherd` engine. It does NOT spawn aetherd as a subprocess; it links the crate code directly via workspace dependencies.

On launch it:
1. Initializes `SharedState` (same as `aetherd` does today)
2. Spawns the indexer/watcher on a background tokio task
3. Spawns the dashboard HTTP server on a background task (for webview rendering)
4. Spawns the MCP server on a background task (for AI agent access)
5. Spawns the LSP server on a background task (for editor integration)

Feature-gated: `--features desktop` in workspace Cargo.toml. The headless CLI binary is still built without Tauri.

## Do NOT

- Modify existing store traits without explicit instruction
- Run `cargo test --workspace` or `cargo clippy --workspace` (per-crate only)
- Add `.mcp.json` to this repo — tool integrations come through plugins
- Skip the spec — read `docs/roadmap/` for the stage, that IS the plan
- Run brainstorming or planning phases — the stage spec has already been designed
- Suggest model selections or provider changes
- Use `/tmp/` or `/mnt/` for build artifacts
- Create worktrees inside `/home/rephu/projects/aether/` — they go at `/home/rephu/aether-phase9-*`
- Refactor the god files listed above as part of Phase 9 (separate effort)
