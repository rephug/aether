# Graph observations and incomplete coverage

Graph tools list relationships supported by the indexed evidence. An empty list
does not establish that a function has no callers, a method is unused, or a
component is disconnected in the compiled program.

## Outgoing recorded calls

`call_edges`, `resolved`, `ambiguous`, and `unresolved` count deduplicated recorded
call references from the selected indexed source symbols. Repeated call sites can
collapse into one reference. A resolved reference has one target under the
installed provenance rules. Unresolved references may be internal, external, or
standard-library calls; their spelling does not establish which.

The context graph reports these direct-call counts for its selected symbol even
when no target is listed. Its dependency entries can also include type,
implementation, and import relationships. Direct-call counts do not describe all
of those edge kinds or certify a complete transitive traversal. Callers are always
reported with incoming completeness `unknown`.

## Incoming candidate scope

`caller_coverage.scope` is `recorded_matching_target_names`. The counters describe
the recorded spellings queried for this selected symbol, once per caller/callee
pair. `attributed` is listed, `ambiguous` can fit this selection, `unresolved` lacks
sufficient target evidence, and `resolved_elsewhere` is excluded by target
evidence. Legacy-language heuristic attribution remains explicitly counted.

These counts are not a census of actual callers. Function pointers, aliases,
unsupported syntax, macro-contained calls, generated code, ignored/rejected
files, and other build configurations can be absent from that candidate set.
Even zero unresolved candidates, or a 100% proof rate among extracted references,
does not establish incoming completeness or compiler-ground-truth recall.

## Versioned usage responses

`aether_usage_matrix` and `aether_suggest_trait_split` now emit `schema_version`
`2.0`. They replace `uncalled_methods` with
`methods_without_attributed_callers`; the old key is never serialized. This is an
explicit response-contract change. Request shapes are unchanged. Deserialization
accepts the old key as an alias for historical observations, without treating it
as an unused-code verdict. Missing coverage fields deserialize as unknown.

Usage matrices expose the candidate coverage and incoming completeness. Shared
consumer groups describe observed attribution, not callers that always occur
together. Trait and file/community suggestions preserve useful positive groups, but observations
without incoming-completeness evidence have Low confidence. A missing observed
caller alone does not create a split suggestion. Isolation percentages describe
only the supplied observed matrix. Internal clustering fit/stability diagnostics
are retained as observed-clustering diagnostics, separately from conservative
architectural recommendation confidence.

The CLI health JSON keeps its base report version and separately marks the
`trait_split_suggestions` extension with
`trait_split_suggestions_schema_version: 2`. Its nested suggestion uses the same
accurate method field; text output says no attributed callers were found.

The health response adds structured `graph_coverage`, including unavailable versus
incomplete observations. Other MCP tools retain their existing response versions. Added caller coverage
fields are additive; `resolved_elsewhere` no longer combines missing proof with
evidence for a different target.

## Availability and graph-derived health

A failed database query or pending proof-policy upgrade is unavailable, not a
successful empty graph. Context output preserves the actionable error. Dashboard
and analysis coverage distinguish unavailable data from usable but incomplete
observations. Observed connectivity and disconnected components remain useful
descriptions, but are not a code-health penalty or an unused-code verdict.
Dashboard PageRank/community/component helpers can fall back from an unavailable
native projection to successfully loaded canonical SQL edges; they do not replace
failed SQL reads with an empty graph. Native health/score paths that lack those
validated inputs retain unavailability, including before score-cache reuse.
Positive proved links and detected cycles remain available. Renderer states do
not color unknown graph observations as confident good/bad health.
