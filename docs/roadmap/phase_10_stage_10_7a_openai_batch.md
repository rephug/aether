# Phase 10.7 — Multi-Provider Batch Pipeline (OpenAI)

## Stage 10.7a — OpenAI Batch API Integration

### Purpose

Add OpenAI as a second batch processing provider alongside the existing Gemini Batch API. This enables users to run nightly batch indexing using GPT-5.4 nano ($0.20/1M input) for scan/triage and GPT-5.4 mini ($0.75/1M input) or GPT-5.4 ($2.50/1M input) for deep passes. The OpenAI Batch API offers 50% discount over real-time pricing, matching Gemini's batch economics.

### Prerequisites

- Stage 10.1 merged (batch pipeline, JSONL generation, prompt hashing)
- `openai_compat` provider already handles real-time OpenAI API calls
- OPENAI_API_KEY set in environment

### What Problem This Solves

Currently, batch indexing is Gemini-only. The JSONL format, submission script, result polling, and ingestion are all hardcoded to Gemini Batch API conventions. Users who prefer OpenAI models (or want to compare SIR quality across providers) can't use batch mode — they're limited to real-time `openai_compat` calls, which are 2x the price and much slower for large codebases.

GPT-5.4 nano at batch pricing ($0.10/1M input, $0.625/1M output after 50% discount) would be the cheapest batch option available — cheaper than Gemini Flash Lite batch.

### Architecture

The current batch pipeline has this flow:

```
batch build → JSONL (Gemini format) → shell script → Gemini Batch API → poll → download → batch ingest
```

The new architecture abstracts the provider-specific parts:

```
batch build --provider openai → JSONL (OpenAI format) → Rust HTTP submission → OpenAI Batch API → poll → download → batch ingest
```

Key changes:
1. **JSONL format abstraction** — `batch build` generates provider-appropriate JSONL
2. **Native Rust submission** — replace the shell script bridge with a Rust HTTP client (reqwest) that handles both Gemini and OpenAI submission/polling
3. **Result format normalization** — OpenAI returns `choices[0].message.content`, Gemini returns `candidates[0].content.parts[0].text`. Normalize before ingest.

### In scope

#### 1. OpenAI JSONL format

OpenAI Batch API expects JSONL lines like:
```json
{
  "custom_id": "symbol_id|prompt_hash",
  "method": "POST",
  "url": "/v1/chat/completions",
  "body": {
    "model": "gpt-5.4-nano",
    "messages": [
      {"role": "system", "content": "You are a code analysis assistant..."},
      {"role": "user", "content": "<prompt>"}
    ],
    "temperature": 0.0,
    "response_format": {"type": "json_object"}
  }
}
```

The existing `build_pass_jsonl_for_ids` function generates Gemini-format JSONL. Refactor to accept a `BatchProvider` enum and format accordingly.

#### 2. Batch provider config

```toml
[batch]
provider = "gemini"            # "gemini" | "openai" — NEW field, default "gemini"
scan_model = "gpt-5.4-nano"   # When provider = "openai"
triage_model = "gpt-5.4-nano"
deep_model = "gpt-5.4-mini"   # or "gpt-5.4" for max quality
```

The `provider` field determines JSONL format, submission endpoint, and result parsing.

#### 3. Native Rust batch submission (replaces shell script for OpenAI)

OpenAI Batch API flow:
1. Upload JSONL file via `POST /v1/files` (purpose: "batch")
2. Create batch via `POST /v1/batches` with input_file_id and endpoint
3. Poll `GET /v1/batches/{batch_id}` until status is "completed" or "failed"
4. Download results via `GET /v1/files/{output_file_id}/content`

Implement in a new `crates/aetherd/src/batch/openai_submit.rs` module. Keep the Gemini shell script as-is (working, tested) — add OpenAI as a parallel path.

#### 4. Result normalization

OpenAI batch results return:
```json
{
  "id": "batch_req_...",
  "custom_id": "symbol_id|prompt_hash",
  "response": {
    "status_code": 200,
    "body": {
      "choices": [{"message": {"content": "{...SIR JSON...}"}}]
    }
  }
}
```

The existing `batch ingest` expects Gemini format. Add a normalization step that converts OpenAI results to the common internal format before ingestion.

#### 5. CLI changes

```bash
# Build JSONL for OpenAI
aetherd --workspace . batch build --pass scan --provider openai

# Full pipeline with OpenAI
aetherd --workspace . batch run --passes scan,triage,deep --provider openai

# Provider from config (default if --provider not specified)
aetherd --workspace . batch run --passes scan,triage,deep
```

Add `--provider` flag to `batch build` and `batch run`. Falls back to `[batch].provider` config, then to "gemini" default.

#### 6. Thinking/reasoning support

GPT-5.4 models support reasoning effort: `none`, `low`, `medium`, `high`, `xhigh`. Map the existing `scan_thinking`/`triage_thinking`/`deep_thinking` config values to OpenAI's format:

```json
"reasoning": {"effort": "low"}
```

For nano: default to `none` (fastest, cheapest). For mini: default to `low`. For 5.4: default to `medium`.

### Out of scope

- Migrating Gemini batch away from shell script (it works, leave it)
- OpenAI real-time provider changes (already works via `openai_compat`)
- OpenAI embedding batch (separate concern)
- Flex processing tier (OpenAI's non-batch async option)

### Pass criteria

1. `aetherd batch build --pass scan --provider openai` generates valid OpenAI JSONL
2. `aetherd batch run --passes scan --provider openai` submits, polls, downloads, and ingests results
3. Prompt hashing works identically (BLAKE3 skip-unchanged)
4. Ingested SIRs are identical quality regardless of provider (same prompt text, different wrapper format)
5. `cargo test -p aetherd` passes
6. Existing Gemini batch pipeline is unaffected

### Estimated effort: 1-2 Claude Code runs

### New config

```toml
[batch]
provider = "openai"                    # NEW — "gemini" | "openai"
scan_model = "gpt-5.4-nano"
triage_model = "gpt-5.4-nano"
deep_model = "gpt-5.4-mini"
scan_thinking = "none"                 # Maps to reasoning.effort for OpenAI
triage_thinking = "low"
deep_thinking = "medium"
```

### Files to create/modify

| File | Action |
|------|--------|
| `crates/aetherd/src/batch/openai_submit.rs` | NEW — OpenAI file upload, batch create, poll, download |
| `crates/aetherd/src/batch/build.rs` | MODIFY — Abstract JSONL format by provider |
| `crates/aetherd/src/batch/ingest.rs` | MODIFY — Normalize OpenAI results before parsing |
| `crates/aetherd/src/batch/run.rs` | MODIFY — Route to OpenAI or Gemini submission |
| `crates/aetherd/src/batch/mod.rs` | MODIFY — Add BatchProvider enum, expose openai module |
| `crates/aetherd/src/cli.rs` | MODIFY — Add --provider flag to batch subcommands |
| `crates/aether-config/src/batch.rs` | MODIFY — Add provider field |
