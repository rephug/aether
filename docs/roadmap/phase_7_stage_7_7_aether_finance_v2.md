# Stage 7.7 — AETHER Finance: Financial Parser + FIR Schema (Revised)

**Phase:** 7 — The Pathfinder
**Prerequisites:** Stage 7.4 (Document Abstraction Layer)
**Feature Flag:** `--features finance`
**Estimated Codex Runs:** 2–3

---

## Purpose

Build the second non-code vertical. AETHER Finance parses financial documents (income statements, balance sheets, invoices, audit reports, transaction ledgers) into structured line items, generates FIR (Financial Intent Representation) annotations, and tracks money flows as a directed graph.

Key capability: **follow the money**. Given an entity or account, trace how funds flow through the graph across documents and time periods. Graph traversal uses SurrealDB's `→`/`←` arrow syntax for multi-hop FLOWS_TO queries.

---

## New Crate: `aether-finance`

```
crates/aether-finance/
├── Cargo.toml
└── src/
    ├── lib.rs              # Re-exports, vertical registration
    ├── parser.rs           # FinancialDocumentParser (implements DocumentParser)
    ├── annotator.rs        # FinancialAnnotator (implements SemanticAnnotator)
    ├── fir.rs              # FIR schema definition + validation
    ├── edges.rs            # Financial edge types registry
    ├── line_item.rs        # Line item extraction logic
    ├── entity.rs           # Entity resolution + normalization (Decision #36)
    ├── money.rs            # Decimal money handling (Decision #35)
    └── text_extract.rs     # PDF/CSV/XLSX text extraction (reuses legal PDF logic)
```

**Dependencies:** `aether-core`, `aether-document`, `aether-infer`, `rust_decimal`, `serde`, `serde_json`, `blake3`, `thiserror`, `async-trait`, `regex`, `csv`, `calamine` (XLSX reading)

