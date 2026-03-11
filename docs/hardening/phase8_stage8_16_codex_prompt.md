# Codex Prompt — Phase 8.16: Embedding Refresh + OpenAI-Compat Provider

CRITICAL BUILD SETTINGS — use these for ALL cargo commands:
```
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

You are implementing two small features that unblock rapid embedding
model testing. Both are ~50-120 lines each.

Read these files before writing any code:
- `docs/roadmap/phase_8_stage_8_16_embedding_refresh.md` (the full spec)
- `docs/hardening/phase8_stage8_16_session_context.md` (session context)
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
git worktree add ../aether-phase8-embedding-refresh -b feature/phase8-stage8-16-embedding-refresh
cd /home/rephu/projects/aether-phase8-embedding-refresh
```

## SOURCE INSPECTION

Before writing code, run these commands and understand the existing
embedding infrastructure:

```bash
# Find the embedding refresh function
grep -rn "refresh_embedding_if_needed" crates/

# Find where it's called (should be inside SIR pipeline only)
grep -rn "refresh_embedding" crates/aetherd/

# Find the EmbeddingProviderKind enum
grep -rn "enum EmbeddingProviderKind" crates/

# Find the provider construction function
grep -rn "load_embedding_provider_from_config" crates/

# Find the response parser
grep -rn "extract_embedding_vector" crates/

# Find the existing OpenAI-compat INFERENCE provider as reference
grep -rn "OpenAiCompat\|openai_compat\|openai-compat" crates/aether-infer/

# Find CLI argument parsing
grep -rn "index.once\|index_once\|embeddings.only\|embeddings_only" crates/aetherd/

# Find how symbols with existing SIR are queried
grep -rn "list_all_sir\|get_sir\|sir_exists\|list_symbols" crates/aether-store/

# Find the embedding config fields
grep -rn "embeddings\|EmbeddingConfig\|embedding_provider" crates/aether-config/

# Find the timeout/retry pattern in the existing OpenAI-compat inference provider
grep -rn "timeout\|retry\|backoff\|max_retries" crates/aether-infer/
```

Verify these assumptions (adapt if wrong):
1. `refresh_embedding_if_needed` exists and takes a SIR blob + symbol info
2. It is only called from the SIR generation pipeline (no standalone path)
3. `EmbeddingProviderKind` has `Mock`, `Qwen3Local`, and `Candle` variants
4. There is an existing OpenAI-compat provider for INFERENCE (Stage 7.8)
   that you can use as a reference pattern for the EMBEDDING provider
5. The response parser `extract_embedding_vector` handles OpenAI format
6. The CLI uses clap or a similar arg parser
7. The inference OpenAI-compat provider has timeout/retry logic you can reuse

## IMPLEMENTATION

### Part 1: --embeddings-only flag

**Step 1:** Add the CLI flag.

Find the CLI argument struct in aetherd (likely uses clap derive or builder).
Add a flag:
```rust
/// Re-embed all symbols without regenerating SIR.
/// Use after changing the embedding model in config.
#[arg(long)]
embeddings_only: bool,
```

**Step 2:** Add the embedding-only refresh loop.

In the indexing pipeline (wherever `--index-once` is handled), add a
branch for `--embeddings-only`:

```rust
if args.embeddings_only {
    // 1. Load embedding provider from config
    // 2. Query all symbols that have existing SIR from the store
    // 3. For each symbol:
    //    a. Read the existing SIR blob
    //    b. Call refresh_embedding_if_needed with the SIR blob
    //    c. The skip logic inside refresh_embedding_if_needed handles
    //       deduplication (matching provider/model/sir_hash)
    // 4. Report: "Re-embedded N symbols with {provider}/{model}"
    return Ok(());
}
```

Key points:
- Do NOT call any SIR generation functions
- Do NOT call any LLM/inference endpoints
- The only API calls should be to the embedding provider
- If the embedding provider is disabled in config, error early with:
  `"Embedding provider is not configured. Set [embeddings] in config."`
- If a symbol has no current SIR text/blob, skip it and increment a
  skip counter. Do not attempt to generate SIR.
