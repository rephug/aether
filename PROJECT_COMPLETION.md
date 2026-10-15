# AETHER project completion record

Updated **2026-10-15 UTC**. This is the shared accountability checklist for completing and accepting AETHER's code-intelligence workflow. It records verified candidate behavior and remaining work; it is not a release acceptance certificate.

## How to use this record

- Keep the stable item IDs. Each update must name the accepting worker, exact source revision, dated evidence, and next action.
- **Implemented**, **tested**, **deployed**, and **owner accepted** are separate gates. A passing test does not establish semantic correctness or complete graph coverage.
- A proposed owner is **unassigned** until that worker accepts the item. The roles below do not assign work to another team or the project owner.
- No final owner acceptance is recorded. Deployment, provider submissions, public publication, and downstream account changes require their own authorization; this checklist grants none.
- Evidence IDs refer to retained validation receipts. The validation worker holds the private evidence index with exact paths and hashes. Receipts and source payloads are not published by this document; a reviewer without access cannot independently reproduce those claims from this file alone.
- Downstream projects are called **client A** and **client B** here. Their names, owners, and private integration records are mapped in the private evidence index.

## Evidence boundaries

This document starts from public upstream revision `de702c039b75cdfc3c441152b5fe45c622858776`. E01–E11 below describe isolated candidates, and inclusion of those repairs upstream has **not** been established. E12–E17 (2026-10-15) describe upstream `main` itself, which now integrates the Sprint 1 and overnight repairs (E12), and an end-to-end run on it (E13, E15–E17).

