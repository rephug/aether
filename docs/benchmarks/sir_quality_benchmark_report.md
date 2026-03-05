# AETHER SIR Quality Benchmark Report

**Date:** March 5, 2026
**Author:** Robert + Claude (session collaboration)
**Repo:** `rephug/aether` at commit `25278c3`
**Test Corpus:** `tokio-rs/mini-redis` (184 symbols, 26 file rollups = 210 SIR records)
**Stress Test:** `BurntSushi/ripgrep` (3137 symbols)

---

## Executive Summary

We benchmarked 12 inference providers for single-pass SIR generation and tested a multi-pass enrichment pipeline. Key findings:

1. **Claude Sonnet 4.6 produces the highest quality SIR** across all symbol types, but at $0.87/184 symbols.
2. **gemini-3.1-flash-lite self-improvement (triage → triage)** is the efficiency breakthrough — same model run twice with enriched context produces ★★★★ quality for $0.12 total.
3. **A 3-tier pipeline (flash-lite → flash-lite → Sonnet top-N)** catches subtle runtime behaviors no other approach finds, at ~$2.30 for a 3000+ symbol codebase.
4. **qwen3.5:4b local achieved 3137/3137 on ripgrep** with zero failures — the local-first story is solid.
5. **The enriched context is more important than the model.** Flash-lite with context outperforms raw Gemini Pro without it.

**Recommended default pipeline for Stage 8.8:**

| Pass | Provider | Symbols | Purpose |
|------|----------|---------|---------|
| Triage | gemini-3.1-flash-lite (native API, 4k RPM) | All | Full coverage, build context graph |
| Deep | flash-lite self-improvement | All | Enriched re-generation with neighbor intents |
| Premium | Claude Sonnet 4.6 (OpenRouter) | Top 10-20 by priority | Catch runtime subtleties |

---

## 1. Single-Pass Provider Comparison

### 1.1 Coverage & Reliability

All 12 providers achieved 210/184 SIR coverage (184 symbols + 26 file rollups) on mini-redis. Two providers had single-symbol failures:

| Provider | Coverage | Failed Symbol | Failure Mode |
|----------|----------|---------------|-------------|
| gemini-3.1-flash-lite | 183/184 | `Command` | JSON parse error after 3 retries |
| qwen3.5:397b-cloud (Ollama) | 183/184 | `Subscribe::into_frame` | Timed out after 90s |
| All others | 184/184 | — | — |

### 1.2 Cost per 184 Symbols (mini-redis)

| Provider | Cost | $/symbol | Source |
|----------|------|----------|--------|
| gemini-3.1-flash-lite | $0.060 | $0.0003 | Gemini API (native) |
| gemini-flash-latest | $0.128 | $0.0007 | Gemini API (native) |
| MiniMax M2.5 | $0.132 | $0.0007 | OpenRouter |
| Gemini 3 Pro | $0.472 | $0.0026 | Gemini API (native) |
| GLM-5 | $0.461 | $0.0025 | OpenRouter |
| Kimi K2.5 | $0.800 | $0.0043 | OpenRouter |
| Claude Sonnet 4.6 | $0.866 | $0.0047 | OpenRouter |
| GPT-5.3-Codex | $0.962 | $0.0052 | OpenRouter |
| Qwen 3.5 397B | $1.214 | $0.0066 | OpenRouter |
| qwen3.5:4b local | $0.000 | $0.0000 | Ollama (RTX 2070) |
| qwen2.5-coder:7b local | $0.000 | $0.0000 | Ollama (RTX 2070) |
| qwen3.5:397b-cloud | subscription | — | Ollama Cloud |

**Note:** Gemini 3.1 Pro was not available on the native Gemini API key (free tier quota = 0). It ran successfully via OpenRouter at $0.027/symbol for the deep pass test.

### 1.3 Quality Rankings

Evaluated on 4 representative symbols: `read_frame` (async I/O method), `Subscribe::apply` (complex event loop), `Db` struct (type definition), `Db::set` (stateful mutation).

