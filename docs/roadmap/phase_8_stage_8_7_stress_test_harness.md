# Phase 8 — Stage 8.7: Stress Test Harness (The Proving Ground)

**Prerequisites:** Stage 8.1 + 8.2 merged (state reconciliation + progressive indexing + tiered providers)
**Estimated Codex Runs:** 1 (scripting + test infrastructure, ~300 lines)
**Risk Level:** Low — creates new test infrastructure, does not modify production code

---

## Purpose

Before AETHER can claim it works on enterprise-scale monorepos, it must prove it. This stage builds a reproducible benchmark suite that:

1. Clones real-world open-source repositories of varying sizes
2. Runs the full AETHER pipeline end-to-end (Pass 1 + Pass 2 with real inference)
3. Captures metrics: indexing time, peak memory, SIR success rate, query latency, fsck results
4. Produces a formatted "AETHER Scale Report" — both for internal quality gating and external marketing

This is the **validation gate** for Stages 8.1 and 8.2. If progressive indexing doesn't work on Bevy (45K+ symbols) or state reconciliation fails after a simulated crash on a large repo, the stress test catches it before users do.

---

## Design

### Benchmark Repos

The harness tests against a curated set of real-world repos covering different languages, sizes, and structures:

| Tier | Repo | Language | Approx Symbols | Clone Size | Purpose |
|------|------|----------|-----------------|------------|---------|
| Small | `BurntSushi/ripgrep` | Rust | ~2,000 | ~50MB | Baseline, fast iteration |
| Small | `pallets/flask` | Python | ~3,000 | ~30MB | Python ecosystem standard |
| Medium | `astral-sh/ruff` | Rust + Python | ~10,000 | ~200MB | Multi-language, growing codebase |
| Large | `bevyengine/bevy` | Rust | ~45,000 | ~2GB | Massive workspace, many crates |
| Large | `microsoft/TypeScript` | TypeScript | ~30,000 | ~1.5GB | Large TS project, deep type hierarchies |

**Disk space estimates (repos + AETHER index data):**
- `--tier small`: ~200MB (ripgrep + flask)
- `--tier medium`: ~600MB (adds ruff)
- `--tier all`: ~5GB (adds bevy + TypeScript)

The harness clones these at a pinned commit SHA (reproducibility) and stores them in the benchmark directory.

### Benchmark Directory

**Default: `/mnt/d/aether-bench`** (Windows D: drive via WSL2 mount)

**Why not `/tmp/`:** On WSL2, `/tmp/` is RAM-backed tmpfs. Cloning Bevy (~2GB) into tmpfs eats RAM and risks OOM alongside the indexing pipeline. The D: drive has ample space and keeps benchmark data off the constrained ext4.vhdx.

**Why not WSL2 native filesystem:** Robert's ext4.vhdx is at 140GB with ~9GB free. Cloning all tiers would consume ~5GB of repos alone, plus index data. The D: drive avoids this pressure entirely.

**Override:** `--bench-dir <path>` for custom location. If someone has a fast NVMe or plenty of WSL2 space, they can point it wherever.

**Note on I/O performance:** The 9P bridge between WSL2 and Windows drives adds latency (~2-5x slower than native ext4 for small random I/O). This means Pass 1 timings on D: will be slower than production (where repos live on native ext4). The benchmark report includes a note about the storage backend so results are interpreted correctly. For accurate production timings, clone repos to native ext4 if space permits.

### Provider Options

The benchmark supports all four real inference providers. No Mock provider exists — it was removed in Stage 8.2.

| Flag | Provider | Use Case | Requires |
|------|----------|----------|----------|
| `--provider ollama` | `qwen3_local` | **Default.** Local inference via Ollama. Unlimited rate, reproducible. Tests the full pipeline: Pass 1 + Pass 2 + embeddings + crash recovery. | Ollama running with `qwen3.5:9b` pulled |
| `--provider pass1-only` | None | Structural indexing only. Skips SIR generation. Useful for quick AST/graph scaling checks without Ollama. | Nothing |
| `--provider gemini` | `gemini` | Google Gemini Flash cloud API. Tests cloud provider throughput + rate limiting behavior (15 req/min free). | `GEMINI_API_KEY` |
| `--provider nim` | `openai_compat` | NVIDIA NIM cloud API. Tests OpenAI-compat provider path (40 req/min free). | `NVIDIA_NIM_API_KEY` |
| `--provider tiered` | `tiered` | **Hybrid mode.** Top ~20% symbols (score >= 0.8) go to cloud primary (NIM), rest go to Ollama fallback. Tests the full 8.2 routing logic. | `NVIDIA_NIM_API_KEY` + Ollama running |

