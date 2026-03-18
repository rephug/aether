# Phase 10.7 Session Context — Multi-Provider Batch Pipeline

**Date:** 2026-03-18
**Repo:** github.com/rephug/aether (private), commit ef90e95
**Schema version:** 13
**Last merged PR:** #109 (Phase 10.5 Part B)
**LOC:** ~136K across 17 crates

## What we're building

Three-stage batch pipeline expansion:
- **10.7-prep:** Expanded SIR system prompts (3-tier: compact/standard/full) + system/user split + local model context window adjustment
- **10.7a:** `BatchProvider` trait + Gemini native Rust (replaces shell script) + OpenAI batch + provider-scoped config
- **10.7b:** Anthropic batch provider with prompt caching (`cache_control`) + auto-chunking

## Key file locations

### Batch pipeline (all in `crates/aetherd/src/batch/`)
| File | Lines | Role |
|------|-------|------|
| `mod.rs` | 210 | `PassConfig`, `BatchRuntimeConfig`, resolution logic |
| `build.rs` | 449 | JSONL generation, prompt hash check, neighbor context prefetch |
| `run.rs` | 221 | Orchestration, shell script invocation, seismograph hook |
| `ingest.rs` | 334 | Gemini result parsing, SIR upsert, fingerprint writing |
| `hash.rs` | 116 | BLAKE3 prompt hash computation and diffing |
| `extract.rs` | 24 | Tree-sitter symbol extraction wrapper |

### Prompt construction
| File | Role |
|------|------|
| `crates/aether-infer/src/sir_prompt.rs` (445 lines) | `build_sir_prompt_for_kind()`, `build_enriched_sir_prompt()`, `build_enriched_sir_prompt_with_cot()`, `SirEnrichmentContext` struct |
| `crates/aether-infer/src/http.rs` (434 lines) | `build_ollama_generate_body()` (num_ctx=4096), `build_ollama_deep_generate_body()` (num_ctx=8192) |

### Config
| File | Role |
|------|------|
| `crates/aether-config/src/batch.rs` (104 lines) | `BatchConfig` struct — flat fields only, no provider, no prompt_tier |
| `crates/aether-config/src/root.rs` | `AetherConfig` with `pub batch: Option<BatchConfig>` |
| `crates/aether-config/src/inference.rs` | `InferenceProviderKind` enum |

### Shell script
| File | Role |
|------|------|
| `scripts/gemini_batch_submit.sh` (139 lines) | Gemini resumable upload → batch create → poll → download |

### CLI
| File | Role |
|------|------|
| `crates/aetherd/src/cli.rs` | `BatchBuildArgs`, `BatchRunArgs`, `BatchIngestArgs` — no `--provider` flag |

## Critical facts from source inspection

1. **Prompt is a single combined string.** `build_sir_prompt_for_kind()` returns one `String`. No system/user split exists. The real-time Gemini provider (`providers/gemini.rs:86`) calls the same function and puts everything in one `text` field.

2. **`thinking_level()` has no "off"/"none".** Only handles low/medium/high/dynamic. Needs "off"/"none" added.

3. **`ingest.rs` hardcodes Gemini twice.** Line 72: `provider: Some(InferenceProviderKind::Gemini)` when creating `SirPipeline`. Line 145: `provider_name: InferenceProviderKind::Gemini.as_str()` in `UpsertSirIntentPayload`.

4. **Shell script uses resumable upload.** Two-step: POST to get upload URL from response header `x-goog-upload-url`, then PUT file data to that URL. Not a simple file upload.

5. **`run.rs` is synchronous.** All batch functions use `fn`, not `async fn`. The trait has async methods. Need tokio runtime block (follow pattern from `main.rs` lines 232-244, 301-306).

6. **`config_fingerprint()` is `"{model}:{thinking}:{max_chars}"`.** Does not include provider name. Should be updated to include provider to prevent false hash matches when switching providers.

7. **No batch tests exist.** Only `hash.rs` has tests. `build.rs`, `run.rs`, `ingest.rs` have zero tests.

8. **Ollama context windows are hardcoded.** `num_ctx: 4096` for scan/triage, `num_ctx: 8192` for deep. These are in `http.rs` lines 146 and 169. The expanded prompt (4108 tokens) exceeds the scan/triage context window entirely.

9. **Schema version is 13.** Dashboard checks `check_compatibility("core", 13)`. MCP checks the same. No schema migration needed for these stages.

10. **`batch` config is `Option<BatchConfig>`.** Entire section can be absent. All resolution functions handle this with defaults.

## Locked decisions for this work

| # | Decision |
|---|----------|
| 103 | `BatchProvider` trait as batch pipeline abstraction (not enum) |
| 104 | Native Rust replaces shell script for all batch submission |
| 105 | Provider-scoped batch config subsections (`[batch.gemini]`, `[batch.openai]`, `[batch.anthropic]`) |
| 106 | Anthropic batch uses inline JSON with auto-chunking for 10K/32MB limits |
| 107 | Prompt caching enabled by default for Anthropic batch via `cache_control` |
| 108 | Three-tier prompt system (compact/standard/full) with auto-selection based on provider |

## Prompt caching thresholds (verified 2026-03-18)

| Provider | Model | Min tokens |
|----------|-------|-----------|
| OpenAI | All | 1024 |
| Gemini | Flash | ~1028 |
| Gemini | Pro | ~2048 |
| Anthropic | Sonnet 4.5 | 1024 |
| Anthropic | Sonnet 4.6 | 2048 |
| Anthropic | Haiku 4.5 | 4096 |
| Anthropic | Opus 4.6 | 4096 |

Scan system prompt at Tier 3 (full): ~4108 tokens → clears all thresholds.
Enriched system prompt at Tier 3 (full): ~4206 tokens → clears all thresholds.

## What NOT to do

- Do NOT run `cargo test --workspace` (OOM on WSL2). Per-crate only.
- Do NOT modify `aether-infer/src/providers/` (real-time inference path is separate from batch).
- Do NOT delete `scripts/gemini_batch_submit.sh` (deprecate only).
- Do NOT change existing `build_sir_prompt_for_kind()` signature or behavior (backward compat).
- Do NOT bump schema version (no SQLite changes needed).
- Do NOT reference xAI, Grok, or x.ai in any docs.

## Build environment (required for all Claude Code sessions)

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

## Git workflow

- Worktrees at `/home/rephu/<branch-name>` (NOT inside `/home/rephu/projects/`)
- Main repo at `/home/rephu/projects/aether`
- `git worktree add -B <branch> /home/rephu/<branch>`
- PRs via GitHub web UI, squash merge
- After merge: `git switch main && git pull --ff-only && git worktree remove /home/rephu/<branch> && git branch -D <branch>`
