# Phase 6 — The Chronicler (Project Context Layer)

## Strategic Preamble: The Legal Product Question

### Feature-by-Feature Legal Reusability

| Phase | Feature | Code Use | Legal Use | Reusable? |
|-------|---------|----------|-----------|-----------|
| 6.1 | Project memory store | Architecture decisions, rationale | Case notes, research memos, client context | **100%** — same store, same API |
| 6.2 | Temporal coupling (git) | Files that co-change in commits | Documents that co-change in amendments | **Pattern yes, implementation no** — git-specific algorithm, but storage/query layer reusable |
| 6.3 | Test intent extraction | `it("should handle X")` guards | N/A | **0%** — code-specific |
| 6.4 | Session context capture | Agent coding session notes | Agent research session notes | **100%** — same MCP tool, same store |
| 7.1 | Bi-temporal fact tracking | "What did this function do on Jan 15?" | "What was the indemnity cap on March 15?" | **100%** — arguably MORE useful for legal |
| 7.2 | Memory decay/consolidation | Stale symbols rank lower | Closed cases fade, active matters surface | **90%** — same algorithm, different decay curves |
| 7.3 | Multi-agent scoping | Parallel Codex agents on same repo | Multiple attorneys on same matter | **100%** — same concurrency model |
| 8+ | Entity extraction / KG | Extract entities from code comments/docs | Extract parties, dates, obligations from contracts | **Pattern yes, models no** — same CozoDB graph, different extraction prompts |

**Bottom line:** ~70% of Phase 6-7 work directly serves the legal product. The 30% that doesn't (temporal coupling git algorithm, test intent extraction) is code-specific but small in implementation scope.

### Architecture Decision: When and How to Split

**Recommendation: Split at the binary level in Phase 8, not at the product level now.**

The crate workspace already has clean boundaries. The strategy is:

```
SHARED ENGINE (stays in aether workspace)
├── aether-core        # Symbol/document model, stable IDs, diffing
├── aether-store       # SQLite + LanceDB + CozoDB (domain-agnostic)
├── aether-infer       # Provider traits (Gemini, Ollama, etc.)
├── aether-config      # Config loader
├── aether-memory      # NEW Phase 6: project memory, session context, decay
└── aether-temporal    # NEW Phase 7: bi-temporal tracking, fact validity

CODE-SPECIFIC (aether product)
├── aether-parse       # tree-sitter for code symbols
├── aether-sir         # Code SIR schema
├── aether-lsp         # Code hover server
├── aether-mcp         # Code MCP tools
├── aether-git         # Git coupling, commit linkage
└── aetherd            # Code daemon binary

LEGAL-SPECIFIC (aether-legal product, Phase 8+)
├── aether-legal-parse # PDF/DOCX clause extraction
├── aether-legal-sir   # Legal document schema (clauses, obligations, parties)
├── aether-legal-mcp   # Legal MCP tools
└── aether-legal-d     # Legal daemon binary
```

**Why not split now:**
1. You haven't validated legal market demand with design partners yet.
2. Premature abstraction costs more than refactoring later.
3. Phase 6 features don't require separate crates — they extend existing ones.
4. The natural split point is when you need a *different parser* (PDF/DOCX instead of tree-sitter). That's Phase 8.

**Why not keep them merged forever:**
1. Code users shouldn't pull PDF parsing dependencies.
2. Legal users shouldn't pull tree-sitter.
3. Different release cadences — code product iterates weekly, legal product may have slower compliance cycles.
4. Licensing flexibility — BSL for core engine, proprietary for legal-specific features (per your existing IP strategy).

**The Phase 6 design principle:** Build every new storage/query feature as domain-agnostic. When you need a "project note," the schema says `content`, `tags`, `source_type`, `entity_refs` — not `code_file` or `contract_name`. The domain specificity lives in the *tools that write to it* (code MCP vs. legal MCP), not the store itself.

---

## Phase 6 Overview

**Goal:** AETHER understands the *project*, not just the *code*. Add persistent memory for decisions, session context, and cross-file coupling intelligence. Then leverage AETHER's unique multi-layer architecture (AST + SIR + graph + git + vectors) to deliver capabilities that **no existing tool can provide**: semantic drift detection, causal change tracing, graph-powered health metrics, and intent verification.

**Tagline:** "From code intelligence to project intelligence."

**New crates:**
- `aether-memory` — project notes, session context, access tracking, memory search. Depends on `aether-store` and `aether-infer`. No code-specific logic.
- `aether-analysis` — drift detection, causal chains, graph health, intent verification. Depends on `aether-store`, `aether-infer`, `aether-memory`. Domain-agnostic (legal-reusable).

**Prerequisite:** Phase 5 complete (language plugin abstraction, at minimum 5.1 + 5.2).

---

## Stage Plan

| Stage | Name | Novel? | Scope | Codex Runs |
|-------|------|--------|-------|------------|
| 6.1 | Project Memory Store | Borrowed | SQLite table + LanceDB embeddings + MCP tools | 1–2 |
| 6.2 | Multi-Signal Coupling | **Enhanced** | Git co-change + AST dependency + semantic similarity fusion | 2–3 |
| 6.3 | Test Intent Extraction | Enhanced | AST test-string extraction + TESTED_BY edges + guards | 1–2 |
| 6.4 | Session Context + Access Tracking | Borrowed | Session note MCP + access counters + recency boost | 1–2 |
| 6.5 | Memory Search + Unified Query | Enhanced | Hybrid search across SIR + notes + coupling | 1–2 |
| 6.6 | Semantic Drift Detection | **NOVEL** | SIR version diffing + community detection + boundary violations | 1–2 |
| 6.7 | Causal Change Chains | **NOVEL** | Graph traversal × temporal SIR diff for root-cause tracing | 1–2 |
| 6.8 | Graph Health + Intent Verification | **NOVEL** | CozoDB graph algorithms + pre/post-refactor SIR comparison | 1–2 |

---

## Stage 6.1 — Project Memory Store

### Purpose
Give agents (and humans) a place to store and retrieve unstructured project knowledge that isn't extractable from code alone — architecture decisions, meeting notes, design rationale, "why we chose X over Y."

### What It Borrows From
- **Fold:** Semantic search over arbitrary content (but embedded in SQLite+LanceDB, not Qdrant server)
- **Engram:** Session-to-knowledge pipeline concept (but no tiered memory model — flat notes with semantic search outperforms artificial tiers for dev use cases)
- **Mem0:** Extract → Store → Search lifecycle with deduplication (but deterministic, not LLM-judged dedup)
- **EngramPro:** Persistent notes that surface in future impact analyses

### New Crate: `aether-memory`

```
crates/aether-memory/
├── Cargo.toml
└── src/
    ├── lib.rs          # Public API
    ├── note.rs         # ProjectNote model + CRUD
    ├── search.rs       # Hybrid search across notes
    └── dedup.rs        # Content-hash based deduplication
```

**Dependencies:** `aether-core`, `aether-store`, `aether-config`, `serde`, `serde_json`, `blake3`, `thiserror`

### Schema: `project_notes` table in SQLite

```sql
CREATE TABLE IF NOT EXISTS project_notes (
    note_id         TEXT PRIMARY KEY,     -- BLAKE3(content + created_at)
    content         TEXT NOT NULL,        -- Freeform text
    content_hash    TEXT NOT NULL,        -- BLAKE3(normalized_content) for dedup
    source_type     TEXT NOT NULL,        -- "manual" | "session" | "agent" | "import"
    source_agent    TEXT,                 -- Agent ID that created this note (nullable)
    tags            TEXT NOT NULL DEFAULT '[]',  -- JSON array of strings
    entity_refs     TEXT NOT NULL DEFAULT '[]',  -- JSON array of {kind, id} references
    file_refs       TEXT NOT NULL DEFAULT '[]',  -- JSON array of file paths
    symbol_refs     TEXT NOT NULL DEFAULT '[]',  -- JSON array of symbol IDs
    created_at      INTEGER NOT NULL,     -- Unix epoch millis
    updated_at      INTEGER NOT NULL,     -- Unix epoch millis
    access_count    INTEGER NOT NULL DEFAULT 0,
    last_accessed_at INTEGER,             -- Unix epoch millis (nullable)
    is_archived     INTEGER NOT NULL DEFAULT 0  -- Soft delete / archive
);

CREATE INDEX idx_project_notes_content_hash ON project_notes(content_hash);
CREATE INDEX idx_project_notes_source_type ON project_notes(source_type);
CREATE INDEX idx_project_notes_created_at ON project_notes(created_at);
CREATE INDEX idx_project_notes_archived ON project_notes(is_archived);
```

**Design notes:**
- `entity_refs` is intentionally generic: `{kind: "symbol", id: "abc123"}`, `{kind: "file", id: "src/main.rs"}`, `{kind: "person", id: "sarah"}`, `{kind: "clause", id: "indemnity-4.2"}`. This is the legal-reusability hook — the schema doesn't know or care what kinds of entities exist.
- `content_hash` enables deduplication: before inserting, check if a note with the same content hash exists. If so, update `updated_at` and `access_count` instead of creating a duplicate.
- `source_type` distinguishes manual notes from auto-captured session context (Stage 6.4).
- No tiered memory (Core/Conscious/Subconscious). Everything is flat + searchable. Ranking handles relevance.

### LanceDB: `project_notes_vectors` table

```
Schema:
  note_id:   String (foreign key to SQLite)
  vector:    FixedSizeList[Float32, DIM]  (embedding dimension from provider)
  content:   String (denormalized for display)
  created_at: Int64
```

