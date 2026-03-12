# AETHER Showcase: Self-Analyzing Codebase Intelligence

**Date:** March 12, 2026
**Context:** AETHER analyzed and refactored its own codebase, demonstrating
the full intelligence pipeline end-to-end.

---

## The Story

AETHER is a tool that creates persistent semantic intelligence for codebases.
To prove it works, we pointed it at itself — a 55K+ LOC Rust workspace with
15+ crates — and used its own analysis to guide a real refactoring.

### Phase 1: Self-Analysis

AETHER scanned its own codebase and produced:

- **3748 symbols** indexed with 100% SIR coverage
- **3623 triage-enriched SIRs** (flash-lite scan + flash-lite triage with
  enriched cross-symbol context)
- **46 deep SIRs** (Claude Sonnet 4.6 via OpenRouter for highest-priority symbols)
- **1026 TYPE_REF edges** + **12 IMPLEMENTS edges** (structural type connections)
- **1738 CALLS edges** + **23 DEPENDS_ON edges** (call graph)
- **Gemini Embedding 2** (3072-dim, native provider with asymmetric task types)

### Phase 2: Health Assessment

AETHER scored itself:

| Metric | Value |
|--------|-------|
| **Overall health** | 42/100 (Watch) |
| **God Files identified** | 5 files over 1500 lines |
| **Boundary Leakers** | 11/16 crates flagged |
| **Worst file** | aether-store/src/lib.rs — 6817 lines |

Top God Files:
- aether-store/src/lib.rs — 6817 lines
- aether-mcp/src/lib.rs — 4799 lines
- planner_communities.rs — 3390 lines
- aether-config/src/lib.rs — 3173 lines
- aether-infer/src/lib.rs — 2863 lines

### Phase 3: The Experiment — Blind vs AETHER-Informed Refactoring

We gave the same LLM (OpenAI Codex) two identical tasks: propose a
refactoring plan for `aether-store/src/lib.rs` (6817 lines).

**Trial A: Without AETHER (blind)**
- Input: Only the source code
- Time: 9 minutes
- Result: 15 modules, grouped by table names and function prefixes
- Basis: Name similarity and code proximity

**Trial B: With AETHER intelligence**
- Input: Source code + 199 TYPE_REF edges + 1738 CALLS edges +
  11-community Louvain detection + SIR intents + health scores
- Time: 15 minutes
- Result: 13 modules, grouped by proven dependency clusters
- Basis: Structural edge data, community detection, TYPE_REF proof

### Key Differences in the Plans

| Aspect | Blind Plan | AETHER Plan |
|--------|-----------|-------------|
| SIR domain | One big sir.rs | Split into sir_meta.rs + sir_history.rs — AETHER's TYPE_REF edges proved these are distinct clusters |
| Threshold/calibration | Bundled into embeddings.rs | Separate thresholds.rs — AETHER identified as its own community |
| Graph logic | Split across two tiers | Unified graph.rs — AETHER's community detection grouped related symbols |
| Shared helpers | Generic query_utils.rs | Precise lexical.rs — AETHER knew exactly which function was the cross-domain dependency |
| Module count | 15 | 13 (fewer, more cohesive) |
| Layout | 3-tier (api/ + sqlite/ + root) | 2-tier (modules + façade) — simpler |
| Migration order | "By table boundary" | "TYPE_REF-first: extract types before methods that reference them" |
| Size estimates | None | Per-module line estimates from symbol counts |

### Phase 4: Execution and Validation

The AETHER-guided plan was executed:

| Metric | Before | After |
|--------|--------|-------|
| lib.rs lines | 6817 | **751** (façade) |
| Largest module | 6817 | **603** (project_notes.rs) |
| Modules | 1 | **13 + 5 test submodules** |
| aether-store score | 72/100 | **77/100 (+5)** |
| aether-store archetype | God File, Legacy Residue | **Legacy Residue only** |
| aether-store semantic | 45 | **58 (+13)** |
| God File archetype | Present | **Resolved** |
| Public API changes | — | **Zero** |
| Test changes | — | **All pass** |

### Phase 5: Self-Verification

After the refactor, AETHER re-scanned itself and confirmed the improvement
with its own tooling. The tool that diagnosed the problem also validated
the fix. This closed-loop self-analysis is unique to AETHER.

---

## Technical Achievements (Phase 8 Pipeline)

### Community Detection Pipeline

AETHER's split planner uses Louvain community detection on a structural
graph enriched with multiple edge types and rescue heuristics:

1. Test filtering (before graph construction)
2. Type-anchor rescue (hard constraint: struct + impl methods)
3. Container/locality rescue (qualified-name stem)
4. Selective semantic rescue (component-bounded, threshold 0.90)
5. Connected components with loner exclusion
6. Per-component Louvain (γ=0.5)
7. Component-bounded merge with semantic centroid fallback
8. Stability check (co-membership Jaccard, perturbed parameters)
9. Data-driven confidence scoring

