PROMPT 0 — generate ALL Phase/Stage docs in the repo (one shot)

Paste into Codex:

You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and tell me what is dirty.
2) Create a new git branch named chore/roadmap-docs off main.
3) Create a git worktree at ../aether-roadmap-docs for that branch and switch into it.
4) Create a docs/roadmap/ directory with these markdown files (create folders as needed):
   - docs/roadmap/README.md (index that links every file below)
   - docs/roadmap/phase_1_observer.md
   - docs/roadmap/phase_1_stage_1_indexing.md
   - docs/roadmap/phase_1_stage_2_sir_generation.md
   - docs/roadmap/phase_1_stage_3_1_search_lexical.md
   - docs/roadmap/phase_1_stage_3_2_robustness.md
   - docs/roadmap/phase_1_stage_3_3_sir_sqlite_source_of_truth.md
   - docs/roadmap/phase_1_stage_3_4_semantic_search_embeddings.md
   - docs/roadmap/phase_1_stage_3_5_cli_ux_and_config.md
   - docs/roadmap/phase_1_stage_3_6_mcp_and_lsp_ux.md
   - docs/roadmap/phase_1_stage_3_7_vscode_extension_polish.md
   - docs/roadmap/phase_1_stage_3_8_ci_release_packaging.md
   - docs/roadmap/phase_2_historian.md
   - docs/roadmap/phase_2_stage_2_1_sir_versioning.md
   - docs/roadmap/phase_2_stage_2_2_git_linkage.md
   - docs/roadmap/phase_2_stage_2_3_why_queries.md
   - docs/roadmap/phase_3_ghost.md
   - docs/roadmap/phase_3_stage_3_1_host_verification.md
   - docs/roadmap/phase_3_stage_3_2_container_verification.md
   - docs/roadmap/phase_3_stage_3_3_microvm_optional.md

5) Each file must include:
   - Purpose (plain English)
   - What’s in scope / out of scope
   - “Pass criteria” that is testable
   - The exact Codex prompt(s) for that stage (worktree + branch + implement + tests + commit)
6) Use the repo README and crate layout as the source of truth. Do not invent crates that do not exist.
7) Keep docs concise and executable: prompts should be copy/paste ready.
8) Run a quick link check: ensure README.md references docs/roadmap/README.md in a new “Roadmap” section.
9) Do not implement any product code in this prompt. Only docs + README link.
10) Commit with message: "Add roadmap docs (phases and stages)".


PROMPT 1 — create a worktree + branch and do a quick repo scan

Paste into Codex:

You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and tell me what is dirty.
2) Create a new git branch named feature/search-stage1 off main.
3) Create a git worktree at ../aether-search-stage1 for that branch and switch into it (so main stays untouched).
4) In the worktree, scan the repo to find:
   - where indexing happens (aetherd observer/index path),
   - where symbols/SIR are stored (aether-store),
   - where MCP tools are defined (aether-mcp),
   - any existing “search” or “query” functionality.
5) Print a short plan (max 10 lines) describing exactly which files you’ll change for Stage 4 “search-stage1”.
Do not implement yet.
Use rg and open files directly; do not guess.

PROMPT 2 — implement Search Stage 1 (lexical) end-to-end, with tests

Paste into Codex:

Implement "Search Stage 1 (lexical)" in the feature/search-stage1 worktree.

Goal:
- A user can search for symbols by name / qualified name / file path / language with a query string.
- Return top results with symbol_id, qualified_name, file_path, language, and a short snippet/summary if available.
- Expose it via BOTH:
  (A) aetherd CLI flag: --search "<query>"
  (B) aether-mcp new tool: aether_search

Constraints:
- Keep it local-first and dependency-light.
- Prefer SQLite FTS5 if available; otherwise implement a simple LIKE-based fallback.
- Index must be incremental: when symbols change, the search index updates.
- Must not break existing commands: --print-events, --print-sir, --lsp, --index, and MCP tools.
- Add unit/integration tests proving:
  1) indexing + search returns expected symbol
  2) rename updates the search index
  3) removing a symbol removes it from search results
- cargo test must pass workspace-wide.

Deliverables:
- Code changes + tests + docs update in README describing the new search feature and how to run it.
- Commit with message: "Add lexical search (Stage 1) via CLI and MCP"

PROMPT 3 — polish: rate-limit safety + “stale SIR” behavior

Paste into Codex:

Add robustness polish for production-ish use:

1) If inference fails or times out when generating SIR for a changed symbol, do NOT delete existing SIR.
   - Mark the symbol metadata as "sir_status = stale" (or similar) with last_error + last_attempt timestamp.
   - MCP/LSP should surface that status in responses.
