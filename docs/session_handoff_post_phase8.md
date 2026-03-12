# AETHER Session Handoff — Post Phase 8 / Pre-Refactoring

**Date:** March 12, 2026
**Last commit:** `ccc8ec3` — aether-store refactor merged to main

---

## What was accomplished this session

### Phases shipped (all merged to main)

| Stage | Commit | What it did |
|-------|--------|-------------|
| 8.14 | c5e5d1f | Component-bounded semantic rescue — eliminated cross-component bridging |
| 8.15 | db2a3e6 | TYPE_REF + IMPLEMENTS edge extraction — 1026 type_ref, 12 implements edges |
| 8.16 | (merged) | `--embeddings-only` flag + OpenAI-compat embedding provider |
| 8.17 | 5938c05 | Gemini native embedding provider with asymmetric task types |
| Refactor | ccc8ec3 | aether-store/src/lib.rs split from 6817 lines to 13 modules + 751-line façade |

### Decisions locked (#83-89)

- **#83:** Gemini Embedding 2 (`gemini-embedding-2-preview`) is the production embedding model
- **#84:** Semantic rescue threshold → 0.90 (was 0.85, tuned for Gemini's hotter similarity scores)
- **#85:** `--embeddings-only` flag for rapid model testing
- **#86:** OpenAI-compatible embedding provider
- **#87-89:** Gemini native provider with `x-goog-api-key` auth, `EmbeddingPurpose` enum, default Document purpose

### SIR pipeline state

- 3748 symbols, 100% SIR coverage
- 3623 triage-enriched (flash-lite scan + flash-lite triage with enriched context)
- 46 deep (Claude Sonnet 4.6 via OpenRouter)
- All embedded with Gemini Embedding 2 via native provider (3072-dim)

### Ablation baseline (aether-store, post full-triage + refactor)

```
Row 6 (full pipeline):
  communities=11, largest=131, smallest=2, loners=3
  confidence=0.93, stability=0.82
  top modules: str_ops, threshold_ops, graph_ops
```

### Production config

```toml
[inference]
provider = "gemini"
model = "gemini-3.1-flash-lite-preview"
api_key_env = "GEMINI_API_KEY"
concurrency = 12

[embeddings]
enabled = true
provider = "gemini_native"
model = "gemini-embedding-2-preview"
api_key_env = "GEMINI_API_KEY"
dimensions = 3072
vector_backend = "sqlite"

[planner]
semantic_rescue_threshold = 0.90

[sir_quality]
triage_pass = true
triage_provider = "gemini"
triage_model = "gemini-3.1-flash-lite-preview"
triage_api_key_env = "GEMINI_API_KEY"
triage_priority_threshold = 0.0
triage_confidence_threshold = 0.0
triage_max_symbols = 1

deep_pass = false
```

**Note:** triage is currently configured to process only 1 symbol (from the
last deep-only run). Reset `triage_max_symbols = 0` and thresholds to
`0.0`/`1.0` if running full triage again.

---

## What to do next: Refactor remaining God Files

### Health score before (current state)

```
Overall: 43/100 (Watch)

Top issues:
  [FAIL] aether-mcp/src/lib.rs — 4799 lines
  [FAIL] aether-health/src/planner_communities.rs — 3390 lines
  [FAIL] aether-config/src/lib.rs — 3173 lines
  [FAIL] aether-infer/src/lib.rs — 2863 lines
  [FAIL] aetherd/src/sir_pipeline.rs — 2277 lines
```

### Recommended execution order

| Order | File | Lines | Rationale |
|-------|------|-------|-----------|
| 1 | aether-config/src/lib.rs | 3173 | Config structs — cleanest domain boundaries |
| 2 | aether-infer/src/lib.rs | 2863 | Provider dispatch — embedding/ already partially split |
| 3 | aetherd/src/sir_pipeline.rs | 2277 | Pipeline passes — clear sequential structure |
| 4 | aether-health/src/planner_communities.rs | 3390 | Algorithm-heavy — preserve [diag] prints |
| 5 | aether-mcp/src/lib.rs | 4799 | Largest, most handlers, highest risk |

