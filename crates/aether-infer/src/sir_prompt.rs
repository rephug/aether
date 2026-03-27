use aether_sir::SirAnnotation;

use crate::types::SirContext;

const FEW_SHOT_EXAMPLES: &str = r#"Few-shot examples:
1) function
{"intent":"Parse a line-delimited JSON event from a byte stream and return the decoded event or EOF state","inputs":["buffer: pending unread bytes","reader: async byte source"],"outputs":["Ok(Some(Event)) when a complete event was decoded","Ok(None) when EOF is reached cleanly"],"side_effects":["Consumes bytes from reader","Mutates internal buffer"],"dependencies":["serde_json","tokio::io::AsyncReadExt"],"error_modes":["Malformed JSON payload returns error","I/O read failures propagate via ?"],"confidence":0.87}

2) struct
{"intent":"Configuration object that groups retry and timeout knobs so callers can share consistent network policy","inputs":[],"outputs":[],"side_effects":[],"dependencies":["std::time::Duration"],"error_modes":[],"confidence":0.91}

3) test
{"intent":"Verifies that reconnect backoff resets after a successful request to avoid compounding delay across healthy periods","inputs":["mock clock","flaky transport stub"],"outputs":["Pass when next delay equals initial backoff after success"],"side_effects":["Advances simulated clock","Mutates retry state in fixture"],"dependencies":["retry::BackoffPolicy","transport::MockClient"],"error_modes":["Assertion failure when delay is not reset"],"confidence":0.9}

4) trait
{"intent":"Storage abstraction providing typed persistence operations for domain records","inputs":[],"outputs":[],"side_effects":["Persists records to underlying storage backend"],"dependencies":["Record","StorageError"],"error_modes":["Storage backend unavailable","Serialization failure"],"confidence":0.91}"#;

// ---- Response contract (extracted for tier-aware system prompt builder) ----
const RESPONSE_CONTRACT: &str = "\
Respond with STRICT JSON only (no markdown, no prose) and exactly these fields: \
intent (string), inputs (array of string), outputs (array of string), \
side_effects (array of string), dependencies (array of string), \
error_modes (array of string), confidence (number in [0.0,1.0]).\n\
Do not add any extra keys.\n";

// ---- Tier 2: Standard additions (~1580 more tokens) ----

const QUALITY_CALIBRATION: &str = r#"
=== QUALITY CALIBRATION ===

Intent quality guide:
The intent field is the most important field. It should answer: "If someone deleted this code and needed to rewrite it from scratch, what non-obvious decisions would they need to replicate?"

BAD intents (too vague — describe the name, not the behavior):
- "Opens a database" (a 5-year-old could write this from the function name)
- "Returns a result" (describes the type signature, not the behavior)
- "Handles errors" (every function handles errors)
- "A helper function" (says nothing about what it helps with)
- "Processes data" (which data? how? why this way and not another?)

GOOD intents (specific, capture architectural decisions):
- "Opens a SQLite connection at the workspace .aether path with WAL mode and busy timeout, creating the schema if this is the first run"
- "Streaming iterator that lazily fetches batched rows from the database, yielding decoded records one at a time to avoid holding the full result set in memory"
- "Rate-limited HTTP client wrapper that retries with exponential backoff on 429/503, sharing a token bucket across all callers via Arc<Mutex<_>>"
- "Merges two sorted dependency edge lists by source symbol ID, deduplicating edges that appear in both the old and new parse snapshot"

For public API methods: describe the contract (what callers can rely on), not just what it does internally.
For private methods: describe WHY this logic was extracted from its caller — what would happen if it were inlined back?"#;

const CONFIDENCE_CALIBRATION: &str = r#"
Confidence calibration:
0.95-1.0: Every code path visible. No hidden behavior behind traits, closures, macros, or FFI. Simple getters, constructors, small pure functions. You are certain about every field.
0.80-0.94: Most behavior traceable. Some paths go through trait objects, closures, or external crates whose internals you cannot inspect. Typical for real-world functions with moderate complexity.
0.60-0.79: Significant behavior hidden. Macro-generated code, unsafe blocks with non-obvious invariants, deeply nested async state machines, or FFI calls. You are making educated guesses about some fields.
Below 0.60: Substantially guessing. If the code is too opaque to reach 0.60, state that in the intent field ("Intent unclear due to macro expansion / unsafe block / FFI boundary").

Do NOT default to 0.85-0.90 on everything. A struct with no logic is 0.95. An async method calling three trait objects through a closure is 0.75. Calibrate honestly."#;

const ERROR_MODE_GUIDE: &str = r#"
Error mode specificity:
Each error mode should state: WHAT fails, WHEN it fails, and HOW the failure propagates.

BAD error modes (add zero information):
- "Returns an error on failure"
- "Propagates errors via ?"
- "May panic"

GOOD error modes:
- "Returns Err(StoreError::NotFound) when the symbol ID has no matching row in the symbols table"
- "Panics via unwrap() on the mutex lock if a previous holder panicked — latent bug, not intentional"
- "Timeout after 120 seconds if the inference endpoint does not respond, propagated as reqwest::Error::Timeout"
- "Silently returns empty Vec when the graph database is locked by another process — caller sees no edges, not an error"

That last example is critical: silent failures that look like success are the most important error modes to document."#;

