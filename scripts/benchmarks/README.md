# Offline A/B/C trial accounting

`evaluate_agent_trial.py` validates recorded results and prints a JSON summary. It
uses Python's standard library, reads only the two files named on its command
line, and makes no provider, agent, network, subprocess or database calls. It does
not run a trial or judge code correctness itself.

The fixed arms are A (ordinary repository exploration), B (the same task with
optional Aether tools), and C (a task-specific pre-flight map supplied upfront).
Before recording results, freeze task prompts, starting commits, allowed tools,
model versions and reasoning settings, budgets, and a hidden correctness rubric.
Include multi-file work and work requiring historical context. Ground truth
should name expected dependencies and regression checks; do not treat a passing
visible test alone as proof. Randomize arm order and use isolated equivalent
starting states. Report cache conditions and map/index freshness with the trial.

Create a manifest before running any cells:

```json
{
  "schema_version": 1,
  "repository_commit": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "starting_worktree_hash": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
  "protocol_hash": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
  "tasks": ["multi-file-routing", "historical-decision"],
  "arms": ["A", "B", "C"],
  "repetitions": 3,
  "model_strata": ["exact-model-version:low", "exact-model-version:standard"]
}
```

This declares 36 cells: 2 tasks × 3 arms × 3 repetitions × 2 model strata. Use the
actual full Git commit, stable task IDs and exact model/effort identifiers.
`starting_worktree_hash` is a lowercase SHA-256 hash of a reproducible snapshot of
the initial source tree, including tracked changes and relevant untracked files.
Record the snapshot format and exclusion rules alongside the evidence. Even clean
trees require a hash. `protocol_hash` is the SHA-256 hash of the frozen trial
protocol bytes: prompts, rubrics, tool/settings versions, budgets and measurement
rules. Preserve the hashed inputs. These hashes must appear unchanged in every
result; a commit alone cannot identify dirty source or evaluation settings. The
evaluator checks agreement, but does not recompute hashes or establish that the
recorded state was used.

A manifest covers one starting source tree and protocol; use separate manifests
for tasks with different starting states. The shared protocol may declare all
arms' settings and model strata. Repetitions start at 1. Each declared cell must
appear exactly once in the JSONL results. There are no implicit cells or skips.

One result line has this shape (the example shows C):

```json
{"task_id":"multi-file-routing","arm":"C","repetition":1,"model_stratum":"exact-model-version:low","repository_commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","starting_worktree_hash":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","protocol_hash":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","status":"completed","correct":true,"missed_dependencies":0,"regressions":0,"agent_elapsed_seconds":84.2,"agent_tokens":12000,"map_creation_elapsed_seconds":4.5,"map_creation_tokens":800,"map_maintenance_elapsed_seconds":1.2,"map_maintenance_tokens":100,"map_hash":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","db_elapsed_seconds":0.12,"model_generation_elapsed_seconds":60.1}
```

Use measured values and a SHA-256 hash of the exact supplied map bytes. `correct`
is the rubric's boolean outcome. `missed_dependencies` and `regressions` are
nonnegative counts. All token counts are nonnegative integers; times are finite
nonnegative seconds. The optional database and model-generation times remain
separate. Missing measurements are omitted, never filled with zero. They can
overlap and must not be added to total elapsed time or used as proxies for one
another. Record measurement boundaries and hardware in the trial's evidence.

`agent_elapsed_seconds` includes exploration, tool waits, editing, testing and
permission delays after any upfront map is delivered. `agent_tokens` includes all
charged input, output and reasoning tokens across retries and tool round trips,
including the supplied map as input. The preprocessing fields partition additional
work outside the agent session:

- `map_creation_*` measures assembly of the task-specific upfront map. C pays its
  **full** cost on every run; A and B must explicitly record zero.
- `map_maintenance_*` measures Aether index construction and updates needed to
  supply either optional tools or the upfront map. **B and C pay this full index
  cost on every run**, including preparation done before the agent starts or
  reused from an earlier run. A must explicitly record zero.

This policy intentionally avoids hidden preprocessing and amortization assumptions.
Keep the construction/update workload, source window, cache conditions and
measurement boundaries in the evidence. Deterministic preprocessing can have zero
model tokens; its elapsed time still counts. Count each operation exactly once:
index work belongs in maintenance, subsequent map assembly in creation, and
on-demand tool/index work performed inside the measured agent session belongs in
agent time/tokens. Do not add the same work again to preprocessing. Map text counted
as agent input is separate from model tokens spent generating the map; do not
count those input tokens again as map-creation tokens.

A and B omit `map_hash` or set it to null. C provides the hash of its actual upfront
map. If a C run fails before map delivery, record `status: "error"` (or
`"permission_denied"`), `correct: false` and explicit `map_hash: null`, with all
consumed creation/maintenance costs. That failure stays in the denominator. A
completed C run must have its map hash. The evaluator cannot verify measurement truth or hidden preprocessing;
preserve raw transcripts, usage and timing evidence.

