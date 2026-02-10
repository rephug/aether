# docs/roadmap/phase_2_historian.md

# Phase 2 — Historian (Why it changed)

## Goal
Explain change over time:
- When a symbol’s meaning changes (sir_hash changes), store versions.
- Link versions to git commits (later: PRs/issues).
- Provide “why changed” queries.

## Stage 2.1 — SIR Versioning (recommended first)
### Required behavior
- Every time sir_hash changes for a symbol:
  - insert a row into sir_versions with:
    symbol_id, sir_hash, created_at, canonical_sir_json
- Provide CLI/MCP to retrieve history for symbol_id

### Pass criteria
- Offline tests show: index → change symbol → reindex → 2+ versions exist
- Can retrieve versions after restart

## Stage 2.2 — Git linkage
### Required behavior
- For each symbol version, record nearest commit hash or last-modifying commit
- Provide CLI/MCP query: “latest commit for symbol” and “timeline for symbol”

### Pass criteria
- On a fixture repo with a few commits, history returns correct commit hashes

## Stage 2.3 — Surface in LSP/MCP
- LSP hover shows “last changed in <commit>”
- MCP tool returns structured timeline

## Non-goals
- Perfect blame accuracy Day 1
- PR/issue integration for every platform (start minimal)
