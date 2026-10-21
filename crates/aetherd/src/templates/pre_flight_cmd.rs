use super::TemplateContext;

#[derive(Debug, Clone, Copy, Default)]
pub struct PreFlightCommandTemplate;

impl PreFlightCommandTemplate {
    pub fn render(_context: &TemplateContext) -> String {
        r#"---
argument-hint: <task description>
description: Supply one task-specific map before implementation, with dependencies, history, risks and tests
---

Prepare a pre-implementation briefing for this task: $ARGUMENTS

This command doesn't edit files or inject SIRs. It requires no branch name and
must not run enrichment or make model/provider calls.

## Step 1: Receive the assembled map
Call `aether_preflight` first with `task` set to the full task description and
`max_tokens` set to 2500. Set `scope` only when the task already names a relevant
workspace-relative path. Read the returned `map.context` before exploring files.
Keep its revision, source/config/task/map hashes and omitted-item counts with the
briefing. The budget is a serialized byte estimate, not a tokenizer guarantee.

If the tool is unavailable, use the same service through the CLI:
`aetherd --workspace . preflight "<task description>" --max-tokens 2500 --format json`.
Pass the task as one safely quoted argument. A missing index returns a source-only
map without initializing a store. If neither entry point is available, continue
ordinary repository exploration with source search and git history, and state
that no AETHER map was supplied. Do not initialize, reindex or enrich as a fallback.

## Step 2: Check the evidence and gaps
Use the map's investigation order to inspect the cited source and its dependent
files. Keep dependencies separate from reverse impact. Treat name matches and
lineage candidates as heuristic; unknown generations mean freshness is unknown.
A missing relationship or test is not evidence of absence. Read cited commit
messages and note bodies when historical reasons matter; distinguish recorded
text from an explanation inferred from current code. Report missing or omitted
evidence and use ordinary file/git tools to fill the task's most important gaps.
Do not replace the supplied map with a mandatory chain of optional AETHER calls.

## Step 3: Briefing
Preserve the map's fixed sections: revision/freshness/coverage; investigation
order; dependencies and reverse impact; changes and recorded reasons; risks and
uncertainty; relevant tests. Add **Recommended approach:** with the order of
changes and behavior-specific checks to run. Every added recommendation needs a
source citation and a reason; mark proposed tests as unexecuted until they run.
"#
        .to_owned()
    }
}
