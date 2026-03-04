# Phase 8 — The Crucible (Hardening & Scale)

## Strategic Preamble: The Pivot to Depth over Breadth

**Status:** 🚧 Active
**Replaces:** Stage 7.5 (AETHER Legal) and Stage 7.7 (AETHER Finance) as next priorities

### Why We Are Pivoting
AETHER has achieved a bleeding-edge capability: cross-layer intelligence fusing AST, Vector, Graph, and LLM Intent data into a unified, agent-accessible engine.

However, expanding into new verticals (Legal, Finance) before the core engine has survived the reality of 10,000-file enterprise monorepos presents a catastrophic risk. Parsing PDFs via heuristics is fundamentally different from deterministic AST parsing.

Phase 8 halts lateral expansion to focus entirely on **reliability, synchronization, and scale**. AETHER must prove its value on massive codebases before it learns to read contracts.

**Tagline:** "A Ferrari belongs on the track, not off-road."

---

## Phase 8 Overview

**Goal:** Ensure AETHER survives crashes without state corruption, delivers immediate value on large repositories without blocking on 12-hour indexing times, serves teams reliably, and has the observability and testing infrastructure to prove it all works.

## In Scope
- Multi-database state reconciliation and crash recovery
- Progressive, tiered indexing to solve the "Cold Start" latency
- Mock provider removal + tiered cloud/local hybrid inference
- Scale validation on real-world large codebases
- Hardening `aether-query` for concurrent Team Tier deployments
- Operational telemetry and monitoring
- CI-based index publishing for team adoption
- Self-improving SIR quality via feedback loops

## Out of Scope
- Non-code document parsing (PDFs, DOCX, CSV)
- Legal and Financial domain schemas (CIR, FIR)
- New underlying database technologies
- SPA framework migration (deferred to Phase 9)

## Key Decisions Made in Phase 8
- **#44:** Remove Mock provider — dead code that masked real pipeline bugs via a different code path
- **#45:** Default local model updated to `qwen3.5:9b` (from `qwen2.5-coder:7b-instruct-q4_K_M`)
- **#46:** Tiered provider for cloud+local hybrid inference (NIM/Gemini primary + Ollama fallback)

---

## Stage Plan

| Stage | Name | Focus | Priority | Status |
|-------|------|-------|----------|--------|
| 8.1 | State Reconciliation | **Reliability** | 🔴 Run Now | 📋 Spec Ready |
| 8.2 | Progressive Indexing + Tiered Providers | **Performance + Providers** | 🔴 Run Now | 📋 Spec Ready |
| 8.7 | Stress Test Harness | **Validation** | 🔴 Run Now | 📋 Spec Ready |
| 8.3 | Team Tier Hardening | **Scale** | 🟠 Run Next | 📋 Outlined |
| 8.5 | Operational Telemetry | **Observability** | 🟠 Run Next | 📋 Outlined |
| 8.6 | CI Index Publisher | **Adoption** | 🟡 Planned | 📋 Outlined |
| 8.8 | SIR Feedback Loop | **Quality** | 🟡 Planned | 📋 Outlined |
| 8.4 | UI State Preparation | **UX** | ⚪ Defer to Phase 9 | 📋 Outlined |

### Execution Order

```
8.1 State Reconciliation
  ↓
8.2 Progressive Indexing + Tiered Providers (Mock removal, TieredProvider, qwen3.5:9b)
  ↓
8.7 Stress Test Harness (validates 8.1 + 8.2)
  ↓
8.3 Team Tier Hardening
  ↓
8.5 Operational Telemetry
  ↓
8.6 CI Index Publisher
  ↓
8.8 SIR Feedback Loop

8.4 UI State Preparation → Phase 9
```

---

## Stage 8.1 — State Reconciliation Engine

**Full spec:** `phase_8_stage_8_1_state_reconciliation.md`