- Progress reporting: print how many symbols were processed, skipped
  (no SIR), and skipped (already up to date)

**Step 3:** Handle the flag interaction with other flags.

- `--embeddings-only` requires `--index-once` (it doesn't make sense
  as a persistent daemon mode)
- `--embeddings-only` is incompatible with `--force` (force triggers
  SIR regeneration which we're explicitly skipping)
- If both `--embeddings-only` and `--force` are set, error with a
  clear message

### Part 2: OpenAI-compatible embedding provider

**Step 1:** Add the enum variant.

In the file where `EmbeddingProviderKind` is defined:
```rust
pub enum EmbeddingProviderKind {
    Mock,
    Qwen3Local,
    Candle,
    OpenAiCompat,  // NEW
}
```

Update `as_str()`, `from_str()`, Display, and any other trait impls.

**Step 2:** Create the provider struct.

Create `crates/aether-infer/src/embedding/openai_compat.rs` (or add to
the existing embedding module — follow the pattern of the existing
providers):

```rust
pub struct OpenAiCompatEmbeddingProvider {
    client: reqwest::Client,
    endpoint: String,     // e.g. "https://generativelanguage.googleapis.com/v1beta/openai"
    model: String,        // e.g. "gemini-embedding-exp-03-07"
    api_key: String,      // resolved from env var
    task_type: Option<String>,  // optional, Gemini-specific
    dimensions: Option<u32>,    // optional, for custom output dims
}
```

Implement the embedding trait (whatever `EmbeddingProvider` or equivalent
trait exists — inspect the actual source):

The request should be:
```json
POST {endpoint}/embeddings
Authorization: Bearer {api_key}
Content-Type: application/json

{
  "model": "{model}",
  "input": "{text}"
}
```

If `task_type` is set, include it in the request body (Gemini accepts
this as an extra parameter).

If `dimensions` is set, include it in the request body (OpenAI and
Gemini both support this).

For the response, use the existing `extract_embedding_vector` function.

**Timeout and retry:** Use the same timeout/retry/backoff pattern as
the existing OpenAI-compat INFERENCE provider from Stage 7.8. Inspect
that provider to find the pattern, then replicate it. Do NOT invent
new retry logic. If no retry logic exists in the inference provider,
use a simple 3-attempt retry with exponential backoff (1s, 2s, 4s)
and a 30-second per-request timeout.

**Step 3:** Wire into config.

Add config support. Look at how the existing Qwen3Local provider reads
its config (endpoint, model fields). Follow the same pattern:

```toml
[embeddings]
enabled = true
provider = "openai_compat"
model = "gemini-embedding-exp-03-07"
endpoint = "https://generativelanguage.googleapis.com/v1beta/openai"
api_key_env = "GEMINI_API_KEY"
# Optional:
task_type = "CODE_RETRIEVAL"
dimensions = 3072
```

Wire into `load_embedding_provider_from_config`:
```rust
EmbeddingProviderKind::OpenAiCompat => {
    // Read endpoint, model, api_key_env from config
    // Resolve API key from environment
    // Construct OpenAiCompatEmbeddingProvider
}
```

**Step 4:** Inspect the existing OpenAI-compat inference provider.

Stage 7.8 added an OpenAI-compatible INFERENCE provider. Use it as a
reference for:
- How the endpoint URL is constructed
- How the API key is resolved from an env var
- Error handling patterns
- Timeout/retry logic

The embedding provider should follow the same conventions.

## HARD CONSTRAINTS FOR --embeddings-only

These are non-negotiable. If any of these are violated, the stage fails.

- Must NOT parse files or run tree-sitter
- Must NOT mutate symbols, SIR, sir_history, or graph edges in any store
- Must NOT trigger scan/triage/deep SIR generation
- Must NOT call any LLM/inference endpoint (only embedding endpoints)
- ONLY enumerate existing SIR-bearing symbols and refresh embedding rows
- If a symbol has no current SIR text, skip it and log the skip count;
  do not attempt to generate SIR in --embeddings-only mode
- Remote embedding providers must use the same timeout/retry pattern as
  the existing OpenAI-compat inference provider (inspect it first)

## WHAT NOT TO CHANGE

- SIR generation pipeline logic — do not modify
- VectorStore trait or implementations — do not modify
- Community detection or health scoring — do not modify
- Existing Qwen3Local, Candle, or Mock providers — do not modify
- Search/retrieval path — do not modify
- LanceDB or SQLite storage backends — do not modify
- Tree-sitter parsing or edge extraction — do not modify

## TESTS

### Part 1 (--embeddings-only)

1. **`embeddings_only_flag_parses`**
   CLI with `--index-once --embeddings-only` parses without error.

2. **`embeddings_only_rejects_without_index_once`**
   CLI with just `--embeddings-only` (no `--index-once`) produces error.

3. **`embeddings_only_rejects_with_force`**
   CLI with `--embeddings-only --force` produces error.

4. **`embeddings_only_calls_embedding_provider_not_inference`**
   Mock store with 3 symbols that have existing SIR. Mock embedding
   provider. Run embeddings-only. Assert: embedding provider called 3
   times, inference/SIR generation called 0 times.

5. **`embeddings_only_respects_skip_logic`**
   Mock store where 2/3 symbols already have matching embeddings.
   Assert: embedding provider called only 1 time.

6. **`embeddings_only_skips_symbols_without_sir`**
   Mock store with 5 symbols, only 3 have existing SIR. Assert:
   embedding provider called 3 times (not 5). Skip count reported as 2.

7. **`embeddings_only_does_not_mutate_non_embedding_state`**
   Record symbol count, sir row count, and symbol_edges count before
   running --embeddings-only. Assert all three are unchanged after.
   Only embedding/vector rows should change.

### Part 2 (OpenAI-compat)

8. **`openai_compat_embedding_provider_constructs_from_config`**
   Config with `provider = "openai_compat"` and required fields. Assert:
   provider loads successfully.

9. **`openai_compat_embedding_sends_correct_format`**
   Mock HTTP. Assert: POST to `{endpoint}/embeddings` with correct JSON
   body and Authorization header.

10. **`openai_compat_embedding_parses_response`**
    Feed standard OpenAI embedding response JSON. Assert: correct vector.

11. **`openai_compat_config_requires_endpoint`**
    Config missing endpoint. Assert: error with clear message.

12. **`openai_compat_config_requires_model`**
    Config missing model. Assert: error with clear message.

13. **`openai_compat_config_requires_api_key_env`**
    Config missing api_key_env. Assert: error with clear message.

14. **`existing_providers_unchanged`**
    Config with `provider = "qwen3_local"` still works.
    Config with `provider = "candle"` still works.
    Config with `provider = "mock"` still works.

## VALIDATION GATE

```bash
cargo fmt --check
cargo clippy -p aether-infer -p aether-config -p aetherd -- -D warnings
cargo test -p aether-infer
cargo test -p aether-config
cargo test -p aetherd
```

Quick manual validation (if Gemini API key available):
```bash
# Build release binary
cargo build -p aetherd --release

# Test embeddings-only with existing qwen3 local embeddings
$CARGO_TARGET_DIR/release/aetherd --workspace /home/rephu/projects/aether \
  --index-once --embeddings-only
# Should report "Re-embedded N symbols" or "N symbols already up to date"
```

## COMMIT

```bash
git add -A
git commit -m "Add --embeddings-only flag and OpenAI-compat embedding provider

- --embeddings-only iterates existing SIR and re-embeds without
  regenerating SIR, saving 15-30 min of inference credits per model swap
- Hard constraint: does not parse, mutate symbols/SIR/edges, or call
  inference endpoints — only refreshes embedding/vector rows
- OpenAI-compatible embedding provider supports Gemini Embedding 2,
  OpenRouter, and any /v1/embeddings-compatible API
- Uses same timeout/retry pattern as existing inference provider
- Optional task_type and dimensions config fields for providers that
  support them (Gemini CODE_RETRIEVAL, custom output dimensions)
- Existing qwen3_local, candle, and mock providers unchanged"
```

Do NOT push. Robert will review and test with Gemini Embedding 2 first.
