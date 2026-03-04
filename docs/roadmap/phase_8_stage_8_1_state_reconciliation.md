# Phase 8 — Stage 8.1: State Reconciliation Engine

**Prerequisites:** Hardening passes 1–6 merged, Phase 7 complete
**Estimated Codex Runs:** 1 (single-pass, ~400 lines of changes)
**Risk Level:** Medium — touches write paths across all three stores

---

## Purpose

AETHER uses three databases: SQLite (relational/metadata/SIR), LanceDB (vectors), and SurrealDB (graph). The SIR pipeline currently writes to each database independently with no coordination. If `aetherd` crashes mid-pipeline (SIGKILL, OOM, power loss), the system enters a split-brain state:

- Symbol in SQLite with SIR, but no vector in LanceDB → semantic search misses it
- Symbol in SQLite, vector in LanceDB, but no graph node in SurrealDB → graph queries miss it
- Graph edge in SurrealDB referencing a symbol deleted from SQLite → phantom edges

Stage 8.1 guarantees eventual consistency across all stores via a Write-Ahead Intent Log and a state verification tool.

---

## Design

### Write-Ahead Intent Log (WAL)

Before writing to the three DBs, log an "intent to write" in SQLite. This creates a single source of truth for in-flight operations.

**New SQLite table:**
```sql
CREATE TABLE IF NOT EXISTS write_intents (
    intent_id TEXT PRIMARY KEY,
    symbol_id TEXT NOT NULL,
    file_path TEXT NOT NULL,
    operation TEXT NOT NULL,          -- 'upsert_sir' | 'delete_symbol' | 'update_edges'
    status TEXT NOT NULL DEFAULT 'pending',  -- 'pending' | 'sqlite_done' | 'vector_done' | 'graph_done' | 'complete' | 'failed'
    payload_json TEXT,                -- serialized operation payload for replay
    created_at INTEGER NOT NULL,
    completed_at INTEGER,
    error_message TEXT
);

CREATE INDEX IF NOT EXISTS idx_write_intents_status ON write_intents(status);
CREATE INDEX IF NOT EXISTS idx_write_intents_created ON write_intents(created_at);
```

**Schema version:** Increment to version 3 in `run_migrations()`.

### Coordinated Write Flow

The SIR pipeline currently does:
```
1. Generate SIR (inference)
2. Store SIR in SQLite
3. Generate embedding
4. Store embedding in LanceDB
5. Upsert graph node + edges in SurrealDB
```

New flow:
```
1. Generate SIR (inference)
2. Create write_intent in SQLite (status = 'pending', payload = serialized SIR + metadata)
3. Store SIR in SQLite → update intent status = 'sqlite_done'
4. Generate embedding
5. Store embedding in LanceDB → update intent status = 'vector_done'
6. Upsert graph node + edges in SurrealDB → update intent status = 'graph_done'
7. Mark intent status = 'complete', set completed_at
```

