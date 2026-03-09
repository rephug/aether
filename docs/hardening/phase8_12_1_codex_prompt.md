# Phase 8.12.1 — OpenAI-Compatible Embeddings + Embeddings-Only Reindex — Codex Prompt (v2)

## Preflight

```bash
git status --porcelain
# Must be clean. If not, stop and report dirty files.
git pull --ff-only
```

## Branch and worktree

```bash
git checkout -b feature/phase8-embedding-compat
git worktree add ../aether-phase8-embedding-compat feature/phase8-embedding-compat
cd ../aether-phase8-embedding-compat
```

## Build environment (use for ALL cargo commands)

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

## Context — read these files first

```
crates/aether-config/src/lib.rs              — EmbeddingProviderKind, EmbeddingsConfig, normalize_config
crates/aether-infer/src/lib.rs               — EmbeddingProvider trait, Qwen3LocalEmbeddingProvider,
                                               load_embedding_provider_from_config,
                                               extract_embedding_vector, EmbeddingProviderOverrides
crates/aether-infer/src/embedding/mod.rs     — embedding submodule (currently exports candle)
crates/aether-infer/src/embedding/candle.rs  — CandleEmbeddingProvider (reference for module pattern)
crates/aether-store/src/vector.rs            — VectorStore trait, VectorRecord (= SymbolEmbeddingRecord)
crates/aetherd/src/sir_pipeline.rs           — SirPipeline, refresh_embedding_if_needed (private)
crates/aetherd/src/indexer.rs                — run_full_index_once_inner, IndexerConfig
crates/aetherd/src/cli.rs                    — CLI args
crates/aetherd/src/main.rs                   — CLI dispatch, IndexerConfig construction
crates/aether-sir/src/lib.rs                 — canonicalize_sir_json, sir_hash
```

## Critical rule

**If the actual source layout or type signatures differ from this prompt, follow the
source, not the prompt. Stop and report the mismatch rather than forcing the prompt's
assumptions.**

## Scope guard — do NOT modify

- SIR generation pipeline (scan, triage, deep passes)
- Store trait or Store implementations
- Existing embedding providers (Qwen3Local, Candle, Mock) — do not change their behavior
- Reranker providers, reranker config, SearchRerankerKind — do not touch
- Community detection, health scoring, planner, planner_communities, dashboard
- MCP tools, LSP hover, coupling mining, drift detection
- Global community snapshot

## Design principle

Two focused fixes, minimal blast radius. This is an embedding-only phase — do NOT
generalize toward rerankers or other provider types even though reranker scaffolding
exists in the codebase. Reuse existing patterns and module structure. Follow the
existing embedding module layout (`crates/aether-infer/src/embedding/`) rather than
flattening into `lib.rs`.

---

## Implementation steps

### Step 1: Add `OpenAiCompat` variant to `EmbeddingProviderKind`

File: `crates/aether-config/src/lib.rs`

1. Add `#[serde(rename = "openai_compat")] OpenAiCompat` variant to `EmbeddingProviderKind`.
2. Update `as_str()` to return `"openai_compat"`.
3. Update `FromStr` to accept `"openai_compat"`. Update the error message to list the new option.
4. Add `api_key_env: Option<String>` field to `EmbeddingsConfig` with
   `#[serde(default, skip_serializing_if = "Option::is_none")]`.
5. Update `Default for EmbeddingsConfig` — `api_key_env: None`.
6. In `normalize_config`, add validation: if `embeddings.provider == OpenAiCompat` and
   `embeddings.endpoint` is None or empty, push a config warning.
7. If `embeddings.provider == OpenAiCompat` and `embeddings.api_key_env` is None,
   default it to `"OPENAI_COMPAT_API_KEY"`. But if the user explicitly set `api_key_env`
   in config, preserve their value — only apply the default when the field is absent.

Add tests:
- `embedding_provider_kind_from_str_accepts_openai_compat`
- `embedding_provider_kind_openai_compat_as_str_matches_config_value`
- `load_workspace_config_parses_openai_compat_embedding_provider`
- `normalize_warns_on_openai_compat_without_endpoint`
- `normalize_preserves_explicit_api_key_env`

### Step 2: Create `OpenAiCompatEmbeddingProvider`

