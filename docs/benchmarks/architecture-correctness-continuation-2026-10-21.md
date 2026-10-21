# Architecture correctness continuation — 2026-10-21

Status: validation in progress. This report separates implemented behavior, fixture results, existing live-provider evidence and remaining work. The full architecture plan is **not complete**.

## Scope and preservation

Starting repository commit: `5e01a2d3a69fd99fee31ce1b0ee12e11442713d8`; branch `codex/architecture-correctness-plan`. The completed first stage was audited and committed as `7f20b1bd0144c68c0fdc21d072cc36c0d5e1c58c`. Thirty-three changed/new files matched the retained validation fingerprints; diff checks and scans for common credential/private-key patterns found no unrelated changes or detected secrets. A pattern scan is not a guarantee that arbitrary secrets cannot exist.

All database/schema exercises use disposable fixtures. Other worktrees, active stores and sessions were preserved. No PR was merged, no deployment or push performed, no weights downloaded and no alternatives installed. Only the two explicitly requested existing provider tests were attempted; the offline evaluation makes no provider calls. The tracked `/pre-flight` command/template changed manually in this isolated checkout; no `init-agent` regeneration ran.

## Pending PR overlap

GitHub metadata checked October 21, 2026; all three PRs remain open and unmerged.

| PR | Inspected head | Integration finding |
| --- | --- | --- |
| [#222 compact tool profile](https://github.com/rephug/aether/pull/222) | `084c59066bddeb6dc590c3c0b84070314eab9909` | Shared router changes are textually compatible with Stage 0. The new map must be registered deliberately in the compact profile before promotion. |
| [#223 Surreal runtime/session lifetime](https://github.com/rephug/aether/pull/223) | `2ecb52d325ad9fe7bf2472f1502ebc8d525eded1` | Graph-file overlap is textually clean at Stage 0; runtime lifetime tests must still run with transaction/error changes. |
| [#224 honesty/freshness](https://github.com/rephug/aether/pull/224) | `807fcabbcdb989b704d2c633d218f21a937a7d71` | Conflict in `context.rs`: freshness output versus actual serialized budgeting. Router/template registration also needs reconciliation with the new map. |

#222 and #224 conflict independently in configuration, `init_agent.rs` and `templates/mod.rs`. #224 removes `TOOL_DESCRIPTIONS`, which the current baseline's generated-command guard still uses. These changes were inspected, preserved and reported; they were not overwritten or integrated. A textual merge probe is not a behavioral test.

## Exact provider-test disposition

1. `aether-analysis::drift::tests::report_generates_semantic_drift_and_mechanical_fallback` remains an ignored live-service fixture. The explicit attempt failed with HTTP 500 from `http://127.0.0.1:11434/api/embeddings`, after 9.81 seconds of test runtime. Ollama was already running and `qwen3.5:4b` was installed; its metadata listed completion/vision/tools/thinking, **no embedding capability**. The fixture defaults to that model and endpoint through `qwen3_local`. No API key is required. To run successfully it needs an embedding-capable service/model matching this fixture's embedding configuration; the installed model does not supply that capability. No model was installed or silently substituted. The HTTP response body was not captured; the missing advertised embedding capability is consistent with this failure, while its exact server-side cause remains unverified. The failure precedes semantic drift output, leaving embedding-backed drift and model-generated change summaries unverified. Its assertion accepts either model text or mechanical fallback, so even a pass would not verify both summary routes independently.
2. `aetherd --test phase1_step2::step2_embeddings_refresh_when_hash_changes_and_delete_on_symbol_removal` was previously ignored in both default and dashboard runs. The explicit original attempt failed after 1.06 seconds: “embedding metadata should exist.” `new_with_provider` supplies inference only; it does **not** load configured embeddings. Therefore this test needed a code/fixture correction, not credentials. A new constructor accepts existing loaded provider abstractions; the fixture injects source-dependent mock inference and counted deterministic embeddings, removes the misleading ignore, and verifies initial vector identity, edited-source refresh, obsolete provider/model replacement and deletion of all vector rows. It requires no credentials or services. This regression verifies lifecycle behavior, not Ollama/Gemini request behavior, semantic quality or production 3072-dimensional embeddings.

The cloud API-key variables inspected were absent. They are irrelevant to these two fixtures; an authorized production Gemini contract test would separately need its configured API-key environment variable and network access. No such additional trial was launched. No whole-system/provider verification claim follows from these tests.

Raw original failure logs: CLI fixture (retained local evidence; not published), drift fixture (retained local evidence; not published).

## History, graph projection and map changes

Schema 22 retains original evidence in immutable, per-incarnation archives before active deletion. Explicit same-file/language/kind/name signature candidates carry heuristic lineage, while conflicting branches remain retrievable under old IDs. Reintroduction cannot inherit current contracts, tests or annotations. Startup and event paths use conservative matching; unchanged overloads, cross-language definitions and multiple-old candidates do not receive guessed lineage. Migrated annotations are stale. Graph-node/vector cleanup and incoming caller reconstruction are covered separately from SQLite retention.

One structural pre-flight service assembles CLI and MCP output with fixed sections, recorded note IDs/text, path history, both dependency directions, tests, citations, provenance, omissions and visible unknown freshness. It opens only read-only SQLite, with source-only CLI fallback when the index is missing. MCP assembly runs off the async executor. Serialized response budgeting is UTF-8 bytes/4, including MCP SDK metadata, **not an actual tokenizer guarantee**. Generation/projection freshness and compiler-resolved edge completeness remain unknown. The source inventory has explicit file/byte limits and excludes unsupported/non-source files.

Git path history now follows renames, includes deleted-code commits and retains full commit messages. Recorded messages/notes remain author evidence; they are not asserted causal explanations.

## Trait-ID decision

The pure parser audit read **467 Rust files**, parsed **465**, emitted **7,184 symbols from parsed files**, and reported two blocked paths with no unread or bounded-out files. At the initial audit fingerprint `dae75259594d80b617d2ae06158a5f6f35f40f76ef78c7a6bdd545b66ecdb40d`, the real blocked cases were:

- [Secret formatting implementations](../../crates/aether-core/src/secret.rs): legal `Debug::fmt` and `Display::fmt` collapse to `Secret::fmt` with the same signature.
- [Platform process termination](../../crates/aetherd/src/omp_gateway.rs): mutually exclusive cfg alternatives collapse to `terminate`.

The audit stops at the first extraction error in each file, so it does not count every collision. The explicit error is an acceptable temporary guard against silent overwrite; blocking whole scans of valid repositories, including Aether itself, is **not general indexing readiness**. Inherent/trait, multiple qualified generic traits and cfg alternatives have regression fixtures. The guard remains visible.

A migration needs versioned trait/impl/build-context identities; old/new source-location mapping and dry-run; an explicit policy for one-to-many legacy evidence; compatible method/caller resolution and aliases; retained old-ID queries; export/rollback and disposable upgrade fixtures. Changing declaration names alone loses callers and can misattribute contracts. Recommendation: a separately reviewed migration that retains ambiguous old evidence as unassigned, plus an explicit feature/cfg coverage policy. The current archive foundation does not implement that migration.

## SQLite process-death result

The actual lifecycle API was exercised across **50 SIGKILLs**, with **50 observed writer windows**, **100 successful integrity checks** and **50 restart convergences**. Post-kill observations contained 550 fully active and 50 fully retired declarations; after completion, all 600 had exactly one retained retirement. Test runtime was 15.62 seconds; the experiment's measured loop time was 15.593 seconds. Twelve symbols/round, two SIR versions/symbol, 8,192-byte payloads, seed `12573522557516447824`, kill offsets 0–2,499 microseconds, and fixture-only trigger work make the setup reproducible.

This deliberately tests SQLite transaction recovery. A lock observation does not identify the exact SQL statement at death. Mirror files, external graph/vector stores and provider requests were excluded; this is **not cross-store crash-recovery proof**. Raw result (retained local evidence; not published).

## Validation and offline results

The preserved v1 development-fixture run measured 6 synthetic tasks (3 multi-file, 3 historical), 3 languages and 3 repetitions: required literal markers **13/19** on unique tasks (**39/57** repeated), with every marker present for **1/6** tasks (**3/18** maps). Process-inclusive map latency was p50 **61.34 ms**, p95 **76.91 ms**; initial/update indexing and two note writes totaled **1.4700 s**. One missed literal phrase was semantically paraphrased in a retrieved commit, so this strict score is not semantic reason recall. Inspection exposed outside-scope tests, unlinked notes and deleted-path discovery gaps; v1 is preserved before those fixes. The observed corpus is development data, not a holdout.

The fixture consists of language fragments without build/package manifests or valid cross-file imports. It measures evidence-marker retention, not runnable task correctness, dependency precision or regressions. The labelled A/B/C replay contains 54 complete accounting cells; outcomes are assumed fixture inputs, not agent results. In v1 the estimated transcript totals were A 11,673 / B 30,492 / C 16,044 tokens. These use UTF-8 bytes/4, not tokenizer/provider telemetry. Both B and C pay full measured index/update/note preparation on each cell and one map assembly (inside B's tool wait, upfront for C). Total charged fixture time was A ~0 / B 27.5880 / C 27.5879 seconds; CPU/storage are unpriced. Model generation and provider spend were zero in this offline path. MCP discovery/transport overhead was not measured.

Existing live A/B records remain separate: 130 attempts, 128 unique cells, latest outcomes 58/64 ordinary versus 54/64 optional Aether. All-attempt wall time was 7,479.3 versus 8,261.3 seconds; reported price-based USD equivalents were $2.35672136 versus $3.34741375, **estimates rather than invoices**. No C arm or full lifecycle accounting exists in that older study; permissions and discovery confounds remain. It cannot establish this branch's effectiveness.

Preserved v1 report (retained local evidence; not published). Final post-fix measurements and command summaries follow after validation. Full-workspace coverage is performed as serialized per-crate checks rather than `--workspace`, honoring the repository's OOM constraint; dashboard and verification configurations are additional runs. Earlier failures remain in the receipt log and are not discarded.

## Additional ignored tests in the wider workspace

These are distinct from the original two above. No global `--ignored` run was used.

| Test | Specific blocker / unverified behavior |
| --- | --- |
| `aether-infer`, integration `candle_integration::candle_provider_real_model_produces_non_zero_embeddings` | Creates a fresh temporary model directory and would download/load `Qwen/Qwen3-Embedding-0.6B`. Weight downloads and additional model execution are outside authorization. A pass would establish only finite/nonzero 1024-dimensional output, not locked Gemini 3072-dimensional quality. |
| `aether-infer`, integration `reranker_integration::candle_reranker_real_model_produces_finite_scores` | Fresh model directory for `Qwen/Qwen3-Reranker-0.6B`; same download/model-execution restriction. Finite scores would not establish correct ranking. |
| `aether-infer`, integration `reranker_integration::cohere_reranker_api_call_returns_ranked_results` | Requires `COHERE_API_KEY` and reachable Cohere API, and is outside the two specifically authorized attempts. The key is absent; this test currently returns early without it, so forcing it would falsely appear to verify a provider. |
| `aether-health::planner_communities::ablation::ablation_aether_store` | Hard-codes the live `<private-local-path>` checkout. |
| `aether-health::planner_communities::ablation::ablation_aether_config` | Same live checkout; not an isolated fixture. |
| `aether-health::planner_communities::ablation::ablation_aether_mcp` | Same live checkout. Their helper can open writable embedded/vector paths and returns early when data are missing. Running them violates preservation boundaries and can produce misleading passes. |

The desktop host lacks `gtk+-3.0`, `webkit2gtk-4.1`, `javascriptcoregtk-4.1` and `libsoup-3.0` pkg-config development packages. Desktop verification needs the repository CI image or an explicitly provisioned native build environment. No host packages were installed.

## Remaining gates and readiness

The history/map foundations are reviewable only after their checks pass. They do not finish the roadmap. Remaining gates include pending PR conflicts and generation markers, trait/build-context identity migration, a shared stored resolution contract, structural cross-store replay/fencing, recorded revision provenance, real rename/move labels, retention/erasure policy, held-out map fidelity, actual-tokenizer and live A/B/C agent trials, and the broader vector/concurrency/model-fidelity matrix.

An ordinary optional-tool A/B study and simulated A/B/C accounting cannot justify default rollout. Recommend reviewing bounded correctness commits independently; withhold authoritative-map/default rollout until identity, freshness and recovery gates are satisfied.
