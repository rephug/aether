# Phase 8.16 — Embedding Refresh Pipeline — Session Context

**Date:** 2026-03-11
**Branch:** `feature/phase8-stage8-16-embedding-refresh` (to be created)
**Worktree:** `/home/rephu/projects/aether-phase8-embedding-refresh` (to be created)
**Starting commit:** HEAD of main (after 8.15 merge)

## CRITICAL: Read actual source, not this document

```bash
/home/rephu/projects/aether

# Search for existing embedding pipeline code
grep -rn "refresh_embedding_if_needed" crates/
grep -rn "EmbeddingProviderKind" crates/
grep -rn "extract_embedding_vector" crates/
grep -rn "embeddings-only\|embeddings_only" crates/

# Find the OpenAI-compat inference provider (Stage 7.8) as reference
grep -rn "OpenAiCompat\|openai_compat" crates/aether-infer/
```

## Build environment (MUST be set for ALL cargo commands)

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

**Never run `cargo test --workspace`** — OOM risk. Always per-crate.

## What just merged

- **Phase 8.15** — TYPE_REF + IMPLEMENTS edge extraction (1026 type_ref,
  12 implements edges for aether-store)
- **Phase 8.14** — Component-bounded semantic rescue
- **Phase 8.13** — Symbol reconciliation

## The problem being solved

Two gaps block rapid embedding model A/B testing:

1. **No standalone re-embed path.** Changing the embedding model in config
   requires `--index-once --full --force`, which regenerates ALL SIRs via
   Gemini flash-lite (~15-30 min, real API credits) just to trigger
   embedding refresh — even though SIRs are unchanged.

2. **No cloud embedding API support.** `EmbeddingProviderKind` only supports
   `Qwen3Local` (Ollama), `Candle` (local), and `Mock`. Cloud APIs like
   Gemini Embedding 2 use OpenAI-format endpoints that don't exist yet.

### Why this matters now

We want to compare embedding models to find the production default:
- qwen3-embedding:8b (current ablation baseline, 4096-dim)
- qwen3-embedding:4b (2560-dim, less VRAM)
- gemini-embedding-2-preview (3072-dim, CODE_RETRIEVAL task type)

Without these fixes, each comparison costs ~45 minutes and real money.
With them, each takes ~10 minutes.

## What to implement

### Part 1: --embeddings-only flag
- Add CLI flag to aetherd
- When set: iterate all symbols with existing SIR, call embedding
  refresh for each, skip SIR generation entirely
- Respect existing skip logic (matching provider/model/sir_hash)
- ~50-80 lines

### Part 2: OpenAI-compatible embedding provider
- Add `OpenAiCompat` variant to `EmbeddingProviderKind`
- Create provider that sends OpenAI-format requests with Bearer auth
- Reuse existing `extract_embedding_vector` response parser
- Wire into config: `provider = "openai_compat"` with endpoint, model,
  api_key_env fields
- Optional: task_type, dimensions fields for Gemini/OpenAI
- ~80-120 lines

## Key files to inspect

```
# CLI and indexing pipeline
crates/aetherd/src/main.rs (or cli.rs)
crates/aetherd/src/sir_pipeline.rs
crates/aetherd/src/indexer.rs

# Embedding providers
crates/aether-infer/src/embedding/mod.rs
crates/aether-infer/src/lib.rs

# Config
crates/aether-config/src/lib.rs

# Reference: OpenAI-compat inference provider (Stage 7.8)
grep -rn "OpenAiCompat" crates/aether-infer/
```

## Scope guard (must NOT be modified)

- SIR generation pipeline logic
- VectorStore trait or implementations
- Community detection or health scoring
- Existing Qwen3Local or Candle providers
- Search/retrieval path
- LanceDB or SQLite storage backends

## Acceptance criteria

- `--embeddings-only` re-embeds all symbols without SIR generation
- `--embeddings-only` skips symbols whose embedding already matches
- OpenAI-compat provider sends correct request format
- OpenAI-compat provider parses standard response
- Existing providers (qwen3_local, candle, mock) unchanged
- All tests pass, zero clippy warnings

## End-of-stage git sequence

```bash
cd /home/rephu/projects/aether-phase8-embedding-refresh
git push origin feature/phase8-stage8-16-embedding-refresh

# Create PR via GitHub web UI, then after merge:
cd /home/rephu/projects/aether
git switch main
git pull --ff-only
git worktree remove ../aether-phase8-embedding-refresh
git branch -D feature/phase8-stage8-16-embedding-refresh
git worktree prune
```
