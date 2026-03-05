CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- RUSTC_WRAPPER=sccache
- TMPDIR=/home/rephu/aether-target/tmp (mkdir -p first)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

PREFLIGHT:
  git status --porcelain  # must be clean
  git pull --ff-only

BRANCH:
  git worktree add /home/rephu/aether-worktrees/phase8-stage8-8 -b feature/phase8-stage8-8 main

cd /home/rephu/aether-worktrees/phase8-stage8-8

You are working in https://github.com/rephug/aether.
Read AGENTS.md for workspace conventions.
Read docs/hardening/phase8_stage8_8_session_context.md for full architectural context and benchmark findings.

## Context

AETHER's SIR generation uses a single prompt (`build_strict_json_prompt` in
`crates/aether-infer/src/lib.rs:1107`) for all symbol types — no kind-awareness,
no few-shot examples, no enriched context. Benchmarking across 12 providers showed:

1. Kind-aware prompts eliminate lazy intents on simple symbols.
2. Enriched context (neighbor intents + triage SIR + source code) improves quality
   more than switching to a more expensive model.
3. A two-pass pipeline (fast triage then enriched deep) produces high quality at 83%
   less cost than running a premium model on everything.
4. Local qwen3.5:4b benefits from enrichment when using Chain-of-Thought mode
   (thinking enabled, 8192 context) — the bracket parser is required to extract
   JSON from the mixed thinking+JSON output.

This stage implements: bracket parser, kind-aware prompts, enriched deep pass prompts,
two-pass pipeline, local CoT deep mode, pipeline-level fallback, and regeneration CLI.

## Task 1: Extend normalize_candidate_json (bracket parser)

File: `crates/aether-infer/src/lib.rs`

The existing `normalize_candidate_json()` function (around line 1512) strips markdown
fences but does not handle raw text wrapping a JSON object. This is critical for the
local CoT deep pass which outputs `<thinking>...</thinking>` blocks before JSON.

After the existing fence-stripping logic, add a fallback path:

1. Check if the current result starts with `{`. If it does, return it (already clean).
2. Otherwise, find the index of the first `{` and last `}` in the string.
3. If both found and start < end, extract that slice as the candidate.
4. Apply trailing-comma cleanup on the extracted string:
   - Replace patterns where a comma appears before a closing `]` or `}` with
     just the closing bracket. Handle optional whitespace between the comma
     and bracket. Use simple string iteration or the regex crate if it is
     already a dependency (check Cargo.toml — do NOT add regex if not present).
5. Return the cleaned string.

Do NOT create a separate `extract_sir_json()` function. Extend `normalize_candidate_json()`.

Add unit tests in the same file's test module:
- Input: `<thinking>analysis here</thinking>\n{"intent":"test"}` -> extracts JSON
- Input: `Here is the SIR:\n{"intent":"test","inputs":[]}` -> extracts JSON
- Input: `{"inputs":["a","b",],"intent":"test"}` -> strips trailing comma
- Input: `{"intent":"test"}` -> returns unchanged
- Input: `no json here at all` -> returns unchanged (let serde handle the error)
- Input: `<thinking>stuff</thinking>\n` + "```json\n{...}\n```" -> fence stripping takes priority

## Task 2: Kind-aware prompt templates

Create new file: `crates/aether-infer/src/sir_prompt.rs`
Add `pub mod sir_prompt;` to `crates/aether-infer/src/lib.rs`.

### 2a. SirContext changes

Add three fields to the existing `SirContext` struct in `crates/aether-infer/src/lib.rs`:

```rust
pub struct SirContext {
    pub language: String,
    pub file_path: String,
    pub qualified_name: String,
    pub priority_score: Option<f64>,
    pub kind: String,           // NEW — "struct", "function", "method", etc.
    pub is_public: bool,        // NEW — whether the symbol has pub visibility
    pub line_count: usize,      // NEW — number of lines in the symbol text
}
```

