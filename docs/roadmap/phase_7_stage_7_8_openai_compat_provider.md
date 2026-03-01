# Phase 7 — Stage 7.8: OpenAI-Compatible Inference Provider

## Purpose

Add a generic `OpenAiCompat` inference provider that works with any service exposing an OpenAI-compatible `/chat/completions` endpoint. This unlocks Z.ai (Zhipu GLM-4.7/GLM-5), NanoGPT (200+ models including Qwen3 Coder), OpenRouter, and any other OpenAI-compatible API — all with a single provider implementation. Today, AETHER supports exactly two live inference backends: Gemini (cloud) and Qwen3Local (Ollama). Adding a new cloud provider requires writing a full provider struct with bespoke HTTP request/response handling. This stage eliminates that bottleneck permanently.

## Current implementation (what exists)

- `InferenceProvider` trait in `crates/aether-infer/src/lib.rs` with `generate_sir()` and related methods
- `InferenceProviderKind` enum: `Auto`, `Mock`, `Gemini`, `Qwen3Local`
- `GeminiProvider` — uses Gemini-specific REST API format (`/v1beta/models/{model}:generateContent`)
- `Qwen3LocalProvider` — uses Ollama's `/api/generate` format (NOT OpenAI-compatible)
- `InferenceConfig` in `aether-config` with `provider`, `model`, `endpoint`, `api_key_env` fields
- `load_provider_from_env_or_mock()` matches on `InferenceProviderKind` to construct provider
- `summarize_text_with_config()` has a parallel match block for summarization
- `build_strict_json_prompt()` constructs the SIR generation prompt (provider-agnostic)
- Retry logic in `run_sir_parse_validation_retries()` (provider-agnostic)
- Temperature already set for Qwen3Local (0.1); Gemini uses API defaults

## Target implementation

- New `OpenAiCompat` variant in `InferenceProviderKind`
- New `OpenAiCompatProvider` struct implementing `InferenceProvider`
- Talks to `{endpoint}/chat/completions` with standard OpenAI request format
- Supports `system` + `user` messages, `temperature`, `response_format` (JSON mode), streaming (optional)
- Config-driven: endpoint URL, model name, and API key env var are all user-specified
- Works with Z.ai (`https://api.z.ai/api/paas/v4`), NanoGPT (`https://nano-gpt.com/api/v1`), OpenRouter, etc.
- `Auto` provider resolution updated: tries `GEMINI_API_KEY` first (existing behavior), then falls back to checking for `OPENAI_COMPAT_API_KEY` before falling through to Mock
- README updated with provider configuration examples

## In scope

- Add `OpenAiCompat` variant to `InferenceProviderKind` enum in `crates/aether-config/src/lib.rs`:
  ```rust
  pub enum InferenceProviderKind {
      Auto,
      Mock,
      Gemini,
      Qwen3Local,
      OpenAiCompat,  // NEW
  }
  ```
  - `as_str()` returns `"openai_compat"`
  - `FromStr` accepts `"openai_compat"`

- Add constants in `crates/aether-config/src/lib.rs`:
  ```rust
  pub const DEFAULT_OPENAI_COMPAT_API_KEY_ENV: &str = "OPENAI_COMPAT_API_KEY";
  pub const OPENAI_COMPAT_SIR_TEMPERATURE: f32 = 0.1;
  pub const OPENAI_COMPAT_DEFAULT_MAX_TOKENS: u32 = 4096;
  ```

- Create `OpenAiCompatProvider` struct in `crates/aether-infer/src/lib.rs` (or a new `openai_compat.rs` submodule if the file is too large):
  ```rust
  pub struct OpenAiCompatProvider {
      api_key: Secret<String>,
      model: String,
      endpoint: String,  // base URL, e.g. "https://api.z.ai/api/paas/v4"
      client: reqwest::Client,
  }
  ```

- Implement `InferenceProvider` for `OpenAiCompatProvider`:
  - `generate_sir()` sends POST to `{endpoint}/chat/completions` with:
    ```json
    {
      "model": "{model}",
      "messages": [
        {"role": "system", "content": "{system_prompt}"},
        {"role": "user", "content": "{user_prompt}"}
      ],
      "temperature": 0.1,
      "max_tokens": 4096,
      "response_format": {"type": "json_object"}
    }
    ```
  - Parses standard OpenAI response: `choices[0].message.content`
  - Handles error responses: HTTP 4xx/5xx with provider error message extraction
  - `response_format` is attempted first; if the provider returns a 400 indicating it doesn't support JSON mode, falls back to prompting for JSON without `response_format` field (some providers like NanoGPT routing to older models may not support it)

