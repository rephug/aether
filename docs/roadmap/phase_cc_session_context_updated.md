# Phase CC Session Context — Updated After Audit Sessions

## Phase CC Status (Complete + Fixes In Progress)

| Stage | PR | Status |
|---|---|---|
| CC.1 | #131 | Merged |
| CC.1b | #135 | Merged |
| CC.2 | #132 | Merged |
| CC.2b | #133 | Merged |
| CC.3 | #134 | Merged |
| CC.4 | #136 | Merged |
| CC.5 | #137 | Merged |
| CC.6 (Prompt Enhancer) | #138 | Merged |
| CC.7 (VS Code + HTTP) | #139 | Merged |
| Batch triage | Complete | ~5877 symbols, 814 reasoning traces |
| Fix: Reasoning trace persist | Prompt ready | Unblocks full trace coverage |
| Fix: Transaction wrapper | Prompt ready | Resolves findings #1,2,7,25,27 |
| Fix: Silent error suppression | Prompt ready | Resolves findings #6,13,23,26+8 |

## Audit Session Results

### First audit: `/audit aether-store` (14 findings)

| ID | Severity | Category | Symbol | Description |
|---|---|---|---|---|
| 1 | HIGH | state | store_mark_removed | 6 DELETEs without transaction |
| 2 | MEDIUM | state | store_mark_removed | Orphaned data in 9 tables |
| 3 | MEDIUM | logic_error | build_audit_where_clause | LIKE wildcard injection (_ in paths) |
| 4 | MEDIUM | logic_error | store_count_audit_findings_by_severity | Severity total invariant broken |
| 5 | MEDIUM | concurrency | SqliteStore | Mutex poisoning cascade |
| 6 | LOW | silent_failure | current_unix_timestamp | Returns 0 instead of error |
| 7 | MEDIUM | resource_leak | delete_symbol_records_for_ids | Orphaned sir_embeddings |
| 8 | MEDIUM | encoding | normalize_commit_hash | Rejects uppercase hex A-F |
| 9 | MEDIUM | arithmetic | get_schema_version | i64→u32 truncation |
| 10 | LOW | state | store_read_sir_blob | TOCTOU race |
| 11 | LOW | silent_failure | open_graph_store | Silent Cozo→Surreal mapping |
| 12 | LOW | logic_error | infer_symbol_is_public | Substring false positives |
| 13 | LOW | silent_failure | current_time_millis | Copies unwrap_or(0) antipattern |
| 14 | LOW | logic_error | recency_factor | Fragile timestamp heuristic |

### Second audit: `/audit aetherd` (28 findings, 7 submitted)

| ID | Severity | Category | Symbol | Description |
|---|---|---|---|---|
| 22 | MEDIUM | silent_failure | rewrite_prompt | No LLM timeout — CLI hangs forever |
| 23 | MEDIUM | silent_failure | read_sir_annotation | .ok() discards SIR parse errors |
| 24 | MEDIUM | arithmetic | build_symbol_context | NaN risk_score → panic/wrong u32 |
| 25 | HIGH | state | process_removed_symbols | Non-atomic multi-symbol removal |
| 26 | HIGH | concurrency | SharedQueueState | Silent mutex poisoning |
| 27 | MEDIUM | state | persist_successful_generation_sqlite | 5-step non-atomic write |
| 28 | MEDIUM | resource_leak | sync_graph_for_file | Per-file connection opening |

Plus ~19 LOW findings from background agents covering: extract_json_object,
classify_task_type, first_sentence, collect_coupling_notes, read_intent_summary,
BudgetStatus::Omitted, estimate_tokens, consume_sir_requests, get_sir_meta,
handle_failed_generation.

### Post-session audit: `/audit-changes HEAD~10` (2 findings)

| ID | Severity | Category | Symbol | Description |
|---|---|---|---|---|
| 13 | LOW | silent_failure | current_time_millis | Antipattern propagation from aether-store |
| 14 | LOW | logic_error | recency_factor | Fragile timestamp heuristic |

## Two Systemic Antipatterns Identified

**Pattern 1: Non-atomic multi-step writes**
- 3+ instances across aether-store and aetherd
- Same fix: wrap in `Transaction::new_unchecked`
- Correct pattern already exists in `reconcile_and_prune`

**Pattern 2: Silent error suppression via .ok() and if let Ok**
- 10+ instances across 3 crates
- SIR blob reads: 4 files with `.ok().flatten()` discarding parse errors
- Mutex poisoning: `if let Ok(lock)` makes daemon appear healthy while degraded
- Timestamps: `unwrap_or(0)` on SystemTime

## Contracts Created

| ID | Symbol | Clause | Status |
|---|---|---|---|
| 1 | store_mark_removed | must: wrap all DELETEs in single SQLite transaction | Active |

## Cross-Symbol Analysis Results

- `store_mark_removed` has 0 callers in graph (trait dispatch gap)
- 4 actual callers found via manual grep: sir_pipeline (×2), indexer, aether-mcp/sir.rs
- All callers use mark_removed in loops — amplifies crash window

## Tool/Infrastructure Bugs Discovered

| Bug | Severity | Status |
|---|---|---|
| `aether_suggest_trait_split` ambiguity on identical symbols | LOW | Known |
| `aether_suggest_trait_split` 97K response exceeds token limit | LOW | Known |
| `aether_audit_cross_symbol` trait dispatch gap | Limitation | Documented |
| `audit-report` CLI subcommand missing from CC.2 | LOW | Known |
| Reasoning trace not persisting for ~90% of triage symbols | MEDIUM | Fix prompt ready |
| `aether_audit_submit` fails for symbols without SIR rows | LOW | Known |

## Batch Triage Configuration

```toml
[batch]
provider = "gemini"
jsonl_chunk_size = 200

[batch.gemini]
scan_model = "gemini-3.1-flash-lite-preview"
triage_model = "gemini-3.1-flash-lite-preview"
deep_model = "gemini-3.1-flash-lite-preview"
scan_thinking = "off"
triage_thinking = "medium"
deep_thinking = "off"
```

- Medium thinking = 100% trace rate, ~2000 chars avg
- Low thinking = ~9% trace rate (model chooses when to think)
- Full corpus triage cost: ~$17 at medium thinking
- 500-symbol test run: $1.72

## SIR Prompt Fix Applied

Both scan and triage system prompts now include "(Semantic Intent Record)"
to prevent the model from guessing what SIR stands for:
- `sir_prompt.rs` line 342: scan combined prompt
- `sir_prompt.rs` line 401: enriched combined prompt  
- `sir_prompt.rs` line 506: scan split system prompt
- `sir_prompt.rs` line 546: enriched split system prompt

## Decisions Registered

- **#103:** Slash commands generated by init-agent
- **#104:** sir_audit table (schema v18)
- **#105:** Audit + SIR inject MCP tools
- **#106:** CLAUDE.md audit section
- **#107:** aether_sir_context MCP tool
- **#108:** Prompt enhancer (template-first approach)
- **#109:** VS Code daemon HTTP endpoint
- **Next available: #110+**

## Current Repository State

- **Schema:** v18
- **PRs:** Through #139
- **MCP tools:** 40 (was 26 before Phase CC)
- **Slash commands:** 4 (/audit, /refactor, /audit-report, /audit-changes)
- **TOOL_DESCRIPTIONS:** 39 entries in templates/mod.rs
- **Dashboard port:** 9730
- **LOC:** ~155K across 17 crates
