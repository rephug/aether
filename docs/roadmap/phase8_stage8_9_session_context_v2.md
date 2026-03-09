# Phase 8.9 Session Context — Structural Health Score

**Date:** March 8, 2026
**Author:** Robert + Claude (session collaboration)
**Repo:** `rephug/aether` at commit `e47579e` (both post-8.8 fixes pushed, pipeline tests complete)
**Phase:** 8 — The Crucible (Hardening & Scale)
**Prerequisites:** Stage 8.8 (SIR Quality Pipeline), Post-8.8 fixes (PR #67) merged, pipeline test campaign complete

---

## What You Need to Know

I'm building AETHER, a Rust multi-crate workspace (~70k LOC, 15 crates) that creates persistent semantic intelligence for codebases. We're in Phase 8 (The Crucible). This stage adds a structural health scoring system that works on any Rust workspace without requiring an indexed AETHER database.

**Repo:** `https://github.com/rephug/aether` at `/home/rephu/projects/aether`
**Dev environment:** WSL2 Ubuntu, mold linker, sccache, all builds from `/home/rephu/`

---

## What Just Happened

Completed since last session context:

- **Stage 8.8** — SIR quality pipeline: kind-aware prompts, context-enriched deep pass, local CoT mode, regeneration CLI (merged)
- **Post-8.8 fixes** (PR #67) — 13 pipeline bugs fixed: pass naming (scan/triage/deep), deep pass wiring, regenerate deadlock, Auto provider, threshold operators, schema migration v6
- **Post-8.8 fix: use_cot threading** (commit `89f60a1`) — `run_quality_pass` in `crates/aetherd/src/indexer.rs` was deriving `use_cot` from provider type alone, causing triage pass to invoke the deep CoT builder (~7 min/symbol instead of ~13s). Fixed by threading `use_cot: bool` from callers: triage passes `false`, deep passes `true` only for local models. **~15x speedup confirmed.**
- **Post-8.8 fix: regenerate async panic** (commit `e47579e`) — `run_regenerate_command` was declared `async` but called via `runtime.block_on()`, causing a nested runtime panic when `SirPipeline` internally called `self.runtime.block_on()`. Fixed by removing `async` from the function declaration and removing the `block_on` wrapper.
- **Full 14-test pipeline campaign** — all tests complete. Three-pass pipeline (scan → triage → deep) validated end-to-end. Quality progression confirmed via `sir_history`. Scale validated against ripgrep (3137/3137 Gemini scan).
- **SIR benchmark campaign** — Claude Sonnet 4.6 best quality, qwen3.5:4b best local, flash-lite best speed/value

---

## Current Pipeline Configuration (Locked)

**Local three-pass profile** (mini-redis `config.toml`):
- `triage_confidence_threshold = 0.93`, `triage_max_symbols = 40`, `triage_concurrency = 2`

**Gemini three-pass profile**:
- `triage_confidence_threshold = 1.0`, `triage_max_symbols = 200`, `triage_concurrency = 16`
- Gemini three-pass on mini-redis: 63s, 20/20 deep improved, 184/184 SIR coverage, avg deep confidence 0.986–1.0

**Inference decisions (locked):**
- Default cloud triage: `gemini-3.1-flash-lite-preview` (note: not `gemini-3.1-flash-lite` — that returns 404)
- Default cloud deep: flash-lite self-improvement
- Premium deep pass: Claude Sonnet 4.6
- Local default: `qwen3.5:4b` with `think: true`, `num_ctx: 8192`

**Pending model work (not blocking 8.9):**
- Claude-distilled Qwen3.5 4B and 9B GGUF models downloaded from HuggingFace; need Ollama Modelfiles created on ext4 filesystem before benchmarking
- Fix benchmark scripts: `sed -i 's/qwen3\.5:9b/qwen3.5:4b/g' tests/stress/run_benchmark.sh tests/stress/benchmark_helpers.sh`
- Raise `INFERENCE_ATTEMPT_TIMEOUT_SECS` from 90s → 180s in `sir_pipeline.rs` (regenerate timed out on complex symbols)
- Fix `--inference-provider` CLI flag override (currently silently ignored, uses config.toml instead)

---

## Current Codebase Health (Ground Truth)

Verified against live repo at commit `e47579e`:

| Metric | Value | Notes |
|--------|-------|-------|
| Total workspace LOC | ~70,128 | Non-blank non-comment lines across all `src/` |
| Largest file | `aether-store/src/lib.rs` at 6,493 lines | Store trait alone has 52 methods |
| Largest non-store file | `aether-mcp/src/lib.rs` at 4,408 lines | |
| Cozo/CozoDB references | 168 occurrences across 14 files | Includes legacy-gated files |
| Legacy feature flags | ~20 occurrences of `feature = "legacy-cozo"` | Concentrated in aether-store |
| Max internal deps | aetherd at 13, aether-mcp at 9 | |
| Crate count | 15 | |

Known structural issues (these should produce non-zero scores):
- `aether-store` — God File: 6,493-line lib.rs, 52-method Store trait
- `aether-mcp` — Brittle Hub + Legacy Residue: 9 internal deps, cozo refs in non-test code
- `aetherd` — Brittle Hub: 13 internal deps
- `aether-config` — God File: 2,611-line lib.rs

---

## What This Stage Adds

A new `aether-health` crate and `health-score` CLI subcommand that computes a normalized 0–100 health score per crate using structural metrics only (LOC, trait size, dependency count, stale references, feature flags, TODO density). No SIR, no graph data, no indexed workspace required.

Key design decisions for this stage:

1. **New crate (`aether-health`)** — isolated from `aetherd` for future reuse by dashboard, MCP, and LSP
2. **Normalized 0–100 score** — not unbounded penalty points. Severity bands: Healthy (0–24), Watch (25–49), Moderate (50–69), High (70–84), Critical (85–100)
3. **Four archetypes** — God File, Brittle Hub, Churn Magnet, Legacy Residue. Assigned based on which signal bucket contributes the most penalty. Only labeled when score ≥ 25.
4. **Explainability** — every violation includes a human-readable reason sentence, not just a number
5. **No cargo invocation** — crate discovery via Cargo.toml parsing, file analysis via walkdir + line scanning. Target: < 1 second on AETHER.
6. **Score history** — SQLite table in `.aether/meta.sqlite` with git commit tracking and delta display

---

## Architecture

### New crate: aether-health

```
crates/aether-health/
  Cargo.toml          — deps: aether-config, serde, serde_json, rusqlite, toml, walkdir
  src/
    lib.rs            — public API: compute_workspace_score(), compute_crate_score()
    scanner.rs        — workspace discovery from Cargo.toml, per-crate file walker
    metrics.rs        — LOC, trait methods, stale refs, feature flags, TODO density, deps
    scoring.rs        — penalty formula, weight application, normalization to 0-100
    archetypes.rs     — archetype assignment from signal distribution
    explanations.rs   — reason templates (constants, no LLM)
    history.rs        — SQLite read/write for health_score_history table
    output.rs         — table formatter and JSON serializer
```

### Integration points

- `aether-config/src/lib.rs` — new `HealthScoreConfig` struct (additive, no breaking changes)
- `aetherd/src/cli.rs` — new `HealthScore(HealthScoreArgs)` variant in `Commands` enum
- `aetherd/src/main.rs` — dispatch to health-score handler
- Root `Cargo.toml` — add `"crates/aether-health"` to workspace members

### Important: existing Health subcommand

There is already a `Health(HealthArgs)` subcommand in `aetherd` that runs graph-based health analysis via `aether-analysis::HealthAnalyzer`. This stage adds `HealthScore` as a **separate** subcommand — do not modify or replace the existing `Health` command.

- `aether health` — graph-based analysis requiring indexed workspace
- `aether health-score` — structural analysis requiring no indexing

---

## Build Environment

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

Do NOT use `/tmp/` for build artifacts (RAM-backed tmpfs in WSL2).

---

## Files to Read First

- `docs/roadmap/phase_8_stage_8_9_structural_health.md` — full spec (source of truth)
- `crates/aetherd/src/cli.rs` — Commands enum, subcommand arg patterns
- `crates/aetherd/src/main.rs` — subcommand dispatch pattern
- `crates/aether-config/src/lib.rs` — config struct patterns (look at HealthConfig for reference)
- `crates/aether-store/src/lib.rs` — Store trait (the 52-method trait this stage will measure)
- `Cargo.toml` — workspace members list

---

## Crate Test Order (OOM-safe for WSL2)

```bash
cargo fmt --all --check
cargo clippy -p aether-health -p aetherd -p aether-config -- -D warnings
cargo test -p aether-health
cargo test -p aether-config
cargo test -p aetherd
```

Full workspace test (only if the above pass and memory allows):

```bash
cargo test -p aether-core
cargo test -p aether-store
cargo test -p aether-parse
cargo test -p aether-sir
cargo test -p aether-infer
cargo test -p aether-memory
cargo test -p aether-lsp
cargo test -p aether-analysis
cargo test -p aether-mcp
cargo test -p aether-query
```

---

## Post-Merge Sequence

```bash
cd /home/rephu/projects/aether
git switch main
git pull --ff-only origin main
git log --oneline -3

git worktree remove ../aether-phase8-health-score
git branch -D feature/phase8-stage8-9-health-score
git worktree prune
```

---

## Immediate Validation Target

Run `aetherd health-score --workspace . --output json` against AETHER itself after the stage merges. Expected workspace score: **50–70 (Moderate)** based on known structural issues. Commit the baseline JSON to `docs/hardening/health_score_baseline.json` for future delta tracking.

---

## What Comes Next

**Stage 8.10 — Semantic Health Extension:** Adds `--semantic` flag to `health-score` that loads SIR, graph, drift, and community data from an indexed workspace. New signals: drift density, stale SIR ratio, blast radius concentration, test protection gaps, cross-community edge ratio. New archetypes: Boundary Leaker, Zombie File, False Stable. Consumes existing `HealthAnalyzer` output as a signal source.

**Stage 8.11 — Health Surfaces:** Dashboard hotspot leaderboard, MCP tools (`aether_health_hotspots`, `aether_health_explain`), LSP health score in hover card, split planner with file-level recommendations.

**Pipeline follow-on items (not blocking 8.9):**
- Load Claude-distilled Qwen3.5 4B/9B models into Ollama and benchmark vs qwen3.5:4b
- Raise regenerate timeout from 90s → 180s
- Fix `--inference-provider` CLI flag override
- Fix benchmark script model names
