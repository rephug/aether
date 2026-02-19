# Phase 5 - Stage 5.4: Reranker Integration

## Purpose
Add an optional reranking stage to the search pipeline that re-scores candidate results after initial retrieval (lexical + semantic), improving search precision. Currently, hybrid search uses Reciprocal Rank Fusion (RRF) to merge lexical and semantic results — this works but treats all candidates equally. A reranker uses a cross-encoder model to score each (query, candidate) pair, producing more accurate relevance rankings.

## Current implementation (what we're extending)
- Hybrid search pipeline: lexical (SQL LIKE) → semantic (LanceDB ANN) → RRF fusion → return top N
- RRF fusion: `score = Σ 1/(k + rank_i)` across lexical and semantic rank lists
- Decision #6: "Reranking not enabled by default in Phase 1"
- No reranker infrastructure exists
- Candle runtime available from Stage 5.3 (can be reused)
- Embedding code is in `crates/aether-infer/src/embedding/` (refactored in 5.3)
- CLI uses flags (`--download-models`, `--workspace`, `--lsp`, etc.), NOT subcommands

## Target implementation
- `RerankerProvider` trait in `crates/aether-infer` with two implementations:
  - `CandleRerankerProvider` — Qwen3-Reranker-0.6B via Candle (local, offline)
  - `CohereRerankerProvider` — Cohere Rerank API (cloud, higher quality)
- Reranker inserted as optional stage in hybrid search: retrieve → rerank → return
- Config: `[search] reranker = "none" | "candle" | "cohere"` (default: `none`)
- When `none`, the pipeline is unchanged (RRF only)
- When enabled, top-N candidates from RRF are re-scored by the reranker
- Both CLI search and MCP `aether_search` use the reranker (shared pipeline)

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
      pub text: String,        // the text to score against query (SIR intent or symbol metadata)
  }

  pub struct RerankResult {
      pub id: String,
      pub score: f32,          // relevance score from reranker (0.0 to 1.0)
      pub original_rank: usize, // position in pre-rerank list
  }
  ```
- Implement `CandleRerankerProvider` in `crates/aether-infer/src/reranker/candle.rs`:
  - Reuses Candle runtime from Stage 5.3
  - Loads Qwen3-Reranker-0.6B (same lazy-loading OnceLock pattern as CandleEmbeddingProvider)
  - Cross-encoder scoring via hidden-state sigmoid heuristic: tokenize (query, candidate) pair → forward pass through Qwen2 module → mean-pool hidden states → sigmoid → score
  - **Note:** `candle-transformers` exposes Qwen2 hidden states but no built-in reranker classification head. The hidden-state sigmoid heuristic is a pragmatic approximation that produces meaningful relevance differentiation. Can be upgraded in-place when candle-transformers adds classification head support.
  - Model cached in `.aether/models/qwen3-reranker-0.6b/`
- Implement `CohereRerankerProvider` in `crates/aether-infer/src/reranker/cohere.rs`:
  - HTTP client to Cohere Rerank v2 API
  - Config: `[providers.cohere] api_key_env = "COHERE_API_KEY"`
  - Fail at provider construction time if the configured env var is missing or empty (not at config parse time)
- Insert reranker into search pipeline in `crates/aetherd/src/search.rs`:
  - After RRF fusion, if reranker is configured:
    1. Take top `rerank_window` candidates (default: 50, configurable)
    2. Build candidate text from SIR intent (preferred) with fallback to symbol metadata
    3. Call `reranker.rerank(query, candidates, limit)`
    4. Return reranked results
  - If no reranker, pipeline is unchanged
  - **On reranker construction or runtime failure:** fall back to RRF-only results, log warning, set `fallback_reason`
- MCP `aether_search` uses the same shared search pipeline, so reranking is automatically available through MCP when configured
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
- Update `--download-models` flag to also fetch reranker model when `search.reranker = "candle"`

## Out of scope
- Training or fine-tuning the reranker model
- Custom reranker models (only Qwen3-Reranker-0.6B and Cohere supported)
- Changing the lexical or semantic retrieval stages (only adding a post-retrieval rerank)
- Reranking for non-search use cases (e.g., SIR generation candidate selection)
- Quality benchmarking (deferred to Stage 5.5 threshold tuning)
- Full Qwen reranker classification head implementation (hidden-state heuristic is sufficient for now)
- CLI refactor to subcommands (flag-based CLI preserved)

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
[On reranker failure]
  Fall back to RRF results, log warning, set fallback_reason
  ↓
Return top N results
```

