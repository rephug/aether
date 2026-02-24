# Decision Register v4 — Phase 7 Build Packet

Extends Decision Register v3. These decisions are "locked" for Phase 7 to reduce scope churn.

---

## Inherited decisions (unchanged)

| # | Decision | Status |
|---|----------|--------|
| 1 | Phase 1 focus = Observer | ✅ Complete |
| 2 | Local-first architecture (.aether/) | ✅ Active |
| 3 | LSP-first integration | ✅ Active |
| 4 | Cloud-first inference (Gemini Flash) | ✅ Active |
| 5 | API embeddings by default | ✅ Active |
| 6 | Reranking not enabled by default | ✅ Active |
| 7 | tree-sitter for symbol extraction | ✅ Active |
| 8 | Stable BLAKE3 symbol IDs | ✅ Active |
| 9 | Incremental updates only | ✅ Active |
| 10 | SIR stored as JSON blobs + SQLite metadata | ✅ Active |
| 11 | Vector storage → LanceDB | ✅ Active |
| 13 | Linux primary | ✅ Active |
| 14 | Windows supported, no Ghost sandbox | ✅ Active |
| 16 | Strict SIR schema validation | ✅ Active |
| 17 | Cost and rate limiting mandatory | ✅ Active |
| 18 | Structured logging via `tracing` | ✅ Active |
| 19 | Language plugin trait | ✅ Active |
| 20 | Python via tree-sitter | ✅ Active |
| 21 | Candle local embeddings | ✅ Active |
| 22 | Reranker pipeline | ✅ Active |
| 23 | CozoDB (sled backend) replaces KuzuDB | ⚠️ **Superseded by #38** |
| 24–31 | Phase 5–6 decisions | ✅ Active |
| 32 | Dashboard: Vanilla JS + D3 | ⚠️ **Refined by #41** |
| 33 | Feature flags for verticals | ✅ Active |
| 34 | PDF: pdftotext primary | ⚠️ **Fallback updated by #39** |
| 35 | rust_decimal for money | ✅ Active |
| 36 | Entity resolution strategy | ✅ Active |
| 37 | SharedState connection pooling | ✅ Active |

---

## Updated decisions (changed for Phase 7)

### 12. Graph storage → SurrealDB (updated — supersedes #23)

**Phase 4 original:** "CozoDB selected as graph database with sled backend."

**Phase 7 update:** SurrealDB 3.0 replaces CozoDB as the graph storage engine.

**What changed and why:**

The Phase 4 selection of CozoDB was driven by the hard constraint "embeddable, in-process, no server." That constraint no longer applies:

1. **Architecture shifted to server-client.** Phase 7 introduces `aether-query` (separate read-only binary), web dashboard (HTTP API), and Team Tier (shared index). We are building a server architecture regardless.

2. **CozoDB maintenance risk.** GitHub activity is sparse. The last release (v0.7) was months ago. Community questions go unanswered. The `GraphStore` trait was built specifically as an exit path for this scenario.

3. **Sled exclusive lock.** CozoDB/sled prevents concurrent access from multiple processes. The Phase 7 workaround (Stage 7.2: RocksDB migration) added significant complexity solely to solve this.

4. **`links` conflict.** CozoDB's `storage-sqlite` backend conflicts with rusqlite's `libsqlite3-sys` — both declare `links = "sqlite3"`. This forced sled, which caused problem #3.

**Why SurrealDB 3.0:**

| Criterion | CozoDB (current) | SurrealDB 3.0 |
|-----------|-----------------|---------------|
| Language | Rust | Rust |
| Embeddable | ✅ (sled backend) | ✅ (SurrealKV backend) |
| Concurrent access | ❌ Sled exclusive lock | ✅ MVCC, multi-reader/writer |
| Graph traversal | ✅ Datalog recursive | ✅ `→`/`←` arrow syntax, `RELATE` |
| Graph algorithms | ✅ Built-in (PageRank, community) | ❌ None built-in (reimpl ~500 LOC) |
| Vector search | ✅ HNSW (unused) | ✅ HNSW (8x faster in 3.0) |
| Full-text search | ✅ MinHash-LSH | ✅ BM25 with OR operators |
| Bidirectional links | ❌ Manual | ✅ `REFERENCE` keyword + `<~` traversal |
| Computed fields | ❌ | ✅ Schema-level `COMPUTED` |
| Time-travel queries | ❌ | ✅ SurrealKV `VERSION` clause |
| Custom API endpoints | ❌ | ✅ `DEFINE API` |
| Auth system | ❌ | ✅ Built-in (users, scopes, tokens) |
| WASM extensions | ❌ | ✅ Surrealism (graph algo path) |
| File storage | ❌ | ✅ `DEFINE BUCKET` |
| Maintenance | ⚠️ Low velocity | ✅ Very active, 3.0 just shipped |
| License | MPL 2.0 | BSL 1.1 (DBaaS restriction only) |
| Binary size | Moderate | Large (~50-100MB) |

