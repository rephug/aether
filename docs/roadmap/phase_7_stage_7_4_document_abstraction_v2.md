# Stage 7.4 — Universal Document Abstraction Layer (Revised)

**Phase:** 7 — The Pathfinder
**Prerequisites:** Stage 7.1 (Store Pooling)
**Estimated Codex Runs:** 2–3

---

## Purpose

Extract the domain-agnostic core of AETHER's pipeline into traits and types that any vertical (code, legal, finance, etc.) can implement. After this stage, adding a new vertical requires only: (1) implement the parser trait, (2) define the semantic record schema, (3) define edge types. Everything else — storage, embedding, search, graph queries, temporal tracking, MCP serving — carries over automatically.

This is the **architectural foundation** for every vertical in the roadmap.

### Design Principle

The abstraction extracts patterns that *already exist* in the code pipeline and makes them generic. No speculative interfaces.

| Code-Specific | Generic Abstraction |
|---|---|
| `Symbol` | `DocumentUnit` — any atomic piece of a document |
| `SIR` (JSON) | `SemanticRecord` — any structured annotation |
| `CALLS, DEPENDS_ON` | `EdgeType` — any relationship between units |
| `git commit` | `ChangeEvent` — any temporal marker |
| `tree-sitter parse` | `DocumentParser` trait |
| `Gemini SIR gen` | `SemanticAnnotator` trait |

### Why This Includes the Embedding Pipeline (GAP-4 Fix)

The original plan created `document_units` and `semantic_records` tables but did NOT wire them to LanceDB for vector search. Without embeddings, legal and finance verticals would have zero semantic search capability — only lexical. The embedding pipeline MUST be domain-agnostic from day one.

Every `SemanticRecord` provides an `embedding_text()` method. This stage wires that text through `aether-infer` to produce embeddings stored in domain-scoped LanceDB tables.

---

## New Crate: `aether-document`

```
crates/aether-document/
├── Cargo.toml
└── src/
    ├── lib.rs              # Re-exports
    ├── unit.rs             # DocumentUnit trait + GenericUnit type
    ├── record.rs           # SemanticRecord trait + GenericRecord type + schema validation
    ├── edge.rs             # DocumentEdge struct + EdgeTypeRegistry trait
    ├── temporal.rs         # ChangeEvent trait (generic temporal marker)
    ├── parser.rs           # DocumentParser trait
    ├── annotator.rs        # SemanticAnnotator trait
    ├── embedding.rs        # Domain-agnostic embedding pipeline
    └── registry.rs         # VerticalRegistry — registers domain implementations
```

**Dependencies:** `aether-core`, `aether-infer` (for embedding pipeline), `serde`, `serde_json`, `blake3`, `thiserror`, `async-trait`

---

## Trait Definitions

### DocumentUnit

```rust
// crates/aether-document/src/unit.rs

/// The atomic unit of a document — the smallest meaningful piece.
/// In code: a function/struct/enum. In legal: a clause. In finance: a line item.
pub trait DocumentUnit: Send + Sync + 'static {
    fn unit_id(&self) -> &str;
    fn display_name(&self) -> &str;
    fn content(&self) -> &str;
    fn unit_kind(&self) -> &str;
    fn source_path(&self) -> &str;
    fn byte_range(&self) -> (usize, usize);
    fn parent_id(&self) -> Option<&str>;
    fn domain(&self) -> &str;
}

/// Concrete generic implementation for storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenericUnit {
    pub unit_id: String,
    pub display_name: String,
    pub content: String,
    pub unit_kind: String,
    pub source_path: String,
    pub byte_range: (usize, usize),
    pub parent_id: Option<String>,
    pub domain: String,
    pub metadata: serde_json::Value,  // Domain-specific extra fields
}
```

**ID Generation:** `unit_id = BLAKE3(domain + source_path + unit_kind + normalized_content[:200])`

### SemanticRecord

```rust
// crates/aether-document/src/record.rs

pub trait SemanticRecord: Send + Sync + 'static {
    fn schema_name(&self) -> &str;       // "SIR", "CIR", "FIR"
    fn schema_version(&self) -> &str;
    fn unit_id(&self) -> &str;
    fn as_json(&self) -> &serde_json::Value;
    fn content_hash(&self) -> &str;
    fn embedding_text(&self) -> String;  // Text representation for vector search
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenericRecord {
    pub record_id: String,
    pub unit_id: String,
    pub domain: String,
    pub schema_name: String,
    pub schema_version: String,
    pub content_hash: String,
    pub record_json: serde_json::Value,
    pub embedding_text: String,
}
```

