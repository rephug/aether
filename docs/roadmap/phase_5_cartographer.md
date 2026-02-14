# Phase 5: The Cartographer (Expansion & Local Intelligence)

## Purpose
Expand AETHER's map — understand more languages, run search intelligence locally, and make adding future languages mechanical. Phase 5 moves AETHER from Cloud-Only to Hybrid deployment: API for reasoning (SIR generation), local for retrieval (embeddings + reranking).

## Thesis
"AETHER maps new territory — understanding more languages and running without the cloud."

## In scope
- Language plugin abstraction: refactor `aether-parse` so languages are modular and adding new ones is mechanical
- Python language support: full parity with Rust/TypeScript (parsing, symbols, edges, SIR, search)
- Candle local embeddings: Qwen3-Embedding-0.6B in-process, removing cloud dependency for retrieval
- Reranker integration: Qwen3-Reranker-0.6B (local) or Cohere API, improving search beyond RRF fusion
- Adaptive similarity thresholds: per-language tuning of vector search gating

## Out of scope
- Ticket/PR API connectors (Phase 6)
- Event bus refactor (Phase 6 — synchronous pipeline still sufficient)
- Team collaboration / `aether sync` (deferred indefinitely — no users yet)
- Additional languages beyond Python (Phase 6+ — plugin abstraction makes this mechanical)
- Full Local deployment (Ollama for inference — Phase 6+)
- Enterprise connectors (LDAP/SSO, audit logging)

## Pass criteria
1. A `LanguageConfig` struct + optional `LanguageHooks` trait drives all language-specific behavior.
2. Rust and TypeScript are served by the abstraction with identical output to pre-refactor.
3. Python files are parsed, symbols extracted, edges extracted, SIR generated, and searchable.
4. `[embeddings] provider = "candle"` runs Qwen3-Embedding-0.6B locally with no API calls.
5. Reranker improves search precision on a benchmark fixture vs. RRF-only baseline.
6. Adaptive thresholds produce different similarity cutoffs per language.
7. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Stages

| Stage | Name | Description |
|-------|------|-------------|
| 5.1 | Language Plugin Abstraction | Refactor `aether-parse` into modular per-language config + hooks |
| 5.2 | Python Language Support | Full Python parsing, symbols, edges, SIR, search |
| 5.3 | Candle Local Embeddings | In-process Qwen3-Embedding-0.6B via Candle runtime |
| 5.4 | Reranker Integration | Optional reranking stage in search pipeline |
| 5.5 | Adaptive Similarity Thresholds | Per-language vector search gating |

### Dependency chain
```
5.1 language plugin ──► 5.2 Python ───────────────────┐
5.3 Candle embeddings ──► 5.4 reranker ──► 5.5 thresholds ──┤
                                                       ▼
                                              Phase 5 complete
```

5.1 and 5.3 are independent and can run in parallel.
5.2 requires 5.1 (needs plugin abstraction).
5.4 requires 5.3 (reuses Candle runtime).
5.5 requires 5.3 (needs local embeddings for offline threshold tuning).

### Recommended execution order
```
Week 1:  5.1 language plugin + 5.3 Candle embeddings (parallel)
Week 2:  5.2 Python support
Week 3:  5.4 reranker
Week 4:  5.5 adaptive thresholds
```

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase-5-cartographer-rollup off main.
3) Create worktree ../aether-phase-5-cartographer for that branch and switch into it.
4) Implement Phase 5 by completing stage docs in this order:
   - docs/roadmap/phase_5_stage_5_1_language_plugin.md
   - docs/roadmap/phase_5_stage_5_2_python_support.md
   - docs/roadmap/phase_5_stage_5_3_candle_embeddings.md
   - docs/roadmap/phase_5_stage_5_4_reranker.md
   - docs/roadmap/phase_5_stage_5_5_adaptive_thresholds.md
5) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
6) Commit with message: "Complete Phase 5 Cartographer rollout".
```
