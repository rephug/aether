# Phase 8.15 — Enhanced Edge Extraction — Session Context

**Date:** 2026-03-11
**Branch:** `feature/phase8-stage8-15-edge-extraction` (to be created)
**Worktree:** `/home/rephu/projects/aether-phase8-edge-extraction` (to be created)
**Starting commit:** HEAD of main (after 8.14 merge)

## CRITICAL: Read actual source, not this document

```bash
# The live repo is at:
/home/rephu/projects/aether

# Always grep/read actual source before making claims about what exists
# Search the WHOLE workspace — EdgeKind may live outside aether-parse
grep -rn "EdgeKind" crates/
grep -rn "edge_kind_from_capture" crates/
grep -rn "\"calls\"\|\"depends_on\"" crates/
find crates/aether-parse/src/ -name "*.scm"
```

## Build environment (MUST be set for ALL cargo commands)

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

**Never run `cargo test --workspace`** — OOM risk. Always per-crate.

**Binary path:** `$CARGO_TARGET_DIR/release/aetherd` — do not assume
`aetherd` is on PATH.

## What just merged

- **Phase 8.12** — Community detection pipeline (planner_communities.rs)
- **Phase 8.12.2** — First-token bucketing, empty-stem guard, per-step diagnostics
- **Phase 8.13** — Symbol reconciliation + orphan cleanup for --full re-index
- **Phase 8.14** — Component-bounded semantic rescue (cross-component bridging blocked)

## The problem being solved

The community detection pipeline has ~50 orphan symbols in aether-store
that lack structural edges. These are passive data definitions (structs,
enums, type aliases) that are used as parameters and return types but
have zero CALLS edges because they don't call anything.

### 8.14 ablation evidence (aether-store, row 4 with rescue)

```
after_container_rescue: components=45 largest_component=109 rescued=8
after_semantic_rescue:  components=44 largest_component=110 rescued=1
connected_components:   count=3  sizes=[5, 8, 66]
```

The 66-rep component has no internal structure from structural edges.
Louvain partitions it chaotically — producing 7 or 16 communities
depending on γ (0.5 vs 0.6), a 2.3x swing that tanks stability to 0.33.

### Orphan diagnostic data (from 8.14 [diag] orphan_rescue)

- ~50 singleton orphans enter the rescue path
- 49/50 have their best match to another singleton (target_singleton=true)
- Only 1 orphan gets rescued (sim=0.9440 to non-singleton target)
- Many orphans have very high similarity (0.88-0.98) to other singletons
- These are passive record types: SirMetaRecord, DriftResultRecord,
  CommunitySnapshotRecord, enum types, config types, etc.

### Why edge extraction fixes this

`aether-parse` currently extracts only two edge types:
- `CALLS` — function A calls function B (from `call_expression`)
- `DEPENDS_ON` — file A imports from file B (from `use_declaration`)

Passive data types have zero CALLS edges. They need TYPE_REF:
- `fn upsert_sir_meta(record: SirMetaRecord)` → edge upsert_sir_meta → SirMetaRecord
- This gives SirMetaRecord degree > 0 from structural evidence
- Louvain assigns it to the correct community based on real edges

### Current edge extraction (from rust_edges.scm)

```
(call_expression) @edge.call
(use_declaration) @edge.depends_on
```

That's it — two patterns. This stage adds TYPE_REF and IMPLEMENTS.

## What to implement

1. Extend `EdgeKind` with `TypeRef` and `Implements` variants (search
   the WHOLE workspace to find where this enum lives)
2. Extend all serialization/deserialization and match arms across the
   workspace for the new variants
3. Extend `rust_edges.scm` with tree-sitter patterns for type references
   (parameters, return types, generics, references, nested generics)
   and impl trait declarations
4. Implement type resolution: match type_identifier names against the
   symbol table built in the same parse pass. Skip stdlib/external types.
   Deduplicate: one TYPE_REF edge per (source, target) pair.
5. Ensure new edges flow through the existing storage pipeline. If any
   edge-kind filtering is found that blocks the new types, fix it.
6. Support ALL passive definition kinds: structs, enums, traits, type
   aliases — not just structs.

The community detection pipeline (planner_communities.rs) needs ZERO
changes. It already consumes all edges from list_dependency_edges().

## Key files to inspect

```
# Search workspace-wide first:
grep -rn "EdgeKind" crates/
grep -rn "\"calls\"\|\"depends_on\"" crates/
grep -rn "edge_kind_from_capture" crates/

# Likely locations for edge extraction (verify):
crates/aether-parse/src/queries/rust_edges.scm  — tree-sitter query patterns
crates/aether-parse/src/parser.rs               — edge_kind mapping, parse logic
crates/aether-parse/src/types.rs                — EdgeKind enum, ParsedEdge type

# Pipeline flow (check for edge-kind filtering):
crates/aetherd/src/                              — indexing pipeline
crates/aether-store/src/graph_surreal.rs         — edge storage

# Read-only references:
crates/aether-health/src/planner_communities.rs  — community detection (unchanged)
```

## Scope guard (must NOT be modified)

- Community detection logic (planner_communities.rs)
- Health scoring formulas
- Semantic rescue behavior
- Edge storage schema (use existing edge_kind field)
- `[diag]` prints in ablation
- Container rescue, anchor split, bucketing, naming
- Coupling, drift, dashboard code

**Exception:** If source inspection reveals edge-kind filtering in
aetherd or aether-store that blocks the new types, minimal filter fixes
are permitted and expected.

## How to validate

```bash
# Unit tests
cargo fmt --check
cargo clippy -p aether-parse -p aether-store -p aether-health -p aetherd -- -D warnings
cargo test -p aether-parse
cargo test -p aether-store
cargo test -p aether-health

# Build release binary for re-index
cargo build -p aetherd --release

# Re-index to pick up new edge types
rm -f /home/rephu/projects/aether/.aether/graph/LOCK
$CARGO_TARGET_DIR/release/aetherd --workspace /home/rephu/projects/aether --index-once --full

# Ablation with new edges
rm -f /home/rephu/projects/aether/.aether/graph/LOCK
grep vector_backend /home/rephu/projects/aether/.aether/config.toml
# Must say "sqlite"

cargo test -p aether-health -- ablation_aether_store --ignored --nocapture 2>&1
cargo test -p aether-health -- ablation_aether_mcp --ignored --nocapture 2>&1
cargo test -p aether-health -- ablation_aether_config --ignored --nocapture 2>&1
```

## Acceptance criteria

After re-indexing with TYPE_REF + IMPLEMENTS edges:

- **Loner count (aether-store):** < 10 (was ~50)
- **Largest connected component:** < 66 reps (was 66)
- **Communities:** 10+ on aether-store
- **Largest community:** < 100
- **Stability:** >= 0.80 (was 0.33 — significant improvement expected)
- **All existing unit tests pass** across aether-parse, aether-store,
  aether-health, aetherd
- **aether-mcp and aether-config ablations must not regress**
- Zero clippy warnings

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

I'll paste Codex output, errors, and questions as they come up.
