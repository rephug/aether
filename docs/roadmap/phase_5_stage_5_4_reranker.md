# Phase 5 - Stage 5.4: Reranker Integration

## Purpose
Add an optional reranking stage to the search pipeline that re-scores candidate results after initial retrieval (lexical + semantic), improving search precision. Currently, hybrid search uses Reciprocal Rank Fusion (RRF) to merge lexical and semantic results — this works but treats all candidates equally. A reranker uses a cross-encoder model to score each (query, candidate) pair, producing more accurate relevance rankings.

## Current implementation (what we're extending)
- Hybrid search pipeline: lexical (SQL LIKE) → semantic (LanceDB ANN) → RRF fusion → return top N
- RRF fusion: `score = Σ 1/(k + rank_i)` across lexical and semantic rank lists
- Decision #6: "Reranking not enabled by default in Phase 1"
- No reranker infrastructure exists
- Candle runtime available from Stage 5.3 (can be reused)

## Target implementation
- `RerankerProvider` trait in `crates/aether-infer` with two implementations:
  - `CandleRerankerProvider` — Qwen3-Reranker-0.6B via Candle (local, offline)
  - `CohereRerankerProvider` — Cohere Rerank API (cloud, higher quality)
- Reranker inserted as optional stage in hybrid search: retrieve → rerank → return
- Config: `[search] reranker = "none" | "candle" | "cohere"` (default: `none`)
- When `none`, the pipeline is unchanged (RRF only)
- When enabled, top-N candidates from RRF are re-scored by the reranker

## In scope
- Define `RerankerProvider` trait in `crates/aether-infer/src/reranker/mod.rs`:
  ```rust
  #[async_trait]
  pub trait RerankerProvider: Send + Sync {
      /// Re-score a list of candidates against a query.
      /// Returns candidates in descending relevance order with scores.
      async fn rerank(
          &self,
          query: &str,
          candidates: &[RerankCandidate],
          top_n: usize,
      ) -> Result<Vec<RerankResult>>;

      fn provider_name(&self) -> &str;
  }

  pub struct RerankCandidate {
      pub id: String,          // symbol_id
      pub text: String,        // the text to score against query (SIR summary or symbol body)
  }

  pub struct RerankResult {
      pub id: String,
      pub score: f32,          // relevance score from reranker (0.0 to 1.0)
      pub original_rank: usize, // position in pre-rerank list
  }
  ```
- Implement `CandleRerankerProvider` in `crates/aether-infer/src/reranker/candle.rs`:
  - Reuses Candle runtime from Stage 5.3
  - Loads Qwen3-Reranker-0.6B (same lazy-loading pattern as embeddings)
  - Cross-encoder scoring: tokenize (query, candidate) pair → forward pass → sigmoid → score
  - Model cached in `.aether/models/qwen3-reranker-0.6b/`
- Implement `CohereRerankerProvider` in `crates/aether-infer/src/reranker/cohere.rs`:
  - HTTP client to Cohere Rerank v2 API
  - Config: `[providers.cohere] api_key_env = "COHERE_API_KEY"`
  - Rate limiting consistent with Decision #17
- Insert reranker into search pipeline in `crates/aetherd/src/search.rs`:
  - After RRF fusion, if reranker is configured:
    1. Take top `rerank_window` candidates (default: 50, configurable)
    2. Fetch SIR summary text for each candidate
    3. Call `reranker.rerank(query, candidates, limit)`
    4. Return reranked results
  - If no reranker, pipeline is unchanged
- Update MCP search tools to use reranked results when available
- Add config fields:
  ```toml
  [search]
  reranker = "none"           # "none" | "candle" | "cohere"
  rerank_window = 50          # how many RRF candidates to rerank

  [search.candle]
  model_dir = ".aether/models"  # shared with embeddings

  [providers.cohere]
  api_key_env = "COHERE_API_KEY"
  ```
- Update `aether download-models` to also fetch reranker model when `reranker = "candle"`

## Out of scope
- Training or fine-tuning the reranker model
- Custom reranker models (only Qwen3-Reranker-0.6B and Cohere supported)
- Changing the lexical or semantic retrieval stages (only adding a post-retrieval rerank)
- Reranking for non-search use cases (e.g., SIR generation candidate selection)
- Quality benchmarking (deferred to Stage 5.5 threshold tuning)

## Implementation notes

### Reranker pipeline position
```
Query
  ↓
Lexical search (SQL LIKE) → ranked list A
Semantic search (LanceDB ANN) → ranked list B
  ↓
RRF fusion → merged ranked list (top rerank_window)
  ↓
[If reranker configured]
  Reranker scores (query, candidate) pairs → re-sorted list
  ↓
Return top N results
```

### Cross-encoder scoring (Candle)
Qwen3-Reranker-0.6B is a cross-encoder: it takes a (query, document) pair and outputs a single relevance score. This is fundamentally different from embeddings (which encode query and document independently).

```
Input: "[CLS] query text [SEP] document text [SEP]"
Output: scalar logit → sigmoid → relevance score (0.0 to 1.0)
```

