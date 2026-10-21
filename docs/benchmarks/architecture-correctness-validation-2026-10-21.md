# Architecture correctness validation — 2026-10-21

The first implementation stage and existing read-only-mode hardening are implemented and tested in `codex/architecture-correctness-plan`. This is regression evidence, not a demonstrated coding-agent benefit or a database performance benchmark. Later stages in the [implementation plan](../roadmap/architecture-review-implementation.md) remain pending.

Baseline: **`5e01a2d3a69fd99fee31ce1b0ee12e11442713d8`**. This first-stage snapshot was committed as **`7f20b1bd0144c68c0fdc21d072cc36c0d5e1c58c`** after a separate diff/secret review. Subsequent work and corrections are recorded in the [continuation report](architecture-correctness-continuation-2026-10-21.md). The tested source fingerprint is **`cef7260adf0d6e5b9ddec3a6cb1bfa96c9d0b956402a7e9396dd9092ea4d1fe5`**; the machine-readable manifest (retained local evidence; not published) records its definition, individual file hashes and command receipts. Raw command outcomes (retained local evidence; not published) retain earlier failures as well as the final passing runs.

## Checks

| Crate / feature configuration | Passed test executions | Expected ignored executions | Clippy with all targets and denied warnings |
| --- | ---: | ---: | --- |
| aether-parse (retained local evidence; not published) | 108 | 0 | passed |
| aether-store (retained local evidence; not published) | 165 | 0 | passed |
| aether-analysis (retained local evidence; not published) | 54 | 1 | passed |
| aether-memory (retained local evidence; not published) | 17 | 0 | passed |
| aether-mcp (retained local evidence; not published) | 119 | 0 | passed |
| aetherd (retained local evidence; not published) | 472 | 1 | passed |
| aetherd-dashboard (retained local evidence; not published) | 473 | 1 | passed |
| aether-mcp-verification (retained local evidence; not published) | 124 | 0 | passed |

`cargo fmt --all --check` passed. The offline evaluator passed **10 Python tests**. Rust checks ran per crate, one cargo process at a time, with two build jobs, the configured toolchain/linker and build/scratch files under `<private-local-path>`. No workspace build was used. After the final graph changes, store and MCP checks were repeated; the verification-feature run also exercised their final implementation.

At this first-stage checkpoint, three ignored executions were classified as two tests requiring a running local embedding provider: the analysis drift fixture and the CLI embedding refresh fixture (the latter in default and dashboard configurations). They were deliberately not enabled. Every new regression test ran, including the optional verification read-only guard. These skips do not count as a completed provider or model evaluation. **Continuation correction:** the CLI test was misconfigured, rather than inherently provider-dependent; the continuation reproduces that failure, injects a deterministic embedding fixture and removes its ignore. The drift test was attempted against the existing service and failed; see the linked report for the exact blocker.

## Demonstrated behavior

- Incoming neighbors survive callee refresh, forward references resolve when the callee appears, and replacing outgoing edges clears stale relationships. Cycle/diamond traversal returns unique shortest-hop nodes. An exclusive SQLite lock no longer blocks the current-thread executor; a non-Tokio executor remains supported. See [graph fixtures](../../crates/aether-store/src/graph_sqlite.rs) and [neighbor fixtures](../../crates/aether-store/src/tests/neighbors.rs).
- SurrealDB statement errors reach callers and failed replacements roll back. Fault injection also exposed an invalid conditional insert returning success with no node. Symbol upserts now check the returned ID, so that case fails explicitly. See [transaction and upsert fixtures](../../crates/aether-store/src/tests/surreal_correctness.rs).
- Ranked multi-term search finds candidates outside arbitrary alphabetic prefixes; a mock reranker can select beyond rank 100; degradation and response limits remain explicit. A recording embedding mock proves Query purpose and an isolated Lance/SQLite decoy proves configured-backend selection. This does not establish real-model retrieval quality.
- Existing read-only MCP mode creates no absent index, suppresses counters/implicit enrichment, and avoids writable embedded backends. Representative reads and rejected writes preserve the temporary store's file inventory, bytes and mtimes. SQLite WAL coordination remains a documented limit on universal filesystem immutability. See [read-only integration fixtures](../../crates/aether-mcp/tests/readonly.rs).
- Context fitting includes the SDK's text and structured content, escaping and metadata. Unicode, JSON and legacy-layer tests fit the declared UTF-8-byte estimate. This is not a provider-tokenizer guarantee. See [context fixtures](../../crates/aether-mcp/src/tools/context.rs).
- The [offline trial evaluator](../../scripts/benchmarks/README.md) rejects missing/skipped/duplicate or mismatched-provenance cells, retains failures (including map-build failures), charges index preparation in both Aether arms and supplied-map assembly in C, and keeps database/model timings separate. It runs no agents and does not grade patches or infer causality.

## Limits and rollout

Trait-qualified identities remain deferred because the current reconciliation and resolver cannot migrate them safely. The interim Rust collision guard returns an explicit extraction error, including for colliding legal cfg alternatives; this can stop a full scan. No lineage/tombstone migration, complete graph-resolution contract, structural crash-replay suite or unified upfront task map has been delivered by this slice.

Read-only sessions explicitly lack Lance semantic search and embedded Surreal analytics; drift recomputation, causal tracing and prompt enhancement fail before opening writable analysis paths. SQLite-backed source, dependencies, notes and available history remain usable. Writable startup remains the default. The pending #222/#223/#224 changes have not been merged into this branch.

Statement checks and per-backend transactions do not prove cross-store crash consistency, complete forward-edge projection or graph-resolution precision. Full lifecycle replay, held-out map fidelity and the controlled agent trial remain the three next experiments. No latency, recall, token-savings or model-quality claim is promoted from these unit/integration checks.

At that first-stage checkpoint, active stores had not been opened or migrated by this task. Tests used disposable stores. No alternative was installed, no model weights downloaded, no provider/generation trial launched, no session archived and no live agent configuration regenerated.
