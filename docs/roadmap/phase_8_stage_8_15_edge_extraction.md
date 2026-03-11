# Phase 8 — Stage 8.15: Enhanced Edge Extraction (TYPE_REF + IMPLEMENTS)

## Purpose

Add structural edge types to `aether-parse` that connect passive data
definitions (structs, enums, traits, type aliases) to the functions and
methods that use them. This eliminates the ~50 orphan symbols that
currently lack structural edges and depend on semantic rescue heuristics.

## Prerequisites

- Phase 8.12.2 merged (first-token bucketing, empty-stem guard, diagnostics)
- Phase 8.14 merged (component-bounded semantic rescue)
- Phase 8.13 merged (symbol reconciliation)

## Evidence from 8.14 diagnostics

The `[diag] orphan_rescue` prints from the 8.14 ablation proved the
exact scope of the problem on aether-store (5869 LOC, ~214 non-test
symbols):

### Orphan population (proven)

- **~50 singleton orphans** enter the semantic rescue absorption path
- **49/50 have their best match to another singleton target** — there
  are almost no non-singleton components for them to absorb into
- **Only 1 orphan** (`0d1e20...`, sim=0.9440 to non-singleton target
  118) gets rescued under the current rules
- The rest stay as loners or fall into a weakly-structured mega-component

### Community detection impact (proven)

```
Row 4 (+ rescue), after container rescue:
  components=45  largest_component=109  rescued=8

After semantic rescue (component-bounded, 8.14):
  components=44  largest_component=110  rescued=1

Connected components entering Louvain:
  count=3  sizes=[5, 8, 66]
```

The 66-rep component is the problem. Louvain produces 7 to 16
communities depending on γ (0.5 vs 0.6), a 2.3x swing that tanks
stability to 0.33. The component has no internal structure from
structural edges — all the passive record types are degree-0 within it,
giving Louvain nothing to partition against.

### Why TYPE_REF solves this

With TYPE_REF edges, `upsert_sir_meta(record: SirMetaRecord)` creates
a structural edge from `upsert_sir_meta` → `SirMetaRecord`. Since
`upsert_sir_meta` is already structurally connected to the `sir_ops`
cluster, `SirMetaRecord` gains structural degree > 0 and Louvain
assigns it based on hard structural evidence.

**Expected result:**
- Loner count drops from ~50 to near 0
- The 66-rep mega-component breaks into smaller, internally-connected
  pieces (each record type anchored to its consuming methods)
- Louvain γ-sensitivity decreases because partitions are structurally
  determined, not arbitrary
- Stability rises from 0.33 toward 0.90+ without touching rescue code

### Passive record types identified from diagnostics

These are the high-similarity orphans that TYPE_REF will structurally
connect (all had sim > 0.85 to their best match):

- `SirMetaRecord`, `DriftResultRecord`, `ProjectNoteRecord`
- `CommunitySnapshotRecord`, `MigrationRecord`
- Graph operation types, config struct types, enum types, etc.
- All are top-level data definitions with zero outgoing CALLS edges
- All are used as parameters/return types by SqliteStore methods

## Scope

This stage modifies `aether-parse` tree-sitter walkers and query files.
It is a larger scope than the community detection tuning in 8.12-8.14
but is well-contained to the parse crate (plus minimal pipeline fixes
if edge-kind filtering is found elsewhere).

### Languages affected

