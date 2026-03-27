use super::TemplateContext;

#[derive(Debug, Clone, Copy, Default)]
pub struct RefactorDeepCommandTemplate;

impl RefactorDeepCommandTemplate {
    pub fn render(_context: &TemplateContext) -> String {
        r#"---
argument-hint: [file-path]
description: Full refactoring workflow — enhance SIRs with Opus, snapshot intent, refactor, verify
---

Deep refactoring of $ARGUMENTS using AETHER intelligence at every step.
This command enhances SIR quality before refactoring, takes an intent
snapshot, performs the refactoring, and verifies nothing drifted.

## Phase 1: Assess

1. Call `aether_health` to get workspace structural risk scores. Filter the
   returned `critical_symbols`, `bottlenecks`, and `risk_hotspots` down to
   entries whose `file` matches $ARGUMENTS.
2. Call `aether_audit_candidates` with `{ "file_filter": "$ARGUMENTS",
   "top_n": 30 }`. Note which symbols have weak SIRs (low confidence,
   scan/triage-only, missing error modes or side effects).
3. If the file contains a large trait, struct, or impl block, call
   `aether_suggest_trait_split` with that symbol's name plus
   `{ "file": "$ARGUMENTS" }`. Use the clustering suggestions to inform
   split decisions later.

Report: "Found N symbols, M have weak SIRs, K are high-risk."

## Phase 2: Enhance SIRs

For each symbol flagged as weak or high-risk in Phase 1:

1. Call `aether_get_sir` to read the current SIR.
2. Read the actual source code of the symbol.
3. Call `aether_dependencies` to understand callers and callees.
4. Reason carefully about the symbol's true behavior:
   - What is the real intent beyond the obvious?
   - What side effects does it have (locks, IO, state mutation)?
   - What error modes exist, including silent ones?
   - What implicit contracts do callers depend on?
5. Call `aether_sir_inject` with the improved SIR:
   - Set `generation_pass` to `"deep"`
   - Set `confidence` to your actual confidence (0.7-0.95)
   - Set `provider` to `"claude_code"` and `model` to `"opus"`
   - Include `force: true` if overwriting an existing SIR

Track: how many SIRs enhanced, how many were already adequate.

## Phase 3: Snapshot

Run via bash:
```bash
aetherd refactor-prep --file $ARGUMENTS --top-n 30
```

This creates an intent snapshot capturing the now-enhanced SIRs as the
baseline. Note the snapshot ID from the output. You need it in Phase 5.

If `refactor-prep` is not available as a CLI command, call
`aether_refactor_prep` with `{ "file": "$ARGUMENTS", "top_n": 30 }`.

Save the snapshot ID for Phase 5.

## Phase 4: Refactor

Now perform the actual refactoring. Use AETHER intelligence throughout:

1. Call `aether_sir_context` for the highest-risk symbol selected in Phase 1
   using its symbol ID or qualified name. Repeat for any additional symbols
   you need full multi-layer context for (source + SIR + graph + health +
   coupling).
2. For each split, move, or extract decision:
   - Check `aether_dependencies` to understand what breaks if this moves.
   - Check the enhanced SIR for implicit contracts callers depend on.
   - Symbols in the same dependency cycle MUST move together.
   - Boundary leakers (high cross-community edges) are natural split points.
3. After creating new files or modules:
   - Ensure all `pub use` re-exports are in place for backward compatibility.
   - Run `cargo fmt` and `cargo clippy` on affected crates.
   - Run per-crate tests: `cargo test -p <crate>`.
   - Do NOT run `cargo test --workspace`.

## Phase 5: Verify

Run via bash:
```bash
aetherd verify-intent --snapshot <SNAPSHOT_ID_FROM_PHASE_3>
```

Or call `aether_verify_intent` with the snapshot ID.

Review the output:
- **All pass:** Refactoring preserved semantic intent. Done.
- **Drift detected:** For each flagged symbol, read the before/after SIR
  comparison. Determine if the drift is intentional (symbol was genuinely
  restructured) or accidental (something broke). Fix accidental drift.
- **Missing symbols:** Symbols that disappeared need to be accounted for.
  Either they were intentionally deleted, or they were lost in the move.

## Final checklist

- [ ] All per-crate tests pass
- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy -p <crate> -- -D warnings` passes for each affected crate
- [ ] Intent verification shows no unintended drift
- [ ] New files have appropriate module declarations
- [ ] Re-exports preserve backward compatibility
- [ ] Commit message describes what was split or moved and why
"#
        .to_owned()
    }
}
