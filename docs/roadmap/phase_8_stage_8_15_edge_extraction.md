# Phase 8 — Stage 8.15: Enhanced Edge Extraction (TYPE_REF + IMPLEMENTS)

## Purpose

Add structural edge types to `aether-parse` that connect passive data
definitions (structs, enums, traits, type aliases) to the functions and
methods that use them. This eliminates the 10-30 loner symbols that
currently lack structural edges and depend on semantic rescue heuristics.

## Prerequisites

- Phase 8.12.2 merged (first-token bucketing, empty-stem guard, diagnostics)
- Phase 8.14 merged (semantic rescue stabilization)

## Evidence from 8.12.2 diagnostics

The `[diag]` prints proved that 10 loner symbols in aether-store are
passive record types: `SirMetaRecord`, `DriftResultRecord`,
`ProjectNoteRecord`, `CommunitySnapshotRecord`, etc.

These types have ZERO outgoing `CALLS` edges because they are data
definitions — they don't call anything. They are used as parameters,
return types, and field types by SqliteStore methods, but `aether-parse`
currently only extracts `CALLS` (call_expression) and `DEPENDS_ON`
(use_declaration / import_declaration).

### Current edge types (Phase 4.4)

| Edge type | Source | Extraction |
|-----------|--------|-----------|
| CALLS | `call_expression` | function A calls function B |
| DEPENDS_ON | `use_declaration` | file A imports symbol from file B |

### Proposed new edge types

| Edge type | Source | Extraction |
|-----------|--------|-----------|
| TYPE_REF | parameter types, return types, local variable types | function A uses type B as parameter → A → B |
| IMPLEMENTS | `impl Trait for Type` | Type → Trait |
| FIELD_ACCESS | `self.field.method()` | method → field's type |

### Impact on community detection

With TYPE_REF edges, `upsert_sir_meta(record: SirMetaRecord)` would
create an edge from `upsert_sir_meta` → `SirMetaRecord`. Since
`upsert_sir_meta` is already in the `sir_ops` community, `SirMetaRecord`
would have a structural edge into that community. Louvain would assign
it to `sir_ops` based on hard structural evidence instead of semantic
similarity heuristics.

**Expected result:** Loner count drops from 10 to near 0 for aether-store.
Semantic rescue becomes less critical because most passive definitions
already have structural edges. The pipeline becomes structurally complete.

## Scope

This stage modifies `aether-parse` tree-sitter walkers. It is a larger
scope than the community detection tuning in 8.12-8.14.

### Languages affected

- **Rust** (primary — aether-parse already has Rust tree-sitter grammar)
- **TypeScript/JavaScript** (secondary — if Python/TS support is active)
- **Python** (if Phase 5.2 is merged)

### Priority order

1. **TYPE_REF** — highest impact. Captures parameter types, return types,
   field types, generic type arguments. This alone resolves most loners.
2. **IMPLEMENTS** — `impl Trait for Type` edges. Important for trait-heavy
   codebases. Lower volume than TYPE_REF.
3. **FIELD_ACCESS** — `self.conn.execute()` creates edge to the field's
   type. Most complex to extract (requires type inference or heuristic
   matching). Defer if TYPE_REF + IMPLEMENTS are sufficient.

## Implementation approach

### TYPE_REF extraction (Rust)

Tree-sitter node types to match:

```
function_item → parameters → parameter → type_identifier
function_item → return_type → type_identifier
let_declaration → type → type_identifier
impl_item → type → type_identifier (the "self" type)
struct_expression → type → type_identifier
generic_type → type_identifier (for Vec<Foo>, Option<Bar>)
```

For each `type_identifier` found in a function/method body or signature:
1. Resolve the type name to a known symbol in the same file or via imports
2. If resolved, emit a `TYPE_REF` edge from the enclosing function → the type
3. If unresolved (external type like `String`, `Vec`, `HashMap`), skip

**Resolution strategy:** Match against the symbol table already built by
aether-parse. For same-file types, exact name match. For imported types,
follow `use` declarations to resolve the qualified name.