Update ALL call sites that construct `SirContext` to populate the new fields:
- In `sir_pipeline.rs`, `build_job()` has access to the `Symbol` struct which has `kind`.
  For `is_public`, check if the symbol text starts with "pub " or use any existing
  `infer_symbol_is_public()` helper. For `line_count`, count newlines in symbol_text.
- In file-level SIR generation, set kind="file", is_public=true, line_count=file line count.

### 2b. build_sir_prompt_for_kind

```rust
pub fn build_sir_prompt_for_kind(
    symbol_text: &str,
    context: &SirContext,
) -> String
```

This function replaces `build_strict_json_prompt()` for all providers.

All variants share this base:
- "You are generating a Leaf SIR annotation."
- Strict JSON requirement with exact 7 fields
- Context block (language, file_path, qualified_name)
- Symbol text
- 2-3 few-shot examples as const strings (~200 tokens total)

Kind-specific additions appended to the base prompt:

For struct, enum, trait, type_alias:
"For type definitions: describe WHY this type exists, not just WHAT it is.
List contained or extended types as dependencies.
Inputs and outputs should be empty arrays for type definitions.
Good intent example: 'Database handle struct that holds a reference-counted pointer to shared database state, enabling multiple owners including a background task'
Bad intent example: 'Define a database structure'"

For function or method where is_public=true AND line_count > 30:
"For complex public methods: enumerate each distinct return path in outputs
(Ok/Err/None) with WHEN each occurs. Describe what each input parameter represents
and its purpose. For error_modes, describe specific failure conditions with
propagation details — follow the ? operator chain.
Good outputs example: ['Ok(Some(Frame)) when a complete frame has been parsed', 'Ok(None) when the remote peer cleanly closed the connection', 'Err when the connection was reset mid-frame']
Bad outputs example: ['crate::Result<Option<Frame>>']"

For function or method where is_public=false OR line_count <= 30:
"Even for simple functions, provide a descriptive intent — not just a single word
like 'getter' or 'constructor'. Describe what the function accomplishes and why it exists."

For functions where qualified_name contains "test_" or kind contains "test":
"For test functions: describe what behavior is being verified and under what conditions.
For dependencies, list the production code under test."

### 2c. Wire into providers

Update `Qwen3LocalProvider::generate_sir()`, `GeminiProvider::generate_sir()`, and
`OpenAiCompatProvider::generate_sir()` to call `sir_prompt::build_sir_prompt_for_kind()`
instead of `build_strict_json_prompt()`.

The old `build_strict_json_prompt()` function should be removed or marked deprecated.

### 2d. build_enriched_sir_prompt

In `sir_prompt.rs`:

```rust
pub struct SirEnrichmentContext {
    /// File-level rollup intent from triage pass
    pub file_intent: Option<String>,
    /// Intents of neighboring symbols in the same file
    pub neighbor_intents: Vec<(String, String)>,  // (qualified_name, intent)
    /// The triage-pass SIR to improve upon
    pub baseline_sir: Option<SirAnnotation>,
    /// Human-readable explanation of why this symbol was selected for deep pass
    pub priority_reason: String,
}

pub fn build_enriched_sir_prompt(
    symbol_text: &str,
    context: &SirContext,
    enrichment: &SirEnrichmentContext,
) -> String
```

The enriched prompt structure:

"You are improving an existing SIR annotation with deeper analysis.

Context:
- language: {language}
- file_path: {file_path}
- qualified_name: {qualified_name}
- priority: {priority_reason}

File purpose: {file_intent or '(not available)'}

Other symbols in this file:
{for each neighbor, up to max_neighbors: '- {name}: {intent}'}

Previous SIR (improve upon this):
{baseline_sir as JSON}

{kind-specific guidance from build_sir_prompt_for_kind}

Symbol text:
{source code}

