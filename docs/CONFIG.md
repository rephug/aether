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
```

Override precedence is CLI > config file defaults.
