# Stage 7.3 — aether-query Read-Only Server (Revised)

**Phase:** 7 — The Pathfinder
**Prerequisites:** Stage 7.1 (Store Pooling), Stage 7.2 (SurrealDB Migration)
**Estimated Codex Runs:** 2–3

---

## Purpose

Build a lightweight, read-only query server that opens the *live* `.aether/` index while `aetherd` is running. This is the prerequisite for the Team Tier — one machine indexes, every developer queries. The query server exposes the same MCP tools as the full daemon but cannot write to any database.

This is the **first multi-process consumer** of the AETHER index.

### Why This Is Now Simpler

The original 7.3 had significant complexity around sled's exclusive file lock:
- Sled fallback detection (refuse to start if aetherd holds lock)
- Error messages directing users to migrate to RocksDB
- Two code paths (sled exclusive mode vs RocksDB concurrent mode)

**All of that is gone.** Stage 7.2 replaced CozoDB/sled with SurrealDB/SurrealKV. SurrealKV provides MVCC with concurrent readers and writers natively. There is no exclusive lock. There is no backend-dependent behavior. aether-query opens the same `.aether/graph/` directory that aetherd writes to — they coexist naturally.

### What We Avoided

The original plan built snapshot infrastructure (copy `.aether/` → `.aether-snapshot/`, SIGUSR1/SIGUSR2 signaling, `snapshot_meta.json`). That was eliminated in the design review. Then the RocksDB version still needed sled detection logic. Now with SurrealDB, even that is gone. Zero conditional paths for concurrent access.

---

## Architecture

```
Write Path (existing, unchanged):
  aetherd → SQLite (WAL) + LanceDB + SurrealDB/SurrealKV → .aether/

Read Path (NEW, this stage):
  aether-query → opens .aether/ read-only → serves MCP over HTTP
    - SQLite: opened with SQLITE_OPEN_READONLY
    - LanceDB: opened normally (append-only, MVCC-safe concurrent reads)
    - SurrealDB/SurrealKV: opened normally (MVCC, concurrent readers+writers)
    - All connections held in SharedState for process lifetime
```

No snapshots. No file copying. No signals. No backend detection. Direct concurrent access.

---

## New Crate: `aether-query`

```
crates/aether-query/
├── Cargo.toml
└── src/
    ├── main.rs          # Binary entry point, clap CLI
    ├── config.rs        # QueryConfig — index path, bind address, auth token
    ├── server.rs        # Axum HTTP server hosting MCP-over-HTTP/SSE (Decision #40)
    └── health.rs        # Staleness check, version compatibility
```

**Dependencies:** `aether-core`, `aether-store`, `aether-config`, `aether-memory`, `aether-analysis`, `aether-mcp` (shared tool definitions), `tokio`, `axum`, `tower-http`, `clap`, `tracing`, `serde`, `serde_json`

### Why No LSP in MVP

LSP requires a persistent stdio or TCP connection per editor. MCP-over-HTTP is stateless and sufficient for AI agent queries. LSP hover can be added later without architectural changes.

---

## MCP Transport: HTTP/SSE (Decision #40)

This stage implements the HTTP/SSE transport for MCP alongside the existing stdio transport. Both transports serve the same `AetherMcpServer` tool registry.

```
aetherd (existing):    MCP over stdio   → VS Code extension
aether-query (new):    MCP over HTTP    → AI agents, remote access, Team Tier
```

The MCP spec's streamable HTTP transport is used. No custom protocol:
- `POST /mcp` — standard MCP JSON-RPC request/response
- Server-Sent Events for streaming responses (future)

---

## SharedState Integration

The query server reuses `SharedState` from Stage 7.1:

```rust
// In aether-query main.rs
let state = SharedState::open_readonly(&config.index_path)?;
// state.read_only == true
// state.store → Arc<SqliteStore> (read-only)
// state.graph → Arc<dyn GraphStore> (SurrealDB/SurrealKV — concurrent access)
// state.vector_store → Some(Arc<dyn VectorStore>) (read-only)
// state.config → Arc<AetherConfig>
```

The `AetherMcpServer` already uses `self.state` after the 7.1 refactor. Creating an `AetherMcpServer` with a read-only `SharedState` automatically makes all write tools fail cleanly via `state.require_writable()`.

---

## Config: `aether-query.toml`

```toml
[query]
index_path = ".aether"              # Path to the live .aether/ directory
bind_address = "127.0.0.1:9731"     # HTTP + MCP endpoint
auth_token = ""                      # If set, require Bearer token
max_concurrent_queries = 32
read_timeout_ms = 5000

[staleness]
warn_after_minutes = 30              # Warn if index older than this
```

If no config file exists, defaults are used. The index_path defaults to `.aether` in the current directory.

---

## Schema: No New Tables

The query server opens existing databases read-only. No schema changes.

The `schema_version` table (from 7.1) is checked at startup:

```rust
let version = state.store.get_schema_version("core")?;
if version > SUPPORTED_SCHEMA_VERSION {
    return Err("Index was created by a newer aetherd version. Upgrade aether-query.");
}
```

