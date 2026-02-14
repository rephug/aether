# Decision Register v3 — Phase 5 Build Packet

Extends the Phase 4 Decision Register. These decisions are "locked" for Phase 5 to reduce scope churn and help Codex implement confidently.

---

## Inherited decisions (unchanged from Phase 4)

| # | Decision | Status |
|---|----------|--------|
| 1 | Phase 1 focus = Observer | ✅ Complete |
| 2 | Local-first architecture (.aether/) | ✅ Active |
| 3 | LSP-first integration | ✅ Active |
| 4 | Cloud-first inference (Gemini Flash) | ✅ Active |
| 5 | API embeddings by default | ⚠️ Updated (see #25) |
| 6 | Reranking not enabled by default | ⚠️ Updated (see #26) |
| 7 | tree-sitter for symbol extraction | ✅ Active |
| 8 | Stable BLAKE3 symbol IDs | ✅ Active |
| 9 | Incremental updates only | ✅ Active |
| 10 | SIR stored as JSON blobs + SQLite metadata | ✅ Active |
| 11 | Vector storage → LanceDB | ✅ Active |
| 12 | Graph storage → CozoDB | ✅ Active |
| 13 | Linux primary | ✅ Active |
| 14 | Windows supported, no Ghost sandbox | ✅ Active |
| 16 | Strict SIR schema validation | ✅ Active |
| 17 | Cost and rate limiting mandatory | ✅ Active |
| 18 | Structured logging via `tracing` | ✅ Active |
| 19 | Native git via `gix` | ✅ Active |
| 20 | Dependency edges extracted from AST | ✅ Active |
| 21 | SIR hierarchy levels (leaf, file, module) | ✅ Active |
| 22 | Trait-based backend abstraction | ✅ Active |
| 23 | CozoDB replaces KuzuDB | ✅ Active |

---

## Updated decisions (changed for Phase 5)

### 5. Embeddings provider → Configurable with Candle local option (updated)
**Original:** "API embeddings by default (Gemini embedding API or alternative provider behind a trait)."
**Phase 5 update:** Local embeddings via Candle (Qwen3-Embedding-0.6B) added as an alternative. Default remains `gemini` (cloud). The `EmbeddingProvider` trait now has three implementations: Gemini (API), Candle (local), Mock (test).
- Config: `[embeddings] provider = "gemini" | "candle" | "mock"` (default: `gemini`)
- Model: Qwen3-Embedding-0.6B, 1024-dimensional output
- Model storage: `.aether/models/qwen3-embedding-0.6b/`
- Download: automatic on first use via `hf-hub`, or pre-fetch via `aether download-models`
- Loading: lazy (loaded on first embedding request, not at startup)

### 6. Reranking → Optional, off by default, two backends (updated)
**Original:** "Not enabled by default in Phase 1. If added, must be feature-flagged and cost-aware."
**Phase 5 update:** Reranker added as optional post-retrieval stage. Two backends: Candle local (Qwen3-Reranker-0.6B) and Cohere API. Off by default.
- Config: `[search] reranker = "none" | "candle" | "cohere"` (default: `none`)
- Pipeline position: after RRF fusion, before result return
- Rerank window: configurable, default 50 candidates
- Cost-aware: Cohere API calls count against global inference budget (Decision #17)

### 15. Typed event bus → Deferred to Phase 6 (updated)
**Phase 4:** "Deferred to Phase 5 when additional engines require async coordination."
**Phase 5 update:** Synchronous pipeline still sufficient through Phase 5. Language plugins, Candle embeddings, and reranker all work within the existing synchronous flow. Deferred to Phase 6 when ticket connectors and reactive re-indexing may require async coordination.

---

## New decisions (Phase 5)

### 24. Language plugin abstraction (hybrid data + trait)
Languages are defined via a `LanguageConfig` data struct that bundles: language ID, file extensions, tree-sitter grammar, symbol query, edge query, and module markers. An optional `LanguageHooks` trait allows override of behaviors that can't be captured in data (e.g., Python's `__init__.py` module resolution).
- Location: `crates/aether-parse/src/registry.rs`
- Per-language modules: `crates/aether-parse/src/languages/{rust,typescript,python}.rs`
- Query files: `crates/aether-parse/src/queries/{lang}_{symbols,edges}.scm`
- Registry: `LanguageRegistry` maps file extensions → `LanguageConfig`, populated at startup
- All built-in languages are always available (no compile-time or config-time selection)

### 25. Candle for local embeddings
Qwen3-Embedding-0.6B via Candle runtime, in-process with lazy loading. Moves AETHER from Cloud-Only to Hybrid deployment profile.
- Crates: `candle-core`, `candle-nn`, `candle-transformers` (v0.8), `hf-hub`, `tokenizers`
- Architecture: in-process, lazy load on first embedding request
- Output: 1024-dimensional dense vectors
- Storage: `.aether/models/qwen3-embedding-0.6b/` (auto-downloaded from HF Hub)
- Binary impact: ~5-10MB increase; first build adds ~10-15 min compile time
- GPU: CPU-only for now (Metal/CUDA deferred)

### 26. Reranker backends: Candle local + Cohere API
`RerankerProvider` trait with two implementations. Cross-encoder architecture (scores query-document pairs).
- Candle: Qwen3-Reranker-0.6B, same lazy-loading pattern as embeddings
- Cohere: Rerank v2 API, requires API key in `[providers.cohere]`
- Default: off (`reranker = "none"`)
- Rerank window: 50 candidates from RRF, configurable

### 27. Adaptive per-language similarity thresholds
Semantic search uses per-language thresholds instead of a single global threshold.
- Config: `[search.thresholds]` with per-language values
- Calibration: `aether calibrate` command computes thresholds from indexed codebase
- Defaults: rust 0.70, typescript 0.65, python 0.60, global default 0.65
- Precedence: manual config > calibrated > built-in defaults
- Invalidation: thresholds linked to provider/model; provider change triggers warning

### 28. Ticket/PR connectors deferred to Phase 6
External API connectors (GitHub Issues, Linear, Jira) deferred to Phase 6. Phase 5 focuses on language expansion and search intelligence. The connector abstraction (`TicketConnector` trait) will be designed in Phase 6 planning.

### 29. `aether sync` deferred indefinitely
Multi-user collaboration / shared AETHER state has no clear user demand. Deferred until real users provide feedback on collaboration requirements.

---

## Decision principles for Phase 5

1. **Hybrid data + trait:** Language plugins use config-driven data structs with optional trait overrides. Prefer data over code.
2. **Lazy loading:** ML models (embeddings, reranker) are loaded on first use, not at startup. Don't penalize users who don't use local models.
3. **Provider-switchable:** All intelligence backends (embedding, reranking) are config toggles. Switching providers re-indexes as needed.
4. **Backward compatible:** Adding Python doesn't change Rust/TypeScript behavior. Adding local embeddings doesn't break cloud embeddings. All changes are additive.
5. **Scope-strict:** Same as Phase 4 — Codex prompts enumerate exactly which files to modify.
