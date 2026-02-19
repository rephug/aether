# Phase 6 — The Chronicler

## Stage 6.3 — Test Intent Extraction

### Purpose
Parse test files to extract behavioral intent strings (the human-readable descriptions of what tests verify) and link them as guards to the symbols/files they test. Surface these as guardrails when code changes.

### What It Borrows From
- **EngramPro:** Test intent extraction as behavioral guardrails. EngramPro extracts `it("should handle negative balance")` strings and surfaces them during impact analysis.

### Extraction Patterns (tree-sitter)

| Language | Test Pattern | Intent Source |
|----------|-------------|---------------|
| Rust | `#[test] fn test_name()` | Function name (converted: `test_handles_negative_balance` → "handles negative balance") |
| Rust | `#[test]` with doc comment `/// ...` | Doc comment text |
| TypeScript/JS | `it("should handle X", ...)` | First string argument |
| TypeScript/JS | `test("should handle X", ...)` | First string argument |
| TypeScript/JS | `describe("PaymentService", ...)` | First string argument (used as group label) |
| Python | `def test_handles_negative_balance(...)` | Function name (converted) |
| Python | `"""docstring"""` under test function | Docstring text |

### Schema

**SQLite: `test_intents` table**
```sql
CREATE TABLE IF NOT EXISTS test_intents (
    intent_id       TEXT PRIMARY KEY,     -- BLAKE3(file_path + test_name + intent_text)
    file_path       TEXT NOT NULL,
    test_name       TEXT NOT NULL,
    intent_text     TEXT NOT NULL,         -- Human-readable intent string
    group_label     TEXT,                  -- describe() group if applicable
    language        TEXT NOT NULL,
    symbol_id       TEXT,                  -- Symbol ID of test function (if indexed)
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);

CREATE INDEX idx_test_intents_file ON test_intents(file_path);
```

**CozoDB: `tested_by` relation**
```
:create tested_by {
    target_file: String,          -- File being tested (inferred)
    test_file: String,
    =>
    intent_count: Int,
    confidence: Float             -- How confident is the target_file inference
}
```

### Target File Inference
Given a test file, infer which production file(s) it tests:

1. **Naming convention:** `src/payment.rs` ↔ `tests/payment_test.rs`, `src/payment.ts` ↔ `src/payment.test.ts`, `src/__tests__/payment.ts`
2. **Import analysis:** Parse test file imports; production files that are imported are likely targets.
3. **Temporal coupling cross-reference (from 6.2):** If test file and production file have high co-change score, link them.
4. **Confidence:** Convention match = 0.9, Import match = 0.8, Coupling match = coupling_score * 0.7.

### Blast Radius Integration
When `aether_blast_radius` is called (from 6.2), enrich the response with test intents:

```json
{
  "coupled_files": [...],
  "test_guards": [
    {
      "test_file": "crates/aether-store/tests/store_tests.rs",
      "intents": [
        "handles empty symbol table",
        "returns none for missing symbol",
        "increments version on hash change"
      ],
      "confidence": 0.9,
      "inference_method": "naming_convention"
    }
  ]
}
```

### Edge Cases

| Scenario | Behavior |
|----------|----------|
| Test file with no intent strings (just `#[test] fn test_1()`) | Use function name converted to natural language |
| Test file that imports from multiple production files | Create `tested_by` edges for each, split confidence |
| Dynamically generated test names | Skip — only extract statically visible intents |
| Test in same file as production code (Rust `#[cfg(test)] mod tests`) | Target file = same file, confidence = 1.0 |

### Pass Criteria
1. Rust test functions produce extracted intent strings.
2. TypeScript `it()` / `test()` / `describe()` produce extracted intent strings.
3. Python test functions produce extracted intent strings.
4. Target file inference works for naming convention and import patterns.
5. `aether_blast_radius` includes `test_guards` when test intents exist.
6. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

### Exact Codex Prompt
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_6_stage_6_3_test_intents.md for the full specification.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase6-stage6-3-test-intents off main.
3) Create worktree ../aether-phase6-stage6-3 for that branch and switch into it.
4) In crates/aether-parse:
   - Add test intent extraction to each language plugin (Rust, TypeScript/JS, Python).
   - Extract function names, it()/test()/describe() string arguments, doc comments.
   - Return Vec<TestIntent> alongside existing symbol extraction.
5) In crates/aether-store:
   - Add test_intents SQLite table and migration.
   - Add tested_by CozoDB relation.
6) In crates/aether-memory or aether-store:
   - Implement target file inference (naming convention, import analysis, coupling cross-ref).
7) In crates/aether-mcp:
   - Extend aether_blast_radius response to include test_guards field.
   - Add aether_test_intents tool: query test intents for a file/symbol.
8) Add tests:
   - Parse test intent extraction for Rust #[test], TS it()/test(), Python def test_*.
   - Verify target file inference for naming convention and imports.
   - Verify blast_radius includes test_guards.
9) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
10) Commit with message: "Add test intent extraction with behavioral guards"
```
