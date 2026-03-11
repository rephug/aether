# Codex Prompt — Phase 8.15: TYPE_REF + IMPLEMENTS Edge Extraction

CRITICAL BUILD SETTINGS — use these for ALL cargo commands:
```
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

You are adding two new structural edge types to `aether-parse` so that
passive data definitions (structs, enums, traits, type aliases) get
connected to the functions that use them.

Read these files before writing any code:
- `docs/roadmap/phase_8_stage_8_15_edge_extraction.md` (the full spec)
- `docs/hardening/phase8_stage8_15_session_context.md` (session context)
- The ACTUAL source files listed in the source inspection section below

## PREFLIGHT

```bash
cd /home/rephu/projects/aether
git status --porcelain
# Must be clean. If dirty, STOP and report.
git pull --ff-only
```

Create branch and worktree:
```bash
git worktree add ../aether-phase8-edge-extraction -b feature/phase8-stage8-15-edge-extraction
cd /home/rephu/projects/aether-phase8-edge-extraction
```

## SOURCE INSPECTION

Before writing code, run these commands and verify the assumptions in
your reasoning. If any assumption is false, adapt your plan accordingly.

```bash
# Find where EdgeKind is defined — search the WHOLE workspace
grep -rn "enum EdgeKind" crates/

# Find where capture names are mapped to EdgeKind
grep -rn "edge_kind_from_capture" crates/

# Find ALL serialization/deserialization of edge kinds across workspace
grep -rn "\"calls\"\|\"depends_on\"" crates/

# Find any edge-kind filtering in the pipeline
grep -rn "Calls\|DependsOn\|calls\|depends_on" crates/aetherd/src/
grep -rn "Calls\|DependsOn\|calls\|depends_on" crates/aether-store/src/

# Find the tree-sitter query files
find crates/aether-parse/ -name "*.scm" -exec echo {} \; -exec cat {} \;

# Find how parsed edges flow into the store
grep -rn "ParsedEdge\|SymbolEdge\|upsert_edge" crates/aether-parse/
grep -rn "upsert_edge\|store_edge\|insert_edge" crates/aetherd/

