# Acceptance Criteria — Phase 1 “Observer”

This document defines *pass/fail* criteria for the Phase 1 MVP.

---

## A. Install + run

1. **Single-command run (dev mode)**
   - `cargo run -p aetherd -- --workspace <path>` starts the daemon without crashes.
2. **Creates `.aether/`**
   - On first run, creates `./.aether/` with:
     - `config.toml` (default)
     - local databases / indexes (as applicable)
     - logs directory (optional)

---

## B. Watcher + incremental pipeline

3. **Debounced file watching**
   - A rapid sequence of edits results in **one** processing event per file after debounce.
4. **Incremental symbol detection**
   - If only one function changes in a file, AETHER emits “changed symbols” containing that function **only** (not the whole file).
5. **Stable IDs**
   - Moving a function up/down in the file (without changing its signature/body) preserves its symbol ID.
   - Renaming a function changes its symbol ID.

---

## C. SIR generation + validation

6. **SIR generation works**
   - With a valid `GEMINI_API_KEY`, editing a symbol triggers SIR generation.
7. **Strict JSON**
   - SIR must parse as valid JSON and match the SIR schema (minimum required fields).
   - If the model returns invalid JSON, the system retries (bounded retries) and logs the failure.
8. **Caching**
   - If the symbol text hash hasn’t changed, AETHER **does not** re-call inference (no redundant spend).

---

## D. Storage + persistence

9. **Persistent storage**
   - Restarting `aetherd` preserves previously generated SIR and search indexes.
10. **SIR retrieval by ID**
    - Given a symbol ID, AETHER can load the corresponding SIR from disk/db.

---

## E. Search

11. **Lexical search**
    - Querying by exact symbol name returns that symbol in top results.
12. **Semantic search**
    - Querying with natural language returns semantically related results (top K) when embeddings are enabled/configured.
13. **Ranking hygiene**
    - Results include at least: symbol name, file path, snippet/summary, and score.

---

## F. LSP integration (must demo)

14. **Hover**
    - In a supported editor (or via LSP test harness), hovering a symbol returns:
      - symbol name + type (function/class/etc.)
      - 3–8 line plain-English summary from SIR
15. **Command**
    - LSP provides a command like `aether.search` that accepts a query string and returns results.

---

## G. Performance budgets (soft targets)

16. **Edit-to-hover latency**
    - After saving a small file, updated hover should be available within:
      - **<= 2s** for cached SIR (no inference call)
      - **<= 10s** for uncached SIR (cloud call), on typical broadband
17. **Memory**
    - Daemon stays under a reasonable memory ceiling during idle (target: < 300MB).

---

## H. Failure modes (must be safe)

18. **No API key**
    - AETHER runs and provides lexical search + “stale/none” SIR behavior without crashing.
19. **Provider outage / rate limit**
    - AETHER backs off (retry with jitter) and continues running.
20. **Large files**
    - Files over `max_file_size` are skipped and logged; daemon remains stable.
