# Rust and ECMAScript graph edge provenance

A stored reference is not necessarily a graph relationship. Two indexed projects can
both declare `helper` or `Buf::new`; matching that spelling and language does not
establish that one calls the other. Rust, TypeScript, TSX, JavaScript and JSX edges now
carry explicit target evidence alongside their target text:

- `target_symbol_id` identifies a declaration the parser resolved locally. It must
  still agree with the target name, any supplied file path, and the caller's language
  family. A missing or stale ID never falls back to a file or name match.
- `target_file_path`, without an ID, identifies a proven definition file at its exact
  workspace-relative path. The target qualified name must identify one declaration
  in that file and the caller's language family.
- With neither field, a reference remains stored and appears as unresolved coverage.
  The graph does not pick an unrelated declaration even when it is the only matching
  name in the index.

The same evidence gates structural dependencies, denormalized neighbors, call chains,
file dependencies, graph projection, dashboard graph algorithms and context dependency
lookups. Selected-symbol caller views use target IDs, so the caller of one project's
`helper` is not attached to another project's `helper` merely because both incoming
references are valid in their own projects. Language-family
checks still apply: JavaScript may call an explicitly imported TypeScript declaration,
while a Rust reference cannot resolve to a TypeScript declaration with the same name.
Other language parsers retain their existing resolution behavior; this change does not
claim provenance-complete resolution for them.

## Supported bounded resolution

The extractor must be configured with `with_workspace_root` for imports that read
other files. Pure source extraction still resolves supported local declarations.

Local ECMAScript proofs cover lexical function declarations. Local constructor/member
and callable-value inference remain outside this slice.

ECMAScript supports named relative imports, including aliases, from a single existing
module file with direct exported function or class declarations. Extensionless imports
consider TypeScript and JavaScript extensions and directory `index` files. Explicit
directory specifiers, including trailing `/`, `/.`, `/..` and `./`, consider only index
files. More than one existing candidate fails closed, even when only one candidate has been indexed.
An explicit extension names that exact file; TypeScript's substitution of a written
`.js` extension with a `.ts` source is not implemented. Parameter or local variable
shadows do not inherit the imported function's evidence.

Rust local evidence covers lexical free functions, inline module paths, explicit
same-file `self`/`super` aliases, simple inherent `Type::method` and `Self::method`, locally resolved
type references and implementations, and direct nongeneric tuple-struct constructor
calls such as `Wrap(1)`. Unanchored `use` paths remain unresolved because their
meaning differs across Rust editions, and file-local extraction does not establish
the edition or crate-root context. Explicit `crate` imports use the workspace
resolver described below.

A small set of nominal value receivers is also supported: immutable parameters or
local bindings with an explicit concrete nongeneric struct type, `self` in a simple
nongeneric inherent implementation, and immutable bindings initialized directly
with a unit struct name, such as `let store = Store; store.read()`. The struct and
selected inherent method must be nongeneric and unambiguously declared together;
a dot-call target needs an immutable `self` or `&self` receiver. A tracked `&self`
source may only call another `&self` method; ordinary typed `&T` receivers are not
resolved. This does not infer return types or follow arbitrary expressions. Mutation,
reassignment, shadowing, destructuring and generic types/implementations leave raw
references. Trait implementations never supply a guessed method target.

Dot-call proof is deliberately withheld for consuming standard-prelude trait method
names, such as `into` and `collect`, and for caller scopes with globs, extern-crate
bindings, competing local trait methods, or unknown imports that could introduce a
trait. The receiver's verified type import and independently proved local concrete
type imports are allowed; even an unrelated unmodelled import can therefore suppress
a dot-call proof. The guarded prelude spellings are listed in
`crates/aether-parse/src/languages/rust/provenance/receivers.rs`.

Cross-file calls support explicit `crate`, `self` and `super` paths, aliases and grouped
imports through conventional Cargo `src/lib.rs` or `src/main.rs` roots and declared
external module files. An indexed source must be included by that module chain
before it can supply cross-file import evidence. Targets are direct `pub` or
`pub(crate)` free functions, or supported public inherent members of a directly
declared public nongeneric struct. This includes explicit imported static calls
such as `Store::create()` and the same bounded nominal receiver forms on imported
concrete struct types. The defining files are tracked so their changes can invalidate
callers' extracted proof.

An attribute on a Rust `use` makes its introduced names uncertain. Enabled and
disabled `cfg` conditions are both conservative because the extractor does not
evaluate build context. Exact, unqualified `cfg`, `allow`, `warn`, `deny`,
`forbid`, `expect` and `doc` attributes use name-specific uncertainty: those names
cannot provide ID/file evidence or fall through to an outer declaration, while
unrelated free-function bindings can retain proof. Aliases and grouped imports,
including `dep::{self}`, obey the same rule. An attributed wildcard import can
make any imported name uncertain.

