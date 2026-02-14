# Phase 5 - Stage 5.3: Candle Local Embeddings

## Purpose
Add in-process local embedding generation using Qwen3-Embedding-0.6B via the Candle ML runtime. This removes the cloud dependency for retrieval — after this stage, semantic search works fully offline. SIR generation still requires the cloud API (Gemini), but search queries never leave the machine.

## Current implementation (what we're extending)
- `EmbeddingProvider` trait exists in `crates/aether-infer` with:
  - `GeminiEmbeddingProvider` — calls Gemini embedding API
  - `MockEmbeddingProvider` — deterministic fake embeddings for tests
- Embeddings are stored via `VectorStore` trait (LanceDB or SQLite backend)
- Config: `[embeddings] provider = "gemini" | "mock"`, with API key in `[providers.gemini]`
- Embedding dimension is variable (Gemini uses 768-dim by default)

## Target implementation
- New `CandleEmbeddingProvider` that loads Qwen3-Embedding-0.6B weights and runs inference in-process
- Model downloaded from Hugging Face Hub on first use, cached in `.aether/models/`
- Lazy loading: model weights loaded on first embedding request, not at daemon startup
- Config: `[embeddings] provider = "gemini" | "candle" | "mock"` (default remains `gemini`)
- Produces 1024-dimensional embeddings (Qwen3-Embedding-0.6B native dimension)
- Existing Gemini and mock providers unchanged

## In scope
- Add Candle dependencies to workspace:
  ```toml
  candle-core = "0.8"
  candle-nn = "0.8"
  candle-transformers = "0.8"
  hf-hub = "0.3"           # Hugging Face Hub client for model downloads
  tokenizers = "0.21"      # HF tokenizers for text preprocessing
  ```
- Create `CandleEmbeddingProvider` in `crates/aether-infer/src/embedding/candle.rs`
- Implement model management:
  - Download path: `.aether/models/qwen3-embedding-0.6b/`
  - Download on first use via `hf-hub` crate
  - Verify model integrity via SHA256 checksum
  - CLI command: `aether download-models` for pre-fetching
- Implement lazy loading:
  - `CandleEmbeddingProvider::new()` does NOT load the model
  - First call to `embed()` triggers download (if needed) + load
  - Subsequent calls reuse the loaded model (held in `Arc<Mutex<Option<LoadedModel>>>` or `OnceLock`)
- Add config field: `[embeddings] provider = "candle"`
- Add config field: `[embeddings.candle] model_dir = ".aether/models"` (optional override)
- Update `crates/aether-infer/src/embedding/mod.rs` to construct `CandleEmbeddingProvider` when config says `candle`
- Update `crates/aetherd` CLI with `download-models` subcommand

