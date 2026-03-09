# Phase 8.12.1 — OpenAI-Compatible Embeddings + Embeddings-Only Reindex — Session Context

I'm continuing work on AETHER, a Rust multi-crate workspace (~55K+ LOC). We're in Phase 8, adding two targeted fixes discovered during Phase 8.12 post-merge validation.

**Before answering any questions about the codebase, clone or inspect the actual source. Do not rely on project knowledge files — they may be stale snapshots.**

```bash
# Repo is at:
/home/rephu/projects/aether

# Always grep/read actual source before making claims about what exists
```

## What just shipped

**Stage 8.12 — Community Detection Quality (merged to main):**
- Planner-local community detection with type-anchor pre-collapse, semantic rescue, stability checks
- Resolution-aware Louvain, improved naming, diagnostics, ablation harness
- CLI and MCP caller updates for file-scoped split suggestions

## What 8.12.1 fixes

Two bugs discovered during the 8.12 post-merge embedding model upgrade:

### Bug 1: No way to re-embed without full SIR regeneration

`refresh_embedding_if_needed()` only fires inside the SIR generation pipeline.
There is no standalone embedding refresh path. Changing the embedding model in
config requires `--index-once --full --force`, which regenerates ALL SIRs
(3494 symbols) just to trigger embedding refresh — even though SIRs are fresh.

On the AETHER workspace this wastes 15-30 minutes of Gemini API calls for
zero SIR quality change.

Fix: `--embeddings-only` CLI flag that iterates all symbols with existing SIR
and calls `refresh_embedding_if_needed` for each without SIR regeneration.

### Bug 2: No OpenAI-compatible embedding provider

`EmbeddingProviderKind` only supports `Mock`, `Qwen3Local` (Ollama), and
`Candle` (local in-process). Stage 7.8 added `OpenAiCompat` for inference
but explicitly excluded embeddings. This blocks using cloud embedding APIs
like OpenRouter.

The existing `Qwen3LocalEmbeddingProvider` sends Ollama-format requests:
```json
POST .../api/embeddings
{"model": "...", "prompt": "...text..."}
```

OpenAI-compatible endpoints expect:
```json
POST .../v1/embeddings
Authorization: Bearer <key>
{"model": "...", "input": "...text..."}
```

The response parser (`extract_embedding_vector`) already handles the OpenAI
`/data/0/embedding` JSON path. Only the request side needs work.

Fix: `EmbeddingProviderKind::OpenAiCompat` variant + provider struct.

## Key files to inspect

```
crates/aether-config/src/lib.rs              — EmbeddingProviderKind enum, EmbeddingsConfig struct
crates/aether-infer/src/lib.rs               — EmbeddingProvider trait, Qwen3LocalEmbeddingProvider,
                                               load_embedding_provider_from_config,
                                               extract_embedding_vector, EmbeddingProviderOverrides
crates/aether-infer/src/embedding/mod.rs     — embedding submodule (currently: pub mod candle)
crates/aether-infer/src/embedding/candle.rs  — CandleEmbeddingProvider (module pattern reference)
crates/aetherd/src/sir_pipeline.rs           — SirPipeline, refresh_embedding_if_needed (private)
crates/aetherd/src/indexer.rs                — run_full_index_once_inner, IndexerConfig
crates/aetherd/src/cli.rs                    — CLI args struct
crates/aetherd/src/main.rs                   — CLI dispatch, IndexerConfig construction
crates/aether-sir/src/lib.rs                 — canonicalize_sir_json, sir_hash
crates/aether-store/src/lib.rs               — SqliteStore, list_all_symbol_ids, get_sir_meta, read_sir_blob
```

## Important structural notes

- `aether-infer` already uses submodules: `embedding/candle.rs`, `reranker/candle.rs`,
  `reranker/cohere.rs`. New providers should follow this pattern, not flatten into `lib.rs`.
- `Qwen3LocalEmbeddingProvider` currently lives in `lib.rs` (legacy), but new providers
  should go in `embedding/openai_compat.rs` following the candle pattern.
- `--embeddings-only` must use the same config/provider resolution path as normal indexing
  (via `SirPipeline::new` → `load_embedding_provider_from_config`).
- Reranker modules exist but are out of scope for this stage.

## Build environment for all cargo commands:

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

Never run `cargo test --workspace` — OOM risk. Always per-crate:

```bash
cargo fmt --check
cargo clippy -p aether-config -p aether-infer -p aetherd -- -D warnings
cargo test -p aether-config
cargo test -p aether-infer
cargo test -p aetherd
```

## Scope guard (must NOT be modified)

- SIR generation pipeline (scan, triage, deep passes)
- Existing embedding providers (Qwen3Local, Candle, Mock) — behavior unchanged
- Store trait or Store implementations
- Community detection, health scoring, planner, dashboard
- MCP tools, LSP hover, coupling mining, drift detection
- Global community snapshot

## Architecture decisions

- **#63**: OpenAI-compatible embedding provider uses existing `extract_embedding_vector`
  response parser. Request format: `{"model": ..., "input": ...}` with Bearer auth header.
- **#64**: `--embeddings-only` flag reuses `refresh_embedding_if_needed` from `SirPipeline`.
  Does not create a new embedding refresh path — keeps the skip logic (sir_hash + provider +
  model match check) in one place.

## Target config for OpenRouter

```toml
[embeddings]
enabled = true
provider = "openai_compat"
model = "qwen/qwen3-embedding-8b"
endpoint = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"
vector_backend = "lancedb"
```

## End-of-stage git sequence

```bash
cd /home/rephu/projects/aether-phase8-embedding-compat
git push origin feature/phase8-embedding-compat

# Create PR via GitHub web UI, then after merge:
cd /home/rephu/projects/aether
git switch main
git pull --ff-only
git worktree remove ../aether-phase8-embedding-compat
git branch -D feature/phase8-embedding-compat
git worktree prune
```

I'll paste Codex output, errors, and questions as they come up. Help me troubleshoot and make decisions.
