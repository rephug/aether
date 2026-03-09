# Phase 8 — Stage 8.10: Git + Semantic Health Signals

**Codename:** Deep Scan
**Depends on:** Stage 8.9 (Structural Health Score) merged
**New crates:** None
**Modified crates:** `aether-health` (add signal modules), `aether-config` (semantic thresholds)

---

## Purpose

Extend the 8.9 structural health score with two new signal categories that require workspace data:

1. **Git signals** — file churn, author count, blame age. Uses `GitContext` from `aether-core`, which already has `file_log()` and `blame_lines()` via `gix`. No new git infrastructure needed.

2. **Semantic signals** — centrality, drift magnitude, test protection gaps, boundary leakage. Consumes output from existing `HealthAnalyzer`, `CouplingAnalyzer`, and `Store` queries. No new analysis infrastructure needed.

These signals are activated by `--semantic` on the CLI. Without the flag, behavior is identical to 8.9. With the flag, the score incorporates git and indexed-workspace data for a richer picture.

This stage also adds three new archetypes that are only meaningful with semantic data.

---

## Prerequisites

- Stage 8.9 merged — `aether-health` crate exists with structural scoring
- The target workspace must have been indexed (`aetherd --index-once`) for semantic signals
- Git repository must exist for git signals (graceful fallback if not)

---

## In scope

- Git signal extraction using existing `GitContext` API
- Semantic signal extraction using existing `HealthAnalyzer`, `CouplingAnalyzer`, and `Store`
- Updated scoring formula with three signal buckets (structural + git + semantic)
- Three new archetypes: Boundary Leaker, Zombie File, False Stable
- `--semantic` flag on `health-score` CLI
- File-level scoring (aggregate per-symbol data to file level)
- Graceful degradation: missing git → skip git signals, missing index → skip semantic signals

## Out of scope

- Symbol-level scoring in the CLI output (internal computation is per-file)
- Dashboard, MCP, or LSP integration (Stage 8.11)
- Split planner / refactor recommendations (Stage 8.11)
- New git infrastructure or analysis modules
- AST complexity metrics

---

## Signal Model

### Signal buckets and weights

When `--semantic` is active:

```
file_score =
    0.40 * structural_pressure    (from 8.9, unchanged)
  + 0.25 * git_pressure           (new)
  + 0.35 * semantic_pressure       (new)
```

When `--semantic` is not active (or data is unavailable), the structural score from 8.9 is used as-is. No reweighting — the 8.9 score is the 8.9 score.

Workspace score remains LOC-weighted average of per-crate scores.

### Git signals (0.25 weight budget)

All git signals are computed per-file via existing `GitContext` methods, then aggregated to per-crate by taking the max or mean as noted.

| Signal | Source | Normalization | Crate aggregation |
|--------|--------|---------------|-------------------|
| `churn_30d` | Count of commits touching file in last 30 days via `GitContext::file_log()` | 0 commits = 0.0, ≥ 15 commits = 1.0, linear between | Max across files |
| `churn_90d` | Count of commits in last 90 days | 0 = 0.0, ≥ 30 = 1.0, linear | Max across files |
| `author_count` | Unique authors from `GitContext::blame_lines()` | 1 author = 0.0, ≥ 6 authors = 1.0, linear | Max across files |
| `blame_age_spread` | Std deviation of commit timestamps from blame lines | Low spread (recent, uniform) = 0.0, high spread = 1.0 | Mean across files |

Git pressure for a crate:

```
git_pressure =
    0.35 * churn_30d
  + 0.25 * churn_90d
  + 0.25 * author_count
  + 0.15 * blame_age_spread
```

**Fallback:** If `GitContext::open()` returns `None` (no git repo), all git signals are 0.0 and the git bucket contributes nothing. A note is added to the report: `"Git data unavailable — git signals skipped"`.

### Semantic signals (0.35 weight budget)

Semantic signals require an indexed workspace (`.aether/meta.sqlite` + SurrealDB). They are computed by querying existing data through the `Store` trait and `HealthAnalyzer`.

