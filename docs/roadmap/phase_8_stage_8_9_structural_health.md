# Phase 8 — Stage 8.9: Structural Health Score

**Codename:** Health Meter
**Depends on:** Phase 8.8 merged, post-8.8 fixes merged, clean main
**New crates:** `aether-health`
**Modified crates:** `aetherd` (new subcommand), `aether-config` (score thresholds)

---

## Purpose

Give AETHER a built-in codebase health score that produces measurable before/after data, tracks regression over time, and provides explainable diagnostics — not just numbers.

This stage is **structural only**: pure static analysis of source files. No SIR data required, no running daemon, no indexed workspace. Works on any Rust workspace including AETHER itself. Ships as `aether health-score --workspace .`.

The score answers three questions:

1. **How healthy is this codebase right now?** — A normalized 0–100 score per crate and per workspace.
2. **Why is something unhealthy?** — Top violations with plain-English reasons and an archetype label.
3. **Is it getting better or worse?** — Score history with delta tracking per git commit.

A future stage (8.10) extends this with semantic signals from SIR, graph, drift, and community data behind a `--semantic` flag.

---

## In scope

- New `aether-health` crate with structural scoring engine
- New `health-score` subcommand in `aetherd`
- Normalized 0–100 score per crate and workspace aggregate
- Four archetype labels assigned from structural signals
- Per-crate violation list with human-readable reasons
- Score history in `.aether/meta.sqlite` with git commit tracking
- Console table output + JSON output for CI
- Configurable thresholds in `.aether/config.toml`
- CI integration: non-zero exit code when score exceeds threshold

## Out of scope

- Semantic/SIR-derived metrics (Stage 8.10)
- Dashboard integration (future dashboard stage)
- Symbol-level scoring (requires SIR data)
- Split planning / refactor recommendations (requires semantic data)
- Language support beyond Rust
- MCP or LSP integration (plan API surface, ship CLI only)

---

## Score Model

### Score range

**0–100**, where lower is healthier:

| Range | Severity | Meaning |
|-------|----------|---------|
| 0–24 | Healthy | No action needed |
| 25–49 | Watch | Minor issues accumulating |
| 50–69 | Moderate | Active cleanup warranted |
| 70–84 | High | Structural problems impeding development |
| 85–100 | Critical | Urgent refactoring needed |

Crate scores are computed independently. Workspace score is the **weighted average** of crate scores, weighted by each crate's LOC as a fraction of total workspace LOC. This prevents a single tiny crate with one issue from skewing the workspace number.

### Per-crate metrics

All metrics are computed by walking the workspace source tree. No `cargo` invocation required.

| Metric | Source | Warn | Fail | Weight |
|--------|--------|------|------|--------|
| `max_file_loc` | Largest `.rs` file in `src/` (non-blank, non-comment lines) | 800 | 1500 | 0.20 |
| `trait_method_max` | Largest `pub trait` method count in crate | 20 | 35 | 0.20 |
| `internal_dep_count` | Count of `aether-*` deps in `Cargo.toml` | 6 | 10 | 0.15 |
| `todo_density` | `TODO` + `FIXME` per 1000 non-blank lines | 5 | 15 | 0.10 |
| `dead_feature_flags` | `#[cfg(feature = "legacy-*")]` occurrences | 1 | 5 | 0.15 |
| `stale_backend_refs` | Occurrences of `CozoGraphStore\|cozo\|CozoDB` in non-test code | 1 | 3 | 0.20 |

Note: `stale_backend_refs` is generalized from "cozo references" — the metric name is backend-agnostic so it can be extended to any future deprecated backend without renaming.

### Informational metrics (no penalty, displayed in output)

| Metric | Source |
|--------|--------|
| `total_loc` | Sum of non-blank, non-comment lines in `src/` |
| `file_count` | Number of `.rs` files in `src/` |
| `total_lines` | All lines including blank/comment |

### Score computation

Each metric produces a raw penalty:

```
raw_penalty(metric) =
    0                                                              if value <= warn
    (value - warn) / (fail - warn)                                 if warn < value <= fail
    1.0 + 0.5 * (value - fail) / fail                             if value > fail
```

Raw penalty is clamped to [0.0, 2.0].

Per-metric contribution = `raw_penalty * weight * 100`.

Crate score = sum of all metric contributions, clamped to [0, 100].