**Default is `ollama`** because the whole point of the stress test is to validate the real pipeline — SIR generation, write intents, crash recovery, cross-database consistency. `pass1-only` is available as a quick structural check but doesn't exercise what 8.1 and 8.2 built.

When a provider is selected, the benchmark script creates a minimal `.aether/config.toml` per workspace:

```bash
# For --provider ollama (DEFAULT):
cat > "$BENCH_WORKSPACE/.aether/config.toml" <<EOF
[inference]
provider = "qwen3_local"
model = "qwen3.5:9b"
endpoint = "http://127.0.0.1:11434"
EOF

# For --provider nim:
cat > "$BENCH_WORKSPACE/.aether/config.toml" <<EOF
[inference]
provider = "openai_compat"
model = "qwen3.5-397b-a17b"
endpoint = "https://integrate.api.nvidia.com/v1"
api_key_env = "NVIDIA_NIM_API_KEY"
EOF

# For --provider gemini:
cat > "$BENCH_WORKSPACE/.aether/config.toml" <<EOF
[inference]
provider = "gemini"
model = "gemini-flash-latest"
api_key_env = "GEMINI_API_KEY"
EOF

# For --provider tiered (NIM primary + Ollama fallback):
cat > "$BENCH_WORKSPACE/.aether/config.toml" <<EOF
[inference]
provider = "tiered"

[inference.tiered]
primary = "openai_compat"
primary_model = "qwen3.5-397b-a17b"
primary_endpoint = "https://integrate.api.nvidia.com/v1"
primary_api_key_env = "NVIDIA_NIM_API_KEY"
primary_threshold = 0.8
fallback_model = "qwen3.5:9b"
fallback_endpoint = "http://127.0.0.1:11434"
retry_with_fallback = true
EOF

# For --provider pass1-only: no config needed (no inference runs)
```

The benchmark report captures which provider was used and the effective config, so results are reproducible.

### Benchmark Phases

Each repo goes through:

**Phase A — Pass 1 Timing (AST + Graph)**
- Run `aetherd --workspace <path> --index-once` (structural Pass 1 only, from 8.2)
- Capture: wall clock time, peak RSS memory, symbol count, edge count
- Verify: fsck reports zero inconsistencies after Pass 1
- **Always runs regardless of `--provider` flag.**

**Phase B — Pass 2 Partial Timing (SIR Generation)**
- **Skipped when `--provider pass1-only`.**
- Run Pass 2 on the top 100 priority symbols using the selected provider
- Capture: SIR generation time per symbol (p50, p95, p99), success rate, queue drain rate
- Capture: which provider generated each SIR (relevant for tiered mode — shows the cloud/local split)
- Verify: fsck reports zero inconsistencies after partial Pass 2

**Phase C — Query Latency**
- After indexing, run a battery of queries:
  - Lexical search: 10 queries, measure response time
  - Graph query (call chain): 5 queries on high-centrality symbols
  - MCP tool call (aether_get_sir): 5 calls for indexed symbols (if Pass 2 ran), 5 calls for non-indexed (tests on-demand bump behavior)
- Capture: p50, p95, p99 latency for each query type

**Phase D — Simulated Crash Recovery**
- Medium tier only (ruff)
- **Skipped when `--provider pass1-only`** (no SIR writes to crash during)
- Start indexing Pass 2 with the selected provider
- After 50 SIR writes, send SIGKILL to aetherd
- Restart aetherd
- Verify: intent replay recovers incomplete writes (Stage 8.1 validation)
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
Bench Dir: /mnt/d/aether-bench (Windows D: via 9P)
Provider: tiered (openai_compat primary -> qwen3_local fallback, threshold 0.8)