**Result on aether-store:** 11 communities, 131 largest, 3 loners,
0.93 confidence, 0.82 stability.

### Three-Pass SIR Pipeline

| Pass | Model | Purpose | Symbols |
|------|-------|---------|---------|
| Scan | gemini-3.1-flash-lite | Fast SIR, no enrichment | 3748 |
| Triage | gemini-3.1-flash-lite | Enriched context (neighbor intents) | 3623 |
| Deep | Claude Sonnet 4.6 | Premium analysis, top-priority | 46 |

**Key insight:** Enriched cross-symbol context matters more than model
quality. Flash-lite with neighbor intents outperforms premium models
without context.

### Embedding Model Selection

A/B tested two embedding models with ablation validation:

| Model | Dims | Stability (store) | Stability (config) | Cost |
|-------|------|-------------------|--------------------|----|
| qwen3-embedding:8b (local) | 4096 | 0.79 | 0.88 | Free |
| **Gemini Embedding 2** | **3072** | **0.82** | **0.92** | **$0.20/M tokens** |

Gemini Embedding 2 won on 2/3 crates. Native provider with asymmetric
task types: `RETRIEVAL_DOCUMENT` for indexing, `CODE_RETRIEVAL_QUERY`
for search.

### Edge Extraction (Phase 8.15)

Added TYPE_REF and IMPLEMENTS structural edges:
- **1026 TYPE_REF edges** — functions → types they use as parameters/returns
- **12 IMPLEMENTS edges** — types → traits they implement
- Reduced orphan/loner symbols from ~50 to 2-3
- Stability improved from 0.33 to 0.82

---

## Product Positioning

### What AETHER does that no other tool does

1. **Semantic Intent Records (SIRs):** Every symbol has a natural-language
   description of *why* it exists, not just *what* it does. Generated by
   LLM, enriched with cross-symbol context, versioned over time.

2. **Structural graph with TYPE_REF edges:** Not just "who calls who" but
   "which functions use which types." This enables community detection that
   understands the actual dependency structure.

3. **Closed-loop self-analysis:** AETHER can analyze its own codebase,
   diagnose problems, guide refactoring, and validate the fix — all with
   the same tooling.

4. **Asymmetric embeddings:** Documents and queries are embedded differently
   for optimal retrieval, using Gemini Embedding 2's native task types.

### The refactoring demo in one sentence

> "AETHER analyzed a 6817-line God File, identified 11 natural modules
> through structural graph analysis, guided an LLM to produce a more
> precise refactoring plan than it could alone, and then verified the
> improvement with its own health scoring — reducing the file to a
> 751-line façade with 13 focused modules."

---

## Cost Summary

| Operation | Cost | Time |
|-----------|------|------|
| Full scan (3748 symbols) | ~$1.00 | ~5 min |
| Full triage (3623 symbols) | ~$1.00 | ~3 hours (sequential) |
| Deep pass (46 symbols, Sonnet) | ~$1.15 | ~10 min |
| Embedding (3553 symbols, Gemini) | ~$0.10 | ~5 min |
| **Total intelligence generation** | **~$3.25** | **~3.5 hours** |

For a 55K+ LOC codebase. At scale:
- 10K symbols (ruff-size): ~$7
- 45K symbols (bevy-size): ~$30

---

## Raw Data References

### Health score before refactor
```
Overall: 42/100 (Watch)
aether-store: 72/100 — God File, Legacy Residue
  Structural: 100, Git: 65, Semantic: 45
```

### Health score after refactor
```
Overall: 43/100 (Watch) — Delta: +1
aether-store: 77/100 — Legacy Residue (God File RESOLVED)
  Structural: 100, Git: 65, Semantic: 58
```

### Ablation baseline (aether-store, post full-triage)
```
Row 6 (full pipeline):
  communities=11, largest=131, smallest=2, loners=3
  confidence=0.93, stability=0.82
  top modules: str_ops, threshold_ops, graph_ops
```

### Configuration (production)
```toml
[inference]
provider = "gemini"
model = "gemini-3.1-flash-lite-preview"
api_key_env = "GEMINI_API_KEY"
concurrency = 12

[embeddings]
enabled = true
provider = "gemini_native"
model = "gemini-embedding-2-preview"
api_key_env = "GEMINI_API_KEY"
dimensions = 3072
vector_backend = "sqlite"

[planner]
semantic_rescue_threshold = 0.90
```

### Key decisions locked in this session
- #83: Gemini Embedding 2 as production embedding model
- #84: Semantic rescue threshold → 0.90
- #85: --embeddings-only flag for rapid model testing
- #86: OpenAI-compatible embedding provider
- #87-89: Gemini native provider with asymmetric task types

### Git commits from this session
- 8.14: Component-bounded semantic rescue
- 8.15: TYPE_REF + IMPLEMENTS edge extraction (db2a3e6)
- 8.16: --embeddings-only + OpenAI-compat embedding provider
- 8.17: Gemini native embedding provider (5938c05)
- Refactor: aether-store split into 13 modules (ccc8ec3)
