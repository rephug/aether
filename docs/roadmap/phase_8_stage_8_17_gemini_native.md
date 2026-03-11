# Phase 8 — Stage 8.17: Gemini Native Embedding Provider

## Purpose

Add a Gemini-native embedding provider that uses task-type-aware asymmetric
embeddings. Documents (SIR text) are embedded with `RETRIEVAL_DOCUMENT`,
search queries are embedded with `CODE_RETRIEVAL_QUERY`. This improves
retrieval precision by 5-15% over the symmetric OpenAI-compat path.

## Prerequisites

- Phase 8.16 merged (--embeddings-only + OpenAI-compat provider)
- Gemini Embedding 2 validated as production model (Decision #83)

## Why this matters

The current OpenAI-compatible endpoint produces symmetric embeddings —
the same embedding function is used for both indexing and search. Gemini's
native API supports asymmetric embeddings via `taskType`:

- `RETRIEVAL_DOCUMENT` — optimizes the embedding for "being found"
- `CODE_RETRIEVAL_QUERY` — optimizes the embedding for "finding code"

The model adjusts its internal weighting based on the task type. Documents
get embeddings that emphasize their distinguishing content; queries get
embeddings that emphasize the search intent. This asymmetry improves
retrieval precision without any other changes to the pipeline.

**Important:** This does NOT affect community detection / ablation results.
The ablation compares document-to-document similarity. Task type only
helps document-to-query retrieval (MCP search, CLI search, LSP search).

## Gemini native API format

### Request

```
POST https://generativelanguage.googleapis.com/v1beta/models/{model}:embedContent
x-goog-api-key: {api_key}
Content-Type: application/json

{
  "content": {
    "parts": [{"text": "...text to embed..."}]
  },
  "taskType": "RETRIEVAL_DOCUMENT",
  "outputDimensionality": 3072
}
```

### Response

```json
{
  "embedding": {
    "values": [0.1, 0.2, ...]
  }
}
```

Note: this is NOT the OpenAI format. Different URL structure, different
request body, different response body.

### Task types for AETHER

| Context | Task type | Used when |
|---------|-----------|-----------|
| Indexing (SIR embedding) | `RETRIEVAL_DOCUMENT` | `--index-once`, `--embeddings-only`, SIR pipeline |
| Search query | `CODE_RETRIEVAL_QUERY` | MCP `aether_search`, CLI search, LSP search |

### Auth

The native API uses `x-goog-api-key` header (NOT Bearer auth):
```
x-goog-api-key: {api_key}
```

## Implementation

### EmbeddingPurpose enum

The `EmbeddingProvider` trait currently has an `embed()` method that takes
text and returns a vector. It doesn't know whether it's embedding a
document for storage or a query for search.

Add a purpose enum:
```rust
pub enum EmbeddingPurpose {
    Document,  // Embedding SIR text for storage/indexing
    Query,     // Embedding a search query for retrieval
}
```

Extend the `embed()` trait method (or add an `embed_with_purpose()` method)
to accept an optional `EmbeddingPurpose`. Existing providers (`Qwen3Local`,
`Candle`, `OpenAiCompat`) ignore it — they produce symmetric embeddings
regardless. Only `GeminiNative` uses it.

**Backward compatibility:** If adding a parameter to `embed()` is too
disruptive, add a default method `embed_with_purpose()` that delegates to
`embed()` for all existing providers. The Gemini provider overrides it.

### GeminiNativeEmbeddingProvider

New provider struct:
```rust
pub struct GeminiNativeEmbeddingProvider {
    client: reqwest::Client,
    model: String,           // "gemini-embedding-2-preview"
    api_key: String,         // resolved from env var
    dimensions: Option<u32>, // optional, default 3072
}
```

Request construction:
- URL: `https://generativelanguage.googleapis.com/v1beta/models/{model}:embedContent`
- Auth: `x-goog-api-key: {api_key}` header
- Body includes `taskType` based on `EmbeddingPurpose`:
  - `Document` → `"RETRIEVAL_DOCUMENT"`
  - `Query` → `"CODE_RETRIEVAL_QUERY"`
  - If no purpose provided, default to `"RETRIEVAL_DOCUMENT"`
- Body includes `outputDimensionality` if `dimensions` is set

Response parsing:
- Extract `embedding.values` array (NOT the OpenAI format)
- New parser function — cannot reuse `extract_embedding_vector`

Retry: same 3-attempt, 1s/2s/4s backoff as the OpenAI-compat provider.

### Config

```toml
[embeddings]
enabled = true
provider = "gemini_native"
model = "gemini-embedding-2-preview"
api_key_env = "GEMINI_API_KEY"
dimensions = 3072
```

No `endpoint` field needed — the Gemini API URL is fixed and derived
from the model name.

### Caller updates

The indexing pipeline (`refresh_embedding_if_needed`) needs to pass
`EmbeddingPurpose::Document` when embedding SIR text.

The search paths need to pass `EmbeddingPurpose::Query`:
- `crates/aetherd/src/search.rs` — CLI search
- `crates/aether-mcp/src/lib.rs` — MCP `aether_search` tool
- Any LSP search path

**These are the only callers that need changes.** Inspect the actual
source to find all places where `embed()` is called.

## Scope guard

- Do NOT change community detection logic
- Do NOT change health scoring or ablation
- Do NOT change the OpenAI-compat provider
- Do NOT change VectorStore trait or implementations
- Do NOT change SIR generation pipeline logic (only the embedding call)
- Do NOT remove the OpenAI-compat provider — it stays as a generic option

## Key files

```
# EmbeddingProvider trait and purpose enum
crates/aether-infer/src/embedding/mod.rs
crates/aether-infer/src/lib.rs

# New provider
crates/aether-infer/src/embedding/gemini_native.rs  (new file)

# Config
crates/aether-config/src/lib.rs

# Callers that pass EmbeddingPurpose
crates/aetherd/src/sir_pipeline.rs          — indexing (Document)
crates/aetherd/src/search.rs                — CLI search (Query)
crates/aether-mcp/src/lib.rs                — MCP search (Query)
```

## Tests

### Provider tests
- `gemini_native_constructs_from_config`
  Config with `provider = "gemini_native"`. Assert: loads successfully.
- `gemini_native_sends_correct_request_format`
  Mock HTTP. Assert: POST to correct URL, `x-goog-api-key` header (not
  Bearer), request body has `content.parts[0].text`, `taskType`, and
  `outputDimensionality`.
- `gemini_native_uses_retrieval_document_for_document_purpose`
  Call with `EmbeddingPurpose::Document`. Assert: `taskType` is
  `"RETRIEVAL_DOCUMENT"`.
- `gemini_native_uses_code_retrieval_for_query_purpose`
  Call with `EmbeddingPurpose::Query`. Assert: `taskType` is
  `"CODE_RETRIEVAL_QUERY"`.
- `gemini_native_parses_native_response`
  Feed Gemini native response JSON (`{"embedding":{"values":[...]}}`).
  Assert: correct vector extracted.
- `gemini_native_config_requires_model`
  Config missing model. Assert: error.
- `gemini_native_config_requires_api_key_env`
  Config missing api_key_env. Assert: error.

### Backward compatibility
- `existing_openai_compat_provider_unchanged`
  Config with `provider = "openai_compat"`. Assert: still works, ignores
  `EmbeddingPurpose`.
- `existing_qwen3_local_provider_unchanged`
  Config with `provider = "qwen3_local"`. Assert: still works.
- `existing_candle_provider_unchanged`
  Config with `provider = "candle"`. Assert: still works.

### Integration
- `embed_purpose_document_in_indexing_pipeline`
  Verify the indexing pipeline passes `Document` purpose.
- `embed_purpose_query_in_search_path`
  Verify search paths pass `Query` purpose.

## Decisions

- **#87**: Gemini native provider uses `x-goog-api-key` header auth,
  NOT Bearer. URL derived from model name, no endpoint config needed.
- **#88**: `EmbeddingPurpose` is optional in the trait. Existing providers
  ignore it. Only GeminiNative uses it for task type selection.
- **#89**: Default purpose is `Document` when not specified (safe default
  for indexing, which is the most common path).

## Validation

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR

cargo fmt --check
cargo clippy -p aether-infer -p aether-config -p aether-mcp -p aetherd -- -D warnings
cargo test -p aether-infer
cargo test -p aether-config
cargo test -p aether-mcp
cargo test -p aetherd
```

Manual validation with Gemini API key:
```bash
cargo build -p aetherd --release

# Switch config to gemini_native and re-embed
$CARGO_TARGET_DIR/release/aetherd --workspace /home/rephu/projects/aether \
  --index-once --embeddings-only
# Should show "openai_compat" replaced by "gemini_native" in output
```

## End-of-stage git sequence

```bash
cd /home/rephu/projects/aether-phase8-gemini-native
git push origin feature/phase8-stage8-17-gemini-native

# Create PR via GitHub web UI, then after merge:
cd /home/rephu/projects/aether
git switch main
git pull --ff-only
git worktree remove ../aether-phase8-gemini-native
git branch -D feature/phase8-stage8-17-gemini-native
git worktree prune
```