# Check how the community pipeline reads edges
grep -rn "list_dependency_edges\|edge_kind" crates/aether-health/
```

Verify these assumptions (adapt if wrong):
1. `EdgeKind` enum exists with at least `Calls` and `DependsOn` variants
2. `rust_edges.scm` contains patterns for call and use/import extraction
3. `edge_kind_from_capture_name` maps tree-sitter capture suffix to EdgeKind
4. Edges flow from aether-parse → aetherd indexing → aether-store
5. The graph store accepts edge_kind as a string field
6. The community pipeline in aether-health reads ALL edges without filtering

**If you find edge-kind filtering anywhere in the pipeline** (aetherd,
aether-store, aether-health) that would block `"type_ref"` or
`"implements"` strings, fix it. This is explicitly permitted.

## IMPLEMENTATION

### Step 1: Extend EdgeKind

In the file where `EdgeKind` is defined (may be aether-parse, aether-core,
or elsewhere — use grep results), add two new variants:

```rust
pub enum EdgeKind {
    Calls,
    DependsOn,
    TypeRef,       // NEW: function uses type as parameter/return/field
    Implements,    // NEW: type implements trait
}
```

Update ALL match arms and serialization/deserialization for EdgeKind
across the ENTIRE workspace:
- The capture name mapping: `"type_ref"` → `TypeRef`, `"implements"` → `Implements`
- The string serialization: `TypeRef` → `"type_ref"`, `Implements` → `"implements"`
- Any Display, Debug, From, Into, or serde implementations
- Any match/if-let arms in aetherd, aether-store, aether-health
- Any edge-kind filtering in the indexing or query pipeline

Search exhaustively:
```bash
grep -rn "EdgeKind\|edge_kind" crates/
```

### Step 2: Extend rust_edges.scm

Add tree-sitter patterns for TYPE_REF and IMPLEMENTS. The exact syntax
depends on what the Rust tree-sitter grammar exposes. Inspect the grammar
node types first by printing the AST:

```rust
// In a test, print the tree-sitter AST for a snippet to see field names:
let tree = parser.parse(source, None).unwrap();
println!("{}", tree.root_node().to_sexp());
```

Add patterns for:
- Parameter types: `(parameter type: (type_identifier) @edge.type_ref)`
- Return types: `(function_item return_type: (type_identifier) @edge.type_ref)`
- Generic type arguments: `(type_arguments (type_identifier) @edge.type_ref)`
- Reference types: `(reference_type type: (type_identifier) @edge.type_ref)`
- impl trait: `(impl_item trait: (type_identifier) @edge.implements)`

**IMPORTANT:** These patterns may not be exactly right. Tree-sitter
grammars vary in their field names and nesting. You MUST test that the
patterns actually match by running the aether-parse unit tests. If a
pattern doesn't match, use the AST s-expression output to find the
correct field names and nesting, then iterate.

### Step 3: Type resolution, filtering, and deduplication

The tree-sitter query will capture EVERY type_identifier, including
stdlib types. These need to be filtered out.

In the edge extraction code, add:

1. **Skip list for common stdlib/external types:**
   ```rust
   const STDLIB_TYPES: &[&str] = &[
       "String", "str", "Vec", "HashMap", "HashSet", "BTreeMap",
       "BTreeSet", "Option", "Result", "Box", "Arc", "Rc", "Mutex",
       "RwLock", "Cell", "RefCell", "Pin", "Cow", "PhantomData",
       "bool", "u8", "u16", "u32", "u64", "u128", "usize",
       "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64",
       "char", "Self", "Infallible", "Duration", "Instant",
       "PathBuf", "Path", "OsStr", "OsString",
   ];
   ```

2. **Resolution against symbol table:** For types not in the skip list,
   check if the type name matches any symbol extracted from the same
   file (or imported via `use` declarations). For path-qualified types
   like `crate::foo::Bar`, extract the final segment (`Bar`) and match
   against the symbol table. If no match, skip the edge.

3. **Deduplication:** Emit at most ONE TYPE_REF edge per
   `(source_function, target_type)` pair. If `fn foo(a: MyType, b: MyType)`
   appears, emit one edge foo → MyType, not two.

4. **Edge construction:** For resolved types, create a TYPE_REF edge
   from the ENCLOSING function/method → the type symbol. The enclosing
   function is the nearest ancestor `function_item` or method definition.

   For IMPLEMENTS, create an edge from the implementing type → the trait.
   Both must resolve to known symbols.

### Step 4: Ensure edges flow through the pipeline

Verify that new edge types flow through the existing indexing pipeline:
- aether-parse produces edges with the new kinds
- aetherd's indexing pipeline stores them via the store trait
- The graph store accepts the new edge_kind strings
- `list_dependency_edges()` returns them alongside CALLS and DEPENDS_ON

**If ANY filtering blocks the new edge kinds, fix it.** Check:
```bash
grep -rn "calls\|depends_on\|Calls\|DependsOn" crates/aetherd/src/
grep -rn "calls\|depends_on\|Calls\|DependsOn" crates/aether-store/src/
grep -rn "edge_kind" crates/aether-health/
```

## WHAT NOT TO CHANGE

- Community detection logic (planner_communities.rs) — zero changes
- Health scoring formulas
- Semantic rescue behavior
- The `[diag]` prints in ablation
- Container rescue, anchor split, bucketing, naming

**Exception:** Store/pipeline edge-kind filtering may be fixed if source
inspection proves it is necessary.

## TESTS

Add these unit tests in `aether-parse` (in the appropriate test module).
Each test should parse a Rust source snippet and verify the extracted edges.

**Struct targets:**

1. **`type_ref_extracted_from_function_parameter`**
   Parse: `struct MyStruct {} fn foo(bar: MyStruct) {}`
   Assert: one TYPE_REF edge from foo → MyStruct

2. **`type_ref_extracted_from_return_type`**
   Parse: `struct MyStruct {} fn foo() -> MyStruct { todo!() }`
   Assert: one TYPE_REF edge from foo → MyStruct

3. **`type_ref_extracted_from_generic`**
   Parse: `struct MyStruct {} fn foo() -> Vec<MyStruct> { todo!() }`
   Assert: TYPE_REF edge from foo → MyStruct (not Vec)

4. **`type_ref_extracted_from_reference`**
   Parse: `struct MyStruct {} fn foo(bar: &MyStruct) {}`
   Assert: TYPE_REF edge from foo → MyStruct

**Enum, trait, and type alias targets:**

5. **`type_ref_extracted_to_enum`**
   Parse: `enum MyEnum { A, B } fn foo(e: MyEnum) {}`
   Assert: TYPE_REF edge from foo → MyEnum

6. **`type_ref_extracted_to_type_alias`**
   Parse: `type MyAlias = u32; fn foo(x: MyAlias) {}`
   Assert: TYPE_REF edge from foo → MyAlias

7. **`type_ref_extracted_to_trait_object`**
   Parse: `trait MyTrait {} fn foo(t: &dyn MyTrait) {}`
   Assert: TYPE_REF edge from foo → MyTrait
   (If tree-sitter grammar doesn't support `dyn Trait` matching cleanly,
   document the gap and skip this test with a comment explaining why.)

**Path-qualified and nested generics:**

8. **`type_ref_resolves_scoped_type`**
   Parse a snippet with a `use` import and a function parameter using
   the imported type. Assert: edge resolves to the correct symbol.

9. **`type_ref_extracts_inner_from_nested_generic`**
   Parse: `struct MyStruct {} fn foo() -> Option<Result<MyStruct, String>> { todo!() }`
   Assert: TYPE_REF edge from foo → MyStruct (not Option, not Result)

**Deduplication:**

10. **`type_ref_dedupes_repeated_same_type`**
    Parse: `struct MyType {} fn foo(a: MyType, b: MyType) -> MyType { todo!() }`
    Assert: exactly ONE TYPE_REF edge from foo → MyType

**Filtering:**

11. **`type_ref_skips_stdlib_types`**
    Parse: `fn foo(s: String, n: u32) {}`
    Assert: zero TYPE_REF edges

12. **`type_ref_skips_unresolved_types`**
    Parse: `fn foo(x: SomeExternalType) {}` (no definition in scope)
    Assert: zero TYPE_REF edges

13. **`multiple_type_refs_from_one_function`**
    Parse: `struct A {} struct B {} struct C {} fn foo(a: A, b: B) -> C { todo!() }`
    Assert: 3 TYPE_REF edges: foo→A, foo→B, foo→C

14. **`type_ref_and_calls_coexist`**
    Parse a function that both takes a project type as parameter AND calls
    another function. Assert: both CALLS and TYPE_REF edges present.

**IMPLEMENTS:**

15. **`implements_extracted_from_impl_trait`**
    Parse: `trait Store {} struct SqliteStore {} impl Store for SqliteStore {}`
    Assert: one IMPLEMENTS edge from SqliteStore → Store

16. **`implements_no_edge_for_bare_impl`**
    Parse: `struct Foo {} impl Foo { fn bar(&self) {} }`
    Assert: zero IMPLEMENTS edges

## VALIDATION GATE

```bash
cargo fmt --check
cargo clippy -p aether-parse -p aether-store -p aether-health -p aetherd -- -D warnings
cargo test -p aether-parse
cargo test -p aether-store
cargo test -p aether-health
cargo test -p aetherd
```

Then build the release binary, re-index, and run ablation:

```bash
# Build release binary
cargo build -p aetherd --release

