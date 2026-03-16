# AETHER Validation Gates

## When to use

Run these gates before every commit. All three must pass. No exceptions.

## Gate sequence

Run in this order — stop on first failure, fix, re-run from the beginning:

### 1. Format check

```bash
cargo fmt --all --check
```

If it fails, fix with `cargo fmt --all` and re-check.

### 2. Clippy (per-crate)

```bash
cargo clippy -p <crate_name> -- -D warnings
```

Run for every crate that was modified. Common crate names:

```
aetherd          aether-core      aether-config
aether-parse     aether-sir       aether-infer
aether-store     aether-mcp       aether-analysis
aether-health    aether-memory    aether-query
aether-lsp       aether-dashboard aether-document
aether-graph-algo
```

For Phase 9, add: `aether-desktop`

**NEVER** run `cargo clippy --workspace` — it will OOM on this machine.

### 3. Tests (per-crate)

```bash
cargo test -p <crate_name>
```

Run for every crate that was modified. Same rule — never `--workspace`.

## After all gates pass

Stage and commit with a descriptive message:

```bash
git add -A
git commit -m "feat(phase9): <what this commit does, for semantic indexing>"
```

PR titles and bodies should include semantic context — AETHER indexes GitHub PR content for its own graph.

## Special validation for Phase 9 (Tauri)

For `aether-desktop`, also verify:

```bash
# Tauri config validation
cargo tauri info

# Check that the webview can be built
cargo tauri build --debug
```

## Quick validation for iterative development

When iterating rapidly on a single crate, you can run a tighter loop:

```bash
cargo fmt --all --check && cargo clippy -p <crate> -- -D warnings && cargo test -p <crate>
```

But before the final commit, re-run clippy and tests for ALL modified crates.

## Known test quirks

- Some tests require a running SurrealDB instance — these are integration tests and may be skipped in unit-test-only runs
- Tests that write to `.aether/` use `tempdir()` — they should not conflict with a real workspace
- The `aether-dashboard` crate tests require the `dashboard` feature: `cargo test -p aether-dashboard --features dashboard`
