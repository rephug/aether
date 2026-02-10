# docs/roadmap/phase_1_stage_3_2_robustness.md

# Phase 1 — Stage 3.2: Robustness Polish (Stale SIR + Rate Limiting)

## Goal
Make AETHER resilient and “safe to leave running” by:
1) Never deleting a previously-good SIR if inference fails
2) Marking stale SIR state with enough info to debug
3) Adding basic rate limiting/backoff so bursts don’t overwhelm the system

## Non-goals
- Cost policy / paywall / quotas
- Sandbox / verification runtime
- Any UI work beyond surfacing stale status via MCP/LSP responses

## Required behavior
### Stale SIR behavior
If inference fails or times out for a symbol:
- Keep the last-good SIR
- Mark symbol metadata:
  - sir_status = "stale"
  - last_error = "<short error>"
  - last_attempt_at = <timestamp>

If inference later succeeds:
- sir_status returns to "ok"
- last_error cleared or preserved separately (either is fine; be consistent)

### Surface stale status
- MCP responses for “get SIR” must include status + last_error if stale
- LSP hover must show a warning banner if stale

### Rate limiting/backoff
- Add a basic in-process limiter (token bucket or semaphore + delay)
- If provider errors look like rate limiting, add exponential backoff (bounded)

## Revised pass criteria for this stage
This stage passes if ALL are true:

1) cargo fmt --check, cargo clippy -- -D warnings, cargo test pass
2) Tests confirm:
   - When inference is forced to fail, previous SIR remains accessible
   - Metadata marks stale with last_error + last_attempt timestamp
   - A later successful inference clears stale
3) MCP/LSP outputs include stale status and warning text (verified via tests where feasible)
4) No heavy dependencies added

## Codex execution prompt

### PROMPT 3 — polish: rate-limit safety + “stale SIR” behavior
Paste into Codex:

Add robustness polish for production-ish use:

1) If inference fails or times out when generating SIR for a changed symbol, do NOT delete existing SIR.
   - Mark the symbol metadata as "sir_status = stale" (or similar) with last_error + last_attempt timestamp.
   - MCP/LSP should surface that status in responses.
2) Add a basic rate limiter / backoff for inference provider calls (even for local providers).
3) Add tests for the stale behavior (failure keeps old SIR and marks stale).

Do not introduce heavy dependencies.
cargo test must pass.
Commit: "Robust inference failure handling and rate limiting"
