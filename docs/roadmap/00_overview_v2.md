# AETHER Roadmap Overview v2 (Aligned to Prospectus)

## Definitions
- **Phase** = a major product milestone that changes what AETHER *is*.
- **Stage** = a shippable chunk inside a Phase, sized for 1–3 Codex CLI runs.
- **✅** = merged to main. **🔧** = in progress. **📋** = planned.

## Completed state (as of Feb 2026)

| Phase | Stage | Status | Description |
|-------|-------|--------|-------------|
| 1 Observer | 1 Indexing | ✅ | tree-sitter parse, stable BLAKE3 IDs, incremental diff |
| 1 Observer | 2 SIR Generation | ✅ | Inference providers (mock/gemini/qwen3), validation, retry |
| 1 Observer | 3.1 Lexical Search | ✅ | CLI + MCP symbol search via SQL LIKE |
| 1 Observer | 3.2 Robustness | ✅ | Stale SIR tracking, error recovery |
| 1 Observer | 3.3 SQLite Source of Truth | ✅ | SQLite canonical store with optional file mirrors |
| 1 Observer | 3.4 Semantic Search | ✅ | Embedding storage + brute-force cosine similarity |
| 1 Observer | 3.5 CLI UX + Config | ✅ | Config loading, CLI overrides, validation |
| 1 Observer | 3.6 MCP + LSP UX | ✅ | Stable response envelopes, hover formatting |
| 1 Observer | 3.7 VS Code Extension | ✅ | Search, status bar, provider selection |
| 1 Observer | 3.8 CI + Release | ✅ | 6-target release pipeline, CI gates |
| 2 Historian | 2.1 SIR Versioning | ✅ | sir_history table, version-on-hash-change |
| 2 Historian | 2.2 Git Linkage | ✅ | commit_hash column via `git rev-parse` |
| 2 Historian | 2.3 Why Queries | ✅ | SIR JSON diff, MCP tool, CLI flag |
| 3 Ghost | 3.1 Host Verification | ✅ | Allowlisted command runner |
| 3 Ghost | 3.2 Container Verification | ✅ | Docker mode with host fallback |
| 3 Ghost | 3.3 MicroVM | 📋 | Firecracker/Hyper-V hooks (stub only) |

## Prospectus gap analysis

The V3.0 Prospectus specifies capabilities that are **not yet implemented**. These become Phase 4.

| Gap | Prospectus Reference | Impact |
|-----|---------------------|--------|
| LanceDB vector backend | Decision #3, §3.1, §5.1 | Semantic search is O(n) brute-force; breaks at ~5K symbols |
| Dependency edge extraction | §3.2.1 (CALLS, DEPENDS_ON) | No relationship data; can't answer "what uses this?" |
| KuzuDB graph database | Decision #2, §3.2 | ⚠️ **ARCHIVED Oct 2025** — replaced by CozoDB (see Decision #23) |
| gix native git | §5 Tech Stack | Shell-out to `git rev-parse` is fragile |
| tracing observability | §5 Tech Stack | All logging is `eprintln!`; no structured output |
| SIR hierarchy (file/module) | §7.1 | Only leaf SIR exists; no rollup |

### CozoDB replaces KuzuDB (Decision #23)

KuzuDB's GitHub repository was **archived by its owner on October 10, 2025**. After evaluating 9 alternatives (SurrealDB, HelixDB, Cayley, Neo4j, ArangoDB, Cognee, Nebula, OrientDB, CozoDB), **CozoDB** was selected:

- **Rust-native**, embeddable in-process (same model as SQLite)
- **Datalog** query language with recursive traversal — ideal for call chains
- **Built-in graph algorithms** (PageRank, shortest path, community detection)
- **MPL 2.0 license** — permissive, compatible with AETHER
- **SQLite backend** — minimal footprint, single-file storage
- `GraphStore` trait provides clean exit path if CozoDB maintenance stalls

---

## Phase 4 — The Architect (Infrastructure Alignment)

**Goal:** Close the gap between the running code and the V3.0 Prospectus.

Stages:
- **Stage 4.1:** LanceDB vector backend (replace brute-force embeddings)
- **Stage 4.2:** Structured logging with `tracing` (replace `eprintln!`)
- **Stage 4.3:** Native git via `gix` (replace shell-out to `git rev-parse`)
- **Stage 4.4:** Dependency edge extraction (CALLS + DEPENDS_ON from tree-sitter)
- **Stage 4.5:** Graph storage via CozoDB (Datalog-powered symbol relationship queries)
- **Stage 4.6:** SIR hierarchy — file and module level rollup

### Dependency chain
```
4.1 LanceDB ──────────────────────────────────┐
4.2 tracing (independent) ────────────────────┤
4.3 gix (independent) ────────────────────────┤
4.4 dependency extraction ──► 4.5 graph store ─┤
                                               ▼
                                     4.6 SIR hierarchy
```

4.1, 4.2, 4.3 are independent and can run in parallel.
4.4 must land before 4.5 (graph needs edge data).
4.6 benefits from all prior stages but only strictly requires 4.1.

---

## Phase 5 — The Cartographer (planned, not scoped)

- Ticket/PR API connectors (Jira, Linear, GitHub Issues)
- Candle local embeddings (Qwen3-Embedding-0.6B)
- Reranker integration
- Python language support
- Adaptive similarity thresholds
- Event bus refactor
- Team collaboration / `aether sync`

---

## How to use this roadmap
Each Stage doc includes: Goal, In/Out of scope, Pass criteria, Exact Codex prompts.
See `CODEX_GUIDE.md` for how to run these prompts effectively.
