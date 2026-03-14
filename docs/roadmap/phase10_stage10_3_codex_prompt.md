# Codex Prompt — Phase 10.3: Agent Integration Hooks

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
- `crates/aetherd/src/cli.rs` (Commands enum — add new variants)
- `crates/aetherd/src/main.rs` (run_subcommand — add dispatch)
- `crates/aether-dashboard/src/api/blast_radius.rs` (example of edge querying pattern)
- `crates/aetherd/src/coupling.rs` (coupling query pattern)
- `crates/aetherd/src/test_intents.rs` (test intents query pattern)
- `crates/aetherd/src/memory.rs` (project memory query pattern)
- `crates/aether-parse/src/` (tree-sitter parsing for sir diff)

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

1. Stage 10.1 is merged — `sir_fingerprint_history` table exists, `sir.prompt_hash` column exists
2. `Commands` enum accepts new variants without breaking existing ones
3. **Structural edges are in SQLite `symbol_edges` table.** Access via `store.get_callers(qualified_name)` and `store.get_dependencies(symbol_id)`. Do NOT query SurrealDB for edges — SurrealKV exclusive lock would crash if daemon is running. Check `crates/aether-store/src/graph.rs` for `store_get_callers` and `store_get_dependencies`.
4. **Coupling data is in SurrealDB**, accessed via `graph_store.list_co_change_edges_for_file()`. There is NO `coupling_pairs` SQLite table. For `sir context` running alongside the daemon, coupling data may be unavailable due to SurrealKV lock — handle gracefully (skip coupling section, note "coupling data unavailable — daemon holds SurrealDB lock").
5. **Project memory table is `project_notes`** (NOT `project_memory`). Check `crates/aether-store/src/schema.rs` for column names and `crates/aetherd/src/memory.rs` for query pattern.
6. `test_intents` table exists — verify column names and query pattern in `crates/aetherd/src/test_intents.rs`
7. **The SIR table is named `sir` (NOT `sir_meta`).** Primary key is `id` (NOT `symbol_id`). Rust struct is `SirMetaRecord` in `sir_meta.rs`. SIR JSON is in the `sir_json` column.
8. Embedding provider — find how to construct one from config and call `embed_text()` or similar for a single string
9. `write_fingerprint_row()` or equivalent from 10.1's batch module — verify function name and signature
10. **`SymbolRecord` in `symbols.rs` has `signature_fingerprint` field.** The `symbols` table also stores it. For `sir diff`, re-parse the file to get a fresh `Symbol` (from `aether-core`) and compare its `signature_fingerprint` and `content_hash` against the stored `SymbolRecord.signature_fingerprint`. Do NOT attempt to extract parameter names/types/return types from tree-sitter manually.

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

Wire all three in `run_subcommand()`.

### Step 2: Symbol resolution

Create a shared helper `resolve_symbol(store, selector) -> Result<SymbolRecord>` that:
1. Try exact match on `qualified_name`
2. Try exact match on `symbol_id`
3. Try fuzzy/prefix match on `qualified_name`
4. Return error if ambiguous (multiple matches) — list candidates

Reuse existing search infrastructure if available.

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
    fn remaining(&self) -> usize { self.max_tokens - self.used_tokens }
    fn try_add(&mut self, content: &str) -> Option<String> {
        let tokens = (content.len() as f64 / CHARS_PER_TOKEN) as usize;
        if tokens <= self.remaining() {
            self.used_tokens += tokens;
            Some(content.to_string())
        } else {
            None  // budget exhausted
        }
    }
}
```

**Assembly order (strict priority):**

1. Target symbol source code + SIR intent/behavior (MUST include — fail if this alone exceeds budget)
2. Test intents guarding this symbol (from `test_intents` SQLite table)
3. 1-hop dependency intents (from SQLite `symbol_edges` via `store.get_dependencies()` → then SIR lookup by id from `sir` table)
4. 1-hop caller signatures (from SQLite `symbol_edges` via `store.get_callers()` → source extract)
5. Coupling data (from SurrealDB `list_co_change_edges_for_file()` — **handle gracefully if SurrealDB lock held by daemon: skip section and note "coupling unavailable"**)
6. Project memory notes (from SQLite `project_notes` table — NOT `project_memory`)
7. Recent git changes (from gix log for the file, last 5 commits)
8. Health/staleness scores (from `sir` table — `staleness_score` column)
9. 2-hop transitive deps (only if depth >= 2 and budget allows)

For each tier: attempt to add. If budget exhausted mid-tier, include what fits and append truncation notice.

**Markdown output template:**

```markdown
# Symbol: {qualified_name}

**Kind:** {kind} | **File:** {file}:{start_line}-{end_line} | **Staleness:** {staleness_score}

## Source
```{lang}
{source_code}
```

## Intent
{sir_intent}

## Behavior
{sir_behavior}

## Test Guards
{for each test intent}
- `{test_name}` — "{description}"

## Dependencies (1 hop)
{for each dep}
- `{qualified_name}` — {sir_intent_first_sentence}

## Callers
{for each caller}
- `{qualified_name}({signature})`

## Coupling
{for each coupled file/symbol}
- `{name}` — temporal {co_change}, semantic {semantic}, structural {static}

## Memory
{for each note}
- {note_text} ({date})

## Recent Changes
{for each commit}
- {relative_date}: {commit_message_first_line} ({short_sha})

