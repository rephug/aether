use super::TemplateContext;

#[derive(Debug, Clone, Copy, Default)]
pub struct ImpactCommandTemplate;

impl ImpactCommandTemplate {
    pub fn render(_context: &TemplateContext) -> String {
        r#"---
argument-hint: <symbol>
description: Assess the blast radius of changing a symbol before editing it
---

Assess what could break if $ARGUMENTS changes.

This command doesn't edit files or inject SIRs.

## Step 1: Resolve the symbol
Call `aether_search` with `query` set to $ARGUMENTS, `mode` set to `lexical` and `limit`
set to 10, and note the match's `symbol_id`. If several symbols match, list them and ask
which one is meant.

## Step 2: Direct dependents
1. Call `aether_dependencies` with its `symbol_id` for callers and callees.
2. Call `aether_get_sir` on the most important callers: what do they expect from this
   symbol (return values, errors, side effects)?
3. Call `aether_test_intents` with its `symbol_id`, and with its file, to find the tests
   that pin its behavior.

## Step 3: Wider coupling
1. Call `aether_blast_radius` with `file` set to the symbol's file for the files that
   change together with it, with their risk levels.
2. Call `aether_health_hotspots` with `limit` set to 10, and `aether_health_explain` for
   the symbol's crate if it is listed.

## Step 4: Risk rating
Rate the change:
- **High:** many callers across files or crates, a hotspot crate, high-risk coupled
  files, or callers that depend on error or side-effect behavior; or no tests.
- **Medium:** callers in a few files, some test coverage, moderate coupling.
- **Low:** few callers in the same file or module, tests present, low coupling.

## Step 5: Report
- the rating, with the two or three facts that decided it;
- files and symbols that need attention if the change goes ahead, each with why;
- tests to run or add before merging.
"#
        .to_owned()
    }
}
