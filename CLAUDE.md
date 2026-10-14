# AETHER — working on this repository

AETHER is a semantic intelligence engine for codebases. It parses every symbol with
tree-sitter, records a Semantic Intent Record (SIR) for it, keeps symbols, edges and
SIRs in SQLite plus a SurrealDB (SurrealKV) graph and a vector store, and serves the
results through an LSP, MCP tools, a CLI, a web dashboard and a Tauri desktop app.

This file is for agents changing AETHER itself. Projects that *use* AETHER get a
generated CLAUDE.md from `aetherd init-agent`; this one is hand-written.

## Facts that are easy to get wrong

- **Core schema: 21.** `PRAGMA user_version` in `crates/aether-store/src/schema.rs`;
  readers check `check_compatibility("core", 21)` (`aether-mcp`, `aether-dashboard`).
  A schema bump is a deliberate, reviewed change, never a side effect.
- **Agent template schema: 4.** `AETHER_AGENT_SCHEMA_VERSION` in `aether-core`; it is
  written into the files `init-agent` generates. Bump it when a generated template
  changes meaningfully.
- **MCP:** `aether-mcp` is a stdio server with 39 tools (40 with the `verification`
  feature, which adds `aether_verify`). `aether-query` serves the same tools read-only
  over streamable HTTP (`POST /mcp`) for a live index; there is no read-write
  MCP-over-HTTP server.
- **Languages parsed:** Rust, TypeScript/JavaScript, Python.
- Re-index from the CLI with `aetherd --workspace . --index-once` (`--full` for the
  SIR passes). There is no `aether_index_once` MCP tool.

## Workspace layout

17 crates under `crates/`:

- `aetherd` — CLI, daemon, indexer, SIR pipeline, batch and continuous jobs,
  `init-agent` templates (`src/templates/`)
- `aether-core` (symbol model, stable IDs), `aether-config`, `aether-parse`,
  `aether-sir`, `aether-infer` (inference and embedding providers)
- `aether-store` (SQLite, SurrealDB graph, vectors), `aether-graph-algo`,
  `aether-analysis`, `aether-health`, `aether-memory`
- `aether-mcp`, `aether-lsp`, `aether-query`, `aether-dashboard`, `aether-desktop`,
  `aether-document`

Specs live in `docs/roadmap/`; decisions in `docs/DECISIONS*.md` and
`docs/roadmap/DECISIONS_v4*.md`.

## Building and validating

Per-crate cargo only. Never `--workspace`: a full workspace build or test needs more
memory than many machines have. Run one cargo job at a time when several worktrees
share a target directory.

For every crate you change:

```bash
cargo fmt --all --check
cargo clippy -p <crate> --all-targets -- -D warnings
cargo test -p <crate>
```

For `aetherd`, also run clippy and tests with `--features dashboard`, as CI does.
`aether-desktop` needs the WebKitGTK/GTK development packages that CI installs.

Keep the target directory outside the repository (set `CARGO_TARGET_DIR`), and point
`TMPDIR` at a disk-backed directory if `/tmp` is small.

## Worktree flow

- One branch and one worktree per change, created next to the checkout, never inside it:
  `git worktree add -B <branch> ../<branch>`.
- Never commit to `main`. Push the branch and open a PR with a descriptive title and
  body; commit and PR text is indexed as project history, so explain the problem, the
  root cause, the fix, the tests and the risk.
- Reproduce first: write the failing test before the fix.

## SurrealKV is single-process

The SurrealKV graph store under `.aether/graph/` can be open in only one process at a
time. A running `aetherd` daemon holds it, so CLI commands that need the store can't
open it while the daemon runs (some detect the daemon and exit with guidance).

- Stop the daemon before such commands: `pkill -x aetherd` (`-x` matches the binary
  name only; `-f` would also kill your shell and any cargo job building aetherd).
- Tests create their own temporary workspaces; never point tests or a second process at
  a real `.aether` directory that a daemon is using.

## Code conventions

- No `unwrap()` or `expect()` in library code; `thiserror` in libraries, `anyhow` in
  binaries, and `.context(...)` on fallible calls.
- SIR JSON is canonical (sorted keys) and hashed; don't change hashing, prompt
  fingerprints or SIR format in a way that makes stored SIRs look stale.
- `aether-store` is called from many async tasks; watch lock scope and blocking I/O in
  async code.
- Don't add crates, swap out SurrealDB or tree-sitter, or recommend models and
  providers without an explicit request.
