# Phase 8 — Stage 8.7: Stress Test Harness (The Proving Ground)

**Prerequisites:** Stage 8.1 + 8.2 merged (state reconciliation + progressive indexing)
**Estimated Codex Runs:** 1 (scripting + test infrastructure, ~300 lines)
**Risk Level:** Low — creates new test infrastructure, does not modify production code

---

## Purpose

Before AETHER can claim it works on enterprise-scale monorepos, it must prove it. This stage builds a reproducible benchmark suite that:

1. Clones real-world open-source repositories of varying sizes
2. Runs the full AETHER pipeline end-to-end (Pass 1 + partial Pass 2)
3. Captures metrics: indexing time, peak memory, SIR success rate, query latency, fsck results
4. Produces a formatted "AETHER Scale Report" — both for internal quality gating and external marketing

This is the **validation gate** for Stages 8.1 and 8.2. If progressive indexing doesn't work on Bevy (45K+ symbols) or state reconciliation fails after a simulated crash on a large repo, the stress test catches it before users do.

---

## Design

### Benchmark Repos

The harness tests against a curated set of real-world repos covering different languages, sizes, and structures:

| Tier | Repo | Language | Approx Symbols | Purpose |
|------|------|----------|-----------------|---------|
| Small | `BurntSushi/ripgrep` | Rust | ~2,000 | Baseline, fast iteration |
| Medium | `astral-sh/ruff` | Rust + Python | ~10,000 | Multi-language, growing codebase |
| Large | `bevyengine/bevy` | Rust | ~45,000 | Massive workspace, many crates |
| TypeScript | `microsoft/TypeScript` | TypeScript | ~30,000 | Large TS project, deep type hierarchies |
| Python | `pallets/flask` | Python | ~3,000 | Python ecosystem standard |

The harness clones these at a pinned commit SHA (reproducibility) and stores them in a temporary directory.

### Benchmark Phases

Each repo goes through:

**Phase A — Pass 1 Timing (AST + Graph)**
- Run `aether index --once --pass1-only` (or equivalent from 8.2)
- Capture: wall clock time, peak RSS memory, symbol count, edge count
- Verify: fsck reports zero inconsistencies after Pass 1

**Phase B — Pass 2 Partial Timing (SIR Generation)**
- Run Pass 2 on the top 100 priority symbols only (using Mock provider for speed, or real provider if configured)
- Capture: SIR generation time per symbol (p50, p95, p99), success rate, queue drain rate
- Verify: fsck reports zero inconsistencies after partial Pass 2

**Phase C — Query Latency**
- After indexing, run a battery of queries:
  - Lexical search: 10 queries, measure response time
  - Graph query (call chain): 5 queries on high-centrality symbols
  - MCP tool call (aether_get_sir): 5 calls for indexed symbols, 5 calls for non-indexed (on-demand bump)
- Capture: p50, p95, p99 latency for each query type

**Phase D — Simulated Crash Recovery**
- Start indexing Pass 2 on the medium repo
- After 50 SIR writes, send SIGKILL to aetherd
- Restart aetherd
- Verify: intent replay recovers incomplete writes
- Run fsck: verify zero inconsistencies after recovery
- Capture: recovery time, replayed intents count

**Phase E — Concurrent Load (Optional)**
- Start aether-query against the indexed repo
- Fire 20 concurrent MCP requests
- Verify: all return successfully, no panics, no stale data
- Capture: p50, p95 response time under load

### Report Format

The harness produces a markdown report:

