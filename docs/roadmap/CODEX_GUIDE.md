# CODEX_GUIDE.md — How to Use Codex with AETHER

## What is this?

This guide explains how to use [OpenAI Codex CLI](https://developers.openai.com/codex/cli) to implement AETHER stages. Each stage doc in `docs/roadmap/` contains copy-paste-ready prompts. This guide covers the workflow, patterns, and gotchas.

---

## Quick Start (30-second version)

```bash
# 1. Open WSL terminal and navigate to the repo
cd /home/rephu/projects/aether

# 2. Start Codex with full permissions (network access needed for cargo)
codex --full-auto

# 3. Paste the prompt from the stage doc
#    e.g., the "Exact Codex prompt(s)" block from phase_4_stage_4_1_lancedb_vector_backend.md

# 4. Codex creates the branch, worktree, implements, tests, and commits
# 5. You review and push
git -C ../aether-phase4-stage4-1-lancedb push -u origin feature/phase4-stage4-1-lancedb
```

---

## WSL2 Build Environment (CRITICAL)

AETHER is developed in WSL2 on Windows. The following settings **must** be applied to prevent OOM crashes during compilation of heavy dependencies (LanceDB, CozoDB, gix).

### .wslconfig (Windows side)

Create or edit `C:\Users\rephu\.wslconfig`:

```ini
[wsl2]
memory=8GB
swap=8GB
```

Restart WSL after changes: `wsl --shutdown` from PowerShell, then `wsl`.

### System dependencies (install once)

```bash
sudo apt-get update && sudo apt-get install -y protobuf-compiler liblzma-dev
```

### Required environment for ALL cargo commands

Every Codex prompt in this project **must** include these build settings:

```
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.
- Do NOT use paths under /mnt/ for build targets — the Windows filesystem (9P) is too slow.
```

**Why each setting matters:**

| Setting | Purpose |
|---------|---------|
| `CARGO_TARGET_DIR=/home/rephu/aether-target` | Build artifacts on fast native Linux FS, not RAM or slow Windows drive |
| `CARGO_BUILD_JOBS=2` | Limits peak RAM during compilation to prevent OOM |
| `PROTOC=$(which protoc)` | LanceDB needs protoc for protobuf compilation |

### Optional: Codex Skill for build settings

Instead of prepending build settings to every prompt, create an AETHER build skill:

```bash
mkdir -p /home/rephu/projects/aether/.agents/skills/aether-build
```

Create `.agents/skills/aether-build/SKILL.md`:

```markdown
---
name: aether-build
description: "Build settings for AETHER project in WSL2. Use for any cargo build, test, clippy, or fmt commands."
---

# AETHER Build Configuration

Always set these before running any cargo command:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)

NEVER use /tmp/ for build artifacts (RAM-backed in WSL2).
NEVER use /mnt/ paths for build targets (slow 9P filesystem).

System dependencies required: protobuf-compiler, liblzma-dev
```

Then reference it in prompts with `$aether-build` or let Codex pick it up implicitly.

---

## The Workflow Pattern

Every stage follows the same 7-step pattern. Codex handles steps 1-6; you handle step 7.

### 1. Preflight
Codex checks that the working tree is clean and main is up to date. If not, it stops and tells you what to fix. **Never skip this** — dirty trees cause merge nightmares.

### 2. Branch
Codex creates a feature branch off `main` with a predictable name:
```
feature/phase4-stage4-1-lancedb
feature/phase4-stage4-2-tracing
feature/phase4-stage4-3-gix
...
```

### 3. Worktree
Codex creates a git worktree at `../aether-<stage-name>` (relative to `/home/rephu/projects/aether`) so your main checkout stays untouched. This means you can have multiple stages in progress simultaneously.

### 4. Implement
Codex reads the stage doc, cross-references the codebase, and writes code. It only modifies files within the stage's scope.

### 5. Test
Codex runs the three validation gates with the required build environment:
```bash
CARGO_TARGET_DIR=/home/rephu/aether-target CARGO_BUILD_JOBS=2 PROTOC=$(which protoc) cargo fmt --all --check
CARGO_TARGET_DIR=/home/rephu/aether-target CARGO_BUILD_JOBS=2 PROTOC=$(which protoc) cargo clippy --workspace -- -D warnings
CARGO_TARGET_DIR=/home/rephu/aether-target CARGO_BUILD_JOBS=2 PROTOC=$(which protoc) cargo test --workspace
```

### 6. Commit
Codex commits with the specified message from the stage doc.

### 7. Review + Push (you)
You review the diff, then push:
```bash
git -C ../aether-<stage-name> push -u origin feature/<branch-name>
```

---

## Two-Pass Mode (Recommended for Complex Stages)

For stages with open design questions (4.1 LanceDB, 4.5 Graph Storage), use two passes:

### Pass 1: Plan Only
Add this to the beginning of your Codex session:

```
Read docs/roadmap/phase_4_stage_4_1_lancedb_vector_backend.md and produce a 
decision-complete implementation plan. Do not modify any files yet. Plan only.
Include:
- Exact files to create/modify
- API shapes and types
- Open questions with recommended defaults
- Risk assessment
End with a <proposed_plan> block.
```

Codex outputs a plan. You review it, ask questions, adjust.

### Pass 2: Implement
```
Implement the approved plan. Follow the Exact Codex prompt from the stage doc.
```

**Why two passes?** Single-pass works for straightforward stages (4.2 tracing, 4.3 gix). But stages with new dependencies, trait design, or migration logic benefit from reviewing the plan before Codex starts writing code.

---

## Stage Execution Order

### Independent (run in any order, parallelizable)
These stages don't depend on each other:
- **4.1 LanceDB** — vector backend
- **4.2 tracing** — structured logging
- **4.3 gix** — native git

You can have three Codex sessions running simultaneously, each in its own worktree.

### Sequential (must run in order)
- **4.4 Dependency Extraction** — requires stages 1-3 to be merged first
- **4.5 Graph Storage** — requires 4.4 (needs edge data)
- **4.6 SIR Hierarchy** — requires at least 4.1 (uses LanceDB for embeddings)

### Recommended execution plan
```
Week 1:  4.1 LanceDB + 4.2 tracing + 4.3 gix  (parallel)
Week 2:  Merge all three, then 4.4 dependency extraction
Week 3:  4.5 graph storage
Week 4:  4.6 SIR hierarchy
```

---

## Codex Configuration Tips

### AGENTS.md (repo root)
Codex reads `AGENTS.md` in your repo root as persistent context. Add this:

```markdown
# AETHER Codex Context

## Repo structure
Rust workspace with 9 crates under `crates/`.
VS Code extension under `vscode-extension/`.

## Build environment (WSL2)
All cargo commands MUST use:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
Do NOT use /tmp/ or /mnt/ paths for build targets.

## Validation gates (always run these before committing)
- cargo fmt --all --check
- cargo clippy --workspace -- -D warnings  
- cargo test --workspace

## Key patterns
- Store trait in crates/aether-store abstracts all persistence
- InferenceProvider and EmbeddingProvider traits abstract AI backends
- Config loaded from .aether/config.toml via crates/aether-config
- MCP tools are in crates/aether-mcp, handler methods on AetherMcpRouter
- SIR is canonical JSON, sorted keys, BLAKE3 hashed

## Do NOT
- Create new crates without explicit instruction
- Modify VS Code extension unless the stage doc says to
- Use unwrap() in library code (use anyhow/thiserror)
- Add dependencies not listed in the stage doc
- Use /tmp/ for CARGO_TARGET_DIR (RAM-backed, causes OOM)
```

### Codex permissions
For full-auto mode with network access (needed for cargo crate downloads):
```bash
codex --full-auto
```

---

## Handling Common Issues

### "Working tree is dirty"
```bash
# Check what's dirty
git status
# Stash or commit your changes
git stash
# Then re-run the Codex prompt
```

### "Branch already exists"
A previous run created the branch. Either:
```bash
# Delete and retry
git branch -D feature/phase4-stage4-1-lancedb
git worktree remove ../aether-phase4-stage4-1-lancedb --force
git worktree prune
```
Or tell Codex to reuse it:
```
The branch feature/phase4-stage4-1-lancedb already exists. 
Switch to the existing worktree at ../aether-phase4-stage4-1-lancedb and continue.
```

### "Cargo test fails"
If tests fail after implementation:
1. Read the error output
2. Tell Codex: `The test xyz_test failed with this error: [paste error]. Fix it.`
3. Codex will iterate until tests pass

### "LanceDB build fails" (Stage 4.1)
LanceDB has a transitive dependency on `lzma-sys`. If the build fails:
```bash
sudo apt-get install liblzma-dev
```

### "WSL2 OOM crash" (Catastrophic failure / E_UNEXPECTED)
WSL ran out of memory. From PowerShell:
```powershell
wsl --shutdown
wsl
```
Then verify `.wslconfig` has `memory=8GB` and `swap=8GB`. Restart Codex — cached build artifacts in `/home/rephu/aether-target` mean it won't start from scratch.

### "CozoDB build fails" (Stage 4.5)
CozoDB with the SQLite backend needs `libsqlite3`. If the build fails:
```bash
sudo apt-get install libsqlite3-dev
```
If it still fails, the stage doc has a fallback: SQLite-only edge resolution (recursive CTEs, no CozoDB).

---

## Prompt Templates

### Standard stage execution
```
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts.

You are working in the repo root of https://github.com/rephug/aether.
Read docs/roadmap/<stage_file>.md for the full specification.
Follow the "Exact Codex prompt(s)" section exactly.
```

### Bug fix after a stage
```
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)

In the worktree at ../aether-<stage>, there is a bug:
[describe the bug]
Fix it while keeping all pass criteria from docs/roadmap/<stage_file>.md.
Run cargo fmt/clippy/test after fixing.
Amend the last commit.
```

### Adding a test to an existing stage
```
In the worktree at ../aether-<stage>, add a test that verifies:
[describe what to test]
The test should be in crates/<crate>/tests/ or as a #[test] in the relevant module.
Run cargo test --workspace to verify.
Amend the last commit.
```

### Cross-referencing multiple stage docs
```
Read these files for context before implementing:
- docs/roadmap/phase_4_stage_4_4_dependency_extraction.md
- docs/roadmap/phase_4_stage_4_5_graph_storage.md
- crates/aether-store/src/lib.rs
- crates/aether-parse/src/lib.rs
Then implement Stage 4.4 only. Do not implement Stage 4.5 yet.
```

---

## After All Stages: Merge Workflow

Once all Phase 4 stages are implemented and pushed:

```bash
# Create PR for each stage (or use GitHub CLI)
gh pr create --base main --head feature/phase4-stage4-1-lancedb \
  --title "Phase 4.1: LanceDB vector backend" \
  --body "Replaces brute-force SQLite embeddings with LanceDB ANN search."

# Merge in dependency order:
# 1. 4.1, 4.2, 4.3 (independent, merge in any order)
# 2. 4.4 (after 1-3 are merged)
# 3. 4.5 (after 4.4)
# 4. 4.6 (after 4.5 or at least 4.1)

# After merging, clean up worktrees
git worktree remove ../aether-phase4-stage4-1-lancedb
git worktree remove ../aether-phase4-stage4-2-tracing
# ... etc
git worktree prune
```

---

## File Inventory

| File | Purpose |
|------|---------|
| `docs/roadmap/00_overview_v2.md` | Master roadmap with status table and gap analysis |
| `docs/roadmap/phase_4_architect.md` | Phase 4 overview and rollup prompt |
| `docs/roadmap/phase_4_stage_4_1_lancedb_vector_backend.md` | LanceDB migration spec |
| `docs/roadmap/phase_4_stage_4_2_structured_logging.md` | tracing migration spec |
| `docs/roadmap/phase_4_stage_4_3_native_git.md` | gix migration spec |
| `docs/roadmap/phase_4_stage_4_4_dependency_extraction.md` | Edge extraction spec |
| `docs/roadmap/phase_4_stage_4_5_graph_storage.md` | Graph DB evaluation + impl spec |
| `docs/roadmap/phase_4_stage_4_6_sir_hierarchy.md` | File/module SIR rollup spec |
| `docs/roadmap/CODEX_GUIDE.md` | This file |
| `docs/roadmap/DECISIONS_v2.md` | Updated decision register for Phase 4 |