Focus on: more specific intent, complete error propagation paths, all side effects
including conditional mutations, correct confidence reflecting your certainty.

Respond with STRICT JSON only. Exactly these fields: intent, inputs, outputs,
side_effects, dependencies, error_modes, confidence."

Truncate neighbor_intents to the configured max (default 10), dropping lowest-priority first.

### 2e. build_enriched_sir_prompt_with_cot

For local models using Chain-of-Thought mode, a variant prompt:

```rust
pub fn build_enriched_sir_prompt_with_cot(
    symbol_text: &str,
    context: &SirContext,
    enrichment: &SirEnrichmentContext,
) -> String
```

Same content as build_enriched_sir_prompt but with added instructions:

"Before outputting JSON, you MUST wrap your analysis inside <thinking> tags:
1. What crucial runtime behaviors are missing from the previous SIR?
2. How do the neighboring symbols dictate how this symbol should be used?
3. What fields will you expand?

After closing your </thinking> tag, output the final JSON."

This prompt is used when the provider is Qwen3Local and deep_pass is enabled.

## Task 3: Two Ollama body builders

File: `crates/aether-infer/src/lib.rs`

The existing `build_ollama_generate_body()` is the triage/fast mode. Add a second:

```rust
fn build_ollama_deep_generate_body(model: &str, prompt: &str) -> Value {
    json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
        // NOTE: no "format": "json" — conflicts with thinking mode
        // NOTE: no "think": false — we WANT thinking for deep pass
        "options": {
            "temperature": 0.3,
            "num_ctx": 8192
        }
    })
}
```

Key differences from triage body:
- No `"format": "json"` — this forces the model into JSON-only mode and prevents thinking
- No `"think": false` — we want the model to reason before producing JSON
- `num_ctx: 8192` instead of 4096 — enriched prompt + thinking needs more room
- `temperature: 0.3` instead of the configured SIR temperature — slightly more creative for analysis

The Qwen3LocalProvider needs a new method for deep pass inference:

```rust
async fn request_deep_candidate_json_with_prompt(&self, prompt: String) -> Result<String, InferError>
```

This method uses `build_ollama_deep_generate_body()` and the response goes through
`normalize_candidate_json()` (which now has the bracket parser) before being returned.

When the deep pass is active and provider is Qwen3Local, use
`build_enriched_sir_prompt_with_cot()` for the prompt and
`request_deep_candidate_json_with_prompt()` for the HTTP call.

## Task 4: SirQualityConfig

File: `crates/aether-config/src/lib.rs`

Add a new config struct:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SirQualityConfig {
    #[serde(default)]
    pub deep_pass: bool,

    #[serde(default = "default_deep_priority_threshold")]
    pub deep_priority_threshold: f64,

    #[serde(default = "default_deep_confidence_threshold")]
    pub deep_confidence_threshold: f64,

    #[serde(default)]
    pub deep_provider: Option<String>,

    #[serde(default)]
    pub deep_model: Option<String>,

    #[serde(default)]
    pub deep_endpoint: Option<String>,

    #[serde(default)]
    pub deep_api_key_env: Option<String>,

    #[serde(default = "default_deep_max_symbols")]
    pub deep_max_symbols: usize,

    #[serde(default = "default_deep_max_neighbors")]
    pub deep_max_neighbors: usize,

    #[serde(default = "default_deep_concurrency")]
    pub deep_concurrency: usize,
}
```

Defaults: deep_pass=false, deep_priority_threshold=0.7, deep_confidence_threshold=0.85,
deep_max_symbols=0 (no limit), deep_max_neighbors=10, deep_concurrency=4.

Add `pub sir_quality: SirQualityConfig` to `AetherConfig` with `#[serde(default)]`.

Also add `pub concurrency: usize` to `InferenceConfig` with default 2 (`default_sir_concurrency`).
This config-based concurrency setting should be used by the SIR pipeline as a fallback
when `--sir-concurrency` is not specified on the CLI.