```markdown
# AETHER Scale Report
Generated: 2026-03-15T14:30:00Z
AETHER Version: 0.8.2 (commit abc123)
System: WSL2 Ubuntu 24.04, 12GB RAM, AMD Ryzen 7 5800X

## Summary
| Repo | Symbols | Pass 1 Time | Peak Memory | SIR Rate (Mock) | fsck |
|------|---------|-------------|-------------|-----------------|------|
| ripgrep | 2,147 | 3.2s | 142MB | 100% | ✅ |
| ruff | 10,438 | 18.7s | 287MB | 99.8% | ✅ |
| bevy | 47,291 | 2m14s | 891MB | 99.1% | ✅ |
| TypeScript | 31,055 | 1m42s | 623MB | 98.7% | ✅ |
| flask | 3,281 | 4.1s | 156MB | 100% | ✅ |

## Pass 1 Detail (AST + Graph)
...

## Pass 2 Detail (SIR Generation — Top 100 Symbols)
...

## Query Latency
| Query Type | p50 | p95 | p99 |
|------------|-----|-----|-----|
| Lexical Search | 12ms | 34ms | 89ms |
| Call Chain | 28ms | 95ms | 210ms |
| MCP get_sir (cached) | 8ms | 19ms | 42ms |
| MCP get_sir (on-demand) | 2.1s | 4.8s | 8.2s |

## Crash Recovery
- Simulated crash after 50 SIR writes on ruff
- Recovery time: 1.3s
- Replayed intents: 3
- Post-recovery fsck: ✅ clean

## Concurrent Load (aether-query)
- 20 concurrent MCP requests
- p50: 45ms, p95: 180ms, p99: 420ms
- Failures: 0
```

### Implementation

The harness is a shell script + a small Rust binary for structured metric collection:

```
tests/stress/
├── run_benchmark.sh         # Orchestrator: clone repos, run phases, generate report
├── repos.toml               # Repo URLs, pinned commits, expected symbol ranges
├── benchmark_runner.rs       # Rust helper: invoke aether CLI, capture metrics, format report
└── reports/                  # Output directory for generated reports (gitignored)
```

**`run_benchmark.sh`** is the entry point. It:
1. Reads `repos.toml` for repo list
2. Clones each repo to `/tmp/aether-bench-{name}/` (or `$BENCH_DIR`)
3. Builds aetherd with `cargo build -p aetherd --release`
4. For each repo, runs the benchmark phases
5. Collects metrics into a JSON intermediate format
6. Calls `benchmark_runner` to generate the markdown report

**`repos.toml`** format:
```toml
[[repos]]
name = "ripgrep"
url = "https://github.com/BurntSushi/ripgrep"
commit = "abc123..."  # pinned for reproducibility
language = "rust"
expected_symbols_min = 1500
expected_symbols_max = 3000
tier = "small"

[[repos]]
name = "bevy"
url = "https://github.com/bevyengine/bevy"
commit = "def456..."
language = "rust"
expected_symbols_min = 40000
expected_symbols_max = 60000
tier = "large"
```

**Memory measurement:** Use `/proc/self/status` VmPeak (Linux) or `/usr/bin/time -v` wrapper.

**Timing:** `std::time::Instant` for Rust-level measurements, `time` command for shell-level.

### CI Integration (Future)

The stress test is designed to run in CI as a nightly job. For now, it runs manually:
```bash
cd /home/rephu/projects/aether
./tests/stress/run_benchmark.sh --tier small  # quick: ripgrep + flask only
./tests/stress/run_benchmark.sh --tier all    # full: all repos (takes 30+ minutes)
```

The `--tier` flag allows running subsets for fast feedback.

---

## Files Created/Modified

| File | Action | Description |
|------|--------|-------------|
| `tests/stress/run_benchmark.sh` | **Create** | Main orchestrator script |
| `tests/stress/repos.toml` | **Create** | Benchmark repo definitions |
| `tests/stress/README.md` | **Create** | How to run benchmarks, interpret results |
| `tests/stress/benchmark_helpers.sh` | **Create** | Shell functions for metric capture |
| `.gitignore` | **Modify** | Add `tests/stress/reports/` and cloned repo dirs |

**Note:** This stage does NOT create a separate Rust binary for the benchmark runner. The shell script invokes `aether` CLI commands directly and captures output. A Rust-based runner can be added later if the shell approach proves insufficient.

---

## Edge Cases