Unknown attributes and `cfg_attr` can expand into additional bindings, so lookups
through their affected scope remain uncertain even for names absent from the
written import. A procedural attribute on `use dep::helper` could instead create
a function that shadows an outer `stable`; simply blocking `helper` is insufficient.
Comments are neither attributes nor wildcard imports. Unsupported attributed
call/import references remain recorded for later re-extraction rather than becoming
graph relationships. The existing extractor can omit unresolved type-reference and
implementation edges. General dot-call dispatch retains its broader competing-trait
checks, so an unrelated unmodelled import may still withhold receiver proof.

Non-`use` items also affect binding proof. An unknown procedural attribute or
`cfg_attr` on a function, struct, constant or other sibling item in a block can
introduce a new binding, including a `let` that shadows an existing same-block
function. Unqualified binding lookups through that block therefore withhold
ID/file proof. Calls retain raw references; existing unresolved type-reference and
implementation endpoints can still be omitted. Explicit qualified paths can retain
their supported proof. Custom derives are treated as possible expansion, including
derive spellings such
as `Debug` and `Clone` that can be shadowed by imported procedural macros. A separate
unaffected block can still prove its supported calls.

Known inert, unqualified built-in item attributes, such as `inline`, `repr` and
diagnostic attributes, do not suppress unrelated sibling binding proof. This does
not relax the conservative policy for an attributed caller or target declaration.
Scoped, raw or unrecognized attribute forms remain uncertain.

At module scope, supported direct free-function and static-member proofs can remain:
expansion cannot replace an existing same-scope written binding with another item
of that name in a valid Rust program without a duplicate-definition error. However,
item or derive expansion can supply competing trait implementations. Dot-call proof
is withheld when those candidates are unmodelled, including in a verified external
target file; an otherwise supported explicit static/UFCS call can retain proof.
These are bounded syntax rules, not a complete Rust macro or attribute evaluator.

Direct statement/item macro invocations in a lookup scope, including trait and implementation
member macros, withhold Rust target proof because they can introduce hidden bindings.
Macros nested inside another expression or a separate method body do not by themselves
suppress every proof in the caller's scope.

Rust globs, reexports, external crates, macro expansion, conditional-compilation
expansion, attributes on source call sites or target declarations, modules and
implementations, custom `#[path]` modules or Cargo target roots, cross-file imports
within inline-module contexts, and ambiguous
ownership between library and binary roots are unsupported. General receiver
inference, ordinary reference-typed receivers, enum/trait-object receivers,
dereference dispatch and trait overload selection are also outside this resolver.
Methods whose dispatch could select an unmodelled trait implementation remain
unresolved even when an inherent method has the same spelling. Attributed callers, including functions marked `#[test]`, retain
raw references without guessed target proof under this bounded attribute policy.

ECMAScript default imports, package imports, tsconfig aliases, local export lists,
reexports, runtime mutation and inferred receiver types are unsupported. A call
through an unsupported import or receiver keeps its reference text without creating
a guessed graph link. This is syntax-based static evidence rather than a replacement
for TypeScript's type checker or Rust's name/type resolution.

Potential direct `eval` withholds local/imported target proof throughout the caller
file, and declarations in a target file containing it cannot prove imported callees.
This includes eval inside nested functions, parentheses and erased TypeScript callee
wrappers. The gate is conservative: shadowed or optional `eval` spellings and escaped
bare identifier calls can also suppress proof. Member calls such as `obj.eval()`
and eval text inside strings/comments do not trigger this specific gate. Raw call
references and import candidate tracking remain available, so removing eval can
restore supported relationships. This is not a complete solver for indirect eval
aliases or runtime code generation.

## Index lifecycle

File-only evidence resolves a target by its proven file and qualified name. A target
signature change can therefore replace its symbol ID while retaining a legitimate
imported relationship, provided that exactly one matching declaration remains in
that file. ID evidence behaves differently: a stale or removed ID never reconnects
through a same-name or file match. Tracked target/module changes mark the extractions
that read them stale; re-extracting those source files supplies current proof rather
than silently rebinding an old ID.

Removing a target clears its graph neighbors while preserving the incoming raw
reference and its evidence. Changed exports, missing files and newly ambiguous
module candidates are re-evaluated when their tracked readers are refreshed. Source
replacement refreshes outgoing edges and both neighbor directions. Import-only
source edits are re-extracted even when the caller's symbol ID and body are unchanged.

Visible `Cargo.toml` additions, edits and removals reach dependency refresh even though
they are metadata rather than source. Existing ignored, hidden or symlinked metadata
can be filtered before watcher refresh despite having been read by import resolution.
After manifest or layout changes involving such unobserved metadata, a full reindex
is required. Automatic stale-reader refresh applies to observed changes; this slice
does not bypass workspace ignore rules to watch metadata.