Add validation: if `sir_quality.deep_pass` is true and provider is `qwen3_local` and
no deep_provider is configured, log info: "Deep pass will use local CoT mode (thinking enabled, 8192 context)."

## Task 5: generation_pass column

File: `crates/aether-store/src/lib.rs`

Add `pub generation_pass: String` to `SirMetaRecord` with default value `"single"`.

Add SQLite migration in the migration chain:
```sql
ALTER TABLE sir ADD COLUMN generation_pass TEXT DEFAULT 'single';
```

Handle the migration safely — ALTER TABLE with DEFAULT is safe for existing rows.

Update `upsert_sir_meta()` to include generation_pass in the INSERT/UPDATE.
Update `get_sir_meta()` to read generation_pass, defaulting to "single" for rows
where the column is NULL (pre-migration data).

Valid values: "single", "triage", "deep", "premium", "regenerated".

## Task 6: Priority scores in batch mode

File: `crates/aetherd/src/indexer.rs`

In `run_full_index_once_inner()`, after collecting `symbols_by_file` (around line 194),
compute priority scores for all candidate symbols:

```rust
// Collect all symbols for priority scoring
let all_candidate_symbols: Vec<Symbol> = symbols_by_file
    .values()
    .flat_map(|syms| syms.iter().cloned())
    .collect();
let priority_scores = compute_symbol_priority_scores(workspace, &store, &all_candidate_symbols);
```

Then in the per-file loop, look up each file's max priority score and pass it:

```rust
for (file_path, symbols) in symbols_by_file {
    let max_priority = symbols.iter()
        .filter_map(|s| priority_scores.get(s.id.as_str()).copied())
        .fold(0.0f64, f64::max);

    // ... build event ...

    sir_pipeline.process_event_with_priority(
        &store, &event, config.force, config.print_sir, &mut stdout,
        Some(max_priority),
    )?;
}
```

This enables TieredProvider routing in batch mode. Currently all symbols go to fallback
because priority_score is None (treated as 0.0).

## Task 7: Two-pass pipeline (--deep flag)

File: `crates/aetherd/src/cli.rs` — add `--deep` flag (bool, default false)
File: `crates/aetherd/src/indexer.rs`

Add `pub deep: bool` to `IndexerConfig`.

When `--index-once --full --deep` is specified:

**Pass 2A (triage):** Run normal SIR generation for all symbols using the main provider.
Set `generation_pass = "triage"` in the SirMetaRecord for all generated SIR.

**Pass 2B (deep):** After triage completes:

1. Query the store for all SIR records generated in the triage pass.
2. Filter to symbols where:
   - `priority_score > config.sir_quality.deep_priority_threshold` OR
   - `confidence < config.sir_quality.deep_confidence_threshold`
3. Cap at `config.sir_quality.deep_max_symbols` if > 0.
4. Sort by priority_score descending (highest priority first).
5. For each candidate symbol, build `SirEnrichmentContext`:
   - `file_intent`: query file-level SIR from store (id pattern for file rollups)
   - `neighbor_intents`: query same-file symbols' intents from store (limit deep_max_neighbors)
   - `baseline_sir`: the triage-pass SIR just generated
   - `priority_reason`: format from priority score components (e.g., "high PageRank + public method")
6. Load the deep provider:
   - If `sir_quality.deep_provider` is set, load that provider
   - Otherwise, reuse the main provider (self-improvement pattern)
   - If main provider is Qwen3Local, use CoT mode (build_enriched_sir_prompt_with_cot + deep body builder)
7. Generate SIR using the enriched prompt.
8. Store with `generation_pass = "deep"`.
9. Log progress: "Deep pass: {n}/{total} symbols, {successes} improved, {failures} failed"

If `--deep` is specified without `--full`, print error: "--deep requires --full (two-pass pipeline needs full triage first)".

## Task 8: Pipeline-level fallback for parse failures

