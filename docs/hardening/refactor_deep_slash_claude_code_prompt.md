# Claude Code Prompt — Add /refactor-deep Slash Command to init-agent

## Preamble

```bash
# Preflight
cd /home/rephu/projects/aether
git status --porcelain        # Must be clean
git pull --ff-only

# Build environment
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=16
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR

# Branch + worktree
git worktree add -B feature/refactor-deep-slash /home/rephu/feature/refactor-deep-slash
cd /home/rephu/feature/refactor-deep-slash
```

---

## Context

AETHER's `init-agent` command generates agent configuration files including
Claude Code slash commands in `.claude/commands/`. Phase CC.1 added four
slash commands: `/audit`, `/refactor`, `/audit-report`, `/audit-changes`.

This task adds a fifth: `/refactor-deep` — a full-cycle refactoring workflow
that enhances SIR quality with Opus before refactoring, takes an intent
snapshot, performs the refactoring, and verifies no semantic drift occurred.

This is a key user-facing feature. Refactoring is one of AETHER's core value
propositions. The user types `/refactor-deep crates/aether-mcp/src/lib.rs`
and gets the full intelligence-driven refactoring experience without needing
to know which MCP tools to call or in what order.

The existing `/refactor` command only *analyzes* refactoring opportunities.
`/refactor-deep` goes further: it *performs* the refactoring end-to-end with
AETHER intelligence at every step.

---

## Source Inspection (MANDATORY)

Read these files before writing any code:

1. `crates/aetherd/src/init_agent.rs` — understand `files_for_platform()`,
   `GeneratedFile`, how slash commands are added as entries with paths like
   `.claude/commands/<name>.md`.

2. `crates/aetherd/src/templates/mod.rs` — understand the template system,
   `TemplateContext`, and how slash command content is rendered.

3. Find where the existing slash command templates live — they were added in
   Phase CC.1. Look for files or functions that render the content for
   `audit.md`, `refactor.md`, `audit-report.md`, and `audit-changes.md`.
   They may be in a dedicated file like `templates/slash_commands.rs` or
   inline in `templates/mod.rs`. FIND THE ACTUAL LOCATION before proceeding.

4. Read the existing `/refactor` slash command template to understand the
   format, frontmatter structure, and instruction style.

Report what you found before writing code.

---

## Implementation

### 1. Add the `/refactor-deep` template

Add a new function (following the pattern of the existing slash command
templates) that returns the content for `refactor-deep.md`:

```markdown
---
argument-hint: [file-path]
description: Full refactoring workflow — enhance SIRs with Opus, snapshot intent, refactor, verify
---

Deep refactoring of $ARGUMENTS using AETHER intelligence at every step.
This command enhances SIR quality before refactoring, takes an intent
snapshot, performs the refactoring, and verifies nothing drifted.

## Phase 1: Assess

1. Call `aether_health` scoped to $ARGUMENTS to get structural risk scores.
2. Call `aether_sir_audit_candidates` scoped to $ARGUMENTS with top_n=30.
   Note which symbols have weak SIRs (low confidence, scan/triage-only,
   missing error modes or side effects).
3. Call `aether_suggest_trait_split` if the target is a large trait or impl
   block — use the clustering suggestions to inform split decisions later.

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
```
aetherd refactor-prep --file $ARGUMENTS --top-n 30
```

This creates an intent snapshot capturing the now-enhanced SIRs as the
baseline. Note the snapshot ID from the output — you need it in Phase 5.

If `refactor-prep` is not available as a CLI command, call
`aether_refactor_prep` MCP tool with `{ "scope": "$ARGUMENTS", "top_n": 30 }`.

Save the snapshot ID for Phase 5.

## Phase 4: Refactor

Now perform the actual refactoring. Use AETHER intelligence throughout:

1. Call `aether_sir_context` for the target file to get the full
   multi-layer context (source + SIR + graph + health + coupling).
2. For each split/move/extract decision:
   - Check `aether_dependencies` to understand what breaks if this moves.
   - Check the enhanced SIR for implicit contracts callers depend on.
   - Symbols in the same dependency cycle MUST move together.
   - Boundary leakers (high cross-community edges) are natural split points.
3. After creating new files/modules:
   - Ensure all `pub use` re-exports are in place for backward compatibility.
   - Run `cargo fmt` and `cargo clippy` on affected crates.
   - Run per-crate tests: `cargo test -p <crate>`.
   Do NOT run `cargo test --workspace`.

## Phase 5: Verify

Run via bash:
```
aetherd verify-intent --snapshot <SNAPSHOT_ID_FROM_PHASE_3>
```

Or call `aether_verify_intent` MCP tool with the snapshot ID.

Review the output:
- **All pass:** Refactoring preserved semantic intent. Done.
- **Drift detected:** For each flagged symbol, read the before/after SIR
  comparison. Determine if the drift is intentional (symbol was genuinely
  restructured) or accidental (something broke). Fix accidental drift.
- **Missing symbols:** Symbols that disappeared need to be accounted for —
  either they were intentionally deleted, or they were lost in the move.

## Final checklist

- [ ] All per-crate tests pass
- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy -p <crate> -- -D warnings` passes for each affected crate
- [ ] Intent verification shows no unintended drift
- [ ] New files have appropriate module declarations
- [ ] Re-exports preserve backward compatibility
- [ ] Commit message describes what was split/moved and why
```

### 2. Register in `files_for_platform`

Add the new `GeneratedFile` entry for `.claude/commands/refactor-deep.md`
in the Claude platform block, right next to the existing slash command
entries (audit.md, refactor.md, audit-report.md, audit-changes.md).

### 3. Update tests

Add an assertion in the existing `init_agent_creates_expected_files_for_all_platforms`
test (or equivalent) that `.claude/commands/refactor-deep.md` exists after
running init-agent.

Add a content assertion that the generated file contains key markers:
- `aether_sir_inject` (the SIR enhancement step)
- `aether_refactor_prep` or `refactor-prep` (the snapshot step)
- `aether_verify_intent` or `verify-intent` (the verification step)
- `argument-hint` (frontmatter present)

### 4. Bump AETHER_AGENT_SCHEMA_VERSION

In `crates/aether-core/src/lib.rs` (or wherever this constant lives),
increment `AETHER_AGENT_SCHEMA_VERSION` by 1. This signals to users that
`init-agent --force` should be re-run to pick up new commands.

Update any test that asserts on the exact schema version value.

---

## Scope Guard

- ONLY modify files in `crates/aetherd/src/templates/` and
  `crates/aetherd/src/init_agent.rs`
- ONLY modify `AETHER_AGENT_SCHEMA_VERSION` in `crates/aether-core/`
- Do NOT modify any MCP tools, store code, schema, or other crates
- Do NOT modify existing slash command templates (audit, refactor, etc.)

---

## Validation

```bash
cargo fmt --all --check
cargo clippy -p aetherd -- -D warnings
cargo test -p aetherd
cargo clippy -p aether-core -- -D warnings
cargo test -p aether-core
```

Do NOT run `cargo test --workspace`.

All must pass before committing.

---

## Commit

```bash
git add -A
git commit -m "feat(init-agent): add /refactor-deep slash command for full-cycle AETHER refactoring

- New slash command: /refactor-deep [file-path]
- 5-phase workflow: assess → enhance SIRs → snapshot → refactor → verify
- Uses aether_sir_inject for Opus-quality SIR enhancement before refactoring
- Uses refactor-prep for intent snapshot baseline
- Uses verify-intent for post-refactor semantic drift detection
- Bumps AETHER_AGENT_SCHEMA_VERSION"
```

Do NOT push automatically. Report commit SHA and wait for review.

Push command (after review):
```bash
git push -u origin feature/refactor-deep-slash
```

**PR title:** `feat(init-agent): add /refactor-deep slash command`

**PR body:**
```
Adds /refactor-deep to the set of generated Claude Code slash commands.

Unlike /refactor (which only analyzes refactoring opportunities),
/refactor-deep performs the full refactoring cycle:

1. Assess — health scores + audit candidates + trait split suggestions
2. Enhance — Opus reads source and injects deep SIRs via aether_sir_inject
3. Snapshot — refactor-prep locks the intent baseline
4. Refactor — actual code restructuring using AETHER context
5. Verify — verify-intent confirms no semantic drift

Usage: /refactor-deep crates/aether-mcp/src/lib.rs

Bumps AETHER_AGENT_SCHEMA_VERSION. Run init-agent --force to regenerate.
```

```bash
# After merge:
git switch main
git pull --ff-only
git worktree remove /home/rephu/feature/refactor-deep-slash
git branch -D feature/refactor-deep-slash
```
