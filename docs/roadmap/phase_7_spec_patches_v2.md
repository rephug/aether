# Phase 7 Spec Patches — Ready to Apply (v2)

All patches organized by target file. Apply these before feeding prompts to Codex.

---

## PART 1: CODEBASE FIXES (Category D — Apply Manually Before Phase 7)

These are diffs to apply to the current codebase. They fix latent bugs that would block compilation or cause runtime failures during Phase 7 development.

### D1. Arrow Version Downgrade

**File:** `Cargo.toml` (workspace root)

```toml
# Change:
arrow-array = "56.2"
arrow-schema = "56.2"

# To:
arrow-array = "54"
arrow-schema = "54"
```

### D2. CozoDB Engine String Revert

**File:** `crates/aether-store/src/graph_cozo.rs`

```rust
// Change:
let db = DbInstance::new("sqlite", &graph_path_str, Default::default())

// To:
let db = DbInstance::new("sled", &graph_path_str, Default::default())
```

Do NOT change the Cargo.toml CozoDB feature flags. Leave as `features = ["storage-sled", "graph-algo"]`. The `links = "sqlite3"` conflict makes `storage-sqlite` impossible alongside `rusqlite`.

### D3. unified_query.rs — spawn_blocking

**File:** `crates/aether-memory/src/unified_query.rs`

Replace all three `std::thread::spawn().join()` blocks. Full before/after for the first block (apply same pattern to `note_lexical` and `test_lexical`):

```rust
// ===== REMOVE (symbol_lexical) =====
        let query_owned = query.to_owned();
        let query = query_owned.clone();
        let store_clone = store.clone();
        let symbol_lexical = std::thread::spawn(move || {
            store_clone
                .search_symbols(query.as_str(), candidate_limit)
                .map_err(Into::into)
        })
        .join()
        .map_err(|err| {
            MemoryError::InvalidInput(format!("symbol search task join failure: {err:?}"))
        })??;

// ===== REPLACE WITH =====
        let query_owned = query.to_owned();
        let q1 = query_owned.clone();
        let store1 = store.clone();
        let symbol_lexical = tokio::task::spawn_blocking(move || {
            store1.search_symbols(q1.as_str(), candidate_limit).map_err(Into::into)
        })
        .await
        .map_err(|err| MemoryError::InvalidInput(format!("symbol search task join failure: {err}")))??;
```

```rust
// ===== REMOVE (note_lexical) =====
        let query = query_owned.clone();
        let store_clone = store.clone();
        let note_lexical = std::thread::spawn(move || {
            store_clone
                .search_project_notes_lexical(query.as_str(), candidate_limit, false, &[])
                .map_err(Into::into)
        })
        .join()
        .map_err(|err| {
            MemoryError::InvalidInput(format!("note search task join failure: {err:?}"))
        })??;

// ===== REPLACE WITH =====
        let q2 = query_owned.clone();
        let store2 = store.clone();
        let note_lexical = tokio::task::spawn_blocking(move || {
            store2.search_project_notes_lexical(q2.as_str(), candidate_limit, false, &[]).map_err(Into::into)
        })
        .await
        .map_err(|err| MemoryError::InvalidInput(format!("note search task join failure: {err}")))??;
```

```rust
// ===== REMOVE (test_lexical) =====
        let query = query_owned.clone();
        let store_clone = store.clone();
        let test_lexical = std::thread::spawn(move || {
            store_clone
                .search_test_intents_lexical(query.as_str(), candidate_limit)
                .map_err(Into::into)
        })
        .join()
        .map_err(|err| {
            MemoryError::InvalidInput(format!("test intent search task join failure: {err:?}"))
        })??;

// ===== REPLACE WITH =====
        let q3 = query_owned.clone();
        let store3 = store.clone();
        let test_lexical = tokio::task::spawn_blocking(move || {
            store3.search_test_intents_lexical(q3.as_str(), candidate_limit).map_err(Into::into)
        })
        .await
        .map_err(|err| MemoryError::InvalidInput(format!("test intent search task join failure: {err}")))??;
```

### D4. Candle Embedding + Reranker — spawn_blocking

**File:** `crates/aether-infer/src/embedding/candle.rs`