Fix the multi-DB split-brain risk (SQLite vs LanceDB vs SurrealDB). Before writing to the three DBs, log a Write-Ahead Intent in SQLite. If `aetherd` crashes mid-transaction, the intent log enables recovery on restart. New `aether fsck` command performs cross-database consistency verification and optional repair.

**Key deliverables:** write_intents table, coordinated write flow in SIR pipeline, intent replay on startup, `aether fsck [--repair]` CLI command.

---

## Stage 8.2 — Progressive Indexing + Tiered Providers

**Full spec:** `phase_8_stage_8_2_progressive_indexing.md`

Three objectives in one stage:

1. **Solve the "Cold Start" problem** — Split indexing into Pass 1 (AST + Graph, seconds) and Pass 2 (SIR + Vectors, background hours). Priority queue orders by git recency, PageRank centrality, symbol kind, and file size. On-demand SIR bump when agents or users query unindexed symbols.

2. **Remove Mock provider** (Decision #44) — Delete MockProvider/MockEmbeddingProvider entirely. They operated via a different code path (file-level vs per-symbol) and masked real inference bugs. Replace with inline test doubles for unit tests.

3. **Add Tiered provider** (Decision #46) — New `TieredProvider` routes high-priority symbols to a cloud provider (NVIDIA NIM with `qwen3.5-397b-a17b`, or Gemini) and bulk symbols to local Ollama (`qwen3.5:9b`). On primary timeout/429, falls back to Ollama automatically. Default local model updated from `qwen2.5-coder:7b-instruct-q4_K_M` to `qwen3.5:9b` (Decision #45).

**Key deliverables:** tiered indexing pipeline, priority queue module, on-demand SIR trigger from MCP/LSP, SIR coverage tracking, TieredProvider, Mock removal, model default update.

---

## Stage 8.7 — Stress Test Harness

**Full spec:** `phase_8_stage_8_7_stress_test_harness.md`

Prove AETHER works at scale. Benchmark suite clones real-world OSS repos (ripgrep, ruff, bevy, TypeScript, flask), runs full pipeline, captures metrics (time, memory, success rate, query latency), simulates crash recovery, and produces a formatted "AETHER Scale Report."

**Key deliverables:** benchmark shell scripts, repo definitions, metric capture, markdown report generator.

---

## Stage 8.3 — Team Tier Hardening

**Full spec:** `phase_8_deferred_stages.md` § 8.3

Bulletproof `aether-query` for multi-user concurrent access. Index hot-swap notification (SQLite write epoch), token-bucket rate limiting, connection pool configuration. Load testing with stress harness first.

---

## Stage 8.5 — Operational Telemetry & Health Dashboard

**Full spec:** `phase_8_deferred_stages.md` § 8.5

Prometheus-compatible `/metrics` endpoint on `aether-query`. Structured metrics for SIR throughput, provider error rates, queue depth, vector store size. Dashboard "System Health" panel. Zero-cost `metrics` crate integration.

---

## Stage 8.6 — CI Index Publisher

**Full spec:** `phase_8_deferred_stages.md` § 8.6

Bridge solo→team adoption. `aether index --ci --output` produces a compressed, portable index archive. `aether index --import` loads a CI-produced archive. Delta mode re-indexes only changed symbols. GitHub Actions and GitLab CI templates included.

---

## Stage 8.8 — SIR Feedback Loop & Regeneration Triggers

**Full spec:** `phase_8_deferred_stages.md` § 8.8

Close the quality loop. `aether regenerate` CLI command bulk-requeues low-confidence SIR. Automatic regeneration on provider change. Quality dashboard view with confidence distribution and provider breakdown. Can auto-regenerate Ollama-produced SIR through Gemini/NIM during off-peak hours.

---

## Stage 8.4 — Interactive State Management (Deferred to Phase 9)

**Full spec:** `phase_8_deferred_stages.md` § 8.4

Alpine.js integration for client-side state. Write-API endpoints for manual SIR editing, graph annotation, project memory curation. SPA migration decision document. **Deferred because this is feature expansion, not hardening.**