> [Context budget: {used_tokens} / {max_tokens} tokens used]
```

**JSON output:** Same data structure as a serde-serializable struct, `serde_json::to_string_pretty()`.

### Step 4: `sir inject` implementation

Create `crates/aetherd/src/sir_inject.rs`:

1. Resolve symbol
2. Load existing SIR from `sir` table (by `id`, NOT `symbol_id` — NOT from `sir_meta` table) — or create empty SirAnnotation
3. **CRITICAL ORDER FOR delta_sem:** If embeddings enabled AND NOT `--no-embed`:
   a. **FIRST: Fetch the OLD embedding** from the vector store BEFORE any writes. Store it in memory. If you upsert the new embedding first, the old vector is permanently overwritten and delta_sem cannot be computed.
4. Update SIR fields:
   - `intent` = args.intent
   - `behavior` = args.behavior (if provided)
   - `edge_cases` = args.edge_cases (if provided)
   - `generation_pass` = `"injected"`
   - `model` = `"manual"` (this is the `model` column in the `sir` table)
   - `updated_at` = now (Unix timestamp)
   - `confidence` = 0.5 (default for injected)
5. If `--dry-run`: print diff of old vs new SIR fields, exit without writing
6. Persist to SQLite via existing SIR upsert function (writes to `sir` table)
7. Mirror to `.aether/sirs/` if `config.storage.mirror_sir_files`
8. If embeddings enabled AND NOT `--no-embed`:
   a. Construct embedding provider from config
   b. Embed the new SIR intent text
   c. Write embedding to vector store (this overwrites the old embedding)
   d. Compute `delta_sem` = cosine_distance(old_embedding, new_embedding) using the vector saved in step 3a
9. Compute new `prompt_hash` and update the `sir` row
10. Write fingerprint history row:
    - `trigger = "inject"`
    - `source_changed = 0`, `neighbor_changed = 0`, `config_changed = 0`
    - `delta_sem` from step 8d (or NULL if embeddings disabled)
11. Print confirmation: "Updated SIR for {qualified_name}. Intent: {first 80 chars}..."

### Step 5: `sir diff` implementation

Create `crates/aetherd/src/sir_diff.rs`:

**IMPORTANT: Do NOT attempt to manually parse parameters, return types, or visibility from tree-sitter.** `aether-parse` provides `Symbol` structs with `signature_fingerprint` and `content_hash` fields, but does not extract detailed AST information like individual parameter names/types. Use fingerprint comparison, not AST extraction.

1. Resolve symbol — get the `SymbolRecord` from SQLite (has `signature_fingerprint`)
2. Load current SIR from `sir` table (by `id`)
3. Re-parse the symbol's source file via `SymbolExtractor` from `aether-parse`:
   ```rust
   let mut extractor = SymbolExtractor::new()?;
   let extracted = extractor.extract_file(file_path, &source_content)?;
   ```
4. Find the matching `Symbol` in the extracted results (by `qualified_name` or `id`)
5. Compare fingerprints:
   - **Signature changed?** Compare `extracted_symbol.signature_fingerprint` vs `stored_record.signature_fingerprint`. If different → `[SIGNATURE] Function signature changed since last SIR generation`
   - **Body changed?** Compare `extracted_symbol.content_hash` vs stored content hash (if available — `content_hash` is on the `Symbol` struct from parsing but may not be in the `symbols` SQLite table). Alternative: compare the source text length or line count as a rough body-change indicator.
   - **File context changed?** Check if the symbol's line range shifted (symbol moved within file)
6. Check SIR metadata:
   - How old is the SIR? (`updated_at` from `sir` table)
   - Which model/pass generated it?
   - What's the staleness score? (if computed by 10.2)
7. Output structured diff:

```
Symbol: my_crate::payments::validate_amount
SIR generated: 2 days ago (triage pass, gemini-3.1-flash-lite-preview)

Changes detected:
  [SIGNATURE] Signature fingerprint changed (function signature modified)
  [BODY] Content hash changed (function body modified)

Recommendation: SIR is stale. Run `aetherd sir inject` or wait for watcher re-index.
```

If no changes detected: "SIR appears current. No structural drift detected."

## VALIDATION GATE

```bash
cargo fmt --all --check
cargo clippy -p aetherd -- -D warnings
cargo test -p aetherd
```

Verify CLI:
```bash
./target/debug/aetherd sir context --help
./target/debug/aetherd sir inject --help
./target/debug/aetherd sir diff --help
```

### Validation criteria

1. All tests pass, zero clippy warnings
2. All three `--help` outputs show expected flags
3. If possible, run on real repo:
   ```bash
   pkill -f aetherd
   rm -f /home/rephu/projects/aether/.aether/graph/LOCK
   cargo build -p aetherd --release
   $CARGO_TARGET_DIR/release/aetherd sir context aether_store::Store::get_sir \
     --workspace /home/rephu/projects/aether --max-tokens 8000
   ```
4. Verify context output contains SIR intent, dependencies, and token budget line
5. Verify inject with `--dry-run` shows diff without writing

## COMMIT

```bash
git add -A
git commit -m "Phase 10.3: Agent integration hooks — sir context, inject, diff

sir context:
- Token-budgeted context assembly with greedy knapsack (9-tier priority)
- Pulls SIR, edges, coupling, test intents, memory, git history, health
- Markdown/JSON/text output formats
- Truncation notice when budget exhausted

sir inject:
- Direct SIR update without inference
- Synchronous re-embedding for read-after-write consistency
- Fingerprint history row with trigger='inject'
- --dry-run and --no-embed flags

sir diff:
- Structural comparison via tree-sitter (no inference)
- Detects signature, visibility, complexity, and dependency changes
- Actionable recommendation in output"
```

Do NOT push automatically. Report commit SHA and wait for review.

Push command (after review):
```bash
git push -u origin feature/phase10-stage10-3-agent-hooks
```
