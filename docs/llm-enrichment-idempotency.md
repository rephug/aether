# Completed batch enrichment

Request identity and completion are different questions. An enriched response
becomes the next baseline, so the next complete request fingerprint legitimately
differs even when source and other inputs are unchanged. Comparing only that new
fingerprint with the last request can repeatedly schedule the operation's own
output. Removing the baseline from the fingerprint would hide a real input.

## Completion reuse

New triage/deep origin sidecars optionally record the baseline actually supplied
to the model. After a successful ingest, a local completion receipt binds that
baseline and the original request fingerprint to the accepted output's SIR hash,
history version and write generation. Receipts live under
`.aether/batch-completions/<pass>/<hashed-symbol-id>.json`, one replaceable file
per symbol/pass; retired identities' files are not automatically pruned.

A later build first computes the complete fingerprint for its current baseline,
as before. If that differs, it can recognize a completed enrichment only when:

- The accepted output is still the exact current write, with fresh status and
  unchanged request provenance. Its stored JSON must still hash to that output.
- Reconstructing the earlier request with its recorded baseline and **current**
  source, context, prompt/schema, model and provider settings reproduces the
  original request fingerprint exactly.

Only the comparison uses the earlier baseline. Any request that is actually
written still contains the current baseline and retains its full fingerprint.
The SIR row and fingerprint history keep the request that really ran; they are
not rewritten to claim that a subsequent prompt was sent.

Completion publication happens after embedding, vector flushing and fingerprint
work, under the existing inject writer lock. A fatal downstream failure creates
no receipt and the existing ingest retry can resume its accepted leaf. A result
superseded by a source edit or another writer cannot acknowledge the replacement.
The write generation distinguishes even a same-content rewrite from the accepted
write; hash/version equality alone is insufficient.

Old origins remain readable and ingest normally without a receipt. They may
therefore need one new enrichment before completion reuse is available. Missing,
corrupt, unreadable or obsolete completion data permits a new request. Cache write
failures are logged without failing an otherwise successful ingest. Cache loss,
storage errors and deliberate cache deletion can consequently allow more work.
No database migration, prompt change or model change is required.

## Scheduling and bounds at PR #230's pinned head

The normal full indexer's triage/deep selectors already skip completed generation
passes. The watcher does not call the continuous monitor because an enriched
baseline changed. `continuous run-once` is a separate explicit invocation;
`hourly`/`nightly` are validated configuration values, not an in-process timer in
this implementation. An external scheduler can invoke it repeatedly.

The monitor ranks all scored symbols, including fresh ones. It takes at most
`continuous.max_requeue_per_run` (default 500) before calling the shared batch
builder. `batch.max_symbols`, when nonzero, further limits candidates; these caps
include cache hits. Chunk size and concurrent-job settings do not impose a
lifetime model-call or monetary budget. Submission defaults to off; when enabled,
each nonempty chunk is submitted and polled before its results are ingested.

Completion reuse applies to explicit batch builds and to this same monitor build
path. It suppresses direct self-baseline repetition while preserving intentional
work for changed source, model/settings, file/neighbor context, caller contracts
or an independently written baseline.

It does **not** promise one call per source revision or general convergence.
Changing generated neighbor/file summaries can still be new context, alternating
passes can change the current write, and separate in-flight builds are not
coalesced. An external schedule can also retry failed/incomplete work. A stricter
per-source budget, context-feedback policy or forced periodic refresh would need
its own explicit policy; none is silently inferred from a hash.

## Offline regression scope

Tests exercise the real JSONL builder and result-ingest path using synthesized
responses and fixed embeddings. They cover repeated triage/deep, continuous
reselection, exact request provenance, source-tail and settings changes, changed
file/neighbor context, same-content writes, failed/resumed ingest, superseded
results, old origins and corrupt cache data. Provider submission is prohibited in
the monitor test double. Executed results belong in the validation receipt.

These tests establish application behavior, not semantic explanation quality.
The held-out quality and repeated-cycle cost evaluation in
[the scan evaluation plan](llm-scan-evidence-evaluation.md) still requires a
separately approved model-call budget.
