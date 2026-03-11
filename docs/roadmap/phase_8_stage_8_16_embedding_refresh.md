# Phase 8.16: Embedding Refresh Pipeline + OpenAI-Compatible Embedding Provider

## Purpose

Two small, tightly-scoped fixes that unblock rapid embedding model testing:

1. **`--embeddings-only` flag:** Re-embed all symbols without regenerating SIR.
   Currently, changing embedding models requires `--index-once --full --force`,
   which wastes 15-30 min of cloud inference credits regenerating SIRs that
   are already fresh.

2. **OpenAI-compatible embedding provider:** The current `EmbeddingProviderKind`
   only supports Ollama-format local models (`Qwen3Local`) and Candle. Cloud
   embedding APIs (Gemini Embedding 2, OpenRouter, etc.) use OpenAI-format
   endpoints that AETHER can't talk to yet.

Together, these enable the test loop: change config → `--embeddings-only` →
ablation → compare. Each model test takes ~10 minutes instead of ~45.

## Prerequisites

- Phase 8.15 merged (structural edges)
- Phase 8.14 merged (component-bounded rescue)

## Evidence / motivation

From the embedding issues backlog (`aether_embedding_issues_backlog.md`):

- `refresh_embedding_if_needed()` only fires inside the SIR generation pipeline.
  There is no standalone embedding refresh path.
- `EmbeddingProviderKind` only supports `Mock`, `Qwen3Local`, and `Candle`.
  The Gemini embedding API uses a different request format than Ollama.
- The response parser already handles the OpenAI `/data/0/embedding` JSON path.
  Only the request side needs work.

### Immediate use case: Gemini Embedding 2

