---
argument-hint: <task description>
description: Brief the affected area before implementing a task, using AETHER structural intelligence
---

Prepare a pre-implementation briefing for this task: $ARGUMENTS

This command only reads. Do not edit files, inject SIRs or run `aether_refactor_prep`
(it calls the configured inference provider) during a pre-flight.

## Step 1: Resolve the affected area
1. Pick the task's key nouns (types, functions, modules, files) and call `aether_search`
   for each with `limit` set to 10: `mode` set to `lexical` for exact names, `hybrid`
   for concepts. Hybrid search embeds the query with the project's embeddings provider;
   a lexical fallback in the response is fine.
2. Keep the 3-8 symbols most likely to change, with their `symbol_id`s, and the files
   that hold them.
3. If nothing plausible matches, ask the user where the task starts and stop.

## Step 2: What exists and what it means
For each kept symbol:
1. Call `aether_sir_context` with `symbol` set to its `symbol_id` and `max_tokens` set
   to 3000. One call returns its source, SIR, graph neighbors, health and test intents.
2. For a neighbor you need to understand, find its `symbol_id` with `aether_search` and
   call `aether_get_sir` with it.

Treat a SIR with confidence below 0.5, or an intent starting with `[MOCK]`, as a
placeholder: read the source instead of trusting its description.

## Step 3: Coupling and dependencies
1. Call `aether_dependencies` with each kept symbol's `symbol_id` for its callers and
   callees.
2. When the task changes methods of a trait or struct, call `aether_usage_matrix` with
   `symbol` set to it to see which files use which methods.

## Step 4: Health and constraints
1. Call `aether_health_hotspots` with `limit` set to 10. If an affected crate is listed,
   call `aether_health_explain` with its `crate_name`.
2. Call `aether_contract_list` with `symbol` set to each kept symbol's `symbol_id` to
   find active behavioral contracts. Skip this if the contract tools are unavailable.

## Step 5: Briefing
Answer in this shape, as briefly as the task allows:
- **What exists:** the symbols and files involved, one line each.
- **What it means:** how they work together today, from the SIRs and the source.
- **Constraints:** active contracts, invariants the SIRs state, tests that pin behavior.
- **What could break:** callers and dependents that rely on today's behavior, with file
  paths; say if an affected crate is a health hotspot.
- **Recommended approach:** the order of changes, what to test, and which symbols to run
  `/impact` on before editing.