For N candidates, this requires N forward passes (one per candidate). With 50 candidates at ~50ms each on CPU, that's ~2.5 seconds. This is acceptable for interactive search but should be noted in documentation.

### Cohere API integration
```
POST https://api.cohere.com/v2/rerank
{
  "model": "rerank-v3.5",
  "query": "what calls the authentication handler",
  "documents": ["fn auth_handler() { ... }", "fn login() { ... }"],
  "top_n": 10
}
```
Response includes relevance scores. Rate limit: respect Cohere's rate limits + AETHER's global budget (Decision #17).

### Candidate text selection
What text do we send to the reranker for each candidate?

Priority order:
1. SIR summary (if available) — most semantically rich
2. Symbol signature + first N lines of body — fallback when SIR is missing
3. Symbol name + qualified name — minimal fallback

The reranker input should be reasonably short (under 512 tokens per candidate) for performance.

### Lazy model loading
Same pattern as Stage 5.3:
- `CandleRerankerProvider::new()` doesn't load the model
- First call to `rerank()` triggers download + load
- Model held in `OnceLock<Arc<LoadedRerankerModel>>`
- If both embedding and reranker use Candle, they load independently (different models)

## Edge cases

| Scenario | Behavior |
|----------|----------|
| `reranker = "candle"` but model not downloaded | Auto-download on first rerank call |
| `reranker = "cohere"` but no API key | Error at config validation, not at search time |
| Reranker returns fewer results than requested | Return what's available, log warning |
| All candidates score below 0.01 | Return empty results (irrelevant query) |
| Reranker timeout (Cohere API) | Fall back to RRF-only results, log warning |
| Candle reranker OOM | Unlikely at 0.6B params + 50 candidates; if occurs, reduce rerank_window |
| Query text is empty | Skip reranking, return RRF results directly |
| Zero candidates after RRF | Skip reranking, return empty |

## Pass criteria
1. `RerankerProvider` trait exists with `rerank()` method.
2. `CandleRerankerProvider` produces relevance scores for (query, candidate) pairs.
3. `CohereRerankerProvider` calls Cohere API and returns relevance scores.
4. Config `[search] reranker = "none"` leaves the search pipeline unchanged.
5. Config `[search] reranker = "candle"` inserts reranking after RRF fusion.
6. Reranked results are in descending score order.
7. `aether download-models` fetches reranker model when `reranker = "candle"`.
8. Existing search tests pass with `reranker = "none"`.
9. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

### Test strategy
- Unit tests use `MockRerankerProvider` that returns deterministic scores
- Test that pipeline with mock reranker re-orders results correctly
- Test that pipeline with `reranker = "none"` produces identical results to pre-stage behavior
- Integration tests with real Candle model gated behind `#[ignore]`
- Cohere API tests gated behind `#[ignore]` (requires API key)

## Exact Codex prompt(s)
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read these files for context first:
- docs/roadmap/phase_5_stage_5_4_reranker.md (this file)
- crates/aether-infer/src/embedding/mod.rs (EmbeddingProvider pattern to follow)
- crates/aether-infer/src/embedding/candle.rs (Candle integration from Stage 5.3)
- crates/aetherd/src/search.rs (current search pipeline with RRF)
- crates/aether-config/src/lib.rs (config schema)

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase5-stage5-4-reranker off main.
3) Create worktree ../aether-phase5-stage5-4-reranker for that branch and switch into it.
4) Create RerankerProvider trait in crates/aether-infer/src/reranker/mod.rs:
   - rerank(query, candidates, top_n) → Vec<RerankResult>
   - RerankCandidate { id, text }, RerankResult { id, score, original_rank }
5) Create MockRerankerProvider for testing.
6) Create CandleRerankerProvider in reranker/candle.rs:
   - Lazy-load Qwen3-Reranker-0.6B (same pattern as CandleEmbeddingProvider)
   - Cross-encoder: tokenize (query, document) pair → forward pass → sigmoid → score
   - Model cached in .aether/models/qwen3-reranker-0.6b/
7) Create CohereRerankerProvider in reranker/cohere.rs:
   - HTTP POST to https://api.cohere.com/v2/rerank
   - Model: rerank-v3.5
   - Rate limiting via existing budget system
8) Insert reranker into search pipeline in search.rs:
   - After RRF fusion, if reranker configured: take top rerank_window → fetch SIR text → rerank → return
   - If reranker = "none", pipeline unchanged
9) Update config schema:
   - [search] reranker = "none" | "candle" | "cohere"
   - [search] rerank_window = 50
   - [providers.cohere] api_key_env
10) Update download-models command to also fetch reranker model.
11) Add tests:
    - Mock reranker correctly re-orders candidates
    - Pipeline with reranker = "none" produces identical results to pre-stage
    - Config validation rejects cohere without API key
    - CandleRerankerProvider construction (no model needed)
12) Run:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test --workspace
13) Commit with message: "Add optional reranker with Candle and Cohere backends".
```

## Expected commit
`Add optional reranker with Candle and Cohere backends`
