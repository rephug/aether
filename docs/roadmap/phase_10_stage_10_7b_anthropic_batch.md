# Phase 10.7 — Multi-Provider Batch Pipeline (Anthropic)

## Stage 10.7b — Anthropic Message Batches API Integration

### Purpose

Add Anthropic as a third batch processing provider alongside Gemini and OpenAI. The Anthropic Message Batches API offers 50% cost reduction on all Claude models, processes up to 10,000 requests per batch, and completes within 24 hours. This prepares AETHER to use future Claude models (successors to Haiku) for batch SIR generation as soon as they're released.

### Prerequisites

- Stage 10.1 merged (batch pipeline, JSONL generation, prompt hashing)
- Stage 10.7a recommended (establishes the multi-provider abstraction)
- ANTHROPIC_API_KEY set in environment

### What Problem This Solves

When Anthropic releases next-generation lightweight models (successors to Claude 3.5 Haiku), AETHER users will want to use them for batch SIR generation. Having the batch pipeline ready means switching is a config change, not a code change. The Anthropic batch API is already mature (50% discount, up to 10K requests, 24-hour processing), and the Message Batches API is simpler than both Gemini and OpenAI's batch flows.

Current Claude models for batch consideration:
- **Claude Haiku 4.5** (`claude-haiku-4-5-20251001`) — fastest/cheapest current option, good for scan/triage
- **Claude Sonnet 4.6** (`claude-sonnet-4-6`) — strong reasoning, good for deep pass
- Future lightweight models — the primary motivation for having this ready

### Architecture

The Anthropic Message Batches API has a different (simpler) architecture than Gemini/OpenAI:

```
batch build --provider anthropic
    → JSON request body (NOT JSONL file upload)
    → POST /v1/messages/batches (inline requests array)
    → poll GET /v1/messages/batches/{id} until processing_status == "ended"
    → stream results from results_url
    → batch ingest
```

Key difference: Anthropic takes the requests array inline in the POST body (up to 10K requests), not as an uploaded JSONL file. Results are streamed from a `results_url` as JSONL.

### In scope

#### 1. Anthropic batch request format

Anthropic Message Batches API expects:
```json
{
  "requests": [
    {
      "custom_id": "symbol_id|prompt_hash",
      "params": {
        "model": "claude-haiku-4-5-20251001",
        "max_tokens": 4096,
        "messages": [
          {"role": "user", "content": "<prompt>"}
        ]
      }
    }
  ]
}
```

This is NOT a JSONL file — it's a single JSON body with a `requests` array. For large codebases (>10K symbols), split into multiple batches.

The `batch build` command for Anthropic should generate a JSON file (not JSONL) containing the requests array.

#### 2. Batch provider config

```toml
[batch]
provider = "anthropic"               # "gemini" | "openai" | "anthropic"
scan_model = "claude-haiku-4-5-20251001"
triage_model = "claude-haiku-4-5-20251001"
deep_model = "claude-sonnet-4-6"
```

#### 3. Native Rust batch submission

Anthropic Message Batches API flow:
1. POST `/v1/messages/batches` with inline `requests` array (max 10K)
2. Returns batch ID and `processing_status: "in_progress"`
3. Poll `GET /v1/messages/batches/{batch_id}` until `processing_status: "ended"`
4. Stream results from `results_url` — returns JSONL with one result per line

Implement in `crates/aetherd/src/batch/anthropic_submit.rs`.

Headers required:
```
x-api-key: $ANTHROPIC_API_KEY
anthropic-version: 2023-06-01
content-type: application/json
```

#### 4. Result normalization

Anthropic batch results (streamed JSONL from results_url):
```json
{
  "custom_id": "symbol_id|prompt_hash",
  "result": {
    "type": "succeeded",
    "message": {
      "content": [{"type": "text", "text": "{...SIR JSON...}"}],
      "model": "claude-haiku-4-5-20251001",
      "role": "assistant",
      "stop_reason": "end_turn"
    }
  }
}
```

Or on error:
```json
{
  "custom_id": "symbol_id|prompt_hash",
  "result": {
    "type": "errored",
    "error": {"type": "invalid_request_error", "message": "..."}
  }
}
```

Normalize to the common internal format: extract `content[0].text` for succeeded, mark errored results as failed in ingest.

#### 5. CLI changes

```bash
# Build requests for Anthropic
aetherd --workspace . batch build --pass scan --provider anthropic

# Full pipeline
aetherd --workspace . batch run --passes scan,triage,deep --provider anthropic
```

