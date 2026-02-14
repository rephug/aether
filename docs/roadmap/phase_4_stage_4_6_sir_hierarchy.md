# Phase 4 - Stage 4.6: SIR Hierarchy (File + Module Rollup)

## Purpose
Implement the SIR hierarchy from Prospectus §7.1. Currently only leaf-level SIR exists (per-function, per-struct). The prospectus specifies three levels: Leaf → File → Module. File and module SIR are primarily deterministic aggregation of child nodes, with optional LLM touchup only when aggregation is ambiguous.

## Current implementation
- SIR is generated per extracted symbol (function, struct, enum, trait, class, interface, type alias)
- No file-level summary exists
- No module-level summary exists
- LSP hover only shows individual symbol SIR
- MCP only returns individual symbol SIR

## Target implementation
- **Level 1 (Leaf):** Unchanged — per-symbol SIR from inference provider
- **Level 2 (File):** Deterministic aggregation of all leaf SIR in a file:
  - Combined `exports` list (public symbols)
  - Merged `side_effects` (union of all leaf side effects)
  - Merged `dependencies` (union of all leaf dependencies)
  - Combined `error_modes`
  - Summary `intent` (concatenation of leaf intents, or LLM touchup if > 5 symbols)
- **Level 3 (Module):** Aggregation of file-level SIR for a directory:
  - Same merge strategy as file level
  - Only generated on demand (not on every file change)

## In scope
- Add `SirLevel` enum to `crates/aether-sir`: `Leaf | File | Module`
- Add `FileSir` struct with aggregated fields
- Add file-level SIR generation in `crates/aetherd/src/sir_pipeline.rs`:
  - After all leaf SIR for a file is generated, compute file-level rollup
  - Deterministic merge for side_effects, dependencies, error_modes (sorted union)
  - Intent: if ≤ 5 symbols, concatenate; if > 5, use inference provider for summary
- Store file-level SIR in `sir` table with synthetic ID: `BLAKE3("file:" + normalized_path)`
- Expose in LSP: hovering a file import shows file-level SIR
- Expose in MCP: `aether_get_sir` accepts `level: "leaf" | "file" | "module"` parameter
- Module-level SIR generated on demand only (MCP request or explicit CLI flag)

## Out of scope
- Automatic module-level regeneration on every change (too expensive)
- LLM-powered module summaries (keep deterministic for now)
- File-level SIR in vector search index (leaf only for now)

## Implementation notes

### File SIR aggregation (deterministic)
```rust
fn aggregate_file_sir(leaf_sirs: &[SirAnnotation]) -> FileSir {
    FileSir {
        intent: if leaf_sirs.len() <= 5 {
            leaf_sirs.iter().map(|s| &s.intent).join("; ")
        } else {
            // Use inference provider for summary
            generate_file_summary(leaf_sirs)
        },
        exports: leaf_sirs.iter()
            .map(|s| s.qualified_name.clone())
            .sorted().dedup().collect(),
        side_effects: leaf_sirs.iter()
            .flat_map(|s| s.side_effects.iter().cloned())
            .sorted().dedup().collect(),
        dependencies: leaf_sirs.iter()
            .flat_map(|s| s.dependencies.iter().cloned())
            .sorted().dedup().collect(),
        error_modes: leaf_sirs.iter()
            .flat_map(|s| s.error_modes.iter().cloned())
            .sorted().dedup().collect(),
        symbol_count: leaf_sirs.len(),
        confidence: leaf_sirs.iter()
            .map(|s| s.confidence)
            .fold(0.0f32, f32::min), // Worst-case confidence
    }
}
```

### Synthetic IDs
- File SIR: `BLAKE3("file:" + language + ":" + normalized_path)`
- Module SIR: `BLAKE3("module:" + language + ":" + normalized_dir_path)`

## Pass criteria
1. After indexing a file with 3 functions, a file-level SIR record exists in the `sir` table.
2. File SIR `side_effects` is the union of all leaf side effects (no duplicates).
3. File SIR `intent` concatenates leaf intents for small files.
4. MCP `aether_get_sir` with `level: "file"` returns the file rollup.
5. LSP hover on an import statement shows the imported file's SIR summary.
6. Module SIR is only generated on explicit request, not automatically.
7. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_4_stage_4_6_sir_hierarchy.md for full spec.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase4-stage4-6-sir-hierarchy off main.
3) Create worktree ../aether-phase4-stage4-6-hierarchy for that branch and switch into it.
4) Add SirLevel enum and FileSir struct to crates/aether-sir.
5) Implement deterministic file-level SIR aggregation in crates/aetherd/src/sir_pipeline.rs:
   - After leaf SIR generation, aggregate all leaf SIR for the file
   - Union side_effects, dependencies, error_modes (sorted, deduped)
   - Concatenate intents for ≤ 5 symbols, use inference for > 5
6) Store file SIR with synthetic ID in sir table via crates/aether-store.
7) Add level parameter to aether_get_sir MCP tool (leaf | file | module).
8) Add module-level on-demand aggregation (only when explicitly requested).
9) Add tests:
   - File with 3 functions → correct aggregated side_effects
   - File SIR intent concatenation for small files
   - MCP level parameter returns correct SIR level
   - Module SIR only generated on request
10) Run:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test --workspace
11) Commit with message: "Add file and module SIR hierarchy rollup".
```

## Expected commit
`Add file and module SIR hierarchy rollup`
