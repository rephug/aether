# PR226 repair scope and rollout limits

This repair keeps PR226 as a draft. Merge, deployment, guard activation, and live
store migration remain separate decisions. Its test receipts describe isolated
fixtures and the exact tested source revision, not a production installation.

## Review repairs

- Watcher identity collisions quarantine the affected file, retain its prior rows
  and read timestamp, and allow unrelated files to continue. The same receipt is
  updated for startup, git batches, and normal debounce processing. Recovery uses
  the complete successful file read to retire declarations removed while blocked,
  including after a watcher restart. The file remains the quarantine unit: other
  declarations in that file are intentionally frozen too.
- Freshness consumers (including MCP status and doctor) report blocked or unknown
  quarantine coverage as stale, with a reason. Quarantine does not advance a
  completed full-sync marker. An empty quarantine receipt is not evidence of a
  completed generation or a currently running watcher.
- Full/Compact transitions inspect selected guidance as well as obsolete entry
  points. Customized guidance is refused before any file moves. Recognized
  guidance is archived, and a recovery inventory is written before the first
  rename. A partial transition can require manual recovery; this is not a
  transaction across the filesystem. Keep all backups until recovery is checked.
  Shared MCP updates preserve other servers, custom Aether fields (including env),
  and arguments other than the explicitly refreshed workspace/profile selection.
- Budget-trimmed evidence is labeled as omitted in the rendered context, including
  when trimming empties a section; an omission is not evidence of absence.
- The route checker supports explicit redirect and same-page alias policies. A
  same-page alias needs the expected page mapping and HTML canonical. Expected
  media must match the complete supported reference set, and an approved entry
  link cannot hide additional unapproved links. Unrelated entry links need an
  explicit ignore list. Relative URLs and query/fragment cache suffixes are
  normalized. Local HTML media/srcset, inline CSS url(), and declared local
  stylesheets are supported. CSS imports, escaped URLs and image-set are refused.
- JavaScript is rejected by default. The explicit pinned_uninterpreted policy
  requires page and referenced script byte receipts; it verifies identity only,
  not JavaScript behavior or imported dependency closure. Dedicated application
  verification is required for script-generated media and runtime routing.
- CI runs the Python route tests and the verification feature for aetherd and
  aether-mcp. CI success covers the checked revision and exercised features only.

## Store compatibility gate before any production use

Schema 22/23 retention cannot protect a store from an older writer. Old aetherd,
LSP, MCP, automation, or helper binaries can still open an upgraded SQLite store
and apply their old destructive deletion behavior. A new-binary check cannot be
retroactively added to those binaries. Do not run mixed writer generations or
use an old binary to downgrade an upgraded store.

Before a separately approved rollout:

1. Inventory every writer process/service and the executable it actually runs.
   Stop or isolate all writers for that workspace; replacing a file on disk does
   not replace a running process. Preserve independent audiobook services.
2. Create and verify a consistent SQLite backup using SQLite's backup mechanism
   (for example its .backup command), or a complete stopped-writer snapshot that
   includes the WAL/journal state. Copying only a live meta.sqlite is insufficient.
   Preserve the associated SIR filesystem payloads and relevant configuration too.
3. Restore that backup into an isolated directory. Exercise indexing, retirement,
   payload recovery, quarantine, and external cleanup there with the candidate
   binary. Record the binary SHA and restored store schema. Never use the live
   workspace for this acceptance run.
4. Replace or disable every old writer before allowing the candidate to migrate
   or write the real store. Keep an independently recoverable pre-upgrade backup.
   Rollback means restoring the coordinated backup with writers stopped; it does
   not mean opening the upgraded store with an older executable.

No production backup, migration, writer replacement or process stop is performed
by this repair pass.

## Scope retained for explicit review

PR226 reconciles earlier development branches and is larger than these repairs.
The following inherited changes remain visible for review; they were not removed
without examining their consumers:

- Doctor probes configured inference/model/embedding capabilities over network
  endpoints. It is not an offline-only status command. This repair does not run
  it against paid providers or assert provider availability.
- The Gemini model default and temperature policy are inherited provider changes;
  the separate Google gateway path has its own temperature behavior. These need
  provider-specific review and are not established by offline route tests.
- file_history invokes git log --follow to include deletions and rename history.
  This changes the inputs of history-based health/priority/context features. The
  compatibility file_log wrapper still maps errors to an empty vector; callers
  requiring truthful degradation must use the fallible file_history API.
- The vector benchmark and agent-trial helpers are development evidence, not
  runtime prerequisites or a reason to run network/GPU workloads in this repair.
  Their purpose and scope should be accepted explicitly, or split before merge.

## Remaining limitations requiring follow-up

Replacement publication and reconciliation now use one SQLite IMMEDIATE
transaction. Current catalog rows, migrated annotations and their original
source hashes, stale status, contracts, lineage, retirement evidence and external
cleanup obligations commit or roll back together. Snapshot planning holds the
inject lock before reading the catalog; watcher and quarantine recovery use the
same publication operation. A failed watcher persist restores the prior observer
acknowledgement, so an unchanged subsequent event can retry without a restart.
The conservative unchanged-overload/sibling rule remains in place. Existing
reconcile-only callers retain their previous metadata-status behavior.

Regressions cover SQL/archive failures, empty and record-only publication,
concurrent replay and actual child-process exits without destructors: after the
replacement insertion, after retirement changes but before commit, and after
commit before mirror cleanup. Restart retains active annotations, contracts,
source hashes and exactly one lineage/retirement. A retired mirror cannot
rehydrate active evidence, including after same-ID reintroduction; explicitly
written new SQLite annotations remain readable. These process-exit tests do not
simulate power loss or establish storage-device durability.
The fix prevents new partial publication; it cannot identify or automatically
repair catalog rows already committed by an older interrupted implementation.
Those rows can be indistinguishable from legitimate existing overloads. Review
or restore such stores before indexing them; retained archives are recovery
material, not proof that active annotations already migrated correctly.

Mirrors, graph/edge replacement, vectors, file-read stamps and freshness markers
remain separate effects. Rollups are conservatively invalidated before canonical
publication and may require rebuilding after an interruption. This is not a
global filesystem/projection transaction. Durable retirement cleanup handles
external deletion; it does not establish a complete projection generation.
Audit rows affected by migration cascades remain in archive JSON rather than
necessarily remaining queryable through the active audit API. Archived lineage
is heuristic, and external projections remain a separately replayed component.
Mixed older writers are a rollout blocker rather than a new-store merge defect:
they must be quiesced/replaced and a verified recoverable backup must exist before
opening a real store with the candidate. New code cannot retrofit a schema fence
into an already-running old binary. No live rollout is approved by green CI.

The bounded quarantine receipt expands at most 128 paths and summarizes four;
omitted paths are disclosed. It is the indexer's latest observation, not a live
scan. Static artifact hashes are separate from Aether's supported-source hash.
The application-specific route fixture is maintained separately from the PR. Its
future mandatory pre-publish job must fail publication when the guard fails;
optional aether_verify configuration alone is not a release gate. Deployment,
HTTP redirect behavior, browser rendering, external imports and CDN cache state
remain unverified.