## Out of scope
- GPU acceleration (CPU-only for now; Candle supports Metal/CUDA but WSL2 GPU passthrough is complex)
- Model quantization (use float16 weights as-is)
- Local inference for SIR generation (that's Full Local deployment, Phase 6+)
- Candle for reranking (that's Stage 5.4)
- Changing the LanceDB vector storage (it already handles arbitrary dimensions)
- Benchmarking Candle vs Gemini quality (deferred to Stage 5.5 threshold tuning)

## Implementation notes

### Model architecture
Qwen3-Embedding-0.6B is a transformer encoder model:
- Architecture: Qwen2-family (similar to BERT-style encoder)
- Parameters: ~600M (float16)
- Output: 1024-dimensional dense vector
- Input: text up to 8192 tokens
- Download size: ~1.2GB

### Candle implementation pattern
```rust
pub struct CandleEmbeddingProvider {
    model_dir: PathBuf,
    model: OnceLock<Arc<LoadedModel>>,
}

struct LoadedModel {
    model: QwenModel,       // candle-transformers Qwen2 model
    tokenizer: Tokenizer,   // HF tokenizer
    device: Device,         // Device::Cpu
}

impl CandleEmbeddingProvider {
    pub fn new(model_dir: PathBuf) -> Self {
        Self {
            model_dir,
            model: OnceLock::new(),
        }
    }

    fn ensure_loaded(&self) -> Result<&Arc<LoadedModel>> {
        self.model.get_or_try_init(|| {
            // 1. Check if model files exist in model_dir
            // 2. If not, download via hf-hub
            // 3. Load weights into Candle tensors
            // 4. Build model + tokenizer
            Ok(Arc::new(LoadedModel { ... }))
        })
    }
}

#[async_trait]
impl EmbeddingProvider for CandleEmbeddingProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let model = self.ensure_loaded()?;
        // Tokenize → run forward pass → mean-pool → normalize → return
        // Note: Candle ops are sync (CPU). Wrap in spawn_blocking if needed.
        todo!()
    }

    fn embedding_dim(&self) -> usize { 1024 }
    fn provider_name(&self) -> &str { "candle" }
    fn model_name(&self) -> &str { "qwen3-embedding-0.6b" }
}
```

### Model download flow
1. Check `.aether/models/qwen3-embedding-0.6b/model.safetensors` exists
2. If not, use `hf-hub` to download from `Qwen/Qwen3-Embedding-0.6B`
3. Files needed: `model.safetensors`, `tokenizer.json`, `config.json`
4. Log download progress via `tracing` (info level)
5. Store SHA256 checksums in `.aether/models/qwen3-embedding-0.6b/checksums.txt`
6. On subsequent loads, verify checksums before loading

### Embedding pipeline
1. Tokenize input text with HF tokenizer (pad/truncate to max_length)
2. Run through Qwen3 encoder (forward pass on CPU)
3. Extract last hidden state
4. Mean pooling over non-padding tokens
5. L2 normalize the resulting vector
6. Return 1024-dim `Vec<f32>`

### Batching
The `embed()` method accepts `&[String]` for batch embedding. For Candle:
- Batch size of 8-16 texts at a time (CPU memory constraint)
- If input is larger, chunk into batches and concatenate results
- Each embedding takes ~50-100ms on CPU; batching amortizes overhead

### Dimension mismatch with existing embeddings
Gemini default dimension is 768; Qwen3-Embedding produces 1024-dim vectors. LanceDB uses per-dimension tables (Decision from Stage 4.1), so this is handled automatically — switching providers creates a new table. Existing Gemini embeddings remain in their 768-dim table; Candle embeddings go to a 1024-dim table. Search uses whichever table matches the active provider.

**Important:** When switching from Gemini to Candle, all symbols need re-embedding. The first search after switching will trigger incremental re-embedding as symbols are accessed, or the user can run `aether reindex --embeddings-only` to batch re-embed everything.

### `download-models` CLI command
```bash
# Pre-download model weights (optional, for offline setup)
aether download-models

# Or with custom model directory
aether download-models --model-dir /path/to/models
```

This is a convenience command — the model auto-downloads on first use anyway.

## Edge cases

| Scenario | Behavior |
|----------|----------|
| No internet during first embedding with `candle` provider | Error with clear message: "Model not found at {path}. Run `aether download-models` or switch to an API provider." |
| Model download interrupted | Incomplete files detected by checksum mismatch → re-download |
| Insufficient disk space for model | Error with message showing required space (~1.2GB) |
| Text exceeds 8192 token limit | Truncate to 8192 tokens, log warning |
| Empty text input | Return zero vector of dimension 1024 |
| Config says `candle` but model files corrupted | Checksum failure → attempt re-download → error if still fails |
| WSL2 memory pressure during inference | Candle CPU inference uses ~2GB RAM peak; within 12GB WSL allocation |
| Switching from `gemini` to `candle` | New embedding dimension → new LanceDB table → existing symbols need re-embedding |

## Build concerns
Candle with CPU features will be a heavy compile. Same mitigations as LanceDB:
- `CARGO_BUILD_JOBS=2` to prevent OOM
- `CARGO_TARGET_DIR=/home/rephu/aether-target` (disk, not tmpfs)
- Expect first build to take 10-15 minutes

## Pass criteria
1. `CandleEmbeddingProvider` loads Qwen3-Embedding-0.6B and produces 1024-dim embeddings.
2. Model files are downloaded on first use and cached in `.aether/models/`.
3. Lazy loading: daemon startup with `candle` provider does NOT load the model until first embedding request.
4. `aether download-models` CLI command pre-fetches model weights.
5. Config toggle: `[embeddings] provider = "candle"` selects the Candle backend.
6. Semantic search works end-to-end with Candle embeddings via LanceDB.
7. Existing `gemini` and `mock` providers are unchanged and tests pass.
8. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

### Test strategy
- Unit tests use a small mock model or pre-computed expected outputs (NOT the full 1.2GB model)
- Integration tests that require the real model should be gated behind `#[ignore]` or a `--include-ignored` flag
- The standard `cargo test --workspace` must pass without downloading the model

## Exact Codex prompt(s)
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read these files for context first:
- docs/roadmap/phase_5_stage_5_3_candle_embeddings.md (this file)
- crates/aether-infer/src/embedding/mod.rs (EmbeddingProvider trait, existing providers)
- crates/aether-store/src/vector.rs (VectorStore trait, LanceDB/SQLite backends)
- crates/aether-config/src/lib.rs (config schema)
- crates/aetherd/src/main.rs (CLI commands)
- crates/aetherd/src/search.rs (search pipeline)

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase5-stage5-3-candle-embeddings off main.
3) Create worktree ../aether-phase5-stage5-3-candle-embeddings for that branch and switch into it.
4) Add workspace dependencies:
   - candle-core = "0.8"
   - candle-nn = "0.8"
   - candle-transformers = "0.8"
   - hf-hub = "0.3"
   - tokenizers = "0.21"
5) Create crates/aether-infer/src/embedding/candle.rs:
   - CandleEmbeddingProvider struct with lazy model loading via OnceLock
   - Model download from HF Hub on first use, cached in .aether/models/
   - Tokenize → forward pass → mean pool → L2 normalize → 1024-dim output
   - Batch processing with chunk size 8-16
6) Update config schema to accept provider = "candle".
   - Add [embeddings.candle] section with model_dir field.
7) Update provider construction in crates/aether-infer to build CandleEmbeddingProvider.
8) Add `download-models` CLI subcommand to crates/aetherd.
9) Add tests:
   - Unit test: CandleEmbeddingProvider construction (no model needed)
   - Unit test: config parsing accepts "candle" provider
   - Unit test: embedding dimension is 1024
   - Integration test (ignored by default): real model produces non-zero embeddings
10) Verify gemini and mock provider tests still pass.
11) Run:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test --workspace
12) Commit with message: "Add Candle local embeddings with Qwen3-Embedding-0.6B".
```

## Expected commit
`Add Candle local embeddings with Qwen3-Embedding-0.6B`
