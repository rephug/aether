# Phase 10 — Stage 10.1b: OMP Subprocess Provider

**Status:** Spec complete (v2, omp-native), not implemented
**Supersedes:** the three-CLI design (claude -p / gemini / codex) from the April 10 planning session
**Decisions:** #126–#131
**Schema:** core v18 → v19 (`fingerprint_history.provider_type` column)
**Depends on:** merged batch pipeline (`crates/aetherd/src/batch/`), Phase 5.6b (AGENTS.md carries omp-facing guidance)
**Ground truth:** `docs/runs/OMP_DISCOVERY_REPORT.md` (read against `@oh-my-pi/pi-coding-agent@18.0.10` source)
**Estimated:** 3 Codex/Claude Code runs

## Purpose

Make subscription-backed inference a first-class $0-marginal-cost batch
provider by driving **oh-my-pi (`omp`)** as a subprocess. omp already
holds OAuth logins for Anthropic, OpenAI Codex, Google Antigravity/Gemini
and ~60 other providers in one credential store (`~/.omp/agent/agent.db`),
so AETHER needs exactly one subprocess provider instead of one per CLI.

## Why omp instead of per-CLI providers

| Concern | Per-CLI design | omp design |
|---|---|---|
| Logins | three CLIs, three auth flows, headless device-code on servers | one store, already logged in |
| Error discrimination | stderr regex per CLI, verified against nothing | typed frames: `errorStatus`, `errorId` bit-flags, `retry-after-ms` |
| Quota wait | guess (5h Claude / 1d Gemini / ?) | omp states the exact wait it computed |
| Process cost | one process per call (1.7–14 s startup) | one long-lived rpc process per worker slot |
| Fallback control | n/a | pinned off via per-invocation overlay |
| Tools | `--allowedTools "mcp__aether*"` wildcard | `--no-tools` for pure inference; host tools available later |

## In scope

- `BatchProvider` trait in `crates/aether-infer` (unchanged from v1 plan)
- One concrete implementation: `OmpProvider` (`provider_type = "cli_omp"`)
- RPC client: spawn, `negotiate_protocol` v2, `set_model` / `set_thinking_level`, `prompt`, frame parsing, `get_last_assistant_text`, `new_session` between prompts, `abort`, clean shutdown (close stdin → exit 0)
- Overlay config generated per run at `.aether/omp-overlay.yml` and passed via `--config`
- Preflight `health_check()`: `omp --version` ≥ 18.0.10, `bun --version` ≥ 1.3.14, `get_login_providers` authenticated for every provider a configured pass references, `get_available_models` contains every configured selector
- Config: `[batch.providers.omp]` + per-pass provider/model selection + batch sizes + `parallel_worker_limit` + `checkpoint_path`
- Frame-based error classification → `QuotaExceeded { resume_after }`, `AuthFailure`, `Transient`, `SubprocessFailure`
- Checkpoint / pause-and-resume (format unchanged from v1, `suggested_resume_after` now real)
- `fingerprint_history.provider_type` column (schema v19) — every `check_compatibility("core", 19)` site updated
- Prerequisite bug fix bundled into Run 1: `resolve_pass_config()` provider-subsection resolution
- Tests: fake `omp` script emitting canned NDJSON for every frame path; integration tests gated by `AETHER_TEST_REAL_OMP=1`
- Docs: AGENTS.md / CLAUDE.md templates mention `provider_type = "cli_omp"`; `docs/OMP_PROVIDER.md`

## Out of scope

