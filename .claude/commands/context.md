---
argument-hint: <file, directory, crate or symbol>
description: Export a self-contained briefing on part of the codebase, for pasting into any AI chat
---

Write a self-contained context document about $ARGUMENTS that the user can paste into
any AI assistant, including one with no access to this repository or its tools.

Use the tools below to gather facts, then write for a reader who has never seen the
code. This command doesn't edit files or inject SIRs.

## Step 1: Resolve the target
- **File:** call `aether_get_sir` with `level` set to `file` and `file_path` set to it.
- **Directory or crate:** call `aether_get_sir` with `level` set to `module`,
  `module_path` set to its directory (for a crate, the directory holding its sources)
  and `language` set to the language of its files: `rust`, `python`, `typescript`,
  `tsx`, `javascript` or `jsx`.
- **Symbol:** call `aether_search` with `mode` set to `lexical` to find it, then
  `aether_sir_context` with `symbol` set to its `symbol_id` and `max_tokens` set to
  4000.
- If the target is ambiguous, list the candidates and ask which one is meant.

## Step 2: Gather
1. Start from the `exports` the file or module SIR from Step 1 lists (for a symbol
   target, its own file's) and pick up to 10 that matter most: entry points and central
   types. Get each one's `symbol_id` with `aether_symbol_lookup`, `query` set to its
   qualified name (a lookup by path returns only the first 100 symbols,
   alphabetically).
2. Call `aether_get_sir` with each one's `symbol_id`. Read the source instead when a
   SIR's confidence is below 0.5 or its intent starts with `[MOCK]`.
3. Call `aether_call_chain` with the `symbol_id` of each entry point and `max_depth` set
   to 2, and `aether_dependencies` on the central types, to see what the area uses and
   what uses it.
4. Call `aether_test_intents` with `file` set to each main file to learn what the tests
   guarantee.
5. Call `aether_health_hotspots` with `limit` set to 10; if the area's crate is listed,
   call `aether_health_explain` with its `crate_name` for its weak spots.

## Step 3: Write the document
Markdown, at most about 1,500 words, in this order:
1. **Title:** "Context: <area> in <project>", taking the project name from its manifest
   or README.
2. **Purpose:** two or three sentences.
3. **Key components:** one bullet per type or function: name, file path, what it does,
   what it takes and returns.
4. **How it works:** the flow of data and control, in prose.
5. **Dependencies:** what the area relies on, and what relies on it.
6. **Guarantees and edge cases:** behavior the code or its tests pin down, and how
   errors are handled.
7. **Known weak spots:** complexity, coupling, missing tests, in plain words.
8. **Key code:** the most important signatures or short excerpts, 30 lines at most.

## Rules for the document
- It stands alone: it never refers to tools, slash commands, this conversation, or
  anything else the reader cannot see.
- Plain language, without the vocabulary of the tools you used: no "SIR", "symbol id",
  "confidence", "generation pass", "health score", "blast radius" or "AETHER", no
  `aether_*` names, and no internal ids or hashes. Write "the code's documented
  behavior" or "summary" for a SIR, "risk" for a health score, and "files that change
  together" for a blast radius, and leave confidence out. The one exception: a banned
  word that is the project's own name or an identifier in its code (a crate, type,
  field or function) is written as the code spells it.
- File paths are relative to the repository root; names are as they appear in code.
- Mark anything inferred rather than read from the code with "(inferred)".

Print the document inside one block fenced with four backticks (````markdown), so code
excerpts inside it keep their own fences and the user can copy it in one go. After the
block, add one line with its approximate word count.