File: `crates/aetherd/src/sir_pipeline.rs`

When the configured provider is `Tiered` and `retry_with_fallback` is true:

If `generate_sir_with_retries()` returns an error where the underlying cause is
`InferError::ParseValidationExhausted`, catch this at the pipeline level and
re-attempt the same symbol with the fallback provider.

This is separate from TieredProvider's existing routing (which routes on priority_score
before any API call). This is a parse-failure safety net for when the primary model
produces valid responses that cannot be parsed as SIR JSON.

Log: "WARN: Primary model parse failure for {qualified_name}. Falling back to {fallback_model}."

Track which provider actually succeeded in `SirMetaRecord.provider` and `SirMetaRecord.model`.

## Task 9: Regeneration CLI

File: `crates/aetherd/src/cli.rs`

Add `Regenerate` as a new subcommand (same pattern as existing `InitAgent`):

```rust
#[derive(Debug, clap::Args)]
pub struct RegenerateArgs {
    #[arg(long, default_value_t = 0.5)]
    pub below_confidence: f32,

    #[arg(long)]
    pub from_provider: Option<String>,

    #[arg(long)]
    pub file: Option<String>,

    #[arg(long)]
    pub deep: bool,

    #[arg(long)]
    pub max: Option<usize>,

    #[arg(long)]
    pub dry_run: bool,
}
```

Implementation in a new function `run_regenerate_command(workspace, args)`:

1. Load store and SIR pipeline.
2. Query store for symbols matching filters:
   - `below_confidence`: WHERE json_extract(sir_json, '$.confidence') < threshold
   - `from_provider`: WHERE provider = value
   - `file`: WHERE file_path = value (from symbols table joined to sir)
3. Cap at `args.max` if set.
4. On `--dry-run`: print a table:
   ```
   Symbol                          Provider        Confidence  Priority
   Subscribe::apply                qwen3.5:4b      0.65        0.89
   Connection::read_frame          qwen3.5:4b      0.55        0.92
   (12 symbols would be regenerated)
   ```
5. Otherwise: for each symbol, regenerate SIR.
   - If `--deep`: build enriched context, use deep provider/CoT mode
   - Otherwise: use main provider with kind-aware prompt
6. Set `generation_pass = "regenerated"` in metadata.
7. Print summary: "Regenerated {n} symbols. {successes} succeeded, {failures} failed."

## Validation

Run per-crate (NOT cargo test --workspace — OOM risk on WSL2 with 12GB RAM):
  cargo fmt --all --check
  cargo clippy --workspace -- -D warnings
  cargo test -p aether-core
  cargo test -p aether-config
  cargo test -p aether-store
  cargo test -p aether-sir
  cargo test -p aether-infer
  cargo test -p aether-parse
  cargo test -p aether-memory
  cargo test -p aether-analysis
  cargo test -p aether-lsp
  cargo test -p aether-mcp
  cargo test -p aether-query
  cargo test -p aetherd

## Expected Commit

"Phase 8.8: SIR quality pipeline — kind-aware prompts, context-enriched deep pass, local CoT mode, regeneration CLI"

Or split if the single commit is too large:
1. "Phase 8.8: extend normalize_candidate_json with bracket parser and trailing comma cleanup"
2. "Phase 8.8: kind-aware SIR prompts with few-shot examples and enriched deep pass prompts"
3. "Phase 8.8: SirQualityConfig, generation_pass column, priority scores in batch mode"
4. "Phase 8.8: two-pass deep pipeline, local CoT mode, pipeline-level fallback, regeneration CLI"

## Porcelain (after merge)

  git push -u origin feature/phase8-stage8-8
  # Create PR on GitHub, merge via GitHub UI
  git switch main
  git pull --ff-only
  git worktree remove /home/rephu/aether-worktrees/phase8-stage8-8
  git branch -d feature/phase8-stage8-8