Embeddings generated using same `aether-infer` embedding providers as SIR vectors. Embedding happens at write time (same as SIR embedding flow).

### MCP Tools

#### `aether_remember`
Store a project note.

**Request:**
```json
{
  "content": "We chose CozoDB over KuzuDB because KuzuDB was archived in Oct 2025. Decision #23.",
  "tags": ["architecture", "graph-storage"],
  "file_refs": ["crates/aether-store/src/graph.rs"],
  "symbol_refs": ["abc123def456"]
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "note_id": "a1b2c3...",
  "action": "created",          -- "created" | "updated_existing" (dedup hit)
  "content_hash": "d4e5f6...",
  "tags": ["architecture", "graph-storage"],
  "created_at": 1708000000000
}
```

**Behavior:**
- Compute `content_hash = BLAKE3(normalize(content))`.
- If existing non-archived note with same `content_hash`: increment `access_count`, update `updated_at`, merge tags. Return `action: "updated_existing"`.
- Otherwise: create new note, generate embedding, insert into both SQLite and LanceDB.
- `source_type` = `"agent"` when called via MCP, `"manual"` when called via CLI.

#### `aether_recall`
Search project notes by query.

**Request:**
```json
{
  "query": "why did we choose the graph database",
  "mode": "hybrid",          -- "lexical" | "semantic" | "hybrid" (default)
  "limit": 5,
  "include_archived": false,
  "tags_filter": ["architecture"]   -- optional
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "query": "why did we choose the graph database",
  "mode_used": "hybrid",
  "result_count": 2,
  "notes": [
    {
      "note_id": "a1b2c3...",
      "content": "We chose CozoDB over KuzuDB because...",
      "tags": ["architecture", "graph-storage"],
      "file_refs": ["crates/aether-store/src/graph.rs"],
      "symbol_refs": ["abc123def456"],
      "source_type": "agent",
      "created_at": 1708000000000,
      "access_count": 3,
      "relevance_score": 0.87
    }
  ]
}
```

**Search implementation:**
- Lexical: SQL `LIKE` on content + tags (same pattern as `aether_search`).
- Semantic: LanceDB ANN on query embedding vs. `project_notes_vectors`.
- Hybrid: RRF fusion of lexical + semantic (same as existing symbol hybrid search).
- Recency boost: `score *= 1.0 + (0.1 * recency_factor)` where `recency_factor` decays over 30 days.
- Access boost: `score *= 1.0 + (0.05 * log(access_count + 1))`.
- Tag filter: SQL `WHERE` pre-filter before ranking.
- Side effect: increment `access_count` and `last_accessed_at` on returned notes.

### CLI Surface

```bash
# Store a note
aether remember "We chose CozoDB because KuzuDB was archived" --tags architecture,graph

# Search notes
aether recall "graph database decision" --mode hybrid --limit 5

# List recent notes
aether notes --limit 10 --since 7d
```

### LSP Surface
No direct LSP integration in 6.1. Notes surface through MCP only. (LSP integration comes in 6.5 as unified hover enrichment.)

### Edge Cases

| Scenario | Behavior |
|----------|----------|
| Empty content | MCP request error: "content must not be empty" |
| Duplicate content (exact hash match) | Update existing: merge tags, bump access_count, return `action: "updated_existing"` |
| Embeddings disabled in config | Semantic/hybrid recall degrades to lexical with `fallback_reason: "embeddings_disabled"` |
| No notes in store | `recall` returns `result_count: 0`, empty `notes` array |
| Very long content (>10KB) | Accept and store, but truncate to first 2KB for embedding |
| Tags with special characters | Normalize: lowercase, trim whitespace, reject empty strings |

### Pass Criteria
1. `aether_remember` creates notes in SQLite with correct schema.
2. `aether_remember` with duplicate content returns `action: "updated_existing"`.
3. `aether_recall` returns relevant notes ranked by hybrid score.
4. `aether recall` CLI works with `--mode`, `--limit`, `--tags` flags.
5. Embedding generation follows existing `aether-infer` provider pattern.
6. Graceful fallback when embeddings disabled.
7. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

### Exact Codex Prompt
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_6_stage_6_1_project_memory.md for the full specification.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase6-stage6-1-project-memory off main.
3) Create worktree ../aether-phase6-stage6-1 for that branch and switch into it.
4) Create new crate crates/aether-memory with:
   - Cargo.toml depending on aether-core, aether-store, aether-config, serde, serde_json, blake3, thiserror
   - src/lib.rs with public API
   - src/note.rs with ProjectNote model, CRUD operations against SQLite
   - src/search.rs with hybrid search (lexical + semantic via LanceDB + RRF)
   - src/dedup.rs with content-hash deduplication
5) Add project_notes table migration in aether-store SQLite initialization.
6) Add project_notes_vectors table creation in aether-store LanceDB initialization.
7) Add MCP tools in crates/aether-mcp/src/lib.rs:
   - aether_remember: store note with dedup, generate embedding
   - aether_recall: hybrid search with recency/access boost
8) Add CLI commands in crates/aetherd:
   - `aether remember <content> --tags <tags>`
   - `aether recall <query> --mode <mode> --limit <n>`
   - `aether notes --limit <n> --since <duration>`
9) Add aether-memory to workspace Cargo.toml members.
10) Add tests:
    - Unit tests in aether-memory for CRUD, dedup, search ranking
    - Integration tests in aether-mcp for MCP tool schema validation
11) Run:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test --workspace
12) Commit with message: "Add project memory store with remember/recall MCP tools"
```

---

## Stage 6.2 — Multi-Signal Coupling Detection

### Purpose
Discover coupled files using three independent signals fused into a single score — something no existing tool does. Surface "blast radius" warnings when an agent or developer touches a file.

### What It Borrows and What It Adds
- **Borrowed from EngramPro:** Git co-change mining concept, blast radius analysis.
- **Borrowed from Zep/Graphiti:** Temporal awareness of relationships.
- **NOVEL — Multi-Signal Fusion:** EngramPro and all academic literature compute coupling from *one signal* (git co-change OR static dependency OR semantic similarity). AETHER fuses all three because it has all three data layers in the same process. When signals agree, extremely high confidence. When they disagree (high temporal coupling, no static dependency, low semantic similarity), the disagreement itself is a signal — flag as hidden operational coupling needing investigation.

### Design Decisions
- **Scope:** File-level coupling only (not symbol-level). Symbol-level would require tracking which symbols changed per commit, which is expensive and noisy. File-level co-change is the proven signal (per EngramPro, per academic research on temporal coupling).
- **Storage:** New CozoDB edge type `CO_CHANGES_WITH` with weight = co-change frequency.
- **Mining window:** Configurable, default last 500 commits.
- **Minimum threshold:** Files must co-change in ≥3 commits to create an edge (eliminates noise from bulk reformatting commits).

### Schema: CozoDB `co_change_edges` relation

```
:create co_change_edges {
    file_a: String,
    file_b: String,
    =>
    co_change_count: Int,
    total_commits_a: Int,
    total_commits_b: Int,
    git_coupling: Float,             -- co_change_count / max(total_commits_a, total_commits_b)
    static_signal: Float,            -- 1.0 if AST dependency exists, else 0.0
    semantic_signal: Float,          -- max SIR embedding similarity between files
    fused_score: Float,              -- weighted combination of all three
    coupling_type: String,           -- "structural" | "temporal" | "semantic" | "hidden_operational" | "multi"
    last_co_change_commit: String,
    last_co_change_at: Int,
    mined_at: Int
}
```

**Single-signal coupling score (git only):**
```
git_coupling = co_change_count / max(total_commits_a, total_commits_b)
```
Range 0.0–1.0.

**Multi-signal fusion (applied after mining):**
For each file pair in `co_change_edges`, also compute:
```
static_signal:   1.0 if any CALLS/DEPENDS_ON edge exists between symbols in file_a and file_b (from CozoDB dependency graph), else 0.0
semantic_signal: max cosine similarity between any SIR embedding in file_a and any SIR embedding in file_b (from LanceDB)
temporal_signal: git_coupling score above
```

**Fused coupling score:**
```
fused_score = 0.5 * temporal_signal + 0.3 * static_signal + 0.2 * semantic_signal
```

Weights chosen because temporal co-change is the strongest empirical predictor of coupling (per Cataldo et al. 2009), static dependency is ground truth when present, and semantic similarity catches conceptual coupling.

**Signal disagreement flag:** If `temporal_signal >= 0.5` AND `static_signal == 0.0` AND `semantic_signal < 0.3`, set `coupling_type = "hidden_operational"` — files change together for reasons not visible in code structure. Worth a project note.

Risk levels (on fused_score): ≥ 0.7 = Critical. ≥ 0.4 = High. ≥ 0.2 = Medium. Below = Low.

### SQLite: `coupling_mining_state` table

```sql
CREATE TABLE IF NOT EXISTS coupling_mining_state (
    id              INTEGER PRIMARY KEY DEFAULT 1,
    last_commit_hash TEXT,
    last_mined_at   INTEGER,
    commits_scanned INTEGER DEFAULT 0
);
```

Tracks where the miner left off for incremental updates.

### Mining Algorithm

```
Phase 1 — Git temporal signal:
1. Read last_commit_hash from coupling_mining_state.
2. Walk commits from HEAD back to last_commit_hash (or 500 commits, whichever is fewer) via gix.
3. For each commit:
   a. Get changed files (diff parent → commit).
   b. Filter out: lockfiles, .gitignore, auto-generated files, files matching [coupling.exclude] patterns.
   c. For each pair (file_a, file_b) where file_a < file_b (canonical ordering):
      - Increment co_change_count in memory map.