### Cross-encoder scoring (Candle)
Qwen3-Reranker-0.6B is architecturally Qwen2-family. The `candle-transformers` crate provides the Qwen2 module for extracting hidden states, but does not include a built-in reranker classification head. The implementation uses a hidden-state sigmoid heuristic:

```
Input: "[CLS] query text [SEP] document text [SEP]"
Forward pass through Qwen2 module → hidden states
Mean-pool hidden states → scalar
Sigmoid → relevance score (0.0 to 1.0)
```

This heuristic produces meaningful relevance differentiation for re-ordering candidates. The Cohere backend provides a higher-quality scoring path for users who need maximum precision.

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
Response includes relevance scores. Provider construction fails immediately if the API key env var is missing or empty — this failure is caught by the search pipeline caller, which falls back to RRF results with a warning.

### Candidate text selection
What text do we send to the reranker for each candidate?

Priority order:
1. SIR `intent` field + `qualified_name` + `kind` — most semantically rich, typically 1-3 sentences
2. `qualified_name` + `kind` + `file_path` — fallback when SIR is missing

The reranker input should be reasonably short (under 512 tokens per candidate) for performance. The SIR intent string is ideal — it's a natural language description of what the symbol does, which is exactly what a cross-encoder scores well against a search query.

### Lazy model loading
Same pattern as Stage 5.3:
- `CandleRerankerProvider::new()` doesn't load the model
- First call to `rerank()` triggers download + load
- Model held in `OnceLock<Arc<LoadedRerankerModel>>`
- If both embedding and reranker use Candle, they load independently (different models)

### Graceful degradation
The reranker follows the same fallback pattern as semantic search throughout AETHER:
- Semantic search falls back to lexical when embeddings unavailable
- Reranker falls back to RRF when reranker unavailable
- The `fallback_reason` field in CLI/MCP response envelopes surfaces why degradation occurred

## Edge cases

| Scenario | Behavior |
|----------|----------|
| `reranker = "candle"` but model not downloaded | Auto-download on first rerank call |
| `reranker = "cohere"` but no API key | Fail at provider construction; search falls back to RRF with warning and `fallback_reason` |
| Reranker returns fewer results than requested | Return what's available, log warning |
| All candidates score below 0.01 | Return empty results (irrelevant query) |
| Reranker timeout (Cohere API) | Fall back to RRF-only results, log warning |
| Candle reranker OOM | Unlikely at 0.6B params + 50 candidates; if occurs, reduce rerank_window |
| Query text is empty | Skip reranking, return RRF results directly |
| Zero candidates after RRF | Skip reranking, return empty |

## Build concerns
Reranker reuses the Candle runtime from Stage 5.3, so no additional heavy compilation expected beyond the new reranker module code. The Cohere provider adds `reqwest` (likely already a transitive dependency). No new system-level dependencies required.

## Pass criteria
1. `RerankerProvider` trait exists with `rerank()` method.
2. `CandleRerankerProvider` produces relevance scores for (query, candidate) pairs using hidden-state sigmoid heuristic.
3. `CohereRerankerProvider` calls Cohere API and returns relevance scores.
4. Config `[search] reranker = "none"` leaves the search pipeline unchanged.
5. Config `[search] reranker = "candle"` inserts reranking after RRF fusion.
6. Reranked results are in descending score order.
7. `--download-models` flag fetches reranker model when `search.reranker = "candle"`.
8. Reranker construction/runtime failure falls back to RRF results with logged warning and `fallback_reason`.
9. Both CLI search and MCP `aether_search` use reranked results when configured.
10. Existing search tests pass with `reranker = "none"`.
11. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

