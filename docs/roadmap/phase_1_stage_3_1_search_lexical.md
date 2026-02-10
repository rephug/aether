# docs/roadmap/phase_1_stage_3_1_search_lexical.md

# Phase 1 — Stage 3.1: Search Stage 1 (Lexical)

## Goal
Add a first search capability that works immediately and locally:
- Search symbols by: name / qualified name / file path / language / (optional) SIR text snippet
- Return: symbol_id, qualified_name, file_path, language, and a short summary if available
- Expose search via:
  (A) aetherd CLI flag: --search "<query>"
  (B) MCP tool: aether_search

## Non-goals
- Vector embeddings / semantic retrieval
- Reranking
- UI polish
- Large refactors to indexing or storage

## Design constraints
- Dependency-light
- Prefer SQLite FTS5 where available, otherwise fallback to LIKE-based search
- Incremental: search index updates when symbols update
- Must not break existing CLI flags or MCP tools

## Revised pass criteria for this stage
This stage passes if ALL are true:

1) cargo fmt --check, cargo clippy -- -D warnings, cargo test all pass
2) In a temp workspace with Rust + TS:
   - After indexing, searching for a known symbol name returns it
   - After renaming a symbol, searching by old name does NOT return it; searching by new name DOES
   - After removing a symbol, it no longer appears in results
3) CLI:
   - `aetherd --workspace . --search "X"` prints stable output fields
4) MCP:
   - tool aether_search(query, limit) returns structured JSON results
5) No network required (mock providers only)

## Codex execution prompts (run in order)

### PROMPT 1 — create a worktree + branch and do a quick repo scan
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
5) Print a short plan (max 10 lines) describing exactly which files you’ll change for Stage 3.1 “search-stage1”.
Do not implement yet.
Use rg and open files directly; do not guess.

### PROMPT 2 — implement Search Stage 1 (lexical) end-to-end, with tests
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

## Merge guidance
After Prompt 2 passes:
- Switch back to main worktree and merge the branch normally.
- Keep the worktree directory for the next stage, or remove it after merging.
