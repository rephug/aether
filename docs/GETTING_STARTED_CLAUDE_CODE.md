# Building AETHER with Claude Code

## Overview

This guide covers using Claude Code (terminal or Desktop) for AETHER development. Claude Code reads `CLAUDE.md` automatically for project context, uses skills in `.claude/skills/` for build and workflow guidance, and can explore the codebase interactively.

## Prerequisites

Verify these in your **WSL2 terminal** before starting:

```bash
# Rust toolchain
rustc --version          # 1.7x+
cargo --version
rust-analyzer --version  # needed by rust-analyzer-lsp plugin

# System dependencies
protoc --version         # protobuf-compiler
pdftotext -v             # poppler-utils
dpkg -l | grep liblzma   # liblzma-dev
mold --version           # fast linker
sccache --stats          # compilation cache

# If anything is missing:
sudo apt-get update && sudo apt-get install -y \
  protobuf-compiler poppler-utils liblzma-dev mold
cargo install sccache --locked

# Git access
ssh -T git@github.com    # Should show "Hi rephug!"
```

For Phase 9 (Tauri), also install:

```bash
sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev \
  libayatana-appindicator3-dev librsvg2-dev
cargo install tauri-cli --version "^2"
```

## One-time setup

### Step 1 — Verify repo and build environment

```bash
cd ~/projects/aether

# Set build environment (or source .envrc if using direnv)
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR

# Verify build works
cargo build -p aether-core
```

### Step 2 — Copy Claude Code files into the repo

Copy the following files from the planning output into the AETHER repo:

```bash
# From wherever the files were generated:
cp CLAUDE.md ~/projects/aether/CLAUDE.md
cp -r .claude ~/projects/aether/.claude

# Verify structure
find ~/projects/aether/.claude -name "*.md" | sort
```

Expected:
```
.claude/skills/aether-build/SKILL.md
.claude/skills/stage-workflow/SKILL.md
.claude/skills/validation-gates/SKILL.md
```

### Step 3 — Commit Phase 9 spec files

Phase 9 specs must be in the repo on main before Claude Code can read them. Copy from your planning project:

```bash
cd ~/projects/aether

# Copy Phase 9 specs into docs/roadmap/
cp /path/to/planning/phase_9_beacon.md docs/roadmap/
cp /path/to/planning/phase_9_stage_9_1_tauri_shell.md docs/roadmap/
cp /path/to/planning/phase_9_stage_9_2_configuration_ui.md docs/roadmap/
cp /path/to/planning/phase_9_stage_9_3_onboarding_wizard.md docs/roadmap/
cp /path/to/planning/phase_9_stage_9_4_enhanced_visualizations.md docs/roadmap/
cp /path/to/planning/phase_9_stage_9_5_native_installers.md docs/roadmap/

# Verify they're all there
ls docs/roadmap/phase_9_*.md
```

### Step 4 — Commit and push everything

```bash
git add CLAUDE.md .claude/ docs/roadmap/phase_9_*.md
git commit -m "Add CLAUDE.md, Claude Code skills, and Phase 9 specs for desktop app development"
git push origin main
```

### Step 5 — Create .envrc (optional but recommended)

If you use `direnv`:

```bash
cat > ~/projects/aether/.envrc << 'EOF'
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
EOF

direnv allow
```

### Step 6 — Install Claude Code plugins

Start Claude Code in the project:

```bash
cd ~/projects/aether
claude
```

Install plugins (type each, confirm with `y`):

**Tier 1 — Core workflow:**
```
/plugin install rust-analyzer-lsp@claude-plugins-official
/plugin install context7@claude-plugins-official
/plugin install github@claude-plugins-official
/plugin install security-guidance@claude-plugins-official
/plugin install commit-commands@claude-plugins-official
/plugin install code-review@claude-plugins-official
/plugin install pr-review-toolkit@claude-plugins-official
/plugin install code-simplifier@claude-plugins-official
```

**Tier 2 — Methodology:**
```
/plugin marketplace add obra/superpowers-marketplace
/plugin install superpowers@superpowers-marketplace
```

**Tier 3 — Optional codebase exploration:**
```
/plugin install feature-dev@claude-plugins-official
```

**GitHub plugin authentication:**

If GitHub shows disconnected in `/mcp`, exit Claude Code and run:

```bash
# Replace YOUR_GITHUB_PAT with your PAT (scopes: repo, read:org, workflow)
claude mcp add-json github '{"type":"http","url":"https://api.githubcopilot.com/mcp","headers":{"Authorization":"Bearer YOUR_GITHUB_PAT"}}'
```

