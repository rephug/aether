# Configuration

AETHER uses a project-local config file at `<workspace>/.aether/config.toml`.

If the file does not exist, `aetherd` creates it on startup with defaults.

## Inference

```toml
[inference]
provider = "auto" # auto | tiered | gemini | qwen3_local | openai_compat | omp | mock
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
  - `omp`: the Oh My Pi auth gateway (see [Oh My Pi models and batch pricing](#oh-my-pi-models-and-batch-pricing)).
  - `mock`: no model at all. Every symbol gets a `[MOCK]` placeholder SIR at confidence 0.1 from tree-sitter facts, so `aether_audit_candidates` ranks it first for `/scan` (zero-Gemini onboarding, Decision #121).
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

## Oh My Pi models and batch pricing

AETHER can address models the way an Oh My Pi (omp) project does: as routes of the form
`provider/model` (`anthropic/claude-fable-5`, `openai-codex/gpt-5.6-sol`,
`opencode-go/deepseek-v4-flash`), billed on whatever credential omp holds for that provider.
Two mechanisms cover the two ways you may want to pay:

| Path | What it bills | When to use it |
| --- | --- | --- |
| `[inference] provider = "omp"` | The omp credential for the route, normally a subscription login (Claude, Codex, opencode) | Online SIR generation: `aetherd index`, watcher reindexing, triage and deep passes |
| `[batch]` with `provider = "auto"` | The provider's own API key at **batch pricing** (50% off for Anthropic Message Batches, OpenAI Batch, Gemini Batch Mode) | `aetherd batch run` and the continuous monitor, for routes whose provider offers a batch API |

The omp gateway has no batch endpoint, so batch pricing is never available on a subscription
route: it always needs the provider's API key (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`,
`GEMINI_API_KEY`, or the `[batch.<provider>].api_key_env` override).

### 1. Log in, then let AETHER start the gateway

```bash
omp auth-broker login anthropic       # once per provider; opens the OAuth flow
aetherd --workspace . omp up          # starts the broker + gateway, detached
aetherd --workspace . omp status      # health, token, number of routes served
aetherd --workspace . omp down        # stops what `up` started
```

`up` spawns `omp auth-broker serve` (serving the credentials in `~/.omp/agent`) and
`omp auth-gateway serve` (OpenAI-compatible endpoint on `http://127.0.0.1:4000`, bearer token
in `~/.omp/auth-gateway.token`), logs them under `.aether/omp/`, and records their pids there.
With `autostart = true` (the default) every `aetherd` run that uses the `omp` provider does
the same on demand, so after login nothing else needs to be running.

```toml
[inference.omp]
autostart = true                   # spawn broker + gateway when the gateway is unreachable
# command = "omp"                  # the omp binary (absolute path allowed)
# broker_bind = "127.0.0.1:8765"
# gateway_bind = "127.0.0.1:4000"  # must agree with inference.endpoint when that is set
# startup_timeout_secs = 20
```

Starting them by hand works too (`omp auth-broker serve`, then `omp auth-gateway serve` with
`OMP_AUTH_BROKER_URL` pointing at the broker); `omp up` notices a healthy gateway and leaves
it alone. The gateway lists the routes it can serve at `GET /v1/models`.

### 2. Point AETHER at it

```toml
[inference]
provider = "omp"
model = "anthropic/claude-fable-5"          # any omp route the gateway can serve
# endpoint = "http://127.0.0.1:4000/v1"     # default; change if you bind elsewhere
# api_key_env = "OMP_GATEWAY_TOKEN"         # default; falls back to ~/.omp/auth-gateway.token
thinking = "medium"                         # forwarded as reasoning_effort (minimal|low|medium|high|xhigh|max)
concurrency = 4

[sir_quality]
triage_provider = "omp"
triage_model = "openai-codex/gpt-5.6-sol"
deep_provider = "omp"
deep_model = "anthropic/claude-fable-5"
```

- The bearer token is read from `OMP_GATEWAY_TOKEN`, then from `$HOME/.omp/auth-gateway.token`
  (or the path in `OMP_GATEWAY_TOKEN_FILE`). Missing both is a configuration error naming the
  command that creates one.
- `model` must be a route (`provider/model`); a bare model name is a validation warning and a
  load error, because the gateway needs the provider namespace to pick a credential.
- `omp` is also accepted as `[inference.tiered].primary`.

### 3. Opt into batch pricing where the provider offers it

```toml
[batch]
provider = "auto"                           # follow the omp route in [inference].model
passes = ["scan", "triage"]
# Models may be written as omp routes; the provider prefix is stripped on submit.
# When a pass has no batch model, the bare model of [inference].model is used.
deep_model = "anthropic/claude-fable-5"
```

With `provider = "auto"`:

- `anthropic/...` submits to the Anthropic Message Batches API,
- `openai/...` to the OpenAI Batch API,
- `google/...` (or `gemini/...`) to Gemini Batch Mode,
- any other route (`openai-codex/...`, `opencode-go/...`, `nanogpt/...`, `ollama/...`) fails
  with an explicit error listing the providers that do offer batch pricing. `aetherd status`
  reports the same as a `batch_provider_auto_no_batch_pricing` warning.

`openai-codex` deliberately does **not** map to the OpenAI batch API: the Codex route is a
ChatGPT subscription login, while the batch API bills an API key, so they are different routes
even when the model name matches. Set `provider = "openai"` explicitly (and `OPENAI_API_KEY`)
to batch those models on API pricing.

`--provider` on `aetherd batch build|ingest|run` overrides the config, and `auto` is valid there
too.

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
- `OMP_GATEWAY_TOKEN` (or `~/.omp/auth-gateway.token`) for `omp`.
- `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY` for batch pricing on the matching batch provider.

No API key is required for `qwen3_local`.

## CLI Overrides

`aetherd` can override config values at runtime:

```bash
--inference-provider <auto|tiered|gemini|qwen3_local|openai_compat|omp|mock>
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
