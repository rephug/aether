# Phase 7 — The Pathfinder (Revised)

## Thesis

Phase 6 proved AETHER can understand a *project*, not just its *code*. Phase 7 takes two orthogonal steps: **outward** (making intelligence shareable across a team) and **downward** (making the engine work on non-code documents). The Pathfinder phase transitions AETHER from a single-user code intelligence tool to a multi-user, multi-domain persistent understanding engine.

**One-sentence summary:** "From one developer, one language — to any team, any document."

---

## Tech Stack Reassessment — What Changed and Why

Phase 7 planning triggered a comprehensive review of foundational technology choices. The architecture has fundamentally shifted from Phase 4's "single-process embedded" model to Phase 7's "multi-process server-client" model. Several decisions that were correct in Phase 4 are now wrong for Phase 7.

### Change 1: SurrealDB 3.0 replaces CozoDB (Decision #38)

**The trigger:** Phase 7 introduces `aether-query` (separate read-only binary), a web dashboard (HTTP API), and Team Tier (shared index across developers). These require concurrent database access from multiple processes. CozoDB's sled backend uses an exclusive file lock that blocks this entirely.

**Why CozoDB was originally chosen:** In Phase 4, the hard constraint was "embeddable, in-process, no server." CozoDB met this with Datalog queries, built-in graph algorithms, and a pure-Rust sled backend. That constraint no longer applies.

**Why CozoDB must go now:**
1. **Sled exclusive lock** — cannot support multi-process access. The entire Stage 7.2 (RocksDB migration) existed solely to work around this.
2. **`links` conflict** — CozoDB's `storage-sqlite` conflicts with rusqlite. This forced sled, which caused problem #1.
3. **Maintenance risk** — sparse GitHub activity, unanswered questions, no release in months.

**Why SurrealDB 3.0 is the right replacement:**
- **SurrealKV embedded backend** — pure-Rust, MVCC, concurrent readers AND writers. No file lock.
- **Record References** — bidirectional edges at schema level (`REFERENCE` + `<~` traversal). Eliminates manual reverse queries.
- **Computed Fields** — derived properties (staleness, health scores) computed at query time in the schema.
- **HNSW vector search** — 8x faster in 3.0. Could eventually absorb LanceDB's role (not in Phase 7).
- **Full-text BM25 with OR operators** — better lexical search than current SQL LIKE.
- **DEFINE API** — custom HTTP endpoints directly in the DB. Future simplification path for Team Tier.
- **Surrealism WASM extensions** — future path for running graph algorithms close to data.
- **Time-travel queries** — `VERSION` clause for temporal queries. Aligns perfectly with AETHER's temporal tracking thesis.
- **Built-in auth** — users, scopes, tokens. Needed for Team Tier.
- **BSL 1.1 license** — restriction is "don't offer as DBaaS" only. AETHER is not a DBaaS.

**What this eliminates:** Stage 7.2 (RocksDB migration) is gone entirely. ~500 lines of snapshot/signal/migration code never need to be written.

**What this costs:** ~500 lines of graph algorithm reimplementation (PageRank, community detection, shortest path). Datalog → SurrealQL query rewrite. Both are bounded, well-understood tasks.

See Decision #38 in DECISIONS_v4.md for full evaluation matrix.

### Change 2: lopdf replaces pdf-extract (Decision #39 revised)