The weights and formula are intentionally simple. Users see exactly why the score is what it is — no black box.

---

## Archetypes

Each crate with score ≥ 25 is assigned one primary archetype based on which signal bucket contributes the most penalty. These are labels, not judgments — shorthand for "what kind of problem is this."

| Archetype | Trigger | What it means |
|-----------|---------|---------------|
| **God File** | `max_file_loc` or `trait_method_max` is the top contributor | Too many responsibilities concentrated in one place |
| **Brittle Hub** | `internal_dep_count` is the top contributor | Central coupling point — changes here ripple everywhere |
| **Churn Magnet** | `todo_density` or `dead_feature_flags` is the top contributor | Accumulating maintenance debt that never gets resolved |
| **Legacy Residue** | `stale_backend_refs` is the top contributor | Deprecated code paths still present after migration |

If a crate scores ≥ 25 and multiple signal buckets are within 10% of each other, assign up to two archetypes.

Crates scoring < 25 get no archetype label.

---

## Explainability

Every crate with score ≥ 25 produces a violations list. Each violation includes:

- **Metric name** and current value
- **Threshold exceeded** (warn or fail)
- **Severity** (warn or fail)
- **Reason** — a single sentence explaining why this matters

Example reasons (generated from templates, not LLM):

- `"Store trait has 52 methods — interfaces this large are hard to implement, test, and evolve independently"`
- `"aether-mcp depends on 9 internal crates — high fan-in means changes propagate widely"`
- `"14 CozoDB references remain in non-test code — migration to SurrealDB is incomplete"`
- `"config/src/lib.rs is 2,611 lines — large files are harder to navigate and reason about"`

The reason templates are defined as constants in `aether-health/src/explanations.rs`. No LLM calls, no runtime cost.

---

## Score History

When `--workspace` points to a directory with `.aether/meta.sqlite`, the command writes results and displays the delta vs the previous run.

Schema:

```sql
CREATE TABLE IF NOT EXISTS health_score_history (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    run_at      INTEGER NOT NULL,          -- unix seconds
    git_commit  TEXT,                       -- HEAD sha if available, NULL otherwise
    score       INTEGER NOT NULL,           -- workspace score 0-100
    score_json  TEXT NOT NULL,              -- full serialized ScoreReport as JSON
    UNIQUE(git_commit)
);
```

If `.aether/` does not exist (running against a non-indexed workspace), history is skipped silently. No error, no warning.

---

## CLI Interface

```
aetherd health-score [OPTIONS]

Options:
  --workspace <path>      Workspace root (default: .)
  --output <table|json>   Output format (default: table)
  --fail-above <N>        Exit code 1 if workspace score > N (default: disabled)
  --no-history            Skip reading/writing score history
  --crate <name>          Scope to specific crate(s) (repeatable)
```

### Example table output

```
AETHER Health Score — /home/rephu/projects/aether
Run: 2026-03-10 14:22  |  Git: a1b2c3d  |  Score: 62/100 (Moderate)  |  Delta: -5 ↓

Crate                  Score  LOC    Files  Archetype
────────────────────────────────────────────────────────────
aether-store             78   6493      9   God File
aether-mcp               71   4408      3   Brittle Hub, Legacy Residue
aetherd                  54  10996     25   Brittle Hub
aether-config            44   2611      1   God File
aether-analysis          38   6472      7   Churn Magnet
aether-infer             22   2842      7   —
aether-lsp               18   1889      1   —
aether-dashboard         12  17947     67   —
aether-parse              8   2608      7   —
aether-memory             6   1972      6   —
aether-query              4    759      5   —
aether-core               0    753      3   —
aether-sir                0    252      1   —
aether-graph-algo         0    664      1   —
aether-document           0    965     10   —

Top issues:
  [FAIL] aether-store:  Store trait has 52 methods (threshold: 35)
  [FAIL] aether-mcp:    14 stale backend references in non-test code
  [FAIL] aetherd:       13 internal crate dependencies (threshold: 10)
  [WARN] aether-config: largest file is 2,611 lines (threshold: 800)
  [WARN] aether-store:  largest file is 6,493 lines (threshold: 800)
```

### Example JSON output