const SIDE_EFFECT_GUIDE: &str = r#"
Side effect classification:
IS a side effect: filesystem writes, network calls, database mutations, logging at warn/error level, mutex acquisition, global state mutation, spawning threads/tasks, signal handling, cache invalidation, environment variable reads that affect behavior.
NOT a side effect: pure computation, reading from an immutable reference, allocating memory (unless allocation IS the point), debug/trace-level logging.
EDGE CASES that ARE side effects: reading from a mutable reference that advances an iterator or cursor position. Reading from a file descriptor (advances read pointer). Acquiring a read lock (blocks writers). Calling Drop on a value that performs cleanup.

Common missed side effects in Rust:
- "Drops the old value when overwriting an Option<T> where T has a non-trivial Drop impl"
- "Advances the BufReader cursor past the consumed bytes"
- "Acquires a write lock on the RwLock, blocking all readers until the guard is dropped"
- "Registers a tracing subscriber that affects all future log output in this thread""#;

const DEPENDENCY_GUIDE: &str = r#"
Dependency naming:
Use the shortest unambiguous path the developer would recognize.

GOOD: "tokio::fs", "serde_json", "reqwest::Client", "blake3::Hasher"
BAD: "std" (too broad), "tokio" (too broad), "crate" (meaningless)

For internal dependencies, use the qualified module path: "crate::store::SqliteStore", "super::Config"
For trait implementations being derived or implemented, list the trait: "serde::Serialize", "std::fmt::Display"
For macro invocations, list the macro: "tokio::select!", "tracing::info!"
Do NOT list: language primitives (String, Vec, Option, Result), standard operators, or the prelude."#;

// ---- Tier 3: Full additions (~2100 more tokens) ----

const ADDITIONAL_EXAMPLES: &str = r#"
Additional examples by code kind:

5) async method with I/O
{"intent":"Polls the batch API endpoint in a loop until the job reaches a terminal state, sleeping between attempts at the configured interval, and returns the download path on success","inputs":["batch_id: opaque job identifier from creation","poll_interval: seconds between status checks","output_dir: where to write downloaded results"],"outputs":["Ok(PathBuf) pointing to the downloaded JSONL results","Err on FAILED/CANCELLED/EXPIRED terminal state"],"side_effects":["Sleeps the current task between poll attempts","Writes downloaded file to output_dir","Logs progress at info level each poll cycle"],"dependencies":["reqwest::Client","tokio::time::sleep","std::fs::write"],"error_modes":["Network timeout during poll — logged and retried next interval","Server returned FAILED state — returns error with server message","Filesystem write permission denied — propagated as io::Error","Poll loop exceeded 24-hour safety limit — returns timeout error"],"confidence":0.88}

6) enum (discriminated union)
{"intent":"Represents the three quality tiers of SIR generation, each selecting a different prompt template and model, used by the batch pipeline to route symbols to the appropriate inference path","inputs":[],"outputs":[],"side_effects":[],"dependencies":["serde::Serialize","serde::Deserialize","clap::ValueEnum"],"error_modes":[],"confidence":0.95}

7) impl method with transaction semantics
{"intent":"Validates schema conformance of a SIR annotation, computes its BLAKE3 content hash for deduplication, then persists both the sir and sir_meta rows in a single SQLite transaction so partial writes cannot leave orphaned metadata","inputs":["symbol_id: content-addressed symbol identifier","sir: the annotation to validate and store","provider_name: which inference backend produced this","model_name: specific model version string"],"outputs":["Ok((canonical_json, sir_hash)) with the normalized JSON and its hash","Err(ValidationError) if required fields are missing before any DB write"],"side_effects":["BEGIN EXCLUSIVE transaction on SQLite","INSERT OR REPLACE into sir table","INSERT OR REPLACE into sir_meta table","COMMIT (or ROLLBACK on error)"],"dependencies":["aether_sir::validate_sir","serde_json::to_string","blake3","rusqlite::Transaction"],"error_modes":["Missing required 'intent' field — validation error before any DB write","Database locked by concurrent process — rusqlite returns SQLITE_BUSY after timeout","sir_meta update fails after sir insert — transaction rolls back both writes","Canonical JSON serialization fails on non-UTF8 content — serde error"],"confidence":0.85}

8) macro-generated or derive code
{"intent":"Derives Serialize/Deserialize for the config struct, enabling round-trip TOML persistence with serde default attributes filling in missing fields from the file","inputs":[],"outputs":[],"side_effects":[],"dependencies":["serde::Serialize","serde::Deserialize","toml"],"error_modes":["Deserialization silently uses default values for missing keys — may mask misconfiguration","Unknown keys in TOML are silently ignored unless deny_unknown_fields is active"],"confidence":0.72}

9) trait implementation method
{"intent":"Implements Display for the health score by formatting the numeric value as a percentage with one decimal place and appending the status label (Healthy/Watch/Critical) based on threshold boundaries","inputs":["self: the HealthScore value to format","f: the output formatter"],"outputs":["Ok(()) after writing formatted string to the formatter","Err(fmt::Error) if the formatter's write buffer is full"],"side_effects":["Writes formatted bytes to the formatter's internal buffer"],"dependencies":["std::fmt::Display","std::fmt::Formatter"],"error_modes":["fmt::Error if write! macro fails — propagated to caller"],"confidence":0.94}

10) closure or anonymous function
{"intent":"Predicate closure passed to Iterator::filter that retains only symbols whose file path matches the target file, used during batch build to scope a rebuild to a single changed file","inputs":["symbol: reference to the Symbol being tested"],"outputs":["true if symbol.file_path matches the target path","false otherwise"],"side_effects":[],"dependencies":["std::path::Path::eq"],"error_modes":[],"confidence":0.96}