| Scenario | Behavior |
|----------|----------|
| Network unavailable (can't clone repos) | Script checks for existing clones, skips download if present |
| Repo at pinned commit no longer exists | Script reports error, skips repo, continues with others |
| OOM during large repo indexing | Script captures exit code, reports "OOM" in results, continues |
| SurrealDB or LanceDB fails to open | Script captures error output, reports in "Errors" section |
| aether binary not built | Script runs `cargo build -p aetherd --release` first |
| Benchmark interrupted (Ctrl+C) | Partial results written; re-run resumes from last incomplete repo |
| Mock provider vs real provider | Default: Mock (fast). `--provider gemini` flag for real inference timing |
| WSL2 memory constraints (12GB) | Large repos may OOM; script tracks and reports peak memory |

---

## Pass Criteria

1. `run_benchmark.sh --tier small` completes successfully on ripgrep + flask.
2. Pass 1 timing is captured for all tested repos.
3. fsck runs after each indexing phase and results are captured.
4. Query latency is measured for lexical search and graph queries.
5. Simulated crash recovery works (SIGKILL + restart + verify).
6. Markdown report is generated in `tests/stress/reports/`.
7. Script handles errors gracefully (OOM, network failure, missing repos).
8. No production code is modified in this stage.

---

## Codex Prompt

```text
==========BEGIN CODEX PROMPT==========

CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- export CARGO_TARGET_DIR=/home/rephu/aether-target
- export CARGO_BUILD_JOBS=2
- export PROTOC=$(which protoc)
- export RUSTC_WRAPPER=sccache
- export TMPDIR=/home/rephu/aether-target/tmp
- mkdir -p $TMPDIR
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_8_stage_8_7_stress_test_harness.md for the full specification.
Read docs/roadmap/phase8_session_context.md for current architecture context.

PREFLIGHT

1) Ensure working tree is clean (`git status --porcelain`). If not, stop and report.
2) `git pull --ff-only` — ensure main is up to date.

BRANCH + WORKTREE

3) Create branch feature/phase8-stage8-7-stress-test off main.
4) Create worktree ../aether-phase8-stage8-7 for that branch and switch into it.
5) Set build environment (copy the exports from the top of this prompt).

NOTE ON AETHER CLI:
- `aetherd --workspace <path>` starts the daemon with file watcher
- `aetherd --workspace <path> --index` does initial index + starts watching
- The exact CLI flags may differ — check `crates/aetherd/src/cli.rs` for current subcommands
- `aether fsck` was added in Stage 8.1 for state verification
- `aether status` shows daemon state including SIR coverage (from Stage 8.2)

NOTE ON INFERENCE PROVIDERS:
- Default: Mock provider (returns placeholder SIR, fast, no API key needed)
- The benchmark should default to Mock for speed/reproducibility
- Optional: `--provider gemini` for real inference timing (requires GEMINI_API_KEY)

NOTE ON MEMORY MEASUREMENT:
- On Linux (WSL2): parse /proc/<pid>/status for VmPeak and VmRSS
- Alternative: wrap command with `/usr/bin/time -v` and parse "Maximum resident set size"
- Both approaches are valid; use whichever is more reliable

=== STEP 1: Create Directory Structure ===

6) Create:
   - `tests/stress/run_benchmark.sh` (executable)
   - `tests/stress/benchmark_helpers.sh` (sourced by run_benchmark.sh)
   - `tests/stress/repos.toml`
   - `tests/stress/README.md`
   - `tests/stress/reports/.gitkeep`

7) Add to `.gitignore`:
   ```
   # Stress test artifacts
   tests/stress/reports/*.md
   tests/stress/repos/
   ```

=== STEP 2: Define Benchmark Repos ===

8) In `tests/stress/repos.toml`, define repos. For the pinned commit SHAs,
   use "HEAD" as a placeholder — the script will resolve to the current HEAD
   at clone time and record the actual SHA in the report. Repos:

   - ripgrep (BurntSushi/ripgrep) — small Rust, tier=small
   - flask (pallets/flask) — small Python, tier=small
   - ruff (astral-sh/ruff) — medium Rust+Python, tier=medium
   - bevy (bevyengine/bevy) — large Rust workspace, tier=large
   - TypeScript (microsoft/TypeScript) — large TypeScript, tier=large

=== STEP 3: Create Benchmark Helpers ===

9) In `tests/stress/benchmark_helpers.sh`, create shell functions:

   - `clone_repo(name, url, commit, dest_dir)` — git clone + checkout, skip if exists
   - `build_aether()` — cargo build -p aetherd --release (check if binary exists first)
   - `measure_command(label, command...)` — run command, capture wall time + exit code
   - `get_peak_memory(pid)` — read VmPeak from /proc/<pid>/status (poll during execution)
   - `run_aether_index(workspace, provider, extra_args)` — run aetherd index in background,
     return PID for memory monitoring
   - `run_aether_fsck(workspace)` — run fsck, capture output, parse pass/fail
   - `run_query_benchmark(workspace, query_type, queries_file)` — execute queries via
     aether CLI or direct HTTP to aether-query, capture latencies
   - `generate_json_result(metrics...)` — output structured JSON for report generation
   - `kill_and_recover(pid, workspace)` — SIGKILL, wait, restart, measure recovery

=== STEP 4: Create Main Orchestrator ===

10) In `tests/stress/run_benchmark.sh`:

    Parse arguments:
    - `--tier small|medium|large|all` (default: small)
    - `--provider mock|gemini|ollama` (default: mock)
    - `--bench-dir <path>` (default: /tmp/aether-bench)
    - `--report-dir <path>` (default: tests/stress/reports)
    - `--skip-clone` (use existing clones)

    Flow:
    a. Source benchmark_helpers.sh
    b. Parse repos.toml (use simple grep/awk — no TOML parser dependency)
    c. Build aether if needed
    d. For each repo matching the selected tier:
       1. Clone repo (or skip if --skip-clone and exists)
       2. Phase A: Run Pass 1 index, capture time + memory + symbol count
       3. Run fsck, capture result
       4. Phase B: Run Pass 2 (top 100 symbols with mock provider), capture SIR metrics
       5. Run fsck again
       6. Phase C: Run query latency tests (lexical search, call chain)
       7. Phase D: (medium tier only) Simulated crash recovery
    e. Collect all results into a JSON array
    f. Generate markdown report from JSON (use heredoc + sed/awk formatting)
    g. Print report path

    The script should be robust:
    - `set -euo pipefail` at top
    - Trap SIGINT/SIGTERM to clean up background processes
    - Log progress to stderr, results to files
    - Continue on individual repo failure (capture error, move to next)

=== STEP 5: Create README ===

11) In `tests/stress/README.md`, document:
    - Purpose of the stress test suite
    - Prerequisites (git, cargo, enough disk space)
    - How to run: quick (--tier small), full (--tier all)
    - How to interpret the report
    - How to add new benchmark repos
    - Memory requirements per tier
    - Known limitations

=== STEP 6: Validation ===

12) Run:
    - `shellcheck tests/stress/run_benchmark.sh` (if shellcheck is available)
    - `shellcheck tests/stress/benchmark_helpers.sh` (if shellcheck is available)
    - Verify the script parses repos.toml correctly: `bash tests/stress/run_benchmark.sh --help`
    - Do a dry run on a tiny repo if time permits

13) Run standard validation (this stage doesn't modify Rust code, but verify nothing broke):
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings

14) Commit with message:
    "Phase 8.7: Add stress test harness — benchmark suite for scale validation"

SCOPE GUARD:
- Do NOT modify any Rust source files (this is test infrastructure only)
- Do NOT add Rust dependencies
- Do NOT create a separate Rust binary for the benchmark runner
- Do NOT require network access for the benchmark to parse/report (only for cloning repos)
- Do NOT hardcode absolute paths — use $CARGO_TARGET_DIR and relative paths
- Keep the shell scripts POSIX-compatible where possible (bash is OK for arrays)
- If shellcheck is not installed, skip linting and note it
- The benchmark does NOT need to run to completion during this Codex session —
  creating the infrastructure is the deliverable

OUTPUT

15) Report:
    - Files created
    - Whether shellcheck passed (or was unavailable)
    - Any issues with the TOML parsing approach
    - Commit SHA

16) Provide push + PR commands:
    ```
    git -C ../aether-phase8-stage8-7 push -u origin feature/phase8-stage8-7-stress-test
    gh pr create --title "Phase 8.7: Stress Test Harness" --body "..." --base main
    ```

==========END CODEX PROMPT==========
```

## Post-Merge Sequence

```bash
cd /home/rephu/projects/aether
git switch main
git pull --ff-only origin main
git log --oneline -3

git worktree remove ../aether-phase8-stage8-7
git branch -d feature/phase8-stage8-7-stress-test
git worktree prune

git status --porcelain
```
