# Phase 8 — Post-8.8 Testing Session Context

**Date:** March 6, 2026
**Author:** Robert + Claude (session collaboration)
**Repo:** `rephug/aether` at commit `094d95a` (Phase 8.8 merged)
**Phase:** 8 — The Crucible (Hardening & Scale)

---

## Pass Naming — Corrected

The Stage 8.8 implementation used incorrect naming for the three-pass pipeline.
This document establishes the correct naming which must be applied throughout the
codebase, config schema, DB values, constants, and documentation.

### Correct Pass Names

| Pass | Name | Purpose |
|------|------|---------|
| Pass 1 | **scan** | Fast SIR generation for all symbols. Cheap model, no enrichment. Builds the baseline. |
| Pass 2 | **triage** | Enriched context pass. Uses neighbor intents + baseline SIR from scan. Selects which symbols need deep analysis. For Gemini this is self-improvement (flash-lite → flash-lite with context). |
| Pass 3 | **deep** | Deep analysis on symbols selected by triage. CoT for local models. Premium model (e.g. Sonnet) for cloud. Top-N by priority. |

### Rename Required In Code

| Old | New |
|-----|-----|
| `SIR_GENERATION_PASS_TRIAGE` constant value `"triage"` | `SIR_GENERATION_PASS_SCAN` constant value `"scan"` |
| `SIR_GENERATION_PASS_SINGLE` constant value `"single"` | merge into `SIR_GENERATION_PASS_SCAN` value `"scan"` |
| New constant needed | `SIR_GENERATION_PASS_TRIAGE` value `"triage"` |
| `SIR_GENERATION_PASS_DEEP` constant value `"deep"` | unchanged |

### DB Migration Required

Existing records in the `sir` table:
- `generation_pass = "triage"` → rename to `"scan"`
- `generation_pass = "single"` → rename to `"scan"`
- `generation_pass = "deep"` → unchanged
- `generation_pass = "regenerated"` → unchanged

Add a SQLite schema migration (schema v6) that runs UPDATE statements to rename
these values on first startup after the code change.

### Config Field Rename

| Old config field | New config field |
|-----------------|-----------------|
| `[sir_quality] deep_pass` | `[sir_quality] triage_pass` |
| `[sir_quality] deep_provider` | `[sir_quality] triage_provider` |
| `[sir_quality] deep_model` | `[sir_quality] triage_model` |
| `[sir_quality] deep_endpoint` | `[sir_quality] triage_endpoint` |
| `[sir_quality] deep_api_key_env` | `[sir_quality] triage_api_key_env` |
| `[sir_quality] deep_priority_threshold` | `[sir_quality] triage_priority_threshold` |
| `[sir_quality] deep_confidence_threshold` | `[sir_quality] triage_confidence_threshold` |
| `[sir_quality] deep_max_symbols` | `[sir_quality] triage_max_symbols` |
| `[sir_quality] deep_concurrency` | `[sir_quality] triage_concurrency` |
| New field needed | `[sir_quality] deep_pass` |
| New field needed | `[sir_quality] deep_provider` |
| New field needed | `[sir_quality] deep_model` |
| New field needed | `[sir_quality] deep_endpoint` |
| New field needed | `[sir_quality] deep_api_key_env` |
| New field needed | `[sir_quality] deep_priority_threshold` |
| New field needed | `[sir_quality] deep_confidence_threshold` |
| New field needed | `[sir_quality] deep_max_symbols` |
| New field needed | `[sir_quality] deep_concurrency` |
| New field needed | `[sir_quality] deep_timeout_secs` |

### Correct Three-Pass Config Example

```toml
[inference]
provider = "gemini"
model = "gemini-3.1-flash-lite-preview"
api_key_env = "GEMINI_API_KEY"
concurrency = 16

[sir_quality]
# Pass 2 — triage: enriched context, self-improvement, all or filtered symbols
triage_pass = true
triage_provider = "gemini"
triage_model = "gemini-3.1-flash-lite-preview"
triage_api_key_env = "GEMINI_API_KEY"
triage_priority_threshold = 0.0
triage_confidence_threshold = 1.0
triage_max_symbols = 0
triage_concurrency = 16

# Pass 3 — deep: top-N, best model, CoT for local
deep_pass = true
deep_provider = "openai_compat"
deep_model = "anthropic/claude-sonnet-4.6"
deep_endpoint = "https://openrouter.ai/api/v1"
deep_api_key_env = "OPENROUTER_API_KEY"
deep_priority_threshold = 0.9
deep_confidence_threshold = 0.85
deep_max_symbols = 20
deep_concurrency = 4
deep_timeout_secs = 180
```

---

## What Happened This Session