4. After walk:
   a. Compute git_coupling = co_change_count / max(total_commits_a, total_commits_b).
   b. Filter pairs with co_change_count >= threshold.

Phase 2 — Static + semantic signal enrichment:
5. For each surviving pair (file_a, file_b):
   a. Static signal: query CozoDB for any CALLS or DEPENDS_ON edge where source symbol is in file_a and target symbol is in file_b (or vice versa). If any edge exists, static_signal = 1.0, else 0.0.
   b. Semantic signal: for all SIR embeddings belonging to symbols in file_a, compute max cosine similarity against all SIR embeddings belonging to symbols in file_b (via LanceDB). Set semantic_signal = max_sim. If no SIR exists for either file, semantic_signal = 0.0.
   c. Compute fused_score = 0.5 * git_coupling + 0.3 * static_signal + 0.2 * semantic_signal.
   d. Classify coupling_type:
      - static_signal > 0 AND git_coupling >= 0.2 → "multi"
      - static_signal > 0 AND git_coupling < 0.2 → "structural"
      - static_signal == 0 AND semantic_signal >= 0.3 → "semantic"
      - static_signal == 0 AND semantic_signal < 0.3 AND git_coupling >= 0.5 → "hidden_operational"
      - else → "temporal"
6. Upsert results into CozoDB co_change_edges.
7. Update coupling_mining_state with HEAD commit hash.
```

**Performance note:** Phase 2 is the expensive part (CozoDB + LanceDB queries per pair). For large repos, batch the Datalog and LanceDB queries. Phase 2 can be deferred (store git_coupling immediately, enrich lazily on first blast_radius query for that pair).

### MCP Tool: `aether_blast_radius`

**Request:**
```json
{
  "file": "crates/aether-store/src/lib.rs",
  "min_risk": "medium"      -- "low" | "medium" | "high" | "critical"
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "target_file": "crates/aether-store/src/lib.rs",
  "mining_state": {
    "commits_scanned": 487,
    "last_mined_at": 1708000000000
  },
  "coupled_files": [
    {
      "file": "crates/aether-mcp/src/lib.rs",
      "risk_level": "critical",
      "fused_score": 0.89,
      "coupling_type": "multi",
      "signals": {
        "temporal": 0.89,
        "static": 1.0,
        "semantic": 0.72
      },
      "co_change_count": 48,
      "total_commits": 54,
      "last_co_change": "a1b2c3d",
      "notes": []                    -- Project notes referencing this file (from 6.1)
    },
    {
      "file": "crates/aether-mcp/tests/mcp_tools.rs",
      "risk_level": "high",
      "fused_score": 0.63,
      "coupling_type": "hidden_operational",
      "signals": {
        "temporal": 0.72,
        "static": 0.0,
        "semantic": 0.15
      },
      "co_change_count": 31,
      "total_commits": 43,
      "last_co_change": "e4f5g6h",
      "notes": ["MCP integration tests must mirror store API changes"]
    }
  ],
  "test_guards": []             -- Populated in Stage 6.3
}
```

**Behavior:**
- If coupling has never been mined, run mining first (may take a few seconds).
- If stale (>100 new commits since last mine), re-mine incrementally.
- Cross-reference coupled files against project notes (from 6.1) that reference those files.

### CLI Surface

```bash
# Mine coupling data
aether mine-coupling --commits 500

# Check blast radius
aether blast-radius crates/aether-store/src/lib.rs --min-risk medium

# Show most coupled file pairs
aether coupling-report --top 20
```

### Config

```toml
[coupling]
enabled = true
commit_window = 500
min_co_change_count = 3
exclude_patterns = ["*.lock", "*.generated.*", ".gitignore"]
```

### Edge Cases

| Scenario | Behavior |
|----------|----------|
| Not a git repo | `mining_state: null`, `coupled_files: []`, no error |
| Fewer than 10 commits | Mine all available, warn "limited history" |
| File not found in any commits | Return empty `coupled_files` |
| Merge commits | Skip merge commits (>1 parent), count only regular commits |
| Binary files in diff | Exclude from coupling analysis |
| Bulk formatting commit (50+ files changed) | Skip commits where >30 files changed (configurable) |

### Pass Criteria
1. Mining scans git history via `gix` and populates CozoDB `co_change_edges` with git_coupling scores.
2. Multi-signal enrichment computes static_signal from CozoDB dependency edges and semantic_signal from LanceDB SIR embeddings.
3. Fused_score correctly combines all three signals with specified weights.
4. `coupling_type` classification correctly identifies "hidden_operational" (high temporal, no static, low semantic).
5. Incremental mining picks up where it left off.
6. `aether_blast_radius` returns coupled files sorted by fused_score with per-signal breakdown.
7. Excluded patterns filter correctly.
8. Bulk commits (>30 files) are skipped.
9. Graceful degradation: if no SIR exists for a file, semantic_signal defaults to 0.0.
10. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

### Exact Codex Prompt
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_6_stage_6_2_temporal_coupling.md for the full specification.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase6-stage6-2-temporal-coupling off main.
3) Create worktree ../aether-phase6-stage6-2 for that branch and switch into it.
4) In crates/aether-store or a new module in aether-memory:
   - Add coupling_mining_state SQLite table and migration.
   - Add co_change_edges CozoDB relation (with git_coupling, static_signal, semantic_signal, fused_score, coupling_type fields).
   - Implement Phase 1 mining: walk gix commits, extract changed files, compute co-change pairs and git_coupling scores.
   - Implement Phase 2 enrichment: for each pair, query CozoDB for CALLS/DEPENDS_ON edges (static_signal), query LanceDB for max SIR embedding cosine similarity (semantic_signal).
   - Implement fused_score = 0.5*temporal + 0.3*static + 0.2*semantic.
   - Implement coupling_type classification (multi, structural, semantic, hidden_operational, temporal).
   - Implement incremental mining (resume from last_commit_hash).
5) Add MCP tool aether_blast_radius in crates/aether-mcp/src/lib.rs:
   - Input: file path + min_risk filter.
   - Output: coupled files with fused_score, per-signal breakdown, coupling_type, cross-referenced against project_notes file_refs.
   - Auto-mine if never mined or stale (>100 new commits).
6) Add CLI commands in crates/aetherd:
   - `aether mine-coupling --commits <n>`
   - `aether blast-radius <file> --min-risk <level>`
   - `aether coupling-report --top <n>`
7) Add [coupling] config section in aether-config.
8) Add tests:
   - Unit tests with synthetic git history (create temp repo with known co-changes).
   - Verify coupling scores match expected values.
   - Verify bulk commit filtering.
   - Integration test for MCP tool schema.
9) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
10) Commit with message: "Add temporal coupling detection with blast radius MCP tool"
```

---

## Stage 6.3 — Test Intent Extraction

### Purpose
Parse test files to extract behavioral intent strings (the human-readable descriptions of what tests verify) and link them as guards to the symbols/files they test. Surface these as guardrails when code changes.

### What It Borrows From
- **EngramPro:** Test intent extraction as behavioral guardrails. EngramPro extracts `it("should handle negative balance")` strings and surfaces them during impact analysis.

### Extraction Patterns (tree-sitter)

| Language | Test Pattern | Intent Source |
|----------|-------------|---------------|
| Rust | `#[test] fn test_name()` | Function name (converted: `test_handles_negative_balance` → "handles negative balance") |
| Rust | `#[test]` with doc comment `/// ...` | Doc comment text |
| TypeScript/JS | `it("should handle X", ...)` | First string argument |
| TypeScript/JS | `test("should handle X", ...)` | First string argument |
| TypeScript/JS | `describe("PaymentService", ...)` | First string argument (used as group label) |
| Python | `def test_handles_negative_balance(...)` | Function name (converted) |
| Python | `"""docstring"""` under test function | Docstring text |

### Schema

**SQLite: `test_intents` table**
```sql
CREATE TABLE IF NOT EXISTS test_intents (
    intent_id       TEXT PRIMARY KEY,     -- BLAKE3(file_path + test_name + intent_text)
    file_path       TEXT NOT NULL,
    test_name       TEXT NOT NULL,
    intent_text     TEXT NOT NULL,         -- Human-readable intent string
    group_label     TEXT,                  -- describe() group if applicable
    language        TEXT NOT NULL,
    symbol_id       TEXT,                  -- Symbol ID of test function (if indexed)
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);

CREATE INDEX idx_test_intents_file ON test_intents(file_path);
```

**CozoDB: `tested_by` relation**
```
:create tested_by {
    target_file: String,          -- File being tested (inferred)
    test_file: String,
    =>
    intent_count: Int,
    confidence: Float             -- How confident is the target_file inference
}
```

### Target File Inference
Given a test file, infer which production file(s) it tests:

1. **Naming convention:** `src/payment.rs` ↔ `tests/payment_test.rs`, `src/payment.ts` ↔ `src/payment.test.ts`, `src/__tests__/payment.ts`
2. **Import analysis:** Parse test file imports; production files that are imported are likely targets.
3. **Temporal coupling cross-reference (from 6.2):** If test file and production file have high co-change score, link them.
4. **Confidence:** Convention match = 0.9, Import match = 0.8, Coupling match = coupling_score * 0.7.

### Blast Radius Integration
When `aether_blast_radius` is called (from 6.2), enrich the response with test intents:

```json
{
  "coupled_files": [...],
  "test_guards": [
    {
      "test_file": "crates/aether-store/tests/store_tests.rs",
      "intents": [
        "handles empty symbol table",
        "returns none for missing symbol",
        "increments version on hash change"
      ],
      "confidence": 0.9,
      "inference_method": "naming_convention"
    }
  ]
}
```

### Edge Cases

| Scenario | Behavior |
|----------|----------|
| Test file with no intent strings (just `#[test] fn test_1()`) | Use function name converted to natural language |
| Test file that imports from multiple production files | Create `tested_by` edges for each, split confidence |
| Dynamically generated test names | Skip — only extract statically visible intents |
| Test in same file as production code (Rust `#[cfg(test)] mod tests`) | Target file = same file, confidence = 1.0 |

### Pass Criteria
1. Rust test functions produce extracted intent strings.
2. TypeScript `it()` / `test()` / `describe()` produce extracted intent strings.
3. Python test functions produce extracted intent strings.
4. Target file inference works for naming convention and import patterns.
5. `aether_blast_radius` includes `test_guards` when test intents exist.
6. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

### Exact Codex Prompt
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_6_stage_6_3_test_intents.md for the full specification.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase6-stage6-3-test-intents off main.
3) Create worktree ../aether-phase6-stage6-3 for that branch and switch into it.
4) In crates/aether-parse:
   - Add test intent extraction to each language plugin (Rust, TypeScript/JS, Python).
   - Extract function names, it()/test()/describe() string arguments, doc comments.
   - Return Vec<TestIntent> alongside existing symbol extraction.
5) In crates/aether-store:
   - Add test_intents SQLite table and migration.
   - Add tested_by CozoDB relation.
6) In crates/aether-memory or aether-store:
   - Implement target file inference (naming convention, import analysis, coupling cross-ref).
7) In crates/aether-mcp:
   - Extend aether_blast_radius response to include test_guards field.
   - Add aether_test_intents tool: query test intents for a file/symbol.
8) Add tests:
   - Parse test intent extraction for Rust #[test], TS it()/test(), Python def test_*.
   - Verify target file inference for naming convention and imports.
   - Verify blast_radius includes test_guards.
9) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
10) Commit with message: "Add test intent extraction with behavioral guards"
```

---

## Stage 6.4 — Session Context + Access Tracking

### Purpose
Let agents capture context during coding sessions and track what gets accessed, so relevance ranking improves over time.

### What It Borrows From
- **Engram:** Inbox-to-knowledge pipeline (but no sub-agent — direct MCP write to store)
- **Mem0:** Automatic memory extraction from conversations + access tracking
- **Fold:** ACT-R decay model (recently/frequently accessed memories surface first)

### MCP Tool: `aether_session_note`

**Request:**
```json
{
  "content": "Refactoring process_payment because the old approach was too slow for batch operations. The new version uses streaming instead of loading all records into memory.",
  "file_refs": ["src/payments/processor.rs"],
  "symbol_refs": ["abc123"],
  "tags": ["refactor", "performance"]
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "note_id": "x1y2z3...",
  "action": "created",
  "source_type": "session"
}
```

This is essentially `aether_remember` with `source_type = "session"` and optimized for in-flow usage (agents call it mid-task without interrupting their work).

### Access Tracking

Add to existing SIR metadata and project notes:

```sql
-- Already in project_notes from 6.1
-- Add to symbols table:
ALTER TABLE symbols ADD COLUMN access_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE symbols ADD COLUMN last_accessed_at INTEGER;
```

**When to increment:**
- `aether_get_sir` → increment symbol's `access_count`
- `aether_explain` → increment symbol's `access_count`
- `aether_recall` → increment returned notes' `access_count`
- `aether_blast_radius` → increment target file's symbols' `access_count`
- LSP hover → increment hovered symbol's `access_count`

**Ranking boost formula (applied in all search/recall):**
```
recency_factor = max(0, 1.0 - (now - last_accessed_at) / (30 * 24 * 3600 * 1000))
access_factor = ln(access_count + 1) / ln(100)  -- normalize to ~1.0 at 100 accesses
boosted_score = raw_score * (1.0 + 0.1 * recency_factor + 0.05 * access_factor)
```

Lightweight. No separate pass. Applied at query time in search ranking.

### Edge Cases

| Scenario | Behavior |
|----------|----------|
| Session note with empty content | Reject with MCP error |
| Session note referencing non-existent symbol | Accept — symbol_refs are advisory, not validated |
| Access count overflow | Cap at i64::MAX (won't happen in practice) |
| LSP hover spam (user scrolling through file) | Debounce: only count one access per symbol per 60 seconds |

### Pass Criteria
1. `aether_session_note` stores notes with `source_type = "session"`.
2. `access_count` and `last_accessed_at` update on symbol/note access.
3. Search results are boosted by recency and access frequency.
4. LSP hover access tracking is debounced (60s per symbol).
5. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

### Exact Codex Prompt
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_6_stage_6_4_session_context.md for the full specification.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase6-stage6-4-session-context off main.
3) Create worktree ../aether-phase6-stage6-4 for that branch and switch into it.
4) In crates/aether-mcp:
   - Add aether_session_note tool (thin wrapper over aether_remember with source_type="session").
5) In crates/aether-store:
   - Add access_count and last_accessed_at columns to symbols table (migration).
   - Add increment_access method for both symbols and project_notes.
   - Add debounce tracking (HashMap<symbol_id, Instant> in memory, not persisted).
6) In crates/aether-mcp and crates/aether-lsp:
   - Add access tracking calls to get_sir, explain, recall, blast_radius, hover.
7) In crates/aether-memory/src/search.rs:
   - Add recency + access boost to ranking formula.
8) Add tests:
   - Session note creation.
   - Access count incrementing.
   - Debounce behavior (second access within 60s doesn't increment).
   - Boosted ranking produces different order than raw ranking.
9) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
10) Commit with message: "Add session context capture and access tracking"
```

---

## Stage 6.5 — Memory Search + Unified Query

### Purpose
Provide a single MCP tool that searches across ALL AETHER knowledge — symbols, SIR, project notes, coupling data, test intents — with unified ranking. Also enrich LSP hover with project context.

### What It Borrows From
- **Fold:** Single search endpoint across all content types
- **Zep:** Unified retrieval combining graph traversal + vector search + keyword matching

### MCP Tool: `aether_ask`

The "I don't know what I'm looking for" tool. Searches everything.

**Request:**
```json
{
  "query": "payment processing retry logic",
  "limit": 10,
  "include": ["symbols", "notes", "coupling", "tests"]  -- default: all
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "query": "payment processing retry logic",
  "result_count": 4,
  "results": [
    {
      "kind": "symbol",
      "id": "abc123",
      "title": "process_payment_with_retry",
      "snippet": "Processes payment with exponential backoff retry...",
      "relevance_score": 0.92,
      "file": "src/payments/processor.rs",
      "language": "rust"
    },
    {
      "kind": "note",
      "id": "x1y2z3",
      "title": null,
      "snippet": "Refactoring process_payment because the old approach was too slow...",
      "relevance_score": 0.85,
      "tags": ["refactor", "performance"],
      "source_type": "session"
    },
    {
      "kind": "test_guard",
      "id": "t1u2v3",
      "title": "retries on transient failure",
      "snippet": "it(\"should retry 3 times on timeout\")",
      "relevance_score": 0.78,
      "test_file": "src/payments/processor.test.ts"
    },
    {
      "kind": "coupled_file",
      "id": null,
      "title": "src/payments/gateway.rs",
      "snippet": "Co-changes with processor.rs in 89% of commits (Critical coupling, type: multi)",
      "relevance_score": 0.71,
      "fused_score": 0.89,
      "coupling_type": "multi"
    }
  ]
}
```

**Search implementation:**
1. Run hybrid search on symbols (existing `aether_search`).
2. Run hybrid search on project notes (`aether_recall` from 6.1).
3. Run text match on test intents.
4. For top symbol results, fetch coupled files from CozoDB.
5. Merge all results, normalize scores to [0, 1], apply RRF fusion across result types.
6. Apply recency + access boost.
7. Return top N.

### LSP Hover Enrichment

When hovering a symbol that has associated project notes or coupling data, append a concise section:

```markdown
## `process_payment`

**Purpose:** Processes payment transactions with retry logic...

**Dependencies:** calls `validate_order`, `gateway.charge`

---
📝 *"Refactored for streaming — old approach loaded all records into memory"* (2d ago)
⚠️ *Co-changes with gateway.rs (89%, Critical)*
🧪 *3 test guards: retries on timeout, handles negative balance, logs audit trail*
```

Rules:
- Show at most 1 note (most relevant), 1 coupling warning (highest risk), 3 test intents.
- Only show if data exists — no empty sections.
- Compact: one line per item.

### Pass Criteria
1. `aether_ask` returns mixed results from symbols, notes, coupling, and test intents.
2. Results are ranked by unified relevance score with cross-type RRF.
3. LSP hover shows project context when available.
4. LSP hover shows nothing extra when no notes/coupling/tests exist (no regression).
5. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

### Exact Codex Prompt
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_6_stage_6_5_unified_query.md for the full specification.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase6-stage6-5-unified-query off main.
3) Create worktree ../aether-phase6-stage6-5 for that branch and switch into it.
4) In crates/aether-memory or crates/aether-mcp:
   - Implement unified search: query symbols, notes, test intents, coupling in parallel.
   - Implement cross-type RRF score normalization and merging.
5) In crates/aether-mcp:
   - Add aether_ask tool with request/response schema per spec.
6) In crates/aether-lsp:
   - Enrich hover output with project notes, coupling warnings, test intents.
   - Respect compactness rules: 1 note, 1 coupling, 3 test intents max.
   - No extra sections when no context data exists.
