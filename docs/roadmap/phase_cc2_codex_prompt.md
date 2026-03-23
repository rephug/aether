# Claude Code Prompt — Stage CC.2: sir_audit Table + Audit MCP Tools + aether_sir_inject

## Preamble

```bash
# Preflight
cd /home/rephu/projects/aether
git status --porcelain        # Must be clean
git pull --ff-only

# Build environment
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR

# Branch + worktree
git worktree add -B feature/cc2-audit-table /home/rephu/feature/cc2-audit-table
cd /home/rephu/feature/cc2-audit-table
```

---

## Context

This stage adds:
1. A dedicated `sir_audit` table for structured audit findings
2. Three audit MCP tools (submit, report, resolve)
3. `aether_sir_inject` MCP tool — closes the feedback loop so Claude Code can
   write improved SIR annotations back to the store via MCP instead of bash

The `sir_inject` MCP tool is critical for the audit workflow: find bug → improve
SIR → inject back → persists for future sessions. Currently `sir-inject` is
CLI-only, which means Claude Code has to construct bash commands with JSON escaping.

Current schema version is 17. This stage bumps to 18.

---

## Source Inspection (MANDATORY — do this before writing any code)

1. Read `crates/aether-store/src/schema.rs` — understand the migration pattern.
   Find the `if version < 17` block and the `ensure_sir_column` helper. Note
   how `PRAGMA user_version` is set. Note the `schema_version` table at the end.

2. Read `crates/aether-store/src/sir_meta.rs` — understand how existing store
   methods are structured (query patterns, error handling, connection access).

3. Read `crates/aether-mcp/src/tools/memory.rs` — understand the pattern for
   MCP tools that do store operations. Note how `AetherRememberRequest` and
   `AetherRememberResponse` are structured, how logic methods are implemented.

4. Read `crates/aether-mcp/src/tools/router.rs` — understand tool registration
   pattern. Note the `#[tool_router]` macro, the `to_mcp_error` helper, the
   `spawn_blocking` pattern for sync store calls.

5. Read `crates/aether-mcp/src/tools/mod.rs` — understand how tool types are
   exported and where server constants live.

6. Find ALL `check_compatibility("core", 17)` call sites:
   ```bash
   grep -rn 'check_compatibility.*core.*17' --include="*.rs"
   ```
   ALL of these must be updated to 18.

7. Find ALL test assertions on schema_version:
   ```bash
   grep -rn 'schema_version\|user_version.*17' --include="*.rs" crates/aether-store/src/tests/ crates/aether-mcp/src/tests/ crates/aether-dashboard/src/tests.rs
   ```

8. Read `crates/aetherd/src/sir_inject.rs` — understand the CLI sir-inject flow.
   Note the key operations: resolve_symbol, read existing SIR, merge fields,
   persist via SirPipeline, refresh embeddings. The MCP tool will replicate the
   core persist logic using store methods directly since SirPipeline is in aetherd
   (not a library crate) and can't be called from the MCP binary.

9. Read `crates/aether-sir/src/lib.rs` (or wherever `canonicalize_sir_json` and
   `sir_hash` are defined) — these are the library functions for SIR serialization
   that the MCP tool will use instead of SirPipeline.

10. Check store methods available for SIR writes:
    ```bash
    grep "fn write_sir_blob\|fn upsert_sir_meta\|fn record_sir_version" crates/aether-store/src/lib.rs
    ```
    These three methods are the building blocks for SIR persistence.

11. Read `crates/aetherd/src/cli.rs` — find how existing subcommands are defined
   (e.g., `TaskHistory`, `SirInject`) to understand the pattern for adding
   `AuditReport`.

9. Read `crates/aetherd/src/main.rs` — find how subcommands are dispatched in
   the `Commands` enum match arm.

---

## Implementation

### Step 1: Schema migration v18 — sir_audit table

In `crates/aether-store/src/schema.rs`, add a new migration block after the
`if version < 17` block:

