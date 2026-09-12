# Phase 10 — Stage 10.1b: Subprocess CLI Providers

**Status:** Spec complete, not implemented
**Decisions:** #126–#131
**Schema:** core v18 → v19 (`fingerprint_history.provider_type` column)
**Depends on:** merged batch pipeline (crates/aetherd/src/batch/),
Phase 5.6b (template docs mention new providers)
**Estimated:** 2–3 Codex runs, ~15–20 hours total

## Purpose

Make subscription-backed CLIs — Claude Code Max, Gemini CLI (Google AI
Ultra), Codex CLI (ChatGPT) — first-class $0-marginal-cost batch
providers alongside the existing HTTP API providers, so the entire
scan/triage/deep enrichment pipeline can run without API spend for users
holding any combination of these subscriptions.

## In scope

- New `BatchProvider` trait in `crates/aether-infer`
- Three concrete subprocess implementations: `GeminiCliProvider`,
  `CodexCliProvider`, `ClaudeCodeMaxProvider`
- Shared `SubprocessProvider` base handling stdin/arg prompt marshaling,
  timeout, stdout capture, stderr classification
- New `[batch.providers.*]` subsections + new `[batch]` fields:
  `scan_provider`, `triage_provider`, `deep_provider`,
  `scan_batch_size`, `triage_batch_size`, `deep_batch_size`,
  `parallel_crate_limit`, `checkpoint_path`
- `[watcher].realtime_provider` extended to reference
  `[batch.providers]` entries
- Checkpoint read/write in the batch runner; resume logic on
  `batch build` startup (detect checkpoint, log banner, resume)
- Per-provider error classification; `health_check()` on startup for all
  configured subprocess providers
- `fingerprint_history` schema extension: `provider_type` column
  (core schema v19 — list ALL `check_compatibility("core", 19)` call
  sites: aether-dashboard/src/state.rs, aether-mcp/src/state.rs, plus any
  test assertions on schema_version.version)
- Prerequisite bug fixes bundled into Run 1: `resolve_pass_config()`
  provider subsection bug; mock provider confidence 1.0 → 0.1 (if WF.0
  has not already shipped it)
- Tests: mock subprocess provider simulating each error condition;
  integration tests with real CLIs gated by `AETHER_TEST_REAL_CLIS=1`
- Docs: CLAUDE.md / GEMINI.md / AGENTS.md templates note the new options

## Out of scope

Cross-provider fallback (explicitly rejected); per-crate provider
overrides; provider config UI (Phase 9.2); subscription auto-detection
(Phase 9.3); non-subscription subprocess providers (Ollama already has a
direct HTTP provider); usage metrics/telemetry (Phase 8.5); cost tracking
(providers are $0 by definition); rate limiter / token bucket —
pause-and-resume IS the rate limiting strategy.

## Locked decisions

### 126. Subprocess providers are a first-class batch provider type
They share the same `BatchProvider` trait and the same prompt hashing /
fingerprint history infrastructure as HTTP providers. The pipeline
dispatches on config lookup, not separate code paths.

### 127. No cross-provider fallback
On quota hit, pause and let the user decide. Automatic fallback is
rejected: (a) users have real preferences about which model generates
which SIRs, (b) reproducibility suffers, (c) single-subscription users
would have no fallback target anyway.

### 128. Batch sizes per pass: 100 / 20 / 1
Symbols per CLI invocation for scan / triage / deep respectively.

### 129. Parallel crate limit: 8
Raised from 4.

### 130. Pause-and-resume checkpoint at `.aether/batch/.checkpoint.json`
On `QuotaExceeded` the runner writes:

```json
{
  "schema_version": 1,
  "paused_at": "2026-04-09T14:23:45Z",
  "provider": "claude-code-max",
  "pass": "triage",
  "reason": "session limit reached",
  "last_completed_symbol_id": "...",
  "remaining_batches": 47,
  "total_symbols_processed": 1847,
  "total_symbols_remaining": 4700,
  "suggested_resume_after": "2026-04-09T19:23:45Z"
}
```

On next `batch build`: detect checkpoint → log resume banner → retry;
prompt-hash skip automatically avoids re-processing completed symbols;
delete checkpoint when the pass completes; update timestamp and exit on a
repeat quota hit. `suggested_resume_after` is advisory (Claude Max ~5h
window, Gemini CLI ~1-day rolling, Codex tier-dependent).

### 131. stderr classification per provider
- Gemini CLI: "quota exceeded"/"rate limit" → QuotaExceeded; "not
  authenticated"/"login required" → AuthFailure; "network"/conn refused →
  Transient; other non-zero → SubprocessFailure
- Codex CLI: "rate_limit_exceeded" → QuotaExceeded;
  "invalid_api_key"/"Please sign in" → AuthFailure; exit 130 (SIGINT) →
  SubprocessFailure (user-cancelled); other → SubprocessFailure
- Claude Code Max: "session limit reached"/"usage limit" → QuotaExceeded;
  "not authenticated" → AuthFailure; other → SubprocessFailure

Patterns MUST be verified against actual CLI output during implementation.

## Integration with the merged batch pipeline

1. **Prompt hashing** — reuse `blake3(source_hash + neighbor_sir_hashes +
   config_hash)` unchanged; resume skips completed symbols for free.
2. **JSONL** — subprocess providers do NOT write JSONL (no external
   service). `batch build --pass <pass>` branches on provider type: HTTP
   writes JSONL, subprocess reads the symbol list from the symbols table
   and pipes prompts to CLIs inline.
3. **Auto-chaining** — `batch run --passes scan,triage,deep` works
   unchanged. A provider auth failure is only fatal when a pass actually
   uses that provider.

## Suggested Codex runs

- **Run 1** (~4–6h): trait + config schema + resolve_pass_config fix +
  schema v19 migration. Mostly plumbing.
- **Run 2** (~6–10h): three concrete providers + error classification +
  unit tests. The bulk of the work.
- **Run 3** (~3–5h): checkpoint/resume + integration tests + docs.

Draft each run's prompt only after the previous run merges.
