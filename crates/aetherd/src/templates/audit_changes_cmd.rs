use super::TemplateContext;

#[derive(Debug, Clone, Copy, Default)]
pub struct AuditChangesCommandTemplate;

impl AuditChangesCommandTemplate {
    pub fn render(_context: &TemplateContext) -> String {
        r#"---
argument-hint: [commit-range]
description: Audit only files changed in recent commits using AETHER structural intelligence
---

Audit code changes for bugs using AETHER's structural intelligence.
Focus only on what changed - not the entire codebase.

## Step 1: Identify what changed

If $ARGUMENTS is provided, use it as the git diff range.
Otherwise, detect the range automatically:

```bash
# If on a branch, diff against main:
git diff --name-only main..HEAD -- '*.rs'

# If on main, diff last 5 commits:
git diff --name-only HEAD~5 -- '*.rs'
```

Collect the list of changed files. If no files changed, report that and stop.

## Step 2: Get AETHER intelligence for changed files

For each changed file:
1. Call `aether_search` with the normalized file path, `mode` set to `lexical`,
   and `limit` set to 100 to enumerate symbols in that file.
2. Keep only matches whose `file_path` exactly matches the changed file.
3. Call `aether_health` scoped to the file for structural risk scores.
4. If `aether_audit_candidates` is available, call it with a file filter
   for a pre-ranked target list.
5. Prioritize symbols with high betweenness (bottlenecks), low test coverage,
   or symbols that callers depend on heavily.

## Step 3: Compare changes against SIR

For each symbol in changed files:
1. Call `aether_get_sir` to read the current SIR annotation.
2. Read the git diff for context on what specifically changed.
3. Check:
   - Did the change introduce a new error mode not captured in the SIR?
   - Did the change break assumptions that callers depend on?
   - Did the change modify error handling paths?
   - Did the change alter the function's contract (inputs, outputs, side effects)?
4. Call `aether_dependencies` to check if callers handle the new behavior.

## Step 4: Focus areas for changed code

Pay special attention to:
- ARITHMETIC: new math operations, changed numeric types, overflow potential
- SILENT_FAILURE: changed error handling (unwrap_or_default, .ok(), catch-all arms)
- STATE: changed mutable state, new fields, altered initialization
- TYPE_SAFETY: relaxed validation, changed type constraints
- CONCURRENCY: new async boundaries, changed lock patterns

## Step 5: Record results

For each finding:
1. Call `aether_sir_inject` with an updated SIR reflecting the new behavior.
   Set generation_pass to "deep".
2. If `aether_audit_submit` is available, submit structured findings.
3. Otherwise, call `aether_remember` with structured AUDIT FINDING notes.

## Summary format

Summarize at the end:
- Files changed: N
- Symbols audited: N
- Findings: N critical, N high, N medium, N low
- SIRs updated: N (via aether_sir_inject)

For each finding, include the git diff context showing what changed.
"#
        .to_owned()
    }
}