| Evidence | What the retained record establishes | What remains separate |
| --- | --- | --- |
| E01 — frozen small project, 2026-10-13 | Windows and native structural scans of the same small Rust project: 30 symbols, 111 edges, 46 neighbors; its 12 source tests passed. The initial scan contained no SIR/history. | This is one complete small-project fixture, not a scan of every supported language or a large repository. |
| E02 — graph provenance repair, 2026-10-13 | Isolated commit `e87f7971f855fb15b604a295b4c8601fcf12f6df` recognizes indexed file-import sources. Five falsely invalid sources became zero; 111 edges and five imports remained. Native gates: 186 unit and five integration tests; 132 MCP calls on each platform. | Changed-file hashes were reconciled; native and Windows full-tree identity was not asserted. Unresolved relationships remain. |
| E03 — real SIR generation and review, 2026-10-13 | 30 retained model-generated annotations cover all 30 indexed symbols: 16 production symbols and 14 tests/helpers. Source review found no material contradiction in the reviewed set. | Model confidence is not calibrated accuracy. Parsing and source binding were separate checks; the normalizer helper alone did not establish semantic grounding. |
| E04 — storage and restart, 2026-10-13 | Actual MCP import/retrieval left 30 SIR and 30 history rows in validation clones; all 30 were fresh with exact canonical hashes after restart on Windows and native execution. The previous 25 records/history were preserved. | Native source-revision rows were recorded/count-checked rather than fully compared. Populated `method_dependencies` was outside the injection schema exercised. |
| E05 — navigation repair, 2026-10-13 | An isolated extension patch selected declarations through the document-symbol provider; 19 consumer checks with a mocked VS Code API and a strict TypeScript check passed. | No real VS Code extension-host navigation acceptance; no installed Rust document-symbol provider was established in the checked environment. |
| E06 — dependency/test retrieval repair, 2026-10-14 | Actual MCP comparison: 47 before, 48 after, 31 after restart requests (126 total, including initialization). All 30 SIRs stayed fresh in each process. Twelve context regressions, per-crate clippy, formatting, and offline MCP build passed. | Patch remains separate from upstream integration. Eight selected contexts were examined; the graph still contains unresolved edges. |
| E07 — retrieval preservation, 2026-10-14 | Thirty SIR/history rows and ten compared data tables retained their contents; clone read telemetry was excluded explicitly. The original fixture was preserved including its telemetry. | Read telemetry changes are expected in the retrieval clone. This does not qualify every write, migration, or recovery path. |
| E08 — editor environment inventory, 2026-10-14 | VS Code was present, but its extension inventory showed no Rust provider; a command-line shim is not an installed editor provider. No host acceptance run or installation occurred. | An authorized, working editor host/provider is still needed. |
| E09 — prior integration work | Separate integration candidates and owner handoffs exist for downstream clients. | Their presence does not prove current repair inclusion, deployment, or end-user acceptance. |
| E10 — provenance and publication check, 2026-10-14 | Repository origin and connected GitHub owner match; the repository is public. The owner separately authorized publication of this sanitized checklist on an isolated docs branch. | Publication of this record does not publish private receipts, deploy repairs, or establish release acceptance. |
| E11 — earlier refresh/cache candidates, 2026-10-07–08 | Retained hover-race/staleness and integration-cache repair records exist. The hover candidate reports 39 regressions and five recorded-response replays. | Those results belong to earlier source/extension bytes; the complete unified-release change matrix and actual current-host behavior remain unqualified. |
| E12 — upstream integration, 2026-10-15 | Integration PRs #202 (merge `e5aacb7`), #205 (`9274d1e`) and #208 (`1bf1cea`) landed 27 PRs on `main`, and #209 (`6299e08`) added Python call-edge resolution. They covered dashboard, VS Code, contract and memory fixes; Java and C# parsing; overload-aware MCP callers; a regenerate guard; `include_inactive`; a shared SurrealDB handle; and a configurable SIR timeout. Each merged with a merge commit after all 40 CI checks passed on code that already contained current `main`. `main`'s tree equals each tested head, and push CI on every merge commit passed. | CI establishes build and test status for this revision, not semantic correctness, deployment or release acceptance. |
| E13 — real local-model SIR on a public Python project, 2026-10-15 | The public python-slugify project (commit `54d81f6`, 13 files) was scanned with `main`'s code: 175 symbols, 598 call and 77 import edges. A local model (phi4-mini on Ollama, provider `qwen3_local`; no source left the machine) generated SIR for 154 of 175 symbols (88%) plus 13 file rollups. All 188 rows record that provider and model, none are mock, and each SIR is bound to the source commit. MCP status, search, SIR, timeline and test-intent reads, and dashboard overview, symbol, search and health reads, returned the stored data. | 21 large symbols, mostly big test classes, still exceed the generation limit on CPU. SIR quality was spot-checked, not reviewed against a rubric. No Python call edge resolved on that revision (see T21 and #209). |
| E14 — Java and C# naming review, 2026-10-15 | Six documented naming and recursion compromises, plus a completeness sweep, were assessed with adversarial checks. One correctness blocker (overload reconciliation moving a deleted overload's SIR and contracts onto its sibling) and one consumer issue (usage-matrix bare-name callers) were fixed before merge, with tests. Twelve items are tracked below as T01–T12. | The classification is review judgment on traced scenarios, not exhaustive testing. |
| E15 — watched incremental change, 2026-10-15 | With the daemon watching the E13 project, `_modern_truncate` was edited twice. After the first edit, the generation attempt timed out. The old SIR was then labeled `stale` with the error, in the store, in MCP `aether_get_sir` and in the dashboard; nothing new was presented as fresh. After the second edit, the watcher regenerated it as version 2 (`qwen3_local`/phi4-mini). Version 2 drops version 1's now-wrong error mode and names invalid input types. `aether_symbol_timeline` lists both versions and `aether_why_changed` reports the modified fields. Every other baseline symbol SIR is byte-identical; only file rollups of files with new SIRs were recomputed. | The edits were uncommitted, so both versions record the base commit. The refreshed SIR does not name the new negative-length `ValueError`, a limit of the small local model. Delete, rename and restart cases were not run. |
| E16 — daemon with dashboard, 2026-10-15 | Before #206, `aetherd --workspace .` with the dashboard exited at startup on a SurrealKV lock held by its own dashboard thread, reproduced on the E13 project. Fixed by #206 (tests that fail without it). On `main` the daemon and dashboard run together on that project. Before #207, 29 symbols failed on a fixed 90 s attempt limit; with `[inference] timeout_secs = 400` the project's main `slugify` function and seven others gained SIR. | Generation length varies on CPU. One symbol still exceeded three 400 s attempts (see T22). |
| E17 — Python dependencies on `main`, 2026-10-15 | With `main` at `6299e08` (#209), a structural index of the E13 project stores 214 call edges, none of them raw expression text; 65 resolve to symbols, up from 0. MCP `aether_dependencies` lists `_modern_slugify` as the caller of `_modern_truncate`. `aether_call_chain` from `slugify` reaches `_modern_slugify` and `_legacy::slugify` at depth 1, the decode, truncate and transliterate helpers at depth 2, and `_numeric_reference` at depth 3. | Package re-exports and other #209 known limits keep some calls unresolved (T21); the 81 test calls to `slugify` via the package re-export are among them. |
| E18 — same-project model comparison, 2026-10-15 | On the E13 project (python-slugify `54d81f6`, 175 symbols), `main` at `beb5ab8` (after #210) generated SIR for 175/175 symbols with 0 failed calls under four configurations. Each used Aether's `openai_compat` provider through a local bridge to the vendors' own CLIs on subscription logins: GPT-6 Luna (Codex) and Claude Haiku 5.5 (Claude Code), each at low and max effort. Results: (1) 40 gold fact sheets were graded blind by two judges from different vendors (Opus 5.5 and GPT-6.1 Sol). Gold-fact completeness was Haiku max 0.74, Haiku low 0.62, Luna low and max 0.50 each; the paired 95% interval for Haiku max minus Haiku low is +0.12 [0.08, 0.16]. (2) Haiku low made the most major errors (0.46 per SIR). The lower judged rate for Luna versus Haiku max (0.08–0.09 vs 0.20) is within noise after adjudication. (3) Median seconds per call: Luna 6.2 (low) and 7.4 (max), Haiku 7.6 (low) and 35.9 (max). API-list-price equivalent per project: $0.085, $0.096, $0.11 and $0.84 respectively. (4) In an agent A/B (6 since-fixed upstream bugs re-seeded plus 2 understanding questions; 4 agents × 2 arms × 2 reps = 128 runs), the arm with an Aether MCP server passed 54/64 and the arm without passed 58/64 (Fisher p = 0.42). Agents called Aether in 22 of the 64 runs that had it, and attaching it added about 12k input tokens to every Claude Code request. | One small Python project and small single-file tasks; the A/B detects only large effects. Claude Code agents were limited to pytest and read-only git, so compare arms within one agent only. Server load varied. The gold sheets were written by a Claude model. Details and caveats are in the comparison report held with the private evidence. |

The frozen small-project source SHA-256 is `1bb7459e84e0311fea5527e08d9ac52f2c7211550e86aadae747ab362274df16`. These are evidence-specific observations, not speed, token, or accuracy improvements over a matched baseline.

## Completion checklist

### A01 — One traceable release candidate

- **Status:** Partial. Upstream integration is done (2026-10-15): `main` contains the Sprint 1 and overnight repairs and passes CI (E12). A release manifest, binaries and acceptance remain open.
- **Responsible worker / proposed owner:** Validation worker retains receipts; proposed release integrator **unassigned**.
- **Next concrete step:** Compare the isolated E02, E05 and E06 candidates with current `main` and record which are superseded. Then cut a release from `main` with a manifest of commit, binaries, configuration and receipts (T19).
- **Blocker:** There is no release package or manifest yet, and the isolated candidates have not been compared with `main`.
- **Objective done criterion:** One manifest binds the chosen commit, all applied patches, configuration, binaries, and test receipts; a clean checkout reproduces the applicable gates. Any platform differences are explicit.
- **Evidence:** E02, E05, E06, E10, E12. The MCP patch is based on the isolated graph commit; the extension repair is a separate candidate.

### A02 — Complete project scans

- **Status:** Tested for the frozen small Rust project (E01) and, on upstream `main`, for a complete public 13-file Python project (E13: 175 symbols). Broader project acceptance is open.
- **Responsible worker / proposed owner:** Validation worker completed the small-project lane; proposed scan qualification owner **unassigned**.
- **Next concrete step:** Agree representative multi-file projects and supported-language expectations, then compare clean scans with an independent file/symbol inventory.
- **Blocker:** A 30-symbol Rust fixture does not establish large-project, multi-language, ignored-file, or partial-failure coverage.
- **Objective done criterion:** Each agreed project has accounted-for files/symbols, explicit exclusions and parse failures, verified source revisions, and matching repeated-scan results; partial scans report incompleteness.
- **Evidence:** E01, E02, E13. Thirty symbols and 111 extracted edges do not mean every relationship was resolved. In E13, no Python call edge resolved before #209.

### A03 — Real model SIR and source-informed review

- **Status:** Real generation, parsing, binding and review shown for 30 symbols (E03). On upstream `main`, a local model generated SIR for 154 of 175 symbols of a public Python project, with provider, model and commit recorded (E13, E16). General semantic accuracy remains open.
- **Responsible worker / proposed owner:** Validation worker retains the completed corpus; proposed semantic-review owner **unassigned**.
- **Next concrete step:** Reuse the retained corpus to define a source-grounded rubric for behavior, side effects, errors, dependencies, and uncertainty; identify uncovered cases before any separately authorized generation.
- **Blocker:** One reviewed corpus is insufficient for a calibrated accuracy claim, and model self-confidence is not an independent score.
- **Objective done criterion:** Every accepted annotation binds to source/configuration provenance and passes the agreed independent review rubric; unsupported assertions and ambiguous behavior are recorded rather than silently accepted.
- **Evidence:** E03, E13, E16. Leaf canonicalization sorts lists; file canonicalization additionally deduplicates its lists. Tests for list ordering alone do not establish duplicate semantics.

### A04 — Persistence, history, and restart

- **Status:** Thirty-record persistence and restart verified on two platforms; broader recovery and schema qualification open.
- **Responsible worker / proposed owner:** Validation worker and native coordinator completed retained replays; proposed storage qualification owner **unassigned**.
- **Next concrete step:** Qualify update history, interrupted writes, recovery, and all supported annotation fields on the unified candidate, including populated `method_dependencies` if exposed by the injection contract.
- **Blocker:** Current injection evidence does not exercise populated method dependencies or every migration/failure path.
- **Objective done criterion:** Accepted writes survive restart with exact canonical content and history; rejected or interrupted writes preserve prior accepted state; provenance rows and supported fields are fully compared.
- **Evidence:** E04, E07. Native replay preserved the original 25-record fixture and imported five replacements into a clone.

### A05 — Dependency retrieval and honest graph coverage

- **Status:** Concrete MCP context omission repaired and tested in an isolated candidate; graph completeness remains open.
- **Responsible worker / proposed owner:** Validation worker completed the repair; proposed graph/MCP integrator **unassigned**.
- **Next concrete step:** Integrate the explicit edge-kind lookup and qualify multi-file calls, type references, imports, ambiguity, and depth limits against source-reviewed expected relationships.
- **Blocker:** The frozen graph has 23 resolved and 88 unresolved edges; extracted relationships cannot be presented as a complete call graph. On `main` before #209, no Python call edge resolved (E13). Since #209 (`6299e08`), Python call edges resolve; 65 of 214 in the E13 project, with the remaining gaps tracked as T21 (E17). Java and C# are merged with naming compromises tracked as T01–T12 (E14).
- **Objective done criterion:** Supported edge kinds appear at each requested depth with correct provenance and labels; unresolved, omitted, truncated, or unsupported relationships remain visible and never become a claim of no dependencies.
- **Evidence:** E02, E06. `sir_hash` context changed from zero displayed outgoing entries to `SirAnnotation [type_ref]` and `canonicalize_sir_json [calls]`; depth two reaches `normalize_complexity_label`.

### A06 — Relevant test retrieval

- **Status:** Direct-test prioritization, truncation labels, and provenance guard implemented and tested in the MCP candidate.
- **Responsible worker / proposed owner:** Validation worker completed the repair; proposed test-retrieval integrator **unassigned**.
- **Next concrete step:** Integrate the repair and qualify multiple files, duplicate names, missing graph links, and bounded result sets.
- **Blocker:** Graph-linked and same-file test candidates establish relevance, not execution coverage or assertion adequacy.
- **Objective done criterion:** Resolved direct tests rank ahead of file fallbacks; every row identifies its selection basis; limits disclose omitted counts; foreign-source or ambiguous evidence cannot masquerade as a direct test.
- **Evidence:** E06. The direct hash-stability test now appears first; output states five of 12 candidates shown and seven omitted. The source-file mismatch regression was fixed and retained.

### A07 — Definition navigation implementation

- **Status:** Isolated repair and mocked-consumer validation complete; integration and real-host acceptance open.
- **Responsible worker / proposed owner:** Validation worker prepared the patch; proposed editor integrator **unassigned**.
- **Next concrete step:** Apply the reviewed extension patch to the chosen release candidate and preserve its qualification, ambiguity, stale-document, and range checks.
- **Blocker:** The extension patch is separate from both upstream and the current MCP repair.
- **Objective done criterion:** Navigation chooses the unique matching declaration from document symbols; absent, failed, ambiguous, or stale results give an actionable message without a misleading call-site jump.
- **Evidence:** E05. Frozen-source examples corrected `sort_and_dedupe` from call-site line 135 to declaration 198 and `normalize_complexity_label` from 65 to 186; 19 mocked-API checks passed.

### A08 — Real editor-host acceptance

- **Status:** Blocked on an available, authorized Rust document-symbol provider and editor host; no real-host pass claimed.
- **Responsible worker / proposed owner:** Proposed editor acceptance worker **unassigned**; environment owner approves any required setup separately.
- **Next concrete step:** Establish the intended installed editor/provider versions, then run navigation and hover interactions on the unified candidate in that actual host.
- **Blocker:** Existing inventory did not establish an installed Rust provider or reusable extension-test host.
- **Objective done criterion:** Recorded user interactions land on the correct definitions and source-bound explanations; edits, duplicate names, provider absence/failure, and stale ranges behave correctly in the real host, with versioned evidence.
- **Evidence:** E05, E08. A standalone walkthrough and mocked API are useful validation artifacts but do not close this gate.

### A09 — Incremental refresh and stale-result handling

- **Status:** Edit-and-refresh shown on upstream `main` (E15): a failed refresh labeled the old SIR stale; a successful one produced a new version with history and a field-level change explanation; unaffected SIRs stayed identical. The full change and restart matrix remains open.
- **Responsible worker / proposed owner:** Proposed indexing/cache qualification owner **unassigned**.
- **Next concrete step:** Extend E15 to add, delete, rename, import change, interrupted refresh and restart, against the selected release revision.
- **Blocker:** Fresh retrieval from an unchanged fixture does not prove correct invalidation after a source change.
- **Objective done criterion:** Each change updates affected symbols, graph edges, and revisions; obsolete identities disappear; stale SIR is clearly withheld or labeled until regenerated; unaffected entries remain valid; restart preserves those decisions.
- **Evidence:** E04 and E07 establish unchanged-source freshness and preservation only. E15 covers edits on `main`. E11's earlier repair receipts must be reconciled to A01 before extending this status.

### A10 — Change classification and explanations

- **Status:** Qualification of the unified candidate remains open.
- **Responsible worker / proposed owner:** Proposed classifier/semantic-review owner **unassigned**.
- **Next concrete step:** Freeze source-reviewed cases for formatting, comments, signature/API changes, behavior, dependencies, and mixed edits; distinguish structural signals from semantic conclusions.
- **Blocker:** Hash stability, successful parsing, and plausible text do not establish behavioral equivalence or a correct explanation of a change.
- **Objective done criterion:** Expected classifications match reviewed cases with explicit uncertainty for insufficient evidence; explanations cite the correct before/after revisions and do not fabricate causality from stale annotations.
- **Evidence:** E03, E04, E07 establish source-bound records and preservation, not a completed change-classification acceptance matrix.

### A11 — Project isolation, provenance, and MCP trust boundaries

- **Status:** Targeted provenance/negative checks passed; full release qualification remains open.
- **Responsible worker / proposed owner:** Proposed storage/MCP boundary owner **unassigned**.
- **Next concrete step:** Reconcile retained guards, then qualify foreign-project identities, path escapes, links, malformed annotations, concurrent updates, and tool error behavior on the unified candidate.
- **Blocker:** Tested guards cover specific paths; no complete trust-boundary audit of the selected release is asserted.
- **Objective done criterion:** Cross-project or escaped reads/writes are rejected before mutation; retrieved source/annotation/edge identities agree; untrusted source text stays data; partial failures surface without plausible fabricated results or damaged prior state.
- **Evidence:** E02 records indexed-file provenance and negative controls; E06 includes the test-caller source guard; E07 records state preservation. Native-specific path tests do not by themselves qualify every platform. E12 adds, on `main`: dashboard cross-origin write rejection (#181), escaping in wizard scripts (#186), a hardened enhance subprocess (#193) and path containment for MCP SIR/source reads (#165).

### A12 — Downstream client A integration

- **Status:** Separate integration work exists; current combined repair acceptance is unverified.
- **Responsible worker / proposed owner:** Existing integration lane retains ownership of its candidate; proposed current-release acceptance owner **unassigned** pending that owner's acknowledgment.
- **Next concrete step:** Reconcile its source/launcher pins with A01 and run the agreed complete user workflow through the actual client.
- **Blocker:** Candidate preparation and earlier trials do not establish the current repaired engine's installed behavior.
- **Objective done criterion:** The client selects the intended project/store, retrieves fresh source-bound context and relevant tests/dependencies, navigates correctly, reports unavailable/stale results, and has an accepted rollback path on the pinned release.
- **Evidence:** E09; exact client identity and candidate records are in the private evidence index.

### A13 — Downstream client B integration

- **Status:** Owner handoff exists; current end-to-end acceptance remains open.
- **Responsible worker / proposed owner:** Proposed client integration owner **unassigned**, to be confirmed through the retained owner handoff.
- **Next concrete step:** Confirm the adapter/tool contract, selected project, and actual agent workflow, then bind an isolated end-to-end run to A01.
- **Blocker:** A handoff is not proof of adapter installation, correct tool use, or accepted production behavior.
- **Objective done criterion:** A recorded workflow requests the intended AETHER tools, consumes fresh provenance-bearing results, handles errors and incomplete graphs, and produces source-checked outcomes under the client's agreed permissions.
- **Evidence:** E09; exact client identity and handoff are in the private evidence index.

### A14 — Measured speed, token use, and accuracy comparison

- **Status:** E18 now covers this for one small Python project. It gives paired SIR quality, latency, token and cost figures for GPT-6 Luna and Claude Haiku 5.5 at their lowest and highest effort, and an agent A/B that shows no detectable gain from Aether on small single-file tasks. No speed, token or accuracy gain from Aether is claimed. A matched comparison on a larger multi-module codebase remains open.
- **Responsible worker / proposed owner:** Proposed comparison owner **unassigned**; project owner agrees the rubric and budget separately.
- **Next concrete step:** Freeze paired tasks, source revisions, model/settings, access permissions, and scoring rubric; reuse eligible retained results and identify the minimum missing authorized runs.
- **Blocker:** Unpaired timings, estimated context budgets, and model confidence do not demonstrate comparative benefit.
- **Objective done criterion:** Report all paired outcomes, actual token counts separately from estimates, latency distributions and failures, source-reviewed accuracy with sample counts, and annotation-generation cost/amortization. Record automatic continuations and exclude them from the primary comparison unless the protocol explicitly includes them.
- **Evidence:** E01 and E06 contain validation timings/requests only. E18 holds the paired single-project comparison and its limits. Existing generation and restart results support correctness checks within their scope, not a comparative improvement claim.

### A15 — Packaging, deployment, and rollback

- **Status:** Candidate binaries and patches exist; deployment and operational acceptance are open.
- **Responsible worker / proposed owner:** Proposed release/deployment owner **unassigned**; environment owner supplies deployment authorization.
- **Next concrete step:** Build the release manifest from A01, publish release binaries (T19), document install/update/rollback steps, and validate the package in an isolated representative environment.
- **Blocker:** There is no single accepted release package with deployment and rollback evidence for the requested clients.
- **Objective done criterion:** Package contents match the accepted source/build manifest; an authorized deployment passes agreed checks and rollback restores a compatible prior state without losing accepted history.
- **Evidence:** E02, E05, E06 establish candidate artifacts only. This documentation change performs no deployment.

### A16 — Owner acceptance

- **Status:** Open; no final owner acceptance recorded.
- **Responsible worker / proposed owner:** Project owner makes the acceptance decision; proposed acceptance coordinator **unassigned**.
- **Next concrete step:** Agree required use cases and thresholds, review the exact candidate with the owner, and resolve or explicitly accept remaining limitations.
- **Blocker:** Unified release, real-host/client validation, comparison requirements, and release evidence remain incomplete.
- **Objective done criterion:** A dated owner decision names the accepted commit/package, demonstrated workflows, agreed thresholds, unresolved limitations, and responsible follow-up owners. Test-suite success alone cannot close the item.
- **Evidence:** A01–A15 and their receipts. No inference of acceptance is made from task authorization or test passes.

### A17 — Reviewable completion record and publication

- **Status:** Sanitized docs candidate committed; public publication authorized on 2026-10-14; updated 2026-10-15 with E12–E16 and the tracked improvements below. Review remains open.
- **Responsible worker / proposed owner:** Validation worker prepares this record; proposed public-review owner **unassigned**.
- **Next concrete step:** Review the sanitized docs branch and its draft PR; use the separately retained private evidence index to assess supporting claims.
- **Blocker:** No publication-authorization blocker remains. Reviewer acceptance and the release gates above remain open.
- **Objective done criterion:** Every item has an accepted owner or explicit unassigned status, next step, blocker, objective criterion, and accessible evidence for its reviewer; any authorized public revision contains no private payloads, infrastructure details, or client identities.
- **Evidence:** E10; local commit and validation receipts are retained separately from this public-safe document.

## Tracked improvements

Non-blocking work found while integrating and reviewing the Java and C# parsers (E14) and running the end-to-end demo (E13, E15, E16). None of it produces a crash or data loss. Most items lose information (an edge or caller is missing); the rest can add a false edge in specific, named situations. Each is small or medium and independent of the others. Owners are **unassigned**.

### Java and C# naming

- **T01 — Package-aware caller narrowing.** `aether_dependencies` drops callers whose target name matches several symbols. Its package narrowing knows `Cargo.toml`, `package.json` and `pyproject.toml`, but not `pom.xml`, `build.gradle(.kts)` or `*.csproj`. Done when a two-module Maven fixture and a two-project .NET fixture attribute each caller to its own module's class.
- **T02 — Language-family edge resolution.** Edges resolve by exact qualified name across languages. A Java call spelled `UserService::getUser` can also resolve to a TypeScript class with the same name. Done when every qualified-name join also matches the language family (TypeScript and JavaScript count as one family), with a Java and TypeScript fixture.
- **T03 — Ambiguity-aware single lookups.** Several context paths pick one symbol with `ORDER BY id LIMIT 1` when a qualified name is shared. Done when they prefer a same-file, then same-package candidate and otherwise say the name is ambiguous.
- **T04 — C# `using static` and alias segments.** A `using static A.B.Type;` adds `Type` to the namespace set, so `Type.Nested` is spelled `Nested`. Done when type segments of static and alias usings are left out, with fixture lines.
- **T05 — Java inherited members.** Unqualified calls to inherited methods are spelled with the calling class, and `toString`/`equals`/`hashCode` can resolve past the innermost class. Done when names every class inherits stay on the innermost class, and calls to methods declared by a superclass in the same file are spelled with that class.
- **T06 — Recursion keeps its self-edge.** Unqualified and `this.` calls to a method's own name get no edge, so overloads that call each other don't form a false cycle. Moving that rule into graph resolution would give recursion its self-edge back. Rust trait impls that share a name (`impl Display`/`impl Debug` both giving `Type::fmt`) can still form the false cycle today. Done when sibling overloads resolve to a self-loop in graph sync and the parser rule is removed.
- **T07 — Direct children only.** `child_method_symbols` counts `Outer::Inner::method` as a method of `Outer`, so usage matrices, trait-split suggestions and class dependency aggregation include nested types' methods. Done when only direct children are returned, matching the guard SIR method dependencies already use.
- **T08 — Unknown receivers.** Java and C# fall back to a bare method name when the receiver's type is unknown, as Rust does. In C# a bare name can only match a same-named top-level type, and in mixed repositories it can match another language's top-level function. Done when calls with an unknown owner get no edge, or a spelling no symbol carries, and fixtures are updated.
- **T09 — Library types in call edges.** The `java.lang` and BCL skip lists apply to type references but not to `new X()` or `X.m()` calls. Done when unshadowed library names are spelled by their full name in call edges too.
- **T10 — Class-qualified self calls.** `Type.method(...)` inside `Type` bypasses the self-call rule. Done when a type-name receiver equal to the enclosing type counts as an own-type call.
- **T11 — C# generic arity.** `Result` and `Result<T>` share a qualified name, so data for the two merges in MCP views. Done when the arity is part of C# type names at declaration and reference.
- **T12 — Smaller parser gaps.** C# local functions are spelled as methods of the enclosing class. Java `method_owner` ignores methods an anonymous class declares itself. Java receivers typed by a type parameter are spelled as types. C# `#if`/`#else` duplicate declarations and same-file partial parts get identical ids. Each has a one-line fix and a fixture.

### Carried over from the overnight handoff

- **T13 — Ambiguous-selector errors list ids.** The shared resolver should list candidate symbol ids.
- **T14 — Dashboard architecture "Directory view" is a no-op.**
- **T15 — VS Code CLI fallback misses `--` before the prompt.**
- **T16 — Causal analyzer mixes seconds and milliseconds outside the day window.**
- **T17 — Dashboard has no catch-panic layer.**
- **T18 — Test heuristics.** The per-file community threshold still counts tests, and three test heuristics should be unified.
- **T19 — CI and releases.** Measure the first CI `aetherd` build and publish release binaries.
- **T20 — Prompt-hash segment width; cleanup across stores isn't transactional; the QUICKSTART "planned" note should go now that the WF commands ship.**

### Found in the end-to-end run

- **T21 — Python call edges (core fix merged via #209, `6299e08`).** Before #209, no Python call edge resolved, so callers, dependencies, call chains, health and blast radius were empty for Python projects. #209 spells Python call targets like Python symbol names: 0 → 64,943 resolved calls on a 4,534-file corpus, and 0 → 58 on the E13 project. Its known limits remain open: package re-exports, `src/` layouts seen from outside `src/`, inherited methods, and cross-language name collisions (T02).
- **T22 — Slow local generation.** On CPU, some symbols still exceed three attempts even at 400 s, and generation length varies between runs for the same symbol. Capping generated tokens for local JSON generation (Ollama `num_predict`) would bound each attempt. Done when the E13 project reaches full coverage on the same machine.
- **T23 — Dashboard coverage counts rollups.** The dashboard overview reports file rollups as SIR coverage (95.4% where symbol coverage is 88%). Done when it reports symbol coverage like `aetherd status`.
- **T24 — Python test layouts in `is_test_symbol`.** Some common Python test layouts are not recognized, so their helpers can join production communities now that Python edges resolve. To be done together with T18.
- **T25 — Batch runs after #209.** `aetherd batch run` compares full prompt hashes, including neighbor context. Its first run after #209 regenerates Python symbols whose callers or callees newly resolve. This is expected and one-time; worth a release note.

- **T28 — SurrealDB open timeout under load.** The graph store gives each open 5 s and does not retry a timeout. On a busy machine (load around 26 on 20 cores while a local model ran), a fresh structural index failed with `SurrealDB open timed out`, and the same command passed on retry. Tests that open SurrealDB fail the same way under load. Done when the open retries timeouts with backoff, or allows a longer configurable limit.

Fixed during the run: T26, the daemon-with-dashboard startup lock (#206), and T27, the configurable SIR attempt timeout (#207). Both are merged via #208.

### Found in the model comparison (E18)

- **T29 — `--index-once` serves stale SIR.** Batch-mode `aetherd --index-once` (with or without `--full`) kept an edited symbol's old SIR marked fresh. Plain mode did not prune deleted symbols, and file rollups were never refreshed. Open as #215, with daemon startup and structural index-once following in #216 (stacked on #215).
- **T30 — Function-local class methods.** A class declared inside a function keeps the function in its name, but its methods drop it (`module::Class::method`). Fixing this changes those symbol ids and #209's resolver guard, so it needs a note about the one-time SIR churn, like T25.
- **T31 — MCP tool surface cost for agents.** Attaching Aether's MCP server (39 tools) adds about 12k input tokens to every Claude Code request, even when no Aether tool is called. Done when agents can attach a compact profile with the few tools they actually used (dependencies, symbol lookup, SIR context).
- **T32 — SIR truncation of large symbols.** Symbols longer than 10,000 characters are truncated before SIR generation; two large test classes in E13 were cut. Done when large symbols are summarised in parts, or the limit and its effect are reported.
- **T33 — Quality-pass HTTP timeout.** Triage and deep passes set their own attempt timeouts, but OpenAI-compatible requests still use the inference request timeout (raised only by `[inference] timeout_secs`). Done when each pass's request timeout follows its own attempt timeout.
- **T34 — Graph store setup race.** Opens sharing a cached SurrealKV handle could set up a new store at the same time and fail with a transaction write conflict. This made a #206 test flaky in CI. Open as #219.

Open pull requests as of 2026-10-15, all reviewed with no blocker found and none merged: #211 (T22), #212 (`openai_compat` thinking warning and summary path), #213 (T28), #214 (T01–T03), #215 and #216 (T29), #217 (T21 re-exports and `src/` layouts), #218 (T18 and T24), #219 (T34).