| Signal | Source | Normalization | Crate aggregation |
|--------|--------|---------------|-------------------|
| `max_centrality` | `SymbolHealthEntry.pagerank` from `HealthAnalyzer.analyze()` | Top pagerank value in crate, normalized against workspace max | Direct (already per-crate) |
| `drift_density` | `Store::list_drift_results()` — ratio of symbols with `magnitude > 0.3` to total symbols in crate | 0% = 0.0, ≥ 30% = 1.0 | Direct |
| `stale_sir_ratio` | `Store::get_sir_meta()` — ratio of symbols where `sir_status = "stale"` or SIR is missing | 0% = 0.0, ≥ 40% = 1.0 | Direct |
| `test_gap` | `Store::list_test_intents_for_file()` — ratio of files with no test intent records among top-centrality files (top 20%) | 0% = 0.0, 100% = 1.0 | Direct |
| `boundary_leakage` | `Store::list_latest_community_snapshot()` — for each file, count symbols belonging to different communities. Ratio of multi-community files to total files | 0% = 0.0, ≥ 50% = 1.0 | Direct |

Semantic pressure for a crate:

```
semantic_pressure =
    0.25 * max_centrality
  + 0.20 * drift_density
  + 0.15 * stale_sir_ratio
  + 0.20 * test_gap
  + 0.20 * boundary_leakage
```

**Fallback:** If `.aether/meta.sqlite` does not exist or the Store cannot be opened, all semantic signals are 0.0 and a note is added: `"Indexed workspace not found — semantic signals skipped. Run aetherd --index-once first."`. The `--semantic` flag does not error on missing data — it degrades gracefully.

---

## New Archetypes

These three archetypes are only assigned when `--semantic` is active and the corresponding signals are available. They supplement the four structural archetypes from 8.9.

| Archetype | Trigger | What it means |
|-----------|---------|---------------|
| **Boundary Leaker** | `boundary_leakage` is above 0.6 AND is the top semantic contributor | Files in this crate mix symbols from multiple graph communities — architectural boundaries are being violated |
| **Zombie File** | `churn_30d` is below 0.1 (very few recent changes) AND `max_centrality` is above 0.6 (still structurally important) | Structurally central but nobody's touching it — likely under-maintained relative to its importance |
| **False Stable** | `drift_density` is above 0.5 AND `churn_30d` is below 0.2 | Meaning is shifting even though the file isn't being edited much — hidden semantic drift |

Assignment rules:
- Semantic archetypes can coexist with structural archetypes (e.g., a crate can be both "God File" and "Boundary Leaker")
- Maximum two archetype labels per crate total (structural + semantic)
- If more than two would apply, keep the two with the highest contributing signal values

---

## Updated Data Model

The `ScoreReport` from 8.9 is extended, not replaced:

```rust
pub struct CrateScore {
    // Existing from 8.9
    pub name: String,
    pub score: u32,
    pub severity: Severity,
    pub archetypes: Vec<Archetype>,
    pub metrics: StructuralMetrics,
    pub violations: Vec<Violation>,

    // New in 8.10
    pub git_signals: Option<GitSignals>,
    pub semantic_signals: Option<SemanticSignals>,
    pub signal_availability: SignalAvailability,
}

pub struct GitSignals {
    pub churn_30d: f64,        // normalized 0-1
    pub churn_90d: f64,
    pub author_count: f64,
    pub blame_age_spread: f64,
    pub git_pressure: f64,     // weighted combination
}

pub struct SemanticSignals {
    pub max_centrality: f64,
    pub drift_density: f64,
    pub stale_sir_ratio: f64,
    pub test_gap: f64,
    pub boundary_leakage: f64,
    pub semantic_pressure: f64,  // weighted combination
}

pub struct SignalAvailability {
    pub git_available: bool,
    pub semantic_available: bool,
    pub notes: Vec<String>,       // explanations for missing data
}
```

JSON output includes these new fields when present; omits them (or sets to `null`) when `--semantic` is not used.

---

## CLI Changes

Add `--semantic` flag to the existing `health-score` subcommand:

```
aetherd health-score [OPTIONS]

Options:
  --workspace <path>      Workspace root (default: .)
  --output <table|json>   Output format (default: table)
  --fail-above <N>        Exit code 1 if workspace score > N
  --no-history            Skip reading/writing score history
  --crate <n>          Scope to specific crate(s) (repeatable)
  --semantic              Enable git + semantic signals (requires indexed workspace)
```

### Updated table output (with --semantic)