```rust
if version < 18 {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS sir_audit (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            symbol_id TEXT NOT NULL,
            audit_type TEXT NOT NULL,
            severity TEXT NOT NULL,
            category TEXT NOT NULL,
            certainty TEXT NOT NULL,
            trigger_condition TEXT NOT NULL,
            impact TEXT NOT NULL,
            description TEXT NOT NULL,
            related_symbols TEXT DEFAULT '[]',
            model TEXT NOT NULL,
            provider TEXT NOT NULL,
            reasoning TEXT,
            status TEXT NOT NULL DEFAULT 'open',
            created_at INTEGER NOT NULL,
            resolved_at INTEGER,
            FOREIGN KEY (symbol_id) REFERENCES sir(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_audit_symbol ON sir_audit(symbol_id);
        CREATE INDEX IF NOT EXISTS idx_audit_severity ON sir_audit(severity);
        CREATE INDEX IF NOT EXISTS idx_audit_status ON sir_audit(status);
        CREATE INDEX IF NOT EXISTS idx_audit_category ON sir_audit(category);
        "#,
    )?;
    conn.execute("PRAGMA user_version = 18", [])?;
}
```

### Step 2: Store methods for audit findings

Create `crates/aether-store/src/audit.rs` with:

1. **Data types:**
   ```rust
   pub struct AuditFinding {
       pub id: i64,
       pub symbol_id: String,
       pub audit_type: String,        // "symbol" or "cross_symbol"
       pub severity: String,          // "critical", "high", "medium", "low", "informational"
       pub category: String,          // "arithmetic", "encoding", "silent_failure", etc.
       pub certainty: String,         // "confirmed", "suspected", "theoretical"
       pub trigger_condition: String,
       pub impact: String,
       pub description: String,
       pub related_symbols: String,   // JSON array of symbol IDs
       pub model: String,
       pub provider: String,
       pub reasoning: Option<String>,
       pub status: String,            // "open", "confirmed", "wontfix", "fixed"
       pub created_at: i64,
       pub resolved_at: Option<i64>,
   }
   ```

2. **`insert_audit_finding()`** — inserts a row, returns the new id.
   Set `created_at` to `chrono::Utc::now().timestamp()` (or use the existing
   timestamp pattern from the store crate).

3. **`query_audit_findings()`** — accepts optional filters: `symbol_id`,
   `severity` (minimum), `category`, `status`, `limit`. Returns `Vec<AuditFinding>`.
   Use parameterized queries. Severity ordering: critical=0, high=1, medium=2,
   low=3, informational=4 — filter by `severity <= threshold`.
   Default sort: severity ASC, created_at DESC.

4. **`resolve_audit_finding()`** — updates `status` and `resolved_at` for a
   given finding id. Returns `Ok(true)` if a row was updated, `Ok(false)` if
   id not found.

5. **`count_audit_findings_by_severity()`** — returns a summary struct with
   counts per severity level for a given scope (optional crate filter by
   joining on symbol file_path).

Add `pub mod audit;` to `crates/aether-store/src/lib.rs` and re-export the types.

Follow the existing patterns in the store crate for connection access and error
handling. Look at how `sir_meta.rs` or `project_note.rs` handle similar operations.

### Step 3: MCP tools — audit.rs

Create `crates/aether-mcp/src/tools/audit.rs` with three MCP tools.

