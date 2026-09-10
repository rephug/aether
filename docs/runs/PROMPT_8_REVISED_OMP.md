# PROMPT 8 (REVISED) — Phase 10.1b Run 1: BatchProvider trait, omp config, overlay, schema v19

**Replaces** PROMPT 8 in `AETHER_PROMPTS_TO_RUN.md`. Requires the Docs PR (Run 0) to have shipped **with the omp-native spec** (`phase_10_stage_10_1b_omp_provider.md` replaces `phase_10_stage_10_1b_subprocess_cli_providers.md` in `docs/runs/specs/` — swap the file before Run 0, and change that bullet of the Run 0 commit message to "Phase 10.1b (OMP Subprocess Provider)"). Also requires `docs/runs/OMP_DISCOVERY_REPORT.md` committed (add it to Run 0 as well; it is the ground truth this prompt cites).

```text
You are working in the repo at https://github.com/rephug/aether. This is
Run 1 of Phase 10.1b (OMP Subprocess Provider): the BatchProvider trait,
omp-aware config schema, the resolve_pass_config bug fix, the overlay
generator, the OmpProvider preflight, and the core schema v19 migration.
Do NOT implement the rpc prompt loop, frame parsing beyond what preflight
needs, or checkpoint/resume — those are Runs 2 and 3.

Read first, in this order:
  docs/roadmap/phase_10_stage_10_1b_omp_provider.md   (Decisions #126-#131)
  docs/runs/OMP_DISCOVERY_REPORT.md                    (sections B.1, B.2, C.3, F.2, F.4)
  crates/aetherd/src/batch/mod.rs                      (BatchPass, PassConfig, resolve_pass_config)
  crates/aether-config/src/batch.rs

PREFLIGHT:
1) git status --porcelain (must be clean)
2) git fetch origin && git switch main && git pull --ff-only
3) git worktree add -B feature/phase-10-1b-run1-omp-plumbing "$HOME/phase-10-1b-run1"
4) cd "$HOME/phase-10-1b-run1"

BUILD ENVIRONMENT (fallback; your local config takes precedence):
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2   # 16 on the Netcup server
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp && mkdir -p "$TMPDIR"

SOURCE INSPECTION (mandatory):
5) Read the merged batch pipeline end to end: batch/{mod,build,run,ingest,
   gemini,openai,anthropic}.rs and aether-config/src/batch.rs. Note how
   HTTP providers are dispatched today and where a provider abstraction
   would slot in with the least churn.
6) Reproduce the resolve_pass_config provider-subsection bug: write a
   FAILING test first showing per-provider [batch.providers.<name>]
   overrides not resolving for all passes, then fix it.
7) Enumerate EVERY check_compatibility("core", 18) call site and every
   test asserting schema_version.version:
   grep -rn 'check_compatibility\|user_version\|schema_version' crates/ --include='*.rs'
   Minimum expected: crates/aether-dashboard/src/state.rs,
   crates/aether-mcp/src/state.rs. Report the full list in the PR body.
8) Check whether `omp` and `bun` exist on this machine (which omp; omp
   --version; bun --version). If absent, everything below must still
   compile and test via the fake-omp harness; real-omp checks are gated.

IMPLEMENTATION:

9) BatchProvider trait in crates/aether-infer (Decision #126). Shape it
   around what batch/{build,run}.rs actually needs:
     - fn provider_type(&self) -> &'static str
     - async fn health_check(&self, passes: &[PassConfig]) -> Result<HealthReport, BatchProviderError>
     - async fn run_batch(&self, pass: BatchPass, prompts: Vec<SymbolPrompt>) -> Result<Vec<SymbolResult>, BatchProviderError>
       (SymbolResult carries symbol_id, raw text, optional reasoning_trace, provider_type, model selector)
   Define BatchProviderError as an enum with exactly these variants and
   fields (from Decision #130):
     QuotaExceeded { provider: String, resume_after: Option<DateTime<Utc>>, detail: String }
     AuthFailure   { provider: String, detail: String }
     Transient     { detail: String }
     SubprocessFailure { exit_code: Option<i32>, detail: String }
     Config        { detail: String }
   Adapt the three HTTP providers as thin implementations (do not rewrite
   their internals).

10) Config schema (aether-config/src/batch.rs), all optional/defaulted:
   [batch]: scan_provider, triage_provider, deep_provider (String),
     scan_batch_size=100, triage_batch_size=20, deep_batch_size=1,
     parallel_worker_limit=8, checkpoint_path=".aether/batch/.checkpoint.json"
   BatchProviderConfig: provider_type (enum: http_gemini | http_openai |
     http_anthropic | cli_omp), command: Option<String>,
     timeout_secs: Option<u64>, overlay_path: Option<PathBuf>,
     passes: Option<HashMap<BatchPass, PassModel { model: String }>>
   Selector validation at config load (Decision #131): a cli_omp pass
   model MUST match ^[a-z0-9-]+/[^:\s]+(:(off|minimal|low|medium|high|xhigh|max|auto))?$
   Reject bare names with a clear error naming the pass. Provide
   thinking mapping from AETHER's existing thinking enum:
   none->off, dynamic->auto, else identity.
   Add a fixture test loading a pre-10.1b config.toml unchanged.

11) Fix resolve_pass_config per the failing test from step 6.

12) Overlay generator (aether-infer, module omp::overlay): write
   .aether/omp-overlay.yml with EXACTLY the content in Decision #127 if
   absent; if present and byte-identical to the expected content, leave
   it; if present and different, do not overwrite — return
   Config error naming the path and the differing keys (the user may have
   customized it deliberately). Test all three cases.

13) OmpProvider skeleton (aether-infer, module omp::provider) with a real
   health_check() and stubbed run_batch() returning Config error
   "OmpProvider::run_batch lands in Phase 10.1b Run 2". health_check():
     a. `<command> --version` parses to >= 18.0.10 (semver compare;
        report actual version)
     b. `bun --version` >= 1.3.14
     c. Ensure overlay exists (step 12)
     d. Spawn `<command> --mode rpc --no-session --no-extensions
        --no-skills --no-rules --no-tools --config <overlay> --cwd <ws>`
        with stdin piped; read the `ready` frame; send
        {"id":"n","type":"negotiate_protocol","protocolVersion":2};
        send get_login_providers and get_available_models; then close
        stdin and wait for exit 0 (timeout 30 s; abort + kill on timeout).
     e. For every provider referenced by a configured cli_omp pass:
        must appear in get_login_providers with authenticated:true
        (else AuthFailure naming the provider). For every configured
        selector: provider/modelId must appear in get_available_models
        (else Config error naming the pass and the nearest matching ids).
     f. stdout parsed as NDJSON only; stderr captured to the log at debug
        level and never parsed.
   Provide a minimal frame reader that handles v2 rpc_chunk reassembly
   (Discovery B.2: frames > 1 MiB split into chunks with chunkId/index/
   count/data) — Run 2 reuses it.

14) Fake omp harness for tests: a small script (bash or Python, under
   crates/aether-infer/tests/fixtures/omp/fake_omp.sh) that honors
   --version, and in --mode rpc emits a canned `ready`, answers
   negotiate_protocol / get_login_providers / get_available_models from
   fixture JSON files, and exits 0 on stdin close. Tests point `command`
   at it. Fixtures: providers_all_authenticated.json,
   providers_anthropic_missing.json, models_basic.json.

15) Core schema v18 -> v19: add provider_type TEXT (nullable) to
   fingerprint_history; migration test opens a v18 fixture and verifies
   v19 + column. Update EVERY call site and assertion found in step 7.

16) Wire startup health_check() into the aetherd batch commands: run for
   every provider referenced by the configured passes; a failure is a
   warning at startup and fatal only when a pass actually uses that
   provider (matches existing HTTP provider behavior).

17) Decision register: add #126-#131 from the spec (they are fully
   specified there; this run locks all six even though #128-#130 are
   exercised in Runs 2-3).

TESTS (per-crate):
- resolve_pass_config failing-then-fixed
- config backward-compat fixture
- selector validation (accept 3 valid, reject bare "sonnet", reject bad
  thinking suffix)
- overlay generator: absent / identical / differing
- health_check against fake omp: all-authenticated passes; anthropic
  missing -> AuthFailure naming "anthropic"; selector not in models ->
  Config error naming the pass; version too old -> Config error
- rpc_chunk reassembly unit test
- migration v18 -> v19
- real omp smoke (ignored unless AETHER_TEST_REAL_OMP=1): health_check
  against the installed omp with whatever is logged in; assert it
  returns without panicking and prints the authenticated provider list.

VALIDATION (per-crate only; never --workspace):
cargo fmt --all --check
cargo clippy -p aether-infer -- -D warnings
cargo clippy -p aether-config -- -D warnings
cargo clippy -p aetherd -- -D warnings
cargo clippy -p aether-store -- -D warnings
cargo test -p aether-infer
cargo test -p aether-config
cargo test -p aetherd
cargo test -p aether-store
cargo test -p aether-mcp
cargo test -p aether-dashboard

COMMIT + PR:
git commit -m "Phase 10.1b Run 1: BatchProvider trait, omp provider config, overlay, schema v19

- BatchProvider trait + BatchProviderError (QuotaExceeded/AuthFailure/
  Transient/SubprocessFailure/Config) in aether-infer; HTTP Gemini/
  OpenAI/Anthropic batch providers adapted as implementations (#126)
- [batch] per-pass provider selection, batch sizes 100/20/1,
  parallel_worker_limit=8, checkpoint_path; [batch.providers.*] gains
  provider_type (incl. cli_omp), command, timeout_secs, overlay_path,
  per-pass explicit provider/model[:thinking] selectors validated at
  load (#131)
- omp overlay generator: .aether/omp-overlay.yml pins fallback chains
  off, memory/autolearn off, project MCP off (#127)
- OmpProvider::health_check: omp >= 18.0.10, bun >= 1.3.14, rpc
  handshake (protocol v2), get_login_providers authenticated check,
  get_available_models selector check; rpc_chunk reassembly
- Fix resolve_pass_config provider subsection resolution
- Core schema v18 -> v19: fingerprint_history.provider_type; all
  check_compatibility sites and schema-version assertions updated
  (full list in PR body)
- Fake omp test harness under tests/fixtures/omp/

run_batch for cli_omp lands in Run 2; checkpoint/resume in Run 3.
Backward compatible: pre-10.1b configs parse unchanged."
git push -u origin feature/phase-10-1b-run1-omp-plumbing
PR title: "Phase 10.1b Run 1: BatchProvider trait + omp provider config + schema v19"
PR body: commit body + the complete check_compatibility call-site list +
the output of step 8 (omp/bun versions on this machine, or "absent").
```

**After merge:** standard cleanup, then run the real smoke on the dev box
(`AETHER_TEST_REAL_OMP=1 cargo test -p aether-infer real_omp -- --ignored --nocapture`)
and send me its output plus, if you can trigger one, a captured usage-limit
frame set (`omp -p --mode json --no-session --model <sel> "reply OK" </dev/null | tee frames.ndjson`).
Both feed directly into the Run 2 prompt.