## Summary
| Repo | Symbols | Pass 1 Time | Peak Memory | SIR Rate | fsck |
|------|---------|-------------|-------------|----------|------|
| ripgrep | 2,147 | 3.2s | 142MB | 100% (100 ollama) | ok |
| ruff | 10,438 | 18.7s | 287MB | 99.8% (18 nim, 82 ollama) | ok |
| bevy | 47,291 | 2m14s | 891MB | 99.1% (21 nim, 79 ollama) | ok |
| TypeScript | 31,055 | 1m42s | 623MB | 98.7% (19 nim, 81 ollama) | ok |
| flask | 3,281 | 4.1s | 156MB | 100% (100 ollama) | ok |

## Pass 1 Detail (AST + Graph)
...

## Pass 2 Detail (SIR Generation — Top 100 Symbols)
| Repo | Provider Split | p50 | p95 | p99 | Success |
|------|---------------|-----|-----|-----|---------|
| ripgrep | 0 cloud / 100 local | 1.2s | 2.1s | 3.4s | 100/100 |
| ruff | 18 cloud / 82 local | 0.8s | 2.5s | 5.1s | 99/100 |

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
- Post-recovery fsck: clean

## Concurrent Load (aether-query)
- 20 concurrent MCP requests
- p50: 45ms, p95: 180ms, p99: 420ms
- Failures: 0

## Notes
- Storage: /mnt/d/aether-bench (Windows D: via 9P — Pass 1 timings ~2-5x slower than native ext4)
- For production-representative timings, clone repos to WSL2 native filesystem
```

### Implementation

The harness is shell scripts only — no separate Rust binary:

```
tests/stress/
├── run_benchmark.sh         # Orchestrator: clone repos, run phases, generate report
├── repos.toml               # Repo URLs, pinned commits, expected symbol ranges
├── benchmark_helpers.sh     # Shell functions for metric capture
├── README.md                # How to run benchmarks, interpret results
└── reports/                 # Output directory for generated reports (gitignored)
```

**`run_benchmark.sh`** is the entry point. It:
1. Reads `repos.toml` for repo list
2. Clones each repo to `$BENCH_DIR/{name}/` (default: `/mnt/d/aether-bench`)
3. Builds aetherd with `cargo build -p aetherd --release`
4. Creates provider-specific `.aether/config.toml` for each benchmark workspace
5. For each repo, runs the benchmark phases
6. Collects metrics into a JSON intermediate format
7. Generates the markdown report from JSON (heredoc + sed/awk formatting)

**Memory measurement:** Use `/usr/bin/time -v` wrapper and parse "Maximum resident set size".

**Timing:** Bash `SECONDS` variable for phase-level, `date +%s%N` for sub-second precision.

### CI Integration (Future)

The stress test is designed to run in CI as a nightly job. For now, it runs manually:
```bash
cd /home/rephu/projects/aether

# Default: Ollama local inference, small repos (ripgrep + flask)
./tests/stress/run_benchmark.sh --tier small

# Quick structural check (no Ollama needed):
./tests/stress/run_benchmark.sh --tier small --provider pass1-only

# Hybrid cloud+local (your production config):
./tests/stress/run_benchmark.sh --tier small --provider tiered

# Full scale with local inference:
./tests/stress/run_benchmark.sh --tier all

# Full scale with hybrid:
./tests/stress/run_benchmark.sh --tier all --provider tiered

# Custom bench directory (if D: drive isn't available):
./tests/stress/run_benchmark.sh --tier small --bench-dir /home/rephu/bench-data
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

**Note:** This stage does NOT create a separate Rust binary for the benchmark runner. The shell script invokes `aetherd` CLI commands directly and captures output. A Rust-based runner can be added later if the shell approach proves insufficient.

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
| Ollama not running (default provider) | Script detects (curl healthcheck), prints setup instructions, exits |
| `--provider nim` but no API key | Script checks env var, reports error, exits |
| `--provider tiered` but Ollama not running | Script detects, reports error, exits |
| `--provider tiered` but no cloud API key | Script checks env var, reports error, suggests `--provider ollama` |
| WSL2 memory constraints (12GB) | Large repos may OOM; script tracks and reports peak memory |
| Pass 2 with rate-limited provider (Gemini 15/min, NIM 40/min) | Script logs effective throughput; tiered mode shows cloud vs local split |
| /mnt/d/ not mounted | Script checks bench-dir exists, prints error with suggestion to use --bench-dir |
| Bench dir on slow filesystem | Report includes storage backend note for context on timings |

---

## Pass Criteria

