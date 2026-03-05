# Phase 8.8 Session Context — SIR Quality Pipeline & Context-Enriched Regeneration

**Date:** March 5, 2026
**Author:** Robert + Claude (session collaboration)
**Repo:** `rephug/aether` at commit `25278c3`
**Phase:** 8 — The Crucible (Hardening & Scale)
**Prerequisites:** Stage 8.2 (Progressive Indexing + Tiered Providers), Stage 8.7 (Stress Test) merged

---

## What Happened Before This Stage

### Benchmark Campaign (12 providers, 3 test types)

We ran the most comprehensive SIR quality benchmark in AETHER's history:

**Single-pass:** 12 inference providers on tokio-rs/mini-redis (184 symbols):
- Claude Sonnet 4.6 ($0.87) — best quality, catches runtime subtleties like RecvError::Lagged
- GPT-5.3-Codex ($0.96) — near-identical to Sonnet
- Kimi K2.5 ($0.80) — best confidence calibration
- MiniMax M2.5 ($0.13) — best value for cloud
- gemini-3.1-flash-lite ($0.06) — 4k RPM speed champion
- qwen3.5:4b local ($0) — 3137/3137 on ripgrep, zero failures, ~1.2s/symbol

**Multi-pass enrichment:** Tested triage → deep on Subscribe::apply across 6 providers:
- Enriched context matters more than the model
- Flash-lite self-improvement (same model twice with context) jumps from ★★½ to ★★★★
- 3-tier (flash-lite → flash-lite → Sonnet top-N) catches edge cases no other approach finds

**Local Chain-of-Thought (CoT):** qwen3.5:4b with thinking enabled + 8192 context + enriched prompt:
- Thinking blocks show genuine analytical reasoning (identifying clean shutdown vs reset, buffer states)
- Intent quality dramatically improved ("multiplexes three distinct asynchronous sources")
- Caught broadcast receiver lagged/closed errors (previously only Sonnet found this)
- Dependencies expanded to 9 items including frame builders
- Requires bracket parser to extract JSON from mixed thinking+JSON output
- Speed was acceptable — not the 1.2s of format:json but well within batch tolerance

**Stress test:** qwen3.5:4b achieved 3137/3137 (100%) on BurntSushi/ripgrep with zero failures.

Full report: `docs/benchmarks/sir_quality_benchmark_report.md`

---

## Current Architecture

### Inference Providers (crates/aether-infer/src/lib.rs)

```
InferenceProviderKind enum: Auto, Tiered, Gemini, Qwen3Local, OpenAiCompat
```

- `TieredProvider` — routes on `context.priority_score` vs threshold. Primary (cloud) for high-priority, fallback (local) for low-priority. Config: flat fields under `[inference.tiered]`
- `SirContext` — has `priority_score: Option<f64>`, `language`, `file_path`, `qualified_name`

### JSON Handling

- `normalize_candidate_json()` (line ~1512) strips markdown fences but does NOT handle:
  - Raw text before/after JSON (thinking tags, conversational filler)
  - Trailing commas in arrays/objects (common small-model error)
- `parse_and_validate_sir()` calls `normalize_candidate_json()` then `serde_json::from_str()`

### SIR Storage (SQLite — NOT SurrealDB)

- `sir` table: id, sir_hash, sir_version, **provider**, **model**, updated_at, sir_json
- `SirMetaRecord` already has `provider` and `model` fields. Only `generation_pass` is new.

### SIR Pipeline (crates/aetherd/src/sir_pipeline.rs)

- `generate_sir_with_retries()` — 3 attempts, 90s timeout, exponential backoff
- `process_event()` calls `process_event_with_priority()` with `None`
- Qwen3LocalProvider uses `run_sir_parse_validation_retries_with_feedback()` — sends parse errors back to the model

### Indexer (crates/aetherd/src/indexer.rs)

- `run_full_index_once_inner()` is the `--index-once --full` path
- Currently calls `sir_pipeline.process_event()` without priority scores
- `compute_symbol_priority_scores()` exists but only used in watcher/queue path

### Current Prompt (line 1107)

`build_strict_json_prompt()` — single prompt for all symbol types. No kind-awareness, no few-shot examples, no enriched context.

### Ollama Body (line 1123)

`build_ollama_generate_body()` sends: `think: false`, `format: "json"`, `num_ctx: 4096`, temperature from config.

---

## What This Stage Builds

### 1. Bracket Parser — extend normalize_candidate_json()

The existing function handles markdown fences. Extend it to handle:
- `<thinking>...</thinking>` blocks before JSON (CoT output from local models)
- Conversational filler text before/after the JSON object
- Trailing commas before `]` or `}` (common small-model error)

This is **critical path** for the local CoT deep pass — without it, enriched local output cannot be parsed.

### 2. Kind-Aware Prompt Templates

New file: `crates/aether-infer/src/sir_prompt.rs`

`build_sir_prompt_for_kind()` branches on symbol kind + visibility + line count:
- struct/enum/trait/type_alias: "Describe WHY this type exists. Inputs and outputs should be empty."
- public function/method >30 lines: "Enumerate each distinct return path. Describe each parameter."
- private or small function/method: "Provide descriptive intent, not just getter or constructor."
- test function: "Describe what behavior is being verified."

