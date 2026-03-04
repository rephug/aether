# Phase 8 — Stage 8.2: Progressive Indexing + Tiered Providers

**Prerequisites:** Stage 8.1 (State Reconciliation) merged
**Estimated Codex Runs:** 1–2 (core pipeline refactor + provider changes)
**Risk Level:** Medium-High — restructures the core indexing pipeline and inference provider system

---

## Purpose

Running LLM inference on every symbol in a large monorepo takes hours or days. Users need to see AETHER's value in minutes. Currently, the indexing pipeline is monolithic: every symbol goes through AST parsing → SIR generation → embedding → graph update before anything is visible. This means a 50,000-symbol repo shows zero value until inference completes.

Stage 8.2 does three things:
1. **Splits indexing into two tiers** so structural value is available in seconds
2. **Adds a priority queue** so the most important symbols get SIR first
3. **Removes Mock provider and adds Tiered provider** for cloud+local hybrid inference

---

## Design

### Tiered Indexing

**Pass 1 — Structural Index (Seconds to Minutes):**
- Parse all files with tree-sitter → extract symbols
- Build dependency graph edges in SurrealDB (imports, calls, type references)
- Store symbol metadata in SQLite (name, kind, file path, signature)
- Enable lexical search immediately
- Enable graph queries (call chain, dependencies, blast radius) immediately
- Dashboard shows symbol counts, dependency graph, module structure
- **No LLM inference. No embeddings. No SIR.**

**Pass 2 — Semantic Index (Background, Hours):**
- Generate SIR for each symbol via inference provider
- Generate embeddings, store in LanceDB
- Enable semantic search
- Enrich graph with SIR-derived data (complexity, side effects)
- Dashboard shows SIR coverage, drift, health metrics
- **Uses priority queue to process most important symbols first.**

### Priority Queue

Symbols are not indexed alphabetically or by file path. The priority queue orders by:

1. **Git recency** (weight: 0.4) — Files modified in the last 10 commits rank highest. Uses `gix` (already in `aether-git`) to read commit history. Score: `1.0 - (commit_position / 10.0)`, clamped to [0.0, 1.0]. Files not in last 10 commits get 0.0.

2. **PageRank centrality** (weight: 0.3) — After Pass 1 builds the dependency graph, run PageRank (from `aether-graph-algo`). High-centrality symbols are structural bottlenecks — indexing them first maximizes blast radius understanding. Score: normalized PageRank value.

3. **Symbol kind priority** (weight: 0.2) — Public APIs and exported symbols first. Score: `pub fn/struct/trait/impl = 1.0`, `fn/struct = 0.7`, `const/static/type = 0.5`, other = 0.3.

4. **File size inverse** (weight: 0.1) — Smaller files are faster to infer, giving quicker incremental value. Score: `1.0 - min(1.0, file_lines / 1000.0)`.

**Combined score:** `0.4 * git_recency + 0.3 * page_rank + 0.2 * kind_priority + 0.1 * size_inverse`

Symbols are processed in descending score order. The queue is a `BinaryHeap<(OrderedFloat<f64>, String)>` (score, symbol_id).

### Tiered Inference Provider

The `Tiered` provider routes symbols to different inference backends based on their priority score from the queue:

```
for each symbol popped from priority queue:
    if score >= primary_threshold (default 0.8):
        try primary provider (Gemini, NIM, or any OpenAI-compat)
        if primary fails (timeout, 429, error) AND retry_with_fallback:
            use fallback (Ollama local)
    else:
        use fallback (Ollama local) directly
    
    store which provider generated the SIR (for future regeneration via 8.8)
```

**Implementation:** `TieredProvider` holds two `Box<dyn InferenceProvider>` — a `primary` and a `fallback`. The routing logic lives in `generate_sir()` which checks `context.priority_score`.

**Config example — NIM primary + Ollama fallback:**
```toml
[inference]
provider = "tiered"

[inference.tiered]
primary = "openai_compat"
primary_model = "qwen3.5-397b-a17b"
primary_endpoint = "https://integrate.api.nvidia.com/v1"
primary_api_key_env = "NVIDIA_NIM_API_KEY"
primary_threshold = 0.8
fallback_model = "qwen3.5:9b"
fallback_endpoint = "http://127.0.0.1:11434"
retry_with_fallback = true
```