11) const or static definition
{"intent":"Compile-time constant defining the maximum number of concurrent triage inference requests, balancing API rate limits against batch throughput on the netcup server hardware","inputs":[],"outputs":[],"side_effects":[],"dependencies":[],"error_modes":[],"confidence":0.97}

12) module-level documentation / re-export block
{"intent":"Public re-export barrel that surfaces the SqliteStore, GraphStore traits, and error types as the crate's external API while keeping implementation modules private","inputs":[],"outputs":[],"side_effects":[],"dependencies":["crate::sqlite::SqliteStore","crate::graph::GraphStore","crate::error::StoreError"],"error_modes":[],"confidence":0.98}"#;

const LANGUAGE_PATTERNS: &str = r#"
=== LANGUAGE-SPECIFIC PATTERNS ===

Rust-specific guidance:
- For functions returning Result<T, E>: list each distinct Err variant in error_modes, not just "returns Err"
- For functions using ? operator: trace the error propagation chain — what concrete error types can reach the caller?
- For unsafe blocks: lower your confidence by 0.10-0.15. Describe what invariant the unsafe code relies on in the intent.
- For #[derive(...)] on structs/enums: the derives ARE dependencies. Serialize, Deserialize, Clone, Debug — list them.
- For trait objects (Box<dyn Trait>, &dyn Trait): note the dynamic dispatch as a dependency on the trait, and acknowledge in the intent that behavior depends on the runtime implementor.
- For lifetime annotations: if lifetimes constrain the API (e.g., 'a ties a reference to a struct), mention this in the intent as it affects how callers can use the return value.
- For Pin<Box<dyn Future>>: this is an async trait workaround — note it as a side effect of Rust's async trait limitations, not as intentional API design.

TypeScript/JavaScript-specific guidance:
- For async/await functions: every await point is a potential failure point. List network, timeout, and rejection errors separately.
- For React components: inputs are props, outputs are the rendered element description, side effects include useEffect hooks, state mutations, and context reads.
- For Express/Koa middleware: inputs include req, res, next. Side effects include response mutation, header setting, and calling next().
- For Promise chains: trace .catch() handlers as error_modes. Unhandled rejections should be explicitly noted.

Python-specific guidance:
- For decorators: describe what the decorator adds/modifies in the wrapped function's behavior.
- For context managers (__enter__/__exit__): side effects include resource acquisition on enter and cleanup on exit.
- For dataclasses/Pydantic models: similar to Rust structs — describe WHY the type exists, list field validators as dependencies.
- For generator functions (yield): outputs should describe what is yielded and when iteration terminates."#;

const ANTI_PATTERNS: &str = r#"
=== ANTI-PATTERNS TO AVOID ===

These patterns produce low-value SIRs. If you find yourself writing any of these, stop and reconsider:

1. Copy-pasting the function signature as the intent:
   BAD: "pub fn open(workspace: &Path) -> Result<Self>" → intent: "Opens workspace path and returns Self"
   This adds zero information beyond what the signature already says.

2. Generic outputs that match the return type:
   BAD: outputs: ["Result<Vec<String>>"]
   GOOD: outputs: ["Ok(Vec<String>) containing de-duplicated symbol IDs sorted alphabetically", "Err(StoreError::Locked) when another process holds the database"]

3. Listing every import as a dependency:
   BAD: dependencies: ["std::path::Path", "std::collections::HashMap", "std::io", "anyhow::Result", "anyhow::Context"]
   GOOD: dependencies: ["anyhow", "rusqlite::Connection"] (only the substantive ones)

4. Empty arrays when there ARE items to list:
   If a function reads from disk, writes to a database, or calls an API — it has side effects.
   If a function can fail — it has error modes.
   Empty arrays should only appear when there genuinely is nothing (e.g., a pure computation helper with no I/O).

5. Confidence of exactly 0.85 on everything:
   This is the "I didn't think about it" default. Vary your confidence based on how much of the code you can actually trace."#;

const OUTPUT_FORMAT_RULES: &str = r#"
=== OUTPUT FORMAT RULES ===

Array ordering:
- inputs: order by importance (most essential parameter first), not by position in the signature
- outputs: order by frequency (most common return path first, then edge cases)
- side_effects: order by severity (most impactful first — database writes before log messages)
- dependencies: order by coupling strength (deeply used dependencies first, incidental ones last)
- error_modes: order by likelihood (most common failures first)

String formatting:
- Each array element should be a complete, readable English phrase
- Do NOT use code syntax in array elements: "Vec<String>" is bad, "vector of string identifiers" is good
- DO use code names for specific types/functions when they add clarity: "reqwest::Error::Timeout" is good because the developer needs to handle that exact type
- Keep individual array elements under 150 characters. If you need more detail, split into multiple elements.

Empty vs singleton arrays:
- A function with no inputs (e.g., a getter) should have inputs: [] — do not invent phantom inputs
- A function with one clear output should have outputs: ["description"] — do not pad with trivial variants"#;

const RELATIONSHIP_CONTEXT: &str = r#"
=== UNDERSTANDING SYMBOL RELATIONSHIPS ===

When analyzing a symbol, consider its role in the larger system:

Callers: What code calls this symbol? How does that affect what this symbol MUST guarantee?
- A function called from an HTTP handler has different reliability requirements than one called from a test
- A method called in a hot loop should note performance-relevant side effects (allocations, locks)

Callees: What does this symbol depend on? How do their failure modes compose?
- If you call three fallible functions via ?, your error_modes section should cover failures from each
- If you call an async function, its timeout behavior becomes your timeout behavior

Siblings: Other symbols in the same file or module share a conceptual boundary.
- A group of methods on the same struct usually implement a coherent interface — your intent should explain this symbol's role in that interface, not just what it does in isolation
- Helper functions should explain what they factor out of their caller and why

Type relationships:
- A struct that implements a trait: the intent should mention the trait and what implementing it enables
- A function that returns a custom error type: the error type's variants should appear in error_modes
- A generic function with trait bounds: the bounds ARE dependencies — they constrain what types can use this function

Ownership and lifetime considerations (Rust):
- Functions that take &self vs &mut self vs self: the ownership model is part of the intent
- Functions that return references: the lifetime relationship to the input is part of the output description
- Functions that clone or Arc-wrap values: the cloning IS a side effect (it's a deliberate architectural choice about sharing vs copying)"#;

const FINAL_GUIDANCE: &str = r#"
=== FINAL GUIDANCE ===

The purpose of a SIR annotation is to let a developer (or an AI assistant) understand what a symbol does WITHOUT reading its source code. The SIR should contain everything the source code teaches you, compressed into structured fields.

Ask yourself: if the source code were deleted and only this SIR remained, could someone rewrite functionally equivalent code? If the answer is "they'd have to guess about error handling" — your error_modes are incomplete. If the answer is "they wouldn't know it writes to the database" — your side_effects are incomplete.

When in doubt, be more specific rather than less. A SIR that's too detailed is still useful. A SIR that's too vague is worthless."#;

// ---- Enrichment-specific (for triage/deep) ----

const ENRICHMENT_GUIDANCE: &str = r#"
=== ENRICHMENT-SPECIFIC GUIDANCE ===

How to improve a baseline SIR:

Intent improvements:
- If the baseline says "handles X" → replace with HOW it handles X and WHAT decisions are embedded
- If the baseline is generic ("processes data") → name the specific data structures, algorithms, or protocols involved
- If the baseline omits async/concurrency behavior → add it: "spawns a background task", "holds a lock for the duration"

Error mode improvements:
- Trace every ? operator in the source to its concrete error type
- Look for unwrap(), expect(), panic!() — these are error modes even if unintentional (document as "latent panic" or "intentional panic with message")
- Check for silent error swallowing: .ok(), .unwrap_or_default(), match with _ => {} arms that discard errors
- If the baseline says "propagates errors" → replace with the specific error type and the condition that triggers it

Side effect improvements:
- Read the function body for I/O operations the baseline missed
- Check Drop implementations of values created in the function
- Look for global state: lazy_static!, OnceCell, thread_local!, static mut
- Check if the function modifies its &mut self receiver in ways the baseline didn't capture

Confidence adjustments:
- LOWER confidence if you find: unsafe blocks, FFI calls, macro-generated code paths, trait objects with unknown implementors
- RAISE confidence if the code is simpler than the baseline assumed (e.g., baseline was conservative about a straightforward getter)
- Never raise confidence above 0.95 unless you can literally see every code path with no abstractions"#;

fn is_type_definition(kind: &str) -> bool {
    matches!(kind, "struct" | "enum" | "trait" | "type_alias")
}

fn is_function_like(kind: &str) -> bool {
    matches!(kind, "function" | "method")
}

fn is_test_like(context: &SirContext) -> bool {
    context
        .qualified_name
        .to_ascii_lowercase()
        .contains("test_")
        || context.kind.to_ascii_lowercase().contains("test")
}

fn kind_specific_guidance(context: &SirContext) -> String {
    let mut sections = Vec::new();

    if is_type_definition(context.kind.as_str()) {
        sections.push(
            "For type definitions: describe WHY this type exists, not just WHAT it is.\n\
List contained or extended types as dependencies.\n\
Inputs and outputs should be empty arrays for type definitions.\n\
Good intent example: 'Database handle struct that holds a reference-counted pointer to shared database state, enabling multiple owners including a background task'\n\
Bad intent example: 'Define a database structure'"
                .to_owned(),
        );
    }

    if is_function_like(context.kind.as_str()) {
        if context.is_public && context.line_count > 30 {
            sections.push(
                "For complex public methods: enumerate each distinct return path in outputs\n\
(Ok/Err/None) with WHEN each occurs. Describe what each input parameter represents\n\
and its purpose. For error_modes, describe specific failure conditions with\n\
propagation details - follow the ? operator chain.\n\
Good outputs example: ['Ok(Some(Frame)) when a complete frame has been parsed', 'Ok(None) when the remote peer cleanly closed the connection', 'Err when the connection was reset mid-frame']\n\
Bad outputs example: ['crate::Result<Option<Frame>>']"
                    .to_owned(),
            );
        } else {
            sections.push(
                "Even for simple functions, provide a descriptive intent - not just a single word\n\
like 'getter' or 'constructor'. Describe what the function accomplishes and why it exists."
                    .to_owned(),
            );
        }

        if is_test_like(context) {
            sections.push(
                "For test functions: describe what behavior is being verified and under what conditions.\n\
For dependencies, list the production code under test."
                    .to_owned(),
            );
        }
    }

    if sections.is_empty() {
        String::new()
    } else {
        format!("\n\nKind-specific guidance:\n{}", sections.join("\n\n"))
    }
}

fn strict_response_contract(_context: &SirContext) -> &'static str {
    "Respond with STRICT JSON only (no markdown, no prose) and exactly these fields: \
intent (string), inputs (array of string), outputs (array of string), side_effects (array of string), dependencies (array of string), error_modes (array of string), confidence (number in [0.0,1.0]).\n\
Do not add any extra keys.\n\n\
"
}

fn enriched_response_contract(_context: &SirContext) -> &'static str {
    "\n\nRespond with STRICT JSON only. Exactly these fields: intent, inputs, outputs,\n\
side_effects, dependencies, error_modes, confidence."
}