```
AETHER Health Score — /home/rephu/projects/aether
Run: 2026-03-15 14:22  |  Git: a1b2c3d  |  Score: 58/100 (Moderate)  |  Delta: -4 ↓
Mode: structural + git + semantic

Crate                  Score  Struct  Git  Semantic  Archetype
──────────────────────────────────────────────────────────────────
aether-store             74    78     42      81     God File, Boundary Leaker
aether-mcp               68    71     55      64     Brittle Hub, Legacy Residue
aetherd                  52    54     61      38     Brittle Hub
aether-config            41    44     28      45     God File
aether-analysis          35    38     52      19     Churn Magnet
aether-infer             24    22     31      18     —
aether-dashboard         15    12      8      28     —
...

Top issues:
  [FAIL] aether-store:    Store trait has 52 methods (threshold: 35)
  [FAIL] aether-store:    High boundary leakage — symbols span 4 communities
  [WARN] aether-analysis: 52 commits in 30 days — high churn concentration
  [WARN] aetherd:         6 authors touching indexer.rs — coordination hotspot
```

The Struct/Git/Semantic sub-scores are the raw bucket values (0–100 scale) before weighting. They help users understand which signal category is driving the score.

---

## Implementation Notes

### Git signal computation

New module: `aether-health/src/git_signals.rs`

```rust
use aether_core::git::GitContext;

pub struct FileGitStats {
    pub commits_30d: usize,
    pub commits_90d: usize,
    pub author_count: usize,
    pub blame_age_std_dev: f64,
}

pub fn compute_file_git_stats(git: &GitContext, file_path: &Path) -> FileGitStats {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let cutoff_30d = now - 30 * 86400;
    let cutoff_90d = now - 90 * 86400;

    let commits = git.file_log(file_path, 500);
    let commits_30d = commits.iter().filter(|c| c.timestamp >= cutoff_30d).count();
    let commits_90d = commits.iter().filter(|c| c.timestamp >= cutoff_90d).count();

    let blame = git.blame_lines(file_path);
    let authors: HashSet<&str> = blame.iter().map(|l| l.author.as_str()).collect();

    // blame_age_std_dev: standard deviation of commit timestamps from blame
    // (implementation detail — straightforward stats)

    FileGitStats {
        commits_30d,
        commits_90d,
        author_count: authors.len(),
        blame_age_std_dev: compute_std_dev(&blame, git),
    }
}
```

Performance note: `blame_lines()` uses `gix::blame_file()` which can be slow on large files with deep history. Cap at the top 20 files per crate by structural score. If a crate has > 20 `.rs` files, only compute git signals for the 20 largest by LOC.

### Semantic signal computation

New module: `aether-health/src/semantic_signals.rs`

This module takes a `&dyn Store` reference and an optional `HealthReport` (from `HealthAnalyzer`). It does NOT instantiate `HealthAnalyzer` itself — the caller (the `health-score` command handler in `aetherd`) is responsible for creating and running `HealthAnalyzer` and passing the result in.

This avoids `aether-health` depending directly on `aether-analysis` or `aether-store`. Instead:

```rust
// In aether-health/src/semantic_signals.rs
pub struct SemanticInput {
    pub symbol_health: Vec<SymbolHealthSummary>,  // simplified view
    pub drift_results: Vec<DriftSummary>,
    pub community_assignments: Vec<CommunitySummary>,
    pub test_intent_coverage: HashMap<String, bool>,  // file_path -> has_tests
}

pub fn compute_semantic_signals(input: &SemanticInput) -> SemanticSignals {
    // Pure computation, no Store dependency
}
```

The `aetherd` command handler bridges the gap:

```rust
// In aetherd health-score command handler
let health_report = HealthAnalyzer::new(&workspace)?.analyze(&request).await?;
let drift_results = store.list_drift_results(/* ... */)?;
let communities = store.list_latest_community_snapshot()?;
// ... build SemanticInput from these ...
let semantic = compute_semantic_signals(&input);
```

This keeps `aether-health` dependency-light (no `aether-store`, no `aether-analysis`) while still consuming their data.

### Dependency architecture

```
aether-health
  depends on: aether-config, aether-core (for GitContext)
  does NOT depend on: aether-store, aether-analysis, aether-mcp

aetherd
  depends on: aether-health, aether-store, aether-analysis (existing)
  bridges semantic data from Store/HealthAnalyzer into aether-health's SemanticInput
```

---

## Configuration

Extend `HealthScoreConfig` in `aether-config`:

