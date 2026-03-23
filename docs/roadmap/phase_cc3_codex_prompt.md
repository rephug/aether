# Claude Code Prompt — Stage CC.3: aether_audit_candidates MCP Tool

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
git worktree add -B feature/cc3-audit-candidates /home/rephu/feature/cc3-audit-candidates
cd /home/rephu/feature/cc3-audit-candidates
```

---

## Context

This stage adds the highest-value composite MCP tool: `aether_audit_candidates`.
It combines structural health data (pagerank, betweenness, test coverage,
dependency cycles) with SIR metadata (confidence, generation_pass) and reasoning
trace uncertainty signals to produce a ranked list of symbols most in need of
deep human or AI review.

This tool is what turns Claude Code from "review this file" into "review these
specific 20 symbols that AETHER's structural analysis flagged as highest risk,
with reasoning hints about what to look for."

**Prerequisite:** Reasoning trace data must be populated. Run triage with thinking
enabled before testing this tool.

---

## Source Inspection (MANDATORY — do this before writing any code)

1. Read `crates/aether-mcp/src/tools/health.rs` — understand how health data
   is accessed. Find how `aether_health_logic` queries health reports, what
   data structures it uses (`ScoreReport`, `FileSymbol`, etc.), and how it
   accesses the store and graph.

2. Read `crates/aether-mcp/src/tools/audit.rs` — this file already exists from
   Stage CC.2. Understand the existing types and logic methods. The new tool
   will be added to this file.

3. Read `crates/aether-analysis/src/health.rs` — understand `compute_health_report`.
   Find what fields are available on health report symbols: pagerank, betweenness,
   in_cycle, test_count, risk_score, etc.

4. Read `crates/aether-store/src/sir_meta.rs` — find how to query SIR metadata
   for a symbol. Look for methods that return `generation_pass`, `confidence`
   (from sir_json), and `reasoning_trace`.

5. Check how `sir_json` is parsed elsewhere:
   ```bash
   grep -rn "sir_json\|SirJson\|serde_json.*sir" --include="*.rs" crates/aether-store/ crates/aether-mcp/ | head -20
   ```
   Understand how confidence is extracted from the JSON blob.

6. Check reasoning_trace access:
   ```bash
   grep -rn "reasoning_trace" --include="*.rs" crates/aether-store/src/ crates/aether-mcp/src/ | head -20
   ```

---

## Implementation

### Step 1: Add request/response types to audit.rs

In `crates/aether-mcp/src/tools/audit.rs`, add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAuditCandidatesRequest {
    /// Maximum number of candidates to return (default 20)
    pub top_n: Option<u32>,
    /// Scope to a specific crate (matches file_path prefix "crates/<name>/")
    pub crate_filter: Option<String>,
    /// Scope to a specific file path
    pub file_filter: Option<String>,
    /// Minimum structural risk score (0.0-1.0, default 0.0)
    pub min_risk: Option<f64>,
    /// Include reasoning_trace excerpts in output (default true)
    pub include_reasoning_hints: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AuditCandidate {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub kind: String,
    /// Composite risk score (0.0-1.0, higher = more risky)
    pub risk_score: f64,
    /// Human-readable risk factors
    pub risk_factors: Vec<String>,
    /// SIR confidence from current generation (if available)
    pub current_confidence: Option<f64>,
    /// Which pass generated the current SIR
    pub generation_pass: Option<String>,
    /// Excerpt from reasoning_trace highlighting uncertainty
    pub reasoning_hint: Option<String>,
    /// Composite priority score (0.0-1.0, higher = more urgent to audit)
    pub audit_priority: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAuditCandidatesResponse {
    pub candidates: Vec<AuditCandidate>,
    pub total_in_scope: u32,
    pub scope_description: String,
}
```

### Step 2: Implement the logic

Add `aether_audit_candidates_logic` method on `AetherMcpServer`.

**Algorithm:**

1. **Get structural health data:** Use the same approach as `aether_health_logic`
   to compute or retrieve the health report. Extract per-symbol metrics:
   pagerank, betweenness_centrality, in_cycle, test_count, risk_score.

2. **Get SIR metadata:** For each symbol in scope, query the `sir` table for
   `generation_pass`, `reasoning_trace`, and parse `sir_json` for confidence.
   Use a single query with `WHERE id IN (...)` rather than N individual queries.

3. **Apply scope filters:**
   - If `crate_filter` is set, filter to symbols whose file_path starts with
     `crates/<crate_filter>/`
   - If `file_filter` is set, filter to symbols in that file
   - If `min_risk` is set, exclude symbols below that risk score