**License compatibility:** SurrealDB's BSL 1.1 restricts only offering SurrealDB as a managed database service. AETHER is a code intelligence tool, not a DBaaS. AETHER's own planned licensing (BSL for core engine) is fully compatible.

**Graph algorithm gap — mitigation plan:**

CozoDB provides built-in PageRank, community detection, and shortest path. SurrealDB does not. Mitigation:

1. **Immediate (Phase 7):** Reimplement in Rust application code (~500 lines total). PageRank ~100 LOC, Louvain community detection ~200 LOC, BFS/DFS shortest path ~100 LOC. These algorithms operate on data fetched from SurrealDB via standard queries.
2. **Future:** Port algorithms to Surrealism WASM extensions for near-data execution. This is strictly an optimization — not a Phase 7 dependency.

**Migration path:**

- `GraphStore` trait provides clean swap point — implement `SurrealGraphStore`
- Existing Datalog queries → SurrealQL (different syntax, same concepts)
- CozoDB/sled data → SurrealDB via one-time migration script
- `.aether/graph.db` (sled) → `.aether/graph/` (SurrealKV directory)

**Current configuration:**

```toml
# Cargo.toml (workspace)
surrealdb = { version = "3.0", features = ["kv-surrealkv"] }
```

```rust
// crates/aether-store/src/graph_surreal.rs
let db = Surreal::new::<SurrealKV>(&graph_path).await?;
db.use_ns("aether").use_db("graph").await?;
```

**What this eliminates:**
- Stage 7.2 (RocksDB migration) — gone entirely
- Sled exclusive lock problem — gone (SurrealKV has MVCC)
- `links` conflict — gone (no CozoDB)
- Manual reverse-edge queries — gone (Record References)

**Revisit trigger:** Only if SurrealDB has critical stability issues in embedded mode during implementation. Fallback: RocksDB-backed CozoDB (original 7.2 plan preserved in git history).

---

## New decisions (Phase 7)

### 38. SurrealDB 3.0 replaces CozoDB for graph storage

**Status:** ✅ Active

See Decision #12 update above. Summary: SurrealDB 3.0 with SurrealKV embedded backend replaces CozoDB/sled. Eliminates concurrent access problems, `links` conflict, and CozoDB maintenance risk. Requires reimplementing ~500 LOC of graph algorithms.

### 39. PDF fallback: lopdf replaces pdf-extract (revised)

**Status:** ✅ Active

**Context:** Decision #34 specified `pdftotext` (Poppler) as primary PDF extractor with `pdf-extract` as Rust-native fallback. `pdf-extract` produces mediocre output on complex layouts.

