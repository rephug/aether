use super::TemplateContext;

#[derive(Debug, Clone, Copy, Default)]
pub struct RefactorCommandTemplate;

impl RefactorCommandTemplate {
    pub fn render(_context: &TemplateContext) -> String {
        r#"---
argument-hint: <crate-or-file>
description: Find refactoring opportunities using AETHER health and community data
---

Analyze $ARGUMENTS for refactoring opportunities using AETHER.

## Step 1: Health assessment
1. Call `aether_health` for the target to get structural risk scores.
2. Look for: high complexity, low cohesion, symbols with high betweenness
   (bottlenecks), dependency cycles, and orphaned code.
3. Call `aether_health_explain` if a crate-level summary is useful.

## Step 2: God file detection
1. Look for files exceeding 500 lines of real code.
2. Call `aether_health_hotspots` to find the hottest crates by health score.

## Step 3: Trait decomposition
If the target contains large traits or impl blocks:
1. Call `aether_suggest_trait_split` to get clustering-based split suggestions.
2. Call `aether_usage_matrix` for a detailed consumer-by-method breakdown.

## Step 4: Community and boundary analysis
1. Check which community each symbol belongs to.
2. Look for symbols that bridge multiple communities - these are boundary
   leakers that might belong in a different module.
3. Call `aether_drift_report` to detect structural anomalies.

## Step 5: Propose refactoring plan
For each refactoring opportunity, explain:
- What to extract, split, or move
- Why (backed by AETHER metrics: health score, betweenness, consumer count)
- Risk level (how many callers affected - use `aether_blast_radius`)
- Suggested order of operations

## Step 6: Prepare for refactoring
If proceeding with refactoring:
1. Call `aether_refactor_prep` to snapshot current SIR intents.
2. After refactoring, call `aether_verify_intent` to confirm no semantic drift.

Record significant findings via `aether_remember`.
"#
        .to_owned()
    }
}
