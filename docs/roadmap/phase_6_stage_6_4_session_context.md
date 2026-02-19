# Phase 6 — The Chronicler

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
