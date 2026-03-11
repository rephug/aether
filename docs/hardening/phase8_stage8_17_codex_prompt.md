# Codex Prompt — Phase 8.17: Gemini Native Embedding Provider

CRITICAL BUILD SETTINGS — use these for ALL cargo commands:
```
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

You are adding a Gemini-native embedding provider that supports
task-type-aware asymmetric embeddings. This is ~80-120 lines of new
code plus minor trait extensions and caller updates.

Read these files before writing any code:
- `docs/roadmap/phase_8_stage_8_17_gemini_native.md` (the full spec)
- `docs/hardening/phase8_stage8_17_session_context.md` (session context)
- The ACTUAL source files listed in the source inspection section below

## PREFLIGHT

```bash
cd /home/rephu/projects/aether
git status --porcelain
# Must be clean. If dirty, STOP and report.
git pull --ff-only
```

Create branch and worktree:
```bash
git worktree add ../aether-phase8-gemini-native -b feature/phase8-stage8-17-gemini-native
cd /home/rephu/projects/aether-phase8-gemini-native
```

## SOURCE INSPECTION

Before writing code, run these commands:

```bash
# Find the EmbeddingProvider trait — exact method signatures
grep -rn "trait.*EmbeddingProvider" crates/aether-infer/src/embedding/
grep -rn "fn embed" crates/aether-infer/src/embedding/

# Read the full trait definition
cat crates/aether-infer/src/embedding/mod.rs

# Find the OpenAI-compat embedding provider as reference
cat crates/aether-infer/src/embedding/openai_compat.rs

# Find EmbeddingProviderKind and its string mapping
grep -rn "enum EmbeddingProviderKind" crates/
grep -rn "as_str\|from_str" crates/aether-infer/src/lib.rs

# Find provider construction
grep -rn "load_embedding_provider_from_config" crates/aether-infer/src/lib.rs

# Find ALL callers of embed() across the workspace
grep -rn "\.embed(" crates/aetherd/
grep -rn "\.embed(" crates/aether-mcp/
grep -rn "\.embed(" crates/aether-infer/

# Find the indexing embedding path
grep -rn "refresh_embedding_if_needed" crates/aetherd/
grep -rn "refresh_embedding" crates/aetherd/src/sir_pipeline.rs

# Find search embedding paths
grep -rn "embed\|embedding" crates/aetherd/src/search.rs
grep -rn "embed\|embedding\|semantic_search\|search_nearest" crates/aether-mcp/src/lib.rs

# Find config structure
grep -rn "EmbeddingsConfig\|EmbeddingConfig" crates/aether-config/
```

Verify these assumptions (adapt if wrong):
1. `EmbeddingProvider` trait has an `embed()` method that takes text and
   returns a vector
2. The trait does NOT currently have any "purpose" or "task type" parameter
3. `EmbeddingProviderKind` has `Mock`, `Qwen3Local`, `Candle`, `OpenAiCompat`
4. The OpenAI-compat provider in `openai_compat.rs` has retry/timeout logic
   you can reuse as a pattern
5. The indexing path calls `embed()` somewhere inside `refresh_embedding_if_needed`
6. The search path calls `embed()` somewhere inside search handlers

## IMPLEMENTATION

### Step 1: Add EmbeddingPurpose enum

In the embedding module (likely `crates/aether-infer/src/embedding/mod.rs`):

```rust
/// Describes why an embedding is being generated.
/// Used by task-type-aware providers (Gemini native) to optimize
/// the embedding for its intended use case.
#[derive(Debug, Clone, Copy, Default)]
pub enum EmbeddingPurpose {
    /// Embedding SIR text for storage/indexing — optimized for "being found"
    #[default]
    Document,
    /// Embedding a search query — optimized for "finding"
    Query,
}
```

### Step 2: Extend the EmbeddingProvider trait

The goal is backward-compatible extension. Inspect the actual trait first.

**Option A (preferred):** Add a default method so existing providers don't
need changes:

```rust
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    // Existing method — keep as-is
    async fn embed(&self, text: &str) -> Result<Vec<f32>, ...>;

    // New method with default delegation
    async fn embed_with_purpose(
        &self,
        text: &str,
        _purpose: EmbeddingPurpose,
    ) -> Result<Vec<f32>, ...> {
        self.embed(text).await
    }
}
```

**Option B:** If the trait uses a different pattern (e.g., takes a config
struct), adapt accordingly. The key requirement is:
- Existing providers (`Qwen3Local`, `Candle`, `OpenAiCompat`, `Mock`) must
  NOT need changes to their `embed()` implementation
- Only `GeminiNative` uses the purpose parameter

### Step 3: Create GeminiNativeEmbeddingProvider

Create `crates/aether-infer/src/embedding/gemini_native.rs`:

```rust
pub struct GeminiNativeEmbeddingProvider {
    client: reqwest::Client,
    model: String,
    api_key: String,
    dimensions: Option<u32>,
}
```

**Request construction:**

```rust
let url = format!(
    "https://generativelanguage.googleapis.com/v1beta/models/{}:embedContent",
    self.model
);