| Rank | Provider | read_frame | Subscribe::apply | Db struct | Db::set | Avg |
|------|----------|-----------|-----------------|-----------|---------|-----|
| 1 | Claude Sonnet 4.6 | ★★★★★ | ★★★★★ | ★★★★★ | ★★★★★ | 5.0 |
| 2 | GPT-5.3-Codex | ★★★★★ | ★★★★★ | ★★★★ | ★★★★★ | 4.8 |
| 3 | Kimi K2.5 | ★★★★½ | ★★★★★ | ★★★★ | — | 4.5 |
| 4 | MiniMax M2.5 | ★★★★ | ★★★★ | ★★★½ | — | 3.8 |
| 5 | Gemini 3.1 Pro | ★★★★ | ★★★½ | ★★★★ | — | 3.8 |
| 6 | qwen3.5:397b (Ollama Cloud) | ★★★★ | ★★★★½ | ★★★½ | — | 4.0 |
| 7 | GLM-5 | ★★★½ | ★★★★ | ★★★★ | — | 3.8 |
| 8 | gemini-flash-latest | ★★★½ | ★★★★ | ★★★★ | — | 3.8 |
| 9 | qwen3.5:4b local | ★★★★ | ★★★½ | ★★½ | — | 3.3 |
| 10 | gemini-3.1-flash-lite | ★★★ | ★★★ | ★★★½ | — | 3.2 |
| 11 | qwen3.5:397b (OpenRouter) | ★★★½ | ★★★ | ★★★½ | — | 3.3 |
| 12 | qwen2.5-coder:7b local | ★★★ | ★★ | ★★ | — | 2.3 |

### 1.4 Quality Dimension Analysis

**Confidence Calibration:**

| Model | Typical Confidence | Assessment |
|-------|-------------------|-----------|
| Gemini models | 0.95–1.00 | Overconfident on simple symbols |
| Claude Sonnet 4.6 | 0.90–0.97 | Best calibrated overall |
| GPT-5.3-Codex | 0.94–0.98 | Slightly overconfident |
| Kimi K2.5 | 0.90–0.92 | Most conservative — arguably most honest |
| MiniMax M2.5 | 0.70–0.95 | Wide range, honest on ambiguous symbols |
| qwen3.5:4b | 0.95 | Consistent but uncalibrated |

**Strengths by Model:**

| Model | Best At |
|-------|---------|
| Claude Sonnet 4.6 | Input descriptions, never lazy, catches subtle runtime behavior |
| GPT-5.3-Codex | Side effect enumeration, allocation/ownership edge cases |
| Kimi K2.5 | Dependency descriptions, most honest confidence |
| MiniMax M2.5 | Return path enumeration, value for cost |
| Gemini flash-lite | Raw throughput (4k RPM), adequate quality |
| qwen3.5:4b local | Best offline option, good on complex methods |

**Weaknesses by Model:**

| Model | Worst At |
|-------|---------|
| Claude Sonnet 4.6 | Cost ($0.0047/symbol) |
| GPT-5.3-Codex | Cost ($0.0052/symbol), slightly overconfident |
| Kimi K2.5 | Speed (87s per deep pass call), cost |
| qwen3.5:4b | Lazy intents on simple symbols ("getter", "Define a database handle structure") |
| qwen2.5-coder:7b | Generic intents ("Handle channel subscriptions and messages for clients") |
| Gemini models | Always 1.0 confidence, raw type signatures in outputs |

---

## 2. Stress Test: Ripgrep

| Metric | Value |
|--------|-------|
| Repository | BurntSushi/ripgrep |
| Total symbols | 3137 |
| SIR coverage | 3137/3137 (100%) |
| Failures | 0 |
| Timeouts | 0 |
| Provider | qwen3.5:4b local (Ollama, RTX 2070) |
| Duration | ~2 hours (06:24 → ~08:30 UTC) |
| Avg speed | ~1.2s/symbol |

This validates that the local pipeline scales to real-world codebases with zero failures on consumer hardware.

---

## 3. Multi-Pass Enrichment Pipeline

### 3.1 Test Design

Triage pass: gemini-3.1-flash-lite on all 184 mini-redis symbols (already completed).

Deep pass: Feed the triage SIR, neighbor intents (same-file symbols), and source code to a second model as "enriched context."

