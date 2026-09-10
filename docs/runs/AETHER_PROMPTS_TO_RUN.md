# AETHER Prompts To Run — Consolidated Execution Document

**Generated:** July 7, 2026. Verified against repo HEAD (PR #145, `aa97383`, March 27 2026).

**Provenance legend:** RECOVERED = near-verbatim from the original chat. RECONSTRUCTED = rebuilt from chat fragments + live repo inspection; sanity-check the file paths it names before pasting (a source-inspection step is built into each prompt as a safety net).

## Golden rule (from your own playbook)

**Every spec file a prompt references must be committed and pushed to `main` BEFORE the prompt is pasted.** Codex/Claude Code worktrees branch off main. That means the **Consolidated Docs PR (companion document) merges before Prompts 4–8 run.** Prompts 1–3 reference no new spec files and can run immediately.

## Execution order

| # | Prompt | Depends on | Runner | Est. |
|---|--------|-----------|--------|------|
| 1 | Housekeeping: force-track agent commands | nothing | Codex (Netcup) | 15 min |
| — | **Ship the Consolidated Docs PR** (companion doc, not a prompt) | nothing | you (web UI) | 20 min |
| 2 | Combined bug-fix run (4 open bugs) | nothing | Codex | 1 PR |
| 3 | WF.0 — /scan zero-Gemini onboarding | Docs PR merged | Codex | 1 PR |
| 4 | WF.1 — four workflow slash commands | Docs PR merged | Codex | 1 PR |
| 5 | WF.1b — /context + /health-check | WF.1 merged | Codex | 1 PR |
| 6 | WF.2 — GitHub Actions CI integration | WF.1 merged | Codex | 1 PR |
| 7 | Phase 5.6b — Gemini template + AGENTS.md rename | Docs PR + Prompt 1 merged | Codex | 1 PR |
| 8 | Phase 10.1b Run 1 — BatchProvider trait + config + schema v19 | Docs PR merged | Codex | 1 PR |

Prompts 5 and 6 are independent of each other. Prompt 8's Runs 2–3 get drafted after Run 1 merges (per the 10.1b spec). Phase 9.6 and Phase 11 stage prompts are deliberately NOT here — 9.6 needs a fresh planning session against the current Tauri code, and Phase 11 needs the adjudication session (replacing Deep Think) before stage specs exist.

**Build environment note:** prompts include the laptop env block (`CARGO_BUILD_JOBS=2`). On the Netcup server your local Codex config already sets `CARGO_BUILD_JOBS=16` — the block is a fallback, not an override of server settings.

---
---

## PROMPT 1 — Housekeeping: force-track agent commands, MCP config, seed GEMINI.md
**Provenance:** RECOVERED (chat 40148709, April 10). One addition flagged inline: step 9b force-adds `enrich_all.sh` and `scripts/scan_all.sh` if present — they're in the same "exists only on your machine" bucket. Verified still needed: `.claude/` and `.mcp.json` are still gitignored at HEAD; only `.claude/skills/` is force-tracked.

```text
You are working in the repo at https://github.com/rephug/aether on a Netcup
server. This is a PURE GIT HOUSEKEEPING PR — no cargo commands, no Rust code
changes. Your local Codex config already has the correct build environment
for this machine; this prompt does not override it.

PURPOSE:
Force-track Claude Code slash commands and MCP server configuration that are
currently gitignored. Seed a .gemini/commands/ placeholder and skeleton
GEMINI.md so Gemini CLI sessions have project context going forward.

PREFLIGHT:

1) Determine the current main checkout path and verify state:
   MAIN_REPO=$(git rev-parse --show-toplevel)
   echo "Main repo: $MAIN_REPO"
   cd "$MAIN_REPO"
   git status --porcelain
   If the working tree has uncommitted tracked changes, stop and report.
   Untracked files under .claude/commands/ and .mcp.json are EXPECTED —
   those are exactly what we want to force-track.

2) Verify what untracked source files are available to track:
   ls -la "$MAIN_REPO/.claude/commands/" 2>/dev/null || echo "NO .claude/commands/"
   ls -la "$MAIN_REPO/.mcp.json" 2>/dev/null || echo "NO .mcp.json"
   Record which exist. If NEITHER exists, stop and report — this PR cannot
   proceed because there are no source files to track. Report to the user
   that the files only exist on their WSL2 local machine and this PR needs
   to be run from there instead.

3) git fetch origin
   git switch main
   git pull --ff-only

4) Create branch and worktree (using $HOME convention):
   WORKTREE="$HOME/force-track-agent-commands"
   git worktree add -B chore/force-track-agent-commands "$WORKTREE"
   cd "$WORKTREE"

IMPLEMENTATION:

5) Read the current .gitignore and identify exclusions for .claude/,
   .mcp.json, and .gemini/:
   cat .gitignore | grep -E "^\.?(claude|mcp|gemini)"

6) Update .gitignore with targeted negations. Apply ONLY the edits needed
   based on what's actually excluded:
   - If ".claude/" is excluded wholesale, replace that line with:
       .claude/*
       !.claude/commands/
       !.claude/commands/**
       !.claude/skills/
       !.claude/skills/**
   - If ".claude/commands/" is excluded directly, remove that line.
   - If ".mcp.json" is excluded, remove that line.
   - Ensure .gemini/ is NOT excluded (add no exclusion; if one exists,
     add negations for .gemini/commands/ following the .claude pattern).

7) Copy the untracked source files from the main checkout into the worktree
   (worktrees don't share untracked files):
   [ -d "$MAIN_REPO/.claude/commands" ] && mkdir -p .claude && cp -r "$MAIN_REPO/.claude/commands" .claude/
   [ -f "$MAIN_REPO/.mcp.json" ] && cp "$MAIN_REPO/.mcp.json" .

8) Seed the Gemini placeholder:
   mkdir -p .gemini/commands
   touch .gemini/commands/.gitkeep

9) Create a skeleton GEMINI.md at the repo root:

cat > GEMINI.md << 'EOF'
# GEMINI.md — AETHER Project Context for Gemini CLI

This is a stub. CLAUDE.md is the source of truth for AETHER's agent
guidance — read it for project structure, semantic intelligence workflow,
and required actions.

## MCP setup

To connect Gemini CLI to AETHER's semantic intelligence:

    gemini mcp add aether --transport http --url http://localhost:9720/mcp

See `.mcp.json` for the canonical MCP server configuration used by Claude
Code. The same endpoint works for Gemini CLI and Codex CLI.

## Build and validation

See CLAUDE.md for the complete build environment, per-crate cargo commands,
and the three-gate validation sequence (fmt, clippy, per-crate tests).

This GEMINI.md is a stub. Phase 5.6b will replace it with a generated
version from `aetherd init-agent --platform gemini`.
EOF

9b) [ADDITION vs. original prompt] Automation scripts live in the same
    "exists only on the author's machine" bucket. If present in the main
    checkout, copy and stage them too:
    [ -f "$MAIN_REPO/enrich_all.sh" ] && cp "$MAIN_REPO/enrich_all.sh" . && git add enrich_all.sh
    [ -f "$MAIN_REPO/scripts/scan_all.sh" ] && mkdir -p scripts && cp "$MAIN_REPO/scripts/scan_all.sh" scripts/ && git add scripts/scan_all.sh
    If neither exists, skip silently — scan_all.sh ships with WF.0.

10) Stage everything:
    git add .gitignore  # only if modified
    [ -d .claude/commands ] && git add -f .claude/commands/
    [ -f .mcp.json ] && git add -f .mcp.json
    git add .gemini/commands/.gitkeep
    git add GEMINI.md
    git status

11) Sanity check — list every file now tracked under these paths:
    git ls-files .claude/commands/ .mcp.json .gemini/ GEMINI.md
    Report the full list. If any expected file is missing, stop and report.

COMMIT:

12) git commit -m "chore: force-track agent commands, MCP config, and seed GEMINI.md

Previously .claude/commands/, .mcp.json, and .gemini/ were gitignored, which
meant Claude Code slash commands like /enrich and /next only existed on the
author's local machine. This PR force-tracks them so they travel with the repo.

- Force-track .claude/commands/ (Claude Code slash commands)
- Force-track .mcp.json (canonical MCP server config)
- Seed .gemini/commands/.gitkeep placeholder for Phase 5.6b
- Add skeleton GEMINI.md pointing at CLAUDE.md as source of truth
- Track enrichment/scan automation scripts if present
- Update .gitignore with targeted negations to keep these paths tracked

Phase 5.6b will add proper Gemini CLI template support to aetherd init-agent
and rename Codex output from .codex-instructions to AGENTS.md.

Decision Register: no new decisions (housekeeping only)."

PUSH AND PR:

13) git push -u origin chore/force-track-agent-commands

14) Output for the PR:
    Title: chore: force-track agent commands, MCP config, and seed GEMINI.md
    Body: (use the commit message body verbatim)
    Target: main
    URL: https://github.com/rephug/aether/pull/new/chore/force-track-agent-commands

DO NOT:
- Do not run cargo commands (no Rust code changed)
- Do not modify any templates in crates/aetherd/src/templates/
- Do not rename .codex-instructions (that's Phase 5.6b)
- Do not add a Gemini template to init_agent.rs (that's Phase 5.6b)
- Do not touch AETHER_AGENT_SCHEMA_VERSION (that's Phase 5.6b)
```

**After merge:** `git switch main && git pull --ff-only && git worktree remove ~/force-track-agent-commands && git branch -D chore/force-track-agent-commands`

---
---

## PROMPT 2 — Combined bug-fix run (4 open bugs)
**Provenance:** NEW (this prompt was offered in the April 4 chat but never written). Covers open bugs #1, #2, #3, #7 from the bug register. Bugs #4, #5, #6 (readonly store, task-history migration, daemon detection) were already fixed in PR #144 — verified in the commit log. Do not re-fix them.

```text
You are working in the repo at https://github.com/rephug/aether. This is a
combined bug-fix PR covering four small, independent open bugs. No new
features, no schema changes, no new MCP tools.

PREFLIGHT:

1) git status --porcelain (must be clean; stop and report otherwise)
2) git fetch origin && git switch main && git pull --ff-only
3) git worktree add -B fix/open-bug-sweep "$HOME/fix-open-bug-sweep"
4) cd "$HOME/fix-open-bug-sweep"

BUILD ENVIRONMENT (fallback; your local config takes precedence):
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2   # 16 on the Netcup server
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p "$TMPDIR"

SOURCE INSPECTION (do this before writing any code):

5) Read crates/aetherd/src/sir_inject.rs and the MCP tool implementation
   of aether_sir_inject in crates/aether-mcp/ — find where the confidence
   value is serialized into the response JSON.
6) Locate the mock inference provider: grep for
   load_provider_from_env_or_mock in crates/aether-infer/src/loaders.rs
   and follow it to the mock provider implementation. Find where the mock
   SIR's confidence is set.
   NOTE: if this file already sets confidence to 0.1 (a WF.0 branch may
   have landed it), skip Bug C below and say so in the PR body.
7) Read crates/aether-infer/src/providers/qwen_local.rs — find
   normalize_candidate_json / the bracket parser, and how PR #126
   structured the reasoning_trace return type for the Gemini provider
   (crates/aether-infer/src/providers/gemini.rs) so the Ollama change
   matches it exactly.
8) Read the aether_audit_candidates tool implementation in
   crates/aether-mcp/ and find how a SIR's generation_pass /
   already-deep status is stored (see the SQLite pre-filter used by the
   v2 /enrich slash command for the intended semantics).
9) Find where embeddings are generated/updated after SIR changes
   (LanceDB vector store write path) and what aetherd
   "regenerate --embed-only" does, so Bug B reuses that path.

IMPLEMENTATION:

Bug A — f32 confidence display artifacts (e.g. 0.9700000286102295) in
aether_sir_inject responses:
- Round confidence to 4 decimal places at serialization time in the MCP
  response (and any other user-facing surface that prints raw f32
  confidence from sir_inject).
- Do NOT change stored precision — display/response rounding only.
- Test: inject a SIR with confidence 0.97, assert the response string
  contains "0.97" and not "0.9700000".

Bug B — stale embeddings after bulk sir_inject:
- When aether_sir_inject (MCP) or `aether sir inject` (CLI) successfully
  updates a SIR, regenerate the embedding for that symbol using the same
  code path as `regenerate --embed-only`, so semantic search reflects
  enriched SIRs immediately.
- If per-symbol regeneration is expensive to wire cleanly, the acceptable
  fallback is: mark the symbol's embedding stale and add a
  `--stale-only` fast path to `regenerate --embed-only`. Prefer the
  direct per-symbol refresh; explain your choice in the PR body.
- Test: inject a SIR, then verify the vector store row for that symbol
  was rewritten (or marked stale under the fallback).

Bug C — aether_audit_candidates doesn't filter already-deep symbols:
- Add filtering at the MCP tool level so symbols whose SIR is already at
  the deep generation pass are excluded from candidates by default.
- Add an `include_deep: bool` parameter (default false) to preserve the
  old behavior when explicitly requested.
- Test: with one deep and one scan-level SIR in a fixture store, the tool
  returns only the scan-level symbol by default and both with
  include_deep=true.

Bug D — Ollama <think> block content discarded in qwen_local.rs:
- In deep mode, parse and extract <think>...</think> content BEFORE
  feeding the remainder to the bracket parser.
- Return the thinking content as reasoning_trace, matching the return
  type structure PR #126 established for Gemini.
- Fast mode is unchanged: reasoning_trace stays None.
- Test: feed a canned deep-mode response containing a <think> block,
  assert reasoning_trace is captured and the SIR JSON still parses.

VALIDATION (per-crate only; NEVER --workspace):

10) cargo fmt --all --check
11) cargo clippy -p aether-mcp -- -D warnings
12) cargo clippy -p aether-infer -- -D warnings
13) cargo clippy -p aetherd -- -D warnings
14) cargo test -p aether-mcp
15) cargo test -p aether-infer
16) cargo test -p aetherd

COMMIT:

17) git add -A && git commit -m "fix: bug sweep — confidence display, stale embeddings, audit candidate filtering, Ollama reasoning traces

- Round confidence to 4 decimal places in aether_sir_inject responses,
  eliminating IEEE 754 display artifacts (0.9700000286102295 -> 0.97)
- Refresh symbol embeddings after sir_inject so semantic search is no
  longer stale after bulk /enrich sessions
- Filter already-deep symbols from aether_audit_candidates at the MCP
  tool level (new include_deep param, default false), replacing the
  SQLite pre-filter workaround in the /enrich slash command
- Capture Ollama <think> block content as reasoning_trace in deep mode
  (qwen_local.rs), matching the PR #126 structure used by Gemini

No schema changes. No new tools. Display, filtering, and capture fixes
only."

PUSH AND PR:

18) git push -u origin fix/open-bug-sweep
19) Report PR title: "fix: bug sweep — confidence display, stale embeddings,
    audit candidate filtering, Ollama reasoning traces"
    Body: use the commit message body, plus one line per bug stating the
    exact files touched (for AETHER's semantic index).
    URL: https://github.com/rephug/aether/pull/new/fix/open-bug-sweep
```

**After merge:** `git switch main && git pull --ff-only && git worktree remove ~/fix-open-bug-sweep && git branch -D fix/open-bug-sweep`

---
---

## PROMPT 3 — WF.0: /scan zero-Gemini onboarding
**Provenance:** RECOVERED in large part (quality guidelines, scan_all.sh, commit message, PR body from chat 8a76f1f5); implementation steps reconstructed against the current repo. Decision renumbered #110 → **#121**. If Prompt 2 already landed the mock-confidence fix, the prompt below detects that and skips it.

```text
You are working in the repo at https://github.com/rephug/aether. This stage
adds a zero-API-key onboarding path for Claude Code Max subscribers: a
/scan slash command for fast baseline SIR coverage plus a parallel
automation script. Read docs/roadmap/phase_wf_the_companion.md (Stage WF.0,
Decision #121) for the full specification.

PREFLIGHT:

1) git status --porcelain (must be clean)
2) git fetch origin && git switch main && git pull --ff-only
3) git worktree add -B feature/wf0-scan-onboarding "$HOME/wf0-scan"
4) cd "$HOME/wf0-scan"

BUILD ENVIRONMENT (fallback; your local config takes precedence):
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2   # 16 on the Netcup server
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp && mkdir -p "$TMPDIR"

SOURCE INSPECTION:

5) Locate the mock inference provider via
   load_provider_from_env_or_mock in crates/aether-infer/src/loaders.rs.
   Check the confidence it assigns to mock SIRs.
6) Read crates/aetherd/src/init_agent.rs and
   crates/aetherd/src/templates/ — note the existing per-command template
   pattern (audit_cmd.rs, refactor_cmd.rs, ...) and how command files are
   registered as .claude/commands/<name>.md entries.
7) Read .claude/commands/enrich.md if present (force-tracked by the
   housekeeping PR) — /scan mirrors its structure at lower depth.

IMPLEMENTATION:

8) Mock provider fix (SKIP if already at 0.1 from the bug-sweep PR):
   - Confidence for mock SIRs: 1.0 -> 0.1
   - Intent prefix: "[MOCK]" so scan targets are identifiable
   - Update any tests asserting the old values.

9) Create the /scan slash command at .claude/commands/scan.md (for the
   AETHER repo itself) with this content contract:

   Usage: /scan <crate> [batch-size]
   Purpose: fast baseline SIR coverage for symbols that only have [MOCK]
   or low-confidence (<0.2) SIRs. COVERAGE over depth — /enrich handles
   quality later.

   Procedure the command instructs Claude Code to follow:
   a. Query aether_audit_candidates for the crate (mock/low-confidence
      first) — or, if the tool-level filter isn't merged yet, use the
      SQLite pre-filter query documented in enrich.md.
   b. Group candidate symbols by source file.
   c. Process 10 symbols per reasoning turn: read each source file ONCE,
      produce SIRs for all its symbols together, then fire all
      aether_sir_inject calls for the batch.
   d. Target confidence ~0.7-0.8 for scan-level SIRs.

   Quality guidelines (include verbatim in the command file):
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

10) Create scripts/scan_all.sh (chmod +x). Behavior:
    - Args: [BATCH_SIZE=100] [MAX_PARALLEL=4] [crates...]
    - Default crate list: aether-core aether-config aether-parse
      aether-store aether-infer aetherd aether-mcp aether-analysis
      aether-health aether-memory aether-lsp aether-document
      aether-dashboard
    - Preflight: pkill -f aetherd / aether-mcp; verify
      .aether/meta.sqlite exists (error with the index-once command if
      not); count mock/low-confidence SIRs via sqlite3 and print
      before/after totals; exit early with a pointer to enrich_all.sh if
      zero mock SIRs remain.
    - Run up to MAX_PARALLEL background jobs of:
      claude -p "/scan <crate> $BATCH_SIZE"
      with per-crate logs under .aether/scan_logs/ and a master log
      scan_<timestamp>.log.
    - Wait for all jobs; print the after-count and a completion summary.

11) Update the CLAUDE.md template (crates/aetherd/src/templates/claude_md.rs)
    with a "Zero-Gemini Onboarding" subsection documenting the flow:
      aetherd --workspace . --index-once --inference-provider mock
      ./scripts/scan_all.sh
      ./enrich_all.sh
    and noting Gemini batch remains recommended for 10K+ symbol codebases.
    Bump AETHER_AGENT_SCHEMA_VERSION (crates/aether-core/src/lib.rs) by
    exactly 1 from its current value.

12) Update the Decision Register (docs/roadmap/DECISIONS_v4.md or the
    current addendum file) with Decision #121 from the WF spec.

VALIDATION:

13) cargo fmt --all --check
14) cargo clippy -p aether-infer -- -D warnings
15) cargo clippy -p aetherd -- -D warnings
16) cargo clippy -p aether-core -- -D warnings
17) cargo test -p aether-infer
18) cargo test -p aetherd
19) bash -n scripts/scan_all.sh  (syntax check)

COMMIT:

20) git add -A && git add -f .claude/commands/scan.md
    git commit -m "feat(wf): add /scan slash command for zero-Gemini onboarding

- Lower mock provider confidence from 1.0 to 0.1 so audit_candidates
  naturally surfaces mock SIRs as highest-priority targets
- Change mock intent prefix to [MOCK] for easy identification
- Add /scan slash command: fast initial SIR coverage via batched
  processing (10 symbols per reasoning turn, ~5-8 sec/symbol effective)
- Add scan_all.sh: parallel crate scanning (4 concurrent sessions default)
- Update init-agent CLAUDE.md template with zero-Gemini onboarding docs

New onboarding flow for Claude Code Max subscribers:
  aetherd --workspace . --index-once --inference-provider mock
  ./scripts/scan_all.sh
  ./enrich_all.sh

Total cost: \$0 (vs \$2 with Gemini scan). No third-party API keys
required. 500-symbol codebase scans in ~15 min unattended with 4
parallel sessions.

Decision #121."

PUSH AND PR:

21) git push -u origin feature/wf0-scan-onboarding
    PR title: "Phase WF.0: /scan slash command for zero-Gemini onboarding"
    PR body:
    Key changes:
    - Mock provider confidence lowered from 1.0 to 0.1 (surfaces mock
      SIRs in audit_candidates)
    - Mock intent prefix changed to [MOCK] for easy identification
    - /scan slash command: batched processing (10 symbols/turn,
      ~5-8 sec/symbol)
    - scan_all.sh: parallel crate scanning (4 concurrent claude -p
      sessions)
    - init-agent CLAUDE.md template updated with zero-Gemini docs

    Performance: 500-symbol codebase scans in ~15 min, 2000 symbols in
    ~1 hour (4 parallel sessions). Gemini batch remains available for
    10K+ codebases. Decision #121.
```

**After merge:** `git switch main && git pull --ff-only && git worktree remove ~/wf0-scan && git branch -D feature/wf0-scan-onboarding`

---
---

## PROMPT 4 — WF.1: four workflow slash commands
**Provenance:** RECONSTRUCTED. The recovered original assumed a single `slash_commands` module; the repo has since standardized on per-command template modules (`audit_cmd.rs`, `refactor_deep_cmd.rs`, ...) via Phase CC — this prompt follows the current pattern instead. Recovered parts kept: CLAUDE.md section text, test list, commit/PR text, decision numbers (shifted to #111–#114).

```text
You are working in the repo at https://github.com/rephug/aether. This stage
implements Phase WF.1: four AETHER-powered slash commands wiring existing
MCP intelligence into the daily coding workflow. Read
docs/roadmap/phase_wf_the_companion.md (Stage WF.1, Decisions #111-#114)
for the full specification.

PREFLIGHT:

1) git status --porcelain (must be clean)
2) git fetch origin && git switch main && git pull --ff-only
3) git worktree add -B feature/phase-wf-slash-commands "$HOME/phase-wf-slash-commands"
4) cd "$HOME/phase-wf-slash-commands"

BUILD ENVIRONMENT (fallback; your local config takes precedence):
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2   # 16 on the Netcup server
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp && mkdir -p "$TMPDIR"

SOURCE INSPECTION:

5) Read crates/aetherd/src/init_agent.rs — note how the five existing
   commands (audit, refactor, refactor-deep, audit-report, audit-changes)
   are registered as .claude/commands/<name>.md entries with
   relative_path + render function, how --force and platform filtering
   work, and how written files are recorded in the outcome.
6) Read crates/aetherd/src/templates/audit_cmd.rs and
   refactor_deep_cmd.rs as style references for command template modules.
7) Read crates/aetherd/src/templates/claude_md.rs and skill_md.rs.
8) Confirm these MCP tool names exist in crates/aether-mcp (use only
   real names in command content): aether_get_sir, aether_sir_context,
   aether_dependencies, aether_usage_matrix, aether_health,
   aether_health_hotspots, aether_contract_list, aether_contract_check,
   aether_verify_intent, aether_blast_radius, aether_call_chain,
   aether_search, aether_symbol_lookup, aether_sir_inject.

IMPLEMENTATION:

Part 1 — Four new template modules in crates/aetherd/src/templates/,
following the existing *_cmd.rs pattern exactly:

- pre_flight_cmd.rs → renders .claude/commands/pre-flight.md
  /pre-flight <task>: pre-implementation briefing. Steps the command
  instructs the agent through: resolve the affected area from the task
  description (aether_search / aether_symbol_lookup), pull SIRs
  (aether_get_sir, aether_sir_context) for the primary symbols, coupling
  and dependents (aether_dependencies, aether_usage_matrix), health
  context (aether_health, aether_health_hotspots), and active contracts
  (aether_contract_list). Output a briefing: what exists, what it means,
  what constraints apply, what could break, recommended approach.

- review_cmd.rs → renders .claude/commands/review.md
  /review: semantic review of the current branch vs main. git diff
  main...HEAD to enumerate changed symbols; for each, compare the change
  against SIR intent (aether_get_sir, aether_verify_intent); check
  contract violations (aether_contract_check); flag semantic drift; for
  symbols whose meaning legitimately changed, update SIRs via
  aether_sir_inject. Output: violations, drift warnings, SIRs updated.

- explain_area_cmd.rs → renders .claude/commands/explain-area.md
  /explain-area <target>: synthesize SIRs, call chains
  (aether_call_chain), and dependencies into a narrative explanation a
  developer can read in two minutes. No raw JSON dumps — prose.

- impact_cmd.rs → renders .claude/commands/impact.md
  /impact <symbol>: aether_blast_radius + aether_dependencies + health
  context. Output: what depends on the symbol, a risk rating
  (low/medium/high with reasoning), and the files/symbols needing
  attention if the change proceeds.

Part 2 — Wire into init-agent:

1. Register the four modules in templates/mod.rs.
2. Add four .claude/commands/ entries in init_agent.rs following the
   existing pattern (pre-flight.md, review.md, explain-area.md,
   impact.md). Respect --force semantics and record files in the
   outcome. Generate only when platform is claude or all.
3. Update the CLAUDE.md template with a new section after
   "Recommended Actions":

   ## Slash Commands

   This project includes AETHER-powered slash commands for Claude Code:

   - /pre-flight <task> — Get an implementation briefing before starting
     a task. AETHER analyzes the affected area, shows relevant SIRs,
     coupling, and health.
   - /review — Review changes on the current branch against main.
     Compares modifications against SIR intent records to catch semantic
     violations.
   - /explain-area <target> — Understand an unfamiliar area of code.
     Synthesizes SIRs, call chains, and dependencies into a narrative
     explanation.
   - /impact <symbol> — Analyze blast radius before making changes.
     Shows what depends on a symbol and rates the risk of modifying it.

   ### Recommended Workflow

   1. /explain-area — Orient yourself in unfamiliar code
   2. /pre-flight — Get a briefing before starting work
   3. Implement the task
   4. /review — Verify changes against semantic intent

4. Bump AETHER_AGENT_SCHEMA_VERSION (crates/aether-core/src/lib.rs) by
   exactly 1 from its current value.

Part 3 — Update the skill template (skill_md.rs): add the slash commands
to the Orient → Discover → Understand → Modify → Verify workflow section
(/explain-area under Orient, /pre-flight under Understand, /impact under
Modify, /review under Verify).

Part 4 — Also write the four command files into THIS repo's own
.claude/commands/ (AETHER dogfoods its own commands) with content
identical to the templates.

Part 5 — Update the Decision Register with #111-#114 from the WF spec.

TESTS (extend the existing init_agent.rs test patterns):

- init_agent_creates_workflow_commands: --platform claude creates all
  four files
- Platform filter: --platform codex does NOT create .claude/commands/
- --force overwrite and skip-existing behavior for the new files
- CLAUDE.md render contains "Slash Commands" heading and all four names
- Schema version bumped (compare against hardcoded prior value)
- Each command render contains its primary MCP tool name (e.g.
  impact.md contains "aether_blast_radius")

VALIDATION:

cargo fmt --all --check
cargo clippy -p aetherd -- -D warnings
cargo clippy -p aether-core -- -D warnings
cargo test -p aetherd
cargo test -p aether-core
Do NOT run cargo test --workspace (OOM risk).

COMMIT:

git add -A
git add -f .claude/commands/*.md
git commit -m "Add workflow slash commands to init-agent and AETHER repo

- Add /pre-flight, /review, /explain-area, /impact slash commands
- Update init-agent to generate .claude/commands/ for new projects
- Update CLAUDE.md template with Slash Commands section
- Update skill template with slash command references
- Bump AETHER_AGENT_SCHEMA_VERSION
- Force-add .claude/commands/ files to git tracking

Decisions #111-#114."
git push -u origin feature/phase-wf-slash-commands

PR:
Title: "Phase WF.1: Workflow slash commands for daily coding companion"
Body: "Adds four AETHER-powered slash commands (/pre-flight, /review,
/explain-area, /impact) that wire existing MCP intelligence into the
daily coding workflow. Updates init-agent to generate these for new
projects.

Slash commands cover the complete coding lifecycle:
- /pre-flight — pre-implementation briefing from SIRs + health + coupling
- /review — post-implementation semantic review against SIR intent
- /explain-area — narrative explanation of unfamiliar code areas
- /impact — blast radius analysis before making changes

Also updates CLAUDE.md template and skill template to reference the new
commands. Decisions #111-#114 locked."
```

**After merge:** `git switch main && git pull --ff-only && git worktree remove ~/phase-wf-slash-commands && git branch -D feature/phase-wf-slash-commands`

---
---

## PROMPT 5 — WF.1b: /context + /health-check
**Provenance:** RECONSTRUCTED from the recovered design sections (chat 9a5c0a4b). Decisions shifted to #115–#116. Depends on WF.1 (Prompt 4) being merged.

```text
You are working in the repo at https://github.com/rephug/aether. This stage
implements Phase WF.1b: two more slash commands completing the daily
companion. Read docs/roadmap/phase_wf_the_companion.md (Stage WF.1b,
Decisions #115-#116). Follow the exact module/registration/test patterns
established by the WF.1 PR (pre_flight_cmd.rs et al.) — read that code
first; do not invent a new pattern.

PREFLIGHT:

1) git status --porcelain (must be clean)
2) git fetch origin && git switch main && git pull --ff-only
3) git worktree add -B feature/phase-wf1b-context-health "$HOME/phase-wf1b"
4) cd "$HOME/phase-wf1b"

BUILD ENVIRONMENT (fallback; your local config takes precedence):
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp && mkdir -p "$TMPDIR"

SOURCE INSPECTION:

5) Read crates/aetherd/src/templates/pre_flight_cmd.rs (WF.1) as the
   style reference, plus init_agent.rs registration and tests.
6) Check whether Phase Repo R.1's `aether context` CLI command exists
   (grep the CLI Commands enum in crates/aetherd/src/cli.rs) — the
   /context command should reference the same intent but is independent.

IMPLEMENTATION:

context_cmd.rs → .claude/commands/context.md
/context <target>: portable intelligence export.
1. Resolve the target (file, symbol, or concept) via aether_search /
   aether_symbol_lookup.
2. Pull SIRs, call chains, dependencies, and health for the area.
3. Assemble a SELF-CONTAINED context document formatted for pasting into
   any AI chat (Claude web, ChatGPT, Gemini web).
Hard rules for the output (Decision #115): plain language, no AETHER
jargon, no MCP tool names, no internal IDs. The document must be useful
to a reader with zero AETHER knowledge — it is a portable snapshot of
codebase understanding. End by offering to refine or narrow the export.

health_check_cmd.rs → .claude/commands/health-check.md
/health-check: session-start overview (Decision #116).
1. Call aether_status and aether_health (+ aether_health_hotspots).
2. Present a concise dashboard: coverage, confidence, health by crate.
3. Cap output at the 5 worst crates and 3 recommended actions.
4. If everything is healthy, say so in two sentences and stop.
Design principle: never more than 30 seconds to read. This is the
"good morning" check, not a deep analysis.

Wiring: register both modules; add both .claude/commands/ entries in
init_agent.rs; add both commands to the CLAUDE.md template Slash Commands
section (with the recommended workflow gaining "/health-check to start
your session"); write both files into this repo's own .claude/commands/;
bump AETHER_AGENT_SCHEMA_VERSION by exactly 1; update the Decision
Register with #115-#116.

TESTS: mirror the WF.1 test set for the two new commands (creation,
platform filter, --force, CLAUDE.md mention, schema bump, content spot
checks: context.md must NOT contain the string "aether_" in its OUTPUT
FORMAT section; health-check.md must contain "aether_health").

VALIDATION:
cargo fmt --all --check
cargo clippy -p aetherd -- -D warnings
cargo clippy -p aether-core -- -D warnings
cargo test -p aetherd
cargo test -p aether-core

COMMIT + PR:
git add -A && git add -f .claude/commands/context.md .claude/commands/health-check.md
git commit -m "Phase WF.1b: /context and /health-check slash commands

- /context <target>: portable intelligence export — self-contained,
  jargon-free context document for pasting into any AI chat
- /health-check: 30-second session-start overview capped at 5 worst
  crates + 3 recommendations
- Generated by init-agent for claude/all platforms; added to CLAUDE.md
  template and this repo's own commands
- Bump AETHER_AGENT_SCHEMA_VERSION

Decisions #115-#116."
git push -u origin feature/phase-wf1b-context-health
PR title: "Phase WF.1b: /context and /health-check slash commands"
PR body: use the commit body + files touched list.
```

**After merge:** standard cleanup (switch main, pull, worktree remove, branch -D).

---
---

## PROMPT 6 — WF.2: GitHub Actions CI integration
**Provenance:** RECONSTRUCTED from recovered design constraints (advisory-only, sticky comment, no SIR generation, no API keys — Decisions #117–#120). Independent of WF.1b; needs WF.1 merged only for the template-module conventions.

```text
You are working in the repo at https://github.com/rephug/aether. This stage
implements Phase WF.2: an advisory GitHub Actions semantic-review workflow
generated by init-agent. Read docs/roadmap/phase_wf_the_companion.md
(Stage WF.2, Decisions #117-#120).

PREFLIGHT:
1) git status --porcelain (must be clean)
2) git fetch origin && git switch main && git pull --ff-only
3) git worktree add -B feature/phase-wf2-ci "$HOME/phase-wf2-ci"
4) cd "$HOME/phase-wf2-ci"

BUILD ENVIRONMENT (fallback; your local config takes precedence):
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp && mkdir -p "$TMPDIR"

SOURCE INSPECTION:
5) Read init_agent.rs and templates/ (WF.1 conventions).
6) Inventory which aetherd CLI subcommands can produce health /
   blast-radius / coupling output non-interactively and with NO inference
   provider configured (grep cli.rs Commands enum; check health and
   graph/analysis subcommands for a --json or machine-readable flag —
   report what exists; add a --json flag to the health report command if
   none exists, as part of this stage).

IMPLEMENTATION:

1. New template module ci_workflow.rs rendering
   .github/workflows/aether-review.yml with this behavior:
   - Triggers on pull_request.
   - Installs/builds aetherd (document both options in the template:
     prebuilt release download if available, cargo build fallback).
   - Runs indexing with --inference-provider mock (NO API keys, no SIR
     generation — Decision #118), then structural analysis only: health
     scores, blast radius of changed symbols (from the PR diff), coupling.
   - Composes a markdown summary and posts it as a STICKY PR comment —
     find and update the previous AETHER comment (marker:
     <!-- aether-review -->) instead of appending (Decision #119).
   - Entire job is advisory: continue-on-error: true, never blocks merge
     (Decision #117).
2. init-agent wiring: generate the workflow for platform claude or all
   behind a new --ci flag (default OFF so init-agent stays
   zero-surprise); record in outcome (Decision #120).
3. docs/CI_SETUP.md: what the workflow does, how to enable it, and the
   opt-in path for SIR-powered review (bring-your-own API key) as a
   documented manual extension — NOT generated.
4. Generate .github/workflows/aether-review.yml for this repo itself.
5. Bump AETHER_AGENT_SCHEMA_VERSION by exactly 1 (template set changed).
6. Update the Decision Register with #117-#120.

TESTS:
- init_agent --ci creates the workflow file; without --ci it does not
- Rendered YAML parses (basic serde_yaml or equivalent check in tests)
- Workflow contains "continue-on-error" and the sticky marker
- Schema version bumped

VALIDATION:
cargo fmt --all --check
cargo clippy -p aetherd -- -D warnings
cargo clippy -p aether-core -- -D warnings
cargo test -p aetherd
cargo test -p aether-core
Also: python3 -c "import yaml,sys;yaml.safe_load(open('.github/workflows/aether-review.yml'))"
(or yq) to validate the generated YAML.

COMMIT + PR:
git commit -m "Phase WF.2: advisory GitHub Actions semantic review workflow

- init-agent --ci generates .github/workflows/aether-review.yml
- Structural analysis only (health, blast radius, coupling) via mock
  provider — no API keys required, no SIR generation by default
- Sticky PR comment (updated in place via <!-- aether-review --> marker)
- Advisory only: continue-on-error, never blocks merges
- docs/CI_SETUP.md documents setup + opt-in SIR-powered review
- Workflow enabled on the AETHER repo itself

Decisions #117-#120."
PR title: "Phase WF.2: advisory GitHub Actions semantic review"
PR body: commit body + note any CLI --json flags added.
```

**After merge:** standard cleanup. Also commit `AETHER_QUICKSTART.md` updates if WF stages changed any command names (WF.3 is a docs check, no Codex run).

---
---

## PROMPT 7 — Phase 5.6b: Gemini template + AGENTS.md rename
**Provenance:** RECOVERED near-verbatim (chat 40148709), with two edits: decision numbers #36–#39 → **#132–#135**, and clippy narrowed to per-crate (the recovered text had one `--workspace` clippy in pass criteria; per your rules everything below is per-crate). Requires Prompt 1 (housekeeping) and the docs PR merged first.

```text
You are working in the repo at https://github.com/rephug/aether. This stage
refreshes the Phase 5.6 Agent Integration Kit to add Gemini CLI support and
rename Codex output from .codex-instructions to AGENTS.md.

Read docs/roadmap/phase_5_stage_5_6b_agent_integration_refresh.md for the
full specification. Read docs/roadmap/phase_5_stage_5_6_agent_integration_kit.md
for the original 5.6 context.

PREFLIGHT:

1) git status --porcelain (must be clean)
2) git fetch origin && git switch main && git pull --ff-only
3) git worktree add -B feature/phase-5-6b-agent-integration-refresh "$HOME/phase-5-6b-refresh"
4) cd "$HOME/phase-5-6b-refresh"

BUILD ENVIRONMENT (fallback; your local config takes precedence):
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2   # 16 on the Netcup server
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp && mkdir -p "$TMPDIR"

IMPLEMENTATION:

5) Read the current implementation:
   - crates/aetherd/src/init_agent.rs
   - crates/aetherd/src/templates/mod.rs
   - crates/aetherd/src/templates/claude_md.rs
   - crates/aetherd/src/templates/codex_instructions.rs
   - crates/aether-core/src/lib.rs (find AETHER_AGENT_SCHEMA_VERSION)

6) Bump AETHER_AGENT_SCHEMA_VERSION in crates/aether-core by exactly 1
   from its current value.

7) Add AgentPlatform::Gemini variant to the enum in init_agent.rs.
   Update the clap value parser to accept "gemini".

8) Create crates/aetherd/src/templates/gemini_md.rs:
   - Render function takes &TemplateContext, returns String
   - Content mirrors claude_md.rs with these substitutions:
     * Title: "GEMINI.md — AETHER Code Intelligence for Gemini CLI"
     * MCP setup command: "gemini mcp add aether --transport http --url http://localhost:9720/mcp"
     * Slash command directory: ".gemini/commands/" instead of ".claude/commands/"
     * References to "Claude Code" -> "Gemini CLI"
   - All other content (languages, verify commands, tool guidance, schema
     version) identical

9) Rename crates/aetherd/src/templates/codex_instructions.rs to
   agents_md.rs. Update the module's public function name from
   render_codex_instructions to render_agents_md. Content stays ~95%
   identical; update any references to ".codex-instructions" in the
   template body to "AGENTS.md".

10) Update crates/aetherd/src/templates/mod.rs:
    - Add `pub mod gemini_md;`
    - Rename `pub mod codex_instructions;` -> `pub mod agents_md;`

11) Update init_agent.rs orchestration:
    - AgentPlatform::Gemini writes workspace/GEMINI.md using gemini_md::render
    - AgentPlatform::Codex writes workspace/AGENTS.md using agents_md::render
      (previously wrote workspace/.codex-instructions)
    - AgentPlatform::All includes all four platforms (Claude, Gemini,
      Codex, Cursor)

12) Add Codex deprecation warning:
    - In the Codex branch of init_agent.rs, after writing AGENTS.md, check
      if workspace/.codex-instructions exists
    - If yes, emit warning to stderr:
      "warning: .codex-instructions exists at workspace root. This file is
       deprecated in favor of AGENTS.md. You can safely delete
       .codex-instructions after verifying AGENTS.md contains the current
       content."
    - Do NOT delete the old file automatically

13) Update existing tests in init_agent.rs:
    - Replace all references to .codex-instructions with AGENTS.md
    - Update init_agent_creates_all_platform_files to expect GEMINI.md and
      AGENTS.md
    - Update generated_claude_contains_schema_version to also test Gemini

14) Add new tests:
    - gemini_template_contains_mcp_setup_command
    - gemini_template_uses_gemini_commands_directory
    - init_agent_gemini_platform_writes_gemini_md
    - init_agent_all_platform_includes_gemini
    - codex_platform_writes_agents_md_not_codex_instructions
    - init_agent_warns_when_old_codex_instructions_exists
    - schema_version_bumped (compare to hardcoded prior value)

15) Update README.md "Agent Integration" section:
    - Add GEMINI.md and AGENTS.md to the "This creates:" list
    - Add a Supported Platforms table with MCP setup commands for all four
      platforms
    - Replace any reference to .codex-instructions with AGENTS.md

16) Update the Decision Register with decisions #132, #133, #134, #135
    from the spec.

VALIDATION:

17) cargo fmt --all --check
18) cargo clippy -p aetherd -- -D warnings
19) cargo clippy -p aether-core -- -D warnings
20) cargo test -p aetherd
21) cargo test -p aether-core

22) Smoke test: in a temp workspace with a minimal .aether/config.toml,
    run the compiled binary:
    cargo run -p aetherd -- --workspace /tmp/init-agent-smoke init-agent --platform all
    Verify: CLAUDE.md, GEMINI.md, AGENTS.md, .cursor/rules all present.
    Verify: .codex-instructions NOT created.

COMMIT + PR:

23) git add -A
    git commit -m "Phase 5.6b: Gemini CLI init-agent support + AGENTS.md rename

- Add AgentPlatform::Gemini generating GEMINI.md (gemini_md.rs template,
  ~95% shared with CLAUDE.md; Gemini MCP setup command and
  .gemini/commands/ directory)
- Rename Codex output from .codex-instructions to AGENTS.md, matching
  current OpenAI convention (codex_instructions.rs -> agents_md.rs)
- Deprecation warning (no auto-delete) when old .codex-instructions exists
- --platform all now covers Claude, Gemini, Codex, Cursor
- Bump AETHER_AGENT_SCHEMA_VERSION; all templates embed the new version
- README Supported Platforms table with per-platform MCP setup

Decisions #132-#135."
    git push -u origin feature/phase-5-6b-agent-integration-refresh
    PR title: "Phase 5.6b: Gemini CLI init-agent support + AGENTS.md rename"
    PR body: commit body verbatim.
```

**After merge:** standard cleanup, then run `aetherd init-agent --platform gemini` on the repo to replace the GEMINI.md stub from Prompt 1.

---
---

## PROMPT 8 — Phase 10.1b Run 1: BatchProvider trait + config + schema v19
**Provenance:** RECONSTRUCTED from the recovered 10.1b spec (in-scope list, decisions, run split). Grounded against the merged batch pipeline (`crates/aetherd/src/batch/`, `crates/aether-config/src/batch.rs`). Incorporates the schema-migration lesson (list every `check_compatibility` call site). Runs 2 and 3 get drafted after this merges.

```text
You are working in the repo at https://github.com/rephug/aether. This is
Run 1 of Phase 10.1b (Subprocess CLI Providers): the BatchProvider trait,
config schema, the resolve_pass_config provider-subsection bug fix, and
the core schema v19 migration. Concrete subprocess providers (Run 2) and
checkpoint/resume (Run 3) come in later PRs — do NOT implement them here.

Read docs/roadmap/phase_10_stage_10_1b_subprocess_cli_providers.md for the
full specification (Decisions #126-#131; this run locks #126 and the
schema groundwork).

PREFLIGHT:

1) git status --porcelain (must be clean)
2) git fetch origin && git switch main && git pull --ff-only
3) git worktree add -B feature/phase-10-1b-run1-trait-config "$HOME/phase-10-1b-run1"
4) cd "$HOME/phase-10-1b-run1"

BUILD ENVIRONMENT (fallback; your local config takes precedence):
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2   # 16 on the Netcup server
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp && mkdir -p "$TMPDIR"

SOURCE INSPECTION (mandatory before writing code):

5) Read the merged batch pipeline end to end:
   - crates/aetherd/src/batch/mod.rs (BatchPass, PassConfig, for_pass,
     resolve_pass_config, resolve_build_pass_config)
   - crates/aetherd/src/batch/{build,run,ingest}.rs
   - crates/aetherd/src/batch/{gemini,openai,anthropic}.rs (how the HTTP
     providers are dispatched today)
   - crates/aether-config/src/batch.rs (BatchConfig, BatchProviderConfig)
6) Reproduce and characterize the resolve_pass_config provider-subsection
   bug: per the bug register, per-provider [batch.providers.<name>]
   subsection overrides are not resolved correctly for all passes. Write
   a failing test FIRST that demonstrates it, then fix.
7) Find the current core schema version (PRAGMA user_version = 18) and
   the fingerprint_history table definition. Enumerate EVERY
   check_compatibility("core", 18) call site — at minimum:
   - crates/aether-dashboard/src/state.rs
   - crates/aether-mcp/src/state.rs
   plus any test assertions on schema_version.version. Grep exhaustively:
   grep -rn 'check_compatibility\|user_version\|schema_version' crates/ --include='*.rs'
   Report the full list in the PR body.

IMPLEMENTATION:

8) New BatchProvider trait in crates/aether-infer (Decision #126):
   subprocess and HTTP providers are the same first-class type; the
   pipeline dispatches on config lookup, not separate code paths. Design
   the trait around what batch/{build,run}.rs actually needs today
   (submit prompts for a pass, report per-symbol results with
   reasoning_trace, expose a provider_type identifier and a
   health_check()). Adapt the three existing HTTP providers to implement
   it (thin adapters are fine; do not rewrite their internals).

9) Config schema additions in crates/aether-config/src/batch.rs:
   - New [batch] fields: scan_provider, triage_provider, deep_provider
     (String provider names resolving into [batch.providers.*]),
     scan_batch_size (default 100), triage_batch_size (default 20),
     deep_batch_size (default 1), parallel_crate_limit (default 8),
     checkpoint_path (default ".aether/batch/.checkpoint.json")
   - BatchProviderConfig gains: provider_type (http_gemini | http_openai |
     http_anthropic | cli_gemini | cli_codex | cli_claude_max), command
     (Option<String>, CLI binary override), timeout_secs (Option<u64>)
   - All new fields optional/defaulted — existing configs must parse
     unchanged (add a test loading a pre-10.1b config.toml fixture).

10) Fix resolve_pass_config per the failing test from step 6.

11) Core schema v18 -> v19 migration: add provider_type TEXT column to
    fingerprint_history (nullable; backfill existing rows with the
    provider name recorded elsewhere if available, else NULL). Update
    EVERY check_compatibility("core", ...) call site found in step 7 to
    19, and every test asserting the schema version. Missing one of these
    caused 3 CI round-trips on Phase 10.4 — do not repeat that.

12) health_check() startup wiring: aetherd batch commands call
    health_check() on every provider referenced by the configured passes;
    a failure is logged as a warning and only becomes fatal when a pass
    actually uses that provider.

13) Update the Decision Register with #126 (and note #127-#131 reserved
    for Runs 2-3).

TESTS:
- failing-then-fixed resolve_pass_config subsection test
- config backward-compat fixture test
- trait adapter tests for the three HTTP providers (dispatch by name)
- migration test: open a v18 fixture store, verify v19 after migration
  and that fingerprint_history has the provider_type column
- schema version assertions updated everywhere

VALIDATION (per-crate only):
cargo fmt --all --check
cargo clippy -p aether-infer -- -D warnings
cargo clippy -p aether-config -- -D warnings
cargo clippy -p aetherd -- -D warnings
cargo clippy -p aether-store -- -D warnings
cargo test -p aether-infer
cargo test -p aether-config
cargo test -p aetherd
cargo test -p aether-store
cargo test -p aether-mcp
cargo test -p aether-dashboard

COMMIT + PR:
git commit -m "Phase 10.1b Run 1: BatchProvider trait, provider config schema, schema v19

- New BatchProvider trait in aether-infer; HTTP Gemini/OpenAI/Anthropic
  batch providers adapted as first-class implementations (Decision #126)
- [batch] gains per-pass provider selection (scan/triage/deep_provider),
  per-pass batch sizes (100/20/1 defaults), parallel_crate_limit=8,
  checkpoint_path; BatchProviderConfig gains provider_type/command/timeout
- Fix resolve_pass_config provider subsection resolution (bug register)
- Core schema v18 -> v19: fingerprint_history.provider_type column; all
  check_compatibility call sites and schema-version test assertions
  updated (full call-site list in PR body)
- Startup health_check() for configured providers (warn, fatal only on use)

Backward compatible: pre-10.1b configs parse unchanged. Subprocess
provider implementations land in Run 2; checkpoint/resume in Run 3.
Decisions #127-#131 reserved."
git push -u origin feature/phase-10-1b-run1-trait-config
PR title: "Phase 10.1b Run 1: BatchProvider trait + provider config + schema v19"
PR body: commit body + the complete check_compatibility call-site list.
```

**After merge:** standard cleanup, then come back to me for the Run 2 prompt (three concrete subprocess providers + error classification) and Run 3 (checkpoint/resume).

---

## After all of this

Remaining tracks, in the order I'd take them: (1) demo recording — unblocked today, the audit commands already exist; (2) validation pass on the untested surfaces (dashboard :9730, LSP hover, Tauri app, VS Code enhancer) before anything appears on camera; (3) Phase 11 adjudication session (Deep Think replacement) → stage specs; (4) Phase 9.6 planning session against the current Tauri code → implementation prompts; (5) GitNexus-inspired additions (Claude Code hooks first — check overlap with the shipped Phase 10.3 before speccing).

*End of prompts document.*