7) Add CLI command:
   - `aether ask <query> --limit <n>`
8) Add tests:
   - Unified search returns mixed result types.
   - LSP hover includes context when available.
   - LSP hover regression: no change when no context data.
   - Cross-type ranking produces sensible ordering.
9) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
10) Commit with message: "Add unified query and LSP hover enrichment"
```

---

## Stage 6.6 — Semantic Drift Detection

### Purpose
Automatically detect when code's *meaning* is changing incrementally — without anyone explicitly noting it — by comparing current SIR against historical SIR. Also detect architectural boundary violations by running community detection on the dependency graph and flagging new cross-community edges.

**This is a genuinely novel capability.** Architectural drift detection in existing tools (ArchUnit, jQAssistant) requires manually maintained architecture models. AETHER detects drift automatically from the code's own semantic history because it has versioned SIR annotations per symbol — something no other system maintains.

### What It Requires
- **SIR versioning** (Phase 2): `sir_history` table with previous SIR versions per symbol.
- **LanceDB embeddings** (Phase 4): SIR embeddings for computing semantic similarity over time.
- **CozoDB graph** (Phase 4): Dependency edges for community detection.
- **gix** (Phase 4): Commit range for bounding drift analysis.

### Drift Detection Algorithm

```
Semantic Drift (per-symbol):
1. For each symbol with SIR:
   a. Get current SIR embedding from LanceDB.
   b. Get SIR embedding from N commits ago (from sir_history, find sir_version closest to target commit).
   c. Compute cosine similarity between current and historical embedding.
   d. If similarity < drift_threshold (default 0.85): flag as "drifted."
   e. Record drift magnitude = 1.0 - similarity.
   f. Record drift period: commit range over which drift accumulated.

Boundary Violation Detection:
1. Run CozoDB Louvain community detection on the full dependency graph:
   ?[node, community] := community_detection_louvain(*dependency_edges[], node, community)
2. For each CALLS/DEPENDS_ON edge where source community ≠ target community:
   a. Check if this cross-community edge existed N commits ago (query sir_history for edge existence).
   b. If edge is NEW since the analysis window: flag as boundary violation.

Structural Anomaly Detection:
1. Hub detection: CozoDB PageRank on dependency graph. Symbols with PageRank > 95th percentile AND PageRank increased by >20% since last analysis = "emerging god objects."
2. Cycle detection: CozoDB SCC (strongly connected components) query. New cycles that didn't exist N commits ago = "emerging circular dependencies."
3. Orphan detection: connected components query. Subgraphs with no edge to the main component = "orphaned code candidates."
```

### Schema

**SQLite: `drift_analysis_state` table**
```sql
CREATE TABLE IF NOT EXISTS drift_analysis_state (
    id                  INTEGER PRIMARY KEY DEFAULT 1,
    last_analysis_commit TEXT,
    last_analysis_at    INTEGER,
    symbols_analyzed    INTEGER DEFAULT 0,
    drift_detected      INTEGER DEFAULT 0
);
```

**SQLite: `drift_results` table**
```sql
CREATE TABLE IF NOT EXISTS drift_results (
    result_id           TEXT PRIMARY KEY,   -- BLAKE3(symbol_id + analysis_commit)
    symbol_id           TEXT NOT NULL,
    file_path           TEXT NOT NULL,
    symbol_name         TEXT NOT NULL,
    drift_type          TEXT NOT NULL,      -- "semantic" | "boundary_violation" | "emerging_hub" | "new_cycle" | "orphaned"
    drift_magnitude     REAL,              -- 0.0-1.0 for semantic drift, null for structural
    current_sir_hash    TEXT,
    baseline_sir_hash   TEXT,
    commit_range_start  TEXT,
    commit_range_end    TEXT,
    detail_json         TEXT NOT NULL,      -- JSON with type-specific details
    detected_at         INTEGER NOT NULL,
    is_acknowledged     INTEGER NOT NULL DEFAULT 0  -- User can acknowledge/dismiss
);

CREATE INDEX idx_drift_results_type ON drift_results(drift_type);
CREATE INDEX idx_drift_results_file ON drift_results(file_path);
CREATE INDEX idx_drift_results_ack ON drift_results(is_acknowledged);
```

### MCP Tool: `aether_drift_report`

**Request:**
```json
{
  "window": "50 commits",           -- or "30d" or "since:a1b2c3d"
  "include": ["semantic", "boundary", "structural"],  -- default: all
  "min_drift_magnitude": 0.15,      -- only for semantic drift
  "include_acknowledged": false
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "analysis_window": {
    "from_commit": "a1b2c3d",
    "to_commit": "e4f5g6h",
    "commit_count": 50,
    "analyzed_at": 1708000000000
  },
  "summary": {
    "symbols_analyzed": 342,
    "semantic_drifts": 5,
    "boundary_violations": 2,
    "emerging_hubs": 1,
    "new_cycles": 0,
    "orphaned_subgraphs": 1
  },
  "semantic_drift": [
    {
      "symbol_id": "abc123",
      "symbol_name": "process_payment",
      "file": "src/payments/processor.rs",
      "drift_magnitude": 0.31,
      "similarity": 0.69,
      "drift_summary": "Function's purpose shifted from single-payment processing to batch payment orchestration over 12 commits",
      "commit_range": ["a1b2c3d", "e4f5g6h"],
      "test_coverage": {
        "has_tests": true,
        "test_count": 3,
        "intents": ["handles timeout", "validates amount", "logs transaction"]
      }
    }
  ],
  "boundary_violations": [
    {
      "source_symbol": "validate_order",
      "source_file": "src/orders/validator.rs",
      "source_community": 3,
      "target_symbol": "charge_card",
      "target_file": "src/payments/gateway.rs",
      "target_community": 7,
      "edge_type": "CALLS",
      "first_seen_commit": "c3d4e5f",
      "note": "New cross-module dependency: orders module now directly calls payments module"
    }
  ],
  "structural_anomalies": {
    "emerging_hubs": [
      {
        "symbol_id": "def456",
        "symbol_name": "AppContext",
        "file": "src/context.rs",
        "current_pagerank": 0.94,
        "previous_pagerank": 0.71,
        "dependents_count": 47,
        "note": "PageRank increased 32% — becoming a god object"
      }
    ],
    "new_cycles": [],
    "orphaned_subgraphs": [
      {
        "symbols": ["old_parser", "legacy_format"],
        "files": ["src/legacy/parser.rs"],
        "total_symbols": 2,
        "note": "No dependency edges to main application — dead code candidate"
      }
    ]
  }
}
```

### MCP Tool: `aether_acknowledge_drift`

**Request:**
```json
{
  "result_ids": ["r1", "r2"],
  "note": "Intentional — process_payment was deliberately expanded to handle batches per ADR-23"
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "acknowledged": 2,
  "note_created": true,
  "note_id": "n1a2b3..."
}
```

Acknowledged drift items get stored as project notes (6.1) and excluded from future reports unless `include_acknowledged: true`.

### CLI Surface

```bash
# Run drift analysis
aether drift-report --window "50 commits" --min-drift 0.15

# Acknowledge drift items
aether drift-ack <result_id> --note "Intentional per ADR-23"

# Show community structure
aether communities --format table
```

### Config

```toml
[drift]
enabled = true
drift_threshold = 0.85          # Cosine similarity below this = drift
analysis_window = "100 commits" # Default analysis window
auto_analyze = false            # Run drift analysis on every indexing pass
hub_percentile = 95             # PageRank percentile for hub detection
```

### Edge Cases

| Scenario | Behavior |
|----------|----------|
| Symbol has no SIR history (new symbol) | Skip — no baseline to compare against |
| Symbol's SIR was regenerated (not changed) | Same embedding → similarity ≈ 1.0 → no false positive |
| Fewer commits than window | Analyze all available, note "limited_history" |
| No dependency edges in CozoDB | Skip community/structural analysis, run semantic drift only |
| Embeddings disabled | Skip semantic drift, run structural analysis only |
| Very large codebase (>10K symbols) | Batch embedding comparisons; limit to symbols changed in window |

### Pass Criteria
1. Semantic drift detection flags symbols whose SIR embedding similarity dropped below threshold.
2. Community detection runs via CozoDB Louvain and identifies module boundaries.
3. Boundary violations correctly identify new cross-community dependency edges.
4. Hub detection flags symbols with rising PageRank above percentile threshold.
5. Cycle detection finds new strongly connected components.
6. Orphan detection identifies disconnected subgraphs.
7. `aether_acknowledge_drift` suppresses items from future reports and creates project notes.
8. Graceful degradation when embeddings or SIR history unavailable.
9. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

### Exact Codex Prompt
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_6_stage_6_6_semantic_drift.md for the full specification.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase6-stage6-6-semantic-drift off main.
3) Create worktree ../aether-phase6-stage6-6 for that branch and switch into it.
4) In crates/aether-store:
   - Add drift_analysis_state and drift_results SQLite tables and migrations.
   - Add query helpers: get SIR embedding at commit N for a symbol (join sir_history + LanceDB).
5) In crates/aether-memory or new module crates/aether-analysis:
   - Implement semantic drift detection:
     a. For symbols changed in window, get current embedding and baseline embedding.
     b. Compute cosine similarity, flag if below threshold.
   - Implement boundary violation detection:
     a. Run CozoDB Louvain community detection.
     b. Identify cross-community CALLS/DEPENDS_ON edges not present in baseline.
   - Implement structural anomaly detection:
     a. PageRank for hub detection (compare current vs. baseline percentile).
     b. SCC for new cycle detection.
     c. Connected components for orphan detection.
6) In crates/aether-mcp:
   - Add aether_drift_report tool with request/response schema per spec.
   - Add aether_acknowledge_drift tool that marks items acknowledged and creates project note.
7) Add CLI commands:
   - `aether drift-report --window <window> --min-drift <threshold>`
   - `aether drift-ack <result_id> --note <text>`
   - `aether communities --format table`
8) Add [drift] config section in aether-config.
9) Add tests:
   - Synthetic SIR history with known drift — verify detection.
   - Synthetic dependency graph with known communities — verify boundary violation detection.
   - Hub detection with synthetic PageRank data.
   - Acknowledge flow: drift item → acknowledged → excluded from report.
   - Graceful degradation when no SIR history or no embeddings.
10) Run:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test --workspace
11) Commit with message: "Add semantic drift detection with boundary and structural analysis"
```