**Feature Flag (Decision #33):**
```toml
[features]
finance = ["aether-finance"]
```

---

## FIR (Financial Intent Representation) Schema

```json
{
  "schema_name": "FIR",
  "schema_version": "1.0",
  "line_type": "revenue | expense | asset | liability | equity | transfer | tax | depreciation | amortization | provision | dividend | interest | gain | loss | adjustment | other",
  "description": "Plain English description of this financial item",
  "amount": {
    "value": "1234567.89",
    "currency": "USD",
    "period": "Q3 2025",
    "direction": "debit | credit | net"
  },
  "counterparty": {
    "name": "Acme Corp",
    "normalized_name": "ACME CORP",
    "entity_type": "company | individual | government | internal"
  },
  "account": {
    "name": "Accounts Receivable",
    "code": "1200",
    "category": "current_asset"
  },
  "temporal": {
    "transaction_date": "2025-09-15",
    "reporting_period": "Q3 2025",
    "fiscal_year": "FY2025"
  },
  "source_document": {
    "type": "income_statement | balance_sheet | invoice | ledger | audit_report | tax_filing | bank_statement",
    "reference": "Invoice #INV-2025-0847"
  },
  "audit_trail": {
    "confidence": "high | medium | low",
    "extraction_method": "structured | llm_assisted | manual",
    "notes": "Amount cross-referenced with bank statement"
  },
  "regulatory_flags": ["material_transaction", "related_party", "above_reporting_threshold"],
  "references": ["Note 4", "Schedule A", "Line 42"]
}
```

---

## Financial Edge Types

| Edge Type | Meaning | Example |
|---|---|---|
| `FLOWS_TO` | Money moves from source to target | Revenue → Bank Account |
| `NETS_AGAINST` | Two items offset each other | Receivable vs. Payment |
| `CONSOLIDATES_INTO` | Line item rolls up into summary | Dept expenses → Total OpEx |
| `REFERENCES` | Cross-reference between documents | Invoice references PO |
| `ADJUSTS` | One entry modifies another | Write-off adjusts receivable |
| `FUNDED_BY` | Asset or expense funded by source | Project funded by loan |
| `AUDITED_BY` | Audit report covers a line item | Audit covers Q3 revenue |
| `RECONCILES_WITH` | Two entries should match | Bank statement vs. ledger |

All financial edges are stored in the SurrealDB `document_edge` table (from Stage 7.4) with `domain = "finance"`. Multi-hop traversal uses SurrealDB arrow syntax: `SELECT ->document_edge[WHERE edge_type = "FLOWS_TO"]->document_node FROM ...`

---

## Money Handling (Decision #35)

```rust
// crates/aether-finance/src/money.rs
use rust_decimal::Decimal;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Money {
    pub value: Decimal,
    pub currency: String,  // ISO 4217: "USD", "EUR", "GBP"
}

impl Money {
    pub fn new(value: &str, currency: &str) -> Result<Self> {
        let value = Decimal::from_str(value)
            .map_err(|e| anyhow!("Invalid decimal: {value} — {e}"))?;
        Ok(Self { value, currency: currency.to_uppercase() })
    }

    pub fn zero(currency: &str) -> Self {
        Self { value: Decimal::ZERO, currency: currency.to_uppercase() }
    }
}
```

**Rules:**
- All monetary values use `rust_decimal::Decimal` — NEVER `f64`
- Store in original currency — no automatic conversion
- Currency stored as ISO 4217 code alongside amount
- Addition/subtraction only between same currencies; cross-currency operations error

---

## Entity Resolution (Decision #36)

```rust
// crates/aether-finance/src/entity.rs

pub struct EntityResolver {
    aliases: HashMap<String, String>,  // Loaded from entity_aliases table
}

impl EntityResolver {
    /// Normalize an entity name for matching.
    /// "Acme Corp." → "ACME CORP"
    /// "ACME CORPORATION" → "ACME CORP" (via alias table)
    pub fn normalize(&self, name: &str) -> String {
        let upper = name.to_uppercase();
        let stripped = strip_suffixes(&upper, &["INC", "INC.", "CORP", "CORP.", "LLC", "LTD", "LTD.", "CO.", "CO"]);
        let trimmed = stripped.trim().to_string();
        // Check alias table
        self.aliases.get(&trimmed).cloned().unwrap_or(trimmed)
    }

    /// LLM-assisted disambiguation for ambiguous names.
    /// "Apple" could be Apple Inc, Apple Bank, etc.
    pub async fn disambiguate(&self, name: &str, context: &str, infer: &dyn InferenceProvider) -> Result<String>;
}
```

**SQLite table:**
```sql
CREATE TABLE IF NOT EXISTS entity_aliases (
    alias           TEXT PRIMARY KEY,
    canonical_name  TEXT NOT NULL,
    entity_type     TEXT NOT NULL,  -- "company" | "individual" | "government" | "internal"
    created_by      TEXT NOT NULL DEFAULT 'auto',  -- "auto" | "manual"
    created_at      INTEGER NOT NULL
);
```

Manual curation: `aether finance entity-alias add "MSFT" "MICROSOFT CORP" --type company`

---

## Line Item Extraction Strategy

Financial documents vary enormously. Extraction strategy per format:

### Structured formats (CSV, XLSX)
Direct column mapping. User provides column hints or parser auto-detects:
- Look for columns with monetary patterns (`$1,234.56`, `(1,234.56)`)
- Look for date columns
- Look for account/description columns

```rust
pub struct StructuredExtractor;

impl StructuredExtractor {
    pub fn extract_csv(&self, path: &Path, hints: &ColumnHints) -> Result<Vec<GenericUnit>>;
    pub fn extract_xlsx(&self, path: &Path, sheet: Option<&str>, hints: &ColumnHints) -> Result<Vec<GenericUnit>>;
}

pub struct ColumnHints {
    pub amount_column: Option<String>,
    pub date_column: Option<String>,
    pub description_column: Option<String>,
    pub account_column: Option<String>,
    pub currency: String,  // Default currency for the document
}
```

### Unstructured formats (PDF)
LLM-assisted extraction:
1. Extract text via pdftotext; fallback to lopdf (Decision #39 revised)
2. Send to LLM with financial extraction prompt
3. LLM returns structured line items as JSON

### PDF/Text Extraction (Decisions #34, #39)

```rust
// crates/aether-finance/src/text_extract.rs

async fn extract_pdf(path: &Path) -> Result<String> {
    // Primary: pdftotext (Poppler) via Command::new()
    // Fallback: lopdf Rust crate (Decision #39 revised) — pure Rust, no C++ deps
    // Error if neither available
}
```

**Primary:** `pdftotext` (Poppler) — best quality for most financial PDFs.
**Fallback:** `lopdf` Rust crate (Decision #39 revised) — pure Rust PDF text stream extraction. Lower quality than pdftotext on complex tabular layouts, but zero C++ dependencies. Replaces both `pdf-extract` (mediocre output) and the rejected `pdfium-render` (required unshippable C++ dynamic library).
**No OCR.** Scanned documents return clear error.

### Supported Document Types

| Type | Extensions | Extraction Method |
|---|---|---|
| Income Statement | PDF | LLM-assisted |
| Balance Sheet | PDF | LLM-assisted |
| Invoice | PDF | LLM-assisted |
| Transaction Ledger | CSV, XLSX | Structured column mapping |
| Bank Statement | CSV, PDF | Structured or LLM-assisted |
| Audit Report | PDF | LLM-assisted (sections → units) |
| Tax Filing | PDF | LLM-assisted |

---

## MCP Tools (finance-specific)

### `aether_finance_ingest`

**Request:**
```json
{
  "path": "/documents/financials/q3_income_statement.pdf",
  "document_type": "income_statement",
  "entity": "Acme Corp",
  "period": "Q3 2025",
  "currency": "USD",
  "tags": ["quarterly", "acme"]
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "document_path": "/documents/financials/q3_income_statement.pdf",
  "line_items_extracted": 87,
  "fir_generated": 87,
  "edges_extracted": 34,
  "embeddings_stored": 87,
  "entities_resolved": 12,
  "total_amounts": {
    "revenue": "45,678,901.23 USD",
    "expenses": "38,234,567.89 USD",
    "net": "7,444,333.34 USD"
  }
}
```

### `aether_finance_search`

**Request:**
```json
{
  "query": "revenue from enterprise customers",
  "entity_filter": ["Acme Corp"],
  "period_filter": "Q3 2025",
  "line_types": ["revenue"],
  "mode": "hybrid",
  "limit": 10
}
```

### `aether_finance_trace`

**"Follow the money"** — trace how funds flow through the SurrealDB document graph.

**Request:**
```json
{
  "entity": "Acme Corp",
  "direction": "outflow",
  "period": "Q3 2025",
  "min_amount": "100000",
  "max_hops": 3
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "trace_root": "ACME CORP",
  "flows": [
    {
      "path": ["Acme Corp", "Revenue", "Operating Income", "Tax Provision", "IRS"],
      "amount": "2,233,000.00 USD",
      "documents": ["q3_income_statement.pdf", "tax_provision_q3.xlsx"],
      "hop_count": 4
    }
  ],
  "total_outflow": "38,234,567.89 USD",
  "entity_count": 8
}
```

**Implementation:** Uses SurrealDB graph traversal via `document_edge` relations with `edge_type = "FLOWS_TO"`. For multi-hop traces beyond SurrealDB's single-query depth, falls back to the application-level BFS from Stage 7.2's `graph_algorithms.rs`.

### `aether_finance_reconcile`

Cross-check amounts between documents.

**Request:**
```json
{
  "document_a": "q3_income_statement.pdf",
  "document_b": "q3_bank_statement.csv",
  "tolerance": "0.01"
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "matched": 73,
  "unmatched_a": 4,
  "unmatched_b": 7,
  "discrepancies": [
    {
      "description": "Consulting Revenue",
      "amount_a": "125,000.00 USD",
      "amount_b": "124,999.50 USD",
      "difference": "0.50 USD",
      "within_tolerance": true
    }
  ]
}
```

---

## CLI Commands

```bash
aether finance ingest /path/to/q3_income.pdf --type income_statement --entity "Acme" --period "Q3 2025"
aether finance ingest /path/to/transactions.csv --type ledger --currency USD --amount-col "Amount" --date-col "Date"
aether finance search "enterprise revenue" --type revenue --period "Q3 2025"
aether finance trace "Acme Corp" --direction outflow --period "Q3 2025" --min-amount 100000
aether finance reconcile q3_income.pdf q3_bank.csv --tolerance 0.01
aether finance entities                              # List resolved entities
aether finance entity-alias add "MSFT" "MICROSOFT CORP" --type company
```

---

## Test Fixtures (GAP-3 Mitigation)

Synthetic financial documents generated by LLM in Codex session:

```
crates/aether-finance/tests/fixtures/
├── sample_income_statement.txt    # Synthetic Q3 income statement (text format)
├── sample_balance_sheet.txt       # Synthetic balance sheet
├── sample_transactions.csv        # 50 synthetic transactions
├── sample_invoice.txt             # Synthetic invoice
└── README.md                      # Notes on fixture generation
```

All numbers are fictional. No real company data.

---

## File Paths (new/modified)

| Path | Action |
|---|---|
| `crates/aether-finance/Cargo.toml` | Create |
| `crates/aether-finance/src/lib.rs` | Create |
| `crates/aether-finance/src/parser.rs` | Create |
| `crates/aether-finance/src/annotator.rs` | Create |
| `crates/aether-finance/src/fir.rs` | Create |
| `crates/aether-finance/src/edges.rs` | Create |
| `crates/aether-finance/src/line_item.rs` | Create |
| `crates/aether-finance/src/entity.rs` | Create |
| `crates/aether-finance/src/money.rs` | Create |
| `crates/aether-finance/src/text_extract.rs` | Create |
| `crates/aether-finance/tests/fixtures/` | Create — synthetic test documents |
| `crates/aether-store/src/entity_store.rs` | Create — entity_aliases CRUD |
| `crates/aether-store/src/lib.rs` | Modify — add entity_aliases table migration |
| `crates/aether-mcp/src/finance_tools.rs` | Create — finance MCP tools |
| `crates/aether-mcp/src/lib.rs` | Modify — register finance tools (behind feature flag) |
| `crates/aetherd/src/cli.rs` | Modify — add `finance` subcommand group |
| `Cargo.toml` (workspace) | Modify — add aether-finance, add `finance` feature |
| `.github/workflows/ci.yml` | Modify — add aether-finance to feature-matrix testing |

---

## Edge Cases

| Scenario | Behavior |
|---|---|
| Negative amounts in parentheses `(1,234.56)` | Parse as negative Decimal |
| Multiple currencies in one document | Store each in original currency; FLOWS_TO edges note currency |
| Cross-currency transfer | Create FLOWS_TO edge with metadata `{"currency_a": "USD", "currency_b": "EUR"}`. No auto-conversion. |
| CSV with no header row | Require `--skip-rows N` or auto-detect via heuristic |
| XLSX with multiple sheets | Default to first sheet; `--sheet "Revenue"` selects specific |
| Entity name ambiguous | Use LLM disambiguation if available; otherwise store as-is, flag for review |
| Amount precision loss | Never happens — Decimal stores exact values |
| Very large transaction count (>10K line items) | Batch processing: embed in chunks of 100, progress reporting |
| Invoice with no explicit currency | Use `--currency` flag (required for ingest); error if not provided |
| PDF with tables (tabular financial data) | pdftotext + LLM extraction; tables often messy, rely on LLM for structure |
| Reconciliation with rounding differences | `tolerance` parameter (default 0.01); flag within-tolerance as "matched" |

---

## Pass Criteria

1. CSV and XLSX files parse into structured line items with correct Decimal amounts.
2. PDF financial documents produce line items via LLM-assisted extraction.
3. FIR generation produces valid FIR JSON for each line item.
4. FLOWS_TO edges correctly trace money movement between accounts/entities.
5. Entity resolution normalizes names and resolves aliases.
6. `aether_finance_trace` follows money flow graph across documents (SurrealDB traversal).
7. `aether_finance_reconcile` identifies matching and discrepant amounts.
8. All data stored in `document_units` and `semantic_records` tables (NOT `symbols`).
9. Embeddings stored in `doc_finance` LanceDB table; semantic search returns relevant items.
10. Feature flag works: `cargo build` (no features) does NOT compile aether-finance.
11. Existing code pipeline and legal pipeline completely unaffected.
12. Validation gates pass:
    ```
    cargo fmt --all --check
    cargo clippy --workspace --features finance -- -D warnings
    cargo test -p aether-document
    cargo test -p aether-finance
    cargo test -p aether-store
    cargo test -p aether-mcp --features finance
    cargo test -p aetherd --features finance
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

NOTE ON ARCHITECTURE: aether-finance implements DocumentParser and SemanticAnnotator
traits from aether-document (Stage 7.4). Data stored in document_units and
semantic_records SQLite tables, NOT in symbols table. Graph edges stored in SurrealDB
document_edge table (Stage 7.2/7.4) with domain = "finance". Embeddings go to
"doc_finance" LanceDB table via DocumentEmbedder. The code and legal pipelines are
untouched.

NOTE ON GRAPH TRAVERSAL: Money flow tracing (aether_finance_trace) uses SurrealDB's
document_edge table with edge_type = "FLOWS_TO". Multi-hop traversal can use SurrealDB
arrow syntax: SELECT ->document_edge[WHERE edge_type = "FLOWS_TO"]->document_node FROM ...
For deeper traces, fall back to application-level BFS from graph_algorithms.rs (Stage 7.2).

NOTE ON FEATURE FLAG: aether-finance is gated behind workspace feature "finance".
  cargo build                     → does NOT include finance crate
  cargo build --features finance  → includes finance crate
Finance MCP tools gated: #[cfg(feature = "finance")]

NOTE ON MONEY (Decision #35): ALL monetary values use rust_decimal::Decimal. NEVER f64.
Store in original currency (ISO 4217). No auto-conversion. Money struct wraps Decimal + currency.

NOTE ON ENTITY RESOLUTION (Decision #36): Basic normalization (uppercase, strip suffixes
like Inc/Corp/LLC) + entity_aliases SQLite table. LLM disambiguation for ambiguous names.
Manual curation via CLI. Do NOT implement graph-based entity resolution.

NOTE ON PDF EXTRACTION (Decisions #34, #39 revised): Reuse pdftotext/lopdf approach.
Primary = pdftotext (Poppler) via Command::new().
Fallback = lopdf Rust crate (pure Rust, no C++ dependency — NOT pdfium-render,
which requires an unshippable C++ dynamic library at runtime).
Financial PDFs often have tabular data — rely on LLM extraction prompt for structure,
not regex. lopdf may produce lower-quality text on complex tables; the LLM prompt
should handle messy input gracefully.

NOTE ON STRUCTURED FORMATS: CSV via `csv` crate, XLSX via `calamine` crate.
Auto-detect monetary columns via regex for patterns like $1,234.56 or (1,234.56).
User can provide column hints (--amount-col, --date-col, etc.).

NOTE ON PRIOR STAGES:
- Stage 7.2: SurrealDB/SurrealKV for graph storage (replaces CozoDB)
- Stage 7.4: aether-document crate with traits, document_units + semantic_records SQLite
  tables, document_node + document_edge SurrealDB tables, DocumentEmbedder for
  domain-scoped LanceDB tables.
- Stage 7.5 (parallel): aether-legal may or may not be merged. Do not depend on it.

You are working in the repo root at /home/rephu/projects/aether.

Read docs/roadmap/phase_7_stage_7_7_aether_finance.md for the full specification.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase7-stage7-7-aether-finance off main.
3) Create worktree ../aether-phase7-stage7-7 for that branch and switch into it.
4) Create new crate crates/aether-finance with:
   - Cargo.toml depending on aether-core, aether-document, aether-infer, rust_decimal,
     serde, serde_json, blake3, thiserror, async-trait, regex, csv, calamine, lopdf
   - src/lib.rs — registration with VerticalRegistry
   - src/money.rs — Money struct (Decimal + currency), arithmetic guards
   - src/entity.rs — EntityResolver (normalize, aliases, LLM disambiguation)
   - src/line_item.rs — StructuredExtractor for CSV/XLSX, LLM extraction for PDF
   - src/text_extract.rs — PDF extraction (pdftotext primary, lopdf fallback)
   - src/parser.rs — FinancialDocumentParser implementing DocumentParser trait
   - src/annotator.rs — FinancialAnnotator implementing SemanticAnnotator (FIR generation via LLM)
   - src/fir.rs — FIR schema struct, validation, JSON serialization
   - src/edges.rs — FinancialEdgeRegistry (FLOWS_TO, NETS_AGAINST, CONSOLIDATES_INTO, etc.)
5) Add entity_aliases table to aether-store SQLite migrations.
6) Create crates/aether-store/src/entity_store.rs — CRUD for entity_aliases.
7) Generate synthetic test fixtures in crates/aether-finance/tests/fixtures/:
   - sample_income_statement.txt (Q3 income statement, ~30 line items)
   - sample_balance_sheet.txt (~25 line items)
   - sample_transactions.csv (50 transactions with Date, Description, Amount, Account)
   - sample_invoice.txt (10 line items)
8) Add finance MCP tools in crates/aether-mcp/src/finance_tools.rs (gated):
   - aether_finance_ingest: extract → parse → generate FIRs → resolve entities → embed → store
   - aether_finance_search: hybrid search over doc_finance embeddings + lexical
   - aether_finance_trace: SurrealDB graph traversal following FLOWS_TO edges (with BFS fallback)
   - aether_finance_reconcile: cross-document amount matching with tolerance
9) Register finance tools in aether-mcp behind feature gate.
10) Add finance CLI subcommand group in aetherd CLI (gated).
11) Add workspace feature "finance" in Cargo.toml. Add aether-finance to members.
12) Update CI: test with --features finance in feature-matrix job.
13) Add tests:
    - Unit tests for Money arithmetic (add, subtract, zero, cross-currency error)
    - Unit tests for EntityResolver normalization and alias lookup
    - Unit tests for CSV/XLSX extraction (sample_transactions.csv)
    - Unit tests for FIR schema validation
    - Integration test: ingest sample_income_statement.txt, verify line items + FIRs + edges
    - Integration test: trace money flow from entity, verify FLOWS_TO graph
    - Integration test: reconcile two documents, verify match/discrepancy output
    - Integration test: search "revenue" returns relevant line items
14) Run validation:
    - cargo fmt --all --check
    - cargo clippy --workspace --features finance -- -D warnings
    - cargo test -p aether-document
    - cargo test -p aether-finance
    - cargo test -p aether-store
    - cargo test -p aether-mcp --features finance
    - cargo test -p aetherd --features finance
15) Commit with message: "Add AETHER Finance vertical with FIR schema and money flow tracing"

SCOPE GUARD: Do NOT modify the existing code pipeline or legal pipeline. Do NOT
implement currency conversion. Do NOT implement real-time market data feeds. Do NOT
build a finance-specific daemon binary. Do NOT implement tax calculation logic.
Do NOT implement regulatory reporting formats (XBRL, etc.) — that's a future vertical feature.
```
