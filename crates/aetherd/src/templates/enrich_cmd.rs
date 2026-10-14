use super::TemplateContext;

#[derive(Debug, Clone, Copy, Default)]
pub struct EnrichCommandTemplate;

impl EnrichCommandTemplate {
    pub fn render(_context: &TemplateContext) -> String {
        r#"---
argument-hint: <crate-name> [--top N] [--lowest-confidence] [--no-trace]
description: Bulk SIR enrichment — deep analysis by the coding agent itself, no inference API key needed
---

Enrich SIR annotations for symbols in $ARGUMENTS. You do the deep analysis
yourself and write it back through MCP, so no inference API key is needed.

## Step 1: Find what's already enriched

FIRST, run this bash command to get symbol IDs that are already deep:
```bash
sqlite3 .aether/meta.sqlite "
SELECT sir.id FROM sir
JOIN symbols s ON s.id = sir.id
WHERE s.file_path LIKE '$CRATE_DIR/%'
AND sir.generation_pass = 'deep'
AND json_extract(sir.sir_json, '$.confidence') >= 0.85;
"
```
(Replace $CRATE_DIR with the crate's directory relative to the project root,
resolved as in step 1 of `/scan`; in a Cargo workspace it is usually
`crates/<crate>`.)

Save these IDs — you will SKIP them when processing candidates.

## Step 2: Select targets

Parse $ARGUMENTS for options:
- Default: top 30 symbols by audit risk score
- `--top N`: change batch size (max 100 per run)
- `--lowest-confidence`: sort by lowest SIR confidence instead of risk
- `--no-trace`: target symbols missing reasoning_trace

Call `aether_audit_candidates` with:
- `crate_filter` = "$CRATE_DIR/" (the directory from Step 1 with a trailing
  slash; a value containing `/` is matched as a path prefix)
- `top_n` from --top PLUS 20 extra (to account for skipping)
- `include_reasoning_hints` = true
- `include_deep` = true (so deep SIRs below 0.85 confidence come back too)

From the results, REMOVE any symbol whose ID appears in the
already-enriched list from Step 1. Then take the first N remaining.

If `--lowest-confidence` is specified, instead run:
```bash
sqlite3 .aether/meta.sqlite "
SELECT sir.id, s.qualified_name, s.file_path,
  json_extract(sir.sir_json, '$.confidence') as confidence
FROM sir
JOIN symbols s ON s.id = sir.id
WHERE s.file_path LIKE '$CRATE_DIR/%'
AND (sir.generation_pass != 'deep' OR json_extract(sir.sir_json, '$.confidence') < 0.85)
ORDER BY confidence ASC
LIMIT N;
"
```

If `--no-trace` is specified, focus on symbols where reasoning_trace IS NULL
AND generation_pass != 'deep'.

## Step 3: Process each symbol

For EACH symbol in the filtered target list:

### 3a. Gather context
1. Call `aether_get_sir` to read current SIR (intent, error_modes, confidence)
2. Call `aether_sir_context` for the symbol to get cross-symbol context
   (callers, callees, community, file rollup). If ambiguous, use symbol ID.
3. Call `aether_symbol_lookup` with `symbol_ids` = [the symbol ID] and
   `include_source` = true. Analyze the returned `source_text` (don't read the
   file separately) and keep its `source_hash` for step 3c.

### 3b. Deep analysis
Analyze the symbol with this quality bar:

**Intent:** Must describe WHY this code exists, not just WHAT it does.
- Bad: "Deletes a symbol from the store"
- Good: "Atomically removes a symbol and all its dependent state across 16
  tables, ensuring no orphaned data remains. Acts as the single source of
  truth for symbol cleanup, called by both individual removal and batch
  reconciliation paths."

**Error modes:** Trace EVERY failure path:
- Every `?` operator = potential error propagation
- Every `.unwrap()` = potential panic
- Every `.ok()` = silently swallowed error
- Every `if let Ok(...)` = silent failure path
- Every I/O operation = potential I/O error
- Every mutex `.lock()` = potential poisoning
- Every `as` cast = potential truncation/overflow

**Side effects:** Capture ALL observable effects:
- Database writes (which tables, what operations)
- File I/O (reads, writes, deletes)
- Network calls (HTTP, gRPC, DNS)
- Mutex acquisition (which locks, in what order)
- Channel sends (which channels)
- Process spawning
- Environment variable reads

**Dependencies:** List concrete types and functions called, not modules.

**Confidence:** Rate your own understanding:
- 0.95+: You fully understand every code path and edge case
- 0.85-0.94: You understand the main paths, some edge cases uncertain
- 0.70-0.84: Complex logic with unclear invariants
- Below 0.70: Significant uncertainty, needs deeper investigation

### 3c. Inject enriched SIR
Call `aether_sir_inject` with:
- `symbol`: the symbol ID (use ID, not name, to avoid ambiguity)
- `intent`: your deep analysis intent
- `error_modes`: complete list from analysis
- `side_effects`: complete list from analysis
- `dependencies`: the concrete types and functions from your analysis
- `confidence`: your honest assessment
- `source_hash`: the hash from step 3a, so the inject is refused if the file
  changed after you read it (skip that symbol; the daemon regenerates it)
- `generation_pass`: "deep"
- `provider`: "claude-code"
- `model`: the model you are running as
- `force`: true (overwrite even if existing confidence is higher)

### 3d. Flag bugs found during enrichment
If you discover a bug while analyzing a symbol, ALSO call
`aether_audit_submit` with the finding details before continuing
to the next symbol. Enrichment + auditing in one pass.

## Step 4: Progress tracking

After every 5 symbols, report a brief progress line:
```
[5/30] aether-store | avg confidence: 0.91 | errors found: 23 | bugs submitted: 1
```

Do NOT stop to ask questions. Process the full batch continuously.

## Step 5: Summary

After all symbols are processed:
```
Enrichment complete: aether-store (30 symbols)
  Skipped: 15 (already deep with confidence >= 0.85)
  Enriched: 30 new symbols
  Confidence: avg 0.89 (was 0.72)
  Error modes: 67 total (was 31)
  Side effects: 42 total (was 18)
  Bugs found: 3 (submitted to sir_audit)
  Symbols needing follow-up: 2 (confidence < 0.80)
  Remaining un-enriched in crate: N symbols
```

## Quality principles

1. **Depth over speed.** Read the source carefully. One high-quality SIR
   is worth more than ten shallow ones.
2. **Architectural context matters.** Use sir_context to understand how
   this symbol fits in the system, not just what it does locally.
3. **Be honest about uncertainty.** Confidence 0.75 with accurate error
   modes beats 0.95 with missing failure paths.
4. **Document what the scan pass missed.** The reasoning_hint from
   audit_candidates tells you where the scan-level model struggled.
5. **Don't stop to ask.** Process the full batch. Report at intervals.
6. **Use symbol IDs for MCP calls.** Qualified names are often ambiguous
   (e.g., unix_timestamp_secs exists in 10 crates). Always use the ID.
"#
            .to_owned()
    }
}