# Re-index to pick up new edge types — MUST use --full
rm -f /home/rephu/projects/aether/.aether/graph/LOCK
$CARGO_TARGET_DIR/release/aetherd --workspace /home/rephu/projects/aether --index-once --full

# Verify new edges exist (quick sanity check)
# e.g., query the store or check index output for "type_ref" edges

# Run ablation
rm -f /home/rephu/projects/aether/.aether/graph/LOCK
grep vector_backend /home/rephu/projects/aether/.aether/config.toml
# Must say "sqlite"

cargo test -p aether-health -- ablation_aether_store --ignored --nocapture 2>&1
cargo test -p aether-health -- ablation_aether_mcp --ignored --nocapture 2>&1
cargo test -p aether-health -- ablation_aether_config --ignored --nocapture 2>&1
```

### Ablation acceptance criteria

Check the `[diag]` output and compare against 8.14 baseline:

| Metric | 8.14 baseline | 8.15 target |
|--------|--------------|-------------|
| Orphan/loner count | ~50 | < 10 |
| Largest connected component (reps) | 66 | < 50 |
| Communities (row 6, aether-store) | 15 | >= 10 |
| Largest community | 93 | < 100 |
| Stability | 0.33 | >= 0.80 |

Cross-crate: aether-mcp and aether-config ablations must not regress
materially from their current values.

If the ablation shows improvement but doesn't fully meet targets, commit
anyway and print the full output. The diagnostic data determines whether
FIELD_ACCESS is needed as a follow-up.

## COMMIT

```bash
git add -A
git commit -m "Add TYPE_REF and IMPLEMENTS edge extraction to aether-parse

- Extend EdgeKind with TypeRef and Implements variants
- Add tree-sitter query patterns in rust_edges.scm for parameter types,
  return types, generic type arguments, reference types, nested generics,
  and impl trait declarations
- Filter stdlib/external types via skip list + symbol table resolution
- Deduplicate TYPE_REF edges per (source, target) pair
- Support struct, enum, trait, and type alias targets
- Fix any pipeline edge-kind filtering that blocked new types
- New edges flow through existing storage pipeline to community detection
- Expected impact: ~50 orphan passive record types gain structural edges,
  reducing loner count and improving Louvain partition stability"
```

Do NOT push. Robert will review the ablation output first.