**Follow the existing module structure.** The codebase already has:
```
crates/aether-infer/src/embedding/mod.rs     — re-exports candle
crates/aether-infer/src/embedding/candle.rs  — CandleEmbeddingProvider
```

Create: `crates/aether-infer/src/embedding/openai_compat.rs`

Add `pub mod openai_compat;` to `crates/aether-infer/src/embedding/mod.rs`.

```rust
pub struct OpenAiCompatEmbeddingProvider {
    client: reqwest::Client,
    endpoint: String,  // base URL, e.g. "https://openrouter.ai/api/v1"
    pub model: String, // pub so loader can read it for model_name
    api_key: Secret<String>,
}
```

Implementation:

1. Constructor: `pub fn new(endpoint: String, model: String, api_key: Secret<String>) -> Self`
   - Normalize endpoint: strip trailing `/`, strip trailing `/embeddings` if present.
   - Use `inference_http_client()` for the reqwest client (same as other providers).

2. `async fn request_embedding(&self, text: &str) -> Result<Vec<f32>, InferError>`:
   ```rust
   let url = format!("{}/embeddings", self.endpoint);
   let body = json!({
       "model": self.model,
       "input": text
   });
   let response: Value = self.client
       .post(&url)
       .header("Authorization", format!("Bearer {}", self.api_key.expose_secret()))
       .json(&body)
       .send()
       .await?
       .error_for_status()?
       .json()
       .await?;
   extract_embedding_vector(&response)
   ```

   Import `extract_embedding_vector` from the parent module — it already handles the
   OpenAI `/data/0/embedding` JSON path. Check the actual import path; it may be
   `super::extract_embedding_vector` or `crate::extract_embedding_vector`.

3. Implement `EmbeddingProvider`:
   ```rust
   #[async_trait]
   impl EmbeddingProvider for OpenAiCompatEmbeddingProvider {
       async fn embed_text(&self, text: &str) -> Result<Vec<f32>, InferError> {
           self.request_embedding(text).await
       }
   }
   ```

### Step 3: Wire into `load_embedding_provider_from_config`

File: `crates/aether-infer/src/lib.rs`

Add the import for the new provider (follow the existing candle import pattern).

In the `match selected_provider` block inside `load_embedding_provider_from_config`,
add the `OpenAiCompat` arm:

```rust
EmbeddingProviderKind::OpenAiCompat => {
    let api_key_env_name = first_non_empty(
        overrides.api_key_env,
        config.embeddings.api_key_env.clone(),
    )
    .unwrap_or_else(|| DEFAULT_OPENAI_COMPAT_API_KEY_ENV.to_owned());

    let api_key = std::env::var(&api_key_env_name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| InferError::MissingApiKey(api_key_env_name.clone()))?;

    let endpoint = first_non_empty(overrides.endpoint, config.embeddings.endpoint.clone())
        .ok_or(InferError::MissingEndpoint)?;

    let model = first_non_empty(overrides.model, config.embeddings.model.clone())
        .ok_or_else(|| InferError::Config(
            aether_config::ConfigError::Generic(
                "openai_compat embedding provider requires a model".into()
            )
        ))?;

    let provider = OpenAiCompatEmbeddingProvider::new(
        endpoint, model, Secret::new(api_key),
    );
    LoadedEmbeddingProvider {
        model_name: provider.model.clone(),
        provider: Box::new(provider),
        provider_name: EmbeddingProviderKind::OpenAiCompat.as_str().to_owned(),
    }
}
```

Check the actual signature of `InferError::MissingApiKey` — if it takes a `String`,
pass the env var name. If it's a unit variant, adapt to match. Do NOT change the
existing error variant's shape.

Add `api_key_env: Option<String>` to `EmbeddingProviderOverrides` with default `None`.

Add tests:
- `load_embedding_provider_openai_compat_requires_endpoint`
- `load_embedding_provider_openai_compat_requires_model`
- `load_embedding_provider_openai_compat_requires_api_key`
- `load_embedding_provider_openai_compat_constructs_with_valid_config`

### Step 4: Add `--embeddings-only` CLI flag and implementation

File: `crates/aetherd/src/cli.rs`

Add a new flag to the main `Cli` struct:

```rust
#[arg(long, help = "Re-embed all symbols with existing SIR using the current embedding provider, then exit")]
pub embeddings_only: bool,
```

File: `crates/aetherd/src/main.rs`

