# AETHER Code Intelligence

This project uses AETHER for local code intelligence. Use the MCP tools below to ground decisions before making risky edits.

## Agent Schema Version: 1

## Inference Provider
- `gemini`

## MCP Binary Hint
- `./target/debug/aether-mcp`

## Available Tools
- `aether_status`: Get AETHER local store status
- `aether_symbol_lookup`: Lookup symbols by qualified name or file path
- `aether_dependencies`: Get resolved callers and call dependencies for a symbol
- `aether_usage_matrix`: Get a consumer-by-method usage matrix for a trait or struct, showing which files call which methods and suggesting method clusters for trait decomposition
- `aether_suggest_trait_split`: Suggest how to decompose a large trait or struct into smaller capability groups based on consumer usage patterns
- `aether_call_chain`: Get transitive call-chain levels for a symbol
- `aether_search`: Search symbols by name, path, language, or kind
- `aether_remember`: Store project memory note content with deterministic deduplication
- `aether_session_note`: Capture an in-session project note with source_type=session
- `aether_recall`: Recall project memory notes using lexical, semantic, or hybrid retrieval
- `aether_ask`: Search symbols, notes, coupling, and test intents with unified ranking
- `aether_audit_candidates`: Get ranked list of symbols most in need of deep audit review, combining structural risk with SIR confidence and reasoning trace uncertainty
- `aether_audit_cross_symbol`: trace callers and callees with full SIR and source for cross-boundary audit
- `aether_audit_submit`: Submit a structured audit finding for a symbol
- `aether_audit_report`: Query audit findings by crate, severity, category, or status
- `aether_audit_resolve`: Mark an audit finding as fixed, wontfix, or confirmed
- `aether_contract_add`: add a behavioral contract on a symbol
- `aether_contract_list`: list active intent contracts
- `aether_contract_remove`: deactivate an intent contract
- `aether_contract_check`: verify contracts against current SIR
- `aether_contract_violations`: query contract violation history
- `aether_contract_dismiss`: dismiss a contract violation
- `aether_sir_inject`: Write an improved SIR annotation back to the store for a symbol
- `aether_sir_context`: Assemble token-budgeted context for a symbol including source, SIR, graph neighbors, health, reasoning trace, and test intents in one call
- `aether_enhance_prompt`: Enhance a raw coding prompt with indexed codebase context, symbol matches, files, and architectural notes
- `aether_blast_radius`: Analyze coupled files and risk levels for blast-radius impact
- `aether_test_intents`: Query extracted behavioral test intents for a file or symbol
- `aether_drift_report`: Run semantic drift analysis with boundary and structural anomaly detection
- `aether_health`: Get codebase health metrics including critical symbols, bottlenecks, dependency cycles, orphaned code, and risk hotspots.
- `aether_health_hotspots`: Return the hottest workspace crates by health score with archetypes and top violations.
- `aether_health_explain`: Explain one crate's health score, signals, violations, and split suggestions.
- `aether_refactor_prep`: Prepare a file or crate for refactoring by deep-scanning the highest-risk symbols and saving an intent snapshot
- `aether_verify_intent`: Compare current SIR against a saved refactor-prep snapshot and flag semantic drift
- `aether_trace_cause`: Trace likely upstream semantic causes of a downstream breakage
- `aether_acknowledge_drift`: Acknowledge drift findings and create a project note
- `aether_symbol_timeline`: Get ordered SIR timeline entries for a symbol
- `aether_why_changed`: Explain why a symbol changed between two SIR versions or timestamps
- `aether_get_sir`: Get SIR for leaf/file/module level
- `aether_explain`: Explain symbol at a file position using local SIR
- `aether_verify`: Run allowlisted verification commands in host, container, or microvm mode

## Available Languages
- Rust, TypeScript/JavaScript, Python

## Search Modes
- lexical, semantic, hybrid (semantic search is enabled)

## Required Actions (mandatory)
- Always call `aether_get_sir` before reverting, deleting, or refactoring symbols.
- Always call `aether_why_changed` before reverting recent changes.
- Always call `aether_verify` after modifying code.
- If `aether_verify` fails, fix the issue before proceeding.

## Recommended Actions (advisory)
- Consider `aether_search` with hybrid mode when exploring unfamiliar code.
- Consider `aether_symbol_timeline` when reviewing recent changes.
- Consider `aether_call_chain` when tracing dependencies and downstream impact.
- Call `aether_status` at task start to confirm index freshness.

## Audit Workflow

When asked to audit code for bugs, use AETHER MCP tools to guide deep analysis.

### Finding targets
- Call `aether_health` for the target file or crate to get structural risk scores.
- Symbols with high risk_score, high betweenness, or low test_count are priority targets.
- Use `/audit <target>` for a guided audit workflow.

### Recording results
- Call `aether_remember` with structured AUDIT FINDING notes.
- If `aether_audit_submit` is available, prefer it for structured queryable findings.

### Refactoring workflow
- Call `aether_refactor_prep` before refactoring to snapshot intents.
- Call `aether_verify_intent` after refactoring to detect semantic drift.
- Use `/refactor <target>` for a guided refactoring workflow.

## Staleness Guidance
If you have made many rapid edits, call `aether_status` before trusting retrieval results. If symbol or SIR counts look stale, wait for indexing or run `aether_index_once`.

## Verify Commands
- `cargo fmt --all --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