```rust
// Added to existing HealthScoreConfig from 8.9
pub struct HealthScoreConfig {
    // ... existing structural thresholds ...

    // Git signal thresholds
    pub churn_30d_high: usize,        // default: 15
    pub churn_90d_high: usize,        // default: 30
    pub author_count_high: usize,     // default: 6

    // Semantic signal thresholds
    pub drift_density_high: f32,      // default: 0.30
    pub stale_sir_high: f32,          // default: 0.40
    pub test_gap_high: f32,           // default: 0.50 (ratio)
    pub boundary_leakage_high: f32,   // default: 0.50

    // Weight overrides (optional)
    pub structural_weight: Option<f32>,  // default: 0.40
    pub git_weight: Option<f32>,         // default: 0.25
    pub semantic_weight: Option<f32>,    // default: 0.35
}
```

Users can tune weights in `.aether/config.toml`:

```toml
[health_score]
structural_weight = 0.50
git_weight = 0.20
semantic_weight = 0.30
churn_30d_high = 20
```

---

## Tests

### Unit tests (in `aether-health`)

| Test | Description |
|------|-------------|
| `git_signals_normalize_churn` | 0 commits → 0.0, 15 commits → 1.0, 7 commits → ~0.47 |
| `git_signals_normalize_authors` | 1 author → 0.0, 6 → 1.0, 3 → 0.5 |
| `semantic_signals_drift_density` | Known drift records → correct ratio |
| `semantic_signals_test_gap` | Mix of covered/uncovered files → correct ratio |
| `semantic_signals_boundary_leakage` | Known community assignments → correct multi-community ratio |
| `combined_score_with_all_signals` | Structural + git + semantic → weighted correctly |
| `combined_score_missing_git` | Git unavailable → score equals structural-only (no reweight) |
| `combined_score_missing_semantic` | Semantic unavailable → score uses structural + git only |
| `archetype_boundary_leaker` | High boundary leakage → Boundary Leaker assigned |
| `archetype_zombie_file` | Low churn + high centrality → Zombie File assigned |
| `archetype_false_stable` | High drift + low churn → False Stable assigned |
| `max_two_archetypes` | Extreme values trigger many → only top 2 assigned |

### Integration tests

| Test | Description |
|------|-------------|
| `semantic_score_on_real_workspace` | Run with `--semantic` against AETHER (if indexed), assert signals populated |
| `semantic_flag_without_index` | Run `--semantic` on non-indexed workspace → graceful fallback, notes explain |
| `git_signals_on_real_repo` | Run against AETHER → git signals non-zero for active crates |
| `json_output_includes_signals` | `--semantic --output json` includes `git_signals` and `semantic_signals` fields |

---

## Pass Criteria

1. `aetherd health-score --workspace . --semantic` completes in under 10 seconds
2. Without `--semantic`, behavior is identical to 8.9 (no regression)
3. With `--semantic`, output shows Struct/Git/Semantic sub-columns
4. Git signals are non-zero for crates with recent activity
5. Semantic signals are non-zero when workspace is indexed (or gracefully skipped if not)
6. New archetypes appear where appropriate
7. JSON output includes `git_signals`, `semantic_signals`, and `signal_availability`
8. `--fail-above` works with semantic-adjusted scores
9. Score history records semantic scores alongside structural
10. `cargo fmt --check` and `cargo clippy -- -D warnings` pass
11. All unit and integration tests pass

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
- docs/roadmap/phase_8_stage_8_10_semantic_health.md    (this spec — source of truth)
- crates/aether-health/src/lib.rs                        (existing structural scoring from 8.9)
- crates/aether-health/src/scoring.rs                    (penalty formula, normalization)
- crates/aether-health/src/archetypes.rs                 (existing archetype assignment)
- crates/aether-core/src/git.rs                          (GitContext — file_log, blame_lines)
- crates/aether-analysis/src/health.rs                   (HealthAnalyzer, SymbolHealthEntry)
- crates/aether-analysis/src/coupling.rs                 (BlastRadiusEntry)
- crates/aether-store/src/lib.rs                         (Store trait — drift, community, test intent queries)
- crates/aetherd/src/cli.rs                              (HealthScoreArgs — add --semantic flag)
- crates/aether-config/src/lib.rs                        (HealthScoreConfig — extend thresholds)

PREFLIGHT

1) Verify working tree is clean. If dirty, STOP.
2) Create branch: git checkout -b feature/phase8-stage8-10-semantic-health
3) Create worktree: git worktree add ../aether-phase8-semantic-health feature/phase8-stage8-10-semantic-health
4) cd into the worktree.