**Record ID:** `record_id = BLAKE3(unit_id + schema_version + content_hash)`

### DocumentEdge + EdgeTypeRegistry

```rust
// crates/aether-document/src/edge.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentEdge {
    pub source_id: String,
    pub target_id: String,
    pub edge_type: String,      // "CALLS", "REFERENCES", "FLOWS_TO"
    pub domain: String,
    pub weight: f64,            // 0.0–1.0 confidence
    pub metadata: serde_json::Value,
}

pub trait EdgeTypeRegistry: Send + Sync {
    fn domain(&self) -> &str;
    fn valid_edge_types(&self) -> &[&str];
    fn is_valid(&self, edge_type: &str) -> bool;
}
```

### DocumentParser + SemanticAnnotator

```rust
// crates/aether-document/src/parser.rs

#[async_trait]
pub trait DocumentParser: Send + Sync {
    fn domain(&self) -> &str;
    fn supported_extensions(&self) -> &[&str];
    async fn parse(&self, path: &Path, content: &str) -> Result<Vec<GenericUnit>>;
    async fn extract_edges(&self, units: &[GenericUnit]) -> Result<Vec<DocumentEdge>>;
}

// crates/aether-document/src/annotator.rs

#[async_trait]
pub trait SemanticAnnotator: Send + Sync {
    fn domain(&self) -> &str;
    async fn annotate(&self, unit: &GenericUnit) -> Result<GenericRecord>;
    async fn summarize(&self, units: &[GenericUnit], records: &[GenericRecord]) -> Result<GenericRecord>;
}
```

### VerticalRegistry

```rust
// crates/aether-document/src/registry.rs

pub struct VerticalRegistry {
    parsers: HashMap<String, Box<dyn DocumentParser>>,
    annotators: HashMap<String, Box<dyn SemanticAnnotator>>,
    edge_registries: HashMap<String, Box<dyn EdgeTypeRegistry>>,
}

impl VerticalRegistry {
    pub fn new() -> Self;
    pub fn register_parser(&mut self, parser: Box<dyn DocumentParser>);
    pub fn register_annotator(&mut self, annotator: Box<dyn SemanticAnnotator>);
    pub fn register_edge_types(&mut self, registry: Box<dyn EdgeTypeRegistry>);
    pub fn parser_for_domain(&self, domain: &str) -> Option<&dyn DocumentParser>;
    pub fn parser_for_extension(&self, ext: &str) -> Option<&dyn DocumentParser>;
}
```

---

## Embedding Pipeline (GAP-4 Fix)

```rust
// crates/aether-document/src/embedding.rs

/// Domain-agnostic embedding pipeline.
/// Takes a SemanticRecord's embedding_text(), generates vectors via aether-infer,
/// and stores them in domain-scoped LanceDB tables.
pub struct DocumentEmbedder {
    infer: Arc<dyn InferenceProvider>,
    vector_store: Arc<dyn VectorStore>,
}

impl DocumentEmbedder {
    /// Embed a batch of semantic records into their domain's vector table.
    /// Table name: "doc_{domain}" (e.g., "doc_legal", "doc_finance")
    pub async fn embed_records(
        &self,
        domain: &str,
        records: &[GenericRecord],
    ) -> Result<usize>;

    /// Search for similar records using vector similarity.
    pub async fn search(
        &self,
        domain: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(GenericRecord, f32)>>;
}
```

**LanceDB Table Naming:** Each domain gets its own LanceDB table: `doc_legal`, `doc_finance`, etc. This prevents cross-domain pollution in vector search and allows domain-specific embedding models in the future.

**Embedding Model:** Uses the same `aether-infer` providers configured for SIR generation (Gemini embeddings by default, local Ollama as fallback). The embedding model is domain-agnostic — the same model encodes legal clauses and financial line items.

---

## Storage Changes

### SQLite: New Tables (Parallel to Existing)