---

## MCP Tool Surface

The query server reuses the same `AetherMcpServer` tool definitions. Behavior differs only on write tools:

| MCP Tool | Query Server Behavior |
|---|---|
| `aether_search` | ✅ Full functionality |
| `aether_ask` | ✅ Full functionality (unified query) |
| `aether_sir` | ✅ Read SIR for any symbol |
| `aether_recall` | ✅ Search project notes |
| `aether_drift_report` | ✅ Read cached drift results |
| `aether_health` | ✅ Read cached health metrics |
| `aether_callers` / `aether_deps` | ✅ Graph queries (SurrealDB) |
| `aether_coupling` | ✅ Read coupling data |
| `aether_remember` | ❌ Returns `{"error": "read_only_server", "message": "..."}` |
| `aether_index` | ❌ Returns read_only error |
| `aether_reindex` | ❌ Returns read_only error |

Write tool errors use structured JSON:
```json
{
  "error": "read_only_server",
  "message": "This is a read-only query server. Use the full aetherd daemon for write operations."
}
```

---

## HTTP API

```
GET  /health                → {"status": "ok", "index_age_seconds": 42, "schema_version": 1, "stale": false}
POST /mcp                   → MCP JSON-RPC over HTTP (same protocol as aetherd MCP)
GET  /info                  → {"aether_query_version": "0.8.0", "index_path": ".aether", "backend": "surreal", "symbols": 6234, "read_only": true}
```

### Authentication

If `auth_token` is set in config:
- All requests must include `Authorization: Bearer <token>` header
- Missing/wrong token → HTTP 401 `{"error": "unauthorized"}`
- Empty `auth_token` in config → no auth required

### Rate Limiting

Semaphore-based: `max_concurrent_queries` controls how many MCP requests are processed simultaneously. Excess requests get HTTP 429 `{"error": "rate_limited", "max_concurrent": 32}`.

---

## CLI: `aether-query` Binary

```bash
# Start the query server
aether-query serve --config aether-query.toml

# Start with defaults (looks for .aether in current dir)
aether-query serve

# Health check (hits /health endpoint)
aether-query status

# Show index info
aether-query info

# Start with custom index path
aether-query serve --index-path /shared/project/.aether
```

---

## Staleness Warning

Every MCP response includes a `_meta` field when the index is stale:

```json
{
  "results": [...],
  "_meta": {
    "index_stale": true,
    "last_indexed_at": "2026-02-21T14:30:00Z",
    "staleness_minutes": 47,
    "warning": "Index has not been updated in 47 minutes. Results may be outdated."
  }
}
```

Staleness threshold configured via `staleness.warn_after_minutes` (default: 30). Determined by checking `last_indexed_at` from the SQLite metadata.

---

## File Paths (new/modified)

| Path | Action |
|---|---|
| `crates/aether-query/Cargo.toml` | Create |
| `crates/aether-query/src/main.rs` | Create |
| `crates/aether-query/src/config.rs` | Create |
| `crates/aether-query/src/server.rs` | Create |
| `crates/aether-query/src/health.rs` | Create |
| `Cargo.toml` (workspace) | Modify — add aether-query to members |
| `.github/workflows/ci.yml` | Modify — add aether-query to test matrix |

**NOT modified:** aetherd, aether-mcp, aether-store. The query server is purely additive — a new binary that consumes existing shared infrastructure.

---

## Edge Cases

| Scenario | Behavior |
|---|---|
| Index path doesn't exist | Refuse to start: "No .aether directory found at <path>" |
| Index created by newer aetherd | Refuse to start: "Schema version mismatch. Upgrade aether-query." |
| aetherd running + aether-query starting | **Both coexist.** SurrealKV MVCC handles concurrent access. SQLite WAL handles concurrent reads. LanceDB MVCC handles concurrent reads. |
| Write MCP tool called | Returns structured `read_only_server` error |
| Auth token mismatch | HTTP 401 |
| Query timeout | HTTP 504 `{"error": "query_timeout", "timeout_ms": 5000}` |
| Concurrent query limit exceeded | HTTP 429 |
| Index updated while query in flight | Read sees consistent MVCC snapshot (SQLite WAL + SurrealKV + LanceDB all provide this) |
| aetherd stops while query server running | Query server keeps working with last-written data |
| aetherd corrupts index | Query server may return errors; restart after reindex |

---

## Pass Criteria

1. `aether-query serve` starts and opens `.aether/` in read-only mode.
2. All read MCP tools return correct results matching the full daemon's output.
3. Write MCP tools return structured `read_only_server` errors (not panics, not 500s).
4. **Concurrent access verified:** `aetherd` writes to `.aether/` while `aether-query` reads simultaneously. No lock errors, no data corruption.
5. Auth token enforcement works (401 without token, 200 with correct token).
6. `aether-query info` displays index metadata (symbol count, backend, version).
7. `aether-query status` hits `/health` and reports OK/stale.
8. Staleness warning appears in MCP responses when index is older than threshold.
9. Schema version check rejects incompatible indexes at startup.
10. Validation gates pass:
    ```
    cargo fmt --all --check
    cargo clippy --workspace -- -D warnings
    cargo test -p aether-core
    cargo test -p aether-config
    cargo test -p aether-store
    cargo test -p aether-memory
    cargo test -p aether-analysis
    cargo test -p aether-query
    cargo test -p aether-mcp
    cargo test -p aetherd
    ```