2) Add a basic rate limiter / backoff for inference provider calls (even for local providers).
3) Add tests for the stale behavior (failure keeps old SIR and marks stale).

Do not introduce heavy dependencies.
cargo test must pass.
Commit: "Robust inference failure handling and rate limiting"




PHASE 1 — Stage 3.3: Make SQLite the SIR source of truth (no more “file-only” SIR)

Paste into Codex:

You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and tell me what is dirty.
2) Create a new git branch named feature/sir-sqlite-source-of-truth off main.
3) Create a git worktree at ../aether-sir-sqlite for that branch and switch into it.
4) Scan the repo (rg + open files) to find:
   - current SQLite schema for symbols and SIR metadata in crates/aether-store
   - where .aether/sir/<symbol_id>.json is written/read
   - how aetherd triggers store writes and how MCP/LSP reads SIR
5) Implement this change:
   - Store canonical SIR JSON inside SQLite as the PRIMARY source of truth.
   - Keep the .aether/sir/<symbol_id>.json files as an OPTIONAL mirror for debugging/back-compat.
   - Reads must prefer SQLite. If SQLite row missing but file exists, backfill SQLite.
   - Writes must write SQLite first, then (optionally) write the file mirror.
6) Add safe migrations:
   - Create/alter the SIR table to include sir_json TEXT, sir_hash TEXT, updated_at INTEGER (unix), and status fields if you already added them in earlier stages.
   - Migrations must be idempotent and work on existing DBs.
7) Tests:
   - Temp workspace: index with mock provider, confirm SQLite contains sir_json for at least one symbol.
   - Delete the .aether/sir file and confirm SIR still loads from SQLite.
   - Remove the SQLite sir_json for a symbol but keep the file and confirm it backfills into SQLite.
8) Keep dependencies minimal. cargo fmt --check, cargo clippy -- -D warnings, cargo test must pass.
9) Update README: “SIR storage” section describing SQLite as truth + optional file mirror.
10) Commit: "Store SIR JSON in SQLite as source of truth".

PHASE 1 — Stage 3.4: Semantic search (embeddings) layered on top of lexical search

Paste into Codex:

You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and tell me what is dirty.
2) Create a new git branch named feature/semantic-search-embeddings off main.
3) Create a git worktree at ../aether-semantic-search for that branch and switch into it.
4) Repo scan (rg + open files) to locate:
   - existing lexical search (Stage 3.1) code paths (store + CLI + MCP)
   - inference provider plumbing in crates/aether-infer
   - config loader in crates/aether-config
5) Implement semantic search as an OPTIONAL second mode:
   - Add a new embedding provider trait (keep it small and async).
   - Provide a deterministic MockEmbeddingProvider for tests (no network).
   - Provide one real embedding provider using the existing local endpoint pattern used by qwen3_local (if feasible), otherwise leave real provider behind a feature flag and default to mock unless configured.
   - Store embeddings in SQLite in a new table keyed by symbol_id, embedding_dim, embedding_blob, updated_at.
   - Compute embedding input from SIR canonical JSON (or intent + qualified_name + signature) so it’s stable.
   - Add CLI: --search-semantic "<query>" (or --search --mode semantic).
   - Add MCP tool: aether_search_semantic (or extend aether_search with mode parameter).
6) Incremental updates:
   - When a symbol’s SIR hash changes, recompute and upsert its embedding.
   - When a symbol is removed, delete its embedding.
7) Tests:
   - Use MockEmbeddingProvider.
   - Index → semantic search for a word that appears in a symbol’s SIR/intent returns that symbol.
   - Rename/remove updates semantic index correctly.
8) Keep it dependency-light. Prefer storing Vec<f32> as little-endian bytes; brute-force cosine/dot for top-N is acceptable for now.
9) cargo fmt --check, cargo clippy -- -D warnings, cargo test must pass.
10) Commit: "Add semantic search via embeddings (optional, mock-tested)".

PHASE 1 — Stage 3.5: CLI UX + Config hardening (make the tool feel “real”)

Paste into Codex:

You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and tell me what is dirty.
2) Create a new git branch named feature/cli-ux-config-hardening off main.
3) Create a git worktree at ../aether-cli-ux for that branch and switch into it.
4) Scan crates/aetherd and crates/aether-config for current CLI flags and config schema.
5) Implement:
   - Add aetherd commands that do exactly one thing and exit cleanly:
     --index-once (index then exit)
     --print-sir (already exists; ensure it exits predictably)
     --search / --search-semantic (if implemented) must exit after printing results
   - Make config creation and schema evolution robust:
     if .aether/config.toml is missing, create it (already stated in README); ensure it doesn’t overwrite user changes.
     add a [search] section with mode defaults and any knobs you introduced.
   - Add consistent JSON output option:
     --json for search output and status output (CLI only).