```json
{
  "schema_version": 1,
  "run_at": 1741612921,
  "git_commit": "a1b2c3d",
  "workspace_score": 62,
  "severity": "moderate",
  "previous_score": 67,
  "delta": -5,
  "crate_count": 15,
  "total_loc": 70128,
  "crates": [
    {
      "name": "aether-store",
      "score": 78,
      "severity": "high",
      "archetypes": ["God File"],
      "total_loc": 6493,
      "file_count": 9,
      "metrics": {
        "max_file_loc": 6493,
        "trait_method_max": 52,
        "internal_dep_count": 5,
        "todo_density": 0.0,
        "dead_feature_flags": 4,
        "stale_backend_refs": 0
      },
      "violations": [
        {
          "metric": "trait_method_max",
          "value": 52,
          "threshold": 35,
          "severity": "fail",
          "reason": "Store trait has 52 methods — interfaces this large are hard to implement, test, and evolve independently"
        },
        {
          "metric": "max_file_loc",
          "value": 6493,
          "threshold": 1500,
          "severity": "fail",
          "reason": "lib.rs is 6,493 lines — large files are harder to navigate and reason about"
        }
      ]
    }
  ],
  "worst_crate": "aether-store",
  "top_violations": []
}
```

---

## Architecture

### New crate: `aether-health`

This is a library crate, not a binary. It has no dependency on `aetherd`, `aether-store`, `aether-mcp`, or any indexed-workspace infrastructure. Its only workspace dependency is `aether-config` (for threshold configuration).

```
crates/aether-health/
  Cargo.toml
  src/
    lib.rs            — public API: compute_workspace_score(), ScoreReport, CrateScore
    scanner.rs        — workspace discovery from Cargo.toml, per-crate file walker
    metrics.rs        — LOC counter, trait method counter, stale ref scanner, feature flag scanner
    scoring.rs        — penalty formula, weight application, score normalization
    archetypes.rs     — archetype assignment from signal distribution
    explanations.rs   — reason templates, violation formatting
    history.rs        — read/write health_score_history table (rusqlite, spawn_blocking)
    output.rs         — table formatter and JSON serializer
```

#### Dependencies

