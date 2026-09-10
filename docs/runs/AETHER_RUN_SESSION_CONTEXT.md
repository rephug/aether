# AETHER Run Series — Session Context

**Read this file first in every Claude Code session for this run series.**
Last verified against repo: September 10, 2026 — HEAD is still PR #145 (`aa97383`, March 27 2026). Nothing has landed since; every run below is still pending.

## What this run series is

Nine sequential PRs that import six planning specs and implement the work they describe. The master prompt (`CLAUDE_CODE_MASTER_PROMPT.md`) drives one run per session. The full text of each implementation prompt lives in `AETHER_PROMPTS_TO_RUN.md`; the spec files live in `specs/` until Run 0 commits them to `docs/roadmap/`.

| Run | What | Prompt source | Gate before starting |
|-----|------|---------------|---------------------|
| 0 | Docs PR: commit the six spec files | `AETHER_DOCS_PR_CONSOLIDATED.md` (git section) | none |
| 1 | Housekeeping: force-track `.claude/commands/`, `.mcp.json`, seed `GEMINI.md` | `AETHER_PROMPTS_TO_RUN.md` → PROMPT 1 | none (independent of Run 0) |
| 2 | Bug sweep: confidence rounding, stale embeddings, audit-candidate filter, Ollama `<think>` | PROMPT 2 | none |
| 3 | WF.0 `/scan` zero-Gemini onboarding | PROMPT 3 | Run 0 merged |
| 4 | WF.1 four workflow slash commands | PROMPT 4 | Run 0 merged |
| 5 | WF.1b `/context` + `/health-check` | PROMPT 5 | Run 4 merged |
| 6 | WF.2 GitHub Actions CI | PROMPT 6 | Run 4 merged |
| 7 | Phase 5.6b Gemini template + AGENTS.md | PROMPT 7 | Runs 0 and 1 merged |
| 8 | Phase 10.1b Run 1: BatchProvider trait + config + schema v19 | PROMPT 8 | Run 0 merged |

Runs 1 and 2 can go while Run 0's PR is awaiting merge. Runs 5 and 6 are independent of each other.

## Repo facts (verified)

- Main checkout: `/home/rephu/projects/aether` (WSL2 native FS; `/home/rephu/` is correct, never treat it as unexpected). Netcup Server 1: `202.61.193.248`, user `rephu`, `CARGO_BUILD_JOBS=16`.
- Worktrees go in `/home/rephu/<branch-short-name>` — NEVER inside `projects/`.
- Build artifacts: `/home/rephu/aether-target`. sccache + mold are installed.
- 17 crates, ~151K LOC Rust, core schema v18, `AETHER_AGENT_SCHEMA_VERSION = 3` (`crates/aether-core/src/lib.rs:16`).
- 40 MCP tools in `crates/aether-mcp` (all names listed in PROMPT 4, step 8).
- `init-agent` already generates five slash commands via per-command template modules: `crates/aetherd/src/templates/{audit,refactor,refactor_deep,audit_report,audit_changes}_cmd.rs`, registered as `.claude/commands/<name>.md` entries in `crates/aetherd/src/init_agent.rs`. **New commands follow this pattern.**
- Batch pipeline already exists: `crates/aetherd/src/batch/{mod,build,run,ingest,gemini,openai,anthropic}.rs` + `crates/aether-config/src/batch.rs` with `BatchPass` scan/triage/deep, `resolve_pass_config`, prompt hashing. Phase 10.1b builds on it — nothing called "10.1a" needs doing.
- Gitignored at HEAD: `.claude/` (except `.claude/skills/` which is force-tracked) and `.mcp.json`. No `.gemini/`, no `GEMINI.md`, no `scripts/scan_all.sh`, no `enrich_all.sh` in the repo.
- Highest committed decision: **#110** (`docs/hardening/phase_cc_full_sir_inject_codex_prompt.md`). The active decision register is `docs/roadmap/DECISIONS_v4.md` plus `DECISIONS_v4_*_addendum.md` files — append new decisions to the newest addendum or create `DECISIONS_v4_phase_wf_addendum.md` following the existing addendum format.
- Bugs #4, #5, #6 from the old bug register (readonly store in health analyzer, task-history migration, per-target daemon detection) were fixed in PR #144. Only bugs #1, #2, #3, #7 remain (all in Run 2).

## Decision numbers and schema versions (AUTHORITATIVE — do not deviate)

