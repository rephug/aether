use super::TemplateContext;

#[derive(Debug, Clone, Copy, Default)]
pub struct ReviewCommandTemplate;

impl ReviewCommandTemplate {
    pub fn render(_context: &TemplateContext) -> String {
        r#"---
argument-hint: [base-branch]
description: Semantic review of the current branch against SIRs, callers and contracts
---

Review the changes on the current branch for semantic problems, using AETHER.

SIRs describe the code as it was last indexed. Until the daemon re-indexes this branch
they describe the code before the change, which is what this review compares against;
tools that only read SIRs cannot see the new code on their own.

## Step 1: Find what changed
Use $ARGUMENTS as the base branch, or `main` if it is empty:

```bash
git diff --name-only <base>...HEAD
git diff <base>...HEAD
```

If `git status --short` shows uncommitted changes, review `git diff HEAD` as well and say
so in the report. If nothing changed, report that and stop. Call `aether_status`; if the
index is missing, review from the diff alone and say so.

## Step 2: Map the diff to symbols
For each changed source file:
1. Call `aether_symbol_lookup` with `query` set to the file path, `limit` set to 100 and
   `include_source` set to true. It returns at most 100 matches, so for a larger file
   also look up the changed functions by name.
2. Keep matches whose `file_path` equals the file and whose `source_text` contains a
   meaningful added line from the diff (ignore lines such as `}` or `Ok(())`). Note each
   one's `symbol_id` and `source_hash`.
3. Review hunks that match no indexed symbol straight from the diff: new, deleted or
   renamed symbols, symbols whose signature changed (they get a new id with no SIR
   yet), and hunks that only delete lines.

## Step 3: Compare each changed symbol with its SIR
1. Call `aether_get_sir` with the symbol's `symbol_id`.
2. If that SIR may already describe the new code (the daemon regenerated it during the
   branch), call `aether_why_changed` with the `symbol_id`. It compares the two latest
   versions and returns the prior and current intent and the fields that changed;
   `aether_symbol_timeline` lists versions with their times and commits if you need an
   older pair (`from_version`, `to_version`).
3. Compare the pre-change SIR's intent, inputs, outputs, side effects, edge cases and
   error modes with what the code now does, and classify the change:
   - **consistent:** the SIR still describes the code;
   - **intended change:** the meaning changed on purpose, so the SIR needs updating;
   - **drift:** the meaning changed in a way the branch did not intend; a likely bug.

## Step 4: Callers and contracts
1. For each symbol whose behavior or signature changed, call `aether_dependencies` with
   its `symbol_id` and check that its callers still handle it.
2. Call `aether_contract_list` with `symbol` set to each changed symbol's `symbol_id`,
   and judge every active clause against the new code yourself. Skip this if the
   contract tools are unavailable.
3. `aether_contract_check`, and `aether_verify_intent` with the saved `snapshot` id when
   this work started with `/refactor` or `/refactor-deep`, compare stored SIRs. They
   reflect this branch only once its SIRs are refreshed, so treat them as a second
   opinion, not as proof that the new code is fine.

## Step 5: Report
- **Scope:** files changed, symbols reviewed, and hunks reviewed from the diff alone.
- **Findings:** drift, caller assumptions the change breaks, and contract clauses the
  new code violates, each with file:line, severity and a suggested fix.
- **SIR updates:** list the symbols whose meaning legitimately changed, with the new
  intent in one line each, and offer to update them. Only after the user agrees, call
  `aether_sir_inject` for each with:
  - `symbol`: the `symbol_id`;
  - `intent`, `behavior`, `inputs`, `outputs`, `side_effects`, `dependencies`,
    `error_modes` and `edge_cases` describing the new code in full (a field left out
    keeps its old, now stale, value);
  - `confidence`: your honest assessment;
  - `source_hash`: the hash from Step 2, so the write is refused if the file changed
    since you read it;
  - `generation_pass`: "deep", `provider`: "claude-code", and `model`: the model you
    are running as;
  - `force`: true when the stored SIR's confidence is higher than yours, since it
    describes code that no longer exists.
"#
        .to_owned()
    }
}