**Config example — Gemini primary + Ollama fallback:**
```toml
[inference]
provider = "tiered"

[inference.tiered]
primary = "gemini"
primary_model = "gemini-flash-latest"
primary_api_key_env = "GEMINI_API_KEY"
primary_threshold = 0.8
fallback_model = "qwen3.5:9b"
fallback_endpoint = "http://127.0.0.1:11434"
retry_with_fallback = true
```

The fallback is always Ollama (`qwen3_local`). The primary can be `gemini` or `openai_compat`.

### Mock Provider Removal

The `Mock` provider is removed entirely (Decision #44):
- Delete `MockProvider` struct and its `InferenceProvider` impl from `aether-infer`
- Delete `MockEmbeddingProvider` struct and its `EmbeddingProvider` impl
- Remove `Mock` variant from `InferenceProviderKind` enum
- Remove `Mock` variant from `EmbeddingProviderKind` enum
- Update `Auto` provider: if no API key is found, return a clear error instead of silently falling back to Mock
- Update all tests that used `MockProvider` to use either a test-specific inline mock or the real Ollama provider with `#[ignore]`
- Remove `MOCK_EMBEDDING_DIM` constant

**Why:** Mock operated via a different code path (file-level instead of per-symbol), producing fake data that masked real inference pipeline bugs. Tests should use focused unit mocks, not a global fake provider.

### Default Model Update

Update the default Ollama model from `qwen2.5-coder:7b-instruct-q4_K_M` to `qwen3.5:9b` (Decision #45):
- Update `DEFAULT_QWEN_MODEL` constant in `aether-infer`
- Update config documentation
- Update any hardcoded model references

### On-Demand SIR Bump

If an agent or user queries a symbol that lacks SIR (via MCP or LSP Hover), bump that symbol to the **absolute front** of the generation queue.

**MCP behavior:**
- When `aether_get_sir` is called for a symbol with no SIR:
  - Return immediately with `"sir_status": "generating"` and the available structural data (name, kind, dependencies, callers)
  - Bump the symbol to queue position 0
  - The agent can poll again after a few seconds

**LSP hover behavior:**
- When hovering a symbol with no SIR:
  - Return a hover card with structural info + "SIR generation in progress..."
  - Bump the symbol to queue position 0
  - On next hover (after SIR completes), return full SIR

**Dashboard behavior:**
- Symbols without SIR show a badge: `"SIR Pending (Queue Position: N)"`
- X-Ray page shows SIR coverage percentage with a progress indicator

### Background Worker Architecture

The SIR generation runs in a background Tokio task, separate from the file watcher loop:

```
┌─────────────┐     ┌──────────────────┐     ┌─────────────────┐
│ File Watcher │────▶│ Pass 1: AST +    │────▶│ Priority Queue  │
│ (indexer.rs) │     │ Graph (immediate)│     │ (BinaryHeap)    │
└─────────────┘     └──────────────────┘     └────────┬────────┘
                                                      │
                                             ┌────────▼────────┐
                                             │ Pass 2: SIR     │
                                             │ Background      │
                                             │ Worker (N tasks) │
                                             └─────────────────┘
                                                      │
                    ┌──────────────┐                   │
                    │ MCP/LSP      │──── bump ─────────┘
                    │ On-Demand    │
                    └──────────────┘
```

**Provider routing in Pass 2 worker (Tiered mode):**
```
                    ┌─────────────────┐
                    │ Pop from queue  │
                    └────────┬────────┘
                             │
                    ┌────────▼────────┐
                    │ score >= 0.8?   │
                    └──┬──────────┬───┘
                   YES │          │ NO
              ┌────────▼───┐  ┌──▼───────────┐
              │ Try Primary │  │ Use Fallback │
              │ (NIM/Gemini)│  │ (Ollama)     │
              └──┬─────┬───┘  └──────────────┘
             OK  │     │ FAIL
              ┌──▼──┐  └──▶ Use Fallback
              │Store│       (Ollama)
              └─────┘
```

**Concurrency:** The background worker uses `sir_concurrency` from config (default: 2) to process N symbols in parallel. Each task uses the intent log from Stage 8.1.

**Queue persistence:** The queue itself is NOT persisted. On restart, Pass 1 re-scans (fast, seconds), rebuilds the queue from git history + PageRank, then resumes Pass 2 for any symbols missing SIR.

### Progress Tracking

New SQLite method: `count_symbols_with_sir() -> (total_symbols, symbols_with_sir)`

Exposed via:
- MCP: `aether_status` tool already exists — add `sir_coverage` field to response
- Dashboard: X-Ray page already shows SIR coverage — make it live-update
- CLI: `aether status` shows `SIR Coverage: 4,521 / 12,847 (35.2%)`

---

## Files Modified

| File | Action | Description |
|------|--------|-------------|
| `crates/aetherd/src/indexer.rs` | **Modify** | Split into Pass 1 (AST + graph) and Pass 2 (background SIR) |
| `crates/aetherd/src/priority_queue.rs` | **Create** | Priority queue with git recency + PageRank + kind scoring |
| `crates/aetherd/src/sir_pipeline.rs` | **Modify** | Adapt to work as background worker consuming from queue |
| `crates/aether-infer/src/lib.rs` | **Modify** | Remove MockProvider/MockEmbeddingProvider, add TieredProvider, update DEFAULT_QWEN_MODEL |
| `crates/aether-config/src/lib.rs` | **Modify** | Remove `Mock` from enums, add `Tiered` variant + `[inference.tiered]` config section |
| `crates/aether-store/src/sqlite.rs` | **Modify** | Add `count_symbols_with_sir()`, `list_symbols_without_sir()` |
| `crates/aether-mcp/src/lib.rs` | **Modify** | Return "generating" status + bump for missing SIR |
| `crates/aether-lsp/src/lib.rs` | **Modify** | Return partial hover + bump for missing SIR |
| `crates/aetherd/src/cli.rs` | **Modify** | Enhance `aether status` with SIR coverage |
| `crates/aetherd/src/lib.rs` | **Modify** | Re-export priority_queue module |

---

## Edge Cases

| Scenario | Behavior |
|----------|----------|
| Zero git history (fresh clone with squashed history) | Git recency = 0.0 for all files; falls back to PageRank + kind + size scoring |
| Pass 1 finds 0 symbols (empty repo or all ignored) | Pass 2 queue is empty; daemon enters watch-only mode normally |
| On-demand bump for symbol already in progress | No-op; SIR generation already running for that symbol |
| On-demand bump flood (agent querying 100 symbols rapidly) | Queue bump is O(log n) per symbol; existing concurrency limit prevents inference overload |
| Inference provider unavailable during Pass 2 | Intent log marks as 'failed'; symbol stays in queue for retry on next pass or manual `aether fsck --repair` |
| Symbol deleted between Pass 1 and Pass 2 | Pass 2 checks symbol still exists in SQLite before inference; skip if deleted |
| Config change: `sir_concurrency` updated | Takes effect on next daemon restart; live reconfiguration out of scope |
| File watcher triggers re-index of symbol already in Pass 2 queue | Existing intent dedup: if intent for this symbol_id is 'pending', skip re-queue |
| Tiered mode: primary provider returns 429 | If `retry_with_fallback = true`, silently demote to Ollama fallback. Log warning. |
| Tiered mode: primary provider timeout | Same as 429 — demote to fallback. The 10s connect / 120s request timeouts from hardening pass 5 apply. |
| Tiered mode: Ollama not running | Error propagates normally. Symbol stays in queue for retry. |
| `provider = "tiered"` but `[inference.tiered]` section missing | Config validation error at startup with clear message |
| `Auto` provider with no API key | Clear error message instead of silently falling back to removed Mock |
| Tests that previously used MockProvider | Use inline test doubles or `#[ignore]` with real provider |

---

## Pass Criteria

1. Initial indexing completes Pass 1 (AST + graph) within seconds for a small repo (<100 files).
2. Lexical search and graph queries work immediately after Pass 1, before any SIR exists.
3. Pass 2 processes symbols in priority order (git-recent + high-PageRank first).
4. MCP `aether_get_sir` returns `"sir_status": "generating"` for pending symbols and bumps queue.
5. LSP hover returns structural info with "SIR generation in progress" for pending symbols.
6. `aether status` shows SIR coverage percentage.
7. On-demand bump moves a symbol to the front of the queue.
8. Pass 2 uses the intent log from Stage 8.1 for all writes.
9. MockProvider and MockEmbeddingProvider are fully removed. No `Mock` variant in provider enums.
10. TieredProvider correctly routes high-score symbols to primary, low-score to fallback.
11. TieredProvider falls back to Ollama on primary timeout/error when `retry_with_fallback = true`.
12. Default Ollama model is `qwen3.5:9b`.
13. Existing MCP tools, dashboard pages, and CLI commands remain functional.
14. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings` pass.
15. Per-crate tests pass.

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

Read docs/roadmap/phase_8_stage_8_2_progressive_indexing.md for the full specification.
Read docs/roadmap/phase8_session_context.md for current architecture context.

PREFLIGHT

1) Ensure working tree is clean (`git status --porcelain`). If not, stop and report.
2) `git pull --ff-only` — ensure main is up to date.

BRANCH + WORKTREE

3) Create branch feature/phase8-stage8-2-progressive-indexing off main.
4) Create worktree ../aether-phase8-stage8-2 for that branch and switch into it.
5) Set build environment (copy the exports from the top of this prompt).

NOTE ON CURRENT INDEXING PIPELINE:
- `crates/aetherd/src/indexer.rs` — `run_indexing_loop()` and `run_initial_index_once()`
- `initialize_indexer()` creates ObserverState, SqliteStore, SirPipeline
- `observer.initial_symbol_events()` returns all symbol change events at startup
- For each event, `sir_pipeline.process_event()` is called synchronously
- `process_event()` generates SIR via inference, stores in SQLite, generates embedding,
  stores in LanceDB, updates SurrealDB graph — all in one blocking call
- The file watcher loop then processes incremental changes the same way

NOTE ON STAGE 8.1 DEPENDENCY:
- Stage 8.1 added the write intent log (write_intents table in SQLite)
- SIR pipeline now wraps writes in intent flow (create intent → write SQLite →
  write LanceDB → write SurrealDB → mark complete)
- Pass 2 background worker MUST use this same intent flow

NOTE ON GIT ACCESS:
- `crates/aether-git/` has gix-based git operations
- Use gix to read recent commit history for priority scoring
- If gix is not available or repo is not a git repo, skip git scoring (weight = 0)

NOTE ON GRAPH ALGORITHMS:
- `crates/aether-graph-algo/` provides `page_rank_sync()`
- Takes a Vec of edges, returns HashMap<NodeId, f64> of PageRank scores
- Can be called after Pass 1 builds the dependency graph

NOTE ON OBSERVER:
- `ObserverState::initial_symbol_events()` already returns all symbols on startup
- `ObserverState::process_path()` returns symbol change events for incremental changes
- Both return `SymbolChangeEvent` which has: file_path, symbols (Vec<SymbolInfo>)

NOTE ON MOCK PROVIDER REMOVAL (Decision #44):
- MockProvider and MockEmbeddingProvider are being removed entirely
- They used a different code path (file-level vs per-symbol) that masked real bugs
- Tests using Mock should be converted to inline test doubles or #[ignore]
- The `Auto` variant should error clearly if no API key found, not fall back to Mock

NOTE ON DEFAULT MODEL UPDATE (Decision #45):
- Default Ollama model changes from `qwen2.5-coder:7b-instruct-q4_K_M` to `qwen3.5:9b`
- Update DEFAULT_QWEN_MODEL constant in crates/aether-infer/src/lib.rs

=== STEP 1: Remove Mock Provider ===

6) In `crates/aether-infer/src/lib.rs`:
   - Delete the `MockProvider` struct and its `InferenceProvider` impl
   - Delete the `MockEmbeddingProvider` struct and its `EmbeddingProvider` impl
   - Delete the `MOCK_EMBEDDING_DIM` constant
   - Update `load_inference_provider_from_config()`: remove the `Mock` match arm
   - Update `summarize_text_with_config()`: remove the `Mock` match arm
   - Update `load_embedding_provider_from_config()`: remove the `Mock` match arm
   - Update the `Auto` match arm in all three functions: if no API key is found,
     return `Err(InferError::MissingApiKey(...))` with a clear message like
     "No inference API key found. Set GEMINI_API_KEY or configure a provider."
     Do NOT fall back to Mock.
   - Update DEFAULT_QWEN_MODEL to "qwen3.5:9b"

7) In `crates/aether-config/src/lib.rs`:
   - Remove `Mock` from `InferenceProviderKind` enum
   - Remove `Mock` from `EmbeddingProviderKind` enum
   - Add `Tiered` to `InferenceProviderKind` enum
   - Add `TieredConfig` struct:
     ```rust
     pub struct TieredConfig {
         pub primary: String,           // "gemini" or "openai_compat"
         pub primary_model: Option<String>,
         pub primary_endpoint: Option<String>,
         pub primary_api_key_env: String,
         pub primary_threshold: f64,    // default 0.8
         pub fallback_model: Option<String>,    // default "qwen3.5:9b"
         pub fallback_endpoint: Option<String>, // default "http://127.0.0.1:11434"
         pub retry_with_fallback: bool,         // default true
     }
     ```
   - Add `tiered: Option<TieredConfig>` field to `InferenceConfig`
   - Add defaults: primary_threshold = 0.8, fallback_model = "qwen3.5:9b",
     fallback_endpoint = "http://127.0.0.1:11434", retry_with_fallback = true

8) Fix all tests that reference MockProvider or MockEmbeddingProvider:
   - For unit tests that need a fake provider, create a minimal inline test double:
     ```rust
     #[cfg(test)]
     struct TestProvider;
     #[cfg(test)]
     #[async_trait]
     impl InferenceProvider for TestProvider {
         async fn generate_sir(&self, _: &str, ctx: &SirContext) -> Result<SirAnnotation, InferError> {
             Ok(SirAnnotation {
                 intent: format!("Test SIR for {}", ctx.qualified_name),
                 inputs: vec![], outputs: vec![], side_effects: vec![],
                 dependencies: vec![], error_modes: vec![], confidence: 0.9,
             })
         }
     }
     ```
   - For integration tests that test real inference, mark with #[ignore]
   - Compile and ensure no references to `MockProvider` or `MockEmbeddingProvider` remain

=== STEP 2: Add Tiered Provider ===

9) In `crates/aether-infer/src/lib.rs`, add `TieredProvider`:

   ```rust
   pub struct TieredProvider {
       primary: Box<dyn InferenceProvider>,
       fallback: Box<dyn InferenceProvider>,
       threshold: f64,
       retry_with_fallback: bool,
       primary_name: String,
   }

   impl TieredProvider {
       pub fn new(
           primary: Box<dyn InferenceProvider>,
           fallback: Box<dyn InferenceProvider>,
           threshold: f64,
           retry_with_fallback: bool,
           primary_name: String,
       ) -> Self {
           Self { primary, fallback, threshold, retry_with_fallback, primary_name }
       }
   }

   #[async_trait]
   impl InferenceProvider for TieredProvider {
       async fn generate_sir(
           &self,
           symbol_text: &str,
           context: &SirContext,
       ) -> Result<SirAnnotation, InferError> {
           let score = context.priority_score.unwrap_or(0.0);
           if score >= self.threshold {
               match self.primary.generate_sir(symbol_text, context).await {
                   Ok(sir) => return Ok(sir),
                   Err(e) if self.retry_with_fallback => {
                       tracing::warn!(
                           symbol = %context.qualified_name,
                           provider = %self.primary_name,
                           error = %e,
                           "Primary provider failed, falling back to local"
                       );
                   }
                   Err(e) => return Err(e),
               }
           }
           self.fallback.generate_sir(symbol_text, context).await
       }
   }
   ```

10) Add `priority_score: Option<f64>` to `SirContext` (in `aether-sir` or wherever
    SirContext is defined). Default to None. The background worker sets this when
    popping from the priority queue.

11) In `load_inference_provider_from_config()`, add the `Tiered` match arm:
    - Read `config.inference.tiered` (error if missing when provider = "tiered")
    - Construct the primary provider based on `tiered.primary`:
      - "gemini" → GeminiProvider
      - "openai_compat" → OpenAiCompatProvider
    - Construct the fallback as Qwen3LocalProvider with tiered.fallback_model/endpoint
    - Return TieredProvider wrapping both

=== STEP 3: Create Priority Queue Module ===

12) Create `crates/aetherd/src/priority_queue.rs`:

   - `SirPriorityQueue` struct wrapping a `BinaryHeap<Reverse<(OrderedFloat<f64>, String)>>`
     (min-heap by negative score, so highest score comes out first) — OR use a max-heap
     with positive scores. Choose whichever is cleaner.
   - Method: `push(&mut self, symbol_id: String, score: f64)`
   - Method: `bump_to_front(&mut self, symbol_id: &str)` — remove if present, re-insert
     with score = f64::MAX (guaranteed front of queue)
   - Method: `pop(&mut self) -> Option<(f64, String)>` — returns (score, symbol_id)
     so the background worker can pass the score to SirContext.priority_score
   - Method: `len(&self) -> usize`
   - Method: `is_empty(&self) -> bool`
   - Use a HashSet<String> alongside the heap to track which symbol_ids are enqueued,
     preventing duplicate entries.
   - Function: `compute_priority_score(git_recency: f64, page_rank: f64, kind_priority: f64, size_inverse: f64) -> f64`
     = 0.4 * git_recency + 0.3 * page_rank + 0.2 * kind_priority + 0.1 * size_inverse
   - Function: `kind_priority_score(kind: &str, is_public: bool) -> f64`
     pub fn/struct/trait/impl = 1.0, fn/struct = 0.7, const/static/type = 0.5, other = 0.3
   - Function: `size_inverse_score(line_count: usize) -> f64`
     = 1.0 - (line_count as f64 / 1000.0).min(1.0)
   - Thread safety: wrap in Arc<Mutex<SirPriorityQueue>> for shared access between
     the indexer, MCP, and LSP.

=== STEP 4: Add SQLite Query Methods ===

13) In `crates/aether-store/src/sqlite.rs`, add:
   - `count_symbols_with_sir(&self) -> Result<(usize, usize)>` — returns (total_symbols, symbols_with_sir)
     Query: SELECT COUNT(*) FROM symbols; SELECT COUNT(DISTINCT symbol_id) FROM sir WHERE ...
     (adapt to actual table/column names in the codebase)
   - `list_symbol_ids_without_sir(&self) -> Result<Vec<String>>` — returns symbol_ids
     that exist in symbols table but have no corresponding SIR entry
   - `get_symbol_metadata(&self, symbol_id: &str) -> Result<Option<SymbolMetadata>>`
     — returns kind, file_path, is_public (if available), line_count

=== STEP 5: Refactor Indexing Pipeline ===

14) In `crates/aetherd/src/indexer.rs`:

   **Refactor `run_initial_index_once()`:**
   - Rename current implementation to `run_full_index_once()` (for backward compat)
   - New `run_initial_index_once()` does Pass 1 only:
     a. Seed observer from disk (existing)
     b. Process all symbol events through observer (existing — extracts AST symbols)
     c. Store symbol metadata in SQLite (existing path)
     d. Build/update dependency graph in SurrealDB (existing path in sir_pipeline
        or wherever edge extraction happens)
     e. Do NOT call inference. Do NOT generate embeddings.
     f. Log: "Pass 1 complete: {N} symbols indexed, lexical search + graph queries available"

   **Add Pass 2 startup:**
   - After Pass 1, compute priority scores:
     a. Get git recency scores (call into aether-git or use gix directly)
     b. Run PageRank on the dependency graph
     c. For each symbol without SIR, compute combined score and push to priority queue
     d. Log: "Pass 2 queued: {N} symbols for SIR generation"

   **Refactor `run_indexing_loop()`:**
   - The file watcher loop remains the same for incremental Pass 1 (AST + graph)
   - Add a background Tokio task (or std::thread if the current pipeline is sync)
     that continuously pops from the priority queue and runs SIR generation
   - **IMPORTANT:** When popping (score, symbol_id), set `context.priority_score = Some(score)`
     before calling the inference provider — this is what TieredProvider uses for routing.
   - The background worker respects `sir_concurrency` for parallelism
   - Use a channel or shared Arc<Mutex<Queue>> for the watcher to add new symbols
     to the Pass 2 queue as files change

   **IMPORTANT:** The current pipeline may be fully synchronous (std::thread, mpsc channels).
   If so, spawn the background SIR worker as a separate std::thread that shares the
   priority queue via Arc<Mutex<>>. Do NOT force a Tokio migration of the watcher loop.

=== STEP 6: On-Demand SIR Bump ===

15) In `crates/aether-mcp/src/lib.rs`:
   - Find the `aether_get_sir` tool handler
   - When SIR is not found for the requested symbol:
     - Return a response with `"sir_status": "generating"` plus available structural
       data (symbol name, kind, file_path, dependencies, callers from graph)
     - If a shared reference to the priority queue is accessible, call bump_to_front()
     - If the queue is not accessible from MCP (likely — MCP runs in aether-query too),
       instead set a flag in SQLite: add a `sir_requests` table. The background worker
       checks this table periodically.

   NOTE: The MCP server runs in both aetherd (has queue access) and aether-query
   (read-only, no queue). For aether-query, the bump must go through a different
   mechanism. The simplest approach: add a `sir_requests` SQLite table that both
   processes can write to. The background worker in aetherd polls this table.

16) In `crates/aether-lsp/src/lib.rs`:
    - Find the hover handler
    - When SIR is not found for the hovered symbol:
      - Return a hover card with structural info + "⏳ SIR generation in progress..."
      - Write to `sir_requests` table (same mechanism as MCP)

=== STEP 7: Status Enhancement ===

17) In `crates/aetherd/src/cli.rs` or wherever `aether status` is handled:
    - Add SIR coverage to the output:
      `SIR Coverage: 4,521 / 12,847 (35.2%)`
    - Use count_symbols_with_sir() from SqliteStore

=== STEP 8: Tests ===

18) Add tests:
    - Priority queue: push multiple symbols, pop returns highest score first
    - Priority queue: bump_to_front moves a symbol ahead of all others
    - Priority queue: duplicate symbol_id is not inserted twice
    - Priority queue: pop returns (score, symbol_id) tuple
    - compute_priority_score returns weighted sum correctly
    - kind_priority_score returns correct values for each kind
    - count_symbols_with_sir returns accurate counts
    - list_symbol_ids_without_sir returns only symbols missing SIR
    - On-demand request table: write request, read it, clear it
    - TieredProvider: routes high-score symbols to primary
    - TieredProvider: routes low-score symbols to fallback
    - TieredProvider: falls back on primary error when retry_with_fallback = true
    - TieredProvider: propagates primary error when retry_with_fallback = false
    - TieredConfig deserializes correctly from TOML
    - InferenceProviderKind no longer has Mock variant

=== STEP 9: Validation ===

19) Run validation in dependency order:
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

20) Commit with message:
    "Phase 8.2: Progressive indexing + tiered providers — Pass 1/Pass 2 pipeline, priority queue, Mock removal, TieredProvider"

SCOPE GUARD:
- Do NOT add new crates — all changes in existing crates (plus new priority_queue.rs module)
- Do NOT change SurrealDB schema definitions
- Do NOT modify LanceDB table schemas
- Do NOT change the SIR JSON schema (only add optional priority_score to SirContext)
- Do NOT break the `aether index --once` CLI flow (it should still do a full index
  if someone explicitly requests it — add a `--full` flag if needed)
- Do NOT add WebSocket/SSE for live progress updates (polling is fine)
- Do NOT modify dashboard static files in this stage (dashboard will pick up new
  API data automatically via existing endpoints)
- If the current pipeline is fully synchronous, keep it synchronous and use
  std::thread for the background worker — do NOT force async migration
- If any step cannot be applied because the code differs, report what you found and skip

OUTPUT

21) Report:
    - Which steps were applied vs. skipped (with reason)
    - Validation command outcomes (pass/fail per crate)
    - Total lines changed
    - Whether the background worker is thread-based or tokio-based (and why)
    - Number of MockProvider references removed
    - Commit SHA

22) Provide push + PR commands:
    ```
    git -C ../aether-phase8-stage8-2 push -u origin feature/phase8-stage8-2-progressive-indexing
    gh pr create --title "Phase 8.2: Progressive Indexing + Tiered Providers" --body "..." --base main
    ```

==========END CODEX PROMPT==========
```

## Post-Merge Sequence

```bash
cd /home/rephu/projects/aether
git switch main
git pull --ff-only origin main
git log --oneline -3

git worktree remove ../aether-phase8-stage8-2
git branch -d feature/phase8-stage8-2-progressive-indexing
git worktree prune

git status --porcelain
```