| Range | Owner |
|---|---|
| #110 | committed, untouchable |
| #111–#120 | Phase WF (WF.1: 111–114, WF.1b: 115–116, WF.2: 117–120) |
| #121 | WF.0 `/scan` |
| #122–#125 | Phase 9.6 (reserved, not in this series) |
| #126–#131 | Phase 10.1b (Run 8 locks #126; #127–#131 reserved for Runs 2–3) |
| #132–#135 | Phase 5.6b |
| #136–#151 | Phase 11 (reserved) |

Core schema: v18 today → **v19 in Run 8** (`fingerprint_history.provider_type`) → v20/v21 reserved for Phase 11.
Agent schema (`AETHER_AGENT_SCHEMA_VERSION`): bumps by exactly 1 in each of Runs 3, 4, 5, 6, 7 — always relative to the current value, never hardcoded.

## Hard rules (violations have cost real CI round-trips before)

1. **Per-crate cargo only.** Never `cargo test --workspace` or `cargo clippy --workspace` — OOM risk. `cargo fmt --all --check` is the only workspace-wide command allowed.
2. **Core schema bumps must update every `check_compatibility("core", N)` site** — at minimum `crates/aether-dashboard/src/state.rs` and `crates/aether-mcp/src/state.rs` — plus every test asserting `schema_version.version`. Grep exhaustively; list the sites in the PR body.
3. **SurrealKV holds a process-exclusive lock.** Before any command that opens the graph store: `pkill -f aetherd; pkill -f aether-mcp`. If the AETHER MCP server is attached to this Claude Code session, it already holds the lock — tests use temp fixture stores so this is fine, but never run `aetherd --index-once` against the real workspace mid-run.
4. **Spec files referenced by a prompt must be on `main` before the run starts.** Worktrees branch off main.
5. **Worktree discipline:** preflight `git status --porcelain` clean → `git pull --ff-only` → `git worktree add -B <branch> /home/rephu/<short>` → work there → push → PR. Cleanup happens at the start of the *next* run, after Robert confirms the merge.
6. **PR descriptions are indexed as semantic data later** — always a descriptive title and a body that lists files touched and decision numbers.
7. **Never store or print secrets.** `.mcp.json` is force-tracked in Run 1 — verify it contains no API keys before staging (it should reference env vars only). If it contains a literal key, stop and report.

## Gate protocol (how a run ends)

A run ends when the PR is created. Claude Code does NOT merge. Sequence:
1. Push the branch.
2. Create the PR with `gh pr create --title ... --body ...` if `gh` is authenticated; otherwise print the `https://github.com/rephug/aether/pull/new/<branch>` URL.
3. Update `RUN_STATUS.md` (see below).
4. Stop and tell Robert: run number, PR URL, anything that deviated from the prompt, anything the next run needs to know.

Robert merges via the GitHub web UI and starts the next session.

## RUN_STATUS.md (state that survives between sessions)

Claude Code maintains `RUN_STATUS.md` at the repo root (gitignored — add it to `.gitignore` in Run 1 if not present; never commit it). Format:

```
# AETHER Run Status
| Run | Branch | Status | PR | Notes |
|-----|--------|--------|----|-------|
| 0 | docs/import-planning-specs | pr-open | #146 | — |
| 1 | chore/force-track-agent-commands | merged | #147 | .mcp.json had no keys |
| 2 | fix/open-bug-sweep | in-progress | — | Bug B took fallback path |
...
Current AETHER_AGENT_SCHEMA_VERSION: 3
Current core schema: 18
Last verified main HEAD: <sha>
```

At session start Claude Code reads this file, confirms with `git log -1` and `git branch -a` that the recorded state matches reality, and asks Robert to confirm any run marked `pr-open` has actually merged before doing cleanup.

## Known gotchas

- `--index-once` is the correct flag for one-shot indexing; `--inference-provider mock` makes it key-free.
- Dashboard default port 9730; MCP HTTP transport 9720.
- The `resolve_pass_config` provider-subsection bug (Run 8) has no existing test — write the failing test first.
- Bug B (stale embeddings, Run 2) has a sanctioned fallback (mark-stale + `--stale-only`); the prompt asks for the reasoning in the PR body.
- Reconstructed prompts (3, 4, 5, 6, 8) name file paths verified in September 2026, but each contains a mandatory SOURCE INSPECTION step — trust the code over the prompt if they disagree, and report the discrepancy.
- Codex prompts said "Do NOT run cargo test --workspace" — the same applies to Claude Code.
