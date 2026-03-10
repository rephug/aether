# Phase 8.12.2 — Large Anchor Intra-Partitioning — Session Context

I'm continuing work on AETHER, a Rust multi-crate workspace (~55K+ LOC). We're in Phase 8,
fixing the last community detection quality issue discovered during 8.12 ablation testing.

**Before answering any questions about the codebase, clone or inspect the actual source.
Do not rely on project knowledge files — they may be stale snapshots.**

```bash
# Repo is at:
/home/rephu/projects/aether

# Always grep/read actual source before making claims about what exists
```

## What just shipped

**Stage 8.12 — Community Detection Quality (merged):**
- Planner-local community detection with type-anchor pre-collapse, semantic rescue,
  stability checks, resolution-aware Louvain, improved naming, diagnostics

**Stage 8.12.1 — OpenAI-Compatible Embeddings + Embeddings-Only Reindex (merged):**
- `EmbeddingProviderKind::OpenAiCompat` for cloud embedding APIs
- `--embeddings-only` CLI flag for re-embedding without SIR regeneration
- Dedicated `SirPipeline::new_embeddings_only` initializer

**Post-merge validation completed:**
- 3361 symbols re-embedded with qwen3-embedding:8b (4096-dim) via OpenRouter
- Full ablation suite run on aether-store, aether-mcp, aether-config

## The problem this stage fixes

Ablation testing revealed that aether-store/src/lib.rs (5869 LOC, ~214 non-test symbols)
collapses from 7 communities to **2 giant blobs** (155 + 59 symbols) after rescue passes.
The target is 5-10 meaningful modules like `sir_ops`, `migration_ops`, `project_note_ops`.

### Root cause: identified and proven

We ran a controlled split test isolating container rescue vs semantic rescue:

```
3.  type-anchor only:      7 communities, 141 largest, 48 loners
3a. + container only:      2 communities, 155 largest,  0 loners  ← CULPRIT
3b. + semantic only:      21 communities, 116 largest,  6 loners  ← works well
4.  both rescues:          2 communities, 155 largest,  0 loners
```

**Container rescue** connects degree-0 symbols sharing the same qualified-name stem.
In aether-store, most symbols share the `SqliteStore::` stem, so container rescue
bridges ALL separate communities into one giant connected component.

But the deeper problem is upstream: **type-anchor pre-collapse unions ALL 141 SqliteStore
methods into one indivisible super-node**. Even if container rescue is fixed, Louvain
can't split what's already been merged.

### The fix: proven via prototype

We prototyped large-anchor intra-partitioning and tested it:

**Before (8.12 baseline):**
```
type-anchor:    7 communities, 141 largest, 48 loners
+ rescue:       2 communities, 155 largest,  0 loners
full pipeline:  2 communities, 155 largest,  0 loners
```

**After (prototype with anchor splitting + container exclusions):**
```
type-anchor:    8 communities, 114 largest, 63 loners
+ rescue:      17 communities,  90 largest,  0 loners
full pipeline: 16 communities,  92 largest,  0 loners
```

The SqliteStore blob is broken. 16 communities instead of 2, largest dropped from
155 to 92, zero loners. Module names show real domain tokens: `graph_ops`, `sir_ops`,
`table_ops`.

All 52 existing unit tests pass with the prototype applied.

## Key architectural insight

The current type-anchor rule ("type definition and impl methods MUST land in the same
community") is correct for small impls, enums, traits, and cohesive utility types.

It is **wrong for god-service structs** like `SqliteStore` with 141 methods spanning
unrelated domains (SIR management, project notes, migrations, embeddings, write intents,
graph edges, search, calibration).

**Policy change for this stage:** type-anchor provides an anchor namespace, but large
anchor groups (> threshold) are partitioned into sub-communities for planner output.
Small anchor groups remain hard-collapsed as before.

## Key files to inspect

```
crates/aether-health/src/planner_communities.rs  — the entire pipeline lives here:
  - build_anchor_groups()           — builds union-find anchor groups (line ~499)
  - apply_container_rescue()        — singleton-only container rescue (line ~554)
  - apply_semantic_rescue()         — degree 0-1 semantic rescue (line ~616)
  - run_detection()                 — main 10-step pipeline (called from detect_file_communities)
  - run_ablation_pass()             — ablation variant of the pipeline (line ~2200)
  - run_ablation_detection()        — wrapper that calls run_ablation_pass
  - DisjointSet                     — union-find implementation (line ~86)
  - SymbolEntry                     — internal symbol wrapper (line ~43)
  - WeightedGraph                   — edge container (line ~49)

crates/aether-health/src/planner.rs               — naming logic, suggest_split()
crates/aether-health/src/lib.rs                    — public exports
```

## Build environment for all cargo commands:

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

Never run `cargo test --workspace` — OOM risk. Always per-crate:

```bash
cargo fmt --check
cargo clippy -p aether-health -- -D warnings
cargo test -p aether-health
```

## Scope guard (must NOT be modified)

- Health scoring formulas (`combined_score`, `compute_crate_penalty`)
- Existing CLI commands, API endpoints, MCP tools, LSP hover
- Store trait or Store implementations
- Coupling mining, drift detection, dashboard panels
- Edge extraction in aether-parse
- Global community snapshot
- Planner naming logic in planner.rs (except consuming new split data)
- Small anchor groups (< threshold) — behavior must be identical to current

## Architecture decisions

- **#65**: Large anchor groups (> 20 methods) are partitioned into sub-communities
  by domain-token affinity. Small groups remain hard-collapsed.
- **#66**: Type anchor definitions (struct/enum/trait) attach to the largest bucket,
  not bucketed by their own name.
- **#67**: Split-anchor members are excluded from container rescue to prevent
  re-merging.
- **#68**: Both `run_detection` and `run_ablation_pass` are patched — the ablation
  harness must reflect the same pipeline behavior.

## Ablation evidence on disk

```
docs/hardening/ablation-0.6b.txt           — baseline with old 0.6b embeddings
docs/hardening/ablation-openrouter-8b.txt  — baseline with 8b embeddings (if saved)
```

## End-of-stage git sequence

```bash
cd /home/rephu/projects/aether
# This is a direct-to-main commit or short-lived branch, not a full worktree stage
git add -A
git commit -m "Split large anchor groups for community detection quality"
git push origin main
```

I'll paste Codex output, errors, and questions as they come up. Help me troubleshoot
and make decisions.