6) Tests:
   - Add CLI-level tests where feasible (spawn aetherd in temp workspace and assert exit codes and minimal output).
7) cargo fmt --check, cargo clippy -- -D warnings, cargo test must pass.
8) Update README quickstart commands to include --index-once and search usage.
9) Commit: "Harden CLI UX and config schema".

PHASE 1 — Stage 3.6: MCP + LSP UX upgrades (make stored intelligence easy to consume)

Paste into Codex:

You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and tell me what is dirty.
2) Create a new git branch named feature/mcp-lsp-ux-upgrades off main.
3) Create a git worktree at ../aether-mcp-lsp-ux for that branch and switch into it.
4) Scan crates/aether-mcp and crates/aether-lsp for current tool and hover responses.
5) Implement:
   - MCP: extend responses to include sir_status and last_error fields if present (from robustness stage).
   - MCP: add tool aether_search (lexical) if not already, and ensure it returns structured JSON with stable field names.
   - LSP: hover should show:
     first line: intent
     then small labeled sections (inputs/outputs/side_effects/error_modes) when present
     if sir_status is stale, show a warning line at the top
   - Add MCP tool aether_open_symbol that returns file_path + range/line info when available (so agents can open the code quickly).
6) Tests:
   - Unit tests for MCP JSON output shapes.
   - LSP hover formatting test: given a SirAnnotation, output contains the correct sections and stale warning.
7) cargo fmt --check, cargo clippy -- -D warnings, cargo test must pass.
8) Commit: "Improve MCP and LSP UX outputs".

PHASE 1 — Stage 3.7: VS Code extension polish (ship-worthy)

Paste into Codex:

You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and tell me what is dirty.
2) Create a new git branch named feature/vscode-extension-polish off main.
3) Create a git worktree at ../aether-vscode-polish for that branch and switch into it.
4) Scan vscode-extension/ to locate:
   - how it spawns aetherd/aether-lsp
   - how it passes --workspace and config
5) Implement:
   - Add a status bar item showing: AETHER (indexing / idle / stale warnings count).
   - Add command palette commands:
     "AETHER: Index Once"
     "AETHER: Search Symbols" (prompts user, shows quick pick, opens file)
   - Add setting to enable/disable semantic search if present.
6) Tests:
   - If extension already has test harness, add minimal tests; otherwise add a smoke script under vscode-extension that builds and validates activation.
7) Keep it minimal; do not change core protocol.
8) Commit: "Polish VS Code extension UX (status/search/commands)".

PHASE 1 — Stage 3.8: CI + release packaging (so other people can install)

Paste into Codex:

You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and tell me what is dirty.
2) Create a new git branch named feature/ci-release-packaging off main.
3) Create a git worktree at ../aether-ci-release for that branch and switch into it.
4) Scan .github/workflows and existing docs.
5) Implement:
   - CI workflow running on ubuntu-latest:
     cargo fmt --check
     cargo clippy -- -D warnings
     cargo test
   - Add a release workflow (manual dispatch is fine) that builds binaries for:
     linux x86_64
     windows x86_64
     and uploads artifacts
   - Add install docs in README (download artifact, run binary).
6) Commit: "Add CI and release packaging".

PHASE 2 — Historian (everything you need + prompts)
PHASE 2 — Stage 2.1: SIR versioning

Paste into Codex:

You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and tell me what is dirty.
2) Create a new git branch named feature/historian-sir-versioning off main.
3) Create a git worktree at ../aether-historian-v1 for that branch and switch into it.
4) Scan crates/aether-store and aetherd indexing path to find where SIR is written and how sir_hash is computed.
5) Implement:
   - Add a new SQLite table sir_versions:
     symbol_id TEXT
     sir_hash TEXT
     created_at INTEGER
     sir_json TEXT
     primary key (symbol_id, sir_hash, created_at) or a rowid with index on (symbol_id, created_at desc)
   - Whenever a symbol’s SIR hash changes, insert a version row.
   - Add aetherd CLI flag: --history <symbol_id> that prints versions newest-first.
   - Add MCP tool: aether_history {symbol_id, limit} returning versions with created_at and intent snippet.
6) Tests:
   - Temp workspace: index once (1 version), change function meaningfully, reindex (2 versions).
   - Verify versions persist after reopening store.