IMPLEMENTATION

5) Add aether-core dependency to aether-health/Cargo.toml (for GitContext).

6) Create crates/aether-health/src/git_signals.rs:
   - FileGitStats struct: commits_30d, commits_90d, author_count, blame_age_std_dev
   - compute_file_git_stats(git: &GitContext, file_path: &Path) -> FileGitStats
   - normalize_git_signals(stats: &FileGitStats, config: &HealthScoreConfig) -> GitSignals
   - aggregate_crate_git_signals(file_stats: &[GitSignals]) -> GitSignals
   - Performance: cap git analysis at top 20 files per crate by LOC

7) Create crates/aether-health/src/semantic_signals.rs:
   - SemanticInput struct (simplified views of Store data — no Store dependency)
   - compute_semantic_signals(input: &SemanticInput) -> SemanticSignals
   - normalize each signal to 0-1 per spec thresholds

8) Update crates/aether-health/src/scoring.rs:
   - Add combined_score() that takes structural + optional git + optional semantic
   - Weight: 0.40 structural + 0.25 git + 0.35 semantic (when all present)
   - Fallback: if git missing, structural is full score; if semantic missing, structural + git reweighted
   - Keep existing compute_crate_penalty() unchanged for backward compat

9) Update crates/aether-health/src/archetypes.rs:
   - Add Archetype variants: BoundaryLeaker, ZombieFile, FalseStable
   - Add assign_semantic_archetypes() that checks git + semantic signals
   - Update combined assignment: max 2 archetypes total

10) Update crates/aether-health/src/explanations.rs:
    - Add reason templates for new signals and archetypes
    - "High boundary leakage — symbols in {file} span {n} communities"
    - "Structurally central but only {n} commits in 30 days — potential zombie"
    - "{n}% of symbols show semantic drift with minimal file churn — hidden meaning shift"

11) Update CrateScore, ScoreReport in lib.rs:
    - Add git_signals: Option<GitSignals>
    - Add semantic_signals: Option<SemanticSignals>
    - Add signal_availability: SignalAvailability
    - Update JSON serialization

12) Update output.rs:
    - Table format: add Struct/Git/Semantic sub-columns when --semantic
    - JSON format: include new signal fields

13) Extend HealthScoreConfig in aether-config/src/lib.rs:
    - Add git thresholds, semantic thresholds, optional weight overrides
    - Backward compatible defaults

14) Update aetherd CLI:
    a) Add --semantic bool flag to HealthScoreArgs in cli.rs
    b) In command handler: if --semantic, open GitContext, run HealthAnalyzer,
       query Store for drift/community/test data, build SemanticInput, pass to
       aether_health scoring functions

15) Tests per spec — all unit tests in aether-health, integration tests in aetherd.

SCOPE GUARD — do NOT modify:
- Existing structural scoring behavior (8.9 scores must not change)
- HealthAnalyzer internals
- CouplingAnalyzer internals
- Store trait or any Store implementations
- Existing CLI commands other than health-score
- Dashboard, MCP, or LSP code

VALIDATION

16) Run:
    cargo fmt --check
    cargo clippy -p aether-health -p aetherd -p aether-config -- -D warnings
    cargo test -p aether-health
    cargo test -p aether-config
    cargo test -p aetherd

17) Run structural-only (regression check):
    cargo run -p aetherd --bin aetherd -- health-score --workspace . --output json | python3 -m json.tool

18) Run with semantic flag:
    cargo run -p aetherd --bin aetherd -- health-score --workspace . --semantic --output json | python3 -m json.tool

COMMIT

19) Commit: "feat(phase8): add git + semantic signals to health-score with --semantic flag"
```

---

## End-of-Stage Git Sequence

```bash
git push origin feature/phase8-stage8-10-semantic-health
gh pr create \
  --title "Phase 8.10 — Git + Semantic Health Signals" \
  --body "Extends health-score with git churn/author signals and semantic signals (centrality, drift, test gaps, boundary leakage). Adds three new archetypes: Boundary Leaker, Zombie File, False Stable. Activated via --semantic flag with graceful degradation."

# After merge:
git switch main
git pull --ff-only
git worktree remove ../aether-phase8-semantic-health
git branch -D feature/phase8-stage8-10-semantic-health
```
