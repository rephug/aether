# Phase 8.17 — Gemini Native Embedding Provider — Session Context

**Date:** 2026-03-11
**Branch:** `feature/phase8-stage8-17-gemini-native` (to be created)
**Worktree:** `/home/rephu/projects/aether-phase8-gemini-native` (to be created)
**Starting commit:** HEAD of main (after 8.16 merge)

## CRITICAL: Read actual source, not this document

```bash
/home/rephu/projects/aether

# Find the EmbeddingProvider trait
grep -rn "trait.*EmbeddingProvider\|fn embed" crates/aether-infer/src/embedding/
grep -rn "EmbeddingProvider" crates/aether-infer/src/lib.rs

# Find all callers of embed()
grep -rn "\.embed(" crates/aetherd/
grep -rn "\.embed(" crates/aether-mcp/

# Find the existing OpenAI-compat embedding provider as reference
cat crates/aether-infer/src/embedding/openai_compat.rs

# Find EmbeddingProviderKind enum
grep -rn "enum EmbeddingProviderKind" crates/

# Find config loading
grep -rn "load_embedding_provider_from_config" crates/aether-infer/
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

- **Phase 8.15** — TYPE_REF + IMPLEMENTS edge extraction
- **Phase 8.16** — --embeddings-only flag + OpenAI-compat embedding provider
- **Decision #83** — Gemini Embedding 2 locked as production model at threshold 0.90

## The problem being solved

The OpenAI-compat endpoint produces symmetric embeddings — same embedding
function for both indexing and search. Gemini's native API supports
asymmetric embeddings via `taskType`:

- `RETRIEVAL_DOCUMENT` for SIR text being indexed (optimized for "being found")
- `CODE_RETRIEVAL_QUERY` for search queries (optimized for "finding code")

This improves search retrieval precision without any changes to community
detection or the planner pipeline.

## What to implement

1. Add `EmbeddingPurpose` enum (`Document` / `Query`) to the embedding trait
2. Extend `embed()` or add `embed_with_purpose()` to pass purpose through
3. Create `GeminiNativeEmbeddingProvider` that maps purpose to Gemini `taskType`
4. Wire into config as `provider = "gemini_native"`
5. Update callers: indexing passes `Document`, search passes `Query`
6. Existing providers ignore the purpose parameter

## Key differences from OpenAI-compat

| | OpenAI-compat | Gemini native |
|--|--------------|---------------|
| URL | `{endpoint}/embeddings` | `https://...googleapis.com/v1beta/models/{model}:embedContent` |
| Auth | `Authorization: Bearer {key}` | `x-goog-api-key: {key}` |
| Request body | `{"model":..., "input":...}` | `{"content":{"parts":[{"text":...}]}, "taskType":...}` |
| Response | `data[0].embedding` | `embedding.values` |
| Task type | Not supported | `RETRIEVAL_DOCUMENT`, `CODE_RETRIEVAL_QUERY` |

## Scope guard (must NOT be modified)

- Community detection logic (planner_communities.rs)
- Health scoring or ablation
- OpenAI-compat provider (stays as generic option)
- VectorStore trait or implementations
- SIR generation pipeline logic (only the embedding call changes)

## Acceptance criteria

- `gemini_native` provider sends correct native API format
- `x-goog-api-key` auth header (NOT Bearer)
- `RETRIEVAL_DOCUMENT` task type for indexing paths
- `CODE_RETRIEVAL_QUERY` task type for search paths
- Existing providers unchanged
- All tests pass, zero clippy warnings
- Manual validation: `--embeddings-only` works with `gemini_native`

## End-of-stage git sequence

```bash
cd /home/rephu/projects/aether-phase8-gemini-native
git push origin feature/phase8-stage8-17-gemini-native

# Create PR via GitHub web UI, then after merge:
cd /home/rephu/projects/aether
git switch main
git pull --ff-only
git worktree remove ../aether-phase8-gemini-native
git branch -D feature/phase8-stage8-17-gemini-native
git worktree prune
```