### How to run each refactor

For each file, the prompt is in the project knowledge base (uploaded this
session as `refactor_aether_*.md`). The workflow:

1. **Paste the prompt into Codex in plan mode** — it will query AETHER's
   SQLite for TYPE_REF edges and symbol inventory, read the file, and
   propose a split plan
2. **Review the plan** — check module boundaries against TYPE_REF data
3. **Tell Codex to implement** — it works on main directly (no worktree)
4. **After implementation, verify:**
   ```bash
   cargo fmt --check
   cargo clippy --workspace -- -D warnings
   cargo test --workspace
   ```
5. **Commit and push:**
   ```bash
   git add -A
   git commit -m "Refactor {crate}: split {N}-line God File into M modules"
   git push origin main
   ```
6. **Re-index and check health:**
   ```bash
   pkill -f aetherd
   rm -f /home/rephu/projects/aether/.aether/graph/LOCK
   cargo build -p aetherd --release
   $CARGO_TARGET_DIR/release/aetherd --workspace /home/rephu/projects/aether \
     --index-once --full 2>&1
   $CARGO_TARGET_DIR/release/aetherd health-score \
     --workspace /home/rephu/projects/aether --semantic --output table
   ```

### Build environment (MUST be set for ALL cargo commands)

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

### After all five refactors

Run the final health score and compare against the "before" snapshot:

```
Before: 42/100, 5 God File FAILs
Target: 60+/100, 0 God File FAILs
```

Save the before/after health reports for the showcase:
```bash
$CARGO_TARGET_DIR/release/aetherd health-score \
  --workspace /home/rephu/projects/aether \
  --semantic --output json > /home/rephu/aether-target/health_after_all_refactors.json
```

---

## Known issues / context for the new session

- **SurrealKV lock contention:** `--suggest-splits` in health-score crashes
  with "sending into a closed channel." Use `pkill -f aetherd` and
  `rm -f .aether/graph/LOCK` before CLI commands. The split data comes from
  ablation, not from `--suggest-splits`.

- **Triage concurrency bug:** `triage_concurrency = 12` does not actually
  parallelize — triage processes symbols sequentially at ~4s/symbol. Logged
  for a future fix, not blocking refactoring.

- **Boundary Leaker false positives:** 11/16 crates flagged. This is a known
  issue with the global community snapshot (not the file-scoped planner).
  Deferred to a future stage.

- **Codex works on main directly** — it did not create a worktree for the
  aether-store refactor. Check `git status --porcelain` before committing
  to make sure changes are staged.

- **Binary must be rebuilt after code changes:**
  `cargo build -p aetherd --release` before running `aetherd` commands.

---

## Key project knowledge files

- `aether_showcase_data.md` — full marketing showcase with before/after data
- `DECISIONS_v4_phase8_15_16_addendum.md` — decisions #83-89
- `refactor_aether_mcp.md` — refactoring prompt for aether-mcp
- `refactor_planner_communities.md` — refactoring prompt for planner_communities
- `refactor_aether_config.md` — refactoring prompt for aether-config
- `refactor_aether_infer.md` — refactoring prompt for aether-infer
- `refactor_sir_pipeline.md` — refactoring prompt for sir_pipeline
- `refactor_prompt_with_aether.md` — the AETHER-informed prompt used for aether-store (reference)
- `refactor_prompt_without_aether.md` — the blind prompt used for comparison

---

## Upcoming after refactoring

- **LanceDB bug fixes** (ARCH-2 N+1 queries, ARCH-4 slow migration) — one Codex run
- **Triage concurrency fix** — `run_triage_pass` processes sequentially despite concurrency setting
- **Opus-distilled qwen3.5 test** — reasoning model for SIR generation, not embeddings
  (`huggingface.co/Jackrong/Qwen3.5-4B-Claude-4.6-Opus-Reasoning-Distilled-GGUF`)
- **Phase 5.4** — Reranker integration (after embedding model locked)
- **Phase 9** — Tauri desktop app
- **Phase 10** — Batch index pipeline + continuous intelligence + Claude Code integration
- **Commercialization** — four pricing tiers, closed alpha with design partners
