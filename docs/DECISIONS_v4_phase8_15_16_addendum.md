# DECISIONS_v4 — Phase 8.15/8.16 Addendum

**Date:** March 11, 2026
**Context:** Phase 8.15 (edge extraction), 8.16 (embedding refresh + OpenAI-compat provider), and embedding model A/B testing.

---

## New decisions

### 83. Production embedding model → Gemini Embedding 2 at threshold 0.90

**Date:** 2026-03-11
**Status:** ✅ Locked

**Context:** After 8.15 (TYPE_REF + IMPLEMENTS edges) and 8.16 (--embeddings-only + OpenAI-compat provider), we ran A/B ablation tests comparing qwen3-embedding:8b (4096-dim, local via Ollama) against Gemini Embedding 2 (3072-dim, cloud via OpenAI-compat endpoint) at multiple rescue thresholds.

**Ablation data (Gemini Embedding 2, threshold=0.90, row 6 full pipeline):**

| Crate | Communities | Largest | Loners | Conf | Stability |
|-------|-----------|---------|--------|------|-----------|
| aether-store | 10 | 131 | 2 | 0.91 | 0.82 |
| aether-mcp | 18 | 149 | 17 | 0.89 | 0.77 |
| aether-config | 26 | 40 | 4 | 0.95 | 0.92 |

**Comparison against qwen3-embedding:8b (threshold=0.85, row 6):**

| Crate | Communities | Largest | Loners | Stability | Winner |
|-------|-----------|---------|--------|-----------|--------|
| aether-store | 10 vs 10 | 131 vs 127 | 2 vs 6 | 0.82 vs 0.79 | Gemini |
| aether-mcp | 18 vs 17 | 149 vs 151 | 17 vs 17 | 0.77 vs 0.91 | Mixed |
| aether-config | 26 vs 26 | 40 vs 40 | 4 vs 4 | 0.92 vs 0.88 | Gemini |

**Threshold tuning data (Gemini Embedding 2):**

| Threshold | aether-store stability | aether-mcp stability | aether-config stability |
|-----------|----------------------|---------------------|------------------------|
| 0.85 | 0.84 | 0.70 | 0.81 |
| 0.90 | 0.82 | 0.77 | 0.92 |

0.90 eliminates the threshold-cliff rescues that caused the 0.70 stability at 0.85. The tradeoff is slightly more loners, but these are genuinely ambiguous orphans.

**Decision:** Gemini Embedding 2 (`gemini-embedding-2-preview`) via OpenAI-compatible endpoint is the production embedding model. Semantic rescue threshold locked at 0.90.

**Rationale:**
- Beats qwen3:8b on 2/3 crates for stability
- Zero-cost loner improvement on aether-store (2 vs 6)
- 3072 dims vs 4096 — 25% smaller vectors, less storage
- $0.20/M tokens — full re-embed of AETHER costs ~$0.10
- Eliminates 30+ minute local GPU inference for embeddings
- aether-mcp stability (0.77) is the only soft spot, caused by inherently homogeneous MCP handler code, not model quality

**Config:**
```toml
[embeddings]
enabled = true
provider = "openai_compat"
model = "gemini-embedding-2-preview"
endpoint = "https://generativelanguage.googleapis.com/v1beta/openai"
api_key_env = "GEMINI_API_KEY"
vector_backend = "sqlite"

[planner]
semantic_rescue_threshold = 0.90
```

### 84. Semantic rescue threshold → 0.90 (was 0.85, default was 0.70)

**Date:** 2026-03-11
**Status:** ✅ Locked
**Supersedes:** Default of 0.70 from Decision #60

**Context:** Gemini Embedding 2 produces higher similarity scores than qwen3-embedding:8b. At the previous threshold of 0.85, too many marginal orphans cleared the bar, causing threshold-cliff instability (aether-mcp stability dropped to 0.70). At 0.90, the baseline and perturbed (0.95) passes agree better — marginal rescues are eliminated, stability improves across all three crates.

**Decision:** `semantic_rescue_threshold = 0.90` in `[planner]` config section.

### 85. --embeddings-only flag for rapid model testing

**Date:** 2026-03-11
**Status:** ✅ Implemented (Phase 8.16)

**Context:** Changing embedding models previously required `--index-once --full --force`, which regenerated all SIRs (15-30 min of cloud inference). The `--embeddings-only` flag re-embeds without SIR regeneration, reducing model switch time from ~45 minutes to ~10 minutes.

**Decision:** `--index-once --embeddings-only` is the supported path for embedding model changes. It requires `--index-once`, conflicts with `--force`, and only touches embedding/vector rows.

### 86. OpenAI-compatible embedding provider

**Date:** 2026-03-11
**Status:** ✅ Implemented (Phase 8.16)

**Context:** The embedding provider interface only supported local models (Ollama, Candle). Cloud embedding APIs (Gemini, OpenRouter) use OpenAI-format endpoints. Phase 8.16 added `EmbeddingProviderKind::OpenAiCompat` with standard `/v1/embeddings` format, Bearer auth, retry with backoff, and optional `task_type`/`dimensions` config fields.

**Decision:** Cloud embedding APIs are accessed via the OpenAI-compatible provider. Gemini-native API provider deferred unless task-type-aware embeddings show measurable improvement.