let task_type = match purpose {
    EmbeddingPurpose::Document => "RETRIEVAL_DOCUMENT",
    EmbeddingPurpose::Query => "CODE_RETRIEVAL_QUERY",
};

let mut body = serde_json::json!({
    "content": {
        "parts": [{"text": text}]
    },
    "taskType": task_type
});

if let Some(dims) = self.dimensions {
    body["outputDimensionality"] = serde_json::json!(dims);
}
```

**Auth header:** `x-goog-api-key: {api_key}` — NOT `Authorization: Bearer`.

**Response parsing:** The native response format is:
```json
{"embedding": {"values": [0.1, 0.2, ...]}}
```

Parse `response["embedding"]["values"]` as `Vec<f32>`. This is a new
parser — do NOT reuse `extract_embedding_vector` (that's for OpenAI format).

**Retry:** Same 3-attempt, 1s/2s/4s exponential backoff as the OpenAI-compat
provider. Copy the pattern.

**Timeout:** Same as the shared inference HTTP client timeout.

The `embed()` base method (without purpose) should default to
`EmbeddingPurpose::Document`.

### Step 4: Add EmbeddingProviderKind variant and config

Add `GeminiNative` to `EmbeddingProviderKind`:
```rust
pub enum EmbeddingProviderKind {
    Mock,
    Qwen3Local,
    Candle,
    OpenAiCompat,
    GeminiNative,  // NEW
}
```

Update `as_str()` → `"gemini_native"`, `from_str()`, Display, etc.

Config:
```toml
[embeddings]
enabled = true
provider = "gemini_native"
model = "gemini-embedding-2-preview"
api_key_env = "GEMINI_API_KEY"
dimensions = 3072
```

No `endpoint` field — the URL is derived from the model name.

Wire into `load_embedding_provider_from_config`:
```rust
EmbeddingProviderKind::GeminiNative => {
    let api_key = resolve_api_key_from_env(&config.api_key_env)?;
    let provider = GeminiNativeEmbeddingProvider::new(
        config.model.clone(),
        api_key,
        config.dimensions,
    );
    // ... return provider
}
```

### Step 5: Update callers to pass EmbeddingPurpose

Find every call to `embed()` and determine if it should use
`embed_with_purpose()` instead:

**Indexing paths → `EmbeddingPurpose::Document`:**
- `refresh_embedding_if_needed` in `sir_pipeline.rs`
- The `--embeddings-only` path (also in sir_pipeline.rs)

**Search paths → `EmbeddingPurpose::Query`:**
- CLI search in `search.rs`
- MCP `aether_search` / semantic search in `aether-mcp/src/lib.rs`
- Any LSP search path

**How to update:** Change `provider.embed(text)` to
`provider.embed_with_purpose(text, purpose)` at each call site. For
existing providers this delegates to `embed()` with no behavior change.

## WHAT NOT TO CHANGE

- Community detection logic (planner_communities.rs) — zero changes
- Health scoring or ablation harness
- The OpenAI-compat provider — it stays as a generic option
- VectorStore trait or implementations
- SIR generation pipeline logic (only the embedding call)
- Existing providers' `embed()` implementations
- The `[planner]` config section

## TESTS

### Provider tests

1. **`gemini_native_constructs_from_config`**
   Config with `provider = "gemini_native"`, model, api_key_env. Assert:
   provider loads successfully.

2. **`gemini_native_sends_correct_request_format`**
   Mock HTTP. Assert: POST to
   `https://generativelanguage.googleapis.com/v1beta/models/{model}:embedContent`,
   `x-goog-api-key` header present (NOT `Authorization: Bearer`),
   request body has `content.parts[0].text` and `taskType`.