- **Rust** (primary — AETHER's own codebase, immediate ablation benefit)
- TypeScript/JavaScript support can follow later if needed

### Priority order

1. **TYPE_REF** — highest impact. Captures parameter types, return types,
   field types, generic type arguments. This alone resolves most orphans.
2. **IMPLEMENTS** — `impl Trait for Type` edges. Important for trait-heavy
   codebases. Lower volume than TYPE_REF but architecturally significant.
3. **FIELD_ACCESS** — `self.conn.execute()` creates edge to the field's
   type. Most complex to extract (requires type inference or heuristic
   matching). Defer unless TYPE_REF + IMPLEMENTS are insufficient.

## Implementation approach

### EdgeKind extension

Add new variants to `EdgeKind` (search the WHOLE workspace to find where
this lives — it may be in aether-parse, aether-core, or aether-store):

```rust
pub enum EdgeKind {
    Calls,
    DependsOn,
    TypeRef,       // NEW: function uses type as parameter/return/field
    Implements,    // NEW: type implements trait
}
```

Update ALL match arms, serialization, deserialization, and filtering
across the entire workspace. Search:
```bash
grep -rn "EdgeKind" crates/
grep -rn "\"calls\"\|\"depends_on\"" crates/
```

### TYPE_REF deduplication rule

Emit at most ONE TYPE_REF edge per `(source_symbol, target_symbol)` pair.
If a function references the same type multiple times (as parameter AND
return type, or multiple parameters of the same type), only one edge is
stored. This keeps edge weights sane and prevents signature noise.

### TYPE_REF extraction (Rust)

Extend `rust_edges.scm` with tree-sitter patterns for type references.
The key node types:

```
function_item → parameters → parameter → type → type_identifier
function_item → return_type → type_identifier
let_declaration → type → type_identifier
impl_item → type → type_identifier (the self type)
generic_type → type_identifier (for Vec<Foo>, Option<Bar>)
reference_type → type_identifier (for &Foo, &mut Bar)
type_arguments → type_identifier (for nested generics)
```

For each `type_identifier` found in a function/method signature or body:
1. Resolve the type name against known symbols in the same file
2. For path-qualified types like `crate::foo::Bar`, extract the final
   segment and match against the symbol table
3. For nested generics like `Option<Result<MyType, E>>`, recurse into
   type arguments to find project types
4. If resolved, emit a `TYPE_REF` edge from the enclosing function → type
5. If unresolved (stdlib type like `String`, `Vec`, `HashMap`), skip
6. Deduplicate: only one edge per (source, target) pair

**Resolution strategy:** Match against the symbol table already built by
aether-parse during the same parse pass. For same-file types, exact name
match against extracted symbols. For imported types, follow `use`
declarations to resolve the qualified name. Best-effort — unresolved
types produce no edge, not an error.

**Stdlib/external type skip list:** At minimum, skip these common types
that will never resolve to project symbols:
`String`, `str`, `Vec`, `HashMap`, `HashSet`, `BTreeMap`, `BTreeSet`,
`Option`, `Result`, `Box`, `Arc`, `Rc`, `Mutex`, `RwLock`, `Cell`,
`RefCell`, `Pin`, `Cow`, `PhantomData`, `bool`, `u8`, `u16`, `u32`,
`u64`, `u128`, `usize`, `i8`, `i16`, `i32`, `i64`, `i128`, `isize`,
`f32`, `f64`, `char`, `Self`, `Infallible`, `Duration`, `Instant`,
`PathBuf`, `Path`, `OsStr`, `OsString`

### IMPLEMENTS extraction (Rust)

Extend `rust_edges.scm` with:

```
impl_item → trait → type_identifier
impl_item → type → type_identifier
```

For `impl Store for SqliteStore`:
- Emit edge: `SqliteStore` → `Store` (IMPLEMENTS)
- Direction: the implementing type points to the trait it implements

### Edge storage

New edges go into the existing SurrealDB graph store alongside CALLS
and DEPENDS_ON. The `edge_kind` field already supports arbitrary strings.
No schema changes should be needed — just new values in the existing field.

**However:** if source inspection reveals any filtering in the pipeline
that only accepts `"calls"` and `"depends_on"`, fix it to also accept
`"type_ref"` and `"implements"`. This is explicitly permitted even though
the general scope guard says "do not change Store implementations" — small
filter fixes to unblock the new edge kinds are in scope.

### Community detection integration

No changes needed in `planner_communities.rs`. The pipeline already
consumes all edges from `list_dependency_edges()`. New edge types will
automatically flow through structural edge collapse, container rescue,
semantic rescue, Louvain, and merge — because they are structural edges,
not synthetic semantic ones.

The `[diag]` prints will show the impact immediately:
- `after_structural_edges` should show fewer components, more edges
- `connected_components` should show the 66-rep component split
- Loner count should drop significantly

## Scope guard

- Do NOT change community detection logic (planner_communities.rs)
- Do NOT change health scoring formulas
- Do NOT change semantic rescue behavior
- Do NOT change the edge storage schema (use existing edge_kind field)
- Do NOT attempt full type inference — use tree-sitter node matching
  with best-effort resolution against the existing symbol table
- Do NOT change the `[diag]` prints in ablation — they validate impact
- Store trait or implementations: do NOT change UNLESS source inspection
  reveals edge-kind filtering that blocks the new types. Minimal filter
  fixes are permitted.

## Key files

```
crates/aether-parse/src/queries/rust_edges.scm  — tree-sitter query (main change)
crates/aether-parse/src/parser.rs               — edge_kind mapping, parse logic
crates/aether-parse/src/types.rs                — EdgeKind enum, ParsedEdge type
crates/aether-store/src/graph_surreal.rs         — edge storage (read-only ref, unless filter fix needed)
```

**CRITICAL: Search the WHOLE workspace before writing code.** The file
layout may differ from this list:
```bash
grep -rn "EdgeKind" crates/
grep -rn "edge_kind_from_capture" crates/
grep -rn "\"calls\"\|\"depends_on\"" crates/
find crates/aether-parse/ -name "*.scm"
```

## Tests

### Unit tests (aether-parse)

**Struct targets:**
- `type_ref_extracted_from_function_parameter`
  `fn foo(bar: MyStruct)` → TYPE_REF edge from foo → MyStruct
- `type_ref_extracted_from_return_type`
  `fn foo() -> MyStruct` → TYPE_REF edge from foo → MyStruct
- `type_ref_extracted_from_generic`
  `fn foo() -> Vec<MyStruct>` → TYPE_REF edge from foo → MyStruct
- `type_ref_extracted_from_reference`
  `fn foo(bar: &MyStruct)` → TYPE_REF edge from foo → MyStruct

**Enum, trait, and type alias targets:**
- `type_ref_extracted_to_enum`
  `enum MyEnum { A, B } fn foo(e: MyEnum)` → TYPE_REF foo → MyEnum
- `type_ref_extracted_to_type_alias`
  `type MyAlias = u32; fn foo(x: MyAlias)` → TYPE_REF foo → MyAlias
- `type_ref_extracted_to_trait_object`
  `trait MyTrait {} fn foo(t: &dyn MyTrait)` → TYPE_REF foo → MyTrait
  (if tree-sitter grammar supports it — if not, document the gap)

**Path-qualified and nested generics:**
- `type_ref_resolves_scoped_type`
  Test with a path-qualified type like a use-imported type from another
  module. Verify the edge resolves to the correct symbol.
- `type_ref_extracts_inner_from_nested_generic`
  `fn foo() -> Option<Result<MyStruct, E>>` → TYPE_REF foo → MyStruct
  (not foo → Option, not foo → Result)

**Deduplication:**
- `type_ref_dedupes_repeated_same_type`
  `fn foo(a: MyType, b: MyType) -> MyType` → exactly ONE TYPE_REF edge

**Filtering:**
- `type_ref_skips_stdlib_types`
  `fn foo(s: String, n: u32)` → no TYPE_REF edges
- `type_ref_skips_unresolved_types`
  `fn foo(x: UnknownExternalType)` → no TYPE_REF edge
- `multiple_type_refs_from_one_function`
  `fn foo(a: TypeA, b: TypeB) -> TypeC` → 3 TYPE_REF edges
- `type_ref_and_calls_coexist`
  Function with both a project-type parameter and a call → both edges

**IMPLEMENTS:**
- `implements_extracted_from_impl_trait`
  `impl Store for SqliteStore` → IMPLEMENTS edge SqliteStore → Store
- `implements_no_edge_for_bare_impl`
  `impl Foo { fn bar(&self) {} }` → zero IMPLEMENTS edges

### Integration test (ablation validation — #[ignore])
- Re-run `ablation_aether_store` after re-indexing with new edge types
- Expected: loner count drops from ~50 to < 10
- Expected: largest connected component < 66 reps (currently 66)
- Expected: community count stays in 10-20 range
- Expected: stability >= 0.80 (target 0.90, significant improvement from 0.33)

## Decisions to lock

- **#75**: TYPE_REF edges are directional: function → type (the function
  "uses" the type, not vice versa)
