# Phase 4 - Stage 4.1: LanceDB Vector Backend

## Purpose
Replace the brute-force SQLite embedding search with LanceDB ANN indexing. Currently, every semantic search query loads ALL embeddings from SQLite into memory, deserializes JSON, and computes cosine similarity in a Rust loop. This is O(n) and will not scale past ~5K symbols. LanceDB provides sub-50ms ANN search at any scale.

## Current implementation (what we're replacing)
- Embeddings stored as `embedding_json TEXT` in `sir_embeddings` table
- Search: `SELECT symbol_id, embedding_json FROM sir_embeddings WHERE provider=? AND model=? AND embedding_dim=?`
- Then: JSON deserialize → dot product → normalize → sort → take top N
- Hybrid search uses Reciprocal Rank Fusion (RRF) between separate lexical and semantic passes

## Target implementation
- Embeddings stored in LanceDB table at `.aether/vectors/`
- Search: LanceDB native ANN query with pre-built IVF-PQ index
- SQLite `sir_embeddings` table removed (or kept as migration source)
- `VectorStore` trait abstracts the backend for future swaps

## In scope
- Add `lancedb = "0.23"` and `lzma-sys = { features = ["static"] }` to workspace deps
- Create `VectorStore` trait in `crates/aether-store` with: `upsert_embedding`, `delete_embedding`, `search_nearest`, `rebuild_index`
- Implement `LanceVectorStore` using LanceDB Rust SDK
- Implement `SqliteVectorStore` (current behavior) as fallback
- Config toggle: `[embeddings] vector_backend = "lancedb" | "sqlite"` (default: `"lancedb"`)
- Migration path: on first run with `lancedb` backend, read existing `sir_embeddings` rows and write to LanceDB
- Update `crates/aetherd/src/search.rs` to use the `VectorStore` trait
- Update `crates/aetherd/src/sir_pipeline.rs` embedding writes to use the trait
- Store embeddings as `FixedSizeList<Float32>` not JSON strings

## Out of scope
- Full-text BM25 in LanceDB (keep current SQL LIKE lexical search)
- Reranker pipeline
- Changing the RRF hybrid fusion logic (keep it, just swap the semantic source)

## Implementation notes

### LanceDB table schema
```
Table: sir_embeddings
  symbol_id: Utf8 (primary key)
  sir_hash: Utf8
  provider: Utf8
  model: Utf8
  embedding: FixedSizeList<Float32, DIM>
  updated_at: Int64
```

### VectorStore trait shape
```rust
#[async_trait]
pub trait VectorStore: Send + Sync {
    async fn upsert_embedding(&self, record: VectorRecord) -> Result<()>;
    async fn delete_embedding(&self, symbol_id: &str) -> Result<()>;
    async fn search_nearest(
        &self,
        query: &[f32],
        provider: &str,
        model: &str,
        limit: u32,
    ) -> Result<Vec<VectorSearchResult>>;
}
```

### Migration strategy
1. On startup, if config says `lancedb` but `.aether/vectors/` doesn't exist:
2. Check if `sir_embeddings` table has rows
3. If yes, batch-read all rows and write to LanceDB
4. Log migration progress via tracing (or eprintln for now)
5. Do NOT delete SQLite rows (allow rollback by switching config back)

## Pass criteria
1. `cargo test --workspace` passes with mock embedding provider using LanceDB backend.
2. Semantic search returns same top-K results as SQLite backend for identical test data.
3. New workspaces default to LanceDB; existing workspaces migrate on first run.
4. Config toggle between `sqlite` and `lancedb` backends works without data loss.
5. `sir_embeddings` SQLite table is still readable for migration/rollback.
6. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

Read these files for context first:
- docs/roadmap/phase_4_stage_4_1_lancedb_vector_backend.md (this file)
- crates/aether-store/src/lib.rs (current Store trait + SqliteStore)
- crates/aetherd/src/search.rs (current search orchestration)
- crates/aetherd/src/sir_pipeline.rs (current embedding writes)
- crates/aether-config/src/lib.rs (current config schema)
- Cargo.toml (workspace deps)

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase4-stage4-1-lancedb off main.
3) Create worktree ../aether-phase4-stage4-1-lancedb for that branch and switch into it.
4) Add workspace dependencies:
   - lancedb = "0.23"
   - lzma-sys = { version = "*", features = ["static"] }
   - arrow-array = "54"
   - arrow-schema = "54"
   - async-trait = "0.1"
5) Create a VectorStore trait in crates/aether-store with upsert/delete/search methods.
6) Implement LanceVectorStore that stores embeddings in .aether/vectors/ using LanceDB.
7) Implement SqliteVectorStore wrapping the existing brute-force code.
8) Add config field [embeddings] vector_backend = "lancedb" | "sqlite" (default lancedb).
9) Update search.rs and sir_pipeline.rs to use VectorStore trait instead of direct SQLite calls.
10) Add migration: on first lancedb run, copy existing sir_embeddings rows to LanceDB.
11) Add tests:
    - Mock embeddings round-trip through LanceDB backend
    - Search returns correct top-K ordering
    - Migration from SQLite to LanceDB preserves all records
    - Config toggle between backends
12) Run:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test --workspace
13) Commit with message: "Add LanceDB vector backend with migration from SQLite".
```

## Expected commit
`Add LanceDB vector backend with migration from SQLite`
