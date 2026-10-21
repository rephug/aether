# Claude architecture review handoff — 2026-10-21

This branch is an immutable review snapshot for independent Claude cloud review while Codex continues implementation separately. It is a draft, not a merge or rollout candidate. Return findings with file/line evidence, severity, reproducible reasoning, and proposed acceptance tests. Do not implement competing production changes, merge, deploy, access live stores, regenerate agent configuration, or start provider calls, model downloads or paid agent trials.

## Pinned state and ownership

- Public base: `5e01a2d3a69fd99fee31ce1b0ee12e11442713d8` (`main` at capture).
- Source branch: `codex/architecture-correctness-plan`; completed first-slice source commit: `7f20b1bd0144c68c0fdc21d072cc36c0d5e1c58c`.
- Capture window: `2026-10-21T20:34:05.358249+00:00` to `2026-10-21T20:34:06.106962+00:00`.
- Source capture: 860 tracked/explicitly allowlisted new paths; before/copy/after SHA-256 and mode maps matched. Aggregate source manifest SHA-256: `1233cb81e16c0b866d924d0bf71bdff9caf59dd08bb37de6ea7001be4f32293d`.
- The active Codex owner remains responsible for implementation and validation in its original isolated worktree. Its HEAD, branch and index were unchanged during capture. Source files may have changed after this window.
- This public snapshot squashes the source state onto the public base so unpublished intermediate report versions are not introduced through history. It includes the completed correctness slice plus the in-progress schema-22 history/lineage and shared CLI/MCP preflight work. It excludes the separate unapplied cleanup-queue draft, raw local logs/review archives, active databases, sessions, keys, environment files and build artifacts.
- Public-report edits only replace private filesystem links/paths with repository links or explicit local-evidence notices. Code remains identical to captured bytes.

## Read first

1. [Implementation plan](../roadmap/architecture-review-implementation.md).
2. [First-slice validation and limits](architecture-correctness-validation-2026-10-21.md).
3. [Continuation report](architecture-correctness-continuation-2026-10-21.md), whose validation was still in progress at capture.
4. `AGENTS.md`, `CLAUDE.md`, and the relevant repository skills. Their schema/tool-count descriptions still describe the older baseline; inspect current code for the proposed schema 22 and preflight registration.

The source author reports extensive per-crate checks in the first-slice report. These receipts were not rerun for this snapshot and raw logs are not published. The continuation is a consistent source capture, not a claim that every changed byte passed all gates. No Rust build is launched for this handoff because the owner is already validating and shares the server's build resources. Publication checks cover snapshot integrity, diff hygiene, deterministic offline Python unit tests, report sanitization, and common secret patterns only. No live provider or paid trial is run.

## Review priorities

1. **Correctness slice:** unique shortest-depth graph traversal, inbound refresh, SurrealDB statement-error propagation and transactional replacement, configured-backend/query-purpose retrieval, explicit fallback, read-only MCP access and serialized response budgeting.
2. **History and recovery:** retirement transaction boundaries, immutable per-incarnation evidence, conservative lineage, reintroduction, incoming graph repair, and the limits of SQLite-only process-death tests. Do not equate 50 fixture SIGKILL/restarts with cross-store recovery.
3. **Preflight trust:** bounded source inventory, history and recorded reasons, provenance, freshness/unknowns, both dependency directions, relevant tests, omission disclosure and CLI/MCP parity. Bytes/4 is an estimate, not an actual tokenizer guarantee.
4. **Identity blocker:** legal trait and cfg alternatives still collide in Aether itself. The guard prevents silent overwrite but can stop a full scan. Assess the proposed migration requirements; no identity redesign is authorized in this review.
5. **Evaluation:** offline synthetic marker replay and simulated A/B/C accounting demonstrate fixture behavior only. Require held-out map fidelity and real controlled agent results before any default-map rollout or benefit claim.
6. **Integration:** inspect open PRs #222, #223 and #224 against the pinned heads in the continuation report. #224 overlaps freshness/budgeting in `context.rs`; #222/#224 also overlap configuration, agent initialization and templates. Textual compatibility does not establish behavioral compatibility. No pending PR was merged into this snapshot.

## Expected output

Produce a review report with blocking bugs first, exact snapshot file/line references, concrete failure scenarios and narrow test suggestions. Separate demonstrated defects from uncertainty, design tradeoffs and future roadmap work. State whether the completed slice can be reviewed independently, which continuation gates remain, and what prevents safe integration. The production implementation stays with Codex; share findings through the user rather than starting overlapping edits.

Respect locked choices: Rust, tree-sitter, SQLite, SurrealDB/SurrealKV, existing vector interfaces and configured model/provider choices. No new crates, database/parser substitutions or model recommendations. Any optional future test must use disposable fixtures and per-crate Cargo commands, serialized on shared build resources; never `--workspace`.

## Snapshot publication checks

The publisher reran all 17 deterministic Python unit tests under `scripts/benchmarks`; they passed. Diff whitespace checks passed. Two independent scans of the proposed diff and inventory found no common credential patterns, binaries, database/session files or symlinks. Twenty-seven private filesystem links/paths in the three reports were replaced for public distribution; no code bytes were changed. Pattern scans cannot prove the absence of arbitrary secrets. The source/copy hash comparison was repeated before publication.

This snapshot commit uses `[skip ci]` to avoid automatically starting a broad build/test workflow during the owner's ongoing server validation. Rust gates remain the implementation owner's responsibility; this review capture does not certify them. The draft should remain unmerged.