The Rust-native PDF fallback (`pdf-extract`) produces mediocre output on complex layouts. Initially planned to use `pdfium-render` (Google's Pdfium engine), but rejected because it requires a C++ dynamic library (`libpdfium.so`/`pdfium.dll`) at runtime — breaking AETHER's single-binary portability. `lopdf` (pure Rust) provides moderate quality text extraction with zero system dependencies. Primary extraction (`pdftotext` via Poppler) is unchanged.

### Change 3: MCP dual transport — stdio + HTTP/SSE (Decision #40)

Phase 1 was stdio only (VS Code extension). Phase 7's aether-query needs HTTP-based MCP. Both transports now supported, sharing the same tool registry. The MCP spec's streamable HTTP transport is the standard.

### Change 4: HTMX + D3 for dashboard (Decision #41)

Refinement of Decision #32. HTMX handles server-driven UI interactions (partial page updates, search, filtering). D3 handles visualizations. Still no React, no Node.js, no build step.

### Change 5: Record References + Computed Fields (Decisions #42, #43)

SurrealDB 3.0's schema-level features replace application-level workarounds:
- `REFERENCE` keyword makes edges bidirectional automatically
- `COMPUTED` keyword derives properties at query time (staleness, edge counts, health scores)

---

## Corrected Stage Plan (Revised)

| Stage | Name | Novel? | Scope | Codex Runs | Dependencies |
|-------|------|--------|-------|------------|--------------|
| 7.1 | Store Pooling + Shared State | Infrastructure | Connection pooling, shared state refactor, read-only flag | 1–2 | Phase 6 complete |
| 7.2 | SurrealDB Graph Migration | Infrastructure | Replace CozoDB/sled with SurrealDB/SurrealKV, reimpl graph algorithms | 2–3 | 7.1 |
| 7.3 | aether-query Read-Only Server | **New binary** | Lightweight query server using shared state + SurrealDB concurrent access | 2–3 | 7.2 |
| 7.4 | Document Abstraction Layer | **Architectural** | Domain-agnostic traits, tables, embedding pipeline | 2–3 | 7.1 |
| 7.5 | AETHER Legal | **New vertical** | Clause parser, CIR schema, legal MCP tools | 2–3 | 7.4 |
| 7.6 | Web Dashboard | **New UI** | HTTP API + HTMX + D3 visualization | 2–3 | 7.1 (reads from pooled state) |
| 7.7 | AETHER Finance | **New vertical** | Financial parser, FIR schema, money flow tracing | 2–3 | 7.4 |

### Dependency Graph

```
7.1 (Store Pooling) ──────────────────────────────────┐
    │                                                  │
    ├── 7.2 (SurrealDB Migration) ── 7.3 (aether-query)
    │                                                  │
    ├── 7.4 (Document Abstraction) ──┬── 7.5 (Legal)  │
    │                                └── 7.7 (Finance) │
    │                                                  │
    └── 7.6 (Web Dashboard) ──────────────────────────┘
```

**Parallelism opportunities:**
- After 7.1 merges: 7.2 and 7.4 can start in parallel (different concern areas)
- After 7.4 merges: 7.5 and 7.7 can be parallel
- 7.6 depends only on 7.1 (reads from pooled state), so it can start any time after 7.1

**Key change from original:** Stage 7.2 is now SurrealDB migration (bounded, well-understood) instead of RocksDB migration (C++ build dependency, feature flags, two-backend testing matrix). Net complexity is lower despite the larger scope of the migration.

---

## Decisions Locked for Phase 7

See DECISIONS_v4.md for full details. Summary of new/changed decisions:

| # | Decision | Resolution |
|---|----------|------------|
| 12 | Graph storage | SurrealDB 3.0 with SurrealKV backend replaces CozoDB/sled |
| 38 | SurrealDB migration | GraphStore trait provides swap point. Datalog → SurrealQL rewrite. Graph algorithms reimplemented in Rust. |
| 39 | PDF fallback | lopdf replaces pdf-extract (pdfium-render rejected) |
| 40 | MCP transport | stdio (VS Code) + HTTP/SSE (aether-query, Team Tier) |
| 41 | Dashboard tech | HTMX + D3 + Tailwind CSS (CDN). No React, no Node.js. |
| 42 | Bidirectional edges | SurrealDB Record References (`REFERENCE` + `<~`) |
| 43 | Computed fields | SurrealDB `COMPUTED` for derived properties |

Unchanged but confirmed: SharedState pooling (#28/#37), document abstraction approach (#31), vertical feature flags (#33), rust_decimal for money (#35), entity resolution (#36), schema versioning (#37).

---

## Gap Analysis — Issues Identified and Mitigated

### GAP-1: LanceDB concurrent access (MITIGATED)

**Problem:** LanceDB concurrent access when aetherd writes and aether-query reads.

**Analysis:** LanceDB uses append-only Lance columnar format with MVCC. Concurrent reads are natively supported. Write locks are advisory. Stage 7.3 includes explicit verification test.

### GAP-2: Index freshness visibility (MITIGATED)

**Problem:** aether-query users see stale results with no warning.

**Mitigation:** Stage 7.3 adds `last_indexed_at` to status responses and a `staleness_warning` field when index is older than configurable threshold (default: 30 minutes).

### GAP-3: Test fixtures for legal/finance (MITIGATED)

**Problem:** Can't ship copyrighted contracts or financial statements.

**Mitigation:** Synthetic LLM-generated test documents in `tests/fixtures/` for each vertical.

### GAP-4: Document abstraction embedding pipeline (MITIGATED)

**Problem:** Original plan created tables but no embedding pipeline.

**Mitigation:** Stage 7.4 includes domain-agnostic embedding: `SemanticRecord.embedding_text()` → `aether-infer` → domain-scoped LanceDB tables.

### GAP-5: Feature flag complexity in CI (MITIGATED)

**Mitigation:** CI tests three configurations: default (no verticals), `--all-features`, per-vertical matrix.

### GAP-6: SurrealDB embedded stability (NEW — replaces old RocksDB GAP-6)

**Problem:** SurrealDB 3.0 just shipped. Embedded mode (SurrealKV) is newer and less battle-tested than sled.

**Mitigation:**
- SurrealKV is ACID-compliant with MVCC (well-understood guarantees)
- Phase 7.2 includes comprehensive data integrity tests (write → crash → verify)
- `GraphStore` trait preserved as exit path. If SurrealDB has critical issues, fallback to RocksDB-backed CozoDB (original 7.2 plan preserved in git history)
- AETHER's graph workload is modest (~20K edges for self-hosting) — not pushing SurrealKV's limits

### GAP-7: SurrealDB graph algorithm gap (NEW)

**Problem:** CozoDB provides built-in PageRank, community detection, shortest path. SurrealDB has none.

**Mitigation:**
- Immediate: Rust application-level implementations (~500 LOC total)
- These algorithms fetch graph data via standard SurrealQL queries, compute in Rust, return results
- Existing `aether-analysis` crate already owns drift/coupling/health analysis — graph algorithms slot in naturally
- Future: Port to Surrealism WASM extensions for near-data execution (optimization, not Phase 7 scope)

---

## Crate Architecture After Phase 7

```
SHARED ENGINE (domain-agnostic)
├── aether-core        # Symbol/document model, stable IDs, diffing
├── aether-config      # Config loader (all sections)
├── aether-store       # SQLite + LanceDB + SurrealDB (was CozoDB)
├── aether-infer       # Provider traits (Gemini, Ollama, Candle, Mock)
├── aether-memory      # Project notes, session context (Phase 6)
├── aether-analysis    # Drift, coupling, health, graph algorithms
├── aether-document    # NEW: Domain-agnostic traits + embedding pipeline
├── aether-mcp         # MCP tool definitions (shared by aetherd + aether-query)
└── aether-web         # NEW: HTTP API (shared by aetherd + aether-query)

CODE-SPECIFIC
├── aether-parse       # tree-sitter for code symbols
├── aether-sir         # Code SIR schema
├── aether-lsp         # Code hover server
├── aether-git         # Git coupling, commit linkage
└── aetherd            # Code daemon binary

TEAM INFRASTRUCTURE
└── aether-query       # NEW: Read-only query server binary

VERTICALS (feature-gated)
├── aether-legal       # NEW: Legal clause parser + CIR
└── aether-finance     # NEW: Financial parser + FIR + entity resolution
```

---

## Risk Register

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| SurrealDB embedded (SurrealKV) stability issues | Medium | High | GraphStore trait exit path, data integrity tests, modest workload |
| SurrealDB graph algorithm gap | Certain | Medium | ~500 LOC Rust reimplementation. Bounded, well-understood. |
| Graph algorithm correctness (PageRank/Louvain) | Medium | Medium | petgraph data structures, spawn_blocking for async safety, comparison tests against CozoDB baseline |
| Datalog → SurrealQL rewrite errors | Medium | Medium | Comprehensive test suite verifies query equivalence |
| SurrealDB binary size bloat | Medium | Low | Already building multiple binaries. Feature-gated. |
| PDF extraction quality too low for legal | High | High | pdftotext primary (handles 95%+ of legal PDFs), lopdf pure-Rust fallback, manual text input escape hatch. pdfium-render rejected due to C++ runtime dependency breaking portability. |
| SharedState refactor breaks existing tests | Medium | Medium | Stage 7.1 is isolated refactor, all existing tests must pass unchanged |
| LanceDB concurrent access fails | Low | High | Stage 7.3 includes explicit concurrent access verification |
| Document abstraction over-engineered | Medium | Medium | Traits kept minimal (3-5 methods each). No speculative features. |
| Entity resolution too basic for finance | High | Medium | Documented as MVP with explicit upgrade path. Manual curation escape hatch. |
| Dashboard scope creep | Medium | Low | Strict read-only. No editing, no WebSocket, no auth beyond bearer token. |
