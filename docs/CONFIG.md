# Configuration

AETHER uses a project-local config file at `<workspace>/.aether/config.toml`.

If the file does not exist, `aetherd` creates it on startup with defaults.

## Inference

```toml
[inference]
provider = "auto" # auto | tiered | gemini | qwen3_local | openai_compat
# model = "..."
# endpoint = "..."
api_key_env = "GEMINI_API_KEY"
concurrency = 2
```

- `provider`
  - `auto`: local-first. If Ollama is reachable, use `qwen3_local`; otherwise if `api_key_env` is set, use Gemini; otherwise fail with an explicit configuration error.
  - `tiered`: use `[inference.tiered]` routing.
  - `gemini`: always Gemini.
  - `qwen3_local`: always Ollama-compatible local inference.
  - `openai_compat`: always OpenAI-compatible chat completions.
- `model`
  - Optional provider-specific override.
  - Gemini default: `gemini-3.1-flash-lite-preview`
- `endpoint`
  - Optional provider-specific endpoint override.
  - `qwen3_local` default: `http://127.0.0.1:11434`
- `api_key_env`
  - Env var name for Gemini or OpenAI-compatible providers.
  - Default: `GEMINI_API_KEY`
- `concurrency`
  - Default config value is `2`.
  - When `provider = "gemini"` and concurrency is left at the default value, AETHER normalizes it to `16`.
  - For `gemini-3.1-flash-lite-preview`, `concurrency = 16` is safe under the 4000 RPM limit.
  - For local Ollama on consumer hardware, `concurrency = 2` is appropriate.

## Three-Pass SIR Quality Pipeline

```toml
[sir_quality]
# Pass 2 — triage: enriched context, self-improvement, all or filtered symbols
triage_pass = true
triage_provider = "gemini"
triage_model = "gemini-3.1-flash-lite-preview"
triage_api_key_env = "GEMINI_API_KEY"
triage_priority_threshold = 0.0
triage_confidence_threshold = 1.0
triage_max_symbols = 0
triage_concurrency = 16
triage_timeout_secs = 180

# Shared enriched-context limit for quality passes
deep_max_neighbors = 10

# Pass 3 — deep: top-N, best model, CoT for local
deep_pass = true
deep_provider = "openai_compat"
deep_model = "anthropic/claude-sonnet-4.6"
deep_endpoint = "https://openrouter.ai/api/v1"
deep_api_key_env = "OPENROUTER_API_KEY"
deep_priority_threshold = 0.9
deep_confidence_threshold = 0.85
deep_max_symbols = 20
deep_concurrency = 4
deep_timeout_secs = 180
```

- Pass 1 is `scan`
  - Fast baseline SIR generation for all symbols.
- Pass 2 is `triage`
  - Enriched-context improvement pass.
  - Old Stage 8.8 `sir_quality.deep_*` pass-2 fields are treated as legacy `triage_*` fields when no `triage_*` keys are present.
- Pass 3 is `deep`
  - Best-model or CoT improvement pass on top-N selected symbols.
- `deep_max_neighbors`
  - Shared limit for neighbor intents included in enriched prompts.

## Storage

```toml
[storage]
mirror_sir_files = true
graph_backend = "surreal" # surreal | cozo | sqlite
```

- SIR source of truth is SQLite at `.aether/meta.sqlite`.
- Optional mirror files under `.aether/sir/*.json` are secondary copies only.

## Embeddings

```toml
[embeddings]
enabled = false
provider = "qwen3_local" # qwen3_local | candle
vector_backend = "lancedb" # lancedb | sqlite
# model = "qwen3-embeddings-0.6B"
# endpoint = "http://127.0.0.1:11434/api/embeddings"
```

- `enabled = false` keeps search lexical-only.
- `provider = "qwen3_local"` uses a local HTTP embedding endpoint.
- `provider = "candle"` uses the bundled local model path under `[embeddings.candle]`.

## Environment Variables

- `GEMINI_API_KEY` for Gemini, unless `api_key_env` overrides it.
- `OPENAI_COMPAT_API_KEY` or your configured `api_key_env` for `openai_compat`.

No API key is required for `qwen3_local`.

## CLI Overrides

`aetherd` can override config values at runtime:

```bash
--inference-provider <auto|tiered|gemini|qwen3_local|openai_compat>
--inference-model <name>
--inference-endpoint <url>
--inference-api-key-env <ENV_VAR_NAME>
--search-mode <lexical|semantic|hybrid>
--output <table|json>
```

Override precedence is CLI > config file > built-in defaults.

## Validation and Normalization

- Optional strings are trimmed and empty strings are discarded.
- Probability thresholds are clamped into valid ranges.
- `triage_timeout_secs` and `deep_timeout_secs` default to `180`.
- `deep_max_neighbors = 0` normalizes to `10`.
- Missing config files are created, but existing files are never overwritten.
