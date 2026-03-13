# Codex Prompt — Phase 8.18: Fix Boundary Leaker False Positives

CRITICAL BUILD SETTINGS — use these for ALL cargo commands:
```
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

You are modifying ONE file: `crates/aetherd/src/health_score.rs`.

Read these files before writing any code:
- `docs/roadmap/phase_8_stage_8_18_boundary_leaker_fix.md` (the spec)
- `docs/hardening/phase8_stage8_18_session_context.md` (session context)
- `crates/aetherd/src/health_score.rs` (THE ONLY FILE TO MODIFY)
- `crates/aether-health/src/planner_communities.rs` (read-only — understand the `detect_file_communities` public API)

Pay close attention to:
- `load_semantic_input` — the function being changed
- `compute_current_report` — the call site that needs config threading
- `build_file_symbol` — existing helper to build FileSymbol from SymbolRecord
- `detect_file_communities` — already imported, used by suggest-splits

## PREFLIGHT

```bash
cd /home/rephu/projects/aether
git status --porcelain
# Must be clean. If dirty, STOP and report.
git pull --ff-only
```

Create branch and worktree:
```bash
git worktree add ../aether-phase8-boundary-leaker -b feature/phase8-stage8-18-boundary-leaker-fix
cd /home/rephu/aether-phase8-boundary-leaker
```

## SOURCE INSPECTION

Before writing code, inspect `load_semantic_input` and verify these
assumptions in your reasoning. If any assumption is false, STOP and report:

1. `load_semantic_input` takes `workspace: &Path` and returns
   `Result<Option<SemanticInput>>`
2. It reads `community_by_symbol` from `store.list_latest_community_snapshot()`
3. It computes `community_count` per file by counting distinct community
   IDs for that file's symbols in the snapshot
4. `compute_current_report` calls `load_semantic_input(workspace)` and
   has access to `config: &AetherConfig`
5. `detect_file_communities` is already imported at the top of the file
6. `build_file_symbol` is already defined in this file
7. `FileCommunityConfig` is already imported at the top of the file
8. `SurrealGraphStore` or `GraphDependencyEdgeRecord` may need to be
   imported — check what's already available

## IMPLEMENTATION

### The change: Use file-scoped planner for community_count

**Step 1: Update `load_semantic_input` signature**

Add `config: &AetherConfig` parameter:
```rust
fn load_semantic_input(workspace: &Path, config: &AetherConfig) -> Result<Option<SemanticInput>>
```

Update the call site in `compute_current_report` to pass `config`.

**Step 2: Build FileCommunityConfig**

Inside `load_semantic_input`, after getting `centrality` and `store`,
build the planner config:
```rust
let planner_config = FileCommunityConfig {
    semantic_rescue_threshold: config.planner.semantic_rescue_threshold,
    semantic_rescue_max_k: config.planner.semantic_rescue_max_k,
    community_resolution: config.planner.community_resolution,
    min_community_size: config.planner.min_community_size,
};
```

This is the same pattern used in `collect_split_suggestion_entries`.

**Step 3: Get structural edges from SurrealDB**

After the centrality call returns (SurrealDB connection is dropped), open
the graph store again and query all edges:

```rust
let all_edges = runtime
    .block_on(async {
        let graph = SurrealGraphStore::open_readonly(workspace).await?;
        let records = graph.list_dependency_edges().await?;
        Ok::<_, anyhow::Error>(
            records
                .into_iter()
                .map(|record| GraphAlgorithmEdge {
                    source_id: record.source_symbol_id,
                    target_id: record.target_symbol_id,
                    edge_kind: record.edge_kind,
                })
                .collect::<Vec<_>>(),
        )
    })
    .unwrap_or_default();
