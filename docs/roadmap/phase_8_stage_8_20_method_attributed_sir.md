# Phase 8.20: Method-Attributed SIR Dependencies

## Purpose

When AETHER generates a SIR for a trait or struct, the `dependencies` field
is a flat list of every type referenced anywhere in the definition. For a
52-method trait, this produces a 23-item list with no indication of which
methods reference which types. This makes the SIR useless for decomposition
planning — you can't derive method clusters from a flat union.

This stage adds an optional `method_dependencies` field to the SIR schema
for trait and struct symbols. The flat `dependencies` field is preserved
for backward compatibility. The new field maps each method name to its
specific dependency list.

## Prerequisites

- Phase 8.19 merged (usage_matrix tool proves the decomposition use case)
- Phase 8.17 merged (Gemini native provider — used for SIR regeneration)

## What Changes

### 1. SIR Schema Extension

In `crates/aether-sir/src/lib.rs`, add to `SirAnnotation`:

```rust
pub struct SirAnnotation {
    pub intent: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub side_effects: Vec<String>,
    pub dependencies: Vec<String>,
    pub error_modes: Vec<String>,
    pub confidence: f32,
    // NEW: per-method dependency map for traits/structs. None for functions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method_dependencies: Option<HashMap<String, Vec<String>>>,
}
```

`skip_serializing_if = "Option::is_none"` means existing SIRs (all functions,
plus old trait/struct SIRs) serialize identically. No migration needed.
The field is only present when the LLM includes it in the response.

### 2. SIR Prompt Change

In `crates/aether-infer/src/sir_prompt.rs`, modify `kind_specific_guidance`
for type definitions (`struct`, `trait`, `enum`):

Add this guidance when the symbol is a trait or struct with methods:

```
For traits and structs with methods: include a "method_dependencies" field
mapping each method name to its specific dependencies. Example:
"method_dependencies": {
  "upsert_symbol": ["SymbolRecord", "StoreError"],
  "read_sir_blob": ["StoreError"],
  "search_symbols_semantic": ["SemanticSearchResult", "StoreError"]
}
The flat "dependencies" field should still contain the union of all
method dependencies. "method_dependencies" provides the per-method
breakdown.
If the type has no methods (pure data struct, fieldless enum), omit
"method_dependencies" entirely.
```

Update the few-shot example for structs/traits to include the field.

### 3. SIR Validation

In `crates/aether-infer/src/sir_parsing.rs`, `parse_and_validate_sir`:

- `method_dependencies` is optional. If present, validate:
  - It is a JSON object (not array, not string)
  - Each key is a non-empty string
  - Each value is an array of strings
  - Every dependency in `method_dependencies` values also appears in
    the flat `dependencies` array (consistency check)
- If absent or null, set to `None`. Do not reject.

### 4. SIR Canonicalization

In `crates/aether-sir/src/lib.rs`, `canonicalize_sir_json`:

- If `method_dependencies` is `Some`, include it in canonicalization
  with keys sorted alphabetically and each value array sorted.
- If `None`, omit from canonical form.
- This means SIR hashes change for re-generated trait/struct symbols
  (expected — they have new content). Existing function SIRs keep
  identical hashes.

### 5. MCP Tool Enhancement

In `crates/aether-mcp/src/tools/sir.rs`, the `SirAnnotationView` struct
already mirrors `SirAnnotation` fields. Add:

```rust
pub method_dependencies: Option<HashMap<String, Vec<String>>>,
```

This surfaces naturally in `aether_get_sir` responses.

### 6. SIR Regeneration

After the prompt change, existing trait/struct SIRs won't have the field
until re-generated. This happens naturally via:
- `aetherd --workspace . --index-once --full` (re-generates everything)
- `aetherd --workspace . regenerate --below-confidence 1.0` (re-generates all)
- Normal watcher activity (re-generates on edit)

No forced migration. The field appears gradually as symbols are re-indexed.

## Files to Modify

| File | Change |
|------|--------|
| `crates/aether-sir/src/lib.rs` | Add `method_dependencies` field to `SirAnnotation`, update canonicalization |
| `crates/aether-infer/src/sir_prompt.rs` | Add method_dependencies guidance for type definitions, update few-shot |
| `crates/aether-infer/src/sir_parsing.rs` | Validate `method_dependencies` if present |
| `crates/aether-mcp/src/tools/sir.rs` | Add field to `SirAnnotationView` |

## Pass Criteria

1. `SirAnnotation` has `method_dependencies: Option<HashMap<String, Vec<String>>>`.
2. Existing SIR JSON without the field deserializes correctly (backward compat).
3. SIR prompt for traits/structs requests the method_dependencies field.
4. SIR prompt for functions does NOT request the field.
5. Validation accepts SIR with and without method_dependencies.
6. Validation rejects method_dependencies with non-string keys or non-array values.
7. Canonicalization includes method_dependencies when present, omits when None.
8. `aether_get_sir` response includes method_dependencies when available.
9. Generate a SIR for a trait (e.g., Store) and verify method_dependencies is populated.
10. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, per-crate tests pass.

## Estimated Effort

1 Codex run. The changes are small and contained — one new optional field
threaded through prompt, parsing, canonicalization, and MCP response.