7) cargo fmt --check, cargo clippy -- -D warnings, cargo test must pass.
8) Commit: "Add historian: SIR versioning + history queries".

PHASE 2 — Stage 2.2: Git linkage (tie versions to commits)

Paste into Codex:

You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and tell me what is dirty.
2) Create a new git branch named feature/historian-git-linkage off main.
3) Create a git worktree at ../aether-historian-git for that branch and switch into it.
4) Scan how you store symbol file_path and language and any ranges/locations.
5) Implement:
   - Add optional commit_hash TEXT column to sir_versions.
   - On version insert, attempt to resolve current HEAD commit hash for the workspace and store it.
   - Add MCP tool: aether_symbol_timeline {symbol_id} returning versions with commit_hash when available.
   - Add CLI: --history <symbol_id> prints commit_hash if present.
6) Tests:
   - Create a temp git repo in tests, commit file, index, edit file, commit, index again.
   - Assert two versions have different commit hashes.
7) cargo fmt --check, cargo clippy -- -D warnings, cargo test must pass.
8) Commit: "Historian: link SIR versions to git commits".

PHASE 2 — Stage 2.3: “Why changed” queries (diff the meaning)

Paste into Codex:

You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and tell me what is dirty.
2) Create a new git branch named feature/historian-why-queries off main.
3) Create a git worktree at ../aether-historian-why for that branch and switch into it.
4) Implement:
   - Add store method to fetch two sir_json blobs for a symbol by version id or timestamps.
   - Add MCP tool: aether_why_changed {symbol_id, from, to} returning:
     fields_added, fields_removed, fields_modified (based on SIR JSON structure)
   - Add CLI: --why-changed <symbol_id> --from <ts> --to <ts> printing a readable summary.
5) Tests:
   - Make two SIR versions with known differences (mock provider change), verify diff output is correct and stable.
6) cargo fmt --check, cargo clippy -- -D warnings, cargo test must pass.
7) Commit: "Historian: why-changed diff over SIR versions".

PHASE 3 — Ghost (verification) prompts
PHASE 3 — Stage 3.1: Host verification (no sandbox yet)

Paste into Codex:

You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and tell me what is dirty.
2) Create a new git branch named feature/ghost-host-verification off main.
3) Create a git worktree at ../aether-ghost-host for that branch and switch into it.
4) Implement a verification runner that can execute configured commands in the workspace:
   - Config: [verify] commands = ["cargo test", "cargo clippy -- -D warnings"]
   - Add CLI: aetherd --verify (runs commands, returns exit code, prints logs)
   - Add MCP tool: aether_verify {commands?} returning structured results {command, exit_code, stdout, stderr}
5) Tests:
   - Use a temp workspace with a tiny Rust crate to verify command execution works.
   - Test failure case: command returns nonzero and output is captured.
6) Keep it local-only, no sandboxing.
7) cargo fmt --check, cargo clippy -- -D warnings, cargo test must pass.
8) Commit: "Ghost Stage 1: host-based verification runner".

PHASE 3 — Stage 3.2: Container verification (Docker baseline)

Paste into Codex:

You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and tell me what is dirty.
2) Create a new git branch named feature/ghost-docker-verification off main.
3) Create a git worktree at ../aether-ghost-docker for that branch and switch into it.
4) Implement optional Docker execution mode for verification:
   - Config: [verify] mode = "host" or "docker"
   - Docker mode runs commands inside a container image configured in config (default a basic rust image).
   - MCP tool aether_verify must include mode in results.
5) Tests:
   - Keep tests host-only (do not require Docker in CI).
   - Add code paths and unit tests for command construction and config parsing.
6) cargo fmt --check, cargo clippy -- -D warnings, cargo test must pass.
7) Commit: "Ghost Stage 2: docker verification mode (optional)".

PHASE 3 — Stage 3.3: MicroVM acceleration (optional)

Paste into Codex:

You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and tell me what is dirty.
2) Create a new git branch named feature/ghost-microvm-optional off main.
3) Create a git worktree at ../aether-ghost-microvm for that branch and switch into it.
4) Implement the architecture hooks ONLY (no fragile platform promises):
   - Add verify mode = "microvm" but mark it experimental.
   - Provide an interface for a microvm runner, with a stub implementation that returns a clear error unless enabled.
   - Document that Windows uses Hyper-V as baseline and Firecracker is Linux-first, and nested virt under WSL2 is not assumed.
5) No CI dependency on virtualization. Unit tests only.
6) cargo fmt --check, cargo clippy -- -D warnings, cargo test must pass.
7) Commit: "Ghost Stage 3: microvm runner interface (experimental stub)".