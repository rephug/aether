# AETHER Consolidated Docs PR — Import Chat-Only Specs

**Generated:** July 7, 2026
**Purpose:** Single docs-only PR importing all specs written in planning chats (March 31 – April 10) that were never committed to the repo. Verified against repo HEAD (PR #145, commit `aa97383`, March 27 2026).

**Provenance legend:**
- **RECOVERED** — near-verbatim retrieval from the original chat.
- **RECONSTRUCTED** — rebuilt from chat fragments + live repo inspection. Faithful to the original decisions, but review before treating any line as gospel.

---

## Decision number renumbering (AUTHORITATIVE)

The repo already contains **Decision #110** (full SIR inject, `docs/hardening/phase_cc_full_sir_inject_codex_prompt.md`, merged in PR #140). This was discovered during repo verification and invalidates the April 10 renumbering plan, which had Phase WF starting at #110. New allocation — next free number is **#111**:

| Range | Owner | Old (conflicting) claim |
|---|---|---|
| #110 | Full SIR inject (COMMITTED — do not touch) | — |
| #111–#120 | Phase WF (The Companion) | was #110–#119 |
| #121 | /scan zero-Gemini onboarding | was #110 |
| #122–#125 | Phase 9.6 Integrated Agent | was #110–#113 |
| #126–#131 | Phase 10.1b Subprocess CLI Providers | was #110–#115 |
| #132–#135 | Phase 5.6b Agent Integration Refresh | was #36–#39 (ambiguous vs. old register ranges) |
| #136–#151 | Phase 11 Sentinel (reserved) | was #110–#117 |

**Core schema version reservations (also corrected):** Phase 10.1b adds a `provider_type` column to `fingerprint_history`, which requires a core schema bump. Revised: **10.1b → v19, Phase 11.1.1 → v20, Phase 11.2.1 → v21.** (The Phase 11 overview below reflects this.)

All decision numbers inside the spec files in this document are **already renumbered** — copy them as-is.

---

## How to ship this PR

Split each `### FILE:` section below into its own file at the indicated path, then:

```bash
cd /home/rephu/projects/aether
git switch main
git pull --ff-only
git checkout -b docs/import-planning-specs

# Copy the six files into place, then:
git add docs/roadmap/phase_wf_the_companion.md
git add docs/roadmap/phase_9_stage_9_6_integrated_agent.md
git add docs/roadmap/phase_11_sentinel.md
git add docs/roadmap/phase_5_stage_5_6b_agent_integration_refresh.md
git add docs/roadmap/phase_10_stage_10_1b_subprocess_cli_providers.md
git add AETHER_QUICKSTART.md

git commit -m "docs: import Phase WF, 9.6, 11, 5.6b, and 10.1b specs from planning sessions

These specs were written in planning sessions between March 31 and April 10
but never committed. Importing them preserves the work and lets Codex
worktree branches reference them (worktrees branch off main, so referenced
spec files must exist on main before implementation prompts run).

- Phase WF (The Companion): /pre-flight, /review, /explain-area, /impact,
  /context, /health-check slash commands + advisory GitHub Actions CI +
  /scan zero-Gemini onboarding. Decisions #111-#121.
- Phase 9.6 (Integrated Agent): built-in agent orchestrator in the Tauri
  app with Claude Max subprocess / direct API / Ollama modes and
  per-workflow model overrides. Decisions #122-#125.
- Phase 10.1b (Subprocess CLI Providers): BatchProvider trait + Gemini CLI,
  Codex CLI, Claude Code Max subprocess providers with pause-and-resume
  checkpointing. Decisions #126-#131. Core schema v19.
- Phase 5.6b (Agent Integration Refresh): Gemini CLI init-agent template,
  .codex-instructions -> AGENTS.md rename. Decisions #132-#135.
- Phase 11 (The Sentinel): security intelligence layer + cross-layer truth
  detector, overview only. Decisions #136-#151 reserved. Schema v20/v21.
- AETHER_QUICKSTART.md: new-user onboarding guide.

Decision #110 is already taken by the committed full-SIR-inject spec, so
all imported specs were renumbered starting at #111. Stage-level specs for
Phase 11 are not yet written."

git push -u origin docs/import-planning-specs
```

**PR title:** `docs: import Phase WF, 9.6, 11, 5.6b, and 10.1b specs from planning sessions`
**PR body:** use the commit message body verbatim.
After merge: `git switch main && git pull --ff-only && git branch -D docs/import-planning-specs`

---
---

### FILE: `docs/roadmap/phase_wf_the_companion.md`
**Provenance:** RECONSTRUCTED from chat 9a5c0a4b fragments (stage table, decision register, /health-check and /context design sections, WF.1 prompt tail recovered) + repo inspection. Consolidates the original master spec + addendum into one file. The /scan stage (from chat 8a76f1f5) is folded in as WF.0.

```markdown
# Phase WF — The Companion

**Status:** Spec complete, not implemented
**Decisions:** #111–#121
**Depends on:** Phase 5.6 (init-agent), Phase CC (audit slash command infrastructure)
**Schema impact:** AETHER_AGENT_SCHEMA_VERSION bumps (one per stage that changes templates); no core schema change

## Purpose

Phase CC gave AETHER audit and refactor slash commands. Phase WF completes
the daily coding workflow: orientation before work, briefing before
implementation, semantic review after implementation, portable context for
non-MCP AI tools, a daily health check-in, advisory CI review, and a
zero-API-key onboarding path. After Phase WF, `init-agent` generates a
complete AI-coding companion — a new user goes from install to a fully
AETHER-integrated Claude Code workflow without ever needing a Gemini key.

## Stage plan

| Stage | Name | Scope | Codex runs | Dependencies |
|-------|------|-------|------------|--------------|
| WF.0 | Zero-Gemini Onboarding | `/scan` slash command + mock provider fix + `scan_all.sh` | 1 | none |
| WF.1 | Workflow Slash Commands | `/pre-flight`, `/review`, `/explain-area`, `/impact` | 1 | `init-agent` implemented |
| WF.1b | Portable Context + Health Check | `/context`, `/health-check` | 1 | WF.1 |
| WF.2 | CI Integration | GitHub Actions semantic review workflow | 1 | WF.1 |
| WF.3 | Quickstart Guide | `AETHER_QUICKSTART.md` new-user guide | 0 (docs) | WF.1 |

**Execution order:** WF.0 and WF.1 are independent; WF.1b and WF.2 are
independent of each other but both depend on WF.1's template
infrastructure. WF.3 is a docs commit.

## Stage WF.0 — Zero-Gemini Onboarding (`/scan`)

**Problem:** New users need a Gemini flash-lite API key for the initial
scan pass before the Claude Code Max enrichment workflow ($0) can take
over. This is the single biggest onboarding friction point for Max
subscribers.

**Solution:** Index with `--inference-provider mock` (tree-sitter only, no
key), then use a `/scan` slash command in Claude Code to give every symbol
a baseline SIR at Max-subscription cost ($0).

Key mechanics:
- Mock provider confidence drops from 1.0 to **0.1** so
  `aether_audit_candidates` naturally surfaces mock SIRs as the
  highest-priority targets. Mock intent prefix changes to `[MOCK]`.
- `/scan` processes in batches of 10 symbols per reasoning turn, grouping
  symbols by source file (read the file once, produce all its SIRs
  together, then fire all inject calls). ~5–8 sec/symbol effective rate.
- `scripts/scan_all.sh` runs up to 4 parallel `claude -p "/scan <crate>"`
  sessions across crates. 500-symbol codebase ≈ 15 min; 2000 symbols ≈ 1 hr.
- Scan-level SIRs skip reasoning traces and `aether_sir_context` calls —
  coverage over depth; `/enrich` improves quality later.
- Gemini batch remains the recommended path for 10K+ symbol codebases.

New onboarding flow:
```
aetherd --workspace . --index-once --inference-provider mock
./scripts/scan_all.sh
./enrich_all.sh
```
Total cost: $0 (vs $2 Gemini scan). No third-party API keys.

## Stage WF.1 — Workflow Slash Commands

Four commands generated by `init-agent` (extending the existing
`templates/*_cmd.rs` pattern used by /audit, /refactor, /refactor-deep,
/audit-report, /audit-changes):

- **`/pre-flight <task>`** — Pre-implementation briefing. Resolves the
  affected area, pulls SIRs (`aether_get_sir`, `aether_sir_context`),
  coupling and dependencies (`aether_dependencies`, `aether_usage_matrix`),
  health (`aether_health`, `aether_health_hotspots`), and active contracts
  (`aether_contract_list`). Output: what exists, what it means, what
  constraints apply, what could break, recommended approach.
- **`/review`** — Post-implementation semantic review of the current
  branch vs main. Diffs changed symbols, compares against SIR intent
  (`aether_verify_intent`, `aether_get_sir`), checks contract violations
  (`aether_contract_check`), flags semantic drift, and recommends SIR
  updates via `aether_sir_inject` for symbols whose meaning legitimately
  changed.
- **`/explain-area <target>`** — Narrative explanation of an unfamiliar
  area. Synthesizes SIRs, call chains (`aether_call_chain`), and
  dependencies into prose a developer can read in two minutes.
- **`/impact <symbol>`** — Blast radius before change. `aether_blast_radius`
  + `aether_dependencies` + health context, with a risk rating and a list
  of files/symbols that need attention if the change proceeds.

CLAUDE.md template gains a "Slash Commands" section documenting all four
plus the recommended workflow: /explain-area → /pre-flight → implement →
/review.

## Stage WF.1b — Portable Context + Health Check

- **`/context <target>`** — Portable intelligence export. Pulls SIRs, call
  chains, dependencies, and health for an area and assembles a
  self-contained context document formatted for pasting into ANY AI chat
  (Claude web, ChatGPT, Gemini). Plain language — no AETHER jargon, no tool
  names, no internal IDs. Complements the `aether context` CLI (Phase Repo
  R.1): CLI for automation, slash command for interactive refinement.
- **`/health-check`** — Session-start overview. `aether_status` +
  `aether_health` into a concise dashboard: coverage, confidence, health by
  crate, capped at the 5 worst crates and 3 recommendations. Never takes
  more than 30 seconds to read; if everything is healthy it says so and
  gets out of the way.

## Stage WF.2 — CI Integration

GitHub Actions workflow generated by `init-agent` (Claude platform):
- Runs AETHER structural analysis (health scores, blast radius, coupling)
  on PR diffs — no SIR generation, therefore **no API keys required**.
- Posts results as a single sticky PR comment (updated in place, not
  appended).
- Advisory only (`continue-on-error`) — never blocks a merge.
- SIR-powered CI review is opt-in, documented in `docs/CI_SETUP.md`.

## Slash command inventory after Phase WF

`init-agent --platform claude` generates 11 command files:
/audit, /refactor, /refactor-deep, /audit-report, /audit-changes (existing)
+ /scan, /pre-flight, /review, /explain-area, /impact, /context,
/health-check (new; /health-check and /context land in WF.1b).

## Decision register

| # | Decision | Stage |
|---|----------|-------|
| 111 | Slash commands generated by `init-agent` | WF.1 |
| 112 | Slash commands are read-only templates, not dynamic | WF.1 |
| 113 | Four-command workflow: pre-flight → implement → review | WF.1 |
| 114 | Slash command format matches Claude Code convention | WF.1 |
| 115 | `/context` output is self-contained, no AETHER jargon | WF.1b |
| 116 | `/health-check` caps at 5 worst crates + 3 recommendations | WF.1b |
| 117 | CI review is advisory, never blocks | WF.2 |
| 118 | No SIR generation in CI by default | WF.2 |
| 119 | Sticky PR comment pattern | WF.2 |
| 120 | CI workflow generated by `init-agent` (Claude platform) | WF.2 |
| 121 | Mock provider confidence 0.1 + `[MOCK]` prefix so audit_candidates surfaces unscanned symbols; `/scan` batches 10 symbols/turn grouped by file | WF.0 |
```

---
---

### FILE: `docs/roadmap/phase_9_stage_9_6_integrated_agent.md`
**Provenance:** RECONSTRUCTED from chat 8175e220 fragments (all four original decisions + per-workflow config recovered verbatim). The original ~600-line spec had full Rust struct definitions, HTMX layouts, and SSE design; those details should be re-derived at implementation time against the then-current Tauri code. This file preserves every locked decision and the architecture.

```markdown
# Phase 9 — Stage 9.6: Integrated Agent Loop

**Status:** Spec complete, not implemented
**Decisions:** #122–#125 (+ per-workflow overrides folded into #125)
**Depends on:** Stage 9.2 (Configuration UI); benefits from Stage 10.3
(Agent Hooks) but degrades gracefully to the existing 40 MCP tools
**Estimated:** 3–4 Codex runs

## Purpose

Phase 9 as shipped is a dashboard viewer — the core intelligence workflows
(/next, /audit, /enrich) still require Claude Code and terminal
familiarity. Stage 9.6 makes the Tauri app a self-contained product: users
run audit, enrichment, and refactor workflows entirely from the GUI.

## Architecture

A built-in agent orchestrator inside the Tauri app with three execution
modes, selectable in settings:

1. **Claude Max subprocess mode** — spawns
   `claude -p --model <model> --allowedTools "mcp__aether*"`, pipes the
   workflow prompt on stdin, parses stdout for progress. $0 marginal cost
   on a Max subscription.
2. **Direct API mode** — Anthropic, OpenAI, Google, OpenRouter, DeepSeek,
   Ollama, or any OpenAI-compatible endpoint. Pay per token.
3. **Ollama local mode** — localhost, $0, fully offline.

Claude Code remains an optional power interface connecting to the app's
MCP server (HTTP/SSE, port ~9720) — both paths write to the same
SharedState stores and the dashboard reflects results from either.

## Locked decisions

### 122. Dual-mode operation
Built-in agent (calls LLM APIs directly with in-process tool execution)
runs simultaneously alongside the Claude Code bridge (MCP server exposed
for external connection). Both write to the same stores; the dashboard
shows activity from both.

### 123. Separate agent config
The `[agent]` section is independent from `[inference]`. Cheap model for
bulk SIR generation; smart model for agent orchestration.

### 124. Two API formats
Anthropic-native format when the endpoint is `anthropic.com`;
OpenAI-compatible format for everything else (OpenAI, OpenRouter,
DeepSeek, Ollama, etc.).

### 125. In-process tool execution + per-workflow model overrides
The built-in agent calls MCP tool handlers directly against `SharedState`
in Rust — no network hop, no serialization; the MCP transport is only for
external agents. Model selection is per workflow via config, mapping to
`claude -p --model <x>` in Max mode:

```toml
[agent]
mode = "claude_max"          # claude_max | api | ollama
model = "sonnet"             # default

[agent.workflows.audit]
model = "haiku"              # cheap triage

[agent.workflows.enrich]
model = "opus"               # best quality

[agent.workflows.next]
model = "haiku"              # just picking a target
```

## Feature surface

- Workflow templates: audit, enrich, next, refactor, custom prompt
- SSE-streamed activity log in the UI
- Cost tracking with monthly ceilings (API mode only; Max/Ollama are $0)
- Claude Code connection instructions panel with auto-populated
  `claude mcp add aether --url http://localhost:9720/mcp` command
- "Agent Integration" settings button invoking `run_init_agent` internally
  and displaying generated files with copy buttons (closes the
  init-agent-is-CLI-only gap)

## Constraint

SurrealKV holds an exclusive file lock — a separate `aetherd` process
cannot run via stdio while the Tauri app is open. HTTP/SSE MCP is the only
viable transport for coexistence with Claude Code.
```

---
---

### FILE: `docs/roadmap/phase_11_sentinel.md`
**Provenance:** RECONSTRUCTED from chat 3bcf1b16 fragments (full stage tables and framework rationale recovered verbatim; locked decisions from the pending-work inventory). **Change from original:** the Gemini Deep Think stress-test step is removed (no longer have Deep Think access) — replaced by a Claude adjudication session covering the same 10 problem areas before stage specs are written.

```markdown
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
```

---
---

### FILE: `docs/roadmap/phase_5_stage_5_6b_agent_integration_refresh.md`
**Provenance:** RECOVERED — the pass criteria and the complete Codex prompt were retrieved near-verbatim from chat 40148709. Decisions renumbered #36–#39 → **#132–#135**.

```markdown
# Phase 5 — Stage 5.6b: Agent Integration Refresh

**Status:** Spec complete, not implemented
**Decisions:** #132–#135
**Depends on:** Phase 5.6 (Agent Integration Kit), housekeeping PR
(force-track .claude/commands/, seed GEMINI.md stub)
**Estimated:** 1 Codex run

## Purpose

Refresh the Phase 5.6 Agent Integration Kit to (a) add Gemini CLI as a
supported platform in `init-agent`, and (b) rename Codex output from
`.codex-instructions` to `AGENTS.md`, matching the current OpenAI
convention.

## Locked decisions

### 132. Gemini CLI is a first-class init-agent platform
`AgentPlatform::Gemini` generates `GEMINI.md` from a `gemini_md.rs`
template sharing ~95% content with `claude_md.rs`. Only the MCP setup
command (`gemini mcp add aether --transport http --url
http://localhost:9720/mcp`), the slash command directory
(`.gemini/commands/`), and platform naming differ.

### 133. Codex output renames to AGENTS.md
`AgentPlatform::Codex` writes `workspace/AGENTS.md` (was
`.codex-instructions`). Template module renames
`codex_instructions.rs` → `agents_md.rs`,
`render_codex_instructions` → `render_agents_md`.

### 134. Deprecation warning, no auto-delete
If `.codex-instructions` exists after writing AGENTS.md, emit a stderr
warning telling the user it is deprecated and safe to delete after
verifying AGENTS.md. Never delete automatically.

### 135. `--platform all` covers four platforms
Claude, Gemini, Codex, Cursor. `AETHER_AGENT_SCHEMA_VERSION` (currently 3
in `crates/aether-core/src/lib.rs`) increments by exactly 1; all four
templates embed the new version.

## Pass criteria

1. `init-agent --platform gemini` creates GEMINI.md with the Gemini MCP
   setup command and `.gemini/commands/` references
2. `init-agent --platform codex` creates AGENTS.md
3. `init-agent --platform all` creates CLAUDE.md, GEMINI.md, AGENTS.md,
   .cursor/rules, and .agents/skills/aether-context/SKILL.md
4. `.codex-instructions` is NOT generated by any invocation
5. If `.codex-instructions` exists before the run, a deprecation warning
   is printed to stderr
6. `AETHER_AGENT_SCHEMA_VERSION` incremented by exactly 1
7. All four platform templates embed the new schema version
8. Existing 5.6 tests updated to new filenames pass
9. New tests for Gemini rendering and filename migration pass
10. README "Agent Integration" section lists Gemini CLI
11. `cargo fmt --all --check`, `cargo clippy -p aetherd -- -D warnings`,
    `cargo test -p aetherd`, `cargo test -p aether-core` all pass
12. REGRESSION: `--platform claude` output unchanged except schema version

(The exact Codex prompt lives in AETHER_PROMPTS_TO_RUN.md, Prompt 7.)
```

---
---

### FILE: `docs/roadmap/phase_10_stage_10_1b_subprocess_cli_providers.md`
**Provenance:** RECOVERED in large part (scope, decisions, checkpoint format, error classification, integration notes retrieved near-verbatim from chat 40148709). Decisions renumbered #110–#115 → **#126–#131**. **Correction from repo verification:** the HTTP batch pipeline this builds on (prompt hashing, scan/triage/deep passes, per-provider `[batch.providers.*]` config, JSONL build — a.k.a. "10.1a") is **already merged** (`crates/aetherd/src/batch/`, `crates/aether-config/src/batch.rs`). 10.1b is purely additive on it.

```markdown
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
```

---
---

### FILE: `AETHER_QUICKSTART.md` (repo root)
**Provenance:** RECONSTRUCTED (only the opening was recoverable). Grounded in the actual CLI/tooling. Note: the slash-command steps assume WF.0/WF.1 have shipped — commit this file with the docs PR but treat it as forward-looking until those land.

```markdown
# AETHER Quickstart — From Zero to AI-Powered Coding

This guide takes you from a fresh project to a fully AETHER-integrated AI
coding workflow. By the end, your AI coding agent (Claude Code, Codex,
Cursor) will automatically understand your codebase's semantic intent
before writing code and verify its changes afterward.

**Time to first value:** ~15 minutes (index + init-agent).
**Time to full workflow:** ~1 hour (add a scan/enrichment pass).

## 1. Index your codebase

Zero-API-key path (Claude Code Max subscribers):

    aetherd --workspace . --index-once --inference-provider mock

This builds the symbol table and dependency graph with tree-sitter only.
Every symbol gets a `[MOCK]` placeholder SIR at confidence 0.1 — the
`/scan` step below replaces them with real annotations at $0 cost.

Gemini path (faster for 10K+ symbol codebases, ~$2):

    aetherd --workspace . --index-once   # with [inference] configured for Gemini

## 2. Wire up your AI agent

    aetherd --workspace . init-agent --platform claude   # or gemini | codex | cursor | all

This generates CLAUDE.md (behavioral guidance + required actions), a
skill file, and the AETHER slash commands under `.claude/commands/`.
Then connect Claude Code to AETHER's MCP server:

    claude mcp add aether --url http://localhost:9720/mcp

## 3. Get baseline coverage

    ./scripts/scan_all.sh        # parallel /scan across all crates, $0

## 4. Deepen quality where it matters

    /next          # health-guided advisor picks the highest-value target
    /enrich <crate>  # deep SIRs with reasoning traces (Max subscription, $0)

## 5. The daily loop

    /health-check            # 30-second morning check-in
    /explain-area <target>   # orient in unfamiliar code
    /pre-flight <task>       # briefing before you start
    ... implement ...
    /review                  # semantic review vs SIR intent before commit
    /impact <symbol>         # blast radius before risky changes

## Troubleshooting

- **"attempt to write a readonly database" / lock errors:** stop the
  daemon before CLI commands: `pkill -f aetherd && rm -f .aether/graph/LOCK`
- **Semantic search feels stale after bulk enrichment:** run
  `aetherd regenerate --embed-only` (until auto-refresh ships)
- Dashboard lives at http://localhost:9730 when the daemon is running.
```

---

*End of consolidated docs PR document.*
