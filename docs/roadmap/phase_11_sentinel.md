# Phase 11 — The Sentinel

**Status:** Overview spec only. Stage-level specs NOT yet written.
**Decisions:** #136–#151 reserved
**Schema:** core v20 (Stage 11.1.1), v21 (Stage 11.2.1) — v19 is taken by
Phase 10.1b
**Depends on:** Phase 10 complete (fingerprint history, agent hooks)
**Estimated:** 15–22 Codex runs across 9 stages

## Purpose

Two sub-phases. Phase 11.1 (Security Intelligence Layer) makes AETHER's
semantic understanding security-aware: what code trusts, what it exposes,
where tainted data flows. Phase 11.2 (Cross-Layer Truth Detector)
generalizes the detector framework and ships detectors that find
contradictions between what code does and what tests, docs, and commits
claim it does. 11.1 ships first; quality over speed to market. AETHER
complements SAST tools (Semgrep, CodeQL, Snyk) — it does not replace them,
and it never generates exploits (hard constraint).

## The detector framework

Phase 11.1 ships a fixed set of security detectors against an in-tree
detector trait. Phase 11.2 promotes that trait to a public
`aether-detector` crate with registry, scheduling, and plugin loading, and
refactors the 11.1 detectors into `aether-detector-security`. The
framework is the most important piece of code in Phase 11: it makes 11.2 a
sequel rather than a rewrite, and lets future detector families
(compliance, performance, accessibility) ship as crates rather than core
changes.

## Stages

### Phase 11.1 — Security Intelligence Layer

| Stage | Name | Description | Codex runs | Deps |
|-------|------|-------------|-----------|------|
| 11.1.1 | SecurityAnnotation Schema + Security Enrichment | `SecurityAnnotation` nested in SIR layer; security-flavored enrichment prompt; `aetherd enrich-security` CLI; schema bump to v20 | 2–3 | Phase 10 |
| 11.1.2 | Taint Graph + Attack Surface MCP Tools | `TaintFlow` edge type in the dependency graph; extraction during enrichment; `aether_security_attack_surface`, `aether_security_taint` MCP tools | 2–3 | 11.1.1 |
| 11.1.3 | Pattern, Entropy, and CVE Detectors | Non-LLM detectors: secret entropy scanner, dangerous-pattern lints, unsafe-Rust audit, cargo-audit/npm-audit integration; first detectors on the framework | 2–3 | 11.1.1 + framework draft |
| 11.1.4 | Security Drift Detection | Consume fingerprint history; compare prior vs current SecurityAnnotation; alert on negative deltas (auth removed, validation removed, sink added without validation, unsafe expanded) | 1–2 | 11.1.1, 11.1.3 |
| 11.1.5 | Sentinel Alert Surfaces | `aether_sentinel_alerts` / `_explain` / `_acknowledge` MCP tools; `/audit-security`, `/attack-surface`, `/sentinel` slash commands; tray alerts; dashboard `/sentinel` page | 2–3 | 11.1.2–11.1.4 |

### Phase 11.2 — Cross-Layer Truth Detector

| Stage | Name | Description | Codex runs | Deps |
|-------|------|-------------|-----------|------|
| 11.2.1 | Detector Framework Generalization | Public `aether-detector` crate; registry, scheduling, plugin loading; 11.1 detectors move to `aether-detector-security`; alert routing matures; schema v21 | 2–3 | all of 11.1 |
| 11.2.2 | Code↔Test Contradiction Detector | SIR `edge_cases` vs test cases via Phase 6 test-intent infrastructure; alerts on uncovered edge cases and asserted-but-untested behavior | 1–2 | 11.2.1 |
| 11.2.3 | Code↔Docs Contradiction Detector | SIR `purpose` vs README/docstrings/comments; LLM verification pass for ambiguous cases | 1–2 | 11.2.1 |
| 11.2.4 | Code↔Commit Contradiction Detector | Commit message claims vs actual semantic change (fingerprint delta) | 1–2 | 11.2.1 |

## Locked decisions (numbered on commit of stage specs, from #136)

- 5-tier severity model: info / low / medium / high / critical
- BLAKE3 alert deduplication hashing
- `SecurityAnnotation` nested in the SIR layer, not a parallel database
- Taint flows are graph edges of type `TaintFlow`
- Editor-time detection in scope (via Phase Reflex integration); runtime
  application monitoring out of scope (EDR/SIEM territory)
- No exploit generation, ever
- Fuzzing harness generation: strong future-phase candidate building on
  11.1.2 attack-surface work; not in Phase 11 scope

## Prerequisite before stage specs (REVISED)

The original plan stress-tested this design via a Gemini Deep Think prompt
covering 10 problem areas (SecurityAnnotation storage shape, SurrealDB
TaintFlow edge expressibility, detector trait signature, pattern detector
strategy, drift comparison algorithm, alert dedup, fuzzing
forward-compatibility, Reflex integration, evidence/remediation structure,
detector scheduling). Deep Think access is no longer available. Replace
with a dedicated Claude adjudication session working through the same 10
areas against the live repo, producing: (a) a locked Rust trait signature
for the detector framework, and (b) a yes/no on TaintFlow as a SurrealDB
typed edge. Then write stage specs 11.1.1 → 11.2.4.