---

## Stage 6.7 — Causal Change Chains

### Purpose
When something breaks, trace the *semantic* causal chain backward through dependencies to find which upstream change most likely caused it. `git blame` tells who changed a line. This tells you *which upstream semantic change* broke your downstream code — and *what specifically changed* about it.

**This is a genuinely novel capability.** Change Impact Graphs (2009, Orso et al.) propagated file-level changes through dependency graphs but had no semantic understanding — they couldn't tell *what* changed about a function's behavior. AETHER can say "validate_payment now rejects empty currency codes" because it has SIR diff.

### What It Requires
- **CozoDB dependency graph** (Phase 4): CALLS/DEPENDS_ON edges for backward traversal.
- **SIR versioning** (Phase 2): `sir_history` for detecting *what* changed semantically.
- **Multi-signal coupling** (Stage 6.2): fused_score for ranking candidates.
- **gix** (Phase 4): Commit timestamps for recency weighting.

### Algorithm

```
Input: target_symbol_id, lookback_window (default: "20 commits")

1. Get target symbol's current file and all direct + transitive upstream dependencies:
   upstream_symbols = CozoDB recursive Datalog query:
   ?[upstream, depth] :=
       dependency_edges[target_symbol_id, upstream, _], depth = 1
   ?[upstream, depth] :=
       upstream_symbols[mid, d], dependency_edges[mid, upstream, _], depth = d + 1, depth <= max_depth

2. For each upstream symbol:
   a. Query sir_history: did this symbol's SIR change within lookback_window?
   b. If yes:
      - Get before/after SIR text.
      - Compute SIR diff (structured comparison of purpose, edge_cases, dependencies fields).
      - Compute change_magnitude = 1.0 - cosine_similarity(before_embedding, after_embedding).
      - Get commit hash and timestamp of the change.

3. Rank upstream changes by:
   causal_score = recency_weight * coupling_strength * change_magnitude
   Where:
     recency_weight = 1.0 / (1.0 + days_since_change)
     coupling_strength = fused_score from co_change_edges (if exists), else 0.5 * (1.0 / depth)
     change_magnitude = SIR embedding distance (from step 2b)

4. Return top N candidates as causal chain, ordered by causal_score descending.
```

### MCP Tool: `aether_trace_cause`

**Request:**
```json
{
  "symbol": "process_payment",       -- symbol name or ID
  "file": "src/payments/processor.rs",  -- optional, helps disambiguate
  "lookback": "20 commits",
  "max_depth": 5,                    -- dependency graph traversal depth
  "limit": 5                         -- max candidates to return
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "target": {
    "symbol_id": "abc123",
    "symbol_name": "process_payment",
    "file": "src/payments/processor.rs"
  },
  "analysis_window": {
    "lookback": "20 commits",
    "max_depth": 5,
    "upstream_symbols_scanned": 23
  },
  "causal_chain": [
    {
      "rank": 1,
      "causal_score": 0.87,
      "symbol_id": "def456",
      "symbol_name": "validate_currency",
      "file": "src/payments/currency.rs",
      "dependency_path": ["process_payment", "validate_order", "validate_currency"],
      "depth": 2,
      "change": {
        "commit": "a1b2c3d",
        "author": "alice",
        "date": "2026-02-15T14:30:00Z",
        "change_magnitude": 0.42,
        "sir_diff": {
          "purpose_changed": true,
          "purpose_before": "Validates currency code is a recognized ISO 4217 value",
          "purpose_after": "Validates currency code is ISO 4217 AND not in sanctions blocklist",
          "edge_cases_added": ["Empty currency code now rejected (was previously defaulted to USD)"],
          "edge_cases_removed": []
        }
      },
      "coupling": {
        "fused_score": 0.63,
        "coupling_type": "multi"
      }
    },
    {
      "rank": 2,
      "causal_score": 0.54,
      "symbol_id": "ghi789",
      "symbol_name": "gateway_charge",
      "file": "src/payments/gateway.rs",
      "dependency_path": ["process_payment", "gateway_charge"],
      "depth": 1,
      "change": {
        "commit": "b2c3d4e",
        "author": "bob",
        "date": "2026-02-14T09:15:00Z",
        "change_magnitude": 0.28,
        "sir_diff": {
          "purpose_changed": false,
          "edge_cases_added": ["Now throws GatewayTimeoutError after 30s (was 60s)"],
          "edge_cases_removed": []
        }
      },
      "coupling": {
        "fused_score": 0.89,
        "coupling_type": "multi"
      }
    }
  ],
  "no_change_upstream": 21
}
```

### CLI Surface

```bash
# Trace cause of breakage
aether trace-cause process_payment --file src/payments/processor.rs --lookback "20 commits"

# Shorthand: trace from current file context
aether trace-cause --symbol-id abc123 --depth 3
```

### Edge Cases

| Scenario | Behavior |
|----------|----------|
| Target symbol has no dependencies | Return empty `causal_chain`, note: "no upstream dependencies" |
| No upstream SIR changes in window | Return empty `causal_chain`, note: "no semantic changes in window" |
| Circular dependency in graph | CozoDB handles cycles in recursive queries; `depth` limit prevents infinite traversal |
| Symbol not found | MCP error: "symbol not found, try aether_search to find it" |
| No SIR history for upstream symbol | Skip that symbol (can't compute diff), note in response |
| Very deep dependency chain (depth > 10) | Cap at max_depth, note "truncated at depth N" |

### Pass Criteria
1. CozoDB recursive Datalog query correctly traverses upstream dependencies to specified depth.
2. SIR diff correctly identifies changed purpose, edge_cases, and dependencies fields.
3. Change magnitude computed from embedding cosine similarity.
4. Causal score correctly combines recency, coupling, and change magnitude.
5. Results ordered by causal_score descending.
6. Dependency path shows actual traversal chain from target to upstream.
7. Graceful handling of cycles, missing SIR history, and zero-change windows.
8. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

### Exact Codex Prompt
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_6_stage_6_7_causal_chains.md for the full specification.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase6-stage6-7-causal-chains off main.
3) Create worktree ../aether-phase6-stage6-7 for that branch and switch into it.
4) In crates/aether-store or crates/aether-analysis:
   - Implement recursive upstream dependency query in CozoDB Datalog:
     Traverse CALLS/DEPENDS_ON edges backward from target symbol to max_depth.
   - Implement SIR diff: compare two SIR versions field-by-field (purpose, edge_cases, dependencies).
   - Implement change_magnitude from embedding cosine similarity (query LanceDB).
   - Implement causal_score = recency_weight * coupling_strength * change_magnitude.
   - Implement ranking and limit.
5) In crates/aether-mcp:
   - Add aether_trace_cause tool with request/response schema per spec.
   - Symbol resolution: accept name + file, or symbol_id directly.
6) Add CLI command:
   - `aether trace-cause <symbol_name> --file <path> --lookback <window> --depth <n>`
7) Add tests:
   - Synthetic dependency graph A→B→C with known SIR changes — verify correct causal chain.
   - Verify ranking: more recent + higher coupling + higher magnitude = higher score.
   - Verify cycle handling doesn't loop.
   - Verify graceful degradation with missing SIR history.
8) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
9) Commit with message: "Add causal change chain tracing with SIR diff"
```

---

## Stage 6.8 — Graph Health Metrics + Intent Verification

### Purpose
Two capabilities in one stage because both are primarily *queries on existing data* rather than new data pipelines.

**Graph Health Metrics:** Apply CozoDB's built-in graph algorithms to the dependency graph and combine results with SIR quality data to produce an actionable codebase health dashboard. No existing tool does this at the semantic level — SonarQube counts lines and cyclomatic complexity; AETHER can say "this function is the most critical in your codebase (PageRank 0.94), changed meaning 3 times this month (semantic drift), has no test guards, and sits at a module boundary violation."

**Intent Verification:** Before/after refactor comparison of SIR to detect unintended semantic changes — even when all tests pass. Tests verify *behavior*; AETHER verifies *intent preservation*.

### Part A: Graph Health Metrics

#### Algorithm

Run CozoDB built-in graph algorithms on the `dependency_edges` relation, then enrich with SIR and access data.

```
1. PageRank — Most critical symbols:
   ?[symbol, rank] := pagerank(*dependency_edges[], symbol, rank)
   → Symbols everything depends on. Highest blast radius on failure.

2. Community Detection (Louvain) — Actual module boundaries:
   ?[symbol, community] := community_detection_louvain(*dependency_edges[], symbol, community)
   → Compare communities vs. directory structure. Symbols logically together but physically scattered.

3. Betweenness Centrality — Bottleneck symbols:
   ?[symbol, centrality] := betweenness_centrality(*dependency_edges[], symbol, centrality)
   → If these break, most paths through the codebase are disrupted.