If any step fails:
- Log the error on the intent record (status = 'failed', error_message = details)
- Continue processing other symbols (don't block the pipeline)
- Failed intents are retried on next daemon startup or via `aether fsck --repair`

### Intent Replay on Startup

When `aetherd` starts, before entering the watch loop:
```rust
1. Query: SELECT * FROM write_intents WHERE status != 'complete' AND status != 'failed' ORDER BY created_at ASC
2. For each incomplete intent:
   a. Read payload_json to determine what was attempted
   b. Resume from the last successful step:
      - 'pending' → replay from step 2 (SIR already generated, stored in payload)
      - 'sqlite_done' → replay from step 4 (generate embedding, store vector, update graph)
      - 'vector_done' → replay from step 6 (upsert graph only)
      - 'graph_done' → mark 'complete' (all writes succeeded, just didn't mark complete)
3. Log summary: "Recovered N incomplete write intents from previous session"
```

### Stale Intent Cleanup

Intents older than 7 days in 'complete' status are pruned automatically on startup. Intents in 'failed' status are kept until manually reviewed or repaired.

### The `aether fsck` Command

New CLI subcommand that performs cross-database consistency verification:

```
aether fsck [--repair] [--verbose]
```

**Checks performed:**

1. **SQLite → LanceDB consistency:**
   - For each symbol with SIR in SQLite, verify a corresponding vector exists in LanceDB
   - Report: "N symbols missing vectors" (orphaned SIR)

2. **SQLite → SurrealDB consistency:**
   - For each symbol in SQLite, verify a corresponding graph node exists in SurrealDB
   - Report: "N symbols missing graph nodes" (orphaned symbols)

3. **SurrealDB → SQLite consistency:**
   - For each graph node in SurrealDB, verify the symbol exists in SQLite
   - Report: "N phantom graph nodes" (orphaned graph data)

4. **SurrealDB edge integrity:**
   - For each edge in `depends_on`, verify both source and target symbols exist
   - Report: "N dangling edges" (referencing deleted symbols)

5. **LanceDB → SQLite consistency:**
   - For each vector in LanceDB, verify the symbol still exists in SQLite
   - Report: "N orphaned vectors" (stale embeddings)

6. **Write intent cleanup:**
   - Report incomplete intents (not 'complete' or 'failed')
   - With `--repair`: attempt to replay incomplete intents

**`--repair` mode:**
- Orphaned SIR (no vector): Queue symbol for re-embedding
- Orphaned symbols (no graph node): Queue for graph upsert
- Phantom graph nodes: Delete from SurrealDB
- Dangling edges: Delete from SurrealDB
- Orphaned vectors: Delete from LanceDB
- Incomplete intents: Replay from last successful step

**Output format:**
```
AETHER State Verification Report
=================================
Symbols in SQLite:        12,847
Vectors in LanceDB:       12,501
Graph nodes in SurrealDB: 12,843

Inconsistencies found:
  Symbols missing vectors:      346 (queued for repair)
  Symbols missing graph nodes:    4 (queued for repair)
  Phantom graph nodes:            0
  Dangling edges:                12 (removed)
  Orphaned vectors:               0
  Incomplete write intents:       3 (replayed)

Repair complete. Run `aether fsck` again to verify.
```

---

## Files Modified

| File | Action | Description |
|------|--------|-------------|
| `crates/aether-store/src/sqlite.rs` | **Modify** | Add `write_intents` table migration (v3), CRUD methods |
| `crates/aether-store/src/lib.rs` | **Modify** | Add `WriteIntent`, `WriteIntentStatus`, `IntentOperation` types |
| `crates/aetherd/src/sir_pipeline.rs` | **Modify** | Wrap writes in intent log flow |
| `crates/aetherd/src/fsck.rs` | **Create** | Cross-database verification + repair logic |
| `crates/aetherd/src/cli.rs` | **Modify** | Add `fsck` subcommand |
| `crates/aetherd/src/indexer.rs` | **Modify** | Call intent replay on startup |
| `crates/aetherd/src/lib.rs` | **Modify** | Re-export fsck module |

---

## Edge Cases

| Scenario | Behavior |
|----------|----------|
| Crash during intent creation itself | Intent doesn't exist → symbol simply re-processed on next file change |
| SQLite WAL corruption | `aether fsck` falls back to pairwise cross-referencing (SQLite↔LanceDB, SQLite↔SurrealDB) without relying on intent log |
| LanceDB table doesn't exist yet | Skip vector consistency check for that provider/model combo |
| SurrealDB connection fails during fsck | Report error, continue with SQLite↔LanceDB checks |
| Thousands of failed intents (bad model config) | `aether fsck --repair` processes in batches of 100, reports progress |
| Concurrent aetherd + fsck | fsck uses read-only SQLite connection; repair operations use separate write connection with WAL mode |
| Mock provider (no real SIR) | Mock provider still writes to SQLite; intent log works the same way |
| Delete operation (symbol removed from source) | Intent operation = 'delete_symbol'; cleanup removes from all three stores |

---

## Pass Criteria

1. `write_intents` table is created on migration to schema version 3.
2. SIR pipeline creates intent before writing, updates status after each store write.
3. On daemon startup, incomplete intents from previous session are replayed.
4. `aether fsck` reports cross-database inconsistencies accurately.
5. `aether fsck --repair` fixes orphaned records and replays incomplete intents.
6. Existing MCP tools, LSP, dashboard, and CLI commands are unaffected.
7. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings` pass.
8. Per-crate tests pass (see session context for test order).

---

## Codex Prompt

```text
==========BEGIN CODEX PROMPT==========

CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- export CARGO_TARGET_DIR=/home/rephu/aether-target
- export CARGO_BUILD_JOBS=2
- export PROTOC=$(which protoc)
- export RUSTC_WRAPPER=sccache
- export TMPDIR=/home/rephu/aether-target/tmp
- mkdir -p $TMPDIR
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_8_stage_8_1_state_reconciliation.md for the full specification.
Read docs/roadmap/phase8_session_context.md for current architecture context.

PREFLIGHT

1) Ensure working tree is clean (`git status --porcelain`). If not, stop and report.
2) `git pull --ff-only` — ensure main is up to date.

BRANCH + WORKTREE

