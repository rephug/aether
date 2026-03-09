# Phase 8.10 Session Context — Git + Semantic Health Signals

**Date:** March 9, 2026
**Author:** Robert + Claude (session collaboration)
**Repo:** `rephug/aether` at commit `089968c` (PR #68 merged)
**Phase:** 8 — The Crucible (Hardening & Scale)
**Prerequisites:** Stage 8.9 (Structural Health Score) merged ✅

---

## What You Need to Know

AETHER is a Rust multi-crate workspace (~65K LOC, 16 crates) that creates persistent semantic intelligence for codebases. We're in Phase 8 (The Crucible). This stage extends the 8.9 structural health score with git churn signals and semantic signals from the live index, activated via `--semantic`.

**Repo:** `https://github.com/rephug/aether` at `/home/rephu/projects/aether`
**Dev environment:** WSL2 Ubuntu, mold linker, sccache, RTX 2070 8GB

---

## What Just Happened

- **Stage 8.9** (PR #68, commit `089968c`) — New `aether-health` crate. `aether health-score` subcommand with normalized 0-100 scoring, four archetypes (God File, Brittle Hub, Churn Magnet, Legacy Residue), per-crate violations, score history in `.aether/meta.sqlite`
- **Baseline workspace score:** 44/100 (Watch)
- **Worst crate:** `aether-store` at 100/100 (Critical) — God File + Legacy Residue
- **MCP stdio validated:** `aether-mcp` binary connects via Claude Code, 20 tools working
- **aether-query HTTP validated:** `/health`, `/info`, `/mcp` all responding on port 9721, read-only mode confirmed

---

## Current Health Score Baseline (Ground Truth)

| Crate | Score | Severity | Archetypes |
|-------|-------|----------|------------|
| aether-store | 100 | Critical | God File, Legacy Residue |
| aether-config | 64 | Moderate | Legacy Residue |
| aether-analysis | 63 | Moderate | Legacy Residue |
| aetherd | 50 | Moderate | God File |
| aether-lsp | 45 | Watch | Legacy Residue, God File |
| aether-mcp | 44 | Watch | God File |
| aether-memory | 33 | Watch | Legacy Residue |
| aether-infer | 27 | Watch | God File |
| aether-health | 22 | Healthy | — |
| aether-dashboard | 10 | Healthy | — |
| aether-parse | 10 | Healthy | — |
| aether-core | 0 | Healthy | — |
| aether-document | 0 | Healthy | — |
| aether-graph-algo | 0 | Healthy | — |
| aether-query | 0 | Healthy | — |
| aether-sir | 0 | Healthy | — |

**Key finding from 8.9:** `aether-analysis` (50 stale backend refs) and `aether-memory/unified_query.rs` still directly instantiate `CozoGraphStore::open()` — incomplete SurrealDB migration from Stage 7.2. This is real data, not a false positive.

---

## Known Issues (Not Blocking 8.10)

- `aether-mcp` and `aether-query` fight over SurrealKV LOCK — only one can open a workspace at a time
- `aether-query` binds to port 9721 by default (dashboard config uses 9720)
- Incomplete SurrealDB migration: `aether-analysis/src/causal.rs`, `aether-analysis/src/drift.rs`, `aether-memory/src/unified_query.rs` still use `CozoGraphStore::open()` directly

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

- Rust toolchain: stable + mold linker + sccache
- **Never use `/tmp/` for build artifacts — it's RAM-backed tmpfs in WSL2**
- **Never run `cargo test --workspace` — OOM risk. Always per-crate.**

---

## What 8.10 Adds

Extends `aether health-score` with `--semantic` flag. Without it, behavior is identical to 8.9. With it:

**Git signals** (uses existing `GitContext` from `aether-core`):
- `commits_30d`, `commits_90d` — churn rate
- `author_count` — bus factor risk
- `blame_age_std_dev` — staleness variance

**Semantic signals** (consumes existing `HealthAnalyzer`, `CouplingAnalyzer`, `Store`):
- Centrality (pagerank, betweenness)
- Drift magnitude
- Test protection gaps
- Boundary leakage

**Three new archetypes:**
- Boundary Leaker — symbols span multiple communities
- Zombie File — central but low churn (stale but load-bearing)
- False Stable — low structural score but high semantic drift

**Scoring weights with `--semantic`:** 0.40 structural + 0.25 git + 0.35 semantic

**Graceful degradation:** missing git → skip git signals; missing index → skip semantic signals

---

## Stage 8.10 Codex Prompt

The full Codex prompt is embedded in `docs/roadmap/phase_8_stage_8_10_semantic_health.md` (already committed to main). Use that directly — do not reconstruct it here.

Verify it's in the repo before branching:
```bash
ls docs/roadmap/phase_8_stage_8_10_semantic_health.md
```

---

## End-of-Stage Git Sequence

```bash
git push origin feature/phase8-stage8-10-semantic-health
gh pr create \
  --title "Phase 8.10 — Git + Semantic Health Signals" \
  --body "Extends health-score with git churn/author signals and semantic signals (centrality, drift, test gaps, boundary leakage). Adds three new archetypes: Boundary Leaker, Zombie File, False Stable. Activated via --semantic flag with graceful degradation."

# After merge:
git switch main
git pull --ff-only
git worktree remove ../aether-phase8-semantic-health
git branch -D feature/phase8-stage8-10-semantic-health
git worktree prune
```

---

## Test Workspace

```
/home/rephu/aether-bench/mini-redis
```

184/184 SIR coverage, full three-pass pipeline completed. Use this for the `--semantic` validation run.