4. Cycle Detection (SCC) — Circular dependencies:
   ?[symbol, component] := strongly_connected_components(*dependency_edges[], symbol, component)
   → Components with >1 member are circular dependency clusters.

5. Connected Components — Orphaned code:
   ?[symbol, component] := connected_components(*dependency_edges[], symbol, component)
   → Components not connected to main application entry points = dead code candidates.
```

#### Enrichment: Cross-Layer Risk Score

For each symbol, combine graph metrics with SIR quality and access data:

```
risk_score = weighted combination of:
  - pagerank (high = more critical, more risk if it fails)
  - has_sir (false = undocumented critical code)
  - test_coverage (from 6.3 tested_by edges: 0 tests = higher risk)
  - drift_magnitude (from 6.6: drifting symbols are riskier)
  - access_recency (recently accessed = actively used, higher impact)

Composite:
  risk = 0.3 * pagerank_normalized
       + 0.25 * (1.0 - test_coverage_ratio)
       + 0.2 * drift_magnitude
       + 0.15 * (1.0 if no_sir else 0.0)
       + 0.1 * access_recency_factor
```

A symbol with high PageRank, no tests, recent semantic drift, and no SIR = **ticking time bomb.**

#### MCP Tool: `aether_health`

**Request:**
```json
{
  "include": ["critical_symbols", "bottlenecks", "cycles", "orphans", "risk_hotspots"],
  "limit": 10,
  "min_risk": 0.5
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "analysis": {
    "total_symbols": 342,
    "total_edges": 1847,
    "communities_detected": 8,
    "cycles_detected": 2,
    "orphaned_subgraphs": 3,
    "analyzed_at": 1708000000000
  },
  "critical_symbols": [
    {
      "symbol_id": "abc123",
      "symbol_name": "AppContext",
      "file": "src/context.rs",
      "pagerank": 0.94,
      "betweenness": 0.82,
      "dependents_count": 47,
      "has_sir": true,
      "test_count": 2,
      "drift_magnitude": 0.0,
      "risk_score": 0.71,
      "risk_factors": ["high pagerank", "low test coverage relative to criticality"]
    }
  ],
  "bottlenecks": [
    {
      "symbol_id": "def456",
      "symbol_name": "database_pool",
      "file": "src/db/pool.rs",
      "betweenness": 0.91,
      "pagerank": 0.67,
      "note": "91% of dependency paths pass through this symbol"
    }
  ],
  "cycles": [
    {
      "cycle_id": 1,
      "symbols": [
        {"id": "g1", "name": "parse_config", "file": "src/config/parser.rs"},
        {"id": "g2", "name": "validate_config", "file": "src/config/validator.rs"},
        {"id": "g3", "name": "resolve_defaults", "file": "src/config/defaults.rs"}
      ],
      "edge_count": 3,
      "note": "Circular: parse_config → validate_config → resolve_defaults → parse_config"
    }
  ],
  "orphans": [
    {
      "subgraph_id": 1,
      "symbols": [
        {"id": "h1", "name": "old_parser", "file": "src/legacy/parser.rs"},
        {"id": "h2", "name": "legacy_format", "file": "src/legacy/format.rs"}
      ],
      "note": "No inbound dependencies from main application — dead code candidate"
    }
  ],
  "risk_hotspots": [
    {
      "symbol_id": "jkl012",
      "symbol_name": "process_payment",
      "file": "src/payments/processor.rs",
      "risk_score": 0.88,
      "risk_factors": [
        "pagerank 0.78 (top 5%)",
        "semantic drift 0.31 over last 50 commits",
        "only 2 test guards for 7 edge cases in SIR",
        "boundary violation: calls into 2 other communities"
      ]
    }
  ]
}
```

### Part B: Intent Verification

#### Purpose
After a refactor, compare pre-refactor SIR snapshots against post-refactor SIR to detect unintended semantic changes — even when all tests pass.

#### Workflow
1. **Before refactor:** Agent (or human) calls `aether_snapshot_intent` to capture current SIR state for affected symbols.
2. **After refactor:** Agent calls `aether_verify_intent` to compare current SIR against snapshot.
3. **Result:** Symbols where intent was preserved vs. shifted, with specific before/after comparison and test coverage gap analysis.

#### MCP Tool: `aether_snapshot_intent`

**Request:**
```json
{
  "scope": "file",                    -- "file" | "symbol" | "directory"
  "target": "src/payments/processor.rs",
  "label": "pre-batch-refactor"       -- Human label for this snapshot
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "snapshot_id": "snap_a1b2c3",
  "label": "pre-batch-refactor",
  "symbols_captured": 8,
  "created_at": 1708000000000
}
```

**Storage:** Snapshots stored in SQLite `intent_snapshots` table:
```sql
CREATE TABLE IF NOT EXISTS intent_snapshots (
    snapshot_id     TEXT PRIMARY KEY,
    label           TEXT NOT NULL,
    scope           TEXT NOT NULL,     -- "file" | "symbol" | "directory"
    target          TEXT NOT NULL,     -- file path, symbol ID, or directory path
    symbols_json    TEXT NOT NULL,     -- JSON array of {symbol_id, sir_hash, sir_text, embedding}
    created_at      INTEGER NOT NULL
);
```

#### MCP Tool: `aether_verify_intent`

**Request:**
```json
{
  "snapshot_id": "snap_a1b2c3",
  "regenerate_sir": true              -- Re-run SIR generation on changed symbols before comparing
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "snapshot_id": "snap_a1b2c3",
  "label": "pre-batch-refactor",
  "verification": {
    "symbols_checked": 8,
    "intent_preserved": 6,
    "intent_shifted": 2,
    "symbols_removed": 0,
    "symbols_added": 1
  },
  "preserved": [
    {
      "symbol_id": "abc123",
      "symbol_name": "validate_order",
      "similarity": 0.97,
      "status": "preserved"
    }
  ],
  "shifted": [
    {
      "symbol_id": "def456",
      "symbol_name": "process_payment",
      "similarity": 0.62,
      "status": "shifted",
      "before_purpose": "Processes a single payment transaction with retry logic",
      "after_purpose": "Orchestrates batch payment processing with parallel gateway calls",
      "before_edge_cases": ["timeout after 3 retries", "negative amount rejected"],
      "after_edge_cases": ["batch size > 1000 triggers chunking", "partial batch failure returns partial results", "negative amount rejected"],
      "test_coverage_gap": {
        "existing_tests": ["handles timeout", "validates amount"],
        "untested_new_intents": ["batch size chunking", "partial batch failure handling"],
        "recommendation": "Add tests for batch-specific edge cases"
      }
    }
  ],
  "added": [
    {
      "symbol_id": "new789",
      "symbol_name": "batch_chunker",
      "file": "src/payments/processor.rs",
      "note": "New symbol not in original snapshot — verify test coverage"
    }
  ]
}
```

#### Intent Similarity Thresholds
```
similarity >= 0.90 → "preserved" (intent fundamentally unchanged)
similarity >= 0.70 → "shifted_minor" (intent adjusted but recognizable)
similarity < 0.70  → "shifted_major" (intent substantially changed)
```

### CLI Surface

```bash
# Graph health dashboard
aether health --limit 10 --min-risk 0.5

# Show critical symbols
aether health critical --top 10

# Show cycles
aether health cycles

# Show orphaned code
aether health orphans

# Intent verification workflow
aether snapshot-intent --file src/payments/processor.rs --label "pre-refactor"
# ... do refactor ...
aether verify-intent snap_a1b2c3 --regenerate-sir
```

### Config

```toml
[health]
enabled = true
risk_weights = { pagerank = 0.3, test_gap = 0.25, drift = 0.2, no_sir = 0.15, recency = 0.1 }

