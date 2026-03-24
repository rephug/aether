use super::TemplateContext;

#[derive(Debug, Clone, Copy, Default)]
pub struct AuditCommandTemplate;

impl AuditCommandTemplate {
    pub fn render(_context: &TemplateContext) -> String {
        r#"---
argument-hint: <crate-or-file>
description: Deep audit for bugs using AETHER structural intelligence
---

Audit $ARGUMENTS for bugs using AETHER's MCP tools to guide analysis.

## Step 1: Find audit targets
1. Call `aether_health` scoped to $ARGUMENTS to get structural risk scores.
2. Symbols with high risk_score, high betweenness centrality, or low test coverage
   are priority targets.
3. If `aether_audit_candidates` is available, call it with the scope for a
   pre-ranked target list with reasoning hints.
4. If neither tool returns results, fall back to `aether_symbol_lookup` for
   the target file/crate and prioritize large, complex symbols.

## Step 2: Deep analysis for each target symbol
For each high-risk symbol:
1. Call `aether_get_sir` to read the current SIR annotation (intent, error_modes,
   confidence, reasoning_trace).
2. Read the actual source file to inspect full implementation.
3. If reasoning_trace mentions uncertainty, investigate that specific concern.
4. Check for these bug categories:
   - ARITHMETIC: overflow, underflow, NaN propagation, saturating ops that
     invert ordering, negative values in unsigned contexts, exact float comparison
   - ENCODING: UTF-8 boundary slicing, byte vs char indexing, lossy conversions
   - SILENT_FAILURE: unwrap_or_default hiding errors, .ok() discarding info,
     catch-all match arms, Ok(empty) masking errors from callers
   - STATE: stale values carried across iterations, mutable refs outliving scope
   - TYPE_SAFETY: accepting any value when only specific ones are valid,
     no validation on deserialized data
   - CONCURRENCY: mutex poisoning, lock ordering, send across thread boundaries
   - RESOURCE_LEAK: unclosed file handles, connections not returned to pool
   - LOGIC_ERROR: off-by-one, wrong assertion values, inverted conditions

## Step 3: Cross-symbol analysis
When you find a suspicious symbol, check its callers and callees:
1. Call `aether_dependencies` on the symbol to get callers and callees.
2. Call `aether_get_sir` on callers - do they handle the error mode you found?
3. Call `aether_get_sir` on callees - do they have complementary issues?
4. If a data flow crosses multiple symbols, trace it end-to-end and document
   any boundary where assumptions change.

## Step 4: Record results
For each finding:
1. Call `aether_sir_inject` with an improved SIR annotation. Set generation_pass
   to "deep". Include discovered error_modes. If `aether_sir_inject` is not
   available as an MCP tool, run via bash:
   `aetherd sir-inject <symbol-name> --intent "..." --edge-cases "..." --force`
2. If `aether_audit_submit` is available, submit a structured finding with
   severity, category, certainty, trigger condition, and impact.
3. Otherwise, call `aether_remember` with a structured note:
   ```
   AUDIT FINDING: [severity] [category]
   Symbol: [qualified_name]
   File: [file_path]:[line_number]
   Description: [what's wrong]
   Trigger: [what input/condition causes it]
   Impact: [what happens]
   Certainty: confirmed | suspected | theoretical
   ```

Summarize all findings at the end with severity counts.
"#
        .to_owned()
    }
}
