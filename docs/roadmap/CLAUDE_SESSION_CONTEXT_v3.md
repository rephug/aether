# AETHER — Claude Session Context

A living document of environment details, workflow patterns, and implementation decisions. Claude should reference this before giving advice on builds, git operations, or stage transitions.

---

## Environment

### Filesystem — THIS IS CRITICAL
- **Correct project location:** `/home/rephu/projects/aether` (native Linux FS)
- **Old/wrong location:** `/mnt/d/codex/projects/aether` (Windows 9P bridge — slow, causes build issues)
- `/mnt/d/` is only used for occasional `git status` or quick checks — never for builds or Codex work
- **If Robert pastes a terminal prompt showing `/mnt/d/`**, that's just where he happened to be in the shell. The Codex worktrees and builds still run from `/home/rephu/`.
- **Never suggest `/mnt/d/` is correct or frame `/home/rephu/` as surprising.**

### WSL2 Configuration
- Windows machine (MSI), Ubuntu via WSL2
- `.wslconfig`: 12GB memory, 8GB swap
- `/tmp/` is RAM-backed (tmpfs) — **never use for build artifacts**

### Build Settings (MUST be set for every cargo command)
```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
```
- `CARGO_TARGET_DIR` keeps build artifacts on disk, not in tmpfs
- `CARGO_BUILD_JOBS=2` prevents OOM during heavy crate compilation (LanceDB, CozoDB, Candle)
- These are already in the Codex prompts but Claude should verify they're present

### Codex CLI
- OpenAI Codex CLI v0.98.0 with gpt-5.3-codex model
- Runs in full-auto mode with network access
- Reads `AGENTS.md` in repo root for persistent context
- Each stage gets its own worktree under `/home/rephu/`

---

## Git Workflow

### Stage completion sequence (ALWAYS give Robert these exact commands)
After Codex finishes a stage and commits:

```bash
# 1. Push the feature branch
cd /home/rephu/aether-phase5-stage5-X-name
git push origin feature/phase5-stage5-X-name

# 2. Create PR (GitHub CLI or web UI)
gh pr create --base main --head feature/phase5-stage5-X-name \
  --title "Phase 5.X: Description" \
  --body "One-line summary."

# 3. Merge the PR (web UI or CLI)
gh pr merge --squash  # or merge via GitHub web

# 4. Update local main
cd /home/rephu/projects/aether
git checkout main
git pull --ff-only origin main
git log --oneline -3  # confirm the merge commit

# 5. Clean up worktree and branch
git worktree remove ../aether-phase5-stage5-X-name
git branch -d feature/phase5-stage5-X-name
git worktree prune

# 6. Verify clean state before next stage
git status --porcelain -b
```

### Key rules
- **Every stage branches off `main`** — main must have the previous stage merged first
- **Worktrees are created adjacent to the project dir**, not inside it
- `git status --porcelain` = empty means clean (Codex uses this as a preflight gate)
- `-d` (lowercase) for safe branch delete; refuses if not merged
- `--ff-only` for pulls; never create accidental merge commits

---

## Phase 4 Progress (COMPLETED)

### All stages merged to main
| Stage | PR | Description |
|-------|-----|-------------|
| 4.1 | #16 | LanceDB vector backend |
| 4.2 | #15 | Structured tracing |
| 4.3 | #17 | Native gix |
| 4.4 | #18 | Dependency edges |
| 4.5 | TBD | CozoDB graph storage |
| 4.6 | TBD | SIR hierarchy (file/module rollup + TS/JS import-hover) |

### Phase 4 backlog (not blocking Phase 5)
| Item | Description | Spec |
|------|-------------|------|
| 4.7 | LSP Rust `use` import-hover for file-level SIR | `phase_4_stage_4_7_lsp_import_hover.md` |

---

## Phase 5 Progress

### Not started
| Stage | Description |
|-------|-------------|
| 5.1 | Language plugin abstraction (refactor aether-parse) |
| 5.2 | Python language support (requires 5.1) |
| 5.3 | Candle local embeddings (independent of 5.1) |
| 5.4 | Reranker integration (requires 5.3) |
| 5.5 | Adaptive similarity thresholds (requires 5.3) |

### Dependency chain
```
5.1 language plugin ──► 5.2 Python ───────────────────────
5.3 Candle embeddings ──► 5.4 reranker ──► 5.5 thresholds ──┤
                                                       ▼
                                              Phase 5 complete
```

