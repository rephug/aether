# Phase 6 — The Chronicler

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
