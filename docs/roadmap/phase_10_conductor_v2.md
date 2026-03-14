# Phase 10 — The Conductor

## Thesis

Phase 8 built a three-pass SIR quality pipeline that produces excellent semantic intelligence — but it runs synchronously, one symbol at a time, and the watcher treats every file save the same way regardless of what changed. Phase 10 makes the intelligence layer **autonomous**: batch indexing for cold starts at half the cost, a smarter watcher that responds to git operations and uses premium models on actively edited code, continuous drift monitoring that catches semantic rot before it becomes a bug, and CLI commands that let AI agents inject and query SIR context programmatically.

**One-sentence summary:** "AETHER stops waiting to be asked and starts watching, learning, and correcting on its own."

---

## Why This Phase Exists

Three problems drive Phase 10:

1. **Cold-start indexing is expensive and slow.** A 10K-symbol codebase takes hours to index via real-time inference. Gemini's Batch API processes the same workload asynchronously at 50% cost with higher rate limits. Prompt hashing ensures only symbols whose context actually changed are re-submitted — reducing nightly batch costs by 90%+ on quiet days.

2. **The watcher is dumb.** It fires on every file save with the same cheap model. But the symbol you just edited is the one most likely to be queried by your AI tools in the next 30 seconds — it deserves the best model available. And git operations (branch switch, pull, merge, rebase) create wholesale symbol changes that the current watcher misses entirely.

3. **SIRs go stale silently.** Code evolves, dependencies shift, but SIRs generated weeks ago still claim the old semantics. Nobody notices until an AI agent gives wrong context. Continuous drift monitoring with semantic-aware propagation and automatic re-queuing closes this gap.

---

## Key Technical Innovations (from Deep Think review)

- **Noisy-OR staleness scoring** with hard gates for source changes and logistic time decay — no false dampening of critical signals
- **Semantic gate on drift propagation** — neighbor SIR changes only propagate staleness proportional to actual meaning shift (cosine distance between old/new embeddings), eliminating false positives from minor rewording
- **Prompt hashing via BLAKE3** — deterministic fingerprint of each symbol's inference context (source + neighbor SIRs + config) allows batch pipeline to skip unchanged symbols entirely
- **Greedy knapsack context assembly** — token-budgeted context blocks for AI agents, prioritized by signal-to-noise ratio
- **Change fingerprint history** — prompt hash + semantic gate (Δ_sem) logged per symbol per regeneration, creating a timeline of *when meaning shifted* and *why* (source edit, neighbor drift, or model upgrade). Instrumented in Phase 10 for consumption in Phases 11–12.

---

## Change Fingerprint System

Prompt hashing (BLAKE3 over source + neighbor SIRs + config) and the semantic gate (cosine distance between old/new embeddings) combine into a **change fingerprint** — a per-symbol record of what changed and how much meaning shifted at each regeneration event.

Each fingerprint entry records:
- `symbol_id` — which symbol
- `timestamp` — when regenerated
- `prompt_hash` — the new context fingerprint
- `prompt_hash_previous` — the prior fingerprint
- `change_source` — which component of the hash changed: `source`, `neighbor`, `config`, or a combination
- `delta_sem` — cosine distance between old and new SIR embeddings (0.0 = identical meaning, 1.0 = completely different)

This history is stored in a new SQLite table `sir_fingerprint_history` and logged during every SIR regeneration (batch, watcher, or inject). The storage cost is ~100 bytes per entry — negligible even at 50K symbols × daily regeneration.

### What this enables (Phases 11–12)

