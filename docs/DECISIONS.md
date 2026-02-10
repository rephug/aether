# Decision Register — Phase 1 Build Packet

These decisions are “locked” for Phase 1 to reduce scope churn and help Codex implement confidently.

---

## Product decisions

1. **Phase 1 focus = Observer**
   - Ship daemon + LSP + live intent summaries + search.
   - Historian and Ghost are not required for MVP.

2. **Local-first architecture**
   - Data lives under `./.aether/` in the repo (or workspace root).
   - The daemon can be run without an editor (headless).

3. **LSP-first integration**
   - Editor clients consume AETHER via standard LSP (hover + commands).

---

## Intelligence decisions

4. **Cloud-first inference (Day 1)**
   - Default SIR generation uses **Gemini 3 Flash** via API.
   - No local model dependencies in core binary (no Candle/llama.cpp/Ollama in Phase 1).

5. **Embeddings backend (Phase 1 default)**
   - API embeddings by default (Gemini embedding API or alternative provider behind a trait).
   - Local embeddings are Phase 1.5+ optional follow-on.

6. **Reranking**
   - Not enabled by default in Phase 1.
   - If added, must be feature-flagged and cost-aware.

---

## Parsing + identity decisions

7. **Parser strategy**
   - Use tree-sitter (or equivalent) for symbol extraction in supported languages.
   - Keep the parser layer modular per language.

8. **Stable symbol IDs**
   - IDs must be stable across line-number shifts.
   - ID strategy: `BLAKE3( language + file_path + symbol_kind + qualified_name + signature_fingerprint )`.

9. **Incremental updates**
   - Only changed symbols trigger inference and storage updates.

---

## Storage decisions

10. **SIR storage**
    - Store SIR JSON as blobs on disk (e.g., `.aether/sir/<id>.json`) and index metadata in SQLite.

11. **Vector storage**
    - Use LanceDB for vector embeddings and ANN search (local embedded).

12. **Graph storage**
    - Not required in Phase 1 (Historian/graph engine is Phase 2).

---

## Platform decisions

13. **Linux primary**
    - Firecracker/snapshots are not Phase 1 scope.
14. **Windows support**
    - Phase 1 must run on Windows, but without Ghost sandboxing.
    - Any advanced virtualization is explicitly deferred.

---

## Engineering decisions

15. **Typed event bus**
    - Engines communicate through typed events inside `aetherd`.
16. **Strict schema validation**
    - SIR must validate against a Rust schema (serde + manual checks).
17. **Cost and rate limiting**
    - Global inference budget + per-provider rate limits are mandatory.