```rust
// ===== REMOVE =====
        let loaded = Arc::clone(provider.ensure_loaded()?);
        let mut output = Self::embed_texts_with_loaded(loaded.as_ref(), &input)
            .map_err(|err| {
                InferError::ModelUnavailable(format!("candle embedding task failed: {err}"))
            })?;

        Ok(output
            .pop()
            .unwrap_or_else(|| vec![0.0; CANDLE_EMBEDDING_DIM]))

// ===== REPLACE WITH =====
        let output = tokio::task::spawn_blocking(move || {
            let loaded = Arc::clone(provider.ensure_loaded()?);
            Self::embed_texts_with_loaded(loaded.as_ref(), &input)
                .map_err(|err| {
                    InferError::ModelUnavailable(format!("candle embedding task failed: {err}"))
                })
        })
        .await
        .map_err(|err| InferError::ModelUnavailable(format!("candle embedding join failed: {err}")))??;

        Ok(output
            .pop()
            .unwrap_or_else(|| vec![0.0; CANDLE_EMBEDDING_DIM]))
```

**File:** `crates/aether-infer/src/reranker/candle.rs`

```rust
// ===== REMOVE =====
        let loaded = Arc::clone(provider.ensure_loaded()?);
        Self::rerank_sync_with_loaded(
            loaded.as_ref(),
            query.as_str(),
            candidates.as_slice(),
            top_n,
        )
        .map_err(|err| {
            InferError::ModelUnavailable(format!("candle reranker task failed: {err}"))
        })

// ===== REPLACE WITH =====
        tokio::task::spawn_blocking(move || {
            let loaded = Arc::clone(provider.ensure_loaded()?);
            Self::rerank_sync_with_loaded(
                loaded.as_ref(),
                query.as_str(),
                candidates.as_slice(),
                top_n,
            )
            .map_err(|err| {
                InferError::ModelUnavailable(format!("candle reranker task failed: {err}"))
            })
        })
        .await
        .map_err(|err| InferError::ModelUnavailable(format!("candle reranker join failed: {err}")))?
```

### D5. SqliteVectorStore — Hold Connection

**File:** `crates/aether-store/src/vector.rs`

```rust
// ===== REMOVE =====
pub struct SqliteVectorStore {
    workspace_root: PathBuf,
}

impl SqliteVectorStore {
    pub fn new(workspace_root: impl AsRef<Path>) -> Self {
        Self {
            workspace_root: workspace_root.as_ref().to_path_buf(),
        }
    }

    fn store(&self) -> Result<SqliteStore, StoreError> {
        SqliteStore::open(&self.workspace_root)
    }
}

// ===== REPLACE WITH =====
pub struct SqliteVectorStore {
    store: std::sync::Mutex<SqliteStore>,
}

impl SqliteVectorStore {
    pub fn new(workspace_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        Ok(Self {
            store: std::sync::Mutex::new(SqliteStore::open(workspace_root)?),
        })
    }
}
```

**Additional required changes in the same file:**

1. Update call site in `open_vector_store` (or wherever `SqliteVectorStore::new` is called):
```rust
// Change:
SqliteVectorStore::new(workspace_root)
// To:
SqliteVectorStore::new(workspace_root)?
```

2. Update all methods in `impl VectorStore for SqliteVectorStore` — replace every occurrence of:
```rust
self.store()?.some_method(...)
```
with:
```rust
self.store.lock().unwrap().some_method(...)
```

### D6. Dynamic Inotify Registration

**File:** `crates/aetherd/src/indexer.rs`

Insert immediately before the `enqueue_event_paths` call in **both** the `rx.recv_timeout` block and the `rx.try_recv` block:

```rust
// ===== INSERT BEFORE enqueue_event_paths =====
                if let Ok(ref event) = result {
                    for path in &event.paths {
                        if path.is_dir() && !crate::observer::is_ignored_path(path) {
                            let _ = watcher.watch(path, notify::RecursiveMode::NonRecursive);
                        }
                    }
                }
```

This appears in two locations in the event loop. Both need the same insertion.

### D7. LSP File Read Outside Mutex

**File:** `crates/aether-lsp/src/lib.rs`

Step 1 — Update the hover handler to read the file before acquiring the lock:

```rust
// ===== REMOVE =====
        let guard = self.store.lock().await;
        let markdown = resolve_hover_markdown_for_path(
            &self.workspace_root,
            &guard,
            &file_path,
            text_doc_pos.position.line as usize + 1,
            text_doc_pos.position.character as usize + 1,
        );

// ===== REPLACE WITH =====
        let source = match std::fs::read_to_string(&file_path) {
            Ok(s) => s,
            Err(_) => return Ok(Some(no_sir_hover())),
        };

        let guard = self.store.lock().await;
        let markdown = resolve_hover_markdown_for_source(
            &self.workspace_root,
            &guard,
            &file_path,
            &source,
            text_doc_pos.position.line as usize + 1,
            text_doc_pos.position.character as usize + 1,
        );
```

Step 2 — Refactor the hover resolution function. Find `resolve_hover_markdown_for_path` and split it:

```rust
// ===== NEW FUNCTION (add alongside existing) =====

/// Resolve hover markdown from pre-read source text.
/// File I/O has already happened — this only does database lookups.
pub fn resolve_hover_markdown_for_source(
    workspace_root: &Path,
    store: &SqliteStore,
    file_path: &Path,
    source: &str,
    line: usize,
    col: usize,
) -> String {
    // ... same body as resolve_hover_markdown_for_path, but use `source`
    // parameter instead of calling std::fs::read_to_string(file_path)
}

// ===== MODIFY EXISTING (keep for non-LSP callers) =====

/// Resolve hover markdown by reading the file and delegating.
pub fn resolve_hover_markdown_for_path(
    workspace_root: &Path,
    store: &SqliteStore,
    file_path: &Path,
    line: usize,
    col: usize,
) -> String {
    let source = match std::fs::read_to_string(file_path) {
        Ok(s) => s,
        Err(_) => return no_sir_markdown(),
    };
    resolve_hover_markdown_for_source(workspace_root, store, file_path, &source, line, col)
}
```

The exact refactoring depends on where the file read happens inside the current `resolve_hover_markdown_for_path`. The principle: extract the file read to the top, make the rest accept `&str`, and the original function becomes a thin wrapper.

---

## PART 2: CODEX PROMPT PATCHES (Category B — Update Spec Files)

### Patch 1: `phase_7_stage_7_1_store_pooling_v2.md`

**Codex Prompt — Insert after Step 3, before Step 4:**

```
CRITICAL THREAD SAFETY: SqliteStore currently holds a raw rusqlite::Connection,
which is !Send + !Sync. Wrapping SqliteStore in Arc requires the connection to be
safe for shared access. In crates/aether-store/src/lib.rs, change the conn field
in SqliteStore from Connection to Mutex<Connection>. Ensure ALL methods in
SqliteStore acquire the lock via self.conn.lock().unwrap() before executing
queries. This is required for Arc<SqliteStore> to compile — without it, the Rust
compiler will reject Arc<SqliteStore> with:
  error[E0277]: *mut sqlite3 cannot be shared between threads safely
Do NOT use a connection pool (r2d2) for this stage — Mutex is sufficient for the
single-process model. Connection pooling is a future optimization.
```

---

### Patch 2: `phase_7_stage_7_2_surrealdb_migration.md`

**Codex Prompt — Replace current Step 7 entirely with:**

```
7) Create crates/aether-analysis/src/graph_algorithms.rs:
   a) Add petgraph = "0.6" to crates/aether-analysis/Cargo.toml
   b) bfs_shortest_path(db, from_id, to_id) → Vec<String>
   c) page_rank(db, damping, iterations) → HashMap<String, f64>
   d) louvain_communities(db) → HashMap<String, usize>
   e) All algorithms: first fetch edges from SurrealDB via async queries, then
      dump into a petgraph::DiGraph for in-memory computation. Use petgraph's
      node indexing and adjacency iteration — do NOT hand-roll adjacency lists.
      petgraph does NOT include PageRank or Louvain, so implement those on top
      of petgraph's data structures.
   f) CRITICAL ASYNC SAFETY: All CPU-bound graph computation (the iterative
      PageRank loop, Louvain modularity optimization, BFS traversal) MUST
      execute inside tokio::task::spawn_blocking to avoid starving the async
      reactor. Pattern:
        pub async fn page_rank(db: &Surreal<Db>, ...) -> Result<...> {
            let edges = fetch_all_edges(db).await?;  // async: SurrealDB query
            tokio::task::spawn_blocking(move || {
                compute_page_rank_sync(&edges, damping, iterations)  // sync: CPU math
            }).await.map_err(|e| StoreError::Graph(format!("spawn_blocking: {e}")))?
        }
```

**Dependency Changes section — Add:**

```toml
# crates/aether-analysis/Cargo.toml — ADD
petgraph = "0.6"
```