**Original Phase 7 plan:** Replace `pdf-extract` with `pdfium-render` (Google's Pdfium engine via Rust FFI bindings).

**Revision:** `pdfium-render` rejected. It requires the pre-compiled C++ Pdfium dynamic library (`libpdfium.so`, `pdfium.dll`, `libpdfium.dylib`) to be present on the host system at runtime. The binary compiles in CI but panics on user machines without the library installed. This breaks AETHER's single-binary portability.

**Change:** Replace `pdf-extract` with `lopdf` as the pure-Rust fallback.

| Criterion | pdf-extract | pdfium-render (rejected) | lopdf |
|-----------|------------|--------------------------|-------|
| Quality | Poor on complex layouts | Excellent | Moderate — raw text stream extraction |
| Language | Rust | Rust FFI to C++ | Pure Rust |
| Runtime dependency | None | libpdfium.so/dll/dylib (REQUIRED) | None |
| Binary portability | ✅ | ❌ Panics without system library | ✅ |
| Table extraction | Poor | Good | Minimal |
| License | Apache 2.0 | Apache 2.0 / BSD | MIT |

**Stack (unchanged primary, new fallback):**
1. Primary: `pdftotext` (Poppler) via `Command::new()` — best output, requires system install
2. Fallback: `lopdf` — pure Rust, extracts raw text streams, no C++ dependency
3. No OCR (clear error if no extractable text)

**Upgrade path:** If PDF extraction quality proves insufficient for the Legal vertical, evaluate `pdfium-render` with a bundled static library or auto-download strategy in a future phase. For MVP, `pdftotext` handles 95%+ of legal PDFs and `lopdf` covers the rest at lower quality.

```toml
# Cargo.toml (legal/finance features)
lopdf = "0.34"
```

### 40. MCP dual transport: stdio + HTTP/SSE

**Status:** ✅ Active

**Context:** Phase 1 implemented MCP over stdio (standard for VS Code extension host communication). Phase 7 introduces `aether-query` which needs HTTP-based MCP for remote access.

**Change:** Support both transports:
- **stdio** — default for VS Code extension, local development, CLI piping
- **HTTP/SSE** — for aether-query, remote access, Team Tier, dashboard integration

Both transports serve the same MCP tool registry. The transport layer is below the tool dispatch — tools don't know or care which transport delivered the request.

```toml
# aether-query.toml
[transport]
mode = "http"  # "stdio" | "http"
bind = "127.0.0.1:3847"
auth_token = "..."

# aetherd default remains stdio for backward compatibility
```

The MCP spec's HTTP/SSE transport (streamable HTTP) is the standard. No custom protocol.

### 41. Dashboard: HTMX + D3.js + Tailwind CSS (refines Decision #32)

**Status:** ✅ Active

**Context:** Decision #32 specified Vanilla JS + D3 + Tailwind CDN. No React, no Node.js, no build step.

**Refinement:** Add HTMX for server-driven UI interactions. D3 remains for visualizations.

| Component | Role |
|-----------|------|
| HTMX (CDN) | Server-driven partial page updates, API interaction |
| D3.js (CDN) | Graph visualization, charts, heatmaps |
| Tailwind CSS (CDN) | Styling |
| rust-embed | Static file embedding in binary |

**Why HTMX over vanilla fetch():**
- Declarative: `hx-get="/api/v1/search?q=..."` directly on HTML elements
- Partial updates: swap only the results div, not the whole page
- No custom JS needed for API calls, loading states, or pagination
- Still zero build step, zero Node.js

**What stays the same:** No React, no Node.js, no build step, static files embedded via `rust-embed`, all CDN imports.

### 42. SurrealDB Record References for bidirectional edges

**Status:** ✅ Active

**Context:** AETHER's graph model has bidirectional relationships (CALLS/CALLED_BY, DEPENDS_ON/DEPENDED_BY). Currently these require separate forward and reverse queries.

**Change:** Use SurrealDB 3.0's `REFERENCE` keyword to make record links bidirectional at schema level.

```sql
-- Schema definition
DEFINE FIELD target ON calls TYPE record<symbol> REFERENCE;

-- Forward: what does function X call?
SELECT ->calls->symbol FROM symbol:abc123;

-- Reverse: what calls function X? (automatic via REFERENCE)
SELECT <~calls FROM symbol:abc123;
```

**Impact:** Eliminates all manual reverse-edge maintenance. Simplifies graph queries throughout the codebase. The `<~` traversal syntax replaces explicit "find all edges where target = X" queries.

### 43. SurrealDB Computed Fields for derived properties

**Status:** ✅ Active

**Context:** Several AETHER properties are derived from other data — SIR staleness, symbol health scores, edge counts, test coverage status.

**Change:** Use SurrealDB 3.0's `COMPUTED` keyword for fields that are always derived at query time.

```sql
DEFINE FIELD sir_stale ON symbol COMPUTED
    time::now() - sir_updated_at > 30m;

DEFINE FIELD edge_count ON symbol COMPUTED
    count(->calls) + count(->depends_on);

DEFINE FIELD callers ON symbol COMPUTED <~calls;
```

**Benefit:** Computed fields are always current. No need to maintain denormalized counters or staleness flags. The database handles it at query time.

**Guardrails:** Computed fields cannot be combined with `VALUE`, `DEFAULT`, or `ASSERT`. They must be top-level fields only. This matches AETHER's usage — all derived properties are top-level.

---

## Decisions unchanged from v3

All other decisions from DECISIONS_v3.md remain active and unchanged. Key confirmations:

- **SQLite** remains the primary metadata store (symbols, SIR, project notes)
- **LanceDB** remains the vector store (disk-backed HNSW, proven at scale)
- **tree-sitter** remains the parser (no change)
- **Gemini Flash** remains the default inference provider
- **tokio** remains the async runtime
- **Axum** remains the HTTP framework (for dashboard, aether-query HTTP endpoints)
- **TOML** remains the configuration format
- **Feature flags** gate legal/finance/dashboard (Decision #33)
- **rust_decimal** for all monetary values (Decision #35)

---

## Decision summary table

| # | Decision | Status |
|---|----------|--------|
| 12 | Graph storage → SurrealDB 3.0 (was CozoDB) | ⚠️ Updated |
| 23 | CozoDB replaces KuzuDB | ❌ Superseded by #38 |
| 32 | Dashboard tech | ⚠️ Refined by #41 |
| 34 | PDF extraction | ⚠️ Fallback updated by #39 |
| 38 | SurrealDB 3.0 replaces CozoDB | ✅ Active |
| 39 | pdfium-render replaces pdf-extract | ✅ Active |
| 40 | MCP dual transport (stdio + HTTP/SSE) | ✅ Active |
| 41 | Dashboard: HTMX + D3 + Tailwind | ✅ Active |
| 42 | Record References for bidirectional edges | ✅ Active |
| 43 | Computed Fields for derived properties | ✅ Active |