[intent]
enabled = true
similarity_preserved_threshold = 0.90
similarity_shifted_threshold = 0.70
auto_regenerate_sir = true      # Regenerate SIR for changed symbols before comparison
```

### Edge Cases

| Scenario | Behavior |
|----------|----------|
| No dependency edges in CozoDB | Skip graph metrics, return empty results with note |
| Symbol removed during refactor | Listed in `symbols_removed` with its original SIR |
| New symbol added during refactor | Listed in `symbols_added`, not compared (no baseline) |
| Snapshot references symbol whose SIR was never generated | Skip symbol, note "no SIR at snapshot time" |
| Intent verification without prior snapshot | MCP error: "no snapshot found, use aether_snapshot_intent first" |
| CozoDB built-in algorithm not available | Fall back to manual implementation (PageRank iteration, Tarjan's SCC); log warning |
| Very large graph (>50K edges) | CozoDB handles this natively; add timeout (30s default) |
| Embeddings disabled | Graph health works (structure only); intent verification degrades to text diff |

### Pass Criteria
1. PageRank, community detection, betweenness centrality, SCC, and connected components queries execute correctly on CozoDB.
2. Risk score correctly combines pagerank, test coverage, drift, SIR presence, and access recency.
3. Risk hotspots surface symbols with multiple risk factors.
4. Cycle detection finds actual circular dependency chains.
5. Orphan detection identifies disconnected subgraphs.
6. `aether_snapshot_intent` captures SIR state for all symbols in scope.
7. `aether_verify_intent` correctly classifies preserved vs. shifted intents based on similarity threshold.
8. Test coverage gap analysis correctly identifies untested new edge cases.
9. New and removed symbols reported correctly.
10. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

### Exact Codex Prompt
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_6_stage_6_8_graph_health_intent.md for the full specification.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase6-stage6-8-graph-health-intent off main.
3) Create worktree ../aether-phase6-stage6-8 for that branch and switch into it.

PART A — Graph Health Metrics:
4) In crates/aether-analysis (or crates/aether-store):
   - Implement CozoDB graph queries:
     a. PageRank on dependency_edges.
     b. Louvain community detection.
     c. Betweenness centrality.
     d. Strongly connected components (cycle detection).
     e. Connected components (orphan detection).
   - Implement risk score computation:
     Cross-reference PageRank with test_coverage (from tested_by), drift_magnitude (from drift_results),
     SIR presence (from symbols), and access recency (from symbols.last_accessed_at).
   - Implement risk_factors human-readable explanation generation.
5) In crates/aether-mcp:
   - Add aether_health tool with request/response schema per spec.

PART B — Intent Verification:
6) In crates/aether-store:
   - Add intent_snapshots SQLite table and migration.
7) In crates/aether-analysis or crates/aether-memory:
   - Implement snapshot_intent: capture symbol SIR state (text + embedding) for scope.
   - Implement verify_intent:
     a. Load snapshot symbols.
     b. Get current SIR for each symbol (optionally regenerate via aether-infer).
     c. Compute cosine similarity between snapshot embedding and current embedding.
     d. Classify: preserved (>=0.90), shifted_minor (>=0.70), shifted_major (<0.70).
     e. For shifted symbols: compute SIR text diff (purpose, edge_cases fields).
     f. Cross-reference against test_intents: identify untested new edge cases.
8) In crates/aether-mcp:
   - Add aether_snapshot_intent tool.
   - Add aether_verify_intent tool.
9) Add CLI commands:
   - `aether health [critical|cycles|orphans] --limit <n> --min-risk <threshold>`
   - `aether snapshot-intent --file <path> --label <label>`
   - `aether verify-intent <snapshot_id> [--regenerate-sir]`
10) Add [health] and [intent] config sections.
11) Add tests:
    - Graph metrics on synthetic dependency graph with known PageRank, cycles, orphans.
    - Risk score computation with mocked metrics.
    - Snapshot + verify workflow: create snapshot, modify SIR, verify drift detected.
    - Test coverage gap: symbol with new edge cases not covered by tests.
    - Graceful degradation when no graph data or no embeddings.
12) Run:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test --workspace
13) Commit with message: "Add graph health metrics and intent verification"
```

---

## Phase 6 Summary

### Capability Breakdown: Borrowed vs. Novel

| Stage | Capability | Classification |
|-------|-----------|----------------|
| 6.1 | Project Memory Store | **Borrowed** — Fold, Mem0, Engram all do this. AETHER's local-first embedded delivery is cleaner, but capability isn't new. Table-stakes infrastructure. |
| 6.2 | Multi-Signal Coupling | **NOVEL** — Three-signal fusion (git temporal + AST static + SIR semantic) is something nobody does. Academic papers use one signal. EngramPro uses git only. The disagreement detection (hidden operational coupling) is original. |
| 6.3 | Test Intent Extraction | **Enhanced** — EngramPro's concept, but AETHER adds AST-level precision via tree-sitter and graph linkage via CozoDB tested_by edges. Better engineering of an existing idea. |
| 6.4 | Session Context + Access Tracking | **Borrowed** — Engram's inbox concept + Fold's ACT-R decay model. Same thing, structured store instead of markdown. Table-stakes infrastructure. |
| 6.5 | Unified Query | **Enhanced** — Fold does unified search. AETHER can traverse the dependency graph while searching, which Fold can't. Also, LSP hover enrichment with project context is genuinely useful. |
| 6.6 | Semantic Drift Detection | **NOVEL** — Nobody detects drift automatically from code's own semantic history. Existing tools (ArchUnit) require manually maintained architecture models. AETHER detects drift because it has versioned SIR per symbol. Community detection on dependency graph for boundary violations is also novel in this context. |
| 6.7 | Causal Change Chains | **NOVEL** — Graph traversal × temporal SIR diff for root-cause tracing. Change Impact Graphs (2009) propagated file-level changes with no semantic understanding. AETHER can say exactly *what* changed about a function's behavior. |
| 6.8 | Graph Health + Intent Verification | **NOVEL** — CozoDB graph algorithms (PageRank, betweenness, SCC, Louvain) on the dependency graph enriched with SIR quality + test coverage + drift data. No existing tool operates at the semantic graph level. Intent verification (pre/post-refactor SIR comparison) doesn't exist elsewhere. |

**Net assessment:** Stages 6.1 and 6.4 are table-stakes infrastructure that every memory system has. Stages 6.6, 6.7, and 6.8 are genuinely novel capabilities that **only work because AETHER has AST + SIR + graph + git + vectors in the same process.** These are cross-layer queries, not single-layer features.

### New MCP Tools (11 total)

| Tool | Stage | Purpose | Novel? |
|------|-------|---------|--------|
| `aether_remember` | 6.1 | Store project note with dedup | Borrowed |
| `aether_recall` | 6.1 | Search project notes | Borrowed |
| `aether_blast_radius` | 6.2 | Multi-signal coupling + risk analysis | **Novel** (3-signal fusion) |
| `aether_test_intents` | 6.3 | Query test guards for a file/symbol | Enhanced |
| `aether_session_note` | 6.4 | Quick context capture during sessions | Borrowed |
| `aether_ask` | 6.5 | Unified search across all knowledge | Enhanced |
| `aether_drift_report` | 6.6 | Semantic drift + boundary violations | **Novel** |
| `aether_acknowledge_drift` | 6.6 | Dismiss known drift, create project note | **Novel** |
| `aether_trace_cause` | 6.7 | Causal change chain tracing | **Novel** |
| `aether_health` | 6.8 | Graph health dashboard with risk scoring | **Novel** |
| `aether_snapshot_intent` | 6.8 | Capture SIR state before refactor | **Novel** |
| `aether_verify_intent` | 6.8 | Compare SIR state after refactor | **Novel** |

### New CLI Commands

| Command | Stage |
|---------|-------|
| `aether remember` | 6.1 |
| `aether recall` | 6.1 |
| `aether notes` | 6.1 |
| `aether mine-coupling` | 6.2 |
| `aether blast-radius` | 6.2 |
| `aether coupling-report` | 6.2 |
| `aether ask` | 6.5 |
| `aether drift-report` | 6.6 |
| `aether drift-ack` | 6.6 |
| `aether communities` | 6.6 |
| `aether trace-cause` | 6.7 |
| `aether health` | 6.8 |
| `aether snapshot-intent` | 6.8 |
| `aether verify-intent` | 6.8 |

### New/Modified Crates

| Crate | Change |
|-------|--------|
| `aether-memory` | **NEW** — project notes, session context, memory search |
| `aether-analysis` | **NEW** — drift detection, causal chains, graph health, intent verification. Depends on aether-store, aether-infer, aether-memory. Domain-agnostic. |
| `aether-store` | New tables: `project_notes`, `coupling_mining_state`, `test_intents`, `drift_analysis_state`, `drift_results`, `intent_snapshots`. New columns: `symbols.access_count`, `symbols.last_accessed_at`. New CozoDB relations: `co_change_edges` (with multi-signal fields), `tested_by`. New LanceDB table: `project_notes_vectors`. |
| `aether-parse` | Test intent extraction added to language plugins |
| `aether-mcp` | 11 new tools |
| `aether-lsp` | Hover enrichment with notes/coupling/tests |
| `aether-config` | New `[coupling]`, `[memory]`, `[drift]`, `[health]`, `[intent]` config sections |
| `aetherd` | New CLI subcommands |

### Legal-Readiness Checklist

These design choices in Phase 6 specifically enable the future legal product:

- [x] `project_notes.entity_refs` uses generic `{kind, id}` — works for code symbols, legal clauses, contract parties
- [x] `project_notes.source_type` distinguishes manual vs. agent vs. session vs. import — legal can add "extraction" type
- [x] `aether-memory` crate has zero code-specific logic — can be used by legal daemon unchanged
- [x] `aether-analysis` crate is domain-agnostic — graph health works on any dependency graph, intent verification works on any structured annotation
- [x] Hybrid search (lexical + semantic + RRF) is domain-agnostic
- [x] Access tracking and recency boost are domain-agnostic
- [x] CozoDB graph stores generic relations — `tested_by` is code-specific, but `co_change_edges` pattern applies to legal document amendment chains
- [x] Content-hash dedup prevents duplicate notes regardless of domain
- [x] Drift detection pattern applies to legal: detect when contract clause interpretations drift over amendment cycles
- [x] Causal chain pattern applies to legal: trace which amendment caused a compliance exposure
- [x] Intent verification pattern applies to legal: verify that contract amendments preserve original deal terms

### What Phase 7 Adds for Legal

| Phase 7 Feature | Legal Application |
|-----------------|-------------------|
| Bi-temporal fact tracking | "What was the indemnity cap effective March 15?" — contracts have validity periods |
| Memory consolidation | Merge duplicate clause interpretations from different review sessions |
| Multi-agent scoping | Multiple attorneys annotating same contract corpus with attribution |

### Recommended Split Point

**Phase 8: Create `aether-legal-parse` crate.**

That's when you need PDF/DOCX parsing (not tree-sitter), clause extraction prompts (not code SIR), and legal-specific entity types (parties, obligations, monetary terms). At that point:

1. Extract shared crates into a `aether-engine` virtual workspace group.
2. Create `aether-legal-parse`, `aether-legal-sir`, `aether-legal-mcp` crates.
3. Create `aether-legal-d` binary.
4. Same repo, same CI, different release artifacts.
5. BSL license on shared engine, proprietary on legal-specific crates.