1. Shell scripts are created and pass shellcheck (if available).
2. `run_benchmark.sh --tier small` completes on ripgrep and flask with Ollama provider.
3. `run_benchmark.sh --tier small --provider pass1-only` completes without Ollama.
4. fsck runs after each indexing phase and results are captured.
5. Query latency is measured for lexical search and graph queries.
6. Simulated crash recovery works (SIGKILL + restart + verify) — medium tier with real provider.
7. Markdown report is generated in `tests/stress/reports/`.
8. Script handles errors gracefully (OOM, network failure, missing repos, missing provider, missing bench dir).
9. No production code is modified in this stage.

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

NOTE ON AETHER CLI (post-8.2):
- `aetherd --workspace <path> --index-once` runs Pass 1 only (structural index)
- `aetherd --workspace <path> --index-once --full` runs Pass 1 + Pass 2 (full pipeline)
- `aetherd --workspace <path> status` shows SIR coverage
- `aetherd --workspace <path> fsck` runs state verification (Stage 8.1)
- `aetherd --workspace <path> fsck --repair` replays failed intents + repairs inconsistencies
- Check `crates/aetherd/src/cli.rs` for exact flag names — adapt if they differ

NOTE ON INFERENCE PROVIDERS (post-8.2, Mock removed):
- There is NO mock provider. It was removed in Stage 8.2.
- Default benchmark provider is `ollama` (qwen3_local with qwen3.5:9b) — tests full pipeline
- Provider options: ollama (default), pass1-only, gemini, nim (openai_compat), tiered (hybrid)
- The benchmark script creates a `.aether/config.toml` per workspace with provider config
- For tiered mode, primary is openai_compat (NIM qwen3.5-397b-a17b), fallback is qwen3_local (qwen3.5:9b)
- The script must check for Ollama availability by default, and API keys for cloud providers

NOTE ON BENCHMARK DIRECTORY:
- Default bench dir is `/mnt/d/aether-bench` (Windows D: drive)
- Do NOT use /tmp/ — it is RAM-backed tmpfs on WSL2 and will OOM with large repo clones
- The D: drive is accessed via WSL2's 9P bridge — I/O is ~2-5x slower than native ext4
- The report should note the storage backend for context on timing results
- Override with `--bench-dir <path>` for custom location
- Script must verify the bench dir path exists or can be created before proceeding

