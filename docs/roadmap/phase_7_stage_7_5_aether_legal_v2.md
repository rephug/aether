# Stage 7.5 — AETHER Legal: Clause Parser + CIR Schema (Revised)

**Phase:** 7 — The Pathfinder
**Prerequisites:** Stage 7.4 (Document Abstraction Layer)
**Feature Flag:** `--features legal`
**Estimated Codex Runs:** 2–3

---

## Purpose

Build the first non-code vertical. AETHER Legal parses legal documents (contracts, agreements, amendments) into clauses, generates CIR (Clause Intent Representation) annotations, and tracks clause relationships and amendments over time.

This stage implements the `DocumentParser` and `SemanticAnnotator` traits from Stage 7.4 for the legal domain, proving the abstraction works.

---

## New Crate: `aether-legal`

```
crates/aether-legal/
├── Cargo.toml
└── src/
    ├── lib.rs              # Re-exports, vertical registration
    ├── parser.rs           # LegalDocumentParser (implements DocumentParser)
    ├── annotator.rs        # LegalAnnotator (implements SemanticAnnotator)
    ├── cir.rs              # CIR schema definition + validation
    ├── edges.rs            # Legal edge types registry
    ├── clause.rs           # Clause extraction logic (text → clauses)
    └── text_extract.rs     # PDF/DOCX text extraction wrappers
```

**Dependencies:** `aether-core`, `aether-document`, `aether-infer`, `serde`, `serde_json`, `blake3`, `thiserror`, `async-trait`, `regex`