### Phase 5 build concerns
- **Candle crates** (Stages 5.3, 5.4): Heavy compile, similar to LanceDB. `CARGO_BUILD_JOBS=2` is essential. Expect 10-15 min first build.
- **tree-sitter-python** (Stage 5.2): Lighter dependency, should compile quickly.
- **Model downloads** (Stages 5.3, 5.4): Qwen3-Embedding-0.6B (~1.2GB) and Qwen3-Reranker-0.6B (~1.2GB). Downloaded to `.aether/models/` on first use. Pre-fetch with `aether download-models`.

---

## Implementation Decisions Made During Development

These are decisions made in conversation during implementation, not in the original spec docs.

### Stage 4.3: gix
- **Test shell-outs are OK.** Pass criterion "zero `Command::new("git")` matches" applies to production code in `crates/*/src` only. Integration tests that create fixture repos via `git init` are acceptable.

### Stage 4.4: Dependency extraction
- **`aether_dependencies` response shape:** Symbol-centric. Request by `symbol_id`, return both `callers` (incoming CALLS) and `dependencies` (outgoing DEPENDS_ON) arrays with edge records.
- Edges are "unresolved" in 4.4 — `target_qualified_name` is a string, not a resolved symbol ID. Resolution happens in 4.5.

### Stage 4.5: Graph storage
- **`aether_dependencies` returns both arrays** (callers + dependencies) in one MCP call. No direction parameter.
- **SqliteGraphStore uses query-time JOIN/CTE** — no persisted resolved-edge tables. `upsert_*` methods are no-op compatibility stubs.
- **CozoDB is the primary backend** with recursive Datalog for call chains. SQLite is the lightweight fallback.
- **`aether_call_chain` tool:** input is `symbol_id` or `qualified_name`, `max_depth` default 3 clamped 1..=10, output is depth-layered `Vec<Vec<SymbolRecord>>`.

### Stage 4.6: SIR hierarchy
- **MCP `aether_get_sir` strict per-level fields:** leaf requires `symbol_id`, file requires `file_path`, module requires `module_path` + `language`. No cross-field fallback derivation.
- **Response includes explicit `level` field** for unambiguous assertions and client routing.
- **Backward compatibility:** `level` defaults to `leaf` when omitted. Existing callers unchanged.
- **Module language required:** Synthetic module IDs include language, so callers must specify it. File-level infers language from extension.
- **Module intent strategy:** Deterministic concatenation of all file intents, no truncation. Full concatenation even for large modules — natural upgrade path to LLM summaries later.
- **Module aggregation is recursive** by directory path (includes all subdirectories).
- **Partial module aggregation:** Skips files missing file-level SIR. Returns `found=false` only when zero file SIRs available. Coverage fields (`files_with_sir`, `files_total`) always present in module responses.
- **LSP import-hover:** TS/JS relative imports only in 4.6. Rust `use` resolution deferred to 4.7. Falls through to existing leaf hover when resolution fails or no FileSir exists.
- **No embeddings for file/module rollups** — leaf-only embeddings unchanged.

### Phase 5 planning decisions
- **Language plugin:** Hybrid data struct (`LanguageConfig`) with optional trait overrides (`LanguageHooks`). Not a pure trait, not a pure data struct.
- **Candle architecture:** In-process, lazy loading. No sidecar process, no compile-time feature flag.
- **Ticket connectors:** Deferred to Phase 6. Phase 5 focuses on language + search.
- **Event bus:** Deferred to Phase 6. Synchronous pipeline still sufficient.
- **`aether sync`:** Deferred indefinitely. No users yet.

---

## Common Pitfalls (things Claude has gotten wrong before)

1. **Filesystem location:** `/home/rephu/` is correct. `/mnt/d/` is old. Don't hedge or say "check both."
2. **Missing prerequisites:** If Codex can't find artifacts from a previous stage, it probably needs `git pull --ff-only origin main` — the stage was merged but the worktree branched from stale main.
3. **Incomplete handoffs:** When a stage finishes, give the FULL command sequence (push, PR, merge, pull, cleanup, verify) — don't stop at explanation.
4. **Build OOM:** If cargo crashes, check that `CARGO_TARGET_DIR` isn't in `/tmp/` and `CARGO_BUILD_JOBS=2` is set.
5. **Candle build time:** First build with Candle dependencies will be slow (~10-15 min). This is normal, not a failure.

---

*Last updated: 2026-02-13 — Phase 4 complete (all 6 stages merged), Phase 5 ready to start*