NOTE ON MEMORY MEASUREMENT:
- On Linux (WSL2): wrap command with `/usr/bin/time -v` and parse "Maximum resident set size"
- Alternative: parse /proc/<pid>/status for VmPeak and VmRSS
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
   tests/stress/reports/*.json
   tests/stress/repos/
   ```

=== STEP 2: Define Benchmark Repos ===

8) In `tests/stress/repos.toml`, define repos. Use "HEAD" for commit — the script
   resolves to the current HEAD at clone time and records the actual SHA in the report.

   - ripgrep (BurntSushi/ripgrep) — small Rust, tier=small
   - flask (pallets/flask) — small Python, tier=small
   - ruff (astral-sh/ruff) — medium Rust+Python, tier=medium
   - bevy (bevyengine/bevy) — large Rust workspace, tier=large
   - TypeScript (microsoft/TypeScript) — large TypeScript, tier=large

=== STEP 3: Create Benchmark Helpers ===

9) In `tests/stress/benchmark_helpers.sh`, create shell functions:

   - `clone_repo(name, url, commit, dest_dir)` — git clone + checkout, skip if exists
   - `build_aether()` — cargo build -p aetherd --release (check if binary exists first)
   - `measure_command(label, command...)` — run command, capture wall time + exit code + peak memory
   - `run_aether_index(workspace, extra_args)` — run aetherd --index-once with given args
   - `run_aether_full_index(workspace)` — run aetherd --index-once --full
   - `run_aether_fsck(workspace)` — run fsck, capture output, parse pass/fail
   - `run_aether_status(workspace)` — run status, capture SIR coverage numbers
   - `check_ollama()` — curl http://127.0.0.1:11434/api/tags, return 0 if running
   - `check_api_key(env_var_name)` — verify environment variable is set and non-empty
   - `check_bench_dir(path)` — verify path exists or can be created; warn if on tmpfs
   - `write_provider_config(workspace, provider)` — create .aether/config.toml with
     the correct provider configuration:
       - pass1-only: no config needed (no inference)
       - ollama: provider=qwen3_local, model=qwen3.5:9b, endpoint=http://127.0.0.1:11434
       - gemini: provider=gemini, model=gemini-flash-latest, api_key_env=GEMINI_API_KEY
       - nim: provider=openai_compat, model=qwen3.5-397b-a17b,
              endpoint=https://integrate.api.nvidia.com/v1, api_key_env=NVIDIA_NIM_API_KEY
       - tiered: provider=tiered with [inference.tiered] section:
              primary=openai_compat, primary_model=qwen3.5-397b-a17b,
              primary_endpoint=https://integrate.api.nvidia.com/v1,
              primary_api_key_env=NVIDIA_NIM_API_KEY, primary_threshold=0.8,
              fallback_model=qwen3.5:9b, fallback_endpoint=http://127.0.0.1:11434,
              retry_with_fallback=true
   - `detect_storage_backend(path)` — check if path is on tmpfs, 9P (Windows drive),
     or native ext4 and return a label for the report
   - `run_query_benchmark(workspace, query_type)` — execute queries via aether CLI,
     capture latencies
   - `generate_json_result(metrics...)` — output structured JSON for report generation
   - `kill_and_recover(pid, workspace)` — SIGKILL, wait, restart, measure recovery

=== STEP 4: Create Main Orchestrator ===

10) In `tests/stress/run_benchmark.sh`:

    Parse arguments:
    - `--tier small|medium|large|all` (default: small)
    - `--provider ollama|pass1-only|gemini|nim|tiered` (default: ollama)
    - `--bench-dir <path>` (default: /mnt/d/aether-bench)
    - `--report-dir <path>` (default: tests/stress/reports)
    - `--skip-clone` (use existing clones)
    - `--help` (print usage)

    Provider validation on startup:
    - If --provider ollama (default) or tiered: check_ollama(), abort with setup instructions if not running
    - If --provider gemini: check_api_key("GEMINI_API_KEY"), abort if missing
    - If --provider nim: check_api_key("NVIDIA_NIM_API_KEY"), abort if missing
    - If --provider tiered: check both NIM key + Ollama availability
    - If --provider pass1-only: no checks needed

    Bench dir validation:
    - check_bench_dir(), warn if on tmpfs, create if missing

    Flow:
    a. Source benchmark_helpers.sh
    b. Parse repos.toml (use simple grep/awk — no TOML parser dependency)
    c. Build aether if needed
    d. Validate provider prerequisites
    e. Validate and create bench dir
    f. Detect storage backend for report metadata
    g. For each repo matching the selected tier:
       1. Clone repo (or skip if --skip-clone and exists)
       2. Create .aether/config.toml via write_provider_config()
       3. Phase A: Run Pass 1 index, capture time + memory + symbol count
       4. Run fsck, capture result
       5. Phase B: (skip if pass1-only) Run --index-once --full for top 100 symbols,
          capture SIR metrics including provider split for tiered mode
       6. Run fsck again (if Phase B ran)
       7. Phase C: Run query latency tests (lexical search, call chain, MCP get_sir)
       8. Phase D: (medium tier + real provider only) Simulated crash recovery
    h. Collect all results into a JSON array
    i. Generate markdown report from JSON (heredoc + sed/awk formatting)
       - Include provider config and storage backend in report header
       - For tiered mode, show cloud/local split per repo in results
    j. Print report path

    The script should be robust:
    - `set -euo pipefail` at top
    - Trap SIGINT/SIGTERM to clean up background processes
    - Log progress to stderr, results to files
    - Continue on individual repo failure (capture error, move to next)

=== STEP 5: Create README ===

11) In `tests/stress/README.md`, document:
    - Purpose of the stress test suite
    - Prerequisites (git, cargo, Ollama with qwen3.5:9b, enough disk space)
    - Disk space requirements per tier
    - Provider options table with requirements for each
    - Storage considerations (why D: drive, why not /tmp/, when to use native ext4)
    - How to run: default (`--tier small`), structural-only (`--provider pass1-only`),
      hybrid (`--provider tiered`), full scale (`--tier all`)
    - How to interpret the report (provider split, storage backend note)
    - How to add new benchmark repos
    - Memory requirements per tier
    - Known limitations
    - Example commands

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
    "Phase 8.7: Add stress test harness — benchmark suite with provider-aware scale validation"

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