Same `--provider` flag as 10.7a. Falls back to `[batch].provider` config.

#### 6. Extended thinking support

Claude models support extended thinking via the `thinking` parameter:
```json
{
  "model": "claude-sonnet-4-6",
  "max_tokens": 16384,
  "thinking": {
    "type": "enabled",
    "budget_tokens": 8192
  },
  "messages": [...]
}
```

Map the existing `deep_thinking` config to budget levels:
- `none` → omit thinking parameter entirely
- `low` → `budget_tokens: 2048`
- `medium` → `budget_tokens: 4096`
- `high` → `budget_tokens: 8192`

Only include thinking for deep pass. Scan and triage should always omit it for speed/cost.

#### 7. Chunking for large codebases

Anthropic limits to 10,000 requests per batch. For codebases with >10K symbols, split into multiple batches. The existing `jsonl_chunk_size` config can be reused for this purpose.

### Out of scope

- Anthropic real-time provider (could be added separately via the existing openai_compat or a new dedicated provider)
- Anthropic embedding models (they don't offer embedding endpoints currently)
- Prompt caching (Anthropic supports it but it's a real-time optimization, not batch)

### Pass criteria

1. `aetherd batch build --pass scan --provider anthropic` generates valid Anthropic batch request JSON
2. `aetherd batch run --passes scan --provider anthropic` submits, polls, downloads, and ingests results
3. Prompt hashing works identically (BLAKE3 skip-unchanged)
4. Extended thinking activates only for deep pass when configured
5. Large batches automatically chunk at 10K request boundary
6. `cargo test -p aetherd` passes
7. Existing Gemini and OpenAI batch pipelines are unaffected

### Estimated effort: 1-2 Claude Code runs

### New config additions

```toml
[batch]
provider = "anthropic"
scan_model = "claude-haiku-4-5-20251001"
triage_model = "claude-haiku-4-5-20251001"
deep_model = "claude-sonnet-4-6"
scan_thinking = "none"
triage_thinking = "none"
deep_thinking = "medium"           # Maps to thinking.budget_tokens
anthropic_max_tokens = 4096        # max_tokens for Anthropic responses
```

### Files to create/modify

| File | Action |
|------|--------|
| `crates/aetherd/src/batch/anthropic_submit.rs` | NEW — Anthropic batch create, poll, stream results |
| `crates/aetherd/src/batch/build.rs` | MODIFY — Add Anthropic JSON format (not JSONL) |
| `crates/aetherd/src/batch/ingest.rs` | MODIFY — Normalize Anthropic result format |
| `crates/aetherd/src/batch/run.rs` | MODIFY — Route to Anthropic submission path |
| `crates/aetherd/src/batch/mod.rs` | MODIFY — Add Anthropic variant to BatchProvider enum |
| `crates/aetherd/src/cli.rs` | MODIFY — If not done in 10.7a, add --provider flag |
| `crates/aether-config/src/batch.rs` | MODIFY — Add anthropic-specific config fields |

### Pricing comparison (as of March 2026)

| Provider | Model | Input/1M | Output/1M | Batch discount | Effective input |
|----------|-------|----------|-----------|----------------|----------------|
| Google | Gemini 3.1 Flash Lite | ~$0.075 | ~$0.30 | 50% | ~$0.038 |
| OpenAI | GPT-5.4 nano | $0.20 | $1.25 | 50% | $0.10 |
| OpenAI | GPT-5.4 mini | $0.75 | $4.50 | 50% | $0.375 |
| Anthropic | Claude Haiku 4.5 | $0.80 | $4.00 | 50% | $0.40 |
| Anthropic | Claude Sonnet 4.6 | $3.00 | $15.00 | 50% | $1.50 |

Recommended batch config for cost-conscious users: Gemini Flash Lite for scan/triage, GPT-5.4 nano as alternative, Claude Sonnet for deep pass quality.

### Implementation note

If 10.7a is implemented first and establishes a `BatchProvider` enum + provider abstraction, 10.7b becomes much simpler — just implement the Anthropic-specific submission/result modules and plug into the existing framework. If 10.7b is implemented standalone, it needs to create the provider abstraction itself.

Recommended order: 10.7a first (establishes the pattern), 10.7b second (adds Anthropic), then a combined stage to refactor the Gemini shell script into native Rust for consistency (optional, low priority — the script works).