### Step 7 — Verify

Back in Claude Code:

```
/plugin    # Check Installed tab — should show 9-10 plugins
/mcp       # Check github and context7 are connected
```

Test skills detection:
```
What skills do you have access to?
```

Should mention `aether-build`, `validation-gates`, and `stage-workflow`.

## Working with Claude Code

### Starting a stage

```bash
cd ~/projects/aether
claude
```

Tell Claude Code what to do:

```
Implement Stage 9.1 per docs/roadmap/phase_9_stage_9_1_tauri_shell.md.
Start by reading the spec and exploring the existing dashboard crate,
then propose your implementation approach.
```

Claude Code will read `CLAUDE.md` automatically, read the spec, explore the codebase, and propose a plan.

### Recommended flow per stage

**Step 1: Explore + Plan (Plan mode)**
```
Read docs/roadmap/phase_9_stage_9_1_tauri_shell.md. Explore the existing
crates — especially aether-dashboard for the HTTP server and aetherd for
SharedState initialization. Propose your implementation approach.
```

**Step 2: Implement (Auto-accept edits)**
```
Implement the plan. Follow the spec exactly. Write tests alongside code.
```

**Step 3: Review**
Click the diff stats to review changes file-by-file.

**Step 4: Validate**
```
Run the validation gates for all modified crates.
```

**Step 5: Commit, push, PR**
```
Commit with a descriptive message, push, and create a PR via the GitHub plugin.
```

### Key differences from Codex workflow

| Codex | Claude Code |
|-------|-------------|
| Paste multi-page prompt, one-shot | "Implement stage X per the spec" — interactive |
| Runs in batch, can't course-correct | Sees errors, iterates, fixes |
| Needs explicit file paths in prompt | Reads CLAUDE.md + explores repo |
| Build settings in every prompt | Build settings in skills, loaded once |
| Worktree management in prompt | You manage branches (or ask Claude Code to) |
| Spec must be committed to main first | Claude Code reads spec from working tree |

### Tips

**One stage per session.** Start fresh for each stage to keep context focused.

**Let Claude Code see errors.** If a build or test fails, don't diagnose it yourself — Claude Code reads the output and can fix it.

**Context7 auto-triggers** when working with library code. No need to say "use context7" — it fires automatically. Be explicit for specific crates: "look up tauri v2 API docs".

**Source your build environment.** If you run cargo commands in a bare terminal (outside Claude Code), `source .envrc` first.

**Commit between stages.** After each stage passes validation, commit and push before starting the next.

**God file awareness.** Several files are already over 2000 lines (`sir_context.rs`, `indexer.rs`, `cli.rs`, `health_score.rs`). Don't add more code to these — put Phase 9 code in `crates/aether-desktop/`.

## Stage execution order for Phase 9

```
Stage 9.1: Tauri Shell + System Tray   (foundation — must be first)
    │
    ├── Stage 9.2: Configuration UI     (needs 9.1 shell)
    │       │
    │       └── Stage 9.3: Onboarding   (needs 9.2 settings components)
    │
    ├── Stage 9.4: Enhanced Viz         (independent of 9.2/9.3)
    │
    └── Stage 9.5: Native Installers    (independent of 9.2/9.3)
```

After 9.1 merges, 9.2/9.4/9.5 can run in parallel (separate sessions).

## Pre-start checklist for Phase 9

Before starting Stage 9.1, verify:

1. `git log --oneline -5` shows all 10.x and R.x merges
2. Phase 9 specs are committed: `ls docs/roadmap/phase_9_stage_9_*.md`
3. `cargo build -p aether-dashboard` compiles cleanly
4. Dashboard runs: `pkill -f aetherd && cargo run -p aetherd -- --workspace . &` then open `http://localhost:3847`
5. No existing `src-tauri/` or Tauri config in the workspace

## Troubleshooting

**Plugin install fails:** `/plugin marketplace add anthropics/claude-plugins-official`

**rust-analyzer-lsp shows no results:** `rustup component add rust-analyzer`

**GitHub plugin can't access repo:** Check PAT scopes (need `repo`, `read:org`, `workflow`). Regenerate at https://github.com/settings/tokens, then `claude mcp remove github` and re-add.

**Cargo builds OOM:** Verify `CARGO_TARGET_DIR` is on native Linux FS. Run `source .envrc`. Check `CARGO_BUILD_JOBS=2`.

**SurrealKV lock errors:** `pkill -f aetherd && rm -f .aether/graph/LOCK`

**Tauri build fails on missing deps:** Run the `apt-get install` command from Prerequisites for WebKit/GTK libraries.
