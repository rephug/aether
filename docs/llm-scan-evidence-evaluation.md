# Source-grounded SIR scans

This change versions the prompt policy as `sir-evidence-v1`. Existing SIR JSON,
history and outstanding origin sidecars remain readable. It does not migrate a
catalog, change a model, enable a daemon, or run inference during validation.

## Implemented policy

Every prompt tier encourages observations supported by supplied code, specific
conditions and return paths, explicit uncertainty, and correction of earlier
mistakes. It discourages unobserved callee behavior, inferred architecture,
invented error conditions and confidence inflation. These are evidence rules,
not a natural-language blacklist. Terms such as "denylist", "allowlist", "may",
or "does not" are not rejected by themselves.

Live compatibility builders and batch system/user builders share the same
construction. Live defaults stay compact; batch tiers remain configurable. Source
is numbered relative to the target excerpt. The daemon carries original source
size separately from raw source and explicitly discloses truncation in scan,
triage and deep prompts. Caller expectations, file rollups, baseline records and
neighbor summaries are labeled as unverified context. Graph neighbors are not
claimed to share a file. New prompts request source references on observed
effects/errors and permit `Unknown:` entries and empty arrays. Deep mode still
supports provider reasoning; it no longer requires tagged prose that conflicts
with the JSON response contract.

The advisory audit checks missing, malformed, reversed and out-of-range line
references. Its schema-validity and citation-validity results do **not** establish
factual accuracy. It neither rejects nor rewrites records, changes confidence,
nor triggers an extra model call. Legacy origin sidecars without evidence context
do not acquire new audit requirements. Live and new batch results log defects
without logging source or claim contents. The `Unknown:` convention can itself be
misused; only evaluation and source review can determine appropriate abstention.

## Cache and compatibility

New batch identities use `v3:<source>:<context>:<configuration>`, one token inside
`symbol_id|prompt_hash|build_id`. Source covers the full source identity, including
an omitted tail. Context covers the exact dynamic prompt: baseline fields and
confidence, file intent, neighbor identity/order/intent, caller contracts, symbol
metadata, selection reason and source visibility. Configuration covers provider,
pass, model, reasoning, non-secret endpoint/protocol settings, source limit, requested tier, neighbor depth, prompt
version, response schema, and a provider-formatted request template containing
the actual system prompt and output/default settings. Only the per-build key and
dynamic prompt are replaced by fixed placeholders in that template; the dynamic
prompt is hashed separately. No credentials or network operations are needed to
format a request.

Old fingerprints remain readable and distinguishable; they are cache misses for
the new policy, not deleted records. Legacy nested pipe-separated batch keys are
parsed as a complete three-segment fingerprint. The continuous monitor compares
the full source identity for v3 fingerprints and retains the old excerpt comparison
for legacy fingerprints. Watcher fingerprints remain change-history signals, not
proof that a live request was cached under a complete provider request identity.

History keeps its existing `source_changed`, `neighbor_changed`, and
`config_changed` field names. These compare identity components, not verified
edits. For v3, `neighbor_changed` covers the complete dynamic context. Comparing
different fingerprint versions can change these flags solely because the encoding
or policy changed. Source verification is still needed to attribute actual edits.
`sir-diff` understands the v3 whole-source identity. Semantic velocity excludes
format transitions and conservatively filters v3 config changes without a source
change: the broad context digest cannot independently prove a neighbor change.
This can omit a real neighbor change coinciding with a config change; it avoids
attributing a baseline/policy update to observed neighbor behavior. Legacy
neighbor-only comparisons retain their prior behavior.

Because enrichment includes its baseline, accepting an enriched result changes a
subsequent enrichment request's identity. A separate completion receipt now
recognizes an unchanged completed enrichment while preserving the actual request
fingerprint and baseline evidence. It is bound to the accepted output's exact
write and all other effective inputs; see [completion reuse and its regression
scope](llm-enrichment-idempotency.md). Existing candidate and batch limits remain
per-run limits, not a convergence or spending guarantee. Changed context, separate
in-flight builds, cache loss and storage errors can still allow more work.
Prompt changes do not automatically launch a paid rescan.

## Offline checks and their limits

Regression tests cover shared prompt construction, all-tier evidence instructions,
truncation in live and batch prompts, whole-source cache invalidation after an edit
beyond an unchanged excerpt, cache hits for identical inputs, settings changes,
legacy key parsing, and source/context/configuration identity separation.

The audit's 15-case reference-format fixture asserts TP=9, TN=6, FP=0, FN=0 for
**citation defects only**. It includes legitimate blacklist/whitelist vocabulary,
uncertainty, negation, multiple references and malformed/range errors. A separate
test demonstrates the semantic false negative deliberately: an invented statement
with an in-range reference passes this audit. These hand-written fixtures are
development tests, not a held-out semantic-quality evaluation.

## Controlled evaluation plan (not yet executed)

1. Freeze baseline revision `48a62fb` and this candidate, prompt bytes, model route,
   reasoning, source limits, provider parameters and raw outputs. Use fresh isolated
   fixture stores, separate A/B cache namespaces, and no production data writes.
   Any model calls require a separately approved budget. Keep the route/settings
   identical across arms; increased reasoning is not an assumed improvement.
2. Have a source reviewer select 80 held-out symbols before seeing either output,
   stratified across Rust, TypeScript/JavaScript and Python, functions, types and
   tests, pure and stateful logic, opaque callees, visible propagation, and partial
   source. Exclude all few-shot and development fixtures and previously inspected
   fact sheets. Pin source hashes and split by file/project to avoid near-duplicate
   leakage. Withhold the reference facts from generation.
3. Write source-anchored reference facts independently. For enrichment, include
   correct and deliberately corrupted baseline/neighbor summaries and caller
   expectations that the implementation does and does not establish. Mark omitted
   evidence separately from absent behavior. Pair complete/truncated conditions.
   Include instruction-like comments/context and benign allowlist, denylist,
   uncertainty and negation vocabulary; measure inappropriate instruction-following
   and suppression of supported claims rather than matching forbidden words.
4. Randomize anonymized output order. Two reviewers independently label supported,
   contradicted and unsupported claims, important facts missed, valid references,
   reference relevance, and appropriate uncertainty. Resolve disagreements and
   retain both initial labels. A model judge may assist but cannot supply ground
   truth or treat polished wording/high confidence as correctness.
5. Report factual precision, important-fact recall, unsupported/contradicted claims
   per symbol, citation-defect FP/FN and citation-relevance FP/FN, correction of bad
   baselines, inappropriate abstention, malformed JSON, latency, prompt/output
   tokens and actual billed cost separately. Include repeated unchanged-source
   enrichment cycles and cumulative model calls. Assess confidence calibration against
   supported fact coverage, not schema validity. Report paired 95% bootstrap
   intervals grouped by project/file, and slices for truncation/opaque callees.
6. Pre-register acceptance: no schema-validity regression; a reduction in unsupported
   claims with no more than a two-percentage-point decline in important-fact recall;
   inspect every evidence-policy false positive. Treat inconclusive intervals as
   inconclusive. Do not tune policy on held-out results; revised prompts need a new
   holdout. Measure token overhead explicitly, especially compact/local context.
7. After semantic quality passes, compare coding-agent tasks with source verification
   enabled, identical tool budgets, and task-level tests. Measure completion rate,
   wrong edits, source lookups and total token/time cost. Better scan annotations
   alone do not demonstrate better coding outcomes. Keep old records and a prompt
   rollback path until this second stage is satisfactory.