```toml
[dependencies]
aether-config = { path = "../aether-config" }
serde = { workspace = true }
serde_json = { workspace = true }
rusqlite = { workspace = true }
toml = { workspace = true }
walkdir = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

No `tokio` dependency. History operations use `rusqlite` directly — the CLI wraps in `spawn_blocking` if needed.

### Integration into `aetherd`

The `health-score` subcommand is added to `Commands` enum in `cli.rs`. The command handler in `main.rs` calls `aether_health::compute_workspace_score()` and renders the result.

### Workspace member registration

Add `"crates/aether-health"` to `[workspace] members` in the root `Cargo.toml`.

---

## Implementation Notes

### Crate discovery

Read `Cargo.toml` at workspace root. Parse `[workspace] members` as a glob list. For each member, read its `Cargo.toml` to get the crate name and dependency list. No `cargo metadata` — this keeps the command fast and dependency-free.

```rust
// Pseudocode
let workspace_toml = toml::from_str(&std::fs::read_to_string("Cargo.toml")?)?;
let members = workspace_toml["workspace"]["members"].as_array()?;
for member_path in members {
    let crate_toml = toml::from_str(&std::fs::read_to_string(format!("{member_path}/Cargo.toml"))?)?;
    // extract name, deps
}
```

### Trait method counting

Line-oriented scanner — no full AST, no tree-sitter:

1. Find lines matching `pub trait <Name>` (with optional `<T>` generics and `: Bounds`)
2. Track brace depth from the opening `{`
3. Count lines matching `^\s+(async\s+)?fn\s+` inside the trait body
4. Record the max across all traits in the crate

This is fast, correct enough, and has zero external dependencies.

### Stale backend reference detection

`str::contains` scan on each non-blank line. Match patterns: `"CozoGraphStore"`, `"cozo"`, `"CozoDB"`.

**Exclusions:**
- Files under `tests/` directories
- Lines inside `#[cfg(test)]` module bodies (track module depth after seeing `#[cfg(test)]`)
- Files whose name contains `_cozo` (e.g., `graph_cozo.rs`, `graph_cozo_compat.rs`) — these are the legacy-gated implementation files, expected to contain references
- Lines inside `#[cfg(feature = "legacy-cozo")]` scoped blocks

### Legacy feature flag detection

`str::contains` scan for `feature = "legacy-` in all `.rs` and `Cargo.toml` files.

### LOC counting

Non-blank, non-comment lines. A line is a comment if, after trimming, it starts with `//` or `///` or `//!`. Block comments (`/* */`) are tracked with a depth counter. Empty lines are excluded.

### Git commit detection

```rust
std::process::Command::new("git")
    .args(["rev-parse", "--short", "HEAD"])
    .current_dir(&workspace_path)
    .output()
    .ok()
    .and_then(|o| if o.status.success() { String::from_utf8(o.stdout).ok() } else { None })
    .map(|s| s.trim().to_string())
```

Fail gracefully — `None` if git is unavailable.

### Performance target

< 1 second on AETHER's workspace (~70k LOC, 15 crates). No cargo invocation, no network, no LLM calls.

---

## Configuration

Add to `aether-config/src/lib.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthScoreConfig {
    pub file_loc_warn: usize,
    pub file_loc_fail: usize,
    pub trait_method_warn: usize,
    pub trait_method_fail: usize,
    pub internal_dep_warn: usize,
    pub internal_dep_fail: usize,
    pub todo_density_warn: f32,
    pub todo_density_fail: f32,
    pub dead_feature_warn: usize,
    pub dead_feature_fail: usize,
    pub stale_ref_warn: usize,
    pub stale_ref_fail: usize,
    pub stale_ref_patterns: Vec<String>,
}

impl Default for HealthScoreConfig {
    fn default() -> Self {
        Self {
            file_loc_warn: 800,
            file_loc_fail: 1500,
            trait_method_warn: 20,
            trait_method_fail: 35,
            internal_dep_warn: 6,
            internal_dep_fail: 10,
            todo_density_warn: 5.0,
            todo_density_fail: 15.0,
            dead_feature_warn: 1,
            dead_feature_fail: 5,
            stale_ref_warn: 1,
            stale_ref_fail: 3,
            stale_ref_patterns: vec![
                "CozoGraphStore".to_string(),
                "cozo".to_string(),
                "CozoDB".to_string(),
            ],
        }
    }
}
```

Users override in `.aether/config.toml`:

```toml
[health_score]
file_loc_warn = 1000
file_loc_fail = 2000
stale_ref_patterns = ["CozoGraphStore", "cozo", "CozoDB"]
```

---

## Tests

### Unit tests (in `aether-health`)

| Test | Description |
|------|-------------|
| `score_zero_for_clean_crate` | Fabricate a minimal clean crate dir in tempdir, assert score = 0 |
| `trait_method_counter_accuracy` | Inline multi-trait Rust snippet, assert exact counts |
| `stale_ref_excludes_test_modules` | Inline code with refs inside `#[cfg(test)]`, assert not counted |
| `stale_ref_excludes_legacy_files` | File named `graph_cozo.rs`, assert excluded |
| `loc_counter_excludes_comments_blanks` | Known file content, assert exact count |
| `penalty_function_boundary_values` | Assert penalty = 0 at warn, 1.0 at fail, > 1.0 above fail, clamped at 2.0 |
| `archetype_assignment_god_file` | High LOC + trait methods → God File |
| `archetype_assignment_brittle_hub` | High dep count → Brittle Hub |
| `score_clamped_to_100` | Extreme values don't produce score > 100 |
| `workspace_score_is_loc_weighted` | Two crates: one tiny with high score, one large with low score → workspace score close to large crate |

### Integration tests (in `aether-health` or `aetherd`)

| Test | Description |
|------|-------------|
| `score_on_real_workspace` | Run against AETHER's workspace root, assert score > 0 and < 100 |
| `json_output_valid` | `--output json` produces parseable JSON with expected top-level keys |
| `fail_above_exit_code` | `--fail-above 0` exits non-zero on any real workspace |
| `history_written_and_delta` | Run twice against tempdir with `.aether/`, assert second run shows delta |
| `no_aether_dir_no_error` | Run against workspace with no `.aether/`, completes without error |
| `crate_filter_works` | `--crate aether-core` only scores that crate |

---

## Pass Criteria

1. `aetherd health-score --workspace .` completes in under 3 seconds
2. Output shows per-crate scores (0–100), archetype labels, and severity bands
3. Score for AETHER at current HEAD is computed with per-crate breakdown
4. At least one crate shows score ≥ 50 (we know `aether-store` and `aether-mcp` have issues)
5. `--output json` produces valid JSON matching the schema
6. `--fail-above 30` exits 1 on AETHER (since workspace score is > 30)
7. Running twice writes history and shows a delta on the second run
8. Running against a workspace with no `.aether/` completes without error
9. All thresholds are configurable via `[health_score]` in `.aether/config.toml`
10. `cargo fmt --check` and `cargo clippy -- -D warnings` pass on modified crates
11. `cargo test -p aether-health` and `cargo test -p aetherd` pass

---

## Codex Prompt

```
CRITICAL BUILD SETTINGS — use these for ALL cargo commands:
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR

You are working in the repo root of https://github.com/rephug/aether.

Read these files before writing any code:
- docs/roadmap/phase_8_stage_8_9_structural_health.md   (this spec — source of truth)
- crates/aetherd/src/cli.rs                              (subcommand pattern, Commands enum)
- crates/aetherd/src/main.rs                             (CLI dispatch)
- crates/aether-config/src/lib.rs                        (config structs — add HealthScoreConfig)
- crates/aether-store/src/lib.rs                         (Store trait — count its methods for reference)
- Cargo.toml                                             (workspace members list)

PREFLIGHT

1) Verify working tree is clean: git status --porcelain -b
   If dirty, STOP and report dirty files.
2) Create branch: git checkout -b feature/phase8-stage8-9-health-score
3) Create worktree: git worktree add ../aether-phase8-health-score feature/phase8-stage8-9-health-score
4) cd into the worktree.

IMPLEMENTATION

5) Create crates/aether-health/ with Cargo.toml:
   - Workspace member (edition, version, license from workspace)
   - Dependencies: aether-config (path), serde, serde_json, rusqlite, toml, walkdir
   - Dev-dependencies: tempfile

6) Register "crates/aether-health" in root Cargo.toml [workspace] members list.

7) Create crates/aether-health/src/ modules:
   a) lib.rs — public API:
      - pub fn compute_workspace_score(path: &Path, config: &HealthScoreConfig) -> Result<ScoreReport>
      - pub fn compute_crate_score(path: &Path, config: &HealthScoreConfig) -> Result<CrateScore>
      - Re-export ScoreReport, CrateScore, Violation, Archetype, Severity from models

   b) scanner.rs — workspace discovery:
      - Read Cargo.toml at workspace root for [workspace] members
      - For each member: read its Cargo.toml for crate name + dependency list
      - Walk src/ for .rs files
      - No cargo invocation — just TOML parsing and walkdir

   c) metrics.rs — metric computation:
      - count_loc(file_content: &str) -> (non_blank_non_comment, total_lines)
      - count_trait_methods(file_content: &str) -> usize (max across all pub traits)
      - count_stale_refs(file_content: &str, patterns: &[String]) -> usize
        (excluding #[cfg(test)] blocks)
      - count_feature_flags(file_content: &str, pattern: &str) -> usize
      - count_todo_density(file_content: &str) -> f32 (per 1000 non-blank lines)
      - count_internal_deps(cargo_toml: &toml::Value) -> usize

   d) scoring.rs — penalty and normalization:
      - raw_penalty(value, warn, fail) -> f64 (formula from spec)
      - compute_crate_penalty(metrics: &CrateMetrics, config: &HealthScoreConfig) -> f64
      - normalize_to_100(penalty: f64) -> u32
      - compute_workspace_aggregate(crate_scores: &[CrateScore]) -> u32
        (LOC-weighted average)

   e) archetypes.rs — archetype assignment:
      - assign_archetypes(metrics: &CrateMetrics, penalties: &MetricPenalties) -> Vec<Archetype>
      - Archetype enum: GodFile, BrittleHub, ChurnMagnet, LegacyResidue
      - Only assigned when crate score >= 25
      - Primary = highest penalty contributor, secondary if within 10%

   f) explanations.rs — reason templates:
      - fn explain_violation(metric: &str, value: f64, threshold: f64, context: &str) -> String
      - Template constants for each metric type
      - Returns human-readable single-sentence reasons

   g) history.rs — score persistence:
      - create_table_if_needed(conn: &rusqlite::Connection) -> Result<()>
      - write_score(conn: &Connection, report: &ScoreReport) -> Result<()>
      - read_previous_score(conn: &Connection) -> Result<Option<(u32, String)>>
      - All rusqlite calls — caller wraps in spawn_blocking if needed

   h) output.rs — formatters:
      - fn format_table(report: &ScoreReport) -> String
      - fn format_json(report: &ScoreReport) -> String
      - Table format matches spec example output

8) Add HealthScoreConfig to aether-config/src/lib.rs:
   - Struct with all threshold fields per spec
   - Default impl with spec values
   - Add optional [health_score] section to AetherConfig
   - Additive only — no changes to existing config fields

9) Add health-score subcommand to aetherd:
   a) In cli.rs — add HealthScoreArgs struct:
      - workspace: PathBuf (with --workspace, default ".")
      - output: OutputFormat (table/json)
      - fail_above: Option<u32>
      - no_history: bool
      - crate_filter: Vec<String> (--crate, repeatable)
   b) Add HealthScore(HealthScoreArgs) variant to Commands enum
   c) Wire dispatch in main.rs — call aether_health::compute_workspace_score(),
      handle history, render output, check fail_above exit code

10) Unit tests in crates/aether-health/src/:
    - score_zero_for_clean_crate
    - trait_method_counter_accuracy
    - stale_ref_excludes_test_modules
    - loc_counter_excludes_comments_blanks
    - penalty_function_boundary_values
    - archetype_assignment_god_file
    - archetype_assignment_brittle_hub
    - workspace_score_is_loc_weighted

11) Integration test in crates/aether-health/tests/ or aetherd:
    - Run compute_workspace_score against actual AETHER workspace root
    - Assert score > 0 and valid JSON output

SCOPE GUARD — do NOT modify:
- Any existing MCP tools or tool schemas
- Any existing dashboard routes or templates
- Any SIR pipeline code
- Any existing config fields (additive only)
- The existing Health subcommand (graph-based health analysis)
- Any Store trait methods

VALIDATION

12) Run validation:
    cargo fmt --check
    cargo clippy -p aether-health -p aetherd -p aether-config -- -D warnings
    cargo test -p aether-health
    cargo test -p aether-config
    cargo test -p aetherd

13) Run the command against the real workspace:
    cargo run -p aetherd --bin aetherd -- health-score --workspace . --output json | python3 -m json.tool

14) Verify the JSON contains at least 14 crates and workspace_score > 0.

COMMIT

15) Commit: "feat(phase8): add health-score CLI with structural metrics, archetypes, and history tracking"

OUTPUT

16) Report commit SHA.
17) Provide git push command.
```

---

## End-of-Stage Git Sequence

```bash
git push origin feature/phase8-stage8-9-health-score
gh pr create \
  --title "Phase 8.9 — Structural Health Score" \
  --body "Adds \`aether health-score\` subcommand with normalized 0-100 scoring, four diagnostic archetypes, per-crate violation explanations, score history tracking, and CI exit-code integration. New \`aether-health\` crate isolates scoring logic for future dashboard/MCP/LSP reuse."

# After merge:
git switch main
git pull --ff-only
git worktree remove ../aether-phase8-health-score
git branch -D feature/phase8-stage8-9-health-score
```

---

## Stage 8.10 Preview — Semantic Health Extension (Future)

When AETHER has indexed itself with SIR data, extend `health-score` with `--semantic`:

**Additional signals (all require indexed workspace):**

- **Drift density per crate** — ratio of symbols with active drift to total symbols
- **Stale SIR ratio** — symbols where `sir_status = 'stale'` as a percentage
- **Coupling concentration** — if any single file pair has coupling score > 0.8
- **Blast radius concentration** — any symbol with blast radius > 30% of workspace
- **Test protection gap** — high-centrality symbols with no test intent records
- **Cross-community edge ratio** — boundary leakage between graph communities

**Additional archetypes (semantic-only):**

- **Boundary Leaker** — high cross-community edge count relative to internal edges
- **Zombie File** — low recent access + high centrality + outdated SIR
- **False Stable** — low surface churn but high semantic drift magnitude

**Implementation:** Add a `SemanticSignals` struct to `aether-health` that optionally loads from `aether-store` when `--semantic` is passed. The structural score remains the baseline; semantic signals adjust the score up or down within their weight budget.

**Relationship to existing HealthAnalyzer:** The `aether-analysis::HealthAnalyzer` already computes centrality, bottlenecks, cycles, and risk hotspots via graph algorithms. Stage 8.10 should consume `HealthAnalyzer` output as one signal source, not duplicate or replace it.