- Implement `request_openai_compat_summary()` for `summarize_text_with_config()`:
  - Same `/chat/completions` call but without `response_format` (summaries are free text)
  - Returns `choices[0].message.content`

- Wire into `load_provider_from_env_or_mock()`:
  ```rust
  InferenceProviderKind::OpenAiCompat => {
      let api_key = read_env_non_empty(&selected_api_key_env)
          .ok_or_else(|| InferError::MissingApiKey(selected_api_key_env.clone()))?;
      let endpoint = selected_endpoint
          .ok_or_else(|| InferError::MissingEndpoint)?;
      let model = selected_model
          .unwrap_or_else(|| "glm-4.7".to_owned());
      let provider = OpenAiCompatProvider::new(
          Secret::new(api_key), model.clone(), endpoint,
      );
      Ok(LoadedProvider {
          provider: Box::new(provider),
          provider_name: InferenceProviderKind::OpenAiCompat.as_str().to_owned(),
          model_name: model,
      })
  }
  ```

- Wire into `summarize_text_with_config()` match block for `OpenAiCompat`

- Add `MissingEndpoint` variant to `InferError`:
  ```rust
  #[error("openai_compat provider requires an endpoint URL in config")]
  MissingEndpoint,
  ```

- Update `validate_config()` in `aether-config` to warn when `provider = "openai_compat"` but `endpoint` is empty

- Add README section "Cloud Providers (OpenAI-Compatible)" with config examples for:
  - Z.ai (GLM-4.7)
  - Z.ai Coding Plan
  - NanoGPT (Qwen3 Coder)
  - NanoGPT (DeepSeek V3.2)
  - Generic OpenRouter example

- Add tests (see Pass Criteria)

## Out of scope