```

If opening the graph store fails, fall back to empty edges — the planner
will still work, it just won't have structural information (all symbols
become loners, community_count = 0 or 1 per file).

**Step 4: Replace community_count computation**

Remove the `community_by_symbol` lookup from `list_latest_community_snapshot()`.

For each file in the centrality loop, compute community_count using the
planner:

```rust
let community_count = if symbols.is_empty() {
    0
} else {
    let symbol_id_set: HashSet<&str> = symbols
        .iter()
        .map(|s| s.id.as_str())
        .collect();
    let file_edges: Vec<GraphAlgorithmEdge> = all_edges
        .iter()
        .filter(|edge| {
            symbol_id_set.contains(edge.source_id.as_str())
                && symbol_id_set.contains(edge.target_id.as_str())
        })
        .cloned()
        .collect();
    let empty_embeddings = HashMap::new();
    let file_symbols: Vec<FileSymbol> = symbols
        .iter()
        .map(|record| build_file_symbol(&store, record, &empty_embeddings))
        .collect();
    let (assignments, _diagnostics) = detect_file_communities(
        file_edges.as_slice(),
        file_symbols.as_slice(),
        &planner_config,
    );
    assignments
        .iter()
        .map(|(_, community_id)| *community_id)
        .collect::<HashSet<_>>()
        .len()
};
```

**Step 5: Add necessary imports**

Check what's already imported. You may need to add:
- `SurrealGraphStore` from `aether_store`
- `GraphDependencyEdgeRecord` if not already imported (but you may not
  need the type explicitly if you destructure inline)

### What NOT to change

- Do not modify `community_snapshot` table or its writes in drift.rs
- Do not modify `semantic_signals.rs`, `archetypes.rs`, or `scoring.rs`
- Do not modify any file in `aether-health/`, `aether-analysis/`,
  `aether-config/`, or `aether-store/`
- Do not change the planner or its community detection logic
- Do not remove or change the `suggest_splits` code path

### Tests

1. **All existing tests must pass.** Run `cargo test -p aetherd` and
   verify no regressions.

2. **If any test seeds `community_snapshot` and then checks
   `community_count`**, update it to verify planner-derived counts
   instead. The test may need to seed structural edges so the planner
   can produce meaningful communities.

3. **Add test: `boundary_leakage_uses_planner_not_global_snapshot`**
   - Create a minimal workspace with SQLite and SurrealDB stores
   - Seed two files: File A with 6 symbols forming 2 disconnected
     clusters (3+3), File B with 4 fully connected symbols
   - Seed structural edges matching those clusters
   - Seed `community_snapshot` with WRONG data (all symbols in
     community 1) to prove the planner is used, not the snapshot
   - Call `load_semantic_input` and verify:
     - File A: `community_count >= 2`
     - File B: `community_count == 1`
   - This proves the planner drives the count, not the global snapshot

## VALIDATION GATE

```bash
cargo fmt --check
cargo clippy -p aetherd -- -D warnings
cargo test -p aetherd
```

Then run on the real workspace:

```bash
pkill -f aetherd
rm -f /home/rephu/projects/aether/.aether/graph/LOCK
cargo build -p aetherd --release

$CARGO_TARGET_DIR/release/aetherd health-score \
  --workspace /home/rephu/projects/aether \
  --semantic --output table 2>&1
```

### Validation criteria

1. All tests pass, zero clippy warnings
2. Boundary Leaker count: was 11/16, must be at most 5
3. Overall score: was 43, must improve (target: 48+)
4. No regression in structural scores for any crate
5. If the score doesn't improve or gets worse, STOP and report the full
   output — do not commit

## COMMIT

```bash
git add -A
git commit -m "Fix Boundary Leaker false positives: use file-scoped planner for community counts

- Replace global community_snapshot lookup in load_semantic_input with
  per-file detect_file_communities calls
- Structural edges from SurrealDB feed into the proven planner pipeline
  (test filtering, type-anchor rescue, container rescue, Louvain γ=0.5)
- community_count now reflects file-scoped responsibility boundaries,
  not global workspace clusters
- Boundary Leaker count drops from 11/16 to N (measured)
- Overall health score improves from 43 to M (measured)

Decision #89.1: boundary_leakage uses file-scoped planner communities"
```

Do NOT push. Robert will review the health-score output first.
