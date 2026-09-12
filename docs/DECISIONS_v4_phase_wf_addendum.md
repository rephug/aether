# DECISIONS_v4 — Phase WF Addendum

**Date:** September 12, 2026
**Context:** Phase WF (The Companion), imported by the Run 0 docs PR (#150). Decisions are numbered as in `docs/roadmap/phase_wf_the_companion.md`; each stage that ships appends its decision here.

---

## New decisions

### 121. Mock provider confidence 0.1 + `[MOCK]` prefix; `/scan` batches 10 symbols per turn grouped by file

**Date:** 2026-09-12
**Status:** ✅ Implemented (Stage WF.0)

**Context:** New users needed a Gemini key for the initial scan pass before the Claude Code Max enrichment workflow could take over. The spec assumed a key-free mock inference provider already existed with confidence 1.0; the tree had none (`--inference-provider mock` was rejected by the CLI), so WF.0 adds one.

**Decision:** `inference.provider = "mock"` (`InferenceProviderKind::Mock`, `MockInferenceProvider` in `aether-infer`) writes a placeholder SIR per symbol from tree-sitter facts only: intent prefixed `[MOCK]`, empty lists, confidence `MOCK_SIR_CONFIDENCE = 0.1`. The low confidence makes `aether_audit_candidates` rank unscanned symbols first. `/scan <crate> [batch-size]` (`.claude/commands/scan.md`) replaces placeholders 10 symbols per reasoning turn, grouped by source file, targeting confidence 0.7–0.8 with no reasoning traces and no `aether_sir_context` calls; `scripts/scan_all.sh` runs up to 4 `claude -p "/scan <crate>"` sessions in parallel and reports before/after target counts. `AETHER_AGENT_SCHEMA_VERSION` 3 → 4 because the CLAUDE.md template gained the Zero-Gemini Onboarding section.