```sql
-- Works for ALL domains. Parallel to 'symbols' table (which remains unchanged).
CREATE TABLE IF NOT EXISTS document_units (
    unit_id         TEXT PRIMARY KEY,
    domain          TEXT NOT NULL,           -- "code" | "legal" | "finance" | ...
    unit_kind       TEXT NOT NULL,           -- "function" | "clause" | "line_item" | ...
    display_name    TEXT NOT NULL,
    content         TEXT NOT NULL,
    source_path     TEXT NOT NULL,
    byte_range_start INTEGER NOT NULL,
    byte_range_end  INTEGER NOT NULL,
    parent_id       TEXT,                    -- Self-referencing for hierarchy
    metadata_json   TEXT NOT NULL DEFAULT '{}',
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);

CREATE INDEX idx_document_units_domain ON document_units(domain);
CREATE INDEX idx_document_units_source ON document_units(source_path);
CREATE INDEX idx_document_units_kind ON document_units(domain, unit_kind);

-- Semantic records for all domains. Parallel to 'sir' column in symbols.
CREATE TABLE IF NOT EXISTS semantic_records (
    record_id       TEXT PRIMARY KEY,
    unit_id         TEXT NOT NULL,
    domain          TEXT NOT NULL,
    schema_name     TEXT NOT NULL,            -- "SIR" | "CIR" | "FIR"
    schema_version  TEXT NOT NULL,
    content_hash    TEXT NOT NULL,
    record_json     TEXT NOT NULL,            -- Full annotation JSON
    embedding_text  TEXT NOT NULL,            -- Pre-computed embedding input
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    FOREIGN KEY (unit_id) REFERENCES document_units(unit_id)
);

CREATE INDEX idx_semantic_records_unit ON semantic_records(unit_id);
CREATE INDEX idx_semantic_records_domain ON semantic_records(domain);
CREATE INDEX idx_semantic_records_schema ON semantic_records(domain, schema_name);
```

### SurrealDB: Domain-Scoped Graph Tables

Document graph nodes and edges are stored in SurrealDB alongside the existing code graph:

```sql
-- Document unit nodes (domain-scoped, parallel to code symbol table)
DEFINE TABLE IF NOT EXISTS document_node SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS unit_id ON document_node TYPE string;
DEFINE FIELD IF NOT EXISTS domain ON document_node TYPE string;
DEFINE FIELD IF NOT EXISTS unit_kind ON document_node TYPE string;
DEFINE FIELD IF NOT EXISTS display_name ON document_node TYPE string;
DEFINE FIELD IF NOT EXISTS source_path ON document_node TYPE string;
DEFINE INDEX IF NOT EXISTS idx_doc_node_id ON document_node FIELDS unit_id UNIQUE;
DEFINE INDEX IF NOT EXISTS idx_doc_node_domain ON document_node FIELDS domain;

-- Document edges (domain-scoped, typed relationships)
DEFINE TABLE IF NOT EXISTS document_edge SCHEMAFULL TYPE RELATION
    FROM document_node TO document_node;
DEFINE FIELD IF NOT EXISTS edge_type ON document_edge TYPE string;
DEFINE FIELD IF NOT EXISTS domain ON document_edge TYPE string;
DEFINE FIELD IF NOT EXISTS weight ON document_edge TYPE float DEFAULT 1.0;
DEFINE FIELD IF NOT EXISTS metadata_json ON document_edge TYPE string DEFAULT '{}';

-- Record References for bidirectional traversal (Decision #42)
DEFINE FIELD IF NOT EXISTS in ON document_edge TYPE record<document_node> REFERENCE;
DEFINE FIELD IF NOT EXISTS out ON document_edge TYPE record<document_node> REFERENCE;
```

**Why SurrealDB for document graph (not just SQLite):** Document relationships (REFERENCES, SUPERSEDES, FLOWS_TO) form a graph. Graph traversal queries (e.g., "trace all money flows from Entity X") require multi-hop traversal that SQL handles poorly. SurrealDB's `→`/`←` arrow syntax and `RELATE` statements handle this naturally.

**Cross-domain edges:** A document unit CAN reference a code symbol (e.g., a specification clause referencing a function). Cross-domain edges use `document_edge` with source/target from different domain scopes. The `domain` field on the edge indicates the relationship's origin domain.

### LanceDB: Domain-Scoped Vector Tables

Created lazily when the first records for a domain are embedded:
- `doc_legal` — CIR embeddings
- `doc_finance` — FIR embeddings
- (Future domains added automatically)

Schema matches existing `symbols_embeddings` table but with domain prefix.

### Backward Compatibility

The existing code pipeline (`symbols` table, `sir` column, `symbols_embeddings` LanceDB table) is **NOT modified**. The abstraction layer is purely additive. Future migration of the code pipeline to use `document_units` is documented but deferred.

---

## Document Store CRUD

New module in `aether-store`:

```rust
// crates/aether-store/src/document_store.rs

impl SqliteStore {
    // Document Units
    pub fn insert_document_unit(&self, unit: &GenericUnit) -> Result<()>;
    pub fn get_document_unit(&self, unit_id: &str) -> Result<Option<GenericUnit>>;
    pub fn get_units_by_domain(&self, domain: &str) -> Result<Vec<GenericUnit>>;
    pub fn get_units_by_source(&self, source_path: &str) -> Result<Vec<GenericUnit>>;
    pub fn delete_units_by_source(&self, source_path: &str) -> Result<usize>;

    // Semantic Records
    pub fn insert_semantic_record(&self, record: &GenericRecord) -> Result<()>;
    pub fn get_record_by_unit(&self, unit_id: &str) -> Result<Option<GenericRecord>>;
    pub fn get_records_by_domain(&self, domain: &str, limit: usize) -> Result<Vec<GenericRecord>>;
    pub fn search_records_lexical(&self, domain: &str, query: &str, limit: usize) -> Result<Vec<GenericRecord>>;

    // Domain Stats
    pub fn domain_stats(&self, domain: &str) -> Result<DomainStats>;
}

pub struct DomainStats {
    pub unit_count: usize,
    pub record_count: usize,
    pub source_count: usize,
    pub last_updated: i64,
}
```

---

## File Paths (new/modified)

| Path | Action |
|---|---|
| `crates/aether-document/Cargo.toml` | Create |
| `crates/aether-document/src/lib.rs` | Create |
| `crates/aether-document/src/unit.rs` | Create |
| `crates/aether-document/src/record.rs` | Create |
| `crates/aether-document/src/edge.rs` | Create |
| `crates/aether-document/src/temporal.rs` | Create |
| `crates/aether-document/src/parser.rs` | Create |
| `crates/aether-document/src/annotator.rs` | Create |
| `crates/aether-document/src/embedding.rs` | Create |
| `crates/aether-document/src/registry.rs` | Create |
| `crates/aether-store/src/document_store.rs` | Create — CRUD for document_units + semantic_records |
| `crates/aether-store/src/lib.rs` | Modify — add document_store module, new table migrations |
| `crates/aether-store/src/graph_surreal.rs` | Modify — add document_node + document_edge table definitions to ensure_schema() |
| `Cargo.toml` (workspace) | Modify — add aether-document to members |
| `.github/workflows/ci.yml` | Modify — add aether-document to test matrix |

---

## Edge Cases

| Scenario | Behavior |
|---|---|
| Unknown domain in DocumentUnit | VerticalRegistry returns None; caller handles with domain-not-registered error |
| Unit with no content (empty clause) | Accept with empty string; annotator may produce minimal record |
| Duplicate unit_id across domains | Allowed — unit_id hash includes domain in input |
| Parser returns overlapping byte ranges | Accept; display_name used for disambiguation |
| Semantic record JSON doesn't match declared schema | Validate at insert time; reject with schema_validation_error |
| Edge references non-existent unit | Store edge; graph queries handle missing nodes gracefully |
| Very large document (>10MB) | Parser should stream units rather than loading entire content |
| Embedding fails for a record | Log warning, store record without embedding; search will use lexical fallback |
| LanceDB domain table doesn't exist yet | Create lazily on first embed for that domain |

---

## Pass Criteria

