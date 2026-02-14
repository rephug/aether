# Phase 5 - Stage 5.5: Adaptive Similarity Thresholds

## Purpose
Tune vector search gating per language so that semantic search returns relevant results without noise. Currently, AETHER uses a single hardcoded similarity threshold for all languages. But embedding similarity distributions differ by language — Rust symbols with explicit types tend to cluster tighter than Python symbols with dynamic typing and looser naming. A fixed threshold either misses relevant Python results or includes irrelevant Rust noise.

## Current implementation (what we're tuning)
- Semantic search returns results above a fixed similarity threshold (likely ~0.7 or whatever was set in Stage 3.4)
- The threshold is the same for all languages, all providers, all embedding dimensions
- No per-language or per-provider calibration exists
- Hybrid search RRF fusion doesn't use thresholds directly but the semantic candidate set is gated by the threshold

## Target implementation
- Per-language similarity thresholds stored in config and calibrated at index time
- A `calibrate` command computes optimal thresholds by analyzing the embedding distribution of the indexed codebase
- Config allows manual override per language
- Thresholds auto-adjust when switching embedding providers (since different models produce different similarity distributions)

## In scope
- Add per-language threshold config:
  ```toml
  [search.thresholds]
  default = 0.65                # fallback for unknown languages
  rust = 0.70                   # Rust symbols cluster tighter
  typescript = 0.65
  python = 0.60                 # Python symbols are more dispersed
  ```
- Implement threshold calibration algorithm:
  - For each language, sample N random symbol pairs from the same codebase
  - Compute pairwise cosine similarity distribution
  - Set threshold at a percentile that separates "same-module" pairs from "cross-module" pairs
  - Store calibrated thresholds in `.aether/config.toml` (user can override)
- Add `aether calibrate` CLI command:
  ```bash
  # Calibrate thresholds for the current codebase
  aether calibrate

  # Output:
  # Calibrated thresholds:
  #   rust: 0.72 (based on 1,234 symbols)
  #   typescript: 0.64 (based on 567 symbols)
  #   python: 0.58 (based on 890 symbols)
  # Written to .aether/config.toml [search.thresholds]
  ```
- Update semantic search to use per-language thresholds:
  - Query includes the language of the queried context (if available)
  - If no language context, use the `default` threshold
  - If searching across all languages, use the minimum threshold and post-filter
- Update hybrid search RRF to apply language-aware thresholds to the semantic candidate set
- Store threshold metadata:
  ```sql
  CREATE TABLE IF NOT EXISTS threshold_calibration (
      language TEXT PRIMARY KEY,
      threshold REAL NOT NULL,
      sample_size INTEGER NOT NULL,
      provider TEXT NOT NULL,
      model TEXT NOT NULL,
      calibrated_at TEXT NOT NULL
  );
  ```
- Invalidate calibration when embedding provider changes (different model = different distribution)