`gemini-embedding-2-preview` launched recently:
- 3072 dimensions (vs qwen3-embedding:8b's 4096)
- $0.20/M tokens via Gemini API
- Has a `CODE_RETRIEVAL` task type specifically for code search
- OpenAI-compatible endpoint format

Testing it requires both fixes in this stage.

## Part 1: --embeddings-only flag

### Behavior

```bash
aetherd --workspace /path/to/project --index-once --embeddings-only
```

- Iterate all symbols with existing SIR in the SQLite store
- For each symbol, read the SIR blob and call `refresh_embedding_if_needed`
- Skip SIR generation entirely — no inference calls, no LLM API usage
- Respect the existing skip logic: if the embedding already matches the
  current provider/model/sir_hash, skip it
- Works with both `--full` and incremental modes (but `--full` is the
  typical use case for model switching)

### Implementation

Add `--embeddings-only` flag to the CLI argument parser in aetherd.

In the indexing pipeline (`sir_pipeline.rs` or `indexer.rs`):
- When `--embeddings-only` is set, skip the SIR generation loop
- Instead, query all symbols from the store that have existing SIR
- For each, call the embedding refresh path with the existing SIR blob
- Report progress: `"Re-embedding N symbols with {provider}/{model}..."`

**Estimated scope:** ~50-80 lines in aetherd.

### Error handling

- If no SIR exists for a symbol, skip it (don't error)
- If the embedding provider is disabled in config, error early with a
  clear message
- If the embedding API fails for a symbol, log and continue (same as
  the existing pipeline behavior)

## Part 2: OpenAI-compatible embedding provider

### Behavior

Config:
```toml
[embeddings]
enabled = true
provider = "openai_compat"
model = "gemini-embedding-exp-03-07"
endpoint = "https://generativelanguage.googleapis.com/v1beta/openai/"
api_key_env = "GEMINI_API_KEY"
```

For Gemini Embedding 2 specifically, the config would also support
an optional `task_type` field for the code retrieval task:
```toml
task_type = "CODE_RETRIEVAL"  # optional, Gemini-specific
```

### Implementation

1. Add `OpenAiCompat` variant to `EmbeddingProviderKind` enum.

2. Create `OpenAiCompatEmbeddingProvider` struct in
   `crates/aether-infer/src/embedding/`:
   - Sends requests in OpenAI format:
     ```json
     POST {endpoint}/v1/embeddings
     Authorization: Bearer {api_key}
     {"model": "...", "input": "...text..."}
     ```
   - For Gemini, the endpoint already includes the path prefix, so
     the provider should append `/embeddings` (or let the user configure
     the full URL — inspect how the inference OpenAI-compat provider
     in Stage 7.8 handles this)
   - Uses the existing `extract_embedding_vector` for response parsing
     (already handles the OpenAI `/data/0/embedding` JSON path)

3. Wire into `load_embedding_provider_from_config` match block.

4. Add config support under `[embeddings]`:
   - `provider = "openai_compat"`
   - `endpoint` — base URL (required)
   - `model` — model name string (required)
   - `api_key_env` — environment variable name for the API key (required)
   - `task_type` — optional, passed as extra parameter for providers
     that support it (Gemini)
   - `dimensions` — optional, for providers that support custom output
     dimensions

**Estimated scope:** ~80-120 lines.

### Request format details

Standard OpenAI embedding request:
```json
POST /v1/embeddings
{
  "model": "text-embedding-3-large",
  "input": "The food was delicious"
}
```

Gemini's OpenAI-compatible endpoint accepts the same format at:
`https://generativelanguage.googleapis.com/v1beta/openai/embeddings`

The API key goes in the Authorization header: `Bearer {key}`

### Response parsing

The existing `extract_embedding_vector` function already handles:
```json
{
  "data": [
    { "embedding": [0.1, 0.2, ...], "index": 0 }
  ],
  "model": "...",
  "usage": { "prompt_tokens": 8, "total_tokens": 8 }
}
```

No response parsing changes needed.

## Scope guard

- Do NOT change the SIR generation pipeline
- Do NOT change the VectorStore trait or implementations
- Do NOT change community detection or health scoring
- Do NOT change the existing Qwen3Local or Candle providers
- Do NOT change the search/retrieval path
- Do NOT change the LanceDB or SQLite storage backends

## Key files

```
# --embeddings-only flag
crates/aetherd/src/main.rs (or cli.rs)     — CLI argument parsing
crates/aetherd/src/sir_pipeline.rs          — embedding refresh loop
crates/aetherd/src/indexer.rs               — index-once flow

# OpenAI-compat embedding provider
crates/aether-infer/src/embedding/mod.rs    — provider construction
crates/aether-infer/src/embedding/          — new openai_compat.rs module
crates/aether-infer/src/lib.rs              — EmbeddingProviderKind enum
crates/aether-config/src/lib.rs             — config fields

# Inspect how the inference OpenAI-compat provider works (Stage 7.8):
grep -rn "OpenAiCompat\|openai_compat" crates/aether-infer/
```

## Tests

### Part 1 (--embeddings-only)
- `embeddings_only_skips_sir_generation`
  Mock store with 5 existing SIRs. Run with --embeddings-only. Assert:
  embedding provider called 5 times, SIR generation called 0 times.
- `embeddings_only_respects_skip_logic`
  Mock store where 3/5 symbols already have matching embeddings. Assert:
  embedding provider called only 2 times.
- `embeddings_only_errors_when_provider_disabled`
  Config has embeddings disabled. Assert: clear error message.

### Part 2 (OpenAI-compat)
- `openai_compat_provider_constructs_from_config`
  Config with `provider = "openai_compat"` and required fields. Assert:
  provider loads successfully.
- `openai_compat_sends_correct_request_format`
  Mock HTTP server. Assert: request body matches OpenAI format with
  correct model and input fields. Authorization header present.
- `openai_compat_parses_standard_response`
  Feed a standard OpenAI embedding response JSON. Assert: correct
  embedding vector extracted.
- `openai_compat_config_requires_endpoint_and_model`
  Config missing endpoint or model. Assert: error with clear message.
- `existing_qwen3_local_provider_unchanged`
  Config with `provider = "qwen3_local"`. Assert: still works as before.
- `existing_candle_provider_unchanged`
  Config with `provider = "candle"`. Assert: still works as before.

## Decisions

- **#80**: `--embeddings-only` iterates existing SIR, does not generate new SIR
- **#81**: OpenAI-compat embedding provider uses standard `/v1/embeddings`
  format with Bearer auth
- **#82**: Optional `task_type` and `dimensions` config fields for providers
  that support them (Gemini, OpenAI)

## Validation

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR

cargo fmt --check
cargo clippy -p aether-infer -p aether-config -p aetherd -- -D warnings
cargo test -p aether-infer
cargo test -p aether-config
cargo test -p aetherd

# Manual validation: re-embed with a different model
$CARGO_TARGET_DIR/release/aetherd --workspace /home/rephu/projects/aether \
  --index-once --embeddings-only
```

## End-of-stage git sequence

```bash
cd /home/rephu/projects/aether-phase8-embedding-refresh
git push origin feature/phase8-stage8-16-embedding-refresh

# Create PR via GitHub web UI, then after merge:
cd /home/rephu/projects/aether
git switch main
git pull --ff-only
git worktree remove ../aether-phase8-embedding-refresh
git branch -D feature/phase8-stage8-16-embedding-refresh
git worktree prune
```