1. `aether-document` crate compiles with all trait definitions.
2. `GenericUnit` and `GenericRecord` implement all required traits.
3. `VerticalRegistry` correctly dispatches to registered parsers by domain and extension.
4. `document_units` and `semantic_records` SQLite tables created on initialization.
5. `document_node` and `document_edge` SurrealDB tables created in `ensure_schema()`.
6. CRUD operations work: insert unit → insert record → query by domain → query by source path.
7. **Embedding pipeline works:** insert record → embed → search by vector similarity returns the record.
8. Domain-scoped LanceDB tables created lazily (e.g., `doc_legal`).
9. Existing code pipeline (`symbols` table, `sir`, `symbols_embeddings`) is completely unaffected.
10. Validation gates pass:
    ```
    cargo fmt --all --check
    cargo clippy --workspace -- -D warnings
    cargo test -p aether-core
    cargo test -p aether-config
    cargo test -p aether-document
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

NOTE ON ARCHITECTURE: aether-document is a NEW CRATE providing domain-agnostic traits
and types. It does NOT modify the existing code pipeline. The symbols table, SIR column,
symbols_embeddings LanceDB table, and all existing MCP tools remain unchanged. This
stage adds PARALLEL infrastructure (document_units + semantic_records SQLite tables,
document_node + document_edge SurrealDB tables, domain-scoped LanceDB tables).

NOTE ON GRAPH STORAGE: Stage 7.2 replaced CozoDB with SurrealDB. Document graph
nodes and edges go into SurrealDB tables (document_node, document_edge) using RELATE
for edges and TYPE RELATION for the edge table. Add these table definitions to the
ensure_schema() method in graph_surreal.rs. Use Record References (REFERENCE keyword)
for bidirectional traversal on document_edge.

NOTE ON EMBEDDING PIPELINE: Unlike the original plan, this stage INCLUDES the embedding
pipeline. Every SemanticRecord has an embedding_text() method. The DocumentEmbedder
sends that text through aether-infer to produce vectors stored in domain-scoped LanceDB
tables (e.g., "doc_legal", "doc_finance"). This is critical — without embeddings,
future verticals would have no semantic search.

NOTE ON TRAIT DESIGN: Keep traits minimal. The DocumentParser trait has 4 methods.
The SemanticAnnotator trait has 3 methods. Do not add methods "just in case."

NOTE ON PRIOR STAGES:
- Stage 7.1: SharedState with Arc<SqliteStore> + Arc<dyn GraphStore> + Arc<dyn VectorStore>
- Stage 7.2: SurrealDB/SurrealKV for graph storage (SurrealGraphStore implements GraphStore)
- Phase 6 complete: all SQLite tables, SurrealDB graph, LanceDB vectors

You are working in the repo root at /home/rephu/projects/aether.

Read docs/roadmap/phase_7_stage_7_4_document_abstraction.md for the full specification.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase7-stage7-4-document-abstraction off main.
3) Create worktree ../aether-phase7-stage7-4 for that branch and switch into it.
4) Create new crate crates/aether-document with:
   - Cargo.toml depending on aether-core, aether-infer, serde, serde_json, blake3,
     thiserror, async-trait
   - src/lib.rs re-exporting all public types
   - src/unit.rs — DocumentUnit trait + GenericUnit struct with BLAKE3 ID generation
   - src/record.rs — SemanticRecord trait + GenericRecord struct + schema validation
   - src/edge.rs — DocumentEdge struct + EdgeTypeRegistry trait
   - src/temporal.rs — ChangeEvent trait (generic temporal marker)
   - src/parser.rs — DocumentParser async trait
   - src/annotator.rs — SemanticAnnotator async trait
   - src/embedding.rs — DocumentEmbedder: takes GenericRecords, calls aether-infer,
     stores in domain-scoped LanceDB tables (e.g., "doc_legal")
   - src/registry.rs — VerticalRegistry with parser/annotator/edge registration
5) Add document_units and semantic_records tables to aether-store SQLite migrations.
6) Add document_node and document_edge tables to SurrealDB ensure_schema() in
   crates/aether-store/src/graph_surreal.rs. Use TYPE RELATION for document_edge,
   REFERENCE on in/out fields for bidirectional traversal.
7) Create crates/aether-store/src/document_store.rs:
   - CRUD for document_units (insert, get_by_id, get_by_domain, get_by_source_path, delete)
   - CRUD for semantic_records (insert, get_by_unit, get_by_domain, lexical search)
   - Domain stats query
8) Wire DocumentEmbedder to create domain-scoped LanceDB tables lazily.
9) Add aether-document to workspace Cargo.toml members.
10) Add aether-document to .github/workflows/ci.yml test matrix.
11) Add tests:
    - Unit tests for GenericUnit creation and BLAKE3 ID generation
    - Unit tests for GenericRecord schema validation
    - Unit tests for VerticalRegistry dispatch (register parser, lookup by domain/extension)
    - Integration tests for document_store CRUD operations
    - Integration test for embedding pipeline: create record → embed → search → find it
    - Verify existing symbols table and SIR queries still work (regression test)
12) Run validation:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test -p aether-core
    - cargo test -p aether-config
    - cargo test -p aether-document
    - cargo test -p aether-store
    - cargo test -p aether-memory
    - cargo test -p aether-analysis
    - cargo test -p aether-query
    - cargo test -p aether-mcp
    - cargo test -p aetherd
13) Commit with message: "Add universal document abstraction layer with embedding pipeline"

SCOPE GUARD: Do NOT modify the existing symbols table or any existing SIR logic.
Do NOT modify any existing MCP tools. Do NOT create domain-specific parsers or
annotators (those come in Stage 7.5 and 7.7). The ONLY user-visible change is new
database tables created on initialization.
```
