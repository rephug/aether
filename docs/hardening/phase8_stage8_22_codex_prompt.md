# Codex Prompt — Phase 8.22: Refactor-Prep Deep Scan + Intent Verification

CRITICAL BUILD SETTINGS — use these for ALL cargo commands:
```
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

Read the spec and session context first:
- `docs/roadmap/phase_8_stage_8_22_refactor_prep.md`
- `docs/hardening/phase8_stage8_22_session_context.md`

Then read these source files in order:

**Health infrastructure (you consume these, don't modify):**
- `crates/aether-analysis/src/health.rs` (HealthReport, HealthAnalyzer, HealthSymbolRef)
- `crates/aether-health/src/scoring.rs` (health score computation)
- `crates/aether-health/src/planner.rs` (file split planner — follow this pattern)

**Deep pass infrastructure (reuse, don't rebuild):**
- `crates/aetherd/src/sir_pipeline.rs` (generate_sir_with_retries, deep pass enrichment prompt construction)
- `crates/aetherd/src/indexer.rs` (how deep pass is triggered)

**Store layer (add snapshot persistence):**
- `crates/aether-store/src/schema.rs` (migration pattern — add new migration)
- `crates/aether-store/src/lib.rs` (Store sub-traits — add SnapshotStore)
- `crates/aether-store/src/sir_meta.rs` (SIR metadata — check generation_pass, fingerprint)

**MCP layer (add tools):**
- `crates/aether-mcp/src/tools/router.rs` (tool registration pattern)
- `crates/aether-mcp/src/tools/health.rs` (health tools — follow this pattern)
- `crates/aether-mcp/src/tools/mod.rs` (module declarations)

**CLI layer (add subcommands):**
- `crates/aetherd/src/main.rs` (CLI arg parsing with clap)
- `crates/aetherd/src/health_score.rs` (existing health CLI — follow pattern)

## PREFLIGHT

```bash
cd /home/rephu/projects/aether
git status --porcelain
# Must be clean. If dirty, STOP and report.
git pull --ff-only
```

Create branch and worktree:
```bash
git worktree add ../aether-phase8-refactor-prep -b feature/phase8-stage8-22-refactor-prep
cd /home/rephu/aether-phase8-refactor-prep
```

## SOURCE INSPECTION

Before writing code, inspect the actual source and verify these assumptions.
If any assumption is false, STOP and report:

1. `generate_sir_with_retries` exists in sir_pipeline.rs and is pub or pub(crate)
2. The deep pass prompt construction builds enriched context from:
   neighbor intents + existing SIR + source text
3. HealthReport contains critical_symbols, bottlenecks, cycles, risk_hotspots
4. Each health entry has a symbol_id that maps to the symbols table
5. SirMetaRecord has generation_pass and signature_fingerprint fields
6. The Store sub-trait pattern uses a separate trait + blanket impl on SqliteStore

If `generate_sir_with_retries` is not directly reusable (e.g., it's private
or too coupled to the pipeline), extract the enriched prompt construction
logic into a shared function. Do NOT duplicate the prompt template.

## IMPLEMENTATION

### Part 1: Snapshot persistence (aether-store)

**New file: `crates/aether-store/src/snapshots.rs`**

Add SQLite tables for intent snapshots:

```sql
CREATE TABLE IF NOT EXISTS intent_snapshots (
    snapshot_id   TEXT PRIMARY KEY,
    git_commit    TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    scope         TEXT NOT NULL,
    symbol_count  INTEGER NOT NULL,
    deep_count    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS intent_snapshot_entries (
    snapshot_id           TEXT NOT NULL,
    symbol_id             TEXT NOT NULL,
    qualified_name        TEXT NOT NULL,
    file_path             TEXT NOT NULL,
    signature_fingerprint TEXT NOT NULL,
    sir_json              TEXT NOT NULL,
    generation_pass       TEXT NOT NULL,
    was_deep_scanned      INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (snapshot_id, symbol_id),
    FOREIGN KEY (snapshot_id) REFERENCES intent_snapshots(snapshot_id)
);
```

Add migration to `schema.rs` (follow existing migration pattern — next version number).

Define `SnapshotStore` sub-trait:
```rust
pub trait SnapshotStore {
    fn create_snapshot(&self, snapshot: &IntentSnapshot) -> Result<()>;
    fn get_snapshot(&self, snapshot_id: &str) -> Result<Option<IntentSnapshot>>;
    fn list_snapshots(&self) -> Result<Vec<IntentSnapshotSummary>>;
    fn get_snapshot_entries(&self, snapshot_id: &str) -> Result<Vec<SnapshotEntry>>;
    fn delete_snapshot(&self, snapshot_id: &str) -> Result<()>;
}
```

Implement for SqliteStore. Re-export from lib.rs.

### Part 2: Symbol selection (aether-health)

**New file: `crates/aether-health/src/refactor.rs`**

Pure function — data in, selection out. No Store access.

```rust
pub struct RefactorSymbolSelection {
    pub selected: Vec<RefactorCandidate>,
    pub forced_cycle_members: usize,
    pub skipped_fresh: usize,
}

pub struct RefactorCandidate {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub refactor_risk: f64,
    pub risk_factors: Vec<String>,
    pub needs_deep_scan: bool,  // false if already has fresh deep SIR
    pub in_cycle: bool,
}

pub fn select_refactor_targets(
    health_report: &HealthReport,
    in_scope_symbols: &[SymbolRecord],
    existing_sir_meta: &HashMap<String, SirMetaRecord>,
    top_n: usize,
) -> RefactorSymbolSelection {
    // 1. Score each symbol for refactoring risk
    // 2. Force-include cycle members
    // 3. Mark needs_deep_scan = false if fingerprint matches + generation_pass="deep"
    // 4. Take top-n
}
```

### Part 3: CLI orchestration (aetherd)

**New file: `crates/aetherd/src/refactor_prep.rs`**

Orchestrates the full workflow:
1. Resolve scope (--file or --crate) to list of symbol IDs
2. Load or compute health report
3. Call `select_refactor_targets`
4. For each candidate where needs_deep_scan=true:
   - Build enriched prompt (reuse deep pass prompt logic from sir_pipeline)
   - Call generate_sir_with_retries with deep provider
   - Store result
   - Track cost
5. Create intent snapshot (all in-scope symbols, marking which were deep-scanned)
6. Format and print refactoring brief

**New file: `crates/aetherd/src/verify_intent.rs`**

Post-refactor verification:
1. Load snapshot by ID
2. For each entry: compare current SIR against snapshot SIR
3. Compute similarity (cosine on intent embeddings, or string diff if no embeddings)
4. Flag entries below threshold (default 0.85)
5. Report disappeared/new symbols
6. Print pass/fail summary

**Modify: `crates/aetherd/src/main.rs`**

Add two new clap subcommands:
```
RefactorPrep {
    #[arg(long)]
    file: Option<String>,
    #[arg(long)]
    crate_name: Option<String>,  // use "crate_name" to avoid keyword
    #[arg(long, default_value = "20")]
    top_n: usize,
    #[arg(long)]
    local: bool,
    #[arg(long, default_value = "human")]
    output: OutputFormat,
}

VerifyIntent {
    #[arg(long)]
    snapshot: String,
    #[arg(long, default_value = "0.85")]
    threshold: f64,
}
```

### Part 4: MCP tools (aether-mcp)

**New file: `crates/aether-mcp/src/tools/refactor.rs`**

Two tools: `aether_refactor_prep` and `aether_verify_intent`.

Follow the pattern in `tools/health.rs`:
- Parse input JSON
- Call into the same logic as CLI
- Return structured JSON response

Register both tools in `router.rs`.

### Part 5: Tests

**aether-store tests (snapshots.rs):**
1. Round-trip: create snapshot → get snapshot → verify equality
2. List snapshots returns correct count
3. Delete snapshot removes both header and entries
4. Get entries for nonexistent snapshot returns empty vec

**aether-health tests (refactor.rs):**
1. Selection with empty health report returns empty
2. Cycle members are force-included even if below top-n threshold
3. Fresh deep SIR symbols have needs_deep_scan=false
4. Top-n respects the limit

**aetherd integration test (if feasible with mock provider):**
1. refactor-prep with mock provider produces snapshot and brief
2. verify-intent against unchanged snapshot returns all-pass

## SCOPE GUARD — DO NOT MODIFY

- `crates/aetherd/src/sir_pipeline.rs` — only call into it, do not modify
- `crates/aether-analysis/src/health.rs` — only consume HealthReport
- `crates/aether-health/src/scoring.rs` — do not change scoring formulas
- `crates/aether-health/src/planner.rs` — do not change file planner
- `crates/aether-health/src/planner_communities.rs` — do not touch
- Any existing MCP tool files — do not modify
- Any existing Store sub-trait files — do not modify (add new trait)

## VALIDATION

```bash
cargo fmt --all --check
cargo clippy -p aether-store -- -D warnings
cargo test -p aether-store
cargo clippy -p aether-health -- -D warnings
cargo test -p aether-health
cargo clippy -p aetherd -- -D warnings
cargo test -p aetherd
cargo clippy -p aether-mcp -- -D warnings
cargo test -p aether-mcp
```

## COMMIT

```bash
git add -A
git commit -m "feat(phase8.22): add refactor-prep deep scan and intent verification

- aether refactor-prep: targeted Sonnet deep pass on highest-risk symbols
- Symbol selection by refactoring risk (pagerank, betweenness, test coverage,
  blast radius, cross-community edges)
- Intent snapshot persistence for pre/post comparison
- aether verify-intent: post-refactor semantic drift detection
- MCP tools: aether_refactor_prep, aether_verify_intent
- Skip-if-fresh logic avoids redundant API calls
- Human-readable refactoring brief + JSON output mode"
```

Do NOT push automatically. Report commit SHA and wait for review.

Push command (after review):
```bash
git push -u origin feature/phase8-stage8-22-refactor-prep
```

**PR title:** Phase 8.22 — Refactor-Prep Deep Scan + Intent Verification

**PR body:**
Adds `aether refactor-prep` and `aether verify-intent` CLI subcommands plus
corresponding MCP tools (`aether_refactor_prep`, `aether_verify_intent`).

Refactor-prep uses health reports and graph centrality to identify the
highest-risk symbols in a file or crate, runs targeted Sonnet deep passes
on them, and saves an intent snapshot for post-refactor verification.
Verify-intent compares current SIR against a saved snapshot to detect
unintended semantic changes — even when all tests pass.

Key additions:
- Symbol selection by refactoring risk score (pagerank, betweenness,
  test coverage, blast radius, cross-community edges)
- Targeted deep pass with skip-if-fresh logic
- Intent snapshot persistence (new SQLite migration)
- Refactoring brief output (human-readable + JSON)
- Post-refactor semantic drift detection with configurable threshold
- Two new MCP tools for agent integration