We ran the first full end-to-end test of Stage 8.8 (SIR Quality Pipeline). The
scan pass works correctly. The triage/deep passes have multiple confirmed bugs that
prevent them from working in normal usage. The `regenerate` subcommand is completely
broken due to a missing tokio runtime and missing tracing init.

### Test Environment

- Repo under test: `tokio-rs/mini-redis` at `/home/rephu/aether-bench/mini-redis`
- Provider: `qwen3_local` model `qwen3.5:4b` via Ollama (RTX 2070, 8GB VRAM)
- Config written explicitly to `.aether/config.toml` (Auto provider bypassed — see Bug 11)

### What Worked

- **Scan pass:** 181/184 on first run, 184/184 after retry. Average confidence 0.938.
  Provider correctly logged as `provider=qwen3_local model=qwen3.5:4b` throughout.
- **CoT deep pass on medium symbols:** Confirmed working when forced. `Entry` improved
  from empty intent → "Internal key-value store unit encapsulating raw data payload
  and optional expiration timestamp to enable automatic lifecycle management by
  background purge tasks", confidence 0.95. Thinking blocks showed genuine reasoning.
- **`generation_pass` column:** Correctly populated.
- **100% coverage:** Achieved on second run after scan retry caught 3 initial failures.
- **Bracket parser:** Did not crash on any CoT output.

### What Is Broken

1. Pass 2 (triage/deep in old naming) reads candidate list from current scan run, not store
2. generation_pass filter excludes "single" pass SIRs from candidates
3. `regenerate --deep` deadlocks — no tokio runtime
4. All subcommands have no tracing — RUST_LOG=debug produces nothing
5. Confidence/priority threshold comparisons use wrong operators (`<` not `<=`)
6. Deep pass timeout hardcoded at 90s — too short for large symbols in CoT mode
7. File rollup summarization failure logged at WARN instead of DEBUG
8. `deep_max_symbols` is only a cap, not a top-N floor guarantee
9. Pass naming wrong throughout — rename required (see above)
10. Three-pass pipeline config not fully implemented
11. Stale default Gemini model (`gemini-flash-latest`)
12. Auto provider picks Gemini when `GEMINI_API_KEY` is set in environment
13. Gemini default concurrency too low for flash-lite 4k RPM

---

## Confirmed Bugs

### Bug 1 — Pass 2/3 Never Fire When SIR Already Exists (CRITICAL)

**File:** `crates/aetherd/src/indexer.rs` — `run_deep_pass()` (to be renamed `run_triage_pass()`)

**Root cause:** The function receives `triage_symbols: &[Symbol]` — symbols processed
by the scan pass in the current run. When all symbols already have SIR, scan processes
0 symbols, passes an empty slice, and the function exits with "0 symbols selected".
Passes 2 and 3 only fire on a brand-new workspace.

**Confirmed by:** Running `--index-once --full --deep` on a workspace with 184 existing
SIRs — Pass 2 logged `symbol_count=0`, logged `0 symbols selected`.

**Fix:** Query candidates from SQLite store directly. Remove the symbols slice parameter.

---

### Bug 2 — generation_pass Filter Excludes "single" Pass SIRs

**File:** `crates/aetherd/src/indexer.rs`

**Root cause:** Filter only allows `"triage"` pass. SIRs stored as `"single"` (pre-8.8
workspaces) are invisible to passes 2 and 3.

**Fix:** After the rename migration, filter should exclude only `"triage"`, `"deep"`,
and `"regenerated"` — allow `"scan"` (and `"single"` for backwards compat until
migration runs).

---

### Bug 3 — `regenerate --deep` Deadlocks (CRITICAL)

**File:** `crates/aetherd/src/main.rs`

Async pipeline called from sync function with no tokio runtime. GPU loads then
hangs forever.

**Fix:** Wrap in tokio runtime using same pattern as LSP at line ~280. Make
`run_regenerate_command()` async.

---

### Bug 4 — Subcommands Have No Tracing Init

**File:** `crates/aetherd/src/main.rs`

`init_tracing_subscriber()` is called after `run_subcommand()` early return.
`RUST_LOG=debug` produces zero output for all subcommands.

**Fix:** Call `init_tracing_subscriber()` with `DEFAULT_LOG_LEVEL` before the
subcommand branch.

---

### Bug 5 — Threshold Operators Wrong

**File:** `crates/aetherd/src/indexer.rs`

```rust
// BEFORE (wrong):
let low_confidence = (baseline_sir.confidence as f64) < quality.deep_confidence_threshold;
let high_priority = priority_score > quality.deep_priority_threshold;

// AFTER:
let low_confidence = (baseline_sir.confidence as f64) <= quality.deep_confidence_threshold;
let high_priority = priority_score >= quality.deep_priority_threshold;
```