- **#76**: External/stdlib types are not tracked (no edges to String, Vec,
  Option, Result, primitives, etc.)
- **#77**: Resolution is best-effort against the existing symbol table.
  Unresolved types produce no edge (not an error).
- **#78**: FIELD_ACCESS deferred unless TYPE_REF + IMPLEMENTS are
  insufficient to resolve orphans
- **#79**: TYPE_REF edges are deduplicated per (source, target) pair.
  Multiple references to the same type from one function emit one edge.

## Validation

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR

cargo fmt --check
cargo clippy -p aether-parse -p aether-health -p aether-store -p aetherd -- -D warnings
cargo test -p aether-parse
cargo test -p aether-store
cargo test -p aether-health

# Re-index aether with new edge types (use explicit binary path)
rm -f /home/rephu/projects/aether/.aether/graph/LOCK
$CARGO_TARGET_DIR/release/aetherd --workspace /home/rephu/projects/aether --index-once --full

# Run ablation with new edges
rm -f /home/rephu/projects/aether/.aether/graph/LOCK
cargo test -p aether-health -- ablation_aether_store --ignored --nocapture
cargo test -p aether-health -- ablation_aether_mcp --ignored --nocapture
cargo test -p aether-health -- ablation_aether_config --ignored --nocapture
```

## End-of-stage git sequence

```bash
cd /home/rephu/projects/aether-phase8-edge-extraction
git push origin feature/phase8-stage8-15-edge-extraction

# Create PR via GitHub web UI, then after merge:
cd /home/rephu/projects/aether
git switch main
git pull --ff-only
git worktree remove ../aether-phase8-edge-extraction
git branch -D feature/phase8-stage8-15-edge-extraction
git worktree prune
```