3. **`gemini_native_document_purpose_sends_retrieval_document`**
   Call `embed_with_purpose(text, Document)`. Assert: request body has
   `"taskType": "RETRIEVAL_DOCUMENT"`.

4. **`gemini_native_query_purpose_sends_code_retrieval_query`**
   Call `embed_with_purpose(text, Query)`. Assert: request body has
   `"taskType": "CODE_RETRIEVAL_QUERY"`.

5. **`gemini_native_default_embed_uses_document_purpose`**
   Call `embed(text)` (no purpose). Assert: request body has
   `"taskType": "RETRIEVAL_DOCUMENT"`.

6. **`gemini_native_includes_output_dimensionality`**
   Provider configured with `dimensions = 3072`. Assert: request body
   has `"outputDimensionality": 3072`.

7. **`gemini_native_parses_native_response`**
   Feed `{"embedding":{"values":[0.1, 0.2, 0.3]}}`. Assert: returns
   `vec![0.1, 0.2, 0.3]`.

8. **`gemini_native_config_requires_model`**
   Config missing model. Assert: error.

9. **`gemini_native_config_requires_api_key_env`**
   Config missing api_key_env. Assert: error.

### Backward compatibility

10. **`existing_openai_compat_ignores_purpose`**
    Call `embed_with_purpose(text, Query)` on OpenAI-compat provider.
    Assert: works identically to `embed(text)` — purpose is ignored.

11. **`existing_qwen3_local_unchanged`**
    Config with `provider = "qwen3_local"`. Assert: still loads and works.

12. **`existing_candle_unchanged`**
    Config with `provider = "candle"`. Assert: still loads and works.

### Caller integration

13. **`indexing_path_passes_document_purpose`**
    Verify the indexing pipeline uses `embed_with_purpose` with `Document`.

14. **`search_path_passes_query_purpose`**
    Verify search handlers use `embed_with_purpose` with `Query`.

## VALIDATION GATE

```bash
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

# Update config to gemini_native:
# [embeddings]
# provider = "gemini_native"
# model = "gemini-embedding-2-preview"
# api_key_env = "GEMINI_API_KEY"
# dimensions = 3072

export GEMINI_API_KEY="your-key"

# Re-embed with native provider
$CARGO_TARGET_DIR/release/aetherd --workspace /home/rephu/projects/aether \
  --index-once --embeddings-only

# Should show gemini_native/gemini-embedding-2-preview and re-embed all symbols
# Verify no errors — the native format is different so any format mismatch
# will surface as 400 Bad Request errors
```

## COMMIT

```bash
git add -A
git commit -m "Add Gemini native embedding provider with asymmetric task types

- New EmbeddingPurpose enum: Document (RETRIEVAL_DOCUMENT) for indexing,
  Query (CODE_RETRIEVAL_QUERY) for search
- EmbeddingProvider trait gains embed_with_purpose() with default fallback
  to embed() — existing providers unchanged
- GeminiNativeEmbeddingProvider uses Gemini embedContent API format with
  x-goog-api-key auth, taskType, and outputDimensionality
- Indexing paths pass Document purpose, search paths pass Query purpose
- Retry with 3-attempt exponential backoff matching OpenAI-compat provider
- Config: provider = gemini_native with model and api_key_env"
```

Do NOT push. Robert will test with Gemini API key first.
