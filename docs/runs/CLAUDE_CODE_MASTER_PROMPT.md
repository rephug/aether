# Claude Code Master Prompt — AETHER Run Series

## One-time setup (do this before the first session)

Copy the run kit into the repo so Claude Code can read it:

```bash
cd /home/rephu/projects/aether
mkdir -p docs/runs
cp ~/Downloads/AETHER_RUN_SESSION_CONTEXT.md   docs/runs/
cp ~/Downloads/AETHER_PROMPTS_TO_RUN.md         docs/runs/
cp ~/Downloads/AETHER_DOCS_PR_CONSOLIDATED.md   docs/runs/
cp -r ~/Downloads/specs                         docs/runs/specs
echo "RUN_STATUS.md" >> .gitignore
```

(`docs/runs/` gets committed in Run 0 alongside the specs — the prompts and context files are useful history and AETHER will index them.)

Start each session with a fresh context (`/clear` or a new `claude` session) from `/home/rephu/projects/aether`. One run per session — the runs are big enough that context bloat across runs degrades quality.

---

## The prompt (paste this, changing only the run number on the first line)

```text
RUN NUMBER: <N>

You are Claude Code working in the AETHER repository at
/home/rephu/projects/aether (Rust workspace, 17 crates). You are executing
one run from a sequenced series of PRs. Robert is a non-coding technical
director: he reviews your output and merges PRs via the GitHub web UI. You
do not merge.

STEP 1 — ORIENT (do not skip, do not summarize back to me until step 3)

Read, in this order:
  1. docs/runs/AETHER_RUN_SESSION_CONTEXT.md   (rules, decision numbers,
     schema versions, gate protocol — these are binding)
  2. RUN_STATUS.md at the repo root, if it exists
  3. The section of docs/runs/AETHER_PROMPTS_TO_RUN.md for this run
     (Run 0 uses the "How to ship this PR" section of
     docs/runs/AETHER_DOCS_PR_CONSOLIDATED.md instead, with the six files
     already split out under docs/runs/specs/)
  4. Every spec file that prompt references under docs/roadmap/

Then verify reality against RUN_STATUS.md:
  git fetch origin && git log -1 --format='%h %s' origin/main
  git branch -a | grep -E 'feature|chore|fix|docs'
  git worktree list
  cat crates/aether-core/src/lib.rs | grep AETHER_AGENT_SCHEMA_VERSION
  grep -rhoP 'Decision #\d+' docs/ | grep -oP '\d+' | sort -n | tail -1

STEP 2 — GATE CHECK

Confirm every prerequisite for this run (the "Gate before starting"
column in the session context table) is merged into origin/main. If a
prerequisite PR is still open, STOP and tell me; do not proceed.

If RUN_STATUS.md shows the previous run as pr-open, ask me to confirm it
merged. Only after I confirm: switch main, pull --ff-only, remove that
run's worktree, delete its branch, and mark it merged in RUN_STATUS.md.

If a worktree or branch for THIS run already exists (a previous session
was interrupted), inspect it, tell me what state it's in, and ask whether
to resume or start over. Do not silently delete work.

STEP 3 — PLAN

Tell me in under 200 words: what this run will change, which crates it
touches, the branch and worktree path, the schema/decision numbers it
will use, and anything in the prompt that looks stale compared to the
code you just read. Then proceed WITHOUT waiting unless you found a
conflict that changes scope — in that case wait for my answer.

STEP 4 — EXECUTE THE PROMPT EXACTLY

Follow the run's prompt section step by step, including its SOURCE
INSPECTION steps, tests, and validation gates. Rules that override
anything the prompt might be read as implying:
  - Per-crate cargo only. Never --workspace for test or clippy.
  - Stop the daemon before touching the graph store:
    pkill -f aetherd; pkill -f aether-mcp
  - Every core-schema bump updates every check_compatibility("core", N)
    call site and every schema-version test assertion. Grep, don't recall.
  - Agent schema version bumps by exactly 1 from the CURRENT value.
  - Use only the decision numbers assigned in the session context.
  - If the code disagrees with a prompt's file path or function name,
    the code wins — adapt, and note the discrepancy in the PR body.
  - If a validation gate fails, fix the root cause. Do not loosen the
    gate, skip a test, or add #[allow] to get green.
  - Work in the worktree. Do not modify the main checkout.

Give me a one-line progress update after each major prompt step
(inspection done / implementation done / tests written / gates green).
Do not paste large diffs into chat; I review on GitHub.

STEP 5 — FINISH (the gate protocol)

  1. Confirm all validation gates green in the worktree; paste the exact
     commands and their pass/fail lines.
  2. Commit with the message from the prompt. Push the branch.
  3. Create the PR: gh pr create with the prompt's title and body (body
     must include: files touched, decision numbers, schema versions,
     any deviations from the prompt). If gh is not authenticated, print
     the pull/new URL instead.
  4. Update RUN_STATUS.md: this run -> pr-open with PR number/URL, and
     refresh the schema-version and HEAD lines.
  5. Report to me: run number, PR link, deviations, and one sentence on
     what the next run needs to know. Then stop.

NEVER: merge a PR, force-push, rebase shared branches, delete branches
that aren't this series', commit RUN_STATUS.md, print secrets, or run
aetherd --index-once against the real workspace during a run.
```

---

## Per-run quick reference

| Run | Paste line 1 as | Expect Claude Code to | Approx wall time |
|-----|-----------------|----------------------|------------------|
| 0 | `RUN NUMBER: 0` | copy `docs/runs/specs/*` to `docs/roadmap/` + root, commit `docs/runs/`, open docs PR | 10 min |
| 1 | `RUN NUMBER: 1` | force-track commands/mcp config, seed GEMINI.md, check `.mcp.json` for secrets | 15 min |
| 2 | `RUN NUMBER: 2` | four bug fixes across aether-mcp / aether-infer / aetherd | 1–2 h |
| 3 | `RUN NUMBER: 3` | mock provider 0.1, `/scan`, `scripts/scan_all.sh`, CLAUDE.md template | 1 h |
| 4 | `RUN NUMBER: 4` | four `*_cmd.rs` modules + init-agent wiring + skill template | 1–2 h |
| 5 | `RUN NUMBER: 5` | `/context`, `/health-check` following Run 4's pattern | 1 h |
| 6 | `RUN NUMBER: 6` | CI workflow template, `--ci` flag, `docs/CI_SETUP.md`, may add `--json` to health CLI | 1–2 h |
| 7 | `RUN NUMBER: 7` | Gemini template, AGENTS.md rename, deprecation warning, README table | 1 h |
| 8 | `RUN NUMBER: 8` | BatchProvider trait, config fields, resolve_pass_config fix, schema v19 | 3–5 h |

## If a run goes sideways

- **Claude Code wants to widen scope** ("while I'm here I'll also…"): say no. Each PR maps to one prompt; extra work goes in the PR body as a follow-up note.
- **Gates won't go green after several attempts:** have it push the branch as-is with a `WIP:` PR title and write the failing gate output into `RUN_STATUS.md` notes. Bring me the failure text and I'll write a targeted fix prompt.
- **Context gets long mid-run:** ask Claude Code to write its current state to `RUN_STATUS.md` notes, `/clear`, paste the master prompt again with the same run number — Step 2 will detect the existing worktree and offer to resume.
- **After Run 8 merges:** come back to me for the 10.1b Run 2 and Run 3 prompts; they depend on the trait shape Run 1 actually produced.