- **Semantic blast radius:** After a PR merges, identify which symbols *actually shifted meaning*, not just which files were touched. Symbols with high Δ_sem are where downstream consumers may be operating on stale assumptions.
- **Architectural drift detection:** Symbols that keep shifting meaning due to neighbor changes (despite stable own code) are sitting in volatile zones — maintenance risks invisible to static analysis.
- **Real refactor vs cosmetic change:** A 200-file rename shows Δ_sem ≈ 0.01 everywhere. A one-line error handling change shows Δ_sem = 0.3. Small diff, meaningful semantic shift.
- **Model upgrade measurement:** Switching models invalidates all prompt hashes. The resulting Δ_sem distribution per symbol quantifies "was the upgrade worth it?"
- **Agent accountability:** When an agent injects a SIR, the fingerprint records the divergence from the model's version. If the next regeneration disagrees, there's a paper trail.
- **Community semantic stability:** Aggregate Δ_sem across a Louvain community over time to measure module-level conceptual coherence.

Phase 10 instruments the data. Phases 11–12 build the features that consume it.

---

## Stages

| Stage | Name | Description | Codex Runs | Dependencies |
|-------|------|-------------|------------|--------------|
| 10.1 | Batch Index Pipeline + Watcher Intelligence | Gemini Batch API for cold-start, prompt hashing, per-pass model config, smarter watcher with git triggers | 2–3 | Phase 8 complete |
| 10.2 | Continuous Intelligence | Background drift monitoring with Noisy-OR scoring, semantic-gated propagation, automatic re-queue, nightly schedules | 2–3 | 10.1 |
| 10.3 | Agent Integration Hooks | `aetherd sir context` and `aetherd sir inject` CLI commands with token-budgeted context assembly | 1–2 | 10.1 |

### Dependency Graph

```
10.1 (Batch + Watcher) ──────────────────────┐
    │                                         │
    ├── 10.2 (Continuous Intelligence)        │
    │                                         │
    └── 10.3 (Agent Integration Hooks) ───────┘
```

**Parallelism:** After 10.1 merges, 10.2 and 10.3 can proceed in parallel. 10.3 is small enough to be a single Codex run.

**Estimated total: 5–8 Codex runs.**

---

## New Config Sections

Phase 10 adds three new top-level config sections to `AetherConfig`:

### `[batch]`

Per-pass model selection, thinking levels, neighbor context depth, JSONL output directory, auto-chaining behavior, JSONL chunk size. All fields optional with sane defaults.

### `[watcher]`

Real-time model override for file-save events, git operation triggers, debounce settings.

### `[continuous]`

Drift monitor schedule, staleness parameters (sigmoid half-life, decay factor), re-queue limits, auto-submit toggle.

All sections are `Option<T>` with `#[serde(default)]` — existing configs work without changes.

---

## Target Infrastructure

- **Local dev (RTX 2070):** Real-time watcher with Sonnet via OpenRouter for actively edited symbols. Batch not needed — symbol count is manageable.
- **Netcup servers (64GB DDR5, no GPU):** Batch pipeline for nightly re-generation. Cloud-only inference (Gemini flash-lite scan, flash triage, Sonnet deep). Prompt hashing keeps nightly costs minimal on quiet days.
- **CI:** Batch extract + build can run in CI to pre-compute JSONL files for new repos.

---

## What Phase 10 Does NOT Do

- **No UI changes.** Dashboard visibility into batch job status and staleness scores is deferred to Phase 9 (if 9 follows 10) or a separate dashboard stage.
- **No new storage backends.** Uses existing SQLite, SurrealDB, LanceDB stores.
- **No new inference providers.** Reuses existing Gemini, OpenAI-compat, and Ollama providers.
- **No Vertex AI integration.** Uses the Gemini API Batch Mode (direct), not Vertex AI batch prediction. Vertex requires GCS buckets and Google Cloud project setup. The direct API accepts inline JSONL.

---

## Future Enhancement: Risk Integral Trigger

The nightly cron schedule in 10.2 is a simplification. A more adaptive approach: maintain a running sum of workspace risk `M_stale = Σ Priority_i`. When it exceeds a threshold, flush and spawn a batch job. This naturally adapts to development velocity — triggering multiple times during heavy refactors, pausing on quiet days. Deferred to a future enhancement for multi-developer team-tier workspaces.
