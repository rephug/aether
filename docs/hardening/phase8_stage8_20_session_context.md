# Phase 8.20 — Method-Attributed SIR Dependencies — Session Context

**Date:** 2026-03-13
**Branch:** `feature/phase8-stage8-20-method-attributed-sir` (to be created)
**Worktree:** `/home/rephu/aether-phase8-method-sir` (to be created)
**Starting commit:** HEAD of main after 8.19 merges

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

## The problem being solved

When AETHER generates a SIR for the `Store` trait (52 methods), the
`dependencies` field is:

```json
"dependencies": [
  "CalibrationEmbeddingRecord", "CommunitySnapshotRecord",
  "CouplingMiningStateRecord", "DriftAnalysisStateRecord", ...
]
```

This flat list is useless for decomposition — you can't tell which
methods reference which types. The AETHER-informed Codex run cited
these dependency lists as evidence for groupings, but it was circular
reasoning (describing current state, not prescribing ideal decomposition).

The fix: add a `method_dependencies` map that breaks it down per method.

## Key files to read

### SIR schema
- `crates/aether-sir/src/lib.rs` — `SirAnnotation` struct, `canonicalize_sir_json`

### SIR generation prompt
- `crates/aether-infer/src/sir_prompt.rs` — `build_sir_prompt_for_kind`, `kind_specific_guidance`, `FEW_SHOT_EXAMPLES`

### SIR parsing/validation
- `crates/aether-infer/src/sir_parsing.rs` — `parse_and_validate_sir`, `normalize_candidate_json`

### MCP response
- `crates/aether-mcp/src/tools/sir.rs` — `SirAnnotationView`, `aether_get_sir_logic`

## Current SirAnnotation struct

```rust
pub struct SirAnnotation {
    pub intent: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub side_effects: Vec<String>,
    pub dependencies: Vec<String>,
    pub error_modes: Vec<String>,
    pub confidence: f32,
}
```

Adding:
```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method_dependencies: Option<HashMap<String, Vec<String>>>,
```

## Current prompt structure

`build_sir_prompt_for_kind` outputs:
```
You are generating a Leaf SIR annotation.
Respond with STRICT JSON only...
Context: language, file_path, qualified_name
{FEW_SHOT_EXAMPLES}
{kind_specific_guidance}
Symbol text: {source}
```

`kind_specific_guidance` for type definitions currently says:
"describe WHY this type exists... List contained or extended types as
dependencies. Inputs and outputs should be empty arrays."

We add the method_dependencies instruction here.

## Scope guards

- Do NOT change the SIR schema version or force migration
- Do NOT change how function/method SIRs are generated (only traits/structs)
- Do NOT make method_dependencies required — it's always Optional
- The flat `dependencies` field stays — method_dependencies is additive
- Existing SIR JSON must deserialize without errors (backward compat)

## After this stage merges

```bash
git push -u origin feature/phase8-stage8-20-method-attributed-sir
# Create PR via GitHub web UI
# After merge:
git switch main
git pull --ff-only
git worktree remove /home/rephu/aether-phase8-method-sir
git branch -d feature/phase8-stage8-20-method-attributed-sir
```

Then re-generate SIRs for trait symbols to populate the new field:
```bash
aetherd --workspace . regenerate --below-confidence 1.0 --file crates/aether-store/src/lib.rs
```