**Request/Response types:**

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAuditSubmitRequest {
    pub symbol_id: String,
    pub audit_type: Option<String>,       // default "symbol"
    pub severity: String,                  // required
    pub category: String,                  // required
    pub certainty: String,                 // required
    pub trigger_condition: String,         // required
    pub impact: String,                    // required
    pub description: String,              // required
    pub related_symbols: Option<Vec<String>>,
    pub model: Option<String>,            // default "claude_code"
    pub provider: Option<String>,         // default "manual"
    pub reasoning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAuditSubmitResponse {
    pub finding_id: i64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAuditReportRequest {
    pub crate_filter: Option<String>,      // filter by crate name (matches file_path prefix)
    pub min_severity: Option<String>,      // minimum severity to include
    pub category: Option<String>,
    pub status: Option<String>,            // default "open"
    pub limit: Option<u32>,                // default 50
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAuditFindingOutput {
    pub id: i64,
    pub symbol_id: String,
    pub qualified_name: Option<String>,    // resolved from symbols table
    pub file_path: Option<String>,         // resolved from symbols table
    pub audit_type: String,
    pub severity: String,
    pub category: String,
    pub certainty: String,
    pub trigger_condition: String,
    pub impact: String,
    pub description: String,
    pub related_symbols: Vec<String>,
    pub model: String,
    pub status: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAuditReportResponse {
    pub findings: Vec<AetherAuditFindingOutput>,
    pub summary: AuditSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AuditSummary {
    pub total: u32,
    pub critical: u32,
    pub high: u32,
    pub medium: u32,
    pub low: u32,
    pub informational: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAuditResolveRequest {
    pub finding_id: i64,
    pub status: String,                    // "fixed", "wontfix", "confirmed"
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAuditResolveResponse {
    pub finding_id: i64,
    pub new_status: String,
    pub resolved: bool,
}
```

**Logic methods** — implement on `AetherMcpServer`:
- `aether_audit_submit_logic` — validate severity/category/certainty values,
  call store insert, return finding_id.
- `aether_audit_report_logic` — call store query with filters, resolve symbol
  qualified_name and file_path by looking up the symbols table, build response
  with summary counts.
- `aether_audit_resolve_logic` — validate status value, call store resolve,
  return result.

### Step 4: MCP tool — aether_sir_inject

Add to `crates/aether-mcp/src/tools/audit.rs` (or a new `sir_inject.rs` if
the file is getting large):

**Request/Response types:**

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSirInjectRequest {
    /// Symbol ID or qualified name selector
    pub symbol: String,
    /// New intent text (required)
    pub intent: String,
    /// Side effects / behavior summary (optional — merged if provided)
    pub side_effects: Option<Vec<String>>,
    /// Error modes / edge cases (optional — merged if provided)
    pub error_modes: Option<Vec<String>>,
    /// Confidence score (0.0-1.0, default 0.5)
    pub confidence: Option<f32>,
    /// Generation pass label (default "deep")
    pub generation_pass: Option<String>,
    /// Model name for provenance (default "claude_code")
    pub model: Option<String>,
    /// Provider name for provenance (default "manual")
    pub provider: Option<String>,
    /// Force overwrite even if existing SIR has higher confidence
    pub force: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSirInjectResponse {
    pub symbol_id: String,
    pub qualified_name: String,
    pub sir_hash: String,
    pub sir_version: i64,
    pub previous_confidence: Option<f32>,
    pub new_confidence: f32,
    pub status: String,  // "injected", "blocked" (if confidence too high and !force)
    pub note: Option<String>,  // e.g. "embeddings not refreshed — run aetherd regenerate --embed-only"
}
```

**Logic method** — implement `aether_sir_inject_logic` on `AetherMcpServer`:

The MCP tool replicates the core persist logic from `aetherd/src/sir_inject.rs`
using store methods directly. It CANNOT use `SirPipeline` because that lives in
the `aetherd` binary crate, not a library crate. Instead, use these store methods
from the `SirStateStore` trait:

1. **Resolve symbol:** Use `SymbolCatalogStore::resolve_symbol_by_id_or_name`
   or the equivalent method to find the symbol. Check the existing store methods
   for symbol lookup (look at how other MCP tools resolve symbols from selectors).

2. **Read existing SIR:** Call `store.read_sir_blob(symbol_id)` to get current
   SIR JSON. Parse with `serde_json::from_str::<SirAnnotation>()`.

3. **Check confidence blocking:** If existing SIR confidence > 0.5 and !force,
   return response with `status: "blocked"`.

4. **Merge fields:**
   ```rust
   let mut updated = existing_sir.unwrap_or_else(|| SirAnnotation::default());
   updated.intent = request.intent.trim().to_owned();
   if let Some(side_effects) = request.side_effects {
       updated.side_effects = side_effects;
   }
   if let Some(error_modes) = request.error_modes {
       updated.error_modes = error_modes;
   }
   updated.confidence = request.confidence.unwrap_or(0.5);
   ```

5. **Canonicalize and hash:** Use `aether_sir::canonicalize_sir_json(&updated)`
   and `aether_sir::sir_hash(&updated)` from the `aether-sir` library crate.

6. **Write to store:**
   ```rust
   let canonical_json = canonicalize_sir_json(&updated);
   let hash = sir_hash(&updated);
   store.write_sir_blob(symbol_id, &canonical_json)?;
   ```

7. **Update metadata:**
   ```rust
   let provider = request.provider.unwrap_or_else(|| "manual".to_owned());
   let model = request.model.unwrap_or_else(|| "claude_code".to_owned());
   let generation_pass = request.generation_pass.unwrap_or_else(|| "deep".to_owned());
   let now = chrono::Utc::now().timestamp();
   store.upsert_sir_meta(SirMetaRecord {
       id: symbol_id.to_owned(),
       sir_hash: hash.clone(),
       sir_version: existing_meta.map(|m| m.sir_version + 1).unwrap_or(1),
       provider: provider.clone(),
       model: model.clone(),
       generation_pass,
       reasoning_trace: None,
       prompt_hash: None,
       staleness_score: None,
       updated_at: now,
       sir_status: "fresh".to_owned(),
       last_error: None,
       last_attempt_at: now,
   })?;
   ```

8. **Record history:**
   ```rust
   store.record_sir_version_if_changed(
       symbol_id,
       &hash,
       &provider,
       &model,
       &canonical_json,
       now,
       None,  // commit_hash
   )?;
   ```

9. **Return response** with `status: "injected"` and a note:
   `"Embeddings not refreshed — run 'aetherd regenerate --embed-only' if semantic search accuracy matters"`

**Important:** The MCP tool intentionally skips embedding refresh. The CLI
`sir-inject` uses `SirPipeline::new_embeddings_only()` which requires inference
provider initialization. The MCP server doesn't have that wired up, and embedding
refresh is a nice-to-have (the SIR is still searchable by lexical/intent text).
Add the note to the response so the user knows.

### Step 5: Register ALL tools in router.rs

Add the four new tools to the `#[tool_router]` impl block in `router.rs`:

```rust
#[tool(
    name = "aether_sir_inject",
    description = "Write an improved SIR annotation back to the store for a symbol"
)]
pub async fn aether_sir_inject(
    &self,
    Parameters(request): Parameters<AetherSirInjectRequest>,
) -> Result<Json<AetherSirInjectResponse>, McpError> {
    self.verbose_log("MCP tool called: aether_sir_inject");
    let server = self.clone();
    tokio::task::spawn_blocking(move || server.aether_sir_inject_logic(request))
        .await
        .map_err(|err| McpError::internal_error(err.to_string(), None))?
        .map(Json)
        .map_err(to_mcp_error)
}

#[tool(
    name = "aether_audit_submit",
    description = "Submit a structured audit finding for a symbol"
)]
pub async fn aether_audit_submit(
    &self,
    Parameters(request): Parameters<AetherAuditSubmitRequest>,
) -> Result<Json<AetherAuditSubmitResponse>, McpError> {
    self.verbose_log("MCP tool called: aether_audit_submit");
    let server = self.clone();
    tokio::task::spawn_blocking(move || server.aether_audit_submit_logic(request))
        .await
        .map_err(|err| McpError::internal_error(err.to_string(), None))?
        .map(Json)
        .map_err(to_mcp_error)
}

#[tool(
    name = "aether_audit_report",
    description = "Query audit findings by crate, severity, category, or status"
)]
pub async fn aether_audit_report(
    &self,
    Parameters(request): Parameters<AetherAuditReportRequest>,
) -> Result<Json<AetherAuditReportResponse>, McpError> {
    self.verbose_log("MCP tool called: aether_audit_report");
    let server = self.clone();
    tokio::task::spawn_blocking(move || server.aether_audit_report_logic(request))
        .await
        .map_err(|err| McpError::internal_error(err.to_string(), None))?
        .map(Json)
        .map_err(to_mcp_error)
}

#[tool(
    name = "aether_audit_resolve",
    description = "Mark an audit finding as fixed, wontfix, or confirmed"
)]
pub async fn aether_audit_resolve(
    &self,
    Parameters(request): Parameters<AetherAuditResolveRequest>,
) -> Result<Json<AetherAuditResolveResponse>, McpError> {
    self.verbose_log("MCP tool called: aether_audit_resolve");
    let server = self.clone();
    tokio::task::spawn_blocking(move || server.aether_audit_resolve_logic(request))
        .await
        .map_err(|err| McpError::internal_error(err.to_string(), None))?
        .map(Json)
        .map_err(to_mcp_error)
}
```

Add the necessary type imports in the `use super::{...}` block at the top of
router.rs.

### Step 6: Update check_compatibility calls

**CRITICAL — this caused 3 CI round-trips on Phase 10.4. Do not miss any.**

Update ALL of these from 17 to 18:
- `crates/aether-mcp/src/state.rs` — find every `check_compatibility("core", 17)` and change to 18
- `crates/aether-dashboard/src/state.rs` — same

Search for ANY test assertions that reference schema version 17 and update them.

```bash
grep -rn '"core".*17\|version.*17' --include="*.rs" crates/aether-mcp/ crates/aether-dashboard/ crates/aether-store/src/tests/
```

### Step 7: Export types from mod.rs

In `crates/aether-mcp/src/tools/mod.rs`:
1. Add `pub mod audit;`
2. Add the necessary `pub use audit::{...}` re-exports for all request/response types
   (including `AetherSirInjectRequest`, `AetherSirInjectResponse`)

### Step 8: CLI command — audit-report

In `crates/aetherd/src/cli.rs`:
1. Add `AuditReport` variant to the `Commands` enum with fields:
   - `--crate` (optional String)
   - `--min-severity` (optional String, default "low")
   - `--status` (optional String, default "open")
   - `--limit` (optional u32, default 50)

In `crates/aetherd/src/main.rs`:
1. Add `Commands::AuditReport(args) => run_audit_report(workspace, args)` dispatch
2. Implement `run_audit_report()` — opens store, calls `query_audit_findings`,
   prints formatted table to stdout. Format:
   ```
   AETHER Audit Report
   ===================
   Scope: aether-store | Status: open | Min severity: low

   [HIGH] silent_failure — reconcile_and_prune
     File: crates/aether-store/src/lifecycle.rs
     Description: sir_quality rows orphaned on reconciliation
     Certainty: confirmed
     Found: 2026-03-23

   Summary: 0 critical, 3 high, 7 medium, 2 low (12 total)
   ```

### Step 9: Tests

**Store tests** in `crates/aether-store/src/tests/` or `crates/aether-store/src/audit.rs`:
1. Test `insert_audit_finding` + `query_audit_findings` round-trip
2. Test severity filtering
3. Test `resolve_audit_finding` updates status and resolved_at
4. Test `count_audit_findings_by_severity`

**MCP tool tests** — if the existing test infrastructure in
`crates/aether-mcp/tests/mcp_tools.rs` supports adding tests, add basic
integration tests. Otherwise, add unit tests in `audit.rs` itself:
1. Test audit submit + report + resolve flow
2. Test `aether_sir_inject_logic`:
   - Inject into symbol with no existing SIR → succeeds, status "injected"
   - Inject into symbol with existing high-confidence SIR without force → status "blocked"
   - Inject with force=true → succeeds even with high existing confidence
   - Verify sir_hash and sir_version are set correctly after inject

---

## Scope guard

**Modified crates: `aether-store`, `aether-mcp`, `aetherd`.**

Do NOT modify `aether-analysis`, `aether-health`, `aether-infer`, `aether-config`,
`aether-graph-algo`, or any other crate.

---

## Validation gate

```bash
cargo fmt --all --check
cargo clippy -p aether-store -p aether-mcp -p aetherd -- -D warnings
cargo test -p aether-store
cargo test -p aether-mcp
cargo test -p aetherd
```

Do NOT run `cargo test --workspace` — OOM risk.

All commands must pass before committing.

---

## Commit

```bash
git add -A
git commit -m "feat(store,mcp): sir_audit table (schema v18) + audit MCP tools + aether_sir_inject"
```

**PR title:** `feat(store,mcp): sir_audit table (schema v18) + audit MCP tools + aether_sir_inject`

**PR body:**
```
Stage CC.2 of the Claude Code Audit Integration phase.

Changes:
- Schema v18: add sir_audit table for structured audit findings with
  severity, category, certainty, status tracking, and symbol linkage
- Store methods: insert_audit_finding, query_audit_findings,
  resolve_audit_finding, count_audit_findings_by_severity
- MCP tools: aether_audit_submit, aether_audit_report, aether_audit_resolve
- MCP tool: aether_sir_inject — write improved SIR annotations back to store
  via MCP (previously CLI-only). Uses store methods directly instead of
  SirPipeline. Skips embedding refresh (note in response).
- CLI: aetherd audit-report [--crate X] [--min-severity Y] [--status Z]
- Updated check_compatibility("core", 18) in all 5 call sites

Decisions: #104, #105
```

---

## Post-commit

```bash
git push origin feature/cc2-audit-table
# Create PR via GitHub web UI with title + body above
# After merge:
git switch main && git pull --ff-only
git worktree remove /home/rephu/feature/cc2-audit-table
git branch -D feature/cc2-audit-table
```