Add handling BEFORE the `--index-once` block. The `--embeddings-only` path must
honor the same workspace config resolution used by normal indexing. Specifically:

```rust
if cli.embeddings_only {
    return run_embeddings_only_command(&workspace);
}
```

`run_embeddings_only_command` should:
1. Load workspace config via `ensure_workspace_config`.
2. Construct `SirPipeline::new(workspace, 1, ProviderOverrides::default())` — this
   loads the embedding provider from workspace config through the same
   `load_embedding_provider_from_config` path that normal indexing uses. The inference
   provider loaded here is irrelevant (it won't be called).
3. Open `SqliteStore`.
4. Delegate to a new method on `SirPipeline`.

File: `crates/aetherd/src/sir_pipeline.rs`

Add a public method to `SirPipeline`:

```rust
pub fn run_embeddings_only_pass(&self, store: &SqliteStore) -> Result<()>
```

Implementation:
1. List all symbol IDs: `store.list_all_symbol_ids()?`.
2. For each symbol ID:
   a. `store.get_sir_meta(id)` — skip if `None` (no SIR).
   b. `store.read_sir_blob(id)` — skip if `None`.
   c. Parse: `serde_json::from_str::<SirAnnotation>(&blob)` — skip on error.
   d. `let canonical = canonicalize_sir_json(&sir);`
   e. `let hash = sir_hash(&sir);`
   f. Call `self.refresh_embedding_if_needed(id, &hash, &canonical, print_sir, &mut stdout)`.
3. Print progress every 100 symbols: `"Embedding {current}/{total}..."`.
4. Print final summary: `"Embeddings refreshed: {refreshed}/{total} symbols ({skipped} skipped, {errors} errors)"`.

The existing skip logic inside `refresh_embedding_if_needed` handles the
"already embedded with this provider+model+sir_hash" check — no duplicate logic needed.

Add tests:
- `cli_embeddings_only_flag_parses`
- `embeddings_only_skips_symbols_without_sir` (mock-based)

### Step 5: Validation

```bash
cargo fmt --check
cargo clippy -p aether-config -p aether-infer -p aetherd -- -D warnings
cargo test -p aether-config
cargo test -p aether-infer
cargo test -p aetherd
```

Do NOT run `cargo test --workspace` — OOM risk.

### Step 6: Commit

```bash
git add -A
git commit -m "Add OpenAI-compatible embedding provider and --embeddings-only reindex flag"
```

---

## Post-implementation verification

```bash
# Update workspace config for OpenRouter
# Edit /home/rephu/projects/aether/.aether/config.toml:
#
# [embeddings]
# enabled = true
# provider = "openai_compat"
# model = "qwen/qwen3-embedding-8b"
# endpoint = "https://openrouter.ai/api/v1"
# api_key_env = "OPENROUTER_API_KEY"
# vector_backend = "lancedb"

# Rebuild
cargo build --release -p aetherd

# Re-embed all symbols without SIR regeneration
/home/rephu/aether-target/release/aetherd --embeddings-only \
  --workspace /home/rephu/projects/aether

# Verify new LanceDB table was created
ls -la /home/rephu/projects/aether/.aether/vectors/

# Run ablation with new embeddings
cargo test -p aether-health -- ablation --ignored --nocapture 2>&1 | tee /tmp/ablation-openrouter-8b.txt
```

---

## Summary of changes from v1 prompt

1. **Source layout takes precedence.** Added critical rule: if actual source disagrees
   with this prompt, follow the source and report the mismatch.

2. **Provider lives in the embedding module.** New file at
   `crates/aether-infer/src/embedding/openai_compat.rs` following the existing
   `candle.rs` pattern, not flattened into `lib.rs`.

3. **`--embeddings-only` honors the same config resolution as normal indexing.** Uses
   `SirPipeline::new` which calls `load_embedding_provider_from_config` with workspace
   config — same path as `--index-once --full`.

4. **Explicit embedding-only scope.** Added scope guard line for reranker modules.
   Added design principle: do not generalize toward rerankers.

5. **Config defaults preserve explicit user values.** `api_key_env` defaults to
   `OPENAI_COMPAT_API_KEY` only when the field is absent. If the user set
   `api_key_env = "OPENROUTER_API_KEY"` explicitly, that value is preserved.
   Added `normalize_preserves_explicit_api_key_env` test.
