# AETHER Stage Workflow

## When to use

This skill governs how Claude Code approaches AETHER stage implementation. It overrides any default brainstorming or planning behavior — the stage specs in `docs/roadmap/` ARE the plan.

## Core principle

**Do NOT re-derive the spec.** Every AETHER stage has a detailed specification document that has already been designed, reviewed across multiple AI systems, and approved. Your job is to implement the spec faithfully, not to redesign it.

## CRITICAL: Phase 9 specs must be committed first

The Phase 9 spec files exist in project knowledge but are NOT in the repo yet. Before starting any Phase 9 stage, the developer must commit these files to `docs/roadmap/` on main:

- `phase_9_beacon.md` (overview)
- `phase_9_stage_9_1_tauri_shell.md`
- `phase_9_stage_9_2_configuration_ui.md`
- `phase_9_stage_9_3_onboarding_wizard.md`
- `phase_9_stage_9_4_enhanced_visualizations.md`
- `phase_9_stage_9_5_native_installers.md`

If these files are missing from `docs/roadmap/`, stop and tell the developer.

## Workflow per stage

### Step 1: Read the spec

Read the stage spec file from `docs/roadmap/`. Also read `CLAUDE.md` in the repo root for project-wide context, conventions, and architecture decisions.

### Step 2: Explore existing code

Before writing any code, explore the relevant existing crates to understand:

- What traits and types are already available
- How existing modules are structured
- Where new code should connect to existing infrastructure

For Phase 9 specifically, explore:
- `crates/aether-dashboard/` — the HTTP server, routes, templates, SharedState setup
- `crates/aetherd/src/main.rs` — how SharedState is initialized today
- `crates/aetherd/src/indexer.rs` — watcher lifecycle
- `crates/aether-config/` — all config structs (needed for 9.2 settings UI)

Use `rust-analyzer` to jump to definitions and find references.

### Step 3: Propose implementation approach

In Plan mode, list:
- Files to create or modify
- Types and traits to define
- Integration points with existing code
- Questions about ambiguous spec sections

Wait for developer approval before proceeding.

### Step 4: Implement

Switch to implementation. Follow the spec's in-scope list exactly. If the spec says "out of scope", do not implement it regardless of how easy it seems.

Write tests alongside implementation — test-driven development preferred.

### Step 5: Validate

Run the validation gates (see `validation-gates` skill):

```bash
cargo fmt --all --check
cargo clippy -p <crate> -- -D warnings
cargo test -p <crate>
```

### Step 6: Commit and push

```bash
git add -A
git commit -m "feat(phase9): <descriptive message>"
git push origin <branch>
```

Then create PR with descriptive title and body:

```bash
gh pr create \
  --title "Phase 9.X — <Stage Name>" \
  --body "<What this stage delivers, which decisions it implements>"
```

## Rules

- **One stage per session.** Start fresh for each stage.
- **Spec files must be committed to main** before starting implementation.
- **Worktrees branch off main** with `-b` flag: `git worktree add -b <branch> /home/rephu/aether-phase9-<n>`
- **Worktree paths** go at `/home/rephu/aether-phase9-*`, NOT inside `/home/rephu/projects/aether/`
- **Per-crate builds only.** Never `--workspace`.
- **Commit messages matter.** They're indexed by AETHER for semantic intelligence.
- **PR descriptions matter.** AETHER indexes GitHub PR content — include semantic context about what the change does and why.
- **Scope guard:** If the spec has a "SCOPE GUARD" section, those modules/files are off-limits unless the spec explicitly says to modify them.
- **Do not refactor god files** as part of Phase 9 work. The god files in `aetherd/` (sir_context.rs, indexer.rs, cli.rs, etc.) are known issues tracked separately.

## Phase 9 specific context

Phase 9 (The Beacon) wraps AETHER in a Tauri 2.x desktop application. Key architecture points:

- **Stage 9.1 is two parts:**
  - **Part A:** Dashboard completion — add 6 operational pages for batch pipeline (10.1), continuous monitor (10.2), task context (10.6), context export (R.1), presets (R.3), and fingerprint history (10.1). All work in `crates/aether-dashboard/`.
  - **Part B:** Tauri shell — new `crates/aether-desktop/` wrapping the completed dashboard in a native window with system tray.
  - **Part C:** Visual polish — consistent component styling, unified D3 color palette, dark mode audit, CDN deps bundled locally, responsive sidebar collapse, empty states, page transitions. CSS/HTML/JS only — no Rust API changes.
- **New crate:** `crates/aether-desktop/` — Tauri app that embeds `aetherd` directly (no subprocess)
- **Frontend:** HTMX + D3.js + Tailwind CSS (same as existing 27+ page dashboard, Decision #91) — NO React/Vue/Svelte
- **Single binary:** Tauri app compiles all aetherd code in-process (Decision #92)
- **System tray:** Native OS tray with status icons (Decision #93)
- **Feature gate:** `--features desktop` — headless CLI still builds without Tauri

The dashboard HTTP server from Stage 7.6 runs on an ephemeral `127.0.0.1` port inside the Tauri app. The webview loads from it. All UI pages (settings, onboarding) use the same HTMX fragment pattern.

**SharedState** is the central singleton — `Arc<SqliteStore>`, `Arc<dyn GraphStore>`, config, vector store. The Tauri app initializes this the same way `aetherd` does today, then spawns background tasks for indexer, dashboard server, MCP server, and LSP server.

**Config sections** (16 total, all must be surfaced in 9.2 Configuration UI):
inference, embeddings, planner, sir_quality, health, health_score, storage, coupling, drift, search, analysis, verification, dashboard, batch, watcher, continuous