---

### Patch 3: `phase_7_stage_7_6_web_dashboard_v2.md`

**Codex Prompt — Insert after Step 6 (mount dashboard router):**

```
CRITICAL: The Axum HTTP server for the dashboard MUST be spawned on a dedicated
background Tokio task so it does not block the LSP stdio loop or the file watcher.
In crates/aetherd/src/main.rs, after building the dashboard router, spawn it as:

  let dashboard_handle = tokio::spawn(async move {
      let listener = tokio::net::TcpListener::bind(&bind_addr).await
          .expect("dashboard: failed to bind");
      tracing::info!("Dashboard listening on http://{bind_addr}/dashboard/");
      axum::serve(listener, router).await
          .expect("dashboard: server error");
  });

The LSP stdio loop and file watcher continue running on their own tasks. The
dashboard server runs independently. Do NOT .await the dashboard handle in the
main function — let it run in the background.
```

---

## PART 3: SPECIFICATION REVISIONS (Category C — Update Spec Files)

### Patch 4: `DECISIONS_v4.md` — Revise Decision #39

**Replace the entire Decision #39 section with:**

```markdown
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
```

---

### Patch 5: `phase_7_stage_7_5_aether_legal_v2.md`

**5a. In the "PDF/DOCX Text Extraction" section, replace the fallback description:**

```
# REMOVE:
**Fallback:** `pdfium-render` Rust crate (Decision #39) — Google's PDF engine with Rust bindings. Excellent output on complex layouts, self-contained. Replaces the previous `pdf-extract` fallback which produced mediocre results.

# REPLACE WITH:
**Fallback:** `lopdf` Rust crate (Decision #39 revised) — pure Rust PDF text stream extraction. Lower quality than pdftotext on complex layouts, but zero C++ dependencies. Replaces both `pdf-extract` (mediocre output) and the rejected `pdfium-render` (required unshippable C++ dynamic library).
```

**5b. In the `extract_pdf` function comment, replace:**

```rust
# REMOVE:
    // Fallback: pdfium-render Rust crate (Decision #39)

# REPLACE WITH:
    // Fallback: lopdf Rust crate (Decision #39 revised) — pure Rust, no C++ deps
```

**5c. In the Edge Cases table, replace:**

```
# REMOVE:
| pdftotext not installed | Fall back to pdfium-render; log warning about extraction method |

# REPLACE WITH:
| pdftotext not installed | Fall back to lopdf; log warning about reduced extraction quality |
```

**5d. In the Codex Prompt, replace the PDF extraction note:**

```
# REMOVE:
NOTE ON PDF EXTRACTION (Decisions #34, #39):
Primary = pdftotext (Poppler) via Command::new().
Fallback = pdfium-render Rust crate (NOT pdf-extract — Decision #39 replaced it).
  pdfium-render uses Google's pdfium engine with Rust bindings.
  Add to Cargo.toml: pdfium-render = "0.8"
Do NOT implement OCR. If pdftotext not found AND pdfium-render fails, return clear error.

# REPLACE WITH:
NOTE ON PDF EXTRACTION (Decisions #34, #39 revised):
Primary = pdftotext (Poppler) via Command::new().
Fallback = lopdf Rust crate (pure Rust, no C++ dependency).
  lopdf extracts raw text streams from PDF. Lower quality than pdftotext on complex
  layouts, but works everywhere with zero system dependencies.
  Add to Cargo.toml: lopdf = "0.34"
  Do NOT use pdfium-render — it requires a C++ dynamic library at runtime that
  breaks single-binary portability.
Do NOT implement OCR. If pdftotext not found AND lopdf extraction returns empty, return clear error.
```

**5e. In the Codex Prompt Step 4 dependency list, replace:**

```
# REMOVE:
   - Cargo.toml depending on aether-core, aether-document, aether-infer, serde,
     serde_json, blake3, thiserror, async-trait, regex, pdfium-render

# REPLACE WITH:
   - Cargo.toml depending on aether-core, aether-document, aether-infer, serde,
     serde_json, blake3, thiserror, async-trait, regex, lopdf
```

---

### Patch 6: `phase_7_stage_7_7_aether_finance_v2.md`

**6a. In the Codex Prompt, replace the PDF extraction note:**

