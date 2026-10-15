use super::TemplateContext;

#[derive(Debug, Clone, Copy, Default)]
pub struct ExplainAreaCommandTemplate;

impl ExplainAreaCommandTemplate {
    pub fn render(_context: &TemplateContext) -> String {
        r#"---
argument-hint: <file, directory, crate or symbol>
description: Explain an unfamiliar area of the codebase in plain prose
---

Explain $ARGUMENTS to a developer who has not worked on it before.

This command doesn't edit files or inject SIRs. Keep the explanation readable in two
minutes.

## Step 1: Resolve the target
- **File:** call `aether_get_sir` with `level` set to `file` and `file_path` set to it.
- **Directory or crate:** call `aether_get_sir` with `level` set to `module`,
  `module_path` set to its directory (for a crate, the directory holding its sources)
  and `language` set to the language of its files (`rust`, `python`, `typescript`, ...).
- **Symbol:** call `aether_search` with `mode` set to `lexical` to find it, then
  `aether_sir_context` with `symbol` set to its `symbol_id`.
- If the target is ambiguous, list the candidates and ask which one is meant.

## Step 2: Find the main pieces
1. Start from the `exports` the file or module SIR from Step 1 lists (for a symbol
   target, its own file's) and pick the 3-6 that matter most: public entry points and
   the types other code builds on.
2. Get each one's `symbol_id` with `aether_symbol_lookup`, `query` set to its qualified
   name (a lookup by path returns only the first 100 symbols, alphabetically).
3. Call `aether_get_sir` with each one's `symbol_id`. Read the source where a SIR's
   confidence is below 0.5 or its intent starts with `[MOCK]`.

## Step 3: Follow the flow
1. Call `aether_call_chain` with the `symbol_id` of each entry point and `max_depth`
   set to 3 to see what it calls.
2. Call `aether_dependencies` on the central types to see who uses the area from
   outside.

## Step 4: Write the explanation
Use plain prose, about 300-500 words:
- what the area is for, in one or two sentences;
- its main pieces and how they fit together;
- how a typical request or call moves through it;
- what depends on it from outside, and what it depends on;
- surprises and sharp edges (error handling, invariants, shared state);
- where to start reading, as three file paths in order.

Name files and functions so the reader can find them, but do not paste tool output.
"#
        .to_owned()
    }
}
