# AETHER project completion record

Updated **2026-10-14 UTC**. This is the shared accountability checklist for completing and accepting AETHER's code-intelligence workflow. It records verified candidate behavior and remaining work; it is not a release acceptance certificate.

## How to use this record

- Keep the stable item IDs. Each update must name the accepting worker, exact source revision, dated evidence, and next action.
- **Implemented**, **tested**, **deployed**, and **owner accepted** are separate gates. A passing test does not establish semantic correctness or complete graph coverage.
- A proposed owner is **unassigned** until that worker accepts the item. The roles below do not assign work to another team or the project owner.
- No final owner acceptance is recorded. Deployment, provider submissions, public publication, and downstream account changes require their own authorization; this checklist grants none.
- Evidence IDs refer to retained validation receipts. The validation worker holds the private evidence index with exact paths and hashes. Receipts and source payloads are not published by this document; a reviewer without access cannot independently reproduce those claims from this file alone.
- Downstream projects are called **client A** and **client B** here. Their names, owners, and private integration records are mapped in the private evidence index.

## Evidence boundaries

This document starts from public upstream revision `de702c039b75cdfc3c441152b5fe45c622858776`. Validation described below belongs to isolated candidates. Inclusion of those repairs in this upstream revision has **not** been established.

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

The frozen small-project source SHA-256 is `1bb7459e84e0311fea5527e08d9ac52f2c7211550e86aadae747ab362274df16`. These are evidence-specific observations, not speed, token, or accuracy improvements over a matched baseline.

## Completion checklist

### A01 — One traceable release candidate

- **Status:** Partial. Candidate provenance is retained; upstream integration and a single accepted release revision remain open.
- **Responsible worker / proposed owner:** Validation worker retains receipts; proposed release integrator **unassigned**.
- **Next concrete step:** Reconcile the graph commit, MCP context patch, and extension patch against the intended upstream revision; record conflicts and the resulting commit and binary hashes.
- **Blocker:** The tested pieces have distinct source/build histories and are not one demonstrated upstream release.
- **Objective done criterion:** One manifest binds the chosen commit, all applied patches, configuration, binaries, and test receipts; a clean checkout reproduces the applicable gates. Any platform differences are explicit.
- **Evidence:** E02, E05, E06, E10. The MCP patch is based on the isolated graph commit; the extension repair is a separate candidate.

### A02 — Complete project scans

- **Status:** Tested for the frozen small project; broader project acceptance open.
- **Responsible worker / proposed owner:** Validation worker completed the small-project lane; proposed scan qualification owner **unassigned**.
- **Next concrete step:** Agree representative multi-file projects and supported-language expectations, then compare clean scans with an independent file/symbol inventory.
- **Blocker:** A 30-symbol Rust fixture does not establish large-project, multi-language, ignored-file, or partial-failure coverage.
- **Objective done criterion:** Each agreed project has accounted-for files/symbols, explicit exclusions and parse failures, verified source revisions, and matching repeated-scan results; partial scans report incompleteness.
- **Evidence:** E01, E02. Thirty symbols and 111 extracted edges do not mean every relationship was resolved.

### A03 — Real model SIR and source-informed review

- **Status:** Real generation, parsing, binding, and review demonstrated for 30 symbols; general semantic accuracy remains open.
- **Responsible worker / proposed owner:** Validation worker retains the completed corpus; proposed semantic-review owner **unassigned**.
- **Next concrete step:** Reuse the retained corpus to define a source-grounded rubric for behavior, side effects, errors, dependencies, and uncertainty; identify uncovered cases before any separately authorized generation.
- **Blocker:** One reviewed corpus is insufficient for a calibrated accuracy claim, and model self-confidence is not an independent score.
- **Objective done criterion:** Every accepted annotation binds to source/configuration provenance and passes the agreed independent review rubric; unsupported assertions and ambiguous behavior are recorded rather than silently accepted.
- **Evidence:** E03. Leaf canonicalization sorts lists; file canonicalization additionally deduplicates its lists. Tests for list ordering alone do not establish duplicate semantics.

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
- **Blocker:** The frozen graph has 23 resolved and 88 unresolved edges; extracted relationships cannot be presented as a complete call graph.
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

- **Status:** Prior repair evidence exists; the unified release's complete change/restart matrix is not established here.
- **Responsible worker / proposed owner:** Proposed indexing/cache qualification owner **unassigned**.
- **Next concrete step:** Run an isolated matrix of edit, add, delete, rename, import change, interrupted refresh, and restart against the selected release revision.
- **Blocker:** Fresh retrieval from an unchanged fixture does not prove correct invalidation after a source change.
- **Objective done criterion:** Each change updates affected symbols, graph edges, and revisions; obsolete identities disappear; stale SIR is clearly withheld or labeled until regenerated; unaffected entries remain valid; restart preserves those decisions.
- **Evidence:** E04 and E07 establish unchanged-source freshness/preservation only. E11's earlier repair receipts must be reconciled to A01 before extending this status.

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
- **Evidence:** E02 records indexed-file provenance and negative controls; E06 includes the test-caller source guard; E07 records state preservation. Native-specific path tests do not by themselves qualify every platform.

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

- **Status:** No speed, token, or accuracy gain is claimed by this record; a matched comparison remains open.
- **Responsible worker / proposed owner:** Proposed comparison owner **unassigned**; project owner agrees the rubric and budget separately.
- **Next concrete step:** Freeze paired tasks, source revisions, model/settings, access permissions, and scoring rubric; reuse eligible retained results and identify the minimum missing authorized runs.
- **Blocker:** Unpaired timings, estimated context budgets, and model confidence do not demonstrate comparative benefit.
- **Objective done criterion:** Report all paired outcomes, actual token counts separately from estimates, latency distributions and failures, source-reviewed accuracy with sample counts, and annotation-generation cost/amortization. Record automatic continuations and exclude them from the primary comparison unless the protocol explicitly includes them.
- **Evidence:** E01 and E06 contain validation timings/requests only. Existing generation and restart results support correctness checks within their scope, not a comparative improvement claim.

### A15 — Packaging, deployment, and rollback

- **Status:** Candidate binaries and patches exist; deployment and operational acceptance are open.
- **Responsible worker / proposed owner:** Proposed release/deployment owner **unassigned**; environment owner supplies deployment authorization.
- **Next concrete step:** Build the release manifest from A01, document install/update/rollback steps, and validate the package in an isolated representative environment.
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

- **Status:** Sanitized docs candidate committed; public publication authorized on 2026-10-14; review remains open.
- **Responsible worker / proposed owner:** Validation worker prepares this record; proposed public-review owner **unassigned**.
- **Next concrete step:** Review the sanitized docs branch and its draft PR; use the separately retained private evidence index to assess supporting claims.
- **Blocker:** No publication-authorization blocker remains. Reviewer acceptance and the release gates above remain open.
- **Objective done criterion:** Every item has an accepted owner or explicit unassigned status, next step, blocker, objective criterion, and accessible evidence for its reviewer; any authorized public revision contains no private payloads, infrastructure details, or client identities.
- **Evidence:** E10; local commit and validation receipts are retained separately from this public-safe document.