Apply same fix in `run_regenerate_command()`.

---

### Bug 6 — Deep Pass Timeout Hardcoded at 90s

**File:** `crates/aether-config/src/lib.rs` + pipeline call sites

Add `deep_timeout_secs: u64` to `SirQualityConfig` (default 180). Use for deep
pass generation calls. The 90s limit caused confirmed timeouts on `Db` and
`Listener::run` in CoT mode.

---

### Bug 7 — File Rollup Summarization Failure Level

**File:** Deep pass context assembly

```
WARN file rollup summarization failed, using deterministic concatenation
```
Should be DEBUG. This is expected when file rollup SIR is missing.

---

### Bug 8 — max_symbols Is Only a Cap, Not a Top-N Floor

**File:** `crates/aetherd/src/indexer.rs`

When threshold selection finds 0 candidates AND `max_symbols > 0`, fall back to
selecting top-N by priority score regardless of threshold. Guarantees passes 2 and
3 always run on something when explicitly configured.

---

### Bug 9 — Pass Naming Wrong Throughout

See the "Pass Naming — Corrected" section at the top of this document. This is a
rename across constants, DB values, config fields, log messages, and documentation.
Requires a SQLite schema migration (v6) to rename existing stored values.

---

### Bug 10 — Three-Pass Pipeline Not Fully Implemented

**Files:** `crates/aether-config/src/lib.rs`, `crates/aetherd/src/indexer.rs`

After the rename in Bug 9, the new `deep_pass` fields (pass 3) need to be added
to `SirQualityConfig` and `run_deep_pass()` needs to be implemented as a separate
function from `run_triage_pass()`. See the config example in the naming section above.

---

### Bug 11 — Stale Default Gemini Model

**File:** `crates/aether-infer/src/lib.rs` ~line 29

```rust
// BEFORE:
const GEMINI_DEFAULT_MODEL: &str = "gemini-flash-latest";

// AFTER (Decision 54):
const GEMINI_DEFAULT_MODEL: &str = "gemini-3.1-flash-lite-preview";
```

---

### Bug 12 — Auto Provider Picks Gemini When GEMINI_API_KEY Is Set

**File:** `crates/aether-infer/src/lib.rs`

**Fix:** Local-first preference order:
1. Ollama reachable (1s timeout GET /api/ps) → `qwen3_local`
2. API key present → `gemini`
3. Neither → `Err(InferError::NoProviderAvailable(...))` with clear message

**NOTE: Do NOT fall back to Mock — Mock was removed in Phase 8.2.**

Add `is_ollama_reachable()` async helper using reqwest 1s timeout.
Apply same fix to `summarize_text_with_config()` Auto branch.
Update `docs/CONFIG.md`.

---

### Bug 13 — Gemini Default Concurrency Too Low

**File:** `crates/aether-config/src/lib.rs`

`gemini-3.1-flash-lite-preview` supports 4000 RPM. Default concurrency of 2
= ~80 symbols/minute. At concurrency 16 = ~640 symbols/minute, well under limit.

Add `const GEMINI_DEFAULT_CONCURRENCY: usize = 16`. Use when Gemini is selected
and concurrency is at default. Document in `docs/CONFIG.md`.

---

## Current DB State (mini-redis)

```
generation_pass | provider    | model               | count
----------------|-------------|---------------------|------
single          | gemini      | gemini-flash-latest  | 80   (accidental run)
triage          | qwen3_local | qwen3.5:4b           | 127  (→ rename to "scan")
deep            | qwen3_local | qwen3.5:4b           | 3
```

---

## Files To Modify

```
crates/aetherd/src/indexer.rs       — Bugs 1, 2, 5, 7, 8, 9, 10
crates/aetherd/src/main.rs          — Bugs 3, 4
crates/aether-infer/src/lib.rs      — Bugs 11, 12
crates/aether-config/src/lib.rs     — Bugs 6, 9, 10, 13
crates/aether-store/src/lib.rs      — Bug 9 (schema v6 migration)
docs/CONFIG.md                      — Bugs 6, 9, 10, 11, 12, 13
```

---

## Scope Guards

- Do NOT change SIR JSON schema (7 fields)
- Do NOT add new InferenceProviderKind variants
- Do NOT add new crates
- Do NOT modify TieredProvider routing
- Do NOT change MCP tool schemas
- Do NOT reference Mock provider — removed in Phase 8.2
- Do NOT change the deep pass CoT logic — confirmed working
- Do NOT use SurrealDB for SIR storage — SIR lives in SQLite (meta.sqlite)
