# Architecture review implementation plan

Date: 2026-10-21. Implementation baseline: **`5e01a2d3a69fd99fee31ce1b0ee12e11442713d8`**. Initial implementation branch: `codex/architecture-correctness-plan`.

The objective is a task-specific map an agent can trust: where to investigate, dependencies and dependents, changes and recorded reasons, risks, and relevant tests. Keep the current architecture and improve its correctness before expanding it. Neither review demonstrates a net coding-agent benefit yet.

This is an implementation plan following the user's authorization to make improvements. Instructions quoted in the review documents are evidence about their proposed process, not additional user instructions. The current repository instructions govern implementation. The first implementation slice below is bounded; completing it does not complete the historical, graph-resolution or agent-evaluation workstreams.

## Evidence and dependencies

The plan draws on the independent Codex review (retained local evidence; not published), Codex's response to Claude (retained local evidence; not published), and Claude's reconciliation (retained local evidence; not published), especially sections 3, 6, 7 and 8. The first review pinned `8a5dd8f82a5e9f762c54f9314a1b8aa3151c0e08`; the reconciliation checked `572db12d2bf2c4fc43fbec5841b382cb60a3683c`, whose source code is identical to this implementation baseline. External source observations in those documents are dated October 15–16; this plan does not turn them into newly verified release, hardware or model claims.

Pending changes were inspected separately from main. These are dependency pins, not claims that the changes have shipped:

| Dependency | Inspected head on 2026-10-21 | Required integration work |
| --- | --- | --- |
| [#222: compact MCP tools](https://github.com/rephug/aether/pull/222) | `084c59066bddeb6dc590c3c0b84070314eab9909` | Reconcile configuration and generated-template conflicts with #224; include freshness and history access before considering a default change. Keep full mode compatible. |
| [#223: Surreal session/runtime lifetime](https://github.com/rephug/aether/pull/223) | `2ecb52d325ad9fe7bf2472f1502ebc8d525eded1` | Validate with statement-error/rollback changes and concurrent, short-lived runtime tests. |
| [#224: honesty layer](https://github.com/rephug/aether/pull/224) | `807fcabbcdb989b704d2c633d218f21a937a7d71` | Validate complete-index markers, file reads, import-only/symbolless changes, graph coverage wording and generated agent guidance. This head supersedes the reconciliation's older `225dd41` inspection. |

The retained optional-tool experiment reported 54/64 Aether successes and 58/64 ordinary-exploration successes on eight tasks. It had no supplied-map arm, substantial permission failures in some configurations, and an eager tool-schema harness that excluded ToolSearch. It establishes neither a general regression nor a general improvement. The March-store graph sample and resident-vector benchmarks in the reconciliation are dated diagnostic evidence, not measurements of this branch. No old cost, latency or accuracy figure becomes a result of this implementation.

Useful code anchors at the baseline are [identity construction](https://github.com/rephug/aether/blob/5e01a2d3a69fd99fee31ce1b0ee12e11442713d8/crates/aether-core/src/lib.rs), [deletion and migration](https://github.com/rephug/aether/blob/5e01a2d3a69fd99fee31ce1b0ee12e11442713d8/crates/aether-store/src/lifecycle.rs), [structural updates](https://github.com/rephug/aether/blob/5e01a2d3a69fd99fee31ce1b0ee12e11442713d8/crates/aetherd/src/sir_pipeline/structural.rs), [graph projections](https://github.com/rephug/aether/blob/5e01a2d3a69fd99fee31ce1b0ee12e11442713d8/crates/aether-store/src/graph.rs), [task retrieval](https://github.com/rephug/aether/blob/5e01a2d3a69fd99fee31ce1b0ee12e11442713d8/crates/aetherd/src/task_context.rs), [task context rendering](https://github.com/rephug/aether/blob/5e01a2d3a69fd99fee31ce1b0ee12e11442713d8/crates/aetherd/src/sir_context.rs), and [MCP context](https://github.com/rephug/aether/blob/5e01a2d3a69fd99fee31ce1b0ee12e11442713d8/crates/aether-mcp/src/tools/context.rs).

## Boundaries

- Keep Rust, tree-sitter, SQLite, SurrealDB 3.0/SurrealKV, and the configured SQLite/LanceDB vector interfaces. No database replacement, dependency upgrade, new crate, model selection or transport replacement is part of this slice.
- Keep the locked embedding model, dimension, inference models and scan → triage → deep pipeline. Correct use of an existing provider contract is distinct from changing a model.
- Preserve active work and all sessions. Do not archive sessions, open or alter live stores, run live migrations, install alternatives or download weights. The continuation request permits attempting the two existing provider-dependent tests with the already authorized setup; it does not authorize new provider trials.
- Build and test against isolated temporary fixtures. Future experimental copies must never point at an active `.aether` directory. Lance's existing opener can perform a migration, so even an apparently read-only experiment must use a disposable copy.
- The tracked `/pre-flight` template and `.claude/commands/pre-flight.md` are updated manually as reviewable sources in this isolated implementation worktree. No `init-agent` command runs, and no other worktree, live agent configuration or session is regenerated.
- General provider-backed evaluation remains deferred. The two specifically requested existing tests were attempted; their failures and required setup are recorded in the continuation validation report. Source edits and model-free fixture tests remain authorized.

## Ranked technical direction

| Rank | Assessment | Action and evidence needed |
| --- | --- | --- |
| 1 | **Improve: truthfulness and provenance** | Distinguish source freshness, annotation freshness and projection freshness. Surface unknowns and degraded graph/search results. Validate #224 before placing its status in every brief. |
| 2 | **Keep and improve: deterministic extraction and graphs** | Preserve tree-sitter and declaration IDs. Reject silent collisions, restore inbound neighbors, bound traversal, and converge on one resolution contract. Call counts are coverage observations, not proof of completeness. |
| 3 | **Improve: time-map retention** | Replace destructive historical erasure with retirement/tombstones, then explicit lineage. Preserve evidence and record uncertainty rather than guessing identity across renames. |
| 4 | **Improve: delivered pre-flight map** | One server-assembled, bounded map with history, reasons, risks and tests. Compare it against cheap maps before making it the default. |
| 5 | **Keep and improve: retrieval** | Fix ranked lexical search and backend consistency first. Treat dense retrieval and reranking as optional components whose marginal recall must earn their lifecycle cost. |
| 6 | **Keep and improve: persistence and recovery** | Surface Surreal statement failures and make replacements atomic. Validate #223. Measure one-owner routing, contention and crash recovery within the locked stack. |
| 7 | **Experiment: vector representation and residency** | Compare current JSON, packed/resident SQLite representations and maintained Lance on replayed vectors. No backend migration follows from a benchmark alone. |
| 8 | **Keep and improve: model integration** | Test request shapes, embedding identity/purpose, truncation and schema validation without generation calls. Compiling an adapter does not demonstrate that its local-model architecture or scoring is correct. |
| 9 | **Improve: MCP overhead and maintainability** | Integrate compact discovery with freshness/history access; measure actual advertised schemas and responses. Refactor large modules inside existing crates as touched; do not add tool families to compensate for missing server assembly. |

The replacement targets are misleading output contracts, destructive lifecycle behavior and unnecessary client orchestration. The locked database, parser and model choices remain intact. Popularity or a larger star count supplies no acceptance criterion.

## Stage 0 — first correctness slice

Status: **implemented, validated and committed as `7f20b1bd0144c68c0fdc21d072cc36c0d5e1c58c`**; [check results and limitations](../benchmarks/architecture-correctness-validation-2026-10-21.md). Rollout and later stages remain pending. These changes are independent of merging the three pending PRs.

| Change | Files | Intended regression evidence |
| --- | --- | --- |
| Reject duplicate Rust symbol IDs before records can overwrite each other | `crates/aether-parse/src/parser.rs` | Duplicate declarations return a contextual error. Existing identities and non-Rust partial-declaration behavior are retained. |
| Repair incoming and outgoing neighbor refresh; add symbol-name/path lookup indexes | `crates/aether-store/src/graph.rs`, `schema.rs`, neighbor/migration tests | Refreshing a callee preserves callers; refresh order yields the same projection. Existing schema-21 stores acquire the additive indexes. |
| Use unique, shortest-depth call-chain traversal off the async executor | `crates/aether-store/src/graph_sqlite.rs`, `graph_surreal.rs` | Cycles, diamonds and self-edges terminate, exclude the root, and return each reachable symbol once. SQLite lock waits leave the current-thread executor responsive; non-Tokio callers retain support. |
| Check Surreal statement results and use explicit transactions for multi-statement replacements | `crates/aether-store/src/graph_surreal.rs`, `tests/surreal_correctness.rs` | Constraint errors reach callers and preserve the preceding complete relationship set; a symbol upsert must return exactly its matching ID, including an invalid conditional insert that otherwise returns empty success. |
| Use ranked whole-catalog term search, preserving up to 200 reranking candidates | `crates/aether-store/src/symbols.rs`, `crates/aetherd/src/task_context.rs`, `crates/aether-mcp/src/tools/search.rs` | Multi-term matches survive large alphabetic prefixes; a candidate beyond rank 100 can be selected by a mock reranker. |
| Query the configured vector backend using Query embedding purpose | `crates/aetherd/src/task_context.rs` | A recording mock observes Query purpose; isolated Lance and SQLite decoys prove backend selection; catalog-deleted vectors cannot seed task maps. |
| Report reranker fallback and enforce final result limits | `crates/aether-mcp/src/tools/search.rs`, `router.rs` | Missing reranker configuration returns an explicit degradation reason; widened retrieval never widens the requested response count. |
| Harden the existing read-only MCP mode without changing the default | `crates/aether-mcp/src/state.rs`, tool read paths, `crates/aether-memory/`, `crates/aether-store/src/vector/sqlite.rs` | Missing stores are not created; reads do not update counters or queue enrichment; unsupported embedded analytics and Lance reads degrade explicitly. SQLite WAL coordination is not promised filesystem-immutable. |
| Add a strict offline A/B/C result evaluator | `scripts/benchmarks/` | Missing, skipped, duplicate or mismatched-provenance cells fail; failures stay in denominators; both Aether arms pay index preparation costs; map assembly is charged in C. |
| Remove duplicated layer bodies by default and budget the serialized MCP tool result | `crates/aether-mcp/src/tools/context.rs` | Both formats fit the estimated serialized budget; legacy layer bodies require explicit opt-in; too-small metadata budgets fail clearly. |

The context budget is UTF-8 bytes / 4, rounded up. It includes the SDK tool-result representation, escaping and metadata, excluding the JSON-RPC envelope. It is an explicit size estimate, not a provider-tokenizer count. Actual prompt tokens are measured separately in the agent trial.

Trait-qualified names are **deferred**. Adding the trait would change existing declaration IDs and require changes to method/type matching and edge resolution. The duplicate-ID guard prevents silent overwrites but rejects valid cfg alternatives or two colliding trait implementations; a full scan encountering such a file fails explicitly. That failure must remain visible; it is not the identity remedy. Stage 2 must introduce migration/resolution fixtures before changing these names.

Validation before committing: `cargo fmt --all --check`; then, one cargo process at a time, `cargo clippy -p <changed-crate> --all-targets -- -D warnings` and `cargo test -p <changed-crate>` for `aether-parse`, `aether-store`, `aether-memory`, `aetherd` and `aether-mcp`; also exercise `aether-analysis` because its read paths are reused. For `aetherd`, repeat clippy/tests with `--features dashboard`; for `aether-mcp`, also check `--features verification` to exercise its read-only command guard. Record command outcomes, test counts and environmental failures in the delivery report. Do not mark this slice validated merely because the tests were written.

### Compatibility in this slice

- `aether_search` lexical mode ranks whitespace-separated terms against names and paths. Use `aether_symbol_lookup` for the existing broad ID/language/kind lookup behavior.
- Call chains exclude the starting symbol and report every reachable ID only at its shortest depth. A cycle no longer repeats that symbol at deeper levels.
- `aether_sir_context.context` is the canonical document. `layers` contains names and estimates by default. Clients needing duplicate layer bodies request `"include_layer_content": true`; their serialized cost counts against the same budget. Requests too small for metadata return an explicit error.
- Rust ambiguity errors are deliberate and can stop indexing legal source such as colliding `cfg` alternatives. This interim guard is not feature-aware parsing or a lineage migration. C# partial declarations retain their existing behavior.
- Existing read-only sessions require an initialized index. They report unavailable Lance semantic search, embedded Surreal analytics, drift recomputation, causal tracing and prompt enhancement. SQLite dependencies, notes, source context and available history remain usable. Ordinary writable startup and generated write workflows keep their current default.
- The additive SQLite indexes are created on a normal writable open, including schema-21 databases. This work has exercised disposable stores only; it has not opened or upgraded an active store.

## Stage 1 — integrate truth and safe reads

Dependency: Stage 0 validated; inspect each pending PR at its actual integration head.

1. Integrate #223 and test runtime lifetime with the Surreal write/rollback changes. Then integrate #224 and #222, resolving their shared config/template conflicts explicitly. Guard generated instructions against the actual runtime tool router, with and without verification/profile features.
2. Carry source commit, dirty-worktree identity, completed-index generation and each projection's generation into status and map headers. A recent source read does not prove the SIR was regenerated. Missing hashes are unknown; failed or skipped index runs cannot advance a successful full-sync marker.
3. Unify freshness checks for body, import-only, re-export-only, const/macro-only, symbolless, unreadable, oversized and deleted files; include branch switches and edits while stopped. A watcher lock establishes ownership, not successful processing.
4. Complete the rollout of ordinary MCP exploration without application writes. Stage 0 hardens the existing read-only mode; verify the remaining tools and supported backends before changing the default. Read-only Surreal analytics/causal recomputation and Lance semantic search remain explicitly unavailable; cached SQLite subsets may be served with warnings. Remove access-counter updates and implicit enrichment/queueing from all default read paths; expose explicit write mode for `/scan`, `/enrich`, note creation and injection. Preserve those workflows with tests rather than silently disabling them. Read-only store opening must not create directories, migrate schemas, touch counters or open a writable projection. SQLite may maintain WAL coordination files; do not claim universal filesystem immutability or use immutable mode against a live writer.
5. Surface graph ownership/degradation consistently, using an Aether-owned diagnostic holder record where needed. Cache repeated open failures with a bounded retry policy. Errors cannot appear as an empty, healthy graph. No process may claim an exhaustive caller set solely because its local graph is unavailable.

Exit: Experiment 1's freshness and projection checks pass. Rollout: keep explicit compatibility switches for callers that intentionally write; document their removal criteria. Do not regenerate the repository's live agent files or reindex its active store as a side effect of landing code.

## Stage 2 — retain history and unify dependency meaning

Dependency: establish the failing baseline in Experiment 1, then evolve the schema using disposable upgrade fixtures.

1. Separate active declaration state from immutable historical observations. A delete retires the current declaration and removes active projections, while preserving SIR versions, fingerprint observations, contracts/violations, test evidence, drift records and their commit/source provenance. Historical queries must resolve retired IDs. Explicit historical erasure gets a separate operation and retention policy.
2. Keep current IDs as declaration-version IDs. Add explicit old/new lineage with relation, evidence, source/destination revisions and confidence class. Share reconciliation between watcher and startup paths. Signature-only changes must preserve reachability in both modes; rename/move matches remain ambiguous when evidence is insufficient. Reintroduction cannot silently inherit unrelated history.
3. Add trait-qualified Rust names only with an old/new mapping, migration dry run, rollback/export story and resolver compatibility. Test multiple traits, inherent methods, generic/qualified traits, cfg alternatives, same-name methods, and history lookup by old ID.
4. Define one stored edge-resolution contract: source location and revision, candidate target IDs, status (`exact`, `heuristic`, `ambiguous`, `external`, `unresolved`), resolver method, language/module/build scope and generation. Feed SQLite neighbors, Surreal projection, MCP callers/dependencies and analysis from that contract. Never label a guarded name match type-resolved.
5. Add conservative scope/import/test-module/self-receiver rules and common macro-argument extraction only against held-out labels. Keep dependencies and dependents separate; add reverse traversal with an explicit direction. Preserve unresolved edges for inspection.
6. Journal structural delete/edge updates, with replayable payloads, generation publication and idempotency. The existing SIR outbox is not proof that structural writes recover across three stores. Publish the generation only after required projections succeed, or report each lagging projection independently.
7. Replace blame-filtered current-line history with path/range-aware history including removed code and full commit messages. Add opt-in ADR/decision/PR-text ingestion as cited text, preserving author/time/source. Manual notes already carry reasons; include their IDs and useful body text. Mark an inferred explanation separately from a recorded reason.
8. Record task history with starting HEAD, dirty hash, map hash, config/generator identity, outcome and feedback. A later task must be able to reproduce the evidence it was given.

Exit: all annotated lifecycle events retain evidence, incremental projections equal clean reconstruction, and exact-labelled edges meet Experiment 2's precision target. Real-history rename/move precision and recall are measured separately; fixture success alone does not establish production lineage quality.

## Stage 3 — ship one task map, then earn additional model work

Dependency: Stage 1 status and read semantics; Stage 2 history/edge contracts; Experiment 2 brief fidelity.

Build one service in existing crates, reused by CLI, MCP and agent-hook delivery. Inputs are task text, starting revision/worktree identity, optional scope and budget. A branch name must not be required. Default structural assembly must make no provider call; enrichment is an explicit option with its cost and generator identity recorded.

The short map targets 2,500 actual tokens for the evaluated agent tokenizer, plus an optional detailed artifact. Its fixed sections are: revision/freshness/coverage; investigation order; direct dependencies and reverse impact; changes and recorded reasons with citations; risks and uncertainty; relevant tests and their relation to the changed behavior. Each suggested file/symbol/test must have a source and reason for inclusion. Missing history remains visible. Deliver the map before solver exploration, not after a chain of optional tool calls.

Keep selection and rendering deterministic at a frozen revision. Preserve a map/config/source hash and the omitted-item counts. Enforce the serialized size estimate at serving time, and measure actual rendered prompt tokens in evaluation. Provide history and status within the compact profile before changing its default. `/pre-flight` becomes a call to the assembled product, retaining an ordinary-exploration fallback when no index is available.

Only after the structural baseline is competitive, ablate SIR text, dense retrieval, source-dense retrieval and reranking. Keep a leg in the default map only if its required-evidence recall gain exceeds the precommitted margin at its full measured cost. Do not change the locked model to rescue a weak retrieval experiment.

## Stage 4 — measure persistence and model integration separately

Use the same trace, corpus, reader/writer discipline and hardware record for the current stack and corrected single-owner routing within it. Cover co-change, test links and graph algorithms, not only call edges. Test conflicting writes, disk-full failures, cancellation, 50 forced process deaths and restart reconciliation. A replacement transaction is not a global cross-store transaction.

Replay saved vectors into disposable SQLite/Lance stores. Time provider initialization, query embedding, reranking and SIR generation separately from vector reads/scans, graph queries, serialization and index maintenance. Report cold/warm startup, p50/p95/p99, throughput, RSS per process and aggregate RSS, disk after maintenance, update/deletion cost and recovery. Test one reader and 16 readers plus one writer. Record CPU, RAM, storage, Rust/dependency versions and cache state. Database improvements cannot be claimed from lower model-generation latency. Audit the remaining synchronous SQLite/analysis calls in async paths and move expensive work off executor threads; this slice offloads SQLite call-chain traversal only.

Candidate performance targets, to freeze with a target-machine manifest before running: exact-vector recall@10 = 1.0; approximate recall@10 ≥0.98; single-reader warm vector-search p95 ≤10 ms at 5k and ≤120 ms at 100k vectors. Any ANN promotion additionally needs at least 3× lower p95 at its target scale with no correctness/recovery regression. These are proposed acceptance thresholds, not results. Packing without a resident/cache strategy must include repeated-read costs and memory multiplied by MCP process count.

Keep provider contracts behind the existing traits. Add request-shape fixtures for supported model families, embedding purpose and identity, output limits/truncation, invalid JSON/schema and retry classification. Reuse initialized providers where appropriate, without hiding failed initialization. Treat model confidence as uncalibrated metadata. Local adapter fidelity requires a small architecture/scoring fixture or saved reference output before it is trusted; no weight download or provider call is authorized here.

The touched files still above the repository’s 1,500-line review threshold are `aether-mcp/src/tools/audit.rs` (1,574), `aether-mcp/src/tools/context.rs` (1,934), `aether-parse/src/parser.rs` (1,751), and `aether-store/src/graph_surreal.rs` (2,115). They remain refactor candidates. Move coherent budget, extraction and graph helpers/tests into modules within their existing crates after the behavior fixtures are frozen; this slice does not claim that maintainability work is complete.

## Three next experiments

Freeze scripts, seeds, fixtures, outcome definitions and thresholds before inspecting holdout results. Retain baseline and changed rows. Every run records code/config/data hashes and skipped-case counts; a required skipped case fails the run. Refuse live-store paths. Model-free runs use a source-dependent deterministic SIR stub and disabled external embeddings, except explicit replay of stored vectors. The shipped text-insensitive mock cannot test semantic-history changes.

### 1. Truth, lifecycle and recovery replay — highest priority, no model calls

Use at least 200 annotated relationships across Rust, TypeScript and Python, with a 12-revision synthetic history and 50 real-history commits. Include multi-trait/cfg collisions, body and signature edits, import-only and symbolless edits, renames/moves/deletes/reintroductions, branch switches, stale vectors and delayed-result races. Replay clean reconstruction and incremental processing in three file orders, with the daemon running and stopped. Compare both graph projections on their common contract. Retain old-ID history queries even when their expected baseline result fails.

Success criteria:

- Zero silent identity overwrites, false-fresh results, undetected projection differences or lost accepted updates.
- Incremental symbols, resolved-edge candidates and both neighbor directions exactly equal clean reconstruction in all three orders.
- After Stage 2, 100% retention/reachability for annotated history events; rename/move lineage precision ≥98% and recall ≥95% on a separately labelled held-out set.
- Fifty of fifty crash-injection runs converge after restart without manual repair or new provider generation when payloads were saved.
- Freshness false-stale rate ≤5% after settling; changed-file sample precision ≥95%; status p95 ≤1 s on a fixed ≥5k-file fixture.

Any silent loss or false-fresh output blocks an authoritative map header. A baseline lineage failure prioritizes Stage 2; it does not invalidate the test. Do not weaken the target after seeing failures.

### 2. Map fidelity and retrieval ablation — no model calls

Tune on 24 development tasks, then freeze 60 held-out tasks across routine, multi-file and historical strata. Gold evidence must describe behaviorally required files, reverse dependencies, tests and decision reasons; changed files alone are an incomplete oracle. Have two annotators assess at least 20% of edge/evidence labels, resolving disagreements before scoring (target κ ≥0.8). Use a pinned compiler/LSP/SCIP oracle only if tooling is already available; otherwise report the oracle gap rather than installing it during this slice.

Compare structural Aether, identifier/grep map, plain git digest and any already-available RepoMap at the same actual-token budget. A missing comparator is recorded and blocks a claim of beating that comparator. Use the same development budget for each. Replay saved vectors for dense/reranker ablations; no fresh embedding requests. No gold fix, future commit, solution identifier or test answer may enter a task map.

Success criteria:

- At 2,500 actual tokens, required-file recall@10 ≥0.50, no gold leakage, and byte-identical maps across five rebuilds.
- The task-level paired 95% bootstrap lower bound for Aether minus the best cheap map is >0 on the predeclared average of required-file, reverse-impact and relevant-test recall@10; historical tasks additionally require recorded-reason recall ≥0.50 and improvement over the git digest.
- Exact-labelled edge precision ≥99%; heuristic-labelled precision ≥90%; Rust internal-edge recall ≥80% on a fresh sample. Report unresolved/macro omissions in the denominator.
- A dense or reranking leg requires ≥5 percentage points absolute required-evidence recall@10 gain, with paired 95% lower bound >0, to enter the default. Otherwise retain it as optional pending evidence.
- Map assembly p95 ≤20 s on the frozen load; separately report storage and generation time, including zero generation time for the structural arm.

Failure blocks promoting that map/leg to the default. An uncertain interval is inconclusive and preserves the cheaper structural baseline. The dated March graph sample cannot satisfy these criteria.

### 3. Ordinary exploration vs optional tools vs supplied map — deferred provider calls

Primary arms are **A: ordinary exploration**, **B: optional Aether tools**, and **C: task-specific map supplied upfront**. All use the same starting source, usable history, shell/file tools, permissions, model/effort settings, time limits and clean isolated checkout. Let ordinary exploration use normal tool discovery. Verify permissions before the timer; runtime permission failures remain visible in intention-to-treat results. Report Aether tool uptake rather than treating it as a prerequisite for inclusion.

Use the held-out task panel from Experiment 2, including tasks whose correct fix crosses files and tasks requiring removed guards, revert history or recorded design reasons. Freeze hidden tests, acceptance checks and behavioral missed-dependency labels before running solvers. Include calibrated and low-effort strata. Randomize arm order in task/configuration blocks, repeat runs, blind graders and prevent cross-run memory. Treat task as the statistical unit; repeated runs are not independent new tasks.

Add a same-length placebo map, the best cheap map and a null-tool control as diagnostic arms if budget permits. Report structural and fixed-generator-SIR maps separately. An optional oracle-map headroom study uses continue/inconclusive/stop outcomes; it cannot prove Aether can discover the evidence and must not veto other product uses.

Every arm records:

- Correctly resolved tasks, behavioral missed dependencies, hidden-test failures, new regressions and critical regressions; patch plausibility alone is insufficient.
- Time from the same start event through a verified result, tool/server startup, stalls and retries; map creation counts in C.
- Total solver input/output/reasoning where exposed, all subagents, tool discovery/schemas/results, failed calls/retries and map tokens. Add initial parsing, SIR/embedding generation and ongoing index/map maintenance, including failed generation.
- Cold-start lifecycle totals and amortized totals at 10, 50 and 200 sessions per repository. Cache reads/writes, unreported tokens and provider charges remain separate; missing telemetry cannot be assumed zero. Keep raw receipts.

Precommit a task-level paired analysis and multiplicity correction across correctness and token/time efficiency endpoints. A pilot can estimate variance; it cannot establish a universal effect. Freeze confirmatory sample size using retained power simulations for the chosen strata and correlations, targeting ≥80% power for the effect selected in advance. Do not reuse the reconciliation's mismatched 48/60-task power claim.

Promotion requires either a statistically supported correctness improvement or all of: one-sided 95% correctness lower bound above −5 percentage points against A; no increase in critical regressions; and ≥20% lower lifecycle tokens or end-to-end time, with a 95% interval excluding no improvement. C must also beat its placebo on the claimed endpoint and show no meaningful deficit against the best cheap map. B needs a positive content effect against the null-tool control before tool-schema priming is credited to Aether evidence. Clear evidence of >5-point correctness loss or any reproducible added critical regression blocks default rollout. Otherwise the result is inconclusive.

The offline [result evaluator](../../scripts/benchmarks/README.md) validates complete recorded A/B/C matrices and reports descriptive full-cost totals. It does not run agents, grade patches or establish statistical causality. It charges full map/index costs per applicable run; lifecycle amortization and confirmatory paired inference remain separate analysis.

No execution of this experiment, oracle provider calls, fresh embeddings or paid enrichment is authorized by this plan.

## Delivery and rollout

Land the validated Stage 0 changes as a reviewable correctness slice; keep dependent PR integration and schema/history work separately reviewable. Record compatibility changes, especially duplicate Rust rejection, ranked search semantics and optional MCP layer bodies. Run per-crate checks on every affected crate and feature combination required by repository instructions. Avoid workspace builds.

For each later stage, deliver its fixtures, implementation, measured acceptance output, migration/rollback or regeneration story, and unresolved limitations together. Use disposable copies for migration rehearsals; preserve retained evidence and active sessions. A stage is complete only when its exit evidence exists. The default product remains the existing stack until the corresponding truthful-output, fidelity and agent-benefit criteria justify each behavioral rollout.


## Continuation checkpoints — 2026-10-21

The first stage was audited for unrelated changes and common credential/private-key patterns before commit: 33 files reviewed, no candidates detected, and `git diff --check` passed. Its source hashes matched the retained passing validation manifest. Other worktrees, active stores and sessions were preserved.

Pending PRs were refreshed through GitHub on October 21 and remain unmerged at the heads above. Relative to Stage 0, #223's shared graph file merges textually cleanly; #224 conflicts in `context.rs` (freshness rendering versus response budgeting). #222/#224 also conflict in configuration, agent initialization and template registration. This branch's new map registration adds another router/template integration point; #224 removes the old `TOOL_DESCRIPTIONS` guard that this baseline still uses. Textual compatibility is not behavioral validation. No PR merge, cherry-pick, deployment or push was performed.

### Bounded history and projection implementation

Core schema 22 adds immutable per-incarnation retirement archives and explicitly heuristic same-file/language/kind/name signature lineage. Retirement snapshots original SIR versions, fingerprints, contracts/violations, test intents, drift, quality/audit observations and source edges before removing active rows. Conflicting old and new annotation histories remain separately reachable. Same-ID reintroduction starts without inherited active evidence. Archive insertion and active deletion share one SQLite transaction; repeated retirement is idempotent, including legacy orphan audit rows.

Startup, structural watcher and SIR event processing share the conservative reconciliation rule. Multiple old definitions are retired without guessing which owns a replacement; unchanged overload siblings and other languages cannot inherit the removed definition. Migrated annotations are marked stale. Retired graph nodes/vectors are removed; changed callee identities trigger reconstruction of unchanged callers from retained raw references. Both direct and reverse map queries cover calls, dependencies, type references and implementations. These are name-based candidate relationships, not compiler-resolved exact edges.

MCP timeline exposes retired-ID summaries, lineage and opt-in full per-incarnation evidence. Active version selectors retain their existing meaning. Schema-21 read-only clients expose no retention records and the map reports retention unavailable; only a normal writable open upgrades a disposable test store in this task. Existing unrecorded source/destination revisions remain unknown. There is no history erasure/retention policy, guessed rename/move matching or complete stored edge-resolution contract yet.

### Unified map implementation

One provider-free `aetherd::preflight` service is reused by `aetherd preflight` and `aether_preflight`. It assembles investigation order, both dependency directions, full-message path history, recorded notes, annotation/lineage evidence, uncertainty and tests. Git path history follows renames and includes removed-code commits instead of filtering by blame of surviving lines. Failures differ from genuinely empty path history.

The service opens only SQLite read-only and performs no inference, embedding, reranking, SurrealDB or vector initialization. CLI handles an absent index with source-only exploration without creating `.aether`; existing MCP startup still requires an index, so the tracked command includes the CLI fallback. No branch name is required. The tracked command/template in this isolated worktree now supplies the assembled map before investigation; other generated files and active configurations were not regenerated.

Canonical map provenance includes revision, bounded source/worktree/config/task identity, generator and omissions. The serving limit measures serialized bytes/4, including the SDK result for MCP; it is not an actual model-tokenizer limit. Mandatory freshness and uncertainty statements survive trimming. Completed index/projection generations remain unknown pending the honesty integration; the map cannot be called authoritative or made the default based on these fixtures.

### Identity migration reassessment

The read-only audit at source fingerprint `dae75259594d80b617d2ae06158a5f6f35f40f76ef78c7a6bdd545b66ecdb40d` read 467 Rust files (6,791,752 bytes), parsed 465 and reported 7,184 symbols from parsed files. Two files failed: `aether-core/src/secret.rs` has legal `Debug::fmt` and `Display::fmt` implementations whose current identities collide; `aetherd/src/omp_gateway.rs` has mutually exclusive platform `terminate` definitions. There were no unread files or size-limit omissions. The tool reports only the first extraction error per file, so two files is not an exhaustive collision count.

Explicit rejection is acceptable as a temporary data-loss guard; a whole-scan failure is not acceptable as evidence of general indexing readiness. The existing guard remains visible rather than guessing a definition or silently dropping it. Additional fixtures cover inherent/trait, qualified generic traits and cfg alternatives. This limitation blocks clean self-indexing of this repository.

A safe migration requires a versioned definition identity that includes canonical trait/impl context and an explicit build/cfg policy; a source-location mapping/dry-run; preservation of one-to-many ambiguous legacy observations without arbitrary ownership; compatible caller/type/method resolution and aliases; per-incarnation old-ID history access; disposable upgrade rehearsals, export/backup and rollback tests. Adding a trait string only to declarations would change IDs while losing callers and misassigning existing contracts. The archive stage supplies a foundation, not that migration. The recommended decision is a separately reviewed migration with ambiguous legacy evidence retained as unassigned and compiler/build-context coverage explicitly bounded.

### Evaluation and durability evidence

The offline run separates actual map assembly/fidelity measurements from simulated A/B/C accounting and from retrospective existing live A/B data. Simulated outcome fields cannot establish coding correctness, regressions or savings. Map and maintenance work are charged in both applicable arms; the provider/model paths are disabled, so model-generation time and provider spend are zero for this run. No database-only speed claim follows from process wall timings.

The SQLite-only retirement experiment completed 50 SIGKILLs with 50 observed writer windows, 100 integrity checks and 50 restart convergences. Across post-kill observations, 550 declarations remained fully active and 50 were fully retired; all 600 converged to exactly one retained retirement each after finishing remaining removals. Trigger-widened in-flight windows and 0–2,499 microsecond offsets are reproducible fixtures, not coverage of every statement/crash point. Mirror files, graph/vector stores and providers were excluded. Cross-store structural journaling and recovery remain a separate required stage.

Full Stage 1 is pending PR integration/approval and truthful generation publication. Full Stage 2 remains incomplete until identity/resolution contracts, structural replay, revision provenance and held-out lineage coverage exist. Stage 3's assembly and delivery code is implemented, while held-out fidelity and agent-benefit gates remain open. Stage 4's broad vector/concurrency/performance and provider-fidelity matrix remains unfinished; SQLite crash evidence is a deliberately narrower result.