Schema 24 introduced explicit edge evidence. Core schema 25 supplies the internal
writer-version and per-write-generation layout. The semantic Rust target-proof
receipt is versioned separately: policy 2 covers the non-`use` item expansion rules
above as well as attributed imports. A writable open upgrades an older or missing
Rust-proof receipt to 2 even when core schema 25 is already installed, invalidating
historical Rust ID/file hints again. Core schema remains 25.

Invalidation preserves historical target spelling, symbol identities, SIR payloads,
history and contracts, rebuilds SQLite neighbors, and marks native projections for
replay. The historical spelling is not necessarily the original call literal: an
earlier import proof may already have rewritten it. A normal index/startup
re-extracts accepted source files, including unchanged files, to restore supported
evidence and current raw references. Until then, cleared Rust hints remain unresolved.
Neither a layout upgrade nor proof invalidation can reconstruct lexical bindings
from stored names.

Stop older Aether processes **before** a writable upgrade, then restart them with
the corrected build. SQLite triggers reject proof-bearing writes using an older
writer version or an invalid per-write generation, including those from an
already-open old connection. Unclassified legacy proof writes are also fenced so
publishing a Rust source cannot activate an unchecked orphan edge. Raw references
remain writable, and positively identified non-Rust sources retain their existing
resolution policy. This is a proof-write fence, not a general write fence: an old
compiled policy-1 `DELETE` can still remove edges and is not rejected by these
triggers. The fence does not make in-flight older processes or arbitrary SQL safe.

Readonly graph operations require core schema 25 and Rust-proof receipt 2. Symbol
and SIR reads remain available on supported older catalogs. SurrealDB also uses a
durable projection receipt: readonly opens refuse a pending rebuild; writable
opens clear structural projections and replay a consistent snapshot of canonical
nodes and proved edges before completing the receipt. Older native graph handles
cannot be retroactively replaced by a SQLite upgrade. These receipts do not provide
an atomic transaction across the SQLite and native databases. Replaying the native
projection alone does not re-extract source or restore cleared Rust evidence.

An absent proved edge is not evidence that a call does not exist. In particular,
attributes, unsupported dispatch and missing indexed declarations can leave real
calls unresolved. This correction preserves conservative graph resolution; it
does not establish whole-program caller completeness.

Symbol-free file-level references remain raw evidence rather than traversable symbol
nodes. Rust raw identifiers, generic implementation dispatch, and generic/turbofish call
syntax also remain unresolved in this bounded resolver. Bare-name lookup stops at
inline-module boundaries; explicit `self` and `super` paths retain their supported
lexical proofs. Parse errors can leave recoverable symbols and raw references, but
withhold target proofs. The pinned Rust grammar supports older extern-block syntax; modern
`unsafe extern` blocks remain outside its syntax support.

## Regression evidence

`crates/aether-store/tests/graph_provenance.rs` uses the real tree-sitter parser and
SQLite store. It reproduces duplicate helpers/types in unrelated projects, checks
local and explicitly imported calls, preserves raw unresolved references, exercises
ambiguous and missing imports, and checks edits/removal and cross-language guards.
The cases compare canonical graph dependencies, projected neighbors, SQLite call
chains/callers, file dependencies and coverage. Dashboard regressions in
`crates/aether-dashboard/src/tests/graph_provenance.rs` check identity-specific
callers in graph, search and context output, plus incoming orphan metrics.
`crates/aether-parse/tests/rust_provenance.rs` exercises nominal receiver and tuple
constructor positives alongside mutable, shadowed, generic and trait-ambiguous
negatives. MCP regressions preserve parsed usage-matrix and selected-symbol callers;
context regressions keep distinct proved targets that share a spelling, and watcher
regressions move imported targets after an edit outside the caller's symbol body.

The attributed-import regressions cover disabled and enabled conditions, local
IDs and cross-file hints, aliases, grouped `self`, comments, and type/implementation
endpoints. `crates/aether-parse/tests/rust_attribute_provenance.rs` adds block
function/struct/constant expansion, conditional attributes, custom and shadowed
derives, inert built-in controls, module free/static proofs and competing receiver
dispatch. Store upgrade tests reconstruct schema 24 and policy-1 schema-25 catalogs,
keep an old writer open across upgrade, and check repeated invalidation, proof-write
rejection and preservation of non-graph data.

Daemon regressions seed stale proof content and verify a normal unchanged-source
index refreshes it without replacing SIR or contracts. The non-`use` lifecycle cases
in `crates/aetherd/src/indexer/attribute_expansion_tests.rs` cover local ID and
external file proofs for attributed sibling functions, structs and constants.
They retain a separate unaffected control call, remove and restore the attribute,
and check proof restoration/withdrawal while preserving written identities, SIR
payloads and history, mirrors and contracts. Source edits may update freshness
metadata; they do not regenerate the retained explanations.
