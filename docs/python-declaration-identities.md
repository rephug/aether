# Python declarations with colliding legacy identities

Python can declare the same qualified name and signature in conditional branches,
or use the same signature for an overload stub and its implementation. Previously
these declarations had the same ID: ID-keyed snapshots could lose a declaration,
and structural publication could reject repeated current IDs.

The parser now groups Python declarations by their original ID, before edge,
test-intent or snapshot mapping. Singleton groups retain their existing signature
fingerprint and ID. Collision groups receive a versioned contextual fingerprint:

1. Retain the original signature fingerprint as input.
2. Hash structural ancestor headers, branch roles, and decorators. Token framing
   preserves whitespace inside literals while ignoring inter-token formatting
   and comments. Body text, byte offsets and occurrence ordinals are excluded.
3. Store that contextual fingerprint and compute the ID with the existing core
   `stable_symbol_id` function. Qualified names remain unchanged.

This keeps hover ID recomputation consistent with the catalog. Call ownership
uses the final IDs through declaration ranges; test intents also bind to the
final symbols rather than reconstructing the legacy ID. Qualified call targets
can still denote several alternatives; this fix does not prove runtime branch
selection or resolve all dynamic Python calls.

If context still cannot distinguish declarations (for example two same-signature
definitions in the same block), the parser returns `DuplicateSymbolIdentity`
before any ID-keyed projection. The observer can quarantine the file while
retaining prior evidence. Distinct bodies are never silently deduplicated and
neither declaration is selected as the winner.

## Compatibility and retirement

No database schema or core-ID algorithm changes. A fresh structural reparse is
required to publish the new collision identities. Existing singleton IDs are
unchanged; files that gain or lose a collision family can change the affected
members' IDs. Header/decorator changes can also change contextual IDs. Body edits
and unrelated line insertion do not.

An old collapsed ID cannot establish which alternative owned an explanation or
contract. Existing ambiguous old-to-many reconciliation archives that evidence
under its original identity rather than copying it to a branch. Fresh explanations
must be generated for current variants. Historical lookup remains explicit.
Returning from two variants to a singleton must not revive the singleton's old
retired annotations; repeated indexing must not duplicate retirement evidence.
The existing one-to-one signature migration relation remains a candidate
relationship with stale output, not proof of historical ownership.

Do not bypass the store's unique-current-ID guard, rewrite a real catalog by hand,
or mark old explanations fresh during adoption. Use structural indexing first
and inspect the resulting current IDs, retirement evidence and indexing gaps.
