use super::TemplateContext;

#[derive(Debug, Clone, Copy, Default)]
pub struct ScanCommandTemplate;

impl ScanCommandTemplate {
    pub fn render(_context: &TemplateContext) -> String {
        r#"---
description: Fast baseline SIR coverage for a crate — replaces [MOCK] and low-confidence SIRs, coverage over depth
---

# /scan — zero-Gemini baseline coverage

Usage: `/scan <crate> [batch-size]`

Purpose: give every symbol in `<crate>` that only has a `[MOCK]` placeholder or a
low-confidence (< 0.2) SIR a real, scan-level SIR. This is about COVERAGE, not
perfection: deeper enrichment comes later. `batch-size` (default 100) caps how many
symbols this session processes before stopping.

## Procedure

1. Build the target list from the low-confidence set, not from a ranked window.
   Query `.aether/meta.sqlite` directly, so symbols that were already scanned can never
   crowd the remaining placeholders out of a truncated result:
   `SELECT s.id, s.qualified_name, s.file_path FROM symbols s JOIN sir ON sir.id = s.id
   WHERE s.file_path LIKE 'crates/<crate>/%'
     AND (json_extract(sir.sir_json, '$.confidence') < 0.2 OR sir.sir_json LIKE '%"intent":"[MOCK]%')
   ORDER BY s.file_path, s.qualified_name`
   If SQL is not available to you, call `aether_audit_candidates` with
   `crate_filter: "<crate>"` and a `top_n` well above the crate's symbol count
   (for example 1000), then keep only candidates whose `current_confidence` is below
   0.2 or whose SIR intent starts with `[MOCK]`. Never pass `batch-size` as `top_n`:
   the tool ranks by a composite score in which confidence is only one factor.
2. Take the first `batch-size` targets and group them by source file.
3. Work in batches of 10 symbols per reasoning turn: read each source file ONCE,
   produce the SIRs for all of its symbols together, then fire every
   `aether_sir_inject` call for the batch (intent, behavior, inputs, outputs,
   side_effects, dependencies, error_modes, complexity, confidence, and
   `generation_pass: "scan"`, `provider: "claude-code"`, `model: "<your model>"`).
4. Target confidence 0.7–0.8 for scan-level SIRs. Use `force: true` only when replacing a
   `[MOCK]` placeholder that somehow carries a higher confidence.
5. Stop after `batch-size` symbols and print how many targets remain (rerun `/scan` or let
   `scripts/scan_all.sh` loop).

## Quality guidelines

1. Speed over depth. This is about COVERAGE, not perfection. A 0.75
   confidence SIR that correctly describes the function is far better
   than a [MOCK] placeholder. /enrich will improve it later.
2. Don't skip symbols. Every symbol deserves at least a basic SIR.
   Even trivial getters get a scan-level annotation.
3. Don't call aether_sir_context. That's the expensive cross-symbol
   lookup. Save it for /enrich. Just read the source file.
4. Don't write reasoning traces. They're valuable but eat context.
   Save them for /enrich deep passes.
5. DO flag anything suspicious. If you spot a potential bug while
   scanning, note it in error_modes but don't deep-dive.
6. Batch aggressively. Read 10 source files at once, produce all 10
   SIRs in a single reasoning step, then fire off all 10 inject
   calls. Per-turn overhead is the bottleneck — minimize turns, not
   tokens.
7. Group by file. When multiple symbols share a source file, read the
   file once and analyze all its symbols together. This is the single
   biggest throughput win.
"#
        .to_owned()
    }
}