4. **Compute audit_priority:** Combine signals into a single priority score:
   ```
   structural_risk = normalized risk_score from health (0.0-1.0)
   sir_uncertainty = 1.0 - confidence (or 1.0 if no SIR)
   reasoning_uncertainty = 0.3 if reasoning_trace contains uncertainty signals, else 0.0
   pass_factor = match generation_pass {
       "scan" | None => 0.3,    // never had proper analysis
       "triage" => 0.1,         // had baseline but not deep
       "deep" => 0.0,           // already deeply analyzed
   }

   audit_priority = 0.50 * structural_risk
                  + 0.25 * sir_uncertainty
                  + 0.15 * reasoning_uncertainty
                  + 0.10 * pass_factor
   ```

5. **Extract reasoning hints:** If `include_reasoning_hints` is true (default),
   scan `reasoning_trace` for uncertainty signals. Look for substrings:
   "uncertain", "unsure", "cannot determine", "unclear", "might", "possibly",
   "latent", "cannot trace", "difficult to assess". Extract a ~200 char window
   around the first match as the hint.

6. **Build risk_factors list:** For each candidate, build human-readable factors:
   - "high_betweenness" if betweenness > 0.1 (adjust threshold based on source inspection)
   - "in_cycle" if the symbol is in a dependency cycle
   - "low_test_coverage" if test_count == 0
   - "high_pagerank" if pagerank is in top 10%
   - "no_deep_analysis" if generation_pass != "deep"
   - "low_confidence" if confidence < 0.7
   - "triage_uncertainty" if reasoning_trace has uncertainty signals

7. **Sort by audit_priority DESC, take top_n.**

### Step 3: Register in router.rs

Add to the `#[tool_router]` impl block:

```rust
#[tool(
    name = "aether_audit_candidates",
    description = "Get ranked list of symbols most in need of deep audit review, combining structural risk with SIR confidence and reasoning trace uncertainty"
)]
pub async fn aether_audit_candidates(
    &self,
    Parameters(request): Parameters<AetherAuditCandidatesRequest>,
) -> Result<Json<AetherAuditCandidatesResponse>, McpError> {
    self.verbose_log("MCP tool called: aether_audit_candidates");
    let server = self.clone();
    tokio::task::spawn_blocking(move || server.aether_audit_candidates_logic(request))
        .await
        .map_err(|err| McpError::internal_error(err.to_string(), None))?
        .map(Json)
        .map_err(to_mcp_error)
}
```

Add the type imports.

### Step 4: Export types

Update `crates/aether-mcp/src/tools/mod.rs` to re-export the new types.

### Step 5: Tests

Add tests in `crates/aether-mcp/src/tools/audit.rs` or the test module:

1. Test that `aether_audit_candidates_logic` returns candidates sorted by
   audit_priority descending.
2. Test crate_filter scoping.
3. Test min_risk filtering.
4. Test reasoning hint extraction from sample reasoning_trace text.
5. Test empty result when no symbols match filters.

---

## Scope guard

**Modified files: `crates/aether-mcp/src/tools/audit.rs`, `crates/aether-mcp/src/tools/mod.rs`, `crates/aether-mcp/src/tools/router.rs`.**

Do NOT modify store schema, health computation, or any other crates. This tool
queries existing data — it does not write.

---

## Validation gate

```bash
cargo fmt --all --check
cargo clippy -p aether-mcp -- -D warnings
cargo test -p aether-mcp
```

Do NOT run `cargo test --workspace` — OOM risk.

All commands must pass before committing.

---

## Commit

```bash
git add -A
git commit -m "feat(mcp): aether_audit_candidates — ranked audit target selection with reasoning hints"
```

**PR title:** `feat(mcp): aether_audit_candidates — ranked audit target selection with reasoning hints`

**PR body:**
```
Stage CC.3 of the Claude Code Audit Integration phase.

Adds aether_audit_candidates MCP tool that combines:
- Structural risk signals (pagerank, betweenness, cycles, test coverage)
- SIR confidence scores from triage/deep passes
- Reasoning trace uncertainty detection (keyword scanning)
- Generation pass freshness (scan < triage < deep)

Returns ranked candidates with composite audit_priority score,
human-readable risk_factors, and reasoning_hint excerpts showing
where the triage model expressed uncertainty.

Supports scoping by crate, file, and minimum risk threshold.
```

---

## Post-commit

```bash
git push origin feature/cc3-audit-candidates
# Create PR via GitHub web UI with title + body above
# After merge:
git switch main && git pull --ff-only
git worktree remove /home/rephu/feature/cc3-audit-candidates
git branch -D feature/cc3-audit-candidates
```
