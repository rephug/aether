# Phase 8.19a: Fix Usage Matrix Method Name Matching

## Purpose

Patch for `aether_usage_matrix` to find callers of struct/trait methods.
The tool currently queries `symbol_edges WHERE target_qualified_name =
'SqliteStore::upsert_symbol'` and finds nothing. The actual CALLS edges
stored by tree-sitter use bare method names: `target_qualified_name =
'upsert_symbol'` (because `rust_call_target` extracts only the field
name from `store.method()` calls, not the receiver type).

The fix: for each child method, search symbol_edges for both the
qualified name (`SqliteStore::upsert_symbol`) AND the bare method
name (`upsert_symbol`). Deduplicate callers across both queries.

## Root Cause

In `crates/aether-parse/src/parser.rs`, `rust_call_target` handles
`field_expression` calls by extracting only the field name:

```rust
"field_expression" => {
    let field = callee.child_by_field_name("field")?;
    let text = node_text(field, source);
    // Returns "upsert_symbol", not "SqliteStore::upsert_symbol"
}
```

This is correct behavior for tree-sitter (no type inference available).
The fix belongs in the usage_matrix consumer logic, not in the parser.

## Changes

### `crates/aether-mcp/src/tools/usage_matrix.rs`

In the section that queries callers for each child method:

**Current:** Queries `store.get_callers(method.qualified_name)` which
searches for `SqliteStore::upsert_symbol`.

**Fixed:** For each child method, extract the bare name (everything
after the last `::`) and query callers for BOTH:
1. `store.get_callers(method.qualified_name)` — catches `SqliteStore::method()` calls
2. `store.get_callers(bare_method_name)` — catches `instance.method()` calls

Deduplicate callers by `(source_id, file_path)` to avoid double-counting
when both forms match.

```rust
fn bare_name(qualified_name: &str) -> &str {
    qualified_name.rsplit("::").next().unwrap_or(qualified_name)
}

// For each method:
let qualified = &method.qualified_name;
let bare = bare_name(qualified);

let mut callers = store.get_callers(qualified)?;
if bare != qualified {
    let bare_callers = store.get_callers(bare)?;
    // Merge, dedup by source_id
    let existing_ids: HashSet<_> = callers.iter().map(|e| &e.source_id).collect();
    for caller in bare_callers {
        if !existing_ids.contains(&caller.source_id) {
            callers.push(caller);
        }
    }
}
```

**Note on false positives:** Bare name matching means `upsert_symbol`
calls from ANY type (not just SqliteStore) will match. For most
codebases this is fine — method names are sufficiently unique that
false positives are rare. For common names like `new` or `default`,
there could be noise. Accept this tradeoff for v1 — the alternative
(type inference) is far more complex.

### Same fix in type-level dependency aggregation

The `aether_dependencies` type-level aggregation in `search.rs` has
the same issue for its child-method caller queries. Apply the same
bare-name fallback there.

## Files to Modify

| File | Change |
|------|--------|
| `crates/aether-mcp/src/tools/usage_matrix.rs` | Add bare-name caller query + dedup |
| `crates/aether-mcp/src/tools/search.rs` | Same bare-name fix in type-level aggregation |
| `crates/aether-mcp/tests/mcp_tools.rs` | Update tests to verify bare-name resolution |

## Pass Criteria

1. `aether_usage_matrix` for SqliteStore returns method_count: 59 with
   consumer_count >> 37 (most methods now have callers, not just open).
2. `aether_dependencies` for SqliteStore with aggregation returns
   callers from trait method usage, not just constructor calls.
3. Suggested clusters appear (methods with the same consumer sets group).
4. Existing tests pass (no regression on function-level dependencies).
5. `cargo fmt --all --check`, `cargo clippy -p aether-mcp -- -D warnings`,
   `cargo test -p aether-mcp` pass.

## Estimated Effort

Small patch — ~30 lines of code changes plus test updates.