### IMPLEMENTS extraction (Rust)

Tree-sitter node types:

```
impl_item → trait → type_identifier
impl_item → type → type_identifier
```

For `impl Store for SqliteStore`:
- Emit edge: `SqliteStore` → `Store` (IMPLEMENTS)
- All methods inside the impl block already get CALLS edges from their
  call_expression nodes

### Edge storage

New edges go into the existing SurrealDB graph store alongside CALLS
and DEPENDS_ON. The `edge_kind` field already supports arbitrary strings.

```
edge_kind = "type_ref"      — for TYPE_REF
edge_kind = "implements"    — for IMPLEMENTS
edge_kind = "field_access"  — for FIELD_ACCESS (if implemented)
```

### Community detection integration

No changes needed in `planner_communities.rs`. The pipeline already
consumes all edges from `list_dependency_edges()`. New edge types will
automatically flow through structural edge collapse, container rescue,
semantic rescue, Louvain, and merge — because they are structural edges,
not synthetic semantic ones.

The `[diag]` prints will show the impact immediately: `after_structural_edges`
should show fewer components and fewer degree-0 reps.

## Scope guard

- Do NOT change community detection logic (planner_communities.rs)
- Do NOT change health scoring formulas
- Do NOT change semantic rescue behavior
- Do NOT change Store trait or implementations
- Do NOT change the edge storage schema (use existing edge_kind field)
- Do NOT attempt full type inference — use tree-sitter node matching
  with best-effort resolution against the existing symbol table

## Key files

```
crates/aether-parse/src/rust_walker.rs    — tree-sitter Rust walker (main changes)
crates/aether-parse/src/types.rs          — ParsedSymbol, ParsedEdge types
crates/aether-parse/src/lib.rs            — parser entry point
crates/aether-store/src/graph_surreal.rs  — edge storage (read-only ref, no changes needed)
```

## Tests

### Unit tests (aether-parse)
- `type_ref_extracted_from_function_parameter`
  `fn foo(bar: MyStruct)` → TYPE_REF edge from foo → MyStruct
- `type_ref_extracted_from_return_type`
  `fn foo() -> MyStruct` → TYPE_REF edge from foo → MyStruct
- `type_ref_extracted_from_generic`
  `fn foo() -> Vec<MyStruct>` → TYPE_REF edge from foo → MyStruct
- `type_ref_skips_stdlib_types`
  `fn foo(s: String)` → no TYPE_REF edge (String is external)
- `implements_extracted_from_impl_trait`
  `impl Store for SqliteStore` → IMPLEMENTS edge SqliteStore → Store
- `type_ref_resolves_imported_types`
  `use crate::MyStruct;` followed by `fn foo(m: MyStruct)` → correct edge

### Integration test (ablation validation)
- Re-run `ablation_aether_store` after re-indexing with new edge types
- Expected: loner count drops from 10 to 0-3
- Expected: community count stays in 10-16 range (may shift slightly
  due to new structural information)
- Expected: stability >= 0.90

## Decisions to lock

- **#75**: TYPE_REF edges are directional: function → type (the function
  "uses" the type, not vice versa)
- **#76**: External/stdlib types are not tracked (no edges to String, Vec,
  Option, Result, etc.)
- **#77**: Resolution is best-effort against the existing symbol table.
  Unresolved types produce no edge (not an error).
- **#78**: FIELD_ACCESS deferred unless TYPE_REF + IMPLEMENTS are
  insufficient to resolve loners

## Validation

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR

cargo fmt --check
cargo clippy -p aether-parse -p aether-health -p aetherd -- -D warnings
cargo test -p aether-parse
cargo test -p aether-health

# Re-index aether with new edge types
aetherd --workspace /home/rephu/projects/aether --index-once --full

# Run ablation with new edges
rm -f /home/rephu/projects/aether/.aether/graph/LOCK
cargo test -p aether-health -- ablation_aether_store --ignored --nocapture
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