- Streaming support for OpenAI-compatible endpoints (non-streaming is sufficient for SIR generation; streaming adds complexity with SSE parsing for minimal benefit on batch inference)
- Automatic provider discovery or health checking of third-party endpoints
- Token counting or cost estimation for third-party providers (we don't know their pricing)
- Provider-specific authentication schemes (OAuth, API key rotation) — all use Bearer token
- Changing the `Auto` provider resolution order (keep Gemini-first for backward compatibility)
- Embedding support via OpenAI-compatible endpoints (separate concern; Candle local embeddings and Gemini API cover this)
- OpenAI-specific features: function calling, vision inputs, tool use — SIR generation is text-in/JSON-out only
- Rate limiting per-provider (existing global rate limiter applies; provider-specific limits are a Phase 8+ concern)
- Renaming `Qwen3Local` to `Ollama` (same rationale as Stage 5.7 — would break existing configs)

## Locked decisions

### 44. OpenAI-compatible provider as generic gateway

A single `openai_compat` provider kind covers all OpenAI API-compatible services. The alternative — adding individual enum variants per service — creates unbounded enum growth, duplicated HTTP code, and a maintenance burden for what is functionally identical API shapes. The user specifies `endpoint`, `model`, and `api_key_env`; AETHER handles the rest.

### 45. JSON mode with graceful fallback

The provider attempts `response_format: {"type": "json_object"}` first for SIR generation. If the endpoint returns HTTP 400 with an error indicating unsupported feature, the provider retries without `response_format` and relies on the existing prompt-level JSON instructions + retry logic. This maximizes reliability across providers with varying feature sets.

### 46. Temperature 0.1 for all SIR generation via OpenAI-compatible providers

Matches the Ollama local provider (Decision #40 from Stage 5.7). SIR generation is structured extraction, not creative generation. Low temperature produces consistent JSON formatting and fewer hallucinated fields.

### 47. Default model: glm-4.7 (when model not specified)

GLM-4.7 is the most cost-effective code-capable model accessible via OpenAI-compatible APIs. It scores 84.9 on LiveCodeBench V6, has 200K context window, and is available at ~$0.52/M tokens. This default only applies when `provider = "openai_compat"` and `model` is not set — which implies the user set up Z.ai but forgot to specify a model. A sensible default prevents a confusing error.

## Implementation notes

### OpenAI chat completions request format

```rust
#[derive(Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
}

#[derive(Serialize)]
struct ChatMessage {
    role: String,  // "system" or "user"
    content: String,
}

#[derive(Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    format_type: String,  // "json_object"
}
```

### OpenAI chat completions response format

```rust
#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
    total_tokens: Option<u32>,
}
```

### Error response handling

```rust
#[derive(Deserialize)]
struct OpenAiErrorResponse {
    error: Option<OpenAiError>,
}

#[derive(Deserialize)]
struct OpenAiError {
    message: Option<String>,
    #[serde(rename = "type")]
    error_type: Option<String>,
    code: Option<String>,
}
```

On non-2xx responses:
1. Try to parse as `OpenAiErrorResponse` and extract `error.message`
2. If parsing fails, use the raw HTTP status + body as the error message
3. On HTTP 400 with `response_format` in the request, check if the error mentions "response_format" or "json" — if so, retry without `response_format`

### JSON mode fallback logic

```rust
async fn request_sir_json(&self, system: &str, user: &str) -> Result<String, InferError> {
    // First attempt: with response_format
    match self.chat_completion(system, user, Some(json_format())).await {
        Ok(content) => Ok(content),
        Err(InferError::ProviderRejectedFormat) => {
            // Fallback: without response_format
            tracing::info!("Provider does not support response_format; falling back to prompt-only JSON");
            self.chat_completion(system, user, None).await
        }
        Err(e) => Err(e),
    }
}
```

Add `ProviderRejectedFormat` variant to `InferError` (internal only — callers never see it):
```rust
#[error("provider rejected response_format")]
ProviderRejectedFormat,
```

### Provider-specific endpoint notes

| Provider | Base URL | Model examples | Notes |
|----------|----------|---------------|-------|
| Z.ai (Zhipu) | `https://api.z.ai/api/paas/v4` | `glm-4.7`, `glm-5`, `glm-4.6` | OpenAI-compatible. Coding Plan uses `/api/coding/paas/v4` |
| NanoGPT | `https://nano-gpt.com/api/v1` | `qwen3-coder:480b-cloud`, `deepseek-v3.2`, `glm-4.7:cloud` | Subscription covers open-source models. Auth: Bearer token |
| OpenRouter | `https://openrouter.ai/api/v1` | Various | Aggregator |

### Config examples for README

```toml
# === Z.ai (Zhipu GLM-4.7) — Recommended for code SIR ===
[inference]
provider = "openai_compat"
endpoint = "https://api.z.ai/api/paas/v4"
model = "glm-4.7"
api_key_env = "ZAI_API_KEY"

# === Z.ai Coding Plan ($3/month) ===
[inference]
provider = "openai_compat"
endpoint = "https://api.z.ai/api/coding/paas/v4"
model = "glm-4.7"
api_key_env = "ZAI_API_KEY"

# === NanoGPT with Qwen3 Coder ===
[inference]
provider = "openai_compat"
endpoint = "https://nano-gpt.com/api/v1"
model = "qwen3-coder:480b-cloud"
api_key_env = "NANOGPT_API_KEY"

# === NanoGPT with DeepSeek V3.2 ===
[inference]
provider = "openai_compat"
endpoint = "https://nano-gpt.com/api/v1"
model = "deepseek-v3.2"
api_key_env = "NANOGPT_API_KEY"

# === OpenRouter (any model) ===
[inference]
provider = "openai_compat"
endpoint = "https://openrouter.ai/api/v1"
model = "anthropic/claude-sonnet-4.5"
api_key_env = "OPENROUTER_API_KEY"
```

### Usage logging

Log provider, model, endpoint (redacted), and token usage after each successful call:
```rust
tracing::info!(
    provider = "openai_compat",
    model = %self.model,
    endpoint = %redact_url(&self.endpoint),
    prompt_tokens = usage.prompt_tokens,
    completion_tokens = usage.completion_tokens,
    "SIR generation complete"
);
```

`redact_url()` strips API keys from query params (shouldn't appear in headers-only auth, but defensive).

## Edge cases

| Scenario | Behavior |
|----------|----------|
| `provider = "openai_compat"` but `endpoint` is empty | `InferError::MissingEndpoint` at provider load time — clear error before any API call |
| `provider = "openai_compat"` but `api_key_env` points to unset env var | `InferError::MissingApiKey` — same as Gemini path |
| Provider returns empty `choices` array | `InferError::EmptyResponse` — "provider returned no choices" |
| Provider returns `choices[0].message.content = null` | `InferError::EmptyResponse` — "provider returned null content" |
| Provider returns non-JSON in content when JSON mode requested | Normal SIR retry logic handles this (existing `run_sir_parse_validation_retries`) |
| Provider returns HTTP 429 (rate limited) | `InferError::Request` from reqwest — existing retry/backoff applies |
| Provider returns HTTP 401/403 (auth failure) | `InferError::ProviderAuth` — "authentication failed: check {api_key_env}" |
| Provider doesn't support `response_format` | HTTP 400 → `ProviderRejectedFormat` → retry without it |
| Provider returns `finish_reason: "length"` (truncated) | Log warning, return partial content — SIR validation will catch incomplete JSON |
| Network timeout | reqwest timeout (30s default) → `InferError::Request` |
| Endpoint URL has trailing slash | Normalize: strip trailing `/` before appending `/chat/completions` |
| Endpoint URL already includes `/chat/completions` | Detect and don't double-append |

## Pass criteria

1. `InferenceProviderKind::OpenAiCompat` exists and round-trips through `as_str()` / `FromStr`.
2. Config with `provider = "openai_compat"` parses and loads correctly.
3. Config with `provider = "openai_compat"` and missing `endpoint` produces `InferError::MissingEndpoint`.
4. Config with `provider = "openai_compat"` and missing API key produces `InferError::MissingApiKey`.
5. `OpenAiCompatProvider` constructs correct request JSON (verified via mock HTTP server in tests).
6. `OpenAiCompatProvider` parses standard OpenAI response correctly.
7. `OpenAiCompatProvider` extracts error messages from non-2xx responses.
8. JSON mode fallback: when mock server returns 400 for `response_format`, provider retries without it.
9. Empty choices / null content handled gracefully with specific error messages.
10. Endpoint URL normalization works (trailing slashes, pre-existing `/chat/completions` path).
11. `summarize_text_with_config()` works with `OpenAiCompat` provider.
12. `validate_config()` warns when `openai_compat` is set without an endpoint.
13. README has "Cloud Providers (OpenAI-Compatible)" section with Z.ai, NanoGPT, and OpenRouter examples.
14. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)

```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read these files for context first:
- docs/roadmap/phase_7_stage_7_8_openai_compat_provider.md (this file)
- crates/aether-infer/src/lib.rs (InferenceProvider trait, GeminiProvider, Qwen3LocalProvider, load_provider_from_env_or_mock, summarize_text_with_config, run_sir_parse_validation_retries)
- crates/aether-config/src/lib.rs (InferenceProviderKind enum, InferenceConfig, validate_config)
- README.md (for adding provider documentation section)

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase7-stage7-8-openai-compat-provider off main.
3) Create worktree ../aether-phase7-stage7-8 for that branch and switch into it.

4) In crates/aether-config/src/lib.rs:
   a) Add `OpenAiCompat` variant to `InferenceProviderKind` enum
   b) Add `as_str()` returning "openai_compat" and `FromStr` accepting "openai_compat"
   c) Add constants:
      - DEFAULT_OPENAI_COMPAT_API_KEY_ENV = "OPENAI_COMPAT_API_KEY"
      - OPENAI_COMPAT_SIR_TEMPERATURE: f32 = 0.1
      - OPENAI_COMPAT_DEFAULT_MAX_TOKENS: u32 = 4096
      - OPENAI_COMPAT_DEFAULT_MODEL = "glm-4.7"
   d) In validate_config(), add warning when provider is openai_compat but endpoint is None or empty

5) In crates/aether-infer/src/lib.rs (or create crates/aether-infer/src/openai_compat.rs if the file is >1500 lines):
   a) Add InferError variants:
      - MissingEndpoint with message "openai_compat provider requires an endpoint URL in config"
      - ProviderRejectedFormat with message "provider rejected response_format"
      - ProviderAuth(String) with message "authentication failed: {0}"
      - EmptyResponse(String) with message "provider returned empty response: {0}"
   b) Define request/response structs:
      - ChatCompletionRequest { model, messages, temperature, max_tokens, response_format }
      - ChatMessage { role, content }
      - ResponseFormat { type: "json_object" }
      - ChatCompletionResponse { choices, usage }
      - Choice { message, finish_reason }
      - ChoiceMessage { content: Option<String> }
      - Usage { prompt_tokens, completion_tokens, total_tokens }
      - OpenAiErrorResponse { error: Option<OpenAiError> }
      - OpenAiError { message, type, code }
   c) Create OpenAiCompatProvider struct with:
      - api_key: Secret<String>
      - model: String
      - endpoint: String (base URL)
      - client: reqwest::Client
   d) Implement constructor that normalizes endpoint URL:
      - Strip trailing '/'
      - If endpoint ends with '/chat/completions', strip that too (user pasted full URL)
   e) Implement private chat_completion() method:
      - POST to {endpoint}/chat/completions
      - Headers: Authorization: Bearer {key}, Content-Type: application/json
      - On 2xx: parse ChatCompletionResponse, extract content, log usage
      - On 400 with response_format in request: check if error mentions "response_format" or "json_object" or "unsupported" — if so return ProviderRejectedFormat
      - On 401/403: return ProviderAuth with message from error response
      - On other errors: return InferError::Request or decode error message
   f) Implement InferenceProvider for OpenAiCompatProvider:
      - generate_sir(): calls chat_completion with system prompt from build_strict_json_prompt, temperature 0.1, max_tokens 4096, response_format json_object. On ProviderRejectedFormat, retry without response_format.
      - Other trait methods as needed (follow existing pattern from GeminiProvider)
   g) Implement request_openai_compat_summary() for free-text summarization (no response_format)
   
6) Wire OpenAiCompat into load_provider_from_env_or_mock():
   - OpenAiCompat arm: read api_key from env, require endpoint, default model to OPENAI_COMPAT_DEFAULT_MODEL
   - Handle MissingEndpoint and MissingApiKey errors

7) Wire OpenAiCompat into summarize_text_with_config() match block

8) Add tests:
   a) InferenceProviderKind::OpenAiCompat round-trips through as_str/FromStr
   b) Config parsing with provider = "openai_compat" succeeds
   c) Config validation warns on openai_compat without endpoint
   d) load_provider_from_env_or_mock with openai_compat + missing endpoint = MissingEndpoint error
   e) load_provider_from_env_or_mock with openai_compat + missing API key = MissingApiKey error
   f) OpenAiCompatProvider normalizes trailing slash from endpoint
   g) OpenAiCompatProvider normalizes endpoint that already contains /chat/completions
   h) Request body construction: verify JSON structure matches OpenAI spec (use serde_json to build expected vs actual)
   i) Response parsing: mock a valid ChatCompletionResponse JSON string, verify content extraction
   j) Error response parsing: mock error JSON, verify error message extraction
   k) JSON mode fallback: test that ProviderRejectedFormat triggers retry without response_format
   
   NOTE: Do NOT make real HTTP calls in tests. Use serialization/deserialization tests
   for request/response format validation. For HTTP-level tests, use a mock server 
   (e.g., wiremock or mockito crate) only if already in dependencies; otherwise test
   the serialization layer only. The integration test is manual: configure a real 
   provider and run `aetherd --workspace . index`.

9) Add README.md section "Cloud Providers (OpenAI-Compatible)" after the existing 
   "Local Inference" section. Include:
   - Brief explanation: one provider kind, many services
   - Config examples for: Z.ai (GLM-4.7), Z.ai Coding Plan, NanoGPT (Qwen3 Coder),
     NanoGPT (DeepSeek), OpenRouter
   - Note that api_key_env points to an environment variable NAME, not the key itself
   - Note that any OpenAI-compatible endpoint works — the examples are just common ones

10) Run:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test --workspace

11) Commit with message: "Add OpenAI-compatible inference provider for Z.ai, NanoGPT, and others"
```

## Expected commit

`Add OpenAI-compatible inference provider for Z.ai, NanoGPT, and others`

## Dependency

Depends on: Phase 6 complete (same as all Phase 7 stages).
No dependency on other Phase 7 stages — this is a pure `aether-infer` + `aether-config` change.
Can be implemented in parallel with 7.1–7.7.