**Feature Flag (Decision #33):** The entire crate is gated behind a Cargo feature:
```toml
# Workspace Cargo.toml
[features]
legal = ["aether-legal"]
```

---

## CIR (Clause Intent Representation) Schema

```json
{
  "schema_name": "CIR",
  "schema_version": "1.0",
  "clause_type": "obligation | right | condition | definition | representation | warranty | indemnity | termination | confidentiality | limitation | governing_law | dispute_resolution | force_majeure | assignment | notice | amendment | other",
  "summary": "Plain English summary of what this clause does",
  "obligated_party": "Party A | Party B | Both | None",
  "beneficiary_party": "Party A | Party B | Both | None",
  "trigger_condition": "What must happen for this clause to activate (null if always active)",
  "deadline": "Temporal deadline if any (null if none)",
  "financial_impact": {
    "has_financial_impact": true,
    "amount_description": "Description of amounts involved",
    "cap_or_limit": "Any caps or limitations"
  },
  "references": ["Section 4.2", "Exhibit A", "Definition of 'Material Adverse Effect'"],
  "risk_level": "low | medium | high | critical",
  "risk_factors": ["Description of risk factors"],
  "negotiation_notes": "What a reviewer should pay attention to",
  "standard_vs_custom": "standard | modified_standard | fully_custom",
  "governing_jurisdiction": "If specified in this clause"
}
```

---

## Legal Edge Types

| Edge Type | Meaning | Example |
|---|---|---|
| `REFERENCES` | One clause references another by section number | "Subject to Section 4.2" |
| `SUPERSEDES` | Amendment clause replaces an earlier version | Amendment 3 supersedes §2.1 |
| `DEPENDS_ON` | Clause is conditional on another clause | Indemnity depends on Definition of "Loss" |
| `CONFLICTS_WITH` | Two clauses may be inconsistent | Non-compete scope vs. permitted activities |
| `LIMITS` | One clause caps or restricts another | Liability cap limits indemnity |
| `TRIGGERS` | Satisfaction of one activates another | Closing triggers payment obligations |
| `DEFINED_IN` | Term used in clause is defined elsewhere | "Material Adverse Effect" defined in §1.1 |

---

## Clause Extraction Strategy

Legal documents lack AST-equivalent structure. Clause extraction uses:

1. **Structural parsing:** Section numbers (1., 1.1, 1.1.1, (a), (b), (i), (ii)), heading detection, paragraph boundaries via regex
2. **LLM-assisted boundary detection:** For documents without clear section numbering, use inference engine to identify clause boundaries
3. **Hierarchy reconstruction:** Build parent-child relationships from numbering depth

```rust
pub struct ClauseExtractor {
    section_pattern: Regex,  // Matches "1.", "1.1", "(a)", "Section 1", "ARTICLE I"
}

impl ClauseExtractor {
    pub fn extract(&self, text: &str, doc_path: &str) -> Vec<GenericUnit> {
        // 1. Split on section number patterns
        // 2. Reconstruct hierarchy from numbering depth
        // 3. unit_id = BLAKE3(doc_path + "legal" + "clause" + section_number + text[:200])
        // 4. unit_kind based on depth: "article" | "section" | "subsection" | "paragraph"
    }
}
```

### PDF/DOCX Text Extraction (Decisions #34, #39)

```rust
// crates/aether-legal/src/text_extract.rs

pub async fn extract_text(path: &Path) -> Result<String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "pdf" => extract_pdf(path).await,
        "docx" => extract_docx(path).await,
        "txt" | "md" => tokio::fs::read_to_string(path).await.map_err(Into::into),
        _ => Err(anyhow!("Unsupported format: .{ext}. Supported: .pdf, .docx, .txt, .md"))
    }
}

async fn extract_pdf(path: &Path) -> Result<String> {
    // Primary: pdftotext (Poppler) via Command::new()
    // Fallback: lopdf Rust crate (Decision #39 revised) — pure Rust, no C++ deps
    // Error if neither available
}
```

**Primary:** `pdftotext` (Poppler) via `Command::new()` — dramatically better output on complex legal PDFs.
**Fallback:** `lopdf` Rust crate (Decision #39 revised) — pure Rust PDF text stream extraction. Lower quality than pdftotext on complex layouts, but zero C++ dependencies. Replaces both `pdf-extract` (mediocre output) and the rejected `pdfium-render` (required unshippable C++ dynamic library).
**No OCR:** If PDF has no extractable text, return clear error.

---

## MCP Tools (legal-specific)

### `aether_legal_ingest`

**Request:**
```json
{
  "path": "/documents/contracts/nda_acme_2025.pdf",
  "document_type": "contract",
  "parties": ["Acme Corp", "Widget Inc"],
  "effective_date": "2025-03-15",
  "tags": ["nda", "acme"]
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "document_path": "/documents/contracts/nda_acme_2025.pdf",
  "clauses_extracted": 47,
  "cir_generated": 47,
  "edges_extracted": 23,
  "embeddings_stored": 47,
  "hierarchy": { "articles": 8, "sections": 32, "subsections": 7 }
}
```

### `aether_legal_search`

**Request:**
```json
{
  "query": "indemnification obligations for data breach",
  "document_filter": ["nda_acme_2025.pdf"],
  "clause_types": ["indemnity", "limitation"],
  "mode": "hybrid",
  "limit": 10
}
```

Uses both lexical (SQLite FTS on `semantic_records`) and semantic (LanceDB `doc_legal` table) search, fused by the existing retrieval pipeline.

### `aether_legal_compare`

**Request:**
```json
{
  "document_a": "/documents/contracts/nda_v1.pdf",
  "document_b": "/documents/contracts/nda_v2.pdf",
  "comparison_mode": "clause_diff"
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "added_clauses": [...],
  "removed_clauses": [...],
  "modified_clauses": [
    {
      "section": "4.2",
      "change_type": "substantive",
      "summary": "Liability cap increased from $1M to $5M",
      "risk_impact": "higher_exposure"
    }
  ],
  "unchanged_clauses": 38
}
```

---

## CLI Commands

```bash
aether legal ingest /path/to/contract.pdf --type contract --parties "Acme,Widget"
aether legal search "indemnification for breach" --type indemnity --limit 5
aether legal compare v1.pdf v2.pdf --mode clause_diff
aether legal tree /path/to/contract.pdf          # Display clause hierarchy
aether legal obligations --party "Acme Corp" --document nda_acme_2025.pdf
```

---

## Test Fixtures (GAP-3 Mitigation)

Cannot ship copyrighted contracts. Stage includes synthetic test documents:

```
crates/aether-legal/tests/fixtures/
├── sample_nda_v1.txt        # Synthetic NDA with 15 clauses
├── sample_nda_v2.txt        # Modified version (3 clauses changed)
├── sample_msa.txt           # Synthetic MSA with 25 clauses
└── README.md                # Notes on fixture generation
```

Generated by LLM prompt in Codex session, not copied from real contracts.

---

## File Paths (new/modified)

| Path | Action |
|---|---|
| `crates/aether-legal/Cargo.toml` | Create |
| `crates/aether-legal/src/lib.rs` | Create |
| `crates/aether-legal/src/parser.rs` | Create |
| `crates/aether-legal/src/annotator.rs` | Create |
| `crates/aether-legal/src/cir.rs` | Create |
| `crates/aether-legal/src/edges.rs` | Create |
| `crates/aether-legal/src/clause.rs` | Create |
| `crates/aether-legal/src/text_extract.rs` | Create |
| `crates/aether-legal/tests/fixtures/` | Create — synthetic test documents |
| `crates/aether-mcp/src/legal_tools.rs` | Create — legal MCP tools |
| `crates/aether-mcp/src/lib.rs` | Modify — register legal tools (behind feature flag) |
| `crates/aetherd/src/cli.rs` | Modify — add `legal` subcommand group |
| `Cargo.toml` (workspace) | Modify — add aether-legal, add `legal` feature |
| `.github/workflows/ci.yml` | Modify — add aether-legal to feature-matrix testing |

---

## Edge Cases

| Scenario | Behavior |
|---|---|
| PDF with no extractable text (scanned) | Error: "No extractable text. OCR not supported." |
| No section structure detected | Fall back to paragraph-level extraction, log warning |
| Very long clause (>5KB) | Accept; truncate to 2KB for CIR generation prompt |
| Cross-reference to non-existent section | Create edge with metadata `{"resolved": false}` |
| Amendment references original not in system | Store with metadata `{"external_reference": true}` |
| Same filename, different paths | unit_id includes full path — no collision |
| DOCX with tracked changes | Extract accepted text only; note tracked changes in metadata |
| pdftotext not installed | Fall back to lopdf; log warning about reduced extraction quality |

---

## Pass Criteria

1. PDF and DOCX documents parse into clause hierarchies with correct section numbering.
2. CIR generation produces valid CIR JSON for each clause.
3. REFERENCES edges correctly link cross-referenced sections.
4. `aether_legal_compare` identifies added, removed, and modified clauses between versions.
5. Legal MCP tools return properly structured responses.
6. CLI commands work: `ingest`, `search`, `compare`, `tree`.
7. All data stored in `document_units` and `semantic_records` tables (NOT `symbols`).
8. Embeddings stored in `doc_legal` LanceDB table; semantic search returns relevant clauses.
9. Existing code pipeline completely unaffected.
10. Feature flag works: `cargo build` (no features) does NOT compile aether-legal.
11. Validation gates pass:
    ```
    cargo fmt --all --check
    cargo clippy --workspace --features legal -- -D warnings
    cargo test -p aether-document
    cargo test -p aether-legal
    cargo test -p aether-store
    cargo test -p aether-mcp --features legal
    cargo test -p aetherd --features legal
    ```

---

## Codex Prompt

```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- export CARGO_TARGET_DIR=/home/rephu/aether-target
- export CARGO_BUILD_JOBS=1
- export PROTOC=$(which protoc)
- export TMPDIR=/home/rephu/aether-target/tmp
- mkdir -p $TMPDIR
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.
- The repo uses mold linker via .cargo/config.toml — ensure mold and clang are installed.

NOTE ON ARCHITECTURE: aether-legal implements DocumentParser and SemanticAnnotator
traits from aether-document (Stage 7.4). Data stored in document_units and
semantic_records SQLite tables, NOT in symbols table. Graph edges stored in SurrealDB
document_edge table (Stage 7.2/7.4). Embeddings go to "doc_legal" LanceDB table
via DocumentEmbedder from Stage 7.4. The code pipeline is untouched.

NOTE ON FEATURE FLAG: aether-legal is gated behind workspace feature "legal".
  cargo build                    → does NOT include legal crate
  cargo build --features legal   → includes legal crate
Legal MCP tools in aether-mcp are also gated: #[cfg(feature = "legal")]

NOTE ON PDF EXTRACTION (Decisions #34, #39 revised):
Primary = pdftotext (Poppler) via Command::new().
Fallback = lopdf Rust crate (pure Rust, no C++ dependency).
  lopdf extracts raw text streams from PDF. Lower quality than pdftotext on complex
  layouts, but works everywhere with zero system dependencies.
  Add to Cargo.toml: lopdf = "0.34"
  Do NOT use pdfium-render — it requires a C++ dynamic library at runtime that
  breaks single-binary portability.
Do NOT implement OCR. If pdftotext not found AND lopdf extraction returns empty, return clear error.

NOTE ON INFERENCE: CIR generation uses aether-infer providers (Gemini Flash default).
The LLM prompt for CIR is different from SIR — it's a legal-domain prompt producing
the CIR JSON schema. Include the CIR schema in the prompt as a JSON template.

NOTE ON TEST FIXTURES: Create SYNTHETIC test documents — do NOT copy real contracts.
Generate them in the Codex session: simple NDA with 15 clauses, a modified v2 with
3 changed clauses, and an MSA with 25 clauses.

NOTE ON PRIOR STAGES:
- Stage 7.2: SurrealDB/SurrealKV for graph storage (replaces CozoDB)
- Stage 7.4: aether-document crate with DocumentParser, SemanticAnnotator traits,
  document_units + semantic_records SQLite tables, document_node + document_edge
  SurrealDB tables, DocumentEmbedder for domain-scoped LanceDB tables.

You are working in the repo root at /home/rephu/projects/aether.

Read docs/roadmap/phase_7_stage_7_5_aether_legal.md for the full specification.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase7-stage7-5-aether-legal off main.
3) Create worktree ../aether-phase7-stage7-5 for that branch and switch into it.
4) Create new crate crates/aether-legal with:
   - Cargo.toml depending on aether-core, aether-document, aether-infer, serde,
     serde_json, blake3, thiserror, async-trait, regex, lopdf
   - src/lib.rs — registration with VerticalRegistry
   - src/text_extract.rs — PDF extraction (pdftotext primary, lopdf fallback), DOCX
   - src/clause.rs — ClauseExtractor (section number regex, hierarchy reconstruction)
   - src/parser.rs — LegalDocumentParser implementing DocumentParser trait
   - src/annotator.rs — LegalAnnotator implementing SemanticAnnotator (CIR generation via LLM)
   - src/cir.rs — CIR schema struct, validation, JSON serialization
   - src/edges.rs — LegalEdgeRegistry with REFERENCES, SUPERSEDES, DEPENDS_ON, etc.
5) Generate synthetic test fixtures in crates/aether-legal/tests/fixtures/:
   - sample_nda_v1.txt (15 clauses, clear section numbering)
   - sample_nda_v2.txt (same structure, 3 clauses modified)
   - sample_msa.txt (25 clauses with cross-references)
6) Add legal MCP tools in crates/aether-mcp/src/legal_tools.rs (gated: #[cfg(feature = "legal")]):
   - aether_legal_ingest: extract text → parse clauses → generate CIRs → embed → store
   - aether_legal_search: hybrid search over doc_legal embeddings + lexical
   - aether_legal_compare: clause-level diff between two document versions
7) Register legal tools in crates/aether-mcp/src/lib.rs behind feature gate.
8) Add legal CLI subcommand group in crates/aetherd/src/cli.rs (gated).
9) Add workspace feature "legal" in Cargo.toml. Add aether-legal to members.
10) Update CI: test with --features legal in feature-matrix job.
11) Add tests:
    - Unit tests for ClauseExtractor (section numbering patterns, hierarchy)
    - Unit tests for CIR schema validation
    - Unit tests for LegalEdgeRegistry
    - Integration test: ingest sample_nda_v1.txt, verify clauses + CIRs + edges
    - Integration test: compare sample_nda_v1.txt vs v2, verify diff output
    - Integration test: search "indemnification" returns relevant clauses
12) Run validation:
    - cargo fmt --all --check
    - cargo clippy --workspace --features legal -- -D warnings
    - cargo test -p aether-document
    - cargo test -p aether-legal
    - cargo test -p aether-store
    - cargo test -p aether-mcp --features legal
    - cargo test -p aetherd --features legal
13) Commit with message: "Add AETHER Legal vertical with clause parser and CIR schema"

SCOPE GUARD: Do NOT modify the existing code pipeline (symbols, SIR, tree-sitter).
Do NOT implement OCR. Do NOT implement amendment tracking as file watcher — amendments
are ingested manually. Do NOT build a legal-specific daemon binary — legal tools run
inside aetherd via feature flag.
```
