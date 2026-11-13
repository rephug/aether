# Navigating enhanced context

`aetherd enhance` prefers explicitly named symbols over broad namespace or
substring matches. For example, include an owner such as `PacketReader.decode`
when several classes define a method named `decode`.

When the current local source matches an indexed declaration, its context
includes an inclusive source range, such as `src/packet.rs:3-5`. Use that location
to inspect the code before relying on an explanation or changing it.

Unavailable source or a changed declaration produces a warning and no guessed
location. Re-index changed source to refresh the declaration identity. Context
can still include the indexed symbol when its current location is unavailable.

Offline enhancement needs local source files for these locations, in addition
to its index. An index-only copy can return symbol metadata but cannot verify
source locations.

For an unmatched qualified query, enhancement can list **possible indexed
matches** separately in its warnings. It checks camelCase leaf spellings against
snake_case variants while retaining the supplied owner. A missing intermediate
class is suggested only when the qualifier matches an indexed file-module path.
The full candidate name and indexed path are shown; ambiguous owners remain
separate candidates. These hints do not populate `resolved_symbols`, provide
verified source ranges, or claim current explanations. Verify a candidate with
its full qualified name and source before relying on it.

Suggestions are limited to three candidates for each of the first three eligible
unmatched qualified queries. Exact matches keep priority. The existing exact-leaf
catalog queries may materialize many same-leaf records; this limits displayed
hints and the number of queries, not database scan work.


## Focused discovery

When indexed exact matches for an ID, qualified name, capitalized identifier,
or explicitly backtick-quoted identifier establish a target, enhancement reserves at most two additional symbol slots for
broad substring and concept discovery (within the existing eight-symbol ceiling).
Incidental lowercase prose matching a leaf name does not establish a focus.
Duplicate query spellings do not increase this allowance. Exact targets retain
priority. Queries without exact evidence retain the full discovery ceiling.

This is progressive disclosure, not a claim that omitted matches are irrelevant:
ask a focused follow-up query to explore another symbol. Current-source identity
verification and source-location omission warnings remain unchanged.