Test symbol: `Subscribe::apply` — the most complex symbol in mini-redis (public async method with multiplexed I/O, dynamic subscriptions, and shutdown handling).

### 3.2 Single-Symbol Deep Pass Results

| Deep Model | Cost | Time | Intent Quality | Key Differentiator |
|-----------|------|------|---------------|-------------------|
| **Triage baseline (flash-lite raw)** | — | — | ★★½ | Generic, bare parameter names |
| flash-lite → flash-lite | $0.001 | 2s | ★★★★ | "Persistent Pub/Sub session...multiplexing" |
| flash-lite → Gemini 3.1 Pro | $0.027 | 20s | ★★★★½ | "Continuous event loop...multiplexing" + RESP frame awareness |
| flash-lite → MiniMax M2.5 | $0.002 | 24s | ★★★★ | All return paths, fully qualified deps |
| flash-lite → Claude Sonnet 4.6 | $0.023 | 17s | ★★★★★ | 6 output paths, 8 side effects, caught lagged broadcast skip |
| flash-lite → Kimi K2.5 | $0.006 | 87s | ★★★★ | Good deps + descriptions, too slow |
| flash-lite → qwen3.5:4b local | $0.000 | 7s | ★★½ | Hallucinated input, generic errors — can't leverage context |

### 3.3 Key Finding: Context Matters More Than Model

The same flash-lite model run twice — once raw, once with enriched context — jumped from ★★½ to ★★★★. The triage SIR, neighbor intents, and source code gave the model the information it needed to produce dramatically better output. This is the core insight for Stage 8.8.

**What enrichment provides:**
- Neighbor intents → model understands the file's overall responsibility
- Triage SIR as baseline → model knows what to improve rather than starting from scratch
- Source code → model can trace specific error propagation and side effects