---

## Codex Prompt

```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- export CARGO_TARGET_DIR=/home/rephu/aether-target
- export CARGO_BUILD_JOBS=1
- export PROTOC=$(which protoc)
- export TMPDIR=/home/rephu/aether-target/tmp
- mkdir -p $TMPDIR
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.
- The repo uses mold linker via .cargo/config.toml — ensure mold and clang are installed.

NOTE ON ARCHITECTURE: aether-query is a NEW BINARY (not a mode of aetherd).
It depends on aether-store, aether-mcp, aether-config, etc. but NOT on aetherd.
It opens databases in read-only mode using SharedState::open_readonly() from Stage 7.1.

NOTE ON CONCURRENT ACCESS: Stage 7.2 replaced CozoDB/sled with SurrealDB/SurrealKV.
SurrealKV provides MVCC with concurrent readers+writers. There is NO exclusive lock.
aether-query opens the same .aether/graph/ directory that aetherd writes to.
Do NOT implement any sled detection, RocksDB fallback, or backend-conditional logic.
All three databases (SQLite WAL, LanceDB MVCC, SurrealKV MVCC) support concurrent access.

NOTE ON MCP TRANSPORT (Decision #40): This stage implements MCP-over-HTTP.
The existing aetherd uses MCP-over-stdio. aether-query uses MCP-over-HTTP (POST /mcp).
Both transports share the same AetherMcpServer tool definitions.

NOTE ON NO SNAPSHOTS: Do NOT implement any snapshot logic, SIGUSR handling,
or file-copy infrastructure. Direct concurrent access to the live index.

NOTE ON PRIOR STAGES:
- Stage 7.1: SharedState struct with Arc<SqliteStore> + Arc<dyn GraphStore> + read_only flag.
  AetherMcpServer uses self.state for all tool handlers. require_writable() guard.
- Stage 7.2: SurrealDB/SurrealKV replaces CozoDB/sled. MVCC concurrent access.
  SurrealGraphStore implements GraphStore trait.
- Phase 6 complete: 17+ MCP tools, all databases, LSP hover.

You are working in the repo root at /home/rephu/projects/aether.

Read docs/roadmap/phase_7_stage_7_3_aether_query.md for the full specification.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase7-stage7-3-aether-query off main.
3) Create worktree ../aether-phase7-stage7-3 for that branch and switch into it.
4) Create new crate crates/aether-query with:
   - Cargo.toml depending on aether-core, aether-store, aether-config, aether-memory,
     aether-analysis, aether-mcp, tokio, axum, tower-http, clap, tracing, serde, serde_json
   - src/main.rs — binary entry point with clap CLI (serve, status, info subcommands)
   - src/config.rs — QueryConfig struct (index_path, bind_address, auth_token, etc.)
   - src/server.rs — Axum HTTP server:
     - POST /mcp → MCP JSON-RPC handler delegating to AetherMcpServer with read-only SharedState
     - GET /health → staleness check + schema version
     - GET /info → index metadata
   - src/health.rs — Staleness detection (check last_indexed_at vs current time)
5) In server.rs, create SharedState::open_readonly() pointing at the configured index_path.
   Create AetherMcpServer with that read-only state. Route MCP requests through it.
6) Add auth middleware: if config.auth_token is non-empty, require Bearer token on all requests.
7) Add rate limiting: Tokio semaphore with max_concurrent_queries permits.
8) Add aether-query to workspace Cargo.toml members.
9) Add aether-query to .github/workflows/ci.yml test matrix.
10) Add tests:
    - Unit test: SharedState::open_readonly() opens databases, read_only flag is true
    - Unit test: require_writable() returns error when read_only
    - Unit test: QueryConfig deserialization from TOML
    - Integration test: start aether-query against test fixtures, verify MCP read responses
    - Integration test: verify write MCP tools return read_only_server error
    - Integration test: verify auth token enforcement (401/200)
    - Integration test: concurrent access — write to SurrealDB in one process,
      read from aether-query in another, verify no errors
11) Run validation:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test -p aether-core
    - cargo test -p aether-config
    - cargo test -p aether-store
    - cargo test -p aether-memory
    - cargo test -p aether-analysis
    - cargo test -p aether-query
    - cargo test -p aether-mcp
    - cargo test -p aetherd
12) Commit with message: "Add aether-query read-only server binary"

SCOPE GUARD: Do NOT implement LSP (MCP-over-HTTP only for this stage). Do NOT implement
any snapshot or file-copy logic. Do NOT add auto-refresh or index watching — the query
server reads whatever is in the index when the query arrives. Do NOT add WebSocket support.
Do NOT add any sled/RocksDB detection or backend-conditional code.
```
