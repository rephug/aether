# Configuration — Phase 1

AETHER reads config from: `./.aether/config.toml`

It also uses environment variables for provider credentials.

---

## Environment variables

- `GEMINI_API_KEY` — required for default SIR generation (Day 1 cloud path)

Optional (future / alternate providers):
- `ANTHROPIC_API_KEY`
- `OPENAI_API_KEY`
- `VOYAGE_API_KEY`
- `COHERE_API_KEY`

---

## Default config.toml (recommended)

```toml
[general]
mode = "daemon"
project_name = "auto"
log_level = "info"
data_dir = ".aether"

[parser]
debounce_ms = 300
max_file_size = 1000000
extensions = ["ts", "tsx", "js", "jsx", "rs"]

[inference]
provider = "gemini"
model = "gemini-3-flash"
api_key_env = "GEMINI_API_KEY"
max_requests_per_minute = 60
max_tokens_per_day = 2_000_000
retry_max = 3
retry_backoff_ms = 500

[sir]
schema_version = 1
max_symbol_chars = 20_000
canonicalize_before_embed = true

[embeddings]
enabled = true
provider = "gemini"         # Day 1 default: API embeddings
model = "gemini-embedding-001"
output_dimensionality = 1536
batch_size = 64

[search]
lexical_enabled = true
semantic_enabled = true
top_k = 20

[lsp]
enabled = true
listen_addr = "127.0.0.1:9257"
```

---

## Notes on key settings (plain English)

- `debounce_ms`: waits this long after edits stop before processing, to avoid spamming inference.
- `max_file_size`: skip huge/minified files.
- `max_requests_per_minute` / `max_tokens_per_day`: cost controls to prevent runaway spend.
- `max_symbol_chars`: caps how much code is sent for one symbol to the model.
- `output_dimensionality`: lets you shrink embeddings (cheaper storage, faster search).
