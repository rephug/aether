# Phase 4 - Stage 4.2: Structured Logging with `tracing`

## Purpose
Replace all `eprintln!` calls with the `tracing` crate for structured, leveled, filterable logging. The prospectus specifies `tracing + tracing-subscriber` (§5 Tech Stack). Currently there are ~30 `eprintln!` calls scattered across all crates with no filtering, no levels, and no structured fields.

## Current implementation (what we're replacing)
- `eprintln!("aether-store: mirror write failed for symbol {symbol_id}: {err}");`
- `eprintln!("AETHER search fallback: {reason}");`
- No log levels, no structured fields, no way to silence or filter.

## Target implementation
- All crates use `tracing::{info, warn, error, debug, trace}` macros
- `aetherd` binary initializes `tracing-subscriber` with:
  - `RUST_LOG` env var for filtering (default: `info`)
  - Optional JSON output for machine consumption (`--log-format json`)
  - Human-readable default for terminal use
- Span context for key operations: `indexing`, `sir_generation`, `search`, `verification`

## In scope
- Add `tracing = "0.1"` and `tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }` to workspace deps
- Add `tracing` dep to every crate that currently uses `eprintln!`
- Replace every `eprintln!` with appropriate `tracing` macro + structured fields
- Initialize subscriber in `aetherd/src/main.rs` early in startup
- Add `--log-format` CLI flag: `human` (default) | `json`
- Add `[general] log_level` config field (default: `"info"`)

## Out of scope
- OpenTelemetry export
- Log file rotation
- Performance tracing / flamegraph integration

## Pass criteria
1. Zero `eprintln!` calls remain in any crate (grep verification).
2. `RUST_LOG=debug cargo run -p aetherd -- --workspace /tmp/test` produces leveled output.
3. `--log-format json` produces one JSON object per log line.
4. Tests still pass (tracing macros are no-ops without a subscriber in tests).
5. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_4_stage_4_2_structured_logging.md for full spec.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase4-stage4-2-tracing off main.
3) Create worktree ../aether-phase4-stage4-2-tracing for that branch and switch into it.
4) Add workspace dependencies:
   - tracing = "0.1"
   - tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
5) Add tracing dependency to every crate that uses eprintln!
6) Replace ALL eprintln! calls with tracing macros (error!, warn!, info!, debug!).
   Use structured fields: tracing::warn!(symbol_id = %id, error = %err, "mirror write failed")
7) Initialize tracing-subscriber in crates/aetherd/src/main.rs before any other logic.
8) Add --log-format CLI flag (human | json) and [general] log_level config field.
9) Verify: grep -r 'eprintln!' crates/ returns zero matches.
10) Run:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test --workspace
11) Commit with message: "Replace eprintln with structured tracing".
```

## Expected commit
`Replace eprintln with structured tracing`