3) Create branch feature/phase8-stage8-1-state-reconciliation off main.
4) Create worktree ../aether-phase8-stage8-1 for that branch and switch into it.
5) Set build environment (copy the exports from the top of this prompt).

NOTE ON THREE-DATABASE ARCHITECTURE:
- SQLite: `crates/aether-store/src/sqlite.rs` — relational metadata, SIR storage
- LanceDB: `crates/aether-store/src/vector.rs` — vector embeddings
- SurrealDB: `crates/aether-store/src/graph_surreal.rs` — dependency graph
- SharedState: `crates/aether-mcp/src/state.rs` — holds Arc refs to all three

NOTE ON SIR PIPELINE:
- `crates/aetherd/src/sir_pipeline.rs` — processes symbol change events
- Currently writes to SQLite, then LanceDB, then SurrealDB with no coordination
- Each write is independent; crash between writes = split-brain state

NOTE ON SCHEMA MIGRATION:
- Current schema version is 2 (set in `run_migrations()` in sqlite.rs)
- Increment to version 3 for the write_intents table
- Use same migration pattern: check PRAGMA user_version, apply if < 3

=== STEP 1: Add Types ===

6) In `crates/aether-store/src/lib.rs`, add types:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteIntent {
    pub intent_id: String,
    pub symbol_id: String,
    pub file_path: String,
    pub operation: IntentOperation,
    pub status: WriteIntentStatus,
    pub payload_json: Option<String>,
    pub created_at: i64,
    pub completed_at: Option<i64>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WriteIntentStatus {
    Pending,
    SqliteDone,
    VectorDone,
    GraphDone,
    Complete,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IntentOperation {
    UpsertSir,
    DeleteSymbol,
    UpdateEdges,
}
```

Add Display/FromStr impls for WriteIntentStatus and IntentOperation so they
can be stored as TEXT in SQLite.

=== STEP 2: SQLite Migration ===

7) In `crates/aether-store/src/sqlite.rs`, in `run_migrations()`:
   - Add migration for version 2 → 3
   - Create write_intents table with columns: intent_id (TEXT PK), symbol_id (TEXT NOT NULL),
     file_path (TEXT NOT NULL), operation (TEXT NOT NULL), status (TEXT NOT NULL DEFAULT 'pending'),
     payload_json (TEXT), created_at (INTEGER NOT NULL), completed_at (INTEGER),
     error_message (TEXT)
   - Add indexes on status and created_at
   - Update PRAGMA user_version to 3 and schema_version row

8) Add SqliteStore methods:
   - `create_write_intent(&self, intent: &WriteIntent) -> Result<()>`
   - `update_intent_status(&self, intent_id: &str, status: WriteIntentStatus) -> Result<()>`
   - `mark_intent_failed(&self, intent_id: &str, error: &str) -> Result<()>`
   - `mark_intent_complete(&self, intent_id: &str) -> Result<()>`
   - `get_incomplete_intents(&self) -> Result<Vec<WriteIntent>>`
   - `get_intent(&self, intent_id: &str) -> Result<Option<WriteIntent>>`
   - `prune_completed_intents(&self, older_than_secs: i64) -> Result<usize>`
   - `count_intents_by_status(&self) -> Result<HashMap<String, usize>>`

=== STEP 3: Wrap SIR Pipeline in Intent Flow ===

9) In `crates/aetherd/src/sir_pipeline.rs`:
   - Before the current write sequence, create a WriteIntent with a BLAKE3 hash of
     (symbol_id + timestamp) as intent_id, operation = UpsertSir, status = Pending.
   - Serialize the SIR annotation + symbol metadata as payload_json.
   - After each successful write (SQLite SIR, LanceDB vector, SurrealDB graph),
     update the intent status to the next stage.
   - On any write failure, mark intent as Failed with the error message,
     log with tracing::error, and continue to the next symbol.
   - After all writes succeed, mark Complete.
   - Do NOT change the order of writes (SQLite → LanceDB → SurrealDB).
   - The intent creation itself is a single SQLite INSERT — if this fails,
     the symbol will be re-processed on the next file change (safe).

=== STEP 4: Intent Replay on Startup ===

10) In `crates/aetherd/src/indexer.rs`, in `initialize_indexer()`:
    - After opening stores but before returning, call a new function
      `replay_incomplete_intents(&store, &sir_pipeline, &graph_store, &vector_store)`
    - This function queries get_incomplete_intents() and for each:
      - 'pending': deserialize payload_json, replay from SQLite write onward
      - 'sqlite_done': deserialize payload, replay from embedding generation onward
      - 'vector_done': deserialize payload, replay SurrealDB graph upsert only
      - 'graph_done': mark complete (all writes succeeded, just didn't mark)
    - Log: "Replayed N incomplete write intents from previous session"
    - Also call prune_completed_intents with 7 days (604800 seconds)

=== STEP 5: fsck Command ===

11) Create `crates/aetherd/src/fsck.rs` with:

    `pub fn run_fsck(workspace: &Path, repair: bool, verbose: bool) -> Result<FsckReport>`

    FsckReport struct with fields for each check count.

    Check 1 — SQLite → LanceDB: list all symbol_ids from SQLite that have SIR,
    then check each has a vector. Use batch queries (chunks of 500) for efficiency.

    Check 2 — SQLite → SurrealDB: list all symbol_ids from SQLite, check each
    has a graph node in SurrealDB. Batch via SurrealQL:
    `SELECT symbol_id FROM symbol WHERE symbol_id IN $ids`

    Check 3 — SurrealDB → SQLite: list all symbol_ids from SurrealDB graph nodes,
    check each exists in SQLite.

    Check 4 — SurrealDB dangling edges: query edges where source or target symbol
    doesn't exist in the symbol table.

    Check 5 — LanceDB → SQLite: list all symbol_ids from LanceDB vectors,
    check each exists in SQLite.

    Check 6 — Incomplete intents: count and report.

    If `repair`:
    - Orphaned SIR (no vector): log "queued for re-embedding" (actual re-embed
      would need inference; just flag them for now by creating a new intent)
    - Orphaned graph nodes: DELETE from SurrealDB
    - Dangling edges: DELETE from SurrealDB
    - Orphaned vectors: DELETE from LanceDB
    - Incomplete intents: attempt replay (same as startup replay)

    Print formatted report to stdout.

12) In `crates/aetherd/src/cli.rs`, add subcommand:
    ```
    aether fsck [--repair] [--verbose]
    ```
    Route to `fsck::run_fsck()`.

13) Re-export the fsck module from `crates/aetherd/src/lib.rs`.

=== STEP 6: Tests ===

14) Add tests:
    - WriteIntent CRUD: create, update status, mark complete, mark failed
    - Intent pruning: completed intents older than threshold are deleted
    - Incomplete intent query: returns only non-complete, non-failed intents
    - Status round-trip: WriteIntentStatus Display/FromStr works for all variants
    - Schema migration: version 2 → 3 adds write_intents table
    - fsck with clean state: reports zero inconsistencies
    - fsck with orphaned data: detects and reports correct counts

=== STEP 7: Validation ===

15) Run validation in dependency order:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test -p aether-core
    - cargo test -p aether-config
    - cargo test -p aether-graph-algo
    - cargo test -p aether-document
    - cargo test -p aether-store
    - cargo test -p aether-parse
    - cargo test -p aether-sir
    - cargo test -p aether-infer
    - cargo test -p aether-lsp
    - cargo test -p aether-analysis
    - cargo test -p aether-memory
    - cargo test -p aether-mcp
    - cargo test -p aether-query
    - cargo test -p aetherd
    Do NOT use cargo test --workspace (OOM risk on WSL2 with 12GB RAM).

16) Commit with message:
    "Phase 8.1: Add state reconciliation engine — write intent log, intent replay, aether fsck"

SCOPE GUARD:
- Do NOT add new crates — all changes in existing crates
- Do NOT change SurrealDB schema definitions (use existing query methods)
- Do NOT modify LanceDB table schemas
- Do NOT modify existing MCP tool schemas or CLI subcommands (except adding `fsck`)
- Do NOT modify dashboard pages
- Do NOT change the inference provider interface
- Do NOT rename any public types or functions
- The intent log is SQLite-only — do NOT create a separate database for it
- If the SIR pipeline structure has changed since the spec was written,
  adapt the intent wrapping to match the actual code flow
- If any step cannot be applied because the code differs, report what you found and skip

OUTPUT

17) Report:
    - Which steps were applied vs. skipped (with reason)
    - Validation command outcomes (pass/fail per crate)
    - Total lines changed
    - Commit SHA

18) Provide push + PR commands:
    ```
    git -C ../aether-phase8-stage8-1 push -u origin feature/phase8-stage8-1-state-reconciliation
    gh pr create --title "Phase 8.1: State Reconciliation Engine" --body "..." --base main
    ```

==========END CODEX PROMPT==========
```

## Post-Merge Sequence

```bash
cd /home/rephu/projects/aether
git switch main
git pull --ff-only origin main
git log --oneline -3

git worktree remove ../aether-phase8-stage8-1
git branch -d feature/phase8-stage8-1-state-reconciliation
git worktree prune

git status --porcelain
```
