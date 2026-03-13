# Phase 8.21 — Trait Split Planner — Session Context

**Date:** 2026-03-13
**Branch:** `feature/phase8-stage8-21-trait-split-planner` (to be created)
**Worktree:** `/home/rephu/aether-phase8-trait-planner` (to be created)
**Starting commit:** HEAD of main after 8.20 merges

## CRITICAL: Read actual source, not this document

```bash
/home/rephu/projects/aether
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

## What just merged (recent history)

| Commit | What |
|--------|------|
| (pending) | Phase 8.20 — method_dependencies in SIR |
| (pending) | Phase 8.19 — usage_matrix tool, type-level deps, search fallback |
| (pending) | Store trait decomposition into 11 sub-traits |

## The problem being solved

AETHER's health planner can suggest file splits (community detection on
symbols within a file → suggested modules). But when the health score
flags `trait_method_max > 35`, there's no tool to suggest how to split
the trait. During the Store refactoring experiment, decomposition was
done manually by three AI agents arguing about method groupings. The
data to do this algorithmically already exists:

- `aether_usage_matrix` (8.19) produces consumer × method matrices
- `method_dependencies` in SIR (8.20) shows per-method type dependencies
- CALLS edges in symbol_edges show who calls what

The planner just needs to cluster methods by co-consumer patterns.

## Key files to read

### Existing planner
- `crates/aether-health/src/planner.rs` — `suggest_split` (560 lines, file-level planner)
- `crates/aether-health/src/planner_communities.rs` — `detect_file_communities`
- `crates/aether-health/src/planner_communities/` — rescue, anchors, merge, graph modules

### Usage matrix (from 8.19)
- `crates/aether-mcp/src/tools/usage_matrix.rs` — consumer matrix logic to reuse

### Health scoring
- `crates/aetherd/src/health_score.rs` — `--suggest-splits` CLI integration
- `crates/aether-mcp/src/tools/health.rs` — health MCP tools

### SIR (for method_dependencies)
- `crates/aether-sir/src/lib.rs` — `SirAnnotation` with method_dependencies
- `crates/aether-mcp/src/tools/sir.rs` — SIR lookup

## Architecture note

The usage_matrix tool (8.19) lives in aether-mcp and queries Store
directly. The trait split planner should live in aether-health (same
as the file planner) and take pre-computed data as input — it should
NOT import aether-mcp or call MCP tools.

The data flow is:
```
MCP tool (aether_suggest_trait_split)
  → gathers consumer matrix from Store (same logic as usage_matrix)
  → gathers method_dependencies from SIR (optional)
  → calls suggest_trait_split() in aether-health
  → returns structured suggestion

CLI (--suggest-splits)
  → gathers same data from Store
  → calls suggest_trait_split() in aether-health
  → formats as text output
```

The planner function is pure — it takes data in, returns suggestions out.
No Store access, no MCP calls, no side effects.

## Existing file planner pattern to follow

`suggest_split` in `planner.rs` takes:
- `file_path: &str`
- `crate_score: u32`
- `structural_edges: &[GraphAlgorithmEdge]`
- `symbols: &[FileSymbol]`
- `config: &FileCommunityConfig`

Returns `Option<(SplitSuggestion, PlannerDiagnostics)>`.

It calls `detect_file_communities` for the heavy lifting, then formats
the results into `SuggestedModule` entries with names derived from
ranked tokens.

The trait planner follows the same pattern: takes pre-computed data,
does clustering, returns structured suggestion. The clustering uses
bitvectors instead of community detection, but the naming and
formatting patterns should match.

## Scope guards

- Do NOT import aether-mcp from aether-health
- Do NOT add Store access to the planner function
- The planner function is pure: data in, suggestion out
- Do NOT change the existing `suggest_split` for files
- Do NOT change planner_communities
- The MCP tool gathers data and calls the planner; the CLI does the same
- Reuse the STOPWORDS list and token-ranking logic from the file planner

## After this stage merges

```bash
git push -u origin feature/phase8-stage8-21-trait-split-planner
# Create PR via GitHub web UI
# After merge:
git switch main
git pull --ff-only
git worktree remove /home/rephu/aether-phase8-trait-planner
git branch -d feature/phase8-stage8-21-trait-split-planner
```

Then validate against Store:
```bash
aetherd --workspace . health-score --suggest-splits --semantic
```
Verify that the trait split suggestion for Store produces clusters that
roughly match the 11-subtrait decomposition.