```
# REMOVE:
NOTE ON PDF EXTRACTION (Decisions #34, #39): Reuse pdftotext/pdfium-render approach.
Primary = pdftotext (Poppler) via Command::new().
Fallback = pdfium-render Rust crate (NOT pdf-extract — Decision #39 replaced it).
Financial PDFs often have tabular data — rely on LLM extraction prompt for structure,
not regex.

# REPLACE WITH:
NOTE ON PDF EXTRACTION (Decisions #34, #39 revised): Reuse pdftotext/lopdf approach.
Primary = pdftotext (Poppler) via Command::new().
Fallback = lopdf Rust crate (pure Rust, no C++ dependency — NOT pdfium-render,
which requires an unshippable C++ dynamic library at runtime).
Financial PDFs often have tabular data — rely on LLM extraction prompt for structure,
not regex. lopdf may produce lower-quality text on complex tables; the LLM prompt
should handle messy input gracefully.
```

**6b. In the Codex Prompt Step 4 dependency list, replace:**

```
# REMOVE:
     serde_json, blake3, thiserror, async-trait, regex, csv, calamine, pdfium-render

# REPLACE WITH:
     serde_json, blake3, thiserror, async-trait, regex, csv, calamine, lopdf
```

---

### Patch 7: `phase_7_pathfinder_v2.md`

**7a. In "Change 2" section, replace:**

```
# REMOVE:
### Change 2: pdfium-render replaces pdf-extract (Decision #39)

The Rust-native PDF fallback (`pdf-extract`) produces mediocre output on complex layouts. Google's `pdfium-render` is dramatically better. Primary extraction (`pdftotext` via Poppler) is unchanged.

# REPLACE WITH:
### Change 2: lopdf replaces pdf-extract (Decision #39 revised)

The Rust-native PDF fallback (`pdf-extract`) produces mediocre output on complex layouts. Initially planned to use `pdfium-render` (Google's Pdfium engine), but rejected because it requires a C++ dynamic library (`libpdfium.so`/`pdfium.dll`) at runtime — breaking AETHER's single-binary portability. `lopdf` (pure Rust) provides moderate quality text extraction with zero system dependencies. Primary extraction (`pdftotext` via Poppler) is unchanged.
```

**7b. In Risk Register table, replace the PDF row:**

```
# REMOVE:
| PDF extraction quality too low for legal | High | High | pdftotext primary, pdfium-render fallback, manual text input escape hatch |

# REPLACE WITH:
| PDF extraction quality too low for legal | High | High | pdftotext primary (handles 95%+ of legal PDFs), lopdf pure-Rust fallback, manual text input escape hatch. pdfium-render rejected due to C++ runtime dependency breaking portability. |
```

**7c. In Risk Register table, add new row:**

```
| Graph algorithm correctness (PageRank/Louvain) | Medium | Medium | petgraph data structures, spawn_blocking for async safety, comparison tests against CozoDB baseline |
```

---

## SUMMARY CHECKLIST

### Category D — Codebase fixes (apply to code, commit)
- [ ] D1: Arrow version 56.2 → 54
- [ ] D2: graph_cozo.rs engine string "sqlite" → "sled"
- [ ] D3: unified_query.rs std::thread → spawn_blocking (3 blocks)
- [ ] D4: candle embedding spawn_blocking
- [ ] D4: candle reranker spawn_blocking
- [ ] D5: SqliteVectorStore hold Mutex<SqliteStore>
- [ ] D5: Update open_vector_store call site
- [ ] D5: Update VectorStore impl methods
- [ ] D6: indexer.rs dynamic inotify (2 insertion points)
- [ ] D7: LSP read file outside mutex
- [ ] D7: Split resolve_hover_markdown_for_path → resolve_hover_markdown_for_source
- [ ] Run cargo fmt + clippy + full test suite
- [ ] Commit

### Category B — Codex prompt patches (apply to spec files)
- [ ] Patch 1: Stage 7.1 — Mutex<Connection>
- [ ] Patch 2: Stage 7.2 — petgraph + spawn_blocking
- [ ] Patch 3: Stage 7.6 — Axum spawn

### Category C — Spec revisions (apply to spec files)
- [ ] Patch 4: DECISIONS_v4 — Decision #39 → lopdf
- [ ] Patch 5: Stage 7.5 — lopdf throughout (5 locations)
- [ ] Patch 6: Stage 7.7 — lopdf throughout (2 locations)
- [ ] Patch 7: Phase 7 overview — Change 2 + Risk Register (3 locations)
