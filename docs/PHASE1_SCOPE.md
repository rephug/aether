# Phase 1 Scope — “The Observer” (MVP)

**Purpose:** Ship a local-first daemon + LSP server that keeps live, structured intent summaries of a codebase and exposes them in-editor (hover + search).  
**Primary platform:** Linux.  
**Windows:** supported, but **no Ghost sandbox** in Phase 1 (verification is Phase 3).  
**Intelligence path (Day 1):** Cloud-first via Gemini 3 Flash for SIR generation; no local ML dependencies in the core binary.

---

## What Phase 1 delivers

### A. `aetherd` daemon (Rust)
A background process that:
1. **Watches** a workspace for changes (debounced).
2. **Parses** changed files and extracts symbols (functions/classes/types) with stable IDs.
3. **Generates SIR** (Structured Intermediate Representation) for changed symbols via inference provider (Gemini 3 Flash Day 1).
4. **Stores** SIR + metadata locally and keeps it incrementally updated.
5. **Indexes** for search (lexical + vector).
6. **Serves** an LSP endpoint to editor clients.

### B. Editor integration (LSP-first)
Must support:
- **Hover:** show a short, clear SIR summary for the symbol under cursor.
- **Command/search:** return top results for a natural-language query (“semantic search”).

### C. Local data directory
AETHER creates and maintains a project-local directory:
- `./.aether/` (config + database + SIR cache + vector index)

---

## Supported languages (Phase 1)

**Minimum viable set (aligned with prospectus defaults):**
- TypeScript/JavaScript: `ts`, `tsx`, `js`, `jsx`
- Rust: `rs`

**Not in Phase 1 by default:** Python (becomes “standard tier” later per roadmap).

---

## Data products produced in Phase 1

### 1) SIR (Structured Intermediate Representation)
Per-symbol JSON containing (at minimum):
- `intent` (what this symbol is for)
- `inputs` / `outputs` (types + meaning)
- `side_effects` (DB / network / filesystem / process)
- `dependencies` (key calls/imports and what they mean)
- `errors` (throws/returns error, panics, etc.)
- `examples` (optional; short and safe)

### 2) Metadata index
Local metadata for:
- symbol ID, file path, language, range/offsets
- content hash of symbol text
- last SIR hash + version
- timestamps

### 3) Search indexes
- **Lexical:** token/name/path search for fast exact hits
- **Vector:** embeddings over canonicalized SIR for semantic search

---

## What Phase 1 explicitly does NOT include (Non-goals)

**No Historian (Phase 2):**
- No “why did this change?” commit/PR/ticket graph in Phase 1.

**No Ghost Runtime (Phase 3):**
- No sandbox compilation/typecheck/test verification of AI suggestions.
- No microVM snapshots, Firecracker, Hyper-V execution pools.

**No local model stack in core binary:**
- No Candle/llama.cpp/Ollama inference in Phase 1.
- Local embeddings are Phase 1.5+ (optional follow-on).

**No auto-apply refactors:**
- Phase 1 is understanding + retrieval, not autonomous patching.

---

## MVP user stories (what must work)

1) **As a dev**, I open a repo and start `aetherd`.
2) I edit a function and save the file.
3) AETHER updates SIR for that function within a reasonable time.
4) In my editor, I hover the function and see a good plain-English summary.
5) I run “Aether: Search” and type “rate limit retries” and get relevant symbols back.

---

## Constraints / guardrails

- **Incremental updates only:** do not reprocess whole repo on single-file edits.
- **Stable IDs:** symbol identifiers must be robust to line offset shifts.
- **Cost controls:** inference calls must be rate-limited and debounced.
- **Offline mode:** Phase 1 may degrade gracefully (show stale SIR + lexical search) when no API key is present.