Allowed statuses are `completed`, `error` and `permission_denied`. Failures must
have `correct: false`, include every required metric, and remain in the denominator
with their consumed costs. For dependencies/regressions that could not be assessed,
record the observed counts and explain that limitation in the evidence; zeros do
not demonstrate successful coverage. Missing results, skipped runs, duplicate
cells, undeclared model strata, mismatched source/protocol provenance and absent C
map hashes invalidate the whole summary. To add repetitions or tasks, revise the manifest and collect
all new cells; do not silently drop an inconvenient result.

Run the evaluator and its fixture tests:

```bash
python3 scripts/benchmarks/evaluate_agent_trial.py manifest.json results.jsonl > summary.json
python3 -m unittest discover -s scripts/benchmarks -p 'test_*.py'
```

Invalid inputs exit 1 without printing a partial summary. Valid summaries report
all-run correctness, failure statuses, missed dependencies, regressions, total
charged time/tokens, explicit map costs and separate optional component timings.
Results are also broken down by model stratum. These are descriptive summaries,
not confidence intervals or evidence that Aether caused an improvement; analyze
paired task/repetition results and uncertainty before making a product decision.

## Reproducible offline map check

`run_offline_preflight.py` runs the existing `aetherd` binary on the frozen
[`fixtures/preflight_corpus.json`](fixtures/preflight_corpus.json). It creates a
new disposable Git repository, performs structural indexing, commits a signature
change, rename and deletion, indexes again, records two notes, and requests maps
for six tasks spanning Rust, Python and TypeScript. Three tasks require multiple
files and three require recorded historical context. This is a development
fixture, not a held-out or production-quality evaluation. Once it exposes a bug,
an improvement on the same corpus is a regression result, not independent evidence
of general map quality. Configuration selects the
existing mock inference provider, disables embeddings, and uses SQLite. The
commands never pass `--full` and never request model generation or embeddings.
The runner checks the copied binary's help and passes `--no-dashboard` on every
fixture CLI invocation when that feature is compiled. A default build lacks that
flag and the dashboard itself. No temporary HTTP/MCP server is launched.

```bash
python3 scripts/benchmarks/run_offline_preflight.py \
  --aetherd /absolute/path/to/existing/aetherd \
  --tool-source-commit FULL_CODE_COMMIT \
  --output /absolute/path/to/new/artifact-directory \
  --repetitions 3
```

The runner copies the existing binary into the evidence directory so concurrent
Cargo builds cannot change the executed artifact. It records the tool source
commit, full tracked/nonignored source-file hashes and dirty diff separately.
`manifest.repository_commit` identifies the fixture's final Git commit, not the
tool implementation commit; the starting-worktree hash covers the fixture files
and configuration actually used.

The script measures process-inclusive indexing/update/note-ingestion and map
assembly wall time. It scores predeclared path and recorded-reason markers after
removing the verbatim task echo, preserves every missing marker, and records the
binary, corpus, protocol and script hashes. It measures required-evidence marker
recall, not graph precision or patch correctness. Repeated maps do not turn six
synthetic tasks into independent quality samples. Timings include startup, Git,
database access and rendering; they do not isolate database performance. Caches
are uncontrolled. No provider-generation latency is inferred.

The same invocation runs the strict A/B/C accounting evaluator on **simulated
transcripts**. All outcome fields in that replay are correct by construction;
they do not measure an agent's correctness, dependency misses or regressions.
"Agent elapsed" is measured Python transcript construction/hash overhead; B also
replays measured map assembly as an in-session optional tool wait, charged once.
"Agent tokens" are `ceil(UTF-8 bytes / 4)` estimates of those transcripts; no
tokenizer or provider usage was measured. B includes the actual CLI map JSON.
MCP discovery/schema/client transport overhead is unmeasured because no MCP client
or agent session ran. Both Aether arms pay the complete
measured fixture index/update/notes cost; B pays measured assembly inside its
simulated agent time and C pays it upfront. There is no amortization or duplicate
charge. Providers incur zero calls/tokens/dollars; machine
and electricity costs are unpriced. The summary carries
`evaluation_kind: simulated_accounting_replay` and mandatory limitations so it
cannot silently become a live-agent result.

Outputs include `map-fidelity-summary.json`, `map-runs.json`, actual map artifacts,
`command-receipts.json`, `manifest.json`, `protocol.json`, `results.jsonl` and
`accounting-summary.json`. Failed fixture commands retain receipts and exit 1;
they cannot silently become passing accounting rows. An optional
`--legacy-results /path/to/original/results.jsonl` separately summarizes the
existing live-provider A/B records, including retry costs. It supplies no invented
C arm and leaves heterogeneous cached/reasoning token counters separate. Its
reported dollar equivalents remain the original harness's estimates.
