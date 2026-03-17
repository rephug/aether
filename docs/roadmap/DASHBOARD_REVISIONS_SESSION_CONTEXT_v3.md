# Dashboard Revisions Session Context v3 — "Your Codebase, Explained"

## What You Need to Know

I'm building AETHER, a Rust multi-crate workspace that creates persistent semantic intelligence for codebases. We completed the initial dashboard implementation in Stage 7.6 and added six visualization pages in Stage 7.9. During the first E2E validation run on tokio-rs/mini-redis (March 1, 2026), three bugs were found and the dashboard was identified as incomprehensible to non-technical users. This revision addresses both problems and adds two major new capabilities: a narrative engine that turns code analysis into stories, and an LLM collaboration teaching suite that uses the codebase as a textbook.

**Repo:** `https://github.com/rephug/aether` at `/home/rephu/projects/aether`
**Dev environment:** WSL2 Ubuntu, mold linker, all builds from `/home/rephu/`
**Companion docs:**
- `DASHBOARD_REVISIONS_v1.md` — original design spec
- `DASHBOARD_REVISION_CODEX_PROMPTS_v3_part1.md` — Phases A–D (foundation)
- `DASHBOARD_REVISION_CODEX_PROMPTS_v3_part2.md` — Phases E–F (narrative + interactive)
- `DASHBOARD_REVISION_CODEX_PROMPTS_v3_part3.md` — Phases G–H (LLM collaboration suite)

## What Just Happened (E2E Validation on mini-redis)

The first full E2E validation was run March 1, 2026 against `tokio-rs/mini-redis`. Results:

**What worked:**
- Indexing completed: 27 of ~60 symbols got SIR on first pass (Gemini rate limit hit on rest)
- SIR quality was excellent — clean intents, accurate dependencies, correct error modes
- CLI search passed all tests: `Db`, `read_frame`, `cmd/get`, `nonexistent_symbol_xyz` all correct
- Dashboard overview, health, coupling, drift pages loaded with mock provider
- Dashboard port binding and HTMX sidebar navigation functional

**What broke:**
- BUG-DASH-1: `meta.sqlite` startup race — error on every cold start
- BUG-DASH-2: Architecture and causal pages deadlock the entire server
- BUG-DASH-3: Re-indexing reprocesses all symbols including ones that already have SIR

**User feedback (Robert):** "I couldn't understand anything of what I was looking at."

## What's Already on Main (Dashboard Infrastructure)

### Dashboard Crate: `aether-dashboard`
- **Router:** `dashboard_router(state: Arc<SharedState>) -> Router` in `lib.rs`
- **Static files:** `rust-embed` with `#[folder = "src/static/"]`, embedded at compile time
- **JSON endpoints:** `/api/v1/overview`, `/api/v1/graph`, `/api/v1/drift`, `/api/v1/coupling`, `/api/v1/health`
- **HTMX fragments:** `/dashboard/frag/overview`, `/dashboard/frag/graph`, `/dashboard/frag/drift`, `/dashboard/frag/coupling`, `/dashboard/frag/health`
- **Stage 7.9 pages (6 new):** X-Ray, Blast Radius Explorer, Architecture Map, Time Machine, Causal Explorer, Smart Search

