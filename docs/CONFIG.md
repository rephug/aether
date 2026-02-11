# Configuration

AETHER uses a project-local config file:

- `<workspace>/.aether/config.toml`

If the file does not exist, `aetherd` creates it on startup with defaults.

## Current Schema

```toml
[inference]
provider = "auto" # auto | mock | gemini | qwen3_local
# model = "..."
# endpoint = "..."
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true # optional .aether/sir/*.json mirrors

[embeddings]
enabled = false
provider = "mock" # mock | qwen3_local
# model = "qwen3-embeddings-0.6B"
# endpoint = "http://127.0.0.1:11434/api/embeddings"
```

## Inference Fields

- `provider`
  - `auto`: if `api_key_env` env var exists, use Gemini; otherwise use Mock
  - `mock`: always deterministic local mock summaries
  - `gemini`: always Gemini; fails clearly if key missing
  - `qwen3_local`: local HTTP provider (no API key required)
- `model` (optional)
  - Provider-specific model string override
- `endpoint` (optional)
  - Used by `qwen3_local`
  - Default: `http://127.0.0.1:11434`
- `api_key_env`
  - Env var name for Gemini key
  - Default: `GEMINI_API_KEY`

## Storage Fields

- `mirror_sir_files`
  - `true` (default): write `.aether/sir/<symbol_id>.json` mirror files after SQLite writes
  - `false`: SQLite remains the only SIR persistence path

## Embedding Fields

- `enabled`
  - `false` (default): lexical search only
  - `true`: maintain/query semantic embedding index in SQLite
- `provider`
  - `mock`: deterministic offline embeddings for tests/dev
  - `qwen3_local`: local HTTP embedding endpoint
- `model` (optional)
  - Provider-specific model override (default `qwen3-embeddings-0.6B`)
- `endpoint` (optional)
  - Used by `qwen3_local` embeddings
  - Default: `http://127.0.0.1:11434/api/embeddings`

## Environment Variables

- `GEMINI_API_KEY` (or your custom `api_key_env`) for Gemini provider.

No key is required for `mock` or `qwen3_local`.

## CLI Overrides

`aetherd` can override config values at runtime:

```bash
--inference-provider <auto|mock|gemini|qwen3_local>
--inference-model <name>
--inference-endpoint <url>
--inference-api-key-env <ENV_VAR_NAME>
--search-mode <lexical|semantic|hybrid>
```

Override precedence is CLI > config file defaults.
