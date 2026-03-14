# Codex Prompt — Phase 8.19a: Fix Usage Matrix Method Name Matching

CRITICAL BUILD SETTINGS — use these for ALL cargo commands:
```
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

Read the patch spec first:
- `docs/roadmap/phase_8_stage_8_19a_usage_matrix_patch.md`

Then read these source files:
- `crates/aether-mcp/src/tools/usage_matrix.rs` (the tool to fix)
- `crates/aether-mcp/src/tools/search.rs` (type-level aggregation — same fix needed)
- `crates/aether-parse/src/parser.rs` lines 610-627 (rust_call_target — understand WHY bare names exist)

## PREFLIGHT

```bash
cd /home/rephu/projects/aether
git status --porcelain
# Must be clean. If dirty, STOP and report.
git pull --ff-only
```

Create branch and worktree:
```bash
git worktree add -b fix/usage-matrix-bare-names /home/rephu/aether-fix-bare-names
cd /home/rephu/aether-fix-bare-names
```

## SOURCE INSPECTION

Before writing code, verify:

1. In `usage_matrix.rs`, find the loop that queries callers for each child
   method. Confirm it calls `store.get_callers(method.qualified_name)` or
   equivalent using the QUALIFIED name (e.g., `SqliteStore::upsert_symbol`).

2. Check the `symbol_edges` table for actual data. Run mentally or confirm:
   edges with `target_qualified_name = 'upsert_symbol'` (bare) exist,
   but edges with `target_qualified_name = 'SqliteStore::upsert_symbol'`
   (qualified) do NOT exist for method-dispatch calls.

3. In `search.rs`, find the type-level aggregation code for
   `aether_dependencies`. Confirm it has the same caller query pattern.

## IMPLEMENTATION

### Helper function

Add a bare-name extractor (in usage_matrix.rs or a shared location):

```rust
fn bare_name(qualified_name: &str) -> &str {
    qualified_name.rsplit("::").next().unwrap_or(qualified_name)
}
```

### Fix usage_matrix.rs

In the caller-query loop for each child method:

1. Query callers by qualified name (existing behavior)
2. Extract bare name via `bare_name(&method.qualified_name)`
3. If bare name differs from qualified name, also query callers by bare name
4. Merge results, deduplicate by source_id to avoid double-counting

### Fix search.rs type-level aggregation

Apply the same pattern in the aether_dependencies type-level aggregation
code where it queries callers for child methods.

### Tests

Update the usage_matrix integration test in `mcp_tools.rs`:

1. Create a test workspace where file A has a struct with methods, and
   file B calls those methods via `instance.method()` syntax (field
   expression calls that produce bare-name edges).
2. Verify the usage_matrix returns those callers with correct method
   attribution.

## VALIDATION

```bash
cargo fmt --all --check
cargo clippy -p aether-mcp -- -D warnings
cargo test -p aether-mcp
cargo clippy --workspace -- -D warnings
```

## COMMIT

```bash
git add -A
git commit -m "Fix usage_matrix and type-level dependencies: match bare method names from field-expression calls"
```

Do NOT push automatically. Report commit SHA and wait for review.

Push command (after review):
```bash
git push -u origin fix/usage-matrix-bare-names
```
