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