All variants include compact few-shot examples (~200 tokens) from our benchmark baselines.

### 3. Context-Enriched Deep Pass Prompt

`build_enriched_sir_prompt()` includes file intent, neighbor intents, baseline SIR, kind-aware guidance.

For **cloud models**: standard enriched prompt, `format: json` or `response_format`.
For **local models (CoT mode)**: enriched prompt with explicit thinking instructions:
- "Before outputting JSON, analyze inside `<thinking>` tags: what is missing from the baseline SIR?"
- `think: true` (or omit think field), `num_ctx: 8192`, NO `format: "json"` (conflicts with thinking)
- Bracket parser extracts JSON from mixed output

### 4. Two Ollama Body Builders

**Triage (fast, existing behavior):** `think: false`, `format: "json"`, `num_ctx: 4096` — proven 3137/3137 on ripgrep at ~1.2s/symbol.

**Deep (CoT, new):** `think: true` (or omit), NO `format: "json"`, `num_ctx: 8192`, temperature 0.3 — produces thinking block + JSON. Bracket parser required. Slower but dramatically better quality.

### 5. SirQualityConfig

New `[sir_quality]` config section with deep pass settings, max neighbors, concurrency. Also add `concurrency` field to `[inference]`.

### 6. generation_pass Column

Add to `SirMetaRecord` + SQLite migration. Values: `"single"`, `"triage"`, `"deep"`, `"premium"`, `"regenerated"`.

### 7. Priority Scores in Batch Mode

In `run_full_index_once_inner()`, compute and pass priority scores using existing `compute_symbol_priority_scores()`.

### 8. Two-Pass Pipeline (--deep flag)

`--index-once --full --deep`:
- Pass 2A (triage): Normal SIR generation for all symbols
- Pass 2B (deep): Selected symbols get enriched prompt + CoT (local) or enriched prompt (cloud)

If provider is `Qwen3Local`, deep pass uses CoT body builder (thinking enabled, 8192 context).
If provider is `Gemini` or `OpenAiCompat`, deep pass uses standard enriched prompt.

### 9. Pipeline-Level Fallback

On `ParseValidationExhausted` with Tiered provider, re-attempt with fallback. Track which provider succeeded.

### 10. Regeneration CLI

`aether regenerate` subcommand with `--below-confidence`, `--from-provider`, `--deep`, `--dry-run`.

---

## Locked Decisions from Benchmarking

### Decision 54: gemini-3.1-flash-lite as default cloud triage provider
$0.0003/symbol, 4k RPM, adequate quality for triage where deep pass will improve.

### Decision 55: Self-improvement (same model, enriched context) as default deep pass
Flash-lite run twice with enriched context produces ★★★★ quality — better than most premium models run once.

### Decision 56: Claude Sonnet 4.6 as premium deep-pass provider
Top 10-20 highest-priority symbols. Catches runtime subtleties. $0.023/symbol for selective use.

### Decision 57 (REVISED): Local qwen3.5:4b benefits from enrichment via Chain-of-Thought
Original finding (think:false, format:json, 4096 ctx): model could not leverage enrichment.
Revised finding (thinking enabled, no format:json, 8192 ctx): genuine analytical reasoning in thinking blocks, dramatically improved intents, caught broadcast receiver errors. Requires bracket parser for JSON extraction.
Two local modes: fast (single-pass, proven at scale) and deep (CoT enriched, better quality, slower).

### Decision 58: qwen2.5-coder:7b is deprecated
qwen3.5:4b is strictly better on every dimension.

---

## Files to Create

```
crates/aether-infer/src/sir_prompt.rs     # Kind-aware prompts, few-shot examples, enriched prompts
```

## Files to Modify

```
crates/aether-infer/src/lib.rs            # sir_prompt module, extend normalize_candidate_json, CoT body builder, SirContext fields
crates/aether-config/src/lib.rs           # SirQualityConfig, concurrency config
crates/aetherd/src/indexer.rs             # Priority scores in batch, two-pass pipeline, --deep flag
crates/aetherd/src/sir_pipeline.rs        # Pipeline fallback, generation_pass metadata, enriched context assembly
crates/aetherd/src/cli.rs                 # --deep flag, regenerate subcommand
crates/aether-store/src/lib.rs            # generation_pass column migration + SirMetaRecord field
```

## Scope Guards

- Do NOT change the SIR JSON schema (7 fields: intent, inputs, outputs, side_effects, dependencies, error_modes, confidence)
- Do NOT add new inference provider kinds — reuse existing Tiered/Gemini/OpenAiCompat/Qwen3Local
- Do NOT modify TieredProvider routing logic (it already routes on priority_score)
- Do NOT add new crates
- Do NOT modify MCP tool schemas
- Do NOT use SurrealDB for SIR storage — SIR lives in SQLite (meta.sqlite)
- Do NOT add mock providers or mock testing (removed in Phase 8.2)
