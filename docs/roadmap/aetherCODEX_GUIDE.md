# CODEX_GUIDE.md — How to Use Codex with AETHER

## What is this?

This guide explains how to use [OpenAI Codex CLI](https://developers.openai.com/codex/cli) to implement AETHER stages. Each stage doc in `docs/roadmap/` contains copy-paste-ready prompts. This guide covers the workflow, patterns, and gotchas.

---

## Quick Start (30-second version)

```bash
# 1. Open terminal in your aether repo root
cd ~/code/aether

# 2. Start Codex
codex

# 3. Paste the prompt from the stage doc
#    e.g., the "Exact Codex prompt(s)" block from phase_4_stage_4_1_lancedb_vector_backend.md

# 4. Codex creates the branch, worktree, implements, tests, and commits
# 5. You review and push
git -C ../aether-phase4-stage4-1-lancedb push -u origin feature/phase4-stage4-1-lancedb
```

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
Codex creates a git worktree at `../aether-<stage-name>` so your main checkout stays untouched. This means you can have multiple stages in progress simultaneously.

### 4. Implement
Codex reads the stage doc, cross-references the codebase, and writes code. It only modifies files within the stage's scope.

### 5. Test
Codex runs the three validation gates:
```bash
cargo fmt --all --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
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
```

### Codex permissions
By default, Codex runs with network access disabled. For stages that need network access (4.1 LanceDB downloads during build, `cargo` fetching crates), enable it:

```bash
# Option 1: CLI flag (per-session)
codex -c 'sandbox_workspace_write.network_access=true'

# Option 2: Project config (persistent) — add to .codex/config.toml
sandbox_mode = "workspace-write"
[sandbox_workspace_write]
network_access = true
```

For full-auto mode (no approval prompts on safe operations):
```bash
codex --full-auto --model gpt-5.3-codex
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
git worktree remove ../aether-phase4-stage4-1-lancedb
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
# Ubuntu/Debian
sudo apt-get install liblzma-dev

# macOS
brew install xz

# Or use static linking (already in the stage doc)
# lzma-sys = { version = "*", features = ["static"] }
```

### "CozoDB build fails" (Stage 4.5)
CozoDB with the SQLite backend needs `libsqlite3`. If the build fails:
```bash
# Ubuntu/Debian
sudo apt-get install libsqlite3-dev

# macOS (usually pre-installed, but if needed)
brew install sqlite
```
If it still fails, the stage doc has a fallback: SQLite-only edge resolution (recursive CTEs, no CozoDB).

---

## Prompt Templates

### Standard stage execution
```
You are working in the repo root of https://github.com/rephug/aether.
Read docs/roadmap/<stage_file>.md for the full specification.
Follow the "Exact Codex prompt(s)" section exactly.
```

### Bug fix after a stage
```
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