**What enrichment cannot fix:**
- Small model limitations (qwen3.5:4b can't leverage the extra context effectively)
- Missing runtime knowledge (only Sonnet caught `RecvError::Lagged` without source code)

### 3.4 Three-Tier Pipeline Test

Tested whether adding a middle tier before Sonnet improves the final output.

**flash-lite → MiniMax → Sonnet ($0.019/symbol):**
- 5 error modes including `RecvError::Lagged` AND `RecvError::Closed`
- 8 side effects (most comprehensive)
- Confidence: 0.92
- Caught sender-drop edge case that no other path found

**flash-lite → flash-lite → Sonnet ($0.015/symbol):**
- 5 error modes including `RecvError::Lagged` but NOT `RecvError::Closed`
- 6 side effects
- Confidence: 0.85
- Missed the sender-drop edge case

**Verdict:** The MiniMax middle tier adds marginal value ($0.004/symbol more) by giving Sonnet a richer baseline to improve upon. The flash-lite → flash-lite path is good enough as the standard deep foundation.

**Note:** In both 3-tier tests, the source code variable was empty due to a shell escaping issue. Sonnet still inferred the lagged broadcast behavior from context alone. With source code properly included (as Stage 8.8 will implement), results should be even better.

---

## 4. Recommended Pipeline for Stage 8.8

### 4.1 Default Configuration

```toml
[inference]
provider = "gemini"
model = "gemini-3.1-flash-lite-preview"
api_key_env = "GEMINI_API_KEY"
concurrency = 8

[sir_quality]
deep_pass = true
deep_provider = "gemini"
deep_model = "gemini-3.1-flash-lite-preview"
deep_priority_threshold = 0.0       # self-improve everything
deep_confidence_threshold = 1.0     # self-improve everything
deep_max_symbols = 0                # 0 = no limit
deep_concurrency = 8
```

Cost for ripgrep-scale codebase (~3000 symbols): **~$2**

### 4.2 Premium Configuration (Top-N Sonnet)

```toml
[inference]
provider = "gemini"
model = "gemini-3.1-flash-lite-preview"
api_key_env = "GEMINI_API_KEY"
concurrency = 8

[sir_quality]
deep_pass = true
deep_provider = "openai_compat"
deep_model = "anthropic/claude-sonnet-4.6"
deep_endpoint = "https://openrouter.ai/api/v1"
deep_api_key_env = "OPENROUTER_API_KEY"
deep_priority_threshold = 0.9       # only top-priority symbols
deep_confidence_threshold = 0.85    # or low-confidence from triage
deep_max_symbols = 20               # cost control
deep_concurrency = 4
```

Cost for ripgrep-scale: **~$2.40** ($2 triage + $0.40 for 20 Sonnet calls)

### 4.3 Offline Configuration

```toml
[inference]
provider = "qwen3_local"
model = "qwen3.5:4b"
endpoint = "http://127.0.0.1:11434"
concurrency = 2

[sir_quality]
deep_pass = true    # local CoT mode — thinking enabled, 8192 context
deep_max_symbols = 0  # all symbols get deep pass (free)
```

Cost: $0. Triage uses fast mode (think:false, format:json, 4096 ctx). Deep pass uses CoT mode (thinking enabled, 8192 ctx, bracket parser extracts JSON). Quality: significantly better intents, dependencies, and error modes for offline users.

### 4.4 Projected Costs at Scale

| Codebase Size | Triage Only | Triage + Self-Improve | + Sonnet Top 20 |
|---------------|------------|----------------------|----------------|
| 200 symbols (mini-redis) | $0.06 | $0.12 | $0.58 |
| 3,000 symbols (ripgrep) | $1.00 | $2.00 | $2.40 |
| 10,000 symbols (ruff) | $3.30 | $6.60 | $7.00 |
| 45,000 symbols (bevy) | $15.00 | $30.00 | $30.40 |

vs. Claude Sonnet on everything:

| Codebase Size | Sonnet All | Savings with Pipeline |
|---------------|-----------|----------------------|
| 200 symbols | $0.87 | 33% (not worth pipeline overhead) |
| 3,000 symbols | $14.10 | **83%** |
| 10,000 symbols | $47.00 | **85%** |
| 45,000 symbols | $211.50 | **86%** |

---

## 5. Locked Decisions from Benchmarking

### Decision 54: gemini-3.1-flash-lite as default triage provider
At $0.0003/symbol with 4k RPM, flash-lite is 7x cheaper than gemini-flash-latest and 15x cheaper than MiniMax M2.5 while producing adequate triage SIR. The 183/184 reliability (one JSON parse failure) is acceptable for triage where the deep pass will regenerate problem symbols.

### Decision 55: Self-improvement (same model, enriched context) as default deep pass
Flash-lite run twice with enriched context produces ★★★★ quality — better than most premium models run once without context. This eliminates the need for a separate expensive deep-pass provider in the default configuration.

### Decision 56: Claude Sonnet 4.6 as premium deep-pass provider
For the top 10-20 highest-priority symbols, Sonnet catches runtime subtleties (lagged broadcasts, silent stream closure, ownership semantics) that no other model identifies. At $0.023/symbol for selective use, the cost is negligible.

### Decision 57 (REVISED): Local qwen3.5:4b benefits from enrichment via Chain-of-Thought
Original finding (think:false, format:json, 4096 ctx): model could not leverage enrichment — hallucinated inputs, generic errors.
Revised finding (thinking enabled, no format:json, 8192 ctx): genuine analytical reasoning in thinking blocks. Subscribe::apply intent became "multiplexes three distinct asynchronous sources" with 9 dependencies including broadcast receiver errors. read_frame intent identified "polling" nature and distinguished reset from clean shutdown.
Two local modes: **fast** (single-pass, think:false, format:json, 4096 ctx — proven 3137/3137 at ~1.2s/symbol) and **deep** (CoT enriched, thinking enabled, 8192 ctx — better quality, requires bracket parser for JSON extraction).

### Decision 58: qwen2.5-coder:7b is deprecated
qwen3.5:4b is strictly better on every dimension — quality, speed (same ~1.2s/symbol), and reliability. Remove qwen2.5-coder references from documentation and defaults.

---

## 6. Ollama Cloud Models

Ollama Cloud launched September 2025 with datacenter-hosted inference through the same local API (`localhost:11434`). Tested `qwen3.5:397b-cloud`.

**Findings:**
- Works with AETHER's `qwen3_local` provider with zero code changes
- `"think": false` is respected through the API (1.5s vs 7.6s with thinking)
- Quality is high (same model as NIM) but latency is ~5.5s/symbol average
- One timeout on 184 symbols (Subscribe::into_frame at 90s)
- For mini-redis: 17 minutes total vs ~4 minutes for local qwen3.5:4b

**Verdict:** Not suitable for batch SIR generation due to latency. May be useful for on-demand single-symbol deep passes where quality matters more than speed.

---

## 7. Test Artifacts

### Baselines (saved on disk at `/home/rephu/aether-bench/mini-redis/`)

| Baseline Directory | Provider | Coverage |
|-------------------|----------|----------|
| `.aether-baseline-gemini` | gemini-flash-latest (native) | 210/184 |
| `.aether-baseline-gemini-lite` | gemini-3.1-flash-lite (native) | 210/184 |
| `.aether-baseline-gemini31pro` | gemini-3.1-pro-preview (native) | 210/184 |
| `.aether-baseline-claude-sonnet46` | Claude Sonnet 4.6 (OpenRouter) | 210/184 |
| `.aether-baseline-gpt53codex` | GPT-5.3-Codex (OpenRouter) | 210/184 |
| `.aether-baseline-minimax-m25` | MiniMax M2.5 (OpenRouter) | 210/184 |
| `.aether-baseline-glm5` | GLM-5 (OpenRouter) | 210/184 |
| `.aether-baseline-kimi-k25` | Kimi K2.5 (OpenRouter) | 210/184 |
| `.aether-baseline-qwen35-397b-or` | Qwen 3.5 397B (OpenRouter) | 210/184 |
| `.aether-baseline-ollama-cloud` | qwen3.5:397b-cloud (Ollama Cloud) | 210/184 |
| `.aether-baseline-qwen35-4b` | qwen3.5:4b (Ollama local) | 210/184 |
| `.aether-baseline-qwen25coder` | qwen2.5-coder:7b (Ollama local) | 210/184 |

### Two-Pass Test Results

| File | Description |
|------|-------------|
| `/tmp/two-pass-results/flash-lite-self.json` | flash-lite → flash-lite Subscribe::apply |
| `/tmp/two-pass-results/gemini31pro.json` | flash-lite → Gemini 3.1 Pro Subscribe::apply |
| `/tmp/two-pass-results/minimax-m25.json` | flash-lite → MiniMax M2.5 Subscribe::apply |
| `/tmp/two-pass-results/claude-sonnet46.json` | flash-lite → Claude Sonnet Subscribe::apply |
| `/tmp/two-pass-results/kimi-k25.json` | flash-lite → Kimi K2.5 Subscribe::apply |
| `/tmp/two-pass-results/qwen35-4b-local.json` | flash-lite → qwen3.5:4b Subscribe::apply |

### Ripgrep Stress Test

| Item | Location |
|------|----------|
| SIR database | `/home/rephu/aether-bench/ripgrep/.aether/meta.sqlite` |
| Run log | `/tmp/ripgrep-bench.log` |
| Coverage | 3137/3137 (100%), zero failures |

---

## 8. Known Issues & Follow-ups

1. **Benchmark scripts reference qwen3.5:9b** — 7 occurrences in `run_benchmark.sh` and `benchmark_helpers.sh` need updating to `qwen3.5:4b`. Fix: `sed -i 's/qwen3\.5:9b/qwen3.5:4b/g'`.

2. **Tiered provider gets no priority scores in `--index-once --full`** — `run_full_index_once_inner()` calls `process_event()` with `None` for priority_score, routing all symbols to fallback. Fix in Stage 8.8: compute and pass priority scores.

3. **Gemini 3.1 Pro not available on native API key** — free tier quota is 0 for this model. Must use OpenRouter ($0.027/symbol) or upgrade Google AI plan.

4. **Two-pass test scripts dropped source code** — shell variable expansion truncated `$FUNC_SOURCE` in some runs. Stage 8.8 implementation will pass source code programmatically, not through shell variables.

5. **NIM free tier too slow for batch** — rate limits caused failures and timeouts. Consider NIM only for customers with paid NIM subscriptions.
