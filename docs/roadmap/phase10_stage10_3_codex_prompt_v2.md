# Codex Prompt — Phase 10.3: Agent Integration Hooks (v2)

CRITICAL BUILD SETTINGS — use these for ALL cargo commands:
```
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

Never `cargo test --workspace` — always per-crate.

Read these files before writing any code:
- `docs/roadmap/phase_10_stage_10_3_agent_hooks_v2.md` (the spec)
- `docs/roadmap/phase10_stage10_3_session_context.md` (session context)
- `crates/aetherd/src/cli.rs` (Commands enum at line ~525 — add new variants)
- `crates/aetherd/src/main.rs` (run_subcommand at line ~310 — add dispatch)
- `crates/aether-store/src/sir_meta.rs` (SirMetaRecord struct — has `prompt_hash`, `staleness_score`, sir read/write operations)
- `crates/aether-store/src/symbols.rs` (SymbolRecord struct — fields: id, file_path, language, kind, qualified_name, signature_fingerprint, last_seen_at. NO content_hash. Also has `get_symbol_record(symbol_id)`)
- `crates/aether-store/src/graph.rs` (SqliteStore::list_graph_dependency_edges, store_get_callers, store_get_dependencies)
- `crates/aether-store/src/lib.rs` (SymbolRelationStore trait: get_callers → Vec<SymbolEdge>, get_dependencies → Vec<SymbolEdge>; SirStateStore trait: read_sir_blob, get_sir_meta, upsert_sir_meta; SymbolCatalogStore trait: search_symbols, list_symbols_for_file)
- `crates/aether-core/src/lib.rs` (Symbol struct — has content_hash; SymbolEdge struct — source_id, target_qualified_name, edge_kind, file_path)
- `crates/aether-sir/src/lib.rs` (SirAnnotation struct — intent, inputs, outputs, side_effects, dependencies, error_modes, confidence, method_dependencies)
- `crates/aether-parse/src/parser.rs` (SymbolExtractor — `extract_from_path(path, source)`, `extract_with_edges_from_path(path, source)`)
- `crates/aetherd/src/batch/ingest.rs` (pattern for SirPipeline construction, `write_fingerprint_row` helper)
- `crates/aetherd/src/batch/hash.rs` (compute_prompt_hash, decompose_prompt_hash, compute_source_hash_segment)
- `crates/aetherd/src/continuous/math.rs` (cosine_distance_from_embeddings — REUSE, do not duplicate)
- `crates/aetherd/src/sir_pipeline/mod.rs` (SirPipeline — persist_sir_payload_into_sqlite, refresh_embedding_if_needed, load_symbol_embedding, UpsertSirIntentPayload)
- `crates/aetherd/src/sir_pipeline/persist.rs` (UpsertSirIntentPayload struct definition)
- `crates/aetherd/src/memory.rs` (project notes query pattern)
- `crates/aetherd/src/test_intents.rs` (test intents query pattern)

## PREFLIGHT

```bash
cd /home/rephu/projects/aether
git status --porcelain
# Must be clean. If dirty, STOP and report.
git pull --ff-only
```

Create branch and worktree:
```bash
git worktree add ../aether-phase10-agent-hooks -b feature/phase10-stage10-3-agent-hooks
cd /home/rephu/aether-phase10-agent-hooks
```

## SOURCE INSPECTION

Before writing code, verify these assumptions. If any are false, STOP and report:

1. Stages 10.1 and 10.2 are merged — schema version is **10**, `sir_fingerprint_history` table exists, `sir.prompt_hash` and `sir.staleness_score` columns exist, `SirMetaRecord` has both `prompt_hash: Option<String>` and `staleness_score: Option<f64>`.
2. `Commands` enum accepts new variants without breaking existing ones. Currently includes `Batch(BatchArgs)` and `Continuous(ContinuousArgs)` from 10.1/10.2.
3. **Structural edges are in SQLite `symbol_edges` table.** Access via `store.get_callers(target_qualified_name)` → `Vec<SymbolEdge>` and `store.get_dependencies(source_id)` → `Vec<SymbolEdge>`. `SymbolEdge` has `source_id`, `target_qualified_name`, `edge_kind`, `file_path`. Do NOT query SurrealDB for edges — SurrealKV exclusive lock would crash if daemon is running.
4. **Coupling data is in SurrealDB only.** There is NO `coupling_pairs` SQLite table. Access via `graph_store.list_co_change_edges_for_file(file_path, min_fused_score)` through `graph_cozo_compat.rs` shim. **Handle gracefully if SurrealDB lock held by daemon:** skip coupling section entirely, append note "coupling data unavailable — daemon may hold SurrealDB lock".
5. **Project memory table is `project_notes`** (NOT `project_memory`). Query via `MemoryNoteStore::list_project_notes()` or `list_project_notes_for_file_ref()` or `search_project_notes_lexical()`.
6. **Test intents** are in `test_intents` table. Query via `TestIntentStore::list_test_intents_for_file()` or `list_test_intents_for_symbol()`.
7. **The SIR table is named `sir`.** Primary key is `id`. Rust struct is `SirMetaRecord` in `sir_meta.rs`. SIR JSON is read via `SirStateStore::read_sir_blob(symbol_id)`. Metadata via `get_sir_meta(symbol_id)`.
8. **Symbol lookup:** `SqliteStore::get_symbol_record(symbol_id)` returns `Option<SymbolRecord>`. For fuzzy search: `SymbolCatalogStore::search_symbols(query, limit)` returns `Vec<SymbolSearchResult>`.
9. **SymbolExtractor API:** `SymbolExtractor::new()` then `extractor.extract_from_path(path, &source)` → `Result<Vec<Symbol>>`. NOT `extract_file()`. Also: `extract_with_edges_from_path(path, &source)` → `Result<ExtractedFile>`.
10. **`symbols` table does NOT have `content_hash`.** It has `signature_fingerprint`. The `Symbol` struct from `aether-core` (returned by tree-sitter parsing) DOES have `content_hash`, but this is not persisted in SQLite. For `sir diff` body-change detection, use the **prompt_hash source segment**: call `decompose_prompt_hash(stored_prompt_hash)` to get the stored source hash, then recompute `compute_source_hash_segment()` on the current source text and compare. If source segment changed → body changed.
11. **Embedding reuse surface:** `SirPipeline::load_symbol_embedding(symbol_id)`, `SirPipeline::refresh_embedding_if_needed(...)`, `SirPipeline::persist_sir_payload_into_sqlite(store, payload)`. Inject needs a `SirPipeline` instance — follow the pattern in `batch/ingest.rs::ingest_results()` which constructs one with `SirPipeline::new(workspace, 1, ProviderOverrides{...})`.
12. **Cosine distance** lives in `crates/aetherd/src/continuous/math.rs` as `cosine_distance_from_embeddings()`. REUSE this for inject delta_sem.
13. **`write_fingerprint_row()`** is in `crates/aetherd/src/batch/ingest.rs`, re-exported via `crates/aetherd/src/batch/mod.rs`. Signature: `write_fingerprint_row(store, symbol_id, prompt_hash, previous_prompt_hash, trigger, generation_model, generation_pass, delta_sem)`.
14. **No schema migration needed for 10.3.** All required columns exist from 10.1/10.2.

## IMPLEMENTATION

### Step 1: CLI wiring

Add to `Commands` enum in `cli.rs`:

```rust
/// Assemble semantic context for a symbol
SirContext(SirContextArgs),
/// Inject or update a symbol's SIR intent
SirInject(SirInjectArgs),
/// Show SIR vs source code delta
SirDiff(SirDiffArgs),
```

```rust
#[derive(Debug, Clone, Args)]
pub struct SirContextArgs {
    /// Symbol selector: qualified name, symbol ID, or fuzzy search
    pub selector: String,
    /// Output format
    #[arg(long, default_value = "markdown")]
    pub format: String,
    /// Max token budget
    #[arg(long, default_value_t = 16000)]
    pub max_tokens: usize,
    /// Dependency depth (1-3)
    #[arg(long, default_value_t = 1)]
    pub depth: u32,
    /// Sections to include (comma-separated)
    #[arg(long)]
    pub include: Option<String>,
    /// Write to file instead of stdout
    #[arg(long)]
    pub output: Option<String>,
    /// Read symbol selectors from file (one per line)
    #[arg(long)]
    pub symbols: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct SirInjectArgs {
    /// Symbol selector
    pub selector: String,
    /// Intent text
    #[arg(long)]
    pub intent: String,
    /// Behavior summary
    #[arg(long)]
    pub behavior: Option<String>,
    /// Edge cases
    #[arg(long)]
    pub edge_cases: Option<String>,
    /// Overwrite even if existing SIR has higher quality
    #[arg(long)]
    pub force: bool,
    /// Show what would change without persisting
    #[arg(long)]
    pub dry_run: bool,
    /// Skip re-embedding
    #[arg(long)]
    pub no_embed: bool,
}

#[derive(Debug, Clone, Args)]
pub struct SirDiffArgs {
    /// Symbol selector
    pub selector: String,
}
```

Wire all three in `run_subcommand()` in `main.rs`.

### Step 2: Symbol resolution

Create a shared helper `resolve_symbol(store: &SqliteStore, selector: &str) -> Result<SymbolRecord>`:
1. Try `store.get_symbol_record(selector)` — exact match on symbol ID
2. Try `store.search_symbols(selector, 10)` — search by qualified name
3. Filter results: if exactly one match → return it
4. If multiple matches → return error listing candidates with qualified_name and file_path
5. If zero matches → return error "symbol not found: {selector}"

### Step 3: `sir context` implementation

Create `crates/aetherd/src/sir_context.rs`:

**Token budget knapsack:**

```rust
const CHARS_PER_TOKEN: f64 = 3.5;

struct BudgetAllocator {
    max_tokens: usize,
    used_tokens: usize,
}

impl BudgetAllocator {
    fn remaining(&self) -> usize { self.max_tokens.saturating_sub(self.used_tokens) }
    fn try_add(&mut self, content: &str) -> Option<String> {
        let tokens = (content.len() as f64 / CHARS_PER_TOKEN) as usize;
        if tokens <= self.remaining() {
            self.used_tokens += tokens;
            Some(content.to_string())
        } else {
            None
        }
    }
}
```

**Assembly order (strict priority):**

1. Target symbol source code + SIR intent/behavior (MUST include — fail if this alone exceeds budget)
2. Test intents guarding this symbol (from `test_intents` SQLite table via `store.list_test_intents_for_file()` or `store.list_test_intents_for_symbol()`)
3. 1-hop dependency intents (from SQLite `symbol_edges` via `store.get_dependencies(symbol_record.id)` → then SIR lookup via `store.read_sir_blob(target_id)` for each edge. **NOTE:** `get_dependencies` returns `Vec<SymbolEdge>` with `target_qualified_name`, not target ID. You need to resolve the target symbol ID by searching for the qualified name.)
4. 1-hop caller signatures (from SQLite `symbol_edges` via `store.get_callers(symbol_record.qualified_name)` → source extract for each caller)
5. Coupling data (from SurrealDB `list_co_change_edges_for_file()` — **handle gracefully if SurrealDB lock held by daemon: skip section and note "coupling unavailable"**)
6. Project memory notes (from SQLite `project_notes` table via `store.list_project_notes_for_file_ref(file_path)` or `store.search_project_notes_lexical(symbol_name, limit)`)
7. Recent git changes (from `GitContext::open(workspace)` → `git.file_log(path, 5)` for the file, last 5 commits)
8. Health/staleness scores (from `sir` table — `staleness_score` column on SirMetaRecord, if populated by 10.2)
9. 2-hop transitive deps (only if depth >= 2 and budget allows)

For each tier: attempt to add. If budget exhausted mid-tier, include what fits and append truncation notice.

**Output formats:**
- `markdown` — structured markdown (see template below)
- `json` — serde-serializable struct → `serde_json::to_string_pretty()`
- `text` — plain text, minimal formatting

**Markdown output template:**

```markdown
# Symbol: {qualified_name}

**Kind:** {kind} | **File:** {file_path} | **Staleness:** {staleness_score:.2}

## Source
```{lang}
{source_code}
```

## Intent
{sir.intent}

## Behavior
{sir.side_effects joined, sir.error_modes joined}

## Test Guards
{for each test intent}
- `{test_name}` — "{description}"

## Dependencies (1 hop)
{for each dep}
- `{qualified_name}` — {sir_intent_first_sentence}

## Callers
{for each caller}
- `{qualified_name}` ({file_path})

## Coupling
{for each coupled file}
- `{file_path}` — fused {fused_score:.2}

## Memory
{for each note}
- {content_first_line} ({source_type}, {created_at})

## Recent Changes
{for each commit}
- {relative_date}: {message_first_line} ({short_sha})

> [Context budget: {used_tokens} / {max_tokens} tokens used]
```

### Step 4: `sir inject` implementation

Create `crates/aetherd/src/sir_inject.rs`:

1. Resolve symbol via `resolve_symbol()`
2. Load existing SIR JSON from `store.read_sir_blob(symbol_id)` — parse as `SirAnnotation` or create empty one
3. Load existing SirMetaRecord from `store.get_sir_meta(symbol_id)`
4. **CRITICAL ORDER FOR delta_sem:** If embeddings enabled AND NOT `--no-embed`:
   a. **FIRST: Create a `SirPipeline` instance** (follow `batch/ingest.rs::ingest_results()` pattern)
   b. **Load the OLD embedding** via `pipeline.load_symbol_embedding(symbol_id)` BEFORE any writes
5. Update SirAnnotation fields:
   - `intent` = args.intent
   - `side_effects` = parse args.behavior into vec if provided (or leave existing)
   - `error_modes` = parse args.edge_cases into vec if provided (or leave existing)
   - `confidence` = 0.5 (default for injected)
6. If `--dry-run`: print diff of old vs new SIR fields, exit without writing
7. Build `UpsertSirIntentPayload` (from `sir_pipeline/persist.rs`):
   ```rust
   let payload = UpsertSirIntentPayload {
       symbol: symbol_from_record(&symbol_record)?,  // adapter from batch/ingest.rs
       sir: updated_sir,
       provider_name: "manual".to_owned(),
       model_name: "manual".to_owned(),
       generation_pass: "injected".to_owned(),
       commit_hash: None,
   };
   ```
8. Persist via `pipeline.persist_sir_payload_into_sqlite(store, &payload)`
9. Compute new `prompt_hash` — for inject, source and config components are the same as current, only the SIR content changed. Use `compute_prompt_hash(source_text, &[], "manual:inject:0")`
10. Update prompt_hash on the sir row:
    ```rust
    store.upsert_sir_meta(SirMetaRecord { prompt_hash: Some(new_hash), ..current_meta })?;
    ```
11. If embeddings enabled AND NOT `--no-embed`:
    a. Refresh embedding via `pipeline.refresh_embedding_if_needed(...)`
    b. Load new embedding via `pipeline.load_symbol_embedding(symbol_id)`
    c. Compute delta_sem via `cosine_distance_from_embeddings(old_embedding.as_ref(), new_embedding.as_ref())` from `continuous/math.rs`
12. Write fingerprint history row via `write_fingerprint_row()`:
    - `trigger = "inject"`
    - `source_changed = false`, `neighbor_changed = false`, `config_changed = false`
    - `delta_sem` from step 11c (or None if embeddings disabled/skipped)
    - `generation_model = "manual"`, `generation_pass = "injected"`
13. Print confirmation: "Updated SIR for {qualified_name}. Intent: {first 80 chars}..."

### Step 5: `sir diff` implementation

Create `crates/aetherd/src/sir_diff.rs`:

**IMPORTANT:** Do NOT attempt to manually parse parameters, return types, or visibility from tree-sitter. Use fingerprint comparison and prompt hash decomposition.

1. Resolve symbol — get `SymbolRecord` from SQLite
2. Load current SIR metadata from `store.get_sir_meta(symbol_id)`
3. Load current SIR JSON from `store.read_sir_blob(symbol_id)` — parse as `SirAnnotation`
4. Re-parse the symbol's source file:
   ```rust
   let mut extractor = SymbolExtractor::new()?;
   let source = fs::read_to_string(workspace.join(&symbol_record.file_path))?;
   let symbols = extractor.extract_from_path(
       Path::new(&symbol_record.file_path),
       &source,
   )?;
   let fresh_symbol = symbols.iter()
       .find(|s| s.qualified_name == symbol_record.qualified_name)
       .ok_or_else(|| anyhow!("symbol no longer found in source file"))?;
   ```
5. Compare:
   - **Signature changed?** `fresh_symbol.signature_fingerprint != symbol_record.signature_fingerprint`
   - **Body changed?** Use prompt hash source segment: `decompose_prompt_hash(stored_prompt_hash)` gives the stored source hash. Recompute `compute_source_hash_segment(fresh_source_text)` and compare. If different → body changed. If no stored prompt_hash → report "unknown (no prompt hash recorded)".
   - **Symbol moved?** Compare `fresh_symbol.range` start line against stored range (if available — range is on `Symbol` from parse but not in `SymbolRecord`. If unavailable, skip.)
6. Check SIR metadata:
   - How old is the SIR? (`meta.updated_at`)
   - Which model/pass generated it? (`meta.model`, `meta.generation_pass`)
   - Staleness score? (`meta.staleness_score`)
7. Output structured diff:

```
Symbol: {qualified_name}
SIR generated: {days} days ago ({generation_pass} pass, {model})
Staleness score: {staleness_score:.2}

Changes detected:
  [SIGNATURE] Signature fingerprint changed (function signature modified)
  [BODY] Source hash changed (function body modified)

Recommendation: SIR is stale. Run `aetherd sir inject` or wait for watcher re-index.
```

If no changes: "SIR appears current. No structural drift detected."

## SCOPE GUARD — Do NOT modify

- Batch pipeline (10.1)
- Continuous intelligence (10.2)
- Existing CLI subcommands
- Existing SIR pipeline
- Existing watcher behavior
- Existing MCP tools

## VALIDATION GATE

```bash
cargo fmt --all --check
cargo clippy -p aetherd -- -D warnings
cargo test -p aetherd
```

Verify CLI:
```bash
$CARGO_TARGET_DIR/debug/aetherd sir-context --help
$CARGO_TARGET_DIR/debug/aetherd sir-inject --help
$CARGO_TARGET_DIR/debug/aetherd sir-diff --help
```

**NOTE:** clap converts `SirContext` → `sir-context` (kebab-case) by default. Verify the actual subcommand names match what clap generates.

### Validation criteria

1. All tests pass, zero clippy warnings
2. All three `--help` outputs show expected flags
3. Symbol resolution: exact ID match, qualified name search, ambiguous error with candidates
4. Context assembly: token budget enforced, tiers in correct priority order, truncation notice when budget exceeded
5. Inject with `--dry-run` shows diff without writing
6. Inject without `--dry-run` persists SIR, updates prompt_hash, writes fingerprint history row with `trigger = "inject"`
7. Inject with embeddings: old embedding loaded BEFORE persist, delta_sem computed correctly
8. Diff detects signature changes via fingerprint comparison
9. Diff detects body changes via prompt hash source segment comparison
10. Coupling section gracefully skipped when SurrealDB unavailable

## COMMIT

```bash
git add -A
git commit -m "Phase 10.3: Agent integration hooks — sir context, inject, diff

sir context:
- Token-budgeted context assembly with greedy knapsack (9-tier priority)
- Pulls SIR, edges, coupling, test intents, memory, git history, staleness
- Markdown/JSON/text output formats
- Truncation notice when budget exhausted
- Graceful SurrealDB unavailability handling for coupling section

sir inject:
- Direct SIR update without inference
- Synchronous re-embedding with old-before-new ordering for delta_sem
- Fingerprint history row with trigger='inject'
- --dry-run and --no-embed flags
- Prompt hash update for consistency with batch pipeline

sir diff:
- Structural comparison via signature fingerprint and prompt hash source segment
- No inference required — tree-sitter only
- Reports SIR age, model, generation pass, staleness score
- Actionable recommendation in output"
```

**PR title:** Phase 10.3: Agent integration hooks — sir context, inject, diff
**PR body:** Adds three CLI commands for AI agent integration: `sir-context` (token-budgeted semantic context assembly), `sir-inject` (direct SIR update with re-embedding), and `sir-diff` (structural drift detection). Context assembly uses a 9-tier priority knapsack with configurable token budget. Inject writes fingerprint history for traceability. Diff uses signature fingerprints and prompt hash decomposition — no inference needed.

Do NOT push automatically. Report commit SHA and wait for review.

Push command (after review):
```bash
git push -u origin feature/phase10-stage10-3-agent-hooks
```
