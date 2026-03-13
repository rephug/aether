# Codex Prompt — Phase 8.20: Method-Attributed SIR Dependencies

CRITICAL BUILD SETTINGS — use these for ALL cargo commands:
```
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

Read the spec and session context first:
- `docs/roadmap/phase_8_stage_8_20_method_attributed_sir.md`
- `docs/hardening/phase8_stage8_20_session_context.md`

Then read these source files:
- `crates/aether-sir/src/lib.rs` (SirAnnotation struct, canonicalize_sir_json)
- `crates/aether-infer/src/sir_prompt.rs` (build_sir_prompt_for_kind, kind_specific_guidance, FEW_SHOT_EXAMPLES)
- `crates/aether-infer/src/sir_parsing.rs` (parse_and_validate_sir)
- `crates/aether-mcp/src/tools/sir.rs` (SirAnnotationView, aether_get_sir_logic)

## PREFLIGHT

```bash
cd /home/rephu/projects/aether
git status --porcelain
# Must be clean. If dirty, STOP and report.
git pull --ff-only
```

Create branch and worktree:
```bash
git worktree add -b feature/phase8-stage8-20-method-attributed-sir /home/rephu/aether-phase8-method-sir
cd /home/rephu/aether-phase8-method-sir
```

## SOURCE INSPECTION

Before writing code, verify these assumptions. If any is false, STOP and report:

1. `SirAnnotation` in `crates/aether-sir/src/lib.rs` has exactly these fields:
   `intent`, `inputs`, `outputs`, `side_effects`, `dependencies`, `error_modes`, `confidence`.
   There is NO existing `method_dependencies` field.

2. `SirAnnotation` derives `Serialize, Deserialize`. Adding an optional field
   with `#[serde(default, skip_serializing_if = "Option::is_none")]` is
   backward-compatible — existing JSON without the field deserializes to `None`.

3. `canonicalize_sir_json` in `crates/aether-sir/src/lib.rs` produces a
   deterministic JSON string with sorted keys. Check HOW it does this —
   does it use `serde_json::to_string` on the struct, or does it build
   a `serde_json::Value` and sort manually?

4. `parse_and_validate_sir` in `sir_parsing.rs` calls `serde_json::from_str`
   and then does field-level validation. Check what validation exists today.

5. `kind_specific_guidance` in `sir_prompt.rs` has a branch for
   `is_type_definition` (struct, enum, trait, type_alias). The new
   method_dependencies guidance goes in this branch.

6. `SirAnnotationView` in `crates/aether-mcp/src/tools/sir.rs` mirrors
   SirAnnotation fields. Check if it's a direct re-export or a separate struct.

7. `HashMap` is NOT currently imported in `crates/aether-sir/src/lib.rs`.
   You'll need `use std::collections::HashMap;`.

## CHANGE 1: SirAnnotation schema

In `crates/aether-sir/src/lib.rs`:

Add `use std::collections::HashMap;` if not present.

Add to `SirAnnotation` after the `confidence` field:

```rust
    /// Per-method dependency map for traits/structs. None for functions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method_dependencies: Option<HashMap<String, Vec<String>>>,
```

## CHANGE 2: Canonicalization

In `canonicalize_sir_json`, ensure `method_dependencies` is included
when `Some`. The canonical form must be deterministic:
- Method names (keys) sorted alphabetically
- Each method's dependency list sorted alphabetically
- If `None`, the field is omitted from the canonical JSON

Check how the current function works. If it serializes the struct directly,
`skip_serializing_if` handles omission. But the per-key sorting of the
HashMap needs explicit handling — either:
- Build a `BTreeMap` from the `HashMap` before serializing, or
- Sort in the `Value` representation

Choose whichever matches the existing canonicalization pattern.

## CHANGE 3: SIR prompt

In `crates/aether-infer/src/sir_prompt.rs`, in `kind_specific_guidance`,
in the `is_type_definition` branch, add AFTER the existing guidance:

```rust
sections.push(
    "If this type has methods (trait methods, impl methods): include a \
     \"method_dependencies\" field that maps each method name to its specific \
     dependencies as an array of strings. The flat \"dependencies\" array must \
     still contain the union of all method dependencies. Example:\n\
     \"method_dependencies\": {\n\
       \"upsert_symbol\": [\"SymbolRecord\", \"StoreError\"],\n\
       \"read_sir_blob\": [\"StoreError\"]\n\
     }\n\
     If the type has no methods (pure data struct, fieldless enum), omit \
     \"method_dependencies\" entirely."
        .to_owned(),
);
```

Update the struct few-shot example in `FEW_SHOT_EXAMPLES` to show
method_dependencies. Change the existing struct example to a trait
or impl-bearing struct. For example:

```
2) trait
{"intent":"Storage abstraction providing typed persistence operations for domain records","inputs":[],"outputs":[],"side_effects":["Persists records to underlying storage backend"],"dependencies":["Record","StorageError"],"error_modes":["Storage backend unavailable","Serialization failure"],"method_dependencies":{"save":["Record","StorageError"],"load":["Record","StorageError"],"delete":["StorageError"]},"confidence":0.91}
```

Keep the existing struct example if you want, but add the trait example
as example 4. Do not remove existing examples.

## CHANGE 4: SIR validation

In `crates/aether-infer/src/sir_parsing.rs`, in `parse_and_validate_sir`,
after the existing field validation:

```rust
// Validate method_dependencies if present
if let Some(ref md) = sir.method_dependencies {
    for (method_name, deps) in md {
        if method_name.is_empty() {
            return Err("method_dependencies contains empty method name".to_owned());
        }
        if deps.iter().any(|d| d.is_empty()) {
            return Err(format!(
                "method_dependencies[{method_name}] contains empty dependency string"
            ));
        }
    }
}
```

Do NOT enforce that method_dependencies values are subsets of the flat
dependencies list — the LLM may not be perfectly consistent, and we
don't want to reject otherwise valid SIRs over a consistency mismatch.

## CHANGE 5: MCP response

In `crates/aether-mcp/src/tools/sir.rs`, if `SirAnnotationView` is a
separate struct from `SirAnnotation`, add:

```rust
    pub method_dependencies: Option<HashMap<String, Vec<String>>>,
```

And map it in the conversion from `SirAnnotation` to `SirAnnotationView`.

If `SirAnnotationView` is just a type alias or re-export of `SirAnnotation`,
no change needed — it inherits the new field automatically.

## TESTS

In `crates/aether-sir/src/lib.rs` tests (or a new test module):

1. **Backward compat:** Deserialize a JSON string WITHOUT method_dependencies.
   Verify it produces `SirAnnotation` with `method_dependencies: None`.

2. **Round-trip:** Create a `SirAnnotation` with `method_dependencies: Some(...)`.
   Serialize, deserialize, verify equality.

3. **Canonical form:** Create two `SirAnnotation` with the same
   method_dependencies but different HashMap iteration order. Verify
   `canonicalize_sir_json` produces identical strings.

4. **Omission:** Create a `SirAnnotation` with `method_dependencies: None`.
   Serialize to JSON. Verify the output does NOT contain the key
   "method_dependencies".

In `crates/aether-infer/src/sir_parsing.rs` tests:

5. **Validation accepts:** Parse a JSON string with valid method_dependencies.
   Verify it passes validation.

6. **Validation rejects empty key:** Parse a JSON string with `"": ["Foo"]`
   in method_dependencies. Verify it returns error.

## VALIDATION

```bash
cargo fmt --all --check
cargo clippy -p aether-sir -- -D warnings
cargo test -p aether-sir
cargo clippy -p aether-infer -- -D warnings
cargo test -p aether-infer
cargo clippy -p aether-mcp -- -D warnings
cargo test -p aether-mcp
```

## COMMIT

```bash
git add -A
git commit -m "Add method_dependencies field to SIR for per-method dependency mapping on traits and structs"
```

Do NOT push automatically. Report commit SHA and wait for review.

Push command (after review):
```bash
git push -u origin feature/phase8-stage8-20-method-attributed-sir
```