pub fn build_sir_prompt_for_kind(symbol_text: &str, context: &SirContext) -> String {
    format!(
        "You are generating a Leaf SIR (Semantic Intent Record) annotation.\n\
{}\
Context:\n\
- language: {}\n\
- file_path: {}\n\
- qualified_name: {}\n\n\
{}{}\n\n\
Symbol text:\n{}",
        strict_response_contract(context),
        context.language,
        context.file_path,
        context.qualified_name,
        FEW_SHOT_EXAMPLES,
        kind_specific_guidance(context),
        symbol_text,
    )
}

#[derive(Debug, Clone)]
pub struct SirEnrichmentContext {
    /// File-level rollup intent available to enriched quality passes
    pub file_intent: Option<String>,
    /// Intents of neighboring symbols in the same file
    pub neighbor_intents: Vec<(String, String)>,
    /// The baseline SIR to improve upon
    pub baseline_sir: Option<SirAnnotation>,
    /// Human-readable explanation of why this symbol was selected for enriched analysis
    pub priority_reason: String,
    /// Contract clauses from caller symbols that depend on this symbol.
    /// Each entry: (caller qualified name, clause type, clause text)
    pub caller_contract_clauses: Vec<(String, String, String)>,
}

fn format_baseline_sir(baseline_sir: &Option<SirAnnotation>) -> String {
    match baseline_sir {
        Some(sir) => serde_json::to_string(sir).unwrap_or_else(|_| "(not available)".to_owned()),
        None => "(not available)".to_owned(),
    }
}

