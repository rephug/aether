# AETHER Roadmap Overview v3 (Post-Phase 4)

## Definitions
- **Phase** = a major product milestone that changes what AETHER *is* or *where it can see*.
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
| 4 Architect | 4.1 LanceDB | ✅ | LanceDB ANN vector backend |
| 4 Architect | 4.2 Tracing | ✅ | Structured logging via `tracing` |
| 4 Architect | 4.3 gix | ✅ | Native git via `gix` library |
| 4 Architect | 4.4 Dependency Edges | ✅ | CALLS + DEPENDS_ON from tree-sitter AST |
| 4 Architect | 4.5 Graph Storage | ✅ | CozoDB graph with Datalog queries |
| 4 Architect | 4.6 SIR Hierarchy | ✅ | File and module level SIR rollup |

---

## Phase 5 — The Cartographer (Expansion & Local Intelligence)

**Goal:** Expand AETHER's map — understand more languages and run search intelligence locally. Moves deployment from Cloud-Only to Hybrid.

| Stage | Status | Description |
|-------|--------|-------------|
| 5.1 Language Plugin | 📋 | Refactor aether-parse into modular per-language config + hooks |
| 5.2 Python Support | 📋 | Full Python parsing, symbols, edges, SIR, search |
| 5.3 Candle Embeddings | 📋 | In-process Qwen3-Embedding-0.6B via Candle |
| 5.4 Reranker | 📋 | Optional reranking with Candle local + Cohere API |
| 5.5 Adaptive Thresholds | 📋 | Per-language vector search similarity gating |

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

---

## Phase 6 — Planned, not scoped

- Ticket/PR API connectors (GitHub Issues, Linear → Decision #28)
- Jira connector
- Event bus refactor (Decision #15, #27)
- Additional languages (Go, Java, C#) via language plugin
- GPU acceleration for Candle (Metal, CUDA)
- Full Local deployment (Ollama for SIR inference)

## Phase 7+ — Horizon

- Team collaboration / `aether sync` (Decision #29)
- Enterprise connectors (LDAP/SSO, audit logging)
- Bi-directional English↔code bridge
- Graph federation across team members

---

## How to use this roadmap
Each Stage doc includes: Goal, In/Out of scope, Pass criteria, Exact Codex prompts.
See `CODEX_GUIDE.md` for how to run these prompts effectively.