## Out of scope
- Automatic re-calibration (user must run `aether calibrate` manually or after provider switch)
- Per-symbol or per-file thresholds (too granular)
- Calibration for reranker scores (reranker produces absolute relevance scores, not similarity)
- Threshold tuning for lexical search (lexical search doesn't use similarity scores)

## Implementation notes

### Calibration algorithm
The goal is to find a threshold that separates "semantically related" symbol pairs from "unrelated" pairs. The proxy signal: symbols in the same file or same module are more likely to be related.

```
Algorithm: calibrate_threshold(language, embeddings, symbols)
  1. Collect all (symbol_id, file_path, embedding) for the given language
  2. If fewer than 20 symbols, return the default threshold
  3. Sample up to 500 intra-file pairs (symbols in the same file)
  4. Sample up to 500 inter-file pairs (symbols in different files)
  5. Compute cosine similarity for each pair
  6. Find the threshold that maximizes separation:
     - Threshold = (mean_intra + mean_inter) / 2
     - Or: use the 10th percentile of intra-file similarities
       (ensures 90% of same-file pairs are above threshold)
  7. Clamp to [0.3, 0.95] to prevent extreme values
  8. Return threshold
```

### Why per-language?
Empirically, embedding models produce different similarity distributions for different languages:
- **Rust**: strong type annotations and explicit lifetimes create distinctive signatures → tighter clusters → higher threshold
- **TypeScript**: mix of typed and untyped code → moderate spread → moderate threshold
- **Python**: dynamic typing, duck typing, flexible naming → wider spread → lower threshold

### Provider-aware invalidation
When the embedding provider changes (e.g., `gemini` → `candle`), existing thresholds are invalid because:
- Different models produce different embedding spaces
- A threshold of 0.70 for Gemini might be equivalent to 0.60 for Qwen3

The `threshold_calibration` table stores which provider/model was used. On provider change:
1. Log warning: "Embedding provider changed from X to Y. Thresholds may be inaccurate."
2. Use default thresholds until user runs `aether calibrate`
3. Or auto-calibrate if enough embeddings exist in the new provider's vector table

### Search flow with thresholds
```
semantic_search(query, language_hint):
  threshold = config.thresholds.get(language_hint) 
              ?? config.thresholds.default
  
  candidates = vector_store.search_nearest(query_embedding, limit=100)
  filtered = candidates.filter(|c| c.similarity >= threshold)
  return filtered
```

If searching across all languages (no hint):
```
  min_threshold = config.thresholds.values().min()
  candidates = vector_store.search_nearest(query_embedding, limit=100)
  filtered = candidates.filter(|c| {
      let lang_threshold = config.thresholds.get(c.language) ?? min_threshold;
      c.similarity >= lang_threshold
  })
```

### Config precedence
1. Manual override in `[search.thresholds]` in `.aether/config.toml` → highest priority
2. Calibrated value in `threshold_calibration` table → used if no manual override
3. Built-in defaults (rust: 0.70, typescript: 0.65, python: 0.60, default: 0.65) → fallback

## Edge cases

| Scenario | Behavior |
|----------|----------|
| Fewer than 20 symbols for a language | Use default threshold, skip calibration for that language |
| No embeddings exist yet (new project) | Use default thresholds, suggest running `aether calibrate` after indexing |
| Embedding provider changed | Warn and use defaults until re-calibration |
| User manually sets threshold in config | Manual value takes precedence over calibration |
| Mixed-language search query | Use minimum threshold across all languages, post-filter |
| Calibration produces extreme value (<0.3 or >0.95) | Clamp to [0.3, 0.95] range |
| `aether calibrate` run with no indexed symbols | Report "no symbols to calibrate" and exit |

## Pass criteria
1. Per-language thresholds are configurable in `[search.thresholds]`.
2. `aether calibrate` computes thresholds from the indexed codebase and writes to config.
3. Semantic search uses per-language thresholds (not a single global threshold).
4. Threshold calibration records which provider/model was used.
5. Changing embedding provider triggers a threshold invalidation warning.
6. Default thresholds work out of the box (no calibration required for basic usage).
7. Existing search tests pass (default thresholds should be close to the old fixed threshold).
8. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

### Test strategy
- Unit tests with mock embeddings that have known similarity distributions
- Test that calibration algorithm produces expected threshold for synthetic distributions
- Test that search respects per-language thresholds (high-threshold language filters more)
- Test config precedence: manual > calibrated > default
- Test provider change invalidation

## Exact Codex prompt(s)
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read these files for context first:
- docs/roadmap/phase_5_stage_5_5_adaptive_thresholds.md (this file)
- crates/aetherd/src/search.rs (search pipeline, where thresholds are applied)
- crates/aether-store/src/vector.rs (VectorStore trait, search_nearest method)
- crates/aether-config/src/lib.rs (config schema)
- crates/aether-store/src/lib.rs (Store trait, SQLite schema)

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase5-stage5-5-adaptive-thresholds off main.
3) Create worktree ../aether-phase5-stage5-5-adaptive-thresholds for that branch and switch into it.
4) Add per-language threshold config to [search.thresholds] in aether-config:
   - Fields: default, rust, typescript, python (all f32)
   - Defaults: 0.65, 0.70, 0.65, 0.60
5) Add threshold_calibration table to SQLite schema in aether-store:
   - Columns: language, threshold, sample_size, provider, model, calibrated_at
6) Implement calibration algorithm in crates/aetherd/src/calibrate.rs:
   - Sample intra-file and inter-file symbol pairs per language
   - Compute cosine similarity distribution
   - Calculate optimal threshold per language
   - Clamp to [0.3, 0.95]
   - Store in threshold_calibration table and write to config
7) Add `aether calibrate` CLI subcommand.
8) Update semantic search in search.rs to use per-language thresholds:
   - Get language hint from query context
   - Apply appropriate threshold before returning results
9) Add provider-change detection:
   - Compare current provider/model with calibrated provider/model
   - Warn if mismatched, use defaults
10) Add tests:
    - Calibration algorithm with synthetic embedding distributions
    - Search respects per-language thresholds
    - Config precedence: manual > calibrated > default
    - Provider change triggers warning
11) Run:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test --workspace
12) Commit with message: "Add adaptive per-language similarity thresholds".
```

## Expected commit
`Add adaptive per-language similarity thresholds`