fn format_neighbor_intents(neighbor_intents: &[(String, String)]) -> String {
    if neighbor_intents.is_empty() {
        return "(none)".to_owned();
    }

    neighbor_intents
        .iter()
        .map(|(name, intent)| format!("- {name}: {intent}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_enriched_prompt_core(
    symbol_text: &str,
    context: &SirContext,
    enrichment: &SirEnrichmentContext,
    include_cot: bool,
) -> String {
    let mut prompt = format!(
        "You are improving an existing SIR (Semantic Intent Record) annotation with deeper analysis.\n\n\
Context:\n\
- language: {}\n\
- file_path: {}\n\
- qualified_name: {}\n\
- priority: {}\n\n\
File purpose: {}\n\n\
Other symbols in this file:\n{}\n\n\
Previous SIR (improve upon this):\n{}{}\n\n\
Symbol text:\n{}\n\n\
Focus on: more specific intent, complete error propagation paths, all side effects\n\
including conditional mutations, correct confidence reflecting your certainty.",
        context.language,
        context.file_path,
        context.qualified_name,
        enrichment.priority_reason,
        enrichment
            .file_intent
            .as_deref()
            .unwrap_or("(not available)"),
        format_neighbor_intents(&enrichment.neighbor_intents),
        format_baseline_sir(&enrichment.baseline_sir),
        kind_specific_guidance(context),
        symbol_text,
    );

    if !enrichment.caller_contract_clauses.is_empty() {
        prompt.push_str(
            "\n\nCaller contracts (behavioral expectations from callers of this symbol):\n",
        );
        for (caller_name, clause_type, clause_text) in &enrichment.caller_contract_clauses {
            prompt.push_str(&format!(
                "- {caller_name} [{clause_type}]: \"{clause_text}\"\n"
            ));
        }
        prompt.push_str(
            "Ensure your SIR summary clarifies whether these contracted behaviors are preserved.\n",
        );
    }

    if include_cot {
        prompt.push_str(
            "\n\nBefore outputting JSON, you MUST wrap your analysis inside <thinking> tags:\n\
1. What crucial runtime behaviors are missing from the previous SIR?\n\
2. How do the neighboring symbols dictate how this symbol should be used?\n\
3. What fields will you expand?\n\
\nAfter closing your </thinking> tag, output the final JSON.",
        );
    }

    prompt.push_str(enriched_response_contract(context));
    prompt
}

pub fn build_enriched_sir_prompt(
    symbol_text: &str,
    context: &SirContext,
    enrichment: &SirEnrichmentContext,
) -> String {
    build_enriched_prompt_core(symbol_text, context, enrichment, false)
}

pub fn build_enriched_sir_prompt_with_cot(
    symbol_text: &str,
    context: &SirContext,
    enrichment: &SirEnrichmentContext,
) -> String {
    build_enriched_prompt_core(symbol_text, context, enrichment, true)
}

// ---- Prompt tier system ----

/// SIR prompt tier controlling instruction depth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptTier {
    /// ~420 tokens. For local models with 4K context windows.
    Compact,
    /// ~2000 tokens. For local models with 8K-16K context, or cost-conscious cloud.
    Standard,
    /// ~4100 tokens. For cloud providers. Clears all prompt caching thresholds.
    Full,
}

/// Append shared standard-tier sections to a prompt being built.
fn append_standard_sections(prompt: &mut String) {
    prompt.push_str(QUALITY_CALIBRATION);
    prompt.push_str(CONFIDENCE_CALIBRATION);
    prompt.push_str(ERROR_MODE_GUIDE);
    prompt.push_str(SIDE_EFFECT_GUIDE);
    prompt.push_str(DEPENDENCY_GUIDE);
}

/// Append shared full-tier sections to a prompt being built.
fn append_full_sections(prompt: &mut String) {
    prompt.push_str(ADDITIONAL_EXAMPLES);
    prompt.push_str(LANGUAGE_PATTERNS);
    prompt.push_str(ANTI_PATTERNS);
    prompt.push_str(OUTPUT_FORMAT_RULES);
    prompt.push_str(RELATIONSHIP_CONTEXT);
    prompt.push_str(FINAL_GUIDANCE);
}

/// Static system instruction for scan pass at the given tier.
/// This is the cacheable prefix shared across all symbols in a batch.
pub fn sir_scan_system_prompt(tier: PromptTier) -> String {
    let mut prompt =
        String::from("You are generating a Leaf SIR (Semantic Intent Record) annotation.\n");
    prompt.push_str(RESPONSE_CONTRACT);
    prompt.push('\n');
    prompt.push_str(FEW_SHOT_EXAMPLES);

    if tier == PromptTier::Compact {
        return prompt;
    }

    prompt.push_str("\n\n");
    append_standard_sections(&mut prompt);

    if tier == PromptTier::Standard {
        return prompt;
    }

    append_full_sections(&mut prompt);
    prompt
}

/// Per-symbol user prompt for scan pass (dynamic, not cached).
pub fn sir_scan_user_prompt(symbol_text: &str, context: &SirContext) -> String {
    format!(
        "Context:\n\
- language: {}\n\
- file_path: {}\n\
- qualified_name: {}\n\
{}\n\n\
Symbol text:\n{}",
        context.language,
        context.file_path,
        context.qualified_name,
        kind_specific_guidance(context),
        symbol_text,
    )
}

/// Static system instruction for enriched (triage/deep) pass at the given tier.
pub fn sir_enriched_system_prompt(tier: PromptTier) -> String {
    let mut prompt = String::from(
        "You are improving an existing SIR (Semantic Intent Record) annotation with deeper analysis.\n\n\
Your task: given a baseline SIR (from a previous pass), neighboring symbol intents, \
file-level context, and the full source code, produce a strictly better SIR. \"Better\" means:\n\
- More specific intent (capture architectural decisions, not just behavior)\n\
- More complete error_modes (trace every ? operator, every unwrap, every silent failure)\n\
- More complete side_effects (catch mutations, I/O, locks, drops)\n\
- More accurate confidence (lower it if you discovered hidden complexity)\n\n\
Do NOT simply rephrase the baseline. If the baseline is already good, improve the WEAKEST field. \
If you cannot improve it, reproduce it with the same or higher confidence.\n\n\
Respond with STRICT JSON only. Exactly these fields: intent, inputs, outputs, \
side_effects, dependencies, error_modes, confidence.",
    );

    if tier == PromptTier::Compact {
        return prompt;
    }

    prompt.push_str("\n\n");
    prompt.push_str(ENRICHMENT_GUIDANCE);
    append_standard_sections(&mut prompt);

    if tier == PromptTier::Standard {
        return prompt;
    }

    append_full_sections(&mut prompt);
    prompt
}

/// Per-symbol user prompt for enriched (triage/deep) pass.
/// Extracts the dynamic portion from build_enriched_prompt_core().
pub fn sir_enriched_user_prompt(
    symbol_text: &str,
    context: &SirContext,
    enrichment: &SirEnrichmentContext,
    include_cot: bool,
) -> String {
    let mut prompt = format!(
        "Context:\n\
- language: {}\n\
- file_path: {}\n\
- qualified_name: {}\n\
- priority: {}\n\n\
File purpose: {}\n\n\
Other symbols in this file:\n{}\n\n\
Previous SIR (improve upon this):\n{}{}\n\n\
Symbol text:\n{}\n\n\
Focus on: more specific intent, complete error propagation paths, all side effects\n\
including conditional mutations, correct confidence reflecting your certainty.",
        context.language,
        context.file_path,
        context.qualified_name,
        enrichment.priority_reason,
        enrichment
            .file_intent
            .as_deref()
            .unwrap_or("(not available)"),
        format_neighbor_intents(&enrichment.neighbor_intents),
        format_baseline_sir(&enrichment.baseline_sir),
        kind_specific_guidance(context),
        symbol_text,
    );

    if !enrichment.caller_contract_clauses.is_empty() {
        prompt.push_str(
            "\n\nCaller contracts (behavioral expectations from callers of this symbol):\n",
        );
        for (caller_name, clause_type, clause_text) in &enrichment.caller_contract_clauses {
            prompt.push_str(&format!(
                "- {caller_name} [{clause_type}]: \"{clause_text}\"\n"
            ));
        }
        prompt.push_str(
            "Ensure your SIR summary clarifies whether these contracted behaviors are preserved.\n",
        );
    }

    if include_cot {
        prompt.push_str(
            "\n\nBefore outputting JSON, you MUST wrap your analysis inside <thinking> tags:\n\
1. What crucial runtime behaviors are missing from the previous SIR?\n\
2. How do the neighboring symbols dictate how this symbol should be used?\n\
3. What fields will you expand?\n\
\nAfter closing your </thinking> tag, output the final JSON.",
        );
    }

    prompt
}

/// Resolve a prompt tier string from config to a [`PromptTier`] value.
/// "auto" selects based on provider: cloud providers get Full, local gets Compact.
pub fn resolve_prompt_tier(tier_str: &str, provider: &str) -> PromptTier {
    match tier_str.trim().to_ascii_lowercase().as_str() {
        "compact" => PromptTier::Compact,
        "standard" => PromptTier::Standard,
        "full" => PromptTier::Full,
        _ => {
            // "auto" or unrecognized: cloud → full, local → compact
            if provider == "qwen3_local" || provider == "ollama" {
                PromptTier::Compact
            } else {
                PromptTier::Full
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use aether_sir::SirAnnotation;

    use super::*;

    #[test]
    fn build_sir_prompt_for_kind_includes_type_guidance_for_structs() {
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::State".to_owned(),
            priority_score: None,
            kind: "struct".to_owned(),
            is_public: true,
            line_count: 12,
        };
        let prompt = build_sir_prompt_for_kind("pub struct State {}", &context);
        assert!(prompt.contains("For type definitions: describe WHY this type exists"));
    }

    #[test]
    fn build_sir_prompt_for_kind_keeps_function_schema_strict() {
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
            priority_score: None,
            kind: "function".to_owned(),
            is_public: true,
            line_count: 8,
        };

        let prompt = build_sir_prompt_for_kind("pub fn run() {}", &context);
        assert!(prompt.contains("exactly these fields"));
    }

    #[test]
    fn build_sir_prompt_for_kind_includes_complex_public_guidance_for_large_functions() {
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
            priority_score: Some(0.92),
            kind: "function".to_owned(),
            is_public: true,
            line_count: 55,
        };
        let prompt = build_sir_prompt_for_kind("pub fn run() -> Result<()> {}", &context);
        assert!(prompt.contains("For complex public methods"));
        assert!(prompt.contains("Bad outputs example"));
    }

    #[test]
    fn build_sir_prompt_for_kind_includes_test_guidance_for_tests() {
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::test_retry_reset".to_owned(),
            priority_score: None,
            kind: "function".to_owned(),
            is_public: false,
            line_count: 14,
        };
        let prompt = build_sir_prompt_for_kind("fn test_retry_reset() {}", &context);
        assert!(prompt.contains("For test functions: describe what behavior is being verified"));
    }

    #[test]
    fn build_enriched_prompt_includes_context_sections() {
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
            priority_score: Some(0.9),
            kind: "function".to_owned(),
            is_public: true,
            line_count: 60,
        };
        let enrichment = SirEnrichmentContext {
            file_intent: Some("Coordinates request lifecycle".to_owned()),
            neighbor_intents: vec![
                (
                    "demo::parse".to_owned(),
                    "Parses raw bytes into frames".to_owned(),
                ),
                (
                    "demo::flush".to_owned(),
                    "Flushes pending writes".to_owned(),
                ),
            ],
            baseline_sir: Some(SirAnnotation {
                intent: "Baseline".to_owned(),
                behavior: None,
                inputs: Vec::new(),
                outputs: Vec::new(),
                side_effects: Vec::new(),
                dependencies: Vec::new(),
                error_modes: Vec::new(),
                confidence: 0.6,
                edge_cases: None,
                complexity: None,
                method_dependencies: None,
            }),
            priority_reason: "high PageRank + public method".to_owned(),
            caller_contract_clauses: Vec::new(),
        };

        let prompt = build_enriched_sir_prompt("pub fn run() {}", &context, &enrichment);
        assert!(
            prompt
                .contains("You are improving an existing SIR (Semantic Intent Record) annotation")
        );
        assert!(prompt.contains("high PageRank + public method"));
        assert!(prompt.contains("Other symbols in this file"));
        assert!(prompt.contains("Previous SIR (improve upon this):"));
    }

    #[test]
    fn build_enriched_prompt_keeps_type_definition_guidance() {
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::Store".to_owned(),
            priority_score: Some(0.9),
            kind: "trait".to_owned(),
            is_public: true,
            line_count: 60,
        };
        let enrichment = SirEnrichmentContext {
            file_intent: None,
            neighbor_intents: Vec::new(),
            baseline_sir: None,
            priority_reason: "low confidence triage output".to_owned(),
            caller_contract_clauses: Vec::new(),
        };

        let prompt = build_enriched_sir_prompt("pub trait Store {}", &context, &enrichment);
        assert!(prompt.contains("For type definitions: describe WHY this type exists"));
        assert!(!prompt.contains("you MUST include method_dependencies"));
    }

    #[test]
    fn build_enriched_prompt_with_cot_contains_thinking_instructions() {
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
            priority_score: Some(0.9),
            kind: "function".to_owned(),
            is_public: true,
            line_count: 60,
        };
        let enrichment = SirEnrichmentContext {
            file_intent: None,
            neighbor_intents: Vec::new(),
            baseline_sir: None,
            priority_reason: "low confidence triage output".to_owned(),
            caller_contract_clauses: Vec::new(),
        };

        let prompt = build_enriched_sir_prompt_with_cot("pub fn run() {}", &context, &enrichment);
        assert!(prompt.contains("<thinking>"));
        assert!(prompt.contains("After closing your </thinking> tag, output the final JSON."));
    }

    #[test]
    fn build_enriched_prompt_includes_caller_contract_clauses() {
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
            priority_score: Some(0.9),
            kind: "function".to_owned(),
            is_public: true,
            line_count: 60,
        };
        let enrichment = SirEnrichmentContext {
            file_intent: None,
            neighbor_intents: Vec::new(),
            baseline_sir: None,
            priority_reason: "test".to_owned(),
            caller_contract_clauses: vec![
                (
                    "payments::checkout".to_owned(),
                    "must".to_owned(),
                    "reject negative amounts".to_owned(),
                ),
                (
                    "payments::refund".to_owned(),
                    "must_not".to_owned(),
                    "allow duplicate refunds".to_owned(),
                ),
            ],
        };

        let prompt = build_enriched_sir_prompt("pub fn run() {}", &context, &enrichment);
        assert!(prompt.contains("Caller contracts"));
        assert!(prompt.contains("payments::checkout [must]: \"reject negative amounts\""));
        assert!(prompt.contains("payments::refund [must_not]: \"allow duplicate refunds\""));
        assert!(prompt.contains("contracted behaviors are preserved"));
    }

    #[test]
    fn build_enriched_prompt_omits_contracts_when_empty() {
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
            priority_score: None,
            kind: "function".to_owned(),
            is_public: true,
            line_count: 10,
        };
        let enrichment = SirEnrichmentContext {
            file_intent: None,
            neighbor_intents: Vec::new(),
            baseline_sir: None,
            priority_reason: "test".to_owned(),
            caller_contract_clauses: Vec::new(),
        };

        let prompt = build_enriched_sir_prompt("pub fn run() {}", &context, &enrichment);
        assert!(!prompt.contains("Caller contracts"));
    }

    // ---- Tier system tests ----

    #[test]
    fn scan_prompt_tier_compact_matches_original() {
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
            priority_score: None,
            kind: "function".to_owned(),
            is_public: true,
            line_count: 8,
        };
        let original = build_sir_prompt_for_kind("pub fn run() {}", &context);
        let system = sir_scan_system_prompt(PromptTier::Compact);
        let user = sir_scan_user_prompt("pub fn run() {}", &context);
        // Both contain the essential elements
        assert!(
            original.contains("exactly these fields") && system.contains("exactly these fields")
        );
        assert!(original.contains("pub fn run() {}") && user.contains("pub fn run() {}"));
        assert!(original.contains("Few-shot examples") && system.contains("Few-shot examples"));
        assert!(user.contains("language: rust"));
        assert!(user.contains("demo::run"));
    }

    #[test]
    fn tier_sizes_increase_monotonically() {
        let compact = sir_scan_system_prompt(PromptTier::Compact);
        let standard = sir_scan_system_prompt(PromptTier::Standard);
        let full = sir_scan_system_prompt(PromptTier::Full);
        assert!(compact.len() < standard.len());
        assert!(standard.len() < full.len());
        assert!(full.contains("QUALITY CALIBRATION"));
        assert!(full.contains("LANGUAGE-SPECIFIC PATTERNS"));
        assert!(!compact.contains("QUALITY CALIBRATION"));
    }

    #[test]
    fn enriched_system_prompt_tiers_contain_expected_sections() {
        let compact = sir_enriched_system_prompt(PromptTier::Compact);
        let full = sir_enriched_system_prompt(PromptTier::Full);
        assert!(compact.contains("You are improving an existing SIR"));
        assert!(!compact.contains("ENRICHMENT-SPECIFIC GUIDANCE"));
        assert!(full.contains("ENRICHMENT-SPECIFIC GUIDANCE"));
        assert!(full.contains("QUALITY CALIBRATION"));
    }

    #[test]
    fn enriched_user_prompt_contains_dynamic_content() {
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
            priority_score: Some(0.9),
            kind: "function".to_owned(),
            is_public: true,
            line_count: 60,
        };
        let enrichment = SirEnrichmentContext {
            file_intent: Some("Coordinates request lifecycle".to_owned()),
            neighbor_intents: vec![("demo::parse".to_owned(), "Parses raw bytes".to_owned())],
            baseline_sir: None,
            priority_reason: "high PageRank".to_owned(),
            caller_contract_clauses: Vec::new(),
        };

        let user = sir_enriched_user_prompt("pub fn run() {}", &context, &enrichment, false);
        assert!(user.contains("high PageRank"));
        assert!(user.contains("demo::parse: Parses raw bytes"));
        assert!(user.contains("pub fn run() {}"));
        assert!(!user.contains("You are improving")); // System instruction should NOT be in user prompt
        assert!(!user.contains("<thinking>")); // No CoT when include_cot=false

        let user_cot = sir_enriched_user_prompt("pub fn run() {}", &context, &enrichment, true);
        assert!(user_cot.contains("<thinking>"));
    }

    #[test]
    fn resolve_prompt_tier_auto_selects_by_provider() {
        assert_eq!(resolve_prompt_tier("auto", "gemini"), PromptTier::Full);
        assert_eq!(
            resolve_prompt_tier("auto", "qwen3_local"),
            PromptTier::Compact
        );
        assert_eq!(resolve_prompt_tier("auto", "ollama"), PromptTier::Compact);
        assert_eq!(
            resolve_prompt_tier("compact", "gemini"),
            PromptTier::Compact
        );
        assert_eq!(resolve_prompt_tier("full", "ollama"), PromptTier::Full);
        assert_eq!(
            resolve_prompt_tier("standard", "anything"),
            PromptTier::Standard
        );
    }
}