### Dashboard Mounting in aetherd
- Feature-gated: `--features dashboard` on cargo build
- Spawns its own Tokio runtime on a `std::thread::spawn` (separate from LSP's single-threaded runtime)
- Shares `SharedState` (read-only) with MCP tools — `Arc<SharedState>` passed to router
- Binds to `127.0.0.1:9730`
- `--no-dashboard` CLI flag disables even when feature compiled

### HTML Templating & Tech Stack
- **Maud** (`maud = { version = "0.26", features = ["axum"] }`) for server-side HTML fragments
- **HTMX 2.0.4** for SPA-like navigation and interactive updates
- **D3 7.9.0** for visualizations
- **Tailwind CSS** (CDN) for styling
- NO React, NO Node.js, NO build step (Decision #41)

### Data Sources Available via SharedState
- `SqliteStore`: symbol counts, file counts, language breakdown, SIR coverage, schema version
- `Arc<dyn GraphStore>` (SurrealGraphStore): dependency edges, graph traversal, community data
- `Arc<dyn VectorStore>`: embedding similarity queries
- Config: workspace path, inference provider, dashboard settings
- Phase 6 analytics in `aether-analysis`: drift, coupling, health, causal chains
- `aether-graph-algo`: PageRank, Louvain, BFS, SCC, connected components

### Existing CLI Commands (reusable logic)
- `aetherd ask "question"` — unified search across symbols, notes, coupling, test intents
- `aetherd --search "term"` — lexical/semantic/hybrid symbol search
- `aetherd health` — graph health metrics
- `aetherd blast-radius --file src/db.rs` — blast radius computation
- These are local function calls, not MCP protocol. Dashboard can call them directly.

## Three Bugs to Fix (Phase A)

### BUG-DASH-1: meta.sqlite Startup Race Condition
- **Where:** `SharedState::open_readonly()` in `crates/aether-mcp/src/state.rs`
- **What:** SQLite tries to open `.aether/meta.sqlite` before `.aether/` directory exists
- **Fix:** Add `std::fs::create_dir_all(workspace.join(".aether"))` before SQLite connection
- **Severity:** 🟡

### BUG-DASH-2: Architecture/Causal Pages Deadlock Server
- **Where:** HTMX fragment handlers for architecture and causal pages (Stage 7.9)
- **What:** Blocking graph queries on single-threaded dashboard Tokio runtime
- **Fix:** `spawn_blocking()` + `timeout(Duration::from_secs(10))` on ALL graph queries; consider upgrading to `new_multi_thread().worker_threads(2)`
- **Severity:** 🔴

### BUG-DASH-3: SIR Deduplication Not Working
- **Where:** Indexer/observer SIR job submission code
- **What:** Re-index processes all symbols, doesn't check for existing SIR
- **Fix:** Check `.aether/sir/{symbol_hash}.json` exists before submitting job; respect `--force` flag
- **Severity:** 🟠

## Architecture Decisions Still in Effect

- **Decision #41:** HTMX + D3.js + Tailwind CSS. NO React, NO Node.js, NO build step.
- **Decision #38:** SurrealDB 3.0 with SurrealKV backend for graph storage
- **Maud for HTML:** Compile-time checked, no separate template files
- **rust-embed for static files:** Everything baked into binary at compile time
- **Feature-gated:** Dashboard only included with `--features dashboard`

## Phase-by-Phase Overview (8 Phases)

### Phase A: Bug Fixes (1 Codex run)
Fix BUG-DASH-1, BUG-DASH-2, BUG-DASH-3. No new features.

### Phase B: Plain English Layer (1 Codex run)
Additive HTML/CSS on existing pages. Welcome banner, plain English metric labels, explanation headers, Tippy.js tooltips, complexity level selector, health recommendations, coupling summaries. No new API endpoints.

### Phase C: Project Anatomy + Layer Narratives (1–2 Codex runs)
New `/dashboard/anatomy` page with: auto-generated project summary, maturity badge (lifecycle phase as pill not chart), tech stack discovery, role-based layer categorization with **layer narratives** (paragraph explaining how each layer works as a unit), file-level drill-down (click file → see summary + symbols), key actors by centrality, simplified layer-level graph. New HTMX sub-fragments for layer/file expansion.

### Phase D: Dynamic Guided Tour + Glossary (1 Codex run)
Tour: stops dynamically generated based on which layers exist. Glossary: auto-generated dictionary with search, layer/kind filters, pagination.

### Phase E: Narrative Engine — Symbol Deep Dive + File Deep Dive + Flow Narrative (2 Codex runs)
The "click anything and understand it" layer. Three narrative features:

**Symbol Deep Dive** (`/dashboard/symbol/{name}`): Full narrative report for any symbol. Sections: one-line role, how it fits (layer context), who creates/calls it (upstream narrative), who depends on it (downstream narrative grouped by layer), what it depends on, side effects and risks, blast radius in plain English, LLM difficulty rating. Accessible from every symbol name anywhere in the dashboard (Glossary, Anatomy, Tour, Graph, search results).

**File Deep Dive** (`/dashboard/file/{path}`): Full narrative report for a file. How the symbols within the file relate to each other, how the file connects to the rest of the project, what the file's role is in its layer.

**Flow Narrative** (`/dashboard/flow`): Pick a starting symbol (or starting + ending), get a narrated trace through the dependency graph. Each step shows the symbol, its layer, and a plain English description of what happens. Traces data flow direction: entry → processing → storage → output.

All three derive content from existing SIR + dependency graph + layer categorization. No new inference calls.

### Phase F: What Changed Recently + Ask AETHER (1 Codex run)
**What Changed Recently:** Timeline on overview page showing recent changes with semantic context. Sources: filesystem mtimes, SIR timestamps, git log. Time period selector. Also available as full-page view.

**Ask AETHER:** Global search bar at top of every page. Routes to same `run_ask_command` logic as CLI. Renders composed English summary + related component cards inline.

### Phase G: LLM Collaboration Suite Part 1 (1–2 Codex runs)
**LLM Difficulty Radar:** Score every component on how hard it would be for an LLM to generate. Based on SIR error_modes count, side_effects count, dependency count, async/concurrent patterns. Shows as color overlay on Graph page, column in Glossary, section in Symbol Deep Dive.

**Enhanced Prompt Builder** (builds on existing prompt builder concept): 8 goal categories, typeahead symbol search, template-based prompt generation. Now includes Code-to-Spec (generate a buildable specification from SIR data).

**Context Window Advisor:** Given a target symbol/task, compute the minimal sufficient context (which files/types the LLM needs to see). Uses dependency graph to determine required vs. optional vs. noise. Shows line counts per required file.

### Phase H: LLM Collaboration Suite Part 2 (1–2 Codex runs)
**Prompt Decomposer** ("Build This in N Steps"): Given a symbol/file, generate an ordered sequence of prompts that build it from the ground up following the dependency graph. Foundation first, then each dependent layer.

**Verification Checkpoints:** After each decomposer step, generate a checklist of things to verify before proceeding. Derived from SIR: required trait implementations, visibility constraints, invariants from error_modes.

**Prompt Autopsy** ("What Would Work and What Wouldn't"): For a given symbol, generate three prompts at different specificity levels (✅ would work, ⚠️ would produce bugs, ❌ would fail) with explanations of why. Teaching: calibrate prompting specificity to code complexity.

## Phase Dependencies

```
Phase A (Bug Fixes)
  ↓
Phase B (Plain English)
  ↓
Phase C (Anatomy + Layers)
  ↓
  ├── Phase D (Tour + Glossary)
  │     ↓
  │     Phase G (LLM Suite Part 1: Difficulty + Prompts + Context Advisor)
  │       ↓
  │       Phase H (LLM Suite Part 2: Decomposer + Checkpoints + Autopsy)
  │
  ├── Phase E (Narrative Engine: Deep Dives + Flow)
  │
  └── Phase F (Changes + Ask AETHER)
```

D, E, F are independent of each other after C.
G requires D (Glossary for difficulty column) + E (Deep Dive for difficulty section).
H requires G (uses Difficulty Radar scores, builds on Prompt Builder).

## Issues to Verify Before Each Codex Run

### For Phase A (Bugs):
1. Location of `SharedState::open_readonly()` — does it call SQLite before ensuring directory?
2. Stage 7.9 fragment handler files — where are architecture/causal handlers?
3. Dashboard Tokio runtime config — `new_current_thread()` or `new_multi_thread()`?
4. SIR generation job submission point — where does indexer submit to inference?
5. `--force` flag wiring — is it plumbed through to bypass dedup?

### For Phase B (Plain English):
6. Existing fragment handler structure — how are metrics rendered in maud?
7. index.html CDN section — where to add Tippy.js?
8. htmx:afterSwap handler — existing JavaScript for post-swap initialization?

### For Phase C (Anatomy):
9. SIR iteration — does SharedState expose `list_all_sir()` or need filesystem scan?
10. Manifest parsing — does SqliteStore have Cargo.toml data or parse from filesystem?
11. PageRank function signature in `aether-graph-algo`?
12. Layer categorization caching — where to cache for reuse by Tour/Glossary/Narrative?

### For Phase D (Tour + Glossary):
13. Layer data caching from Phase C — how to access cached layers from Tour/Glossary handlers?
14. Tour stop generation — verify layer detection produces >= 2 stops on mini-redis

### For Phase E (Narrative Engine):
15. **Upstream dependency traversal** — does GraphStore have `get_dependents(symbol_id)` or `get_callers(symbol_id)` method? The Symbol Deep Dive needs both directions.
16. **Graph path finding** — does `aether-graph-algo` have shortest path / BFS between two nodes? Flow Narrative needs to trace from A to B.
17. **SIR access by symbol name** — can we look up SIR by symbol name (not just hash)? Deep Dive links use names.
18. **File-to-symbols mapping** — does SqliteStore have a query for "all symbols in file X"?

### For Phase F (Changes + Ask):
19. `run_ask_command` function location and signature — sync or async?
20. Git availability detection — `std::process::Command::new("git").arg("rev-parse")`
21. File mtime tracking — does SqliteStore record mtimes during indexing?

### For Phase G (LLM Suite Part 1):
22. **SIR field access** — can we efficiently query error_modes count, side_effects count, dependency count across all symbols? Difficulty scoring needs bulk access.
23. **Dependency depth** — does the graph support "transitive dependency count" or does Phase G need to compute it via BFS?

### For Phase H (LLM Suite Part 2):
24. **Topological sort** — does `aether-graph-algo` have topological sort? Decomposer needs dependency-ordered generation sequence.
25. **SIR trait/impl data** — does SIR capture which traits a type implements? Verification Checkpoints need this for "must implement Clone" checks.

## Standard Build Settings

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=1
export PROTOC=$(which protoc)
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

## Standard Git Workflow

```bash
# After Codex finishes each phase:
cargo fmt --all --check
cargo clippy --workspace --features dashboard -- -D warnings
cargo test -p aether-dashboard
cargo test -p aether-store
cargo test -p aether-mcp
cargo test -p aetherd --features dashboard

# Push + PR:
git push -u origin feature/dashboard-revisions --force-with-lease
gh pr create --base main --head feature/dashboard-revisions \
  --title "Dashboard revisions: [phase description]" \
  --body "See DASHBOARD_REVISIONS_v1.md for full spec"

# After PR merges:
git switch main
git pull --ff-only
git worktree remove ../aether-dash-revisions
git branch -d feature/dashboard-revisions
```

## What NOT to Touch

- **MCP tools** (`aether-mcp`): Don't modify MCP tool implementations
- **LSP server**: Dashboard changes must not affect LSP behavior
- **CLI commands**: Subcommands remain unchanged (Phase F reuses their logic)
- **Inference pipeline**: No new LLM calls anywhere. All content is template-composed.
- **Database schemas**: No new tables, no migrations. Read-only from existing stores.
- **Existing D3 visualizations**: Add to them, don't replace them.

## Key Patterns

### spawn_blocking for Graph Queries
```rust
let result = tokio::time::timeout(
    Duration::from_secs(10),
    tokio::task::spawn_blocking(move || { /* graph query */ })
).await???;
```

### HTMX Fragment Pattern
```rust
async fn frag_handler(State(state): State<Arc<SharedState>>) -> impl IntoResponse {
    html! { div class="p-6" { /* maud content */ } }
}
```

### Narrative Composition Pattern (NEW — used heavily in E, G, H)
```rust
/// Compose a plain English narrative from SIR data + graph data.
/// All narrative functions follow this pattern:
/// 1. Gather raw data (SIR intents, graph edges, layer assignments)
/// 2. Group and categorize (by layer, by relationship type, by risk)
/// 3. Select templates based on data shape
/// 4. Fill templates with specific data
/// 5. Join sentences into coherent paragraphs
///
/// Example template set for "who depends on this symbol":
///   0 dependents: "Nothing else in the project directly uses {name}."
///   1-3 dependents: "{dep1}, {dep2}, and {dep3} depend on {name}."
///   4+ dependents: "{name} is central to the project — {count} components
///     depend on it, including {top_by_layer_narrative}."
///
/// Layer grouping for 4+ dependents:
///   "all {N} command handlers in the Core Logic layer ({names}),
///    the server's connection handler in the Interface layer, and
///    the blocking client in the Connectors layer"
fn compose_dependents_narrative(symbol: &str, dependents: &[Dependent]) -> String {
    // Implementation follows the pattern above
}
```

### Symbol Link Pattern (NEW — every symbol name in the dashboard is clickable)
```html
<!-- Every symbol name rendered anywhere becomes a Deep Dive link -->
<a hx-get="/dashboard/frag/symbol/Db" hx-target="#main-content"
   class="text-blue-600 hover:underline cursor-pointer">Db</a>
```

## E2E Test Codebase

All validation against `tokio-rs/mini-redis` at `/home/rephu/mini-redis`.
Expected: ~60 symbols, ~15 .rs files, SIR in `.aether/sir/`, graph in `.aether/graph/`.

## North Star

Within 60 seconds of opening the dashboard on an unknown project, answer:
1. What does this project do?
2. What technologies does it use?
3. What are the most important pieces?
4. How do the pieces connect?
5. Where should I start to understand more?
6. What changed recently?
7. (Click anything for the full story)
8. (Ask any question directly)
9. How would I prompt an AI to rebuild this?
10. Where would an AI struggle with this code?