### Test strategy
- Unit tests use `MockRerankerProvider` that returns deterministic scores
- Test that pipeline with mock reranker re-orders results correctly
- Test that pipeline with `reranker = "none"` produces identical results to pre-stage behavior
- Test that reranker failure falls back to RRF results unchanged
- Integration tests with real Candle model gated behind `#[ignore]`
- Cohere API tests gated behind `#[ignore]` (requires API key)

## Exact Codex prompt(s)
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

NOTE: The current CLI uses flags (--workspace, --lsp, --search, etc.), NOT subcommands.
The --download-models flag was added in Stage 5.3. Extend it to also handle the reranker model — do not refactor the CLI parser.

You are working in the repo root of https://github.com/rephug/aether.

Read these files for context first:
- docs/roadmap/phase_5_stage_5_4_reranker.md (full specification)
- crates/aether-infer/src/embedding/candle.rs (Candle integration pattern from Stage 5.3 to follow)
- crates/aether-infer/src/embedding/mod.rs (EmbeddingProvider trait pattern)
- crates/aether-infer/src/lib.rs (provider loading logic)
- crates/aetherd/src/search.rs (current search pipeline with RRF)
- crates/aetherd/src/main.rs (CLI flags including --download-models)
- crates/aether-config/src/lib.rs (config schema)

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase5-stage5-4-reranker off main.
3) Create worktree ../aether-phase5-stage5-4-reranker for that branch and switch into it.
4) Create RerankerProvider trait in crates/aether-infer/src/reranker/mod.rs:
   - rerank(query, candidates, top_n) → Vec<RerankResult>
   - RerankCandidate { id, text }, RerankResult { id, score, original_rank }
5) Create MockRerankerProvider for testing.
6) Create CandleRerankerProvider in reranker/candle.rs:
   - Lazy-load Qwen3-Reranker-0.6B (same OnceLock pattern as CandleEmbeddingProvider)
   - Cross-encoder via hidden-state sigmoid heuristic: tokenize (query, document) pair → forward pass through Qwen2 module → mean-pool hidden states → sigmoid → score
   - Model cached in .aether/models/qwen3-reranker-0.6b/
7) Create CohereRerankerProvider in reranker/cohere.rs:
   - HTTP POST to https://api.cohere.com/v2/rerank
   - Model: rerank-v3.5
   - Fail at provider construction if COHERE_API_KEY env var is missing (not at config parse time)
8) Insert reranker into search pipeline in search.rs:
   - After RRF fusion, if reranker configured: take top rerank_window → build candidate text from SIR intent (fallback to qualified_name + kind + file_path) → rerank → return
   - If reranker = "none", pipeline unchanged
   - On reranker construction or runtime failure: fall back to RRF results, log warning, set fallback_reason
9) Update config schema:
   - [search] reranker = "none" | "candle" | "cohere"
   - [search] rerank_window = 50
   - [search.candle] model_dir (shared with embeddings candle config)
   - [providers.cohere] api_key_env = "COHERE_API_KEY"
10) Update existing --download-models flag to also fetch reranker model when search.reranker = "candle".
11) Add tests:
    - Mock reranker correctly re-orders candidates
    - Pipeline with reranker = "none" produces identical results to pre-stage
    - Reranker failure falls back to RRF unchanged
    - Config parsing accepts all reranker values
    - CandleRerankerProvider construction (no model needed)
    - Integration tests with real Candle model gated behind #[ignore]
    - Cohere API tests gated behind #[ignore]
12) Run:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test --workspace
13) Commit with message: "Add optional reranker with Candle and Cohere backends".
```

## Expected commit
`Add optional reranker with Candle and Cohere backends`