Cross-provider fallback (rejected, #127); host tools (`set_host_tools`) for batch passes — reserved for a later stage and for Phase 9.6; per-crate provider overrides; provider config UI (9.2); `--profile` isolation (it isolates logins too — never use it); the Node SDK path (Bun runtime in-process); `-p` text mode as a primary path (no error discrimination).

## Locked decisions

### 126. omp is the single subprocess provider
`provider_type = "cli_omp"` replaces the planned `cli_gemini` / `cli_codex` / `cli_claude_max`. All subscription routing goes through omp's credential store. HTTP providers (Gemini/OpenAI/Anthropic batch APIs) remain as before; the pipeline dispatches on config lookup, not separate code paths.

### 127. No cross-provider fallback — enforced by overlay
AETHER generates `.aether/omp-overlay.yml` and passes `--config` on every spawn:
```yaml
retry:
  fallbackChains: {}
  modelFallback: false
  usageAwareFallback: false
  maxRetries: 1            # surface quota to AETHER fast; AETHER owns the pause
  # maxDelayMs left at default 300000 (fail-fast when provider wait exceeds 5 min)
providers:
  anthropic:
    serverSideFallback: false
memory:
  backend: off
autolearn:
  enabled: false
mcp:
  enableProjectConfig: false
```
Overlay precedence is global → project → overlay, so this overrides the user's `~/.omp/agent/config.yml` and any project `.omp/config.yml` without modifying either. **Amendment:** omp's *same-provider* OAuth account rotation (several logins for one provider) is not cross-provider and is permitted.

### 128. Batch sizes per pass: 100 / 20 / 1 — one rpc process per worker slot
Symbols per prompt for scan / triage / deep. Each worker slot owns one long-lived `omp --mode rpc --no-session --no-extensions --no-skills --no-rules --no-tools --config <overlay>` process; `new_session` is sent between prompts so context never accumulates. Batch passes run with **zero tools** (`--no-tools` + `mcp.enableProjectConfig: false` → empty `dumpTools`); the pipeline's prompt builder supplies all context in-prompt as it does for HTTP providers.

### 129. Parallel worker limit: 8
`parallel_worker_limit` (default 8) bounds rpc processes. omp's shared `agent.db` is WAL + busy-timeout protected; a usage-limit block recorded by one process is visible to siblings via `auth_credential_blocks`, which stops them burning retries.

### 130. Pause-and-resume keyed off omp's frame signals
Classification, in priority order, from the rpc frame stream:
1. `auto_retry_end { success:false, finalError:"Provider requested <N>ms wait, exceeds retry.maxDelayMs…" }` → `QuotaExceeded { resume_after: now + N }`
2. `auto_retry_start { delayMs, errorId }` with `errorId & 0x0008_0000` (UsageLimit) → `QuotaExceeded { resume_after: now + delayMs }`
3. Terminal assistant message (`message_end`/`agent_end`, role assistant): `stopReason:"error"` with `errorStatus ∈ {429, 402}` or `errorId & UsageLimit` → `QuotaExceeded`, `resume_after` parsed from trailing `retry-after-ms=<n>` if present
4. `errorStatus ∈ {401, 403}` or `errorId & 0x0100_0000` (AuthFailed) or `& 0x4000_0000` (OAuthExpiry), or command response `success:false` whose `error` starts with `No API key found` → `AuthFailure`
5. `errorStatus ≥ 500` or `errorId & 0x0002_0000` (Transient) → `Transient` (retry once at AETHER level, then `SubprocessFailure`)
6. Process exit ≠ 0, malformed frame, or `ready` never received → `SubprocessFailure`

Exit codes are **not** a signal (rpc exits 0 on mid-turn errors). Checkpoint file format is unchanged from the v1 spec (`.aether/batch/.checkpoint.json`, `schema_version: 1`); `suggested_resume_after` is now populated from the signal above rather than a per-CLI guess. Out-of-band `omp usage --json` may refine it later.

### 131. Explicit selectors, no omp roles, validated at preflight
Per-pass model is an explicit `provider/model[:thinking]` string (e.g. `anthropic/claude-sonnet-5:low`). Never a bare name (fuzzy match could pick another provider) and never omp roles (`smol/slow/plan` are the user's global interactive defaults). Thinking mapping from AETHER's enum: `none→off`, `dynamic→auto`, others 1:1. Sent as `--model` at spawn and `set_model` + `set_thinking_level` before each prompt. Preflight rejects a run whose selectors are absent from `get_available_models` or whose providers are not `authenticated` in `get_login_providers`.

## Config

```toml
[batch]
scan_provider   = "omp"
triage_provider = "omp"
deep_provider   = "omp"
scan_batch_size = 100
triage_batch_size = 20
deep_batch_size = 1
parallel_worker_limit = 8
checkpoint_path = ".aether/batch/.checkpoint.json"

[batch.providers.omp]
provider_type = "cli_omp"
command = "omp"                 # or absolute path
timeout_secs = 900              # per prompt
overlay_path = ".aether/omp-overlay.yml"   # generated if absent

[batch.providers.omp.passes]
scan   = { model = "openai-codex/gpt-5.6-luna:minimal" }
triage = { model = "anthropic/claude-sonnet-5:low" }
deep   = { model = "anthropic/claude-fable-5:high" }
```
Existing configs must parse unchanged (all new fields optional/defaulted).

## RPC protocol contract (from the discovery report, 18.0.10)

- Spawn args: `--mode rpc --no-session --no-extensions --no-skills --no-rules --no-tools --config <overlay> --model <selector> --cwd <workspace>`
- First stdout frame: `{"type":"ready","protocolVersion":1,"supportedProtocolVersions":[1,2],…}` → send `{"id":"…","type":"negotiate_protocol","protocolVersion":2}`
- Per prompt: `set_model {provider, modelId}` → `set_thinking_level {level}` → `prompt {message}` → consume frames until `agent_end` → `get_last_assistant_text` → `new_session`
- Frames > 1 MiB arrive as `rpc_chunk {chunkId,index,count,data}` under v2 — reassemble
- Shutdown: close stdin; process exits 0. `abort` on timeout first.
- stdout is NDJSON only; stderr is informational — log it, never parse it for control flow
- MCP tool names, if ever enabled, are `mcp__aether_<tool>` (server key `aether`, redundant prefix stripped)

## Suggested runs

- **Run 1 — plumbing:** `BatchProvider` trait; HTTP providers adapted; config schema; `resolve_pass_config` fix; schema v19; overlay generator; `OmpProvider::health_check()` (version checks + `get_login_providers` + `get_available_models`). No prompts executed yet.
- **Run 2 — the provider:** rpc client, frame parser, text extraction, classification per #130, `new_session` cycling, timeouts/abort, fake-omp test harness covering every frame path.
- **Run 3 — resilience:** checkpoint/resume, parallel worker pool, real-omp integration tests behind `AETHER_TEST_REAL_OMP=1`, docs, template updates.

## Open items carried from discovery (verify on the dev box before Run 2)

1. Is `anthropic` actually logged in via OAuth (`omp token anthropic --list`)? If not, Claude via omp is metered, not $0.
2. Capture one real usage-limit frame set per subscription provider and check it into `crates/aether-infer/tests/fixtures/omp/` — the classifier tests should run against real samples, not just the source-derived shapes.
3. Confirm whether `retry.maxDelayMs: 0` (omp sleeps through the window) is preferable to AETHER-owned pause for unattended overnight server runs; #130 assumes AETHER-owned.
