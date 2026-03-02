# AETHER Dashboard Revision — Run 1: Bug Fixes + Plain English Layer

**Phases combined:** A (Bug Fixes) + B (Plain English Layer)
**Date:** March 2026
**Context file:** `docs/roadmap/DASHBOARD_REVISIONS_SESSION_CONTEXT_v3.md`

Paste everything inside the code fence below into Codex as a single prompt.

---

```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- export CARGO_TARGET_DIR=/home/rephu/aether-target
- export CARGO_BUILD_JOBS=2
- export RUSTC_WRAPPER=sccache
- export PROTOC=$(which protoc)
- export TMPDIR=/home/rephu/aether-target/tmp
- mkdir -p $TMPDIR
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.
- The repo uses mold linker via .cargo/config.toml — ensure mold and clang are installed.

Read docs/roadmap/DASHBOARD_REVISIONS_SESSION_CONTEXT_v3.md for full project context.

=== OVERVIEW ===

This run has TWO tasks executed sequentially:
  TASK 1: Fix three ship-blocking bugs found during E2E validation
  TASK 2: Add a plain English layer to every existing dashboard page

Complete TASK 1 fully before starting TASK 2.

========================================================================
TASK 1: BUG FIXES (Ship-Blocking)
========================================================================

Three bugs were discovered during the first E2E validation run of AETHER on
the tokio-rs/mini-redis codebase. All three must be fixed before any new
dashboard features are added.

=== BUG 1: meta.sqlite Startup Race Condition ===

SYMPTOM: On every fresh start (no .aether/ directory yet), the dashboard state
handler logs:
  ERROR failed to open dashboard state error=sqlite error: unable to open
  database file: /home/rephu/mini-redis/.aether/meta.sqlite

The SurrealDB graph store creates its own subdirectory fine, but meta.sqlite
initialization runs before .aether/ is created by the indexer.

FIX STRATEGY:
1. Find where SharedState or DashboardState opens meta.sqlite
2. Add an ensure-directory step: std::fs::create_dir_all(workspace.join(".aether"))
   BEFORE any SQLite connection attempt
3. This should happen in the SharedState::open_readonly() or equivalent constructor
4. The graph store already does this for its own directory — match that pattern

ACCEPTANCE:
- Running `aetherd --workspace /tmp/empty-project --index-once --inference-provider mock`
  on a directory with NO .aether/ must NOT produce the sqlite error
- The .aether/ directory and meta.sqlite must be created automatically

=== BUG 2: Architecture and Causal Pages Deadlock the Server ===

SYMPTOM: Clicking /dashboard/architecture or /dashboard/causal causes the
entire aetherd HTTP server to stop responding. The process stays alive (port
remains open via ss -tlnp), but all requests hang indefinitely — including
to other pages like /dashboard/ (overview). Only fix is killing the process.

ROOT CAUSE (suspected): The HTMX fragment handlers for architecture and causal
pages perform heavy graph traversal queries against SurrealDB. These are likely
executing as blocking operations on the async Tokio runtime thread, starving
the HTTP handler and preventing any other requests from being served.

The dashboard runs on its own single-threaded Tokio runtime spawned via
std::thread::spawn. A blocking SurrealDB query on that runtime's only thread
will deadlock everything.

FIX STRATEGY:
1. Find the HTMX fragment handlers for /dashboard/frag/architecture and
   /dashboard/frag/causal (and their corresponding /api/v1/ JSON endpoints)
2. Wrap ALL graph traversal queries in tokio::task::spawn_blocking() so they
   run on a threadpool thread instead of the async runtime thread
3. Add a timeout to every graph query: tokio::time::timeout(Duration::from_secs(10), ...)
4. If the timeout fires, return an HTMX fragment with a user-friendly error:
   "This analysis is taking too long. Try reducing the graph scope or run
   `aetherd health` from the CLI for faster results."
5. Apply this same spawn_blocking + timeout pattern to ALL dashboard API
   handlers that query the graph store, not just architecture and causal.
   This includes: /api/v1/graph, /api/v1/health, /api/v1/coupling,
   /api/v1/drift, and any others that touch GraphStore or SurrealDB.

ALSO CHECK:
- The dashboard's Tokio runtime configuration. If it's new_current_thread(),
  switch to new_multi_thread() with worker_threads(2) so blocking
  one thread doesn't kill everything. This is the primary fix alongside
  spawn_blocking.
- Whether SharedState::open_readonly() holds any locks that could conflict with
  the indexer's write path if aetherd is running in --lsp --index mode.

ACCEPTANCE:
- Click /dashboard/architecture — page loads within 10 seconds or shows timeout error
- Click /dashboard/causal — page loads within 10 seconds or shows timeout error
- While architecture is loading, /dashboard/ (overview) still responds
- No deadlocks under any combination of page navigation

=== BUG 3: SIR Deduplication Not Working on Re-Index ===

SYMPTOM: Running `aetherd --workspace . --index-once --inference-provider gemini`
twice processes ALL symbols again, including the 27 that already succeeded on
the first run. This wastes API rate limits and makes iterative testing painful.

ROOT CAUSE (suspected): The indexer's change detection is based on file content
hashing (detecting whether source files changed), but doesn't check whether SIR
already exists for individual symbols. When file hashes match on re-run, there
may be a logic error that still submits generation jobs for all symbols.

FIX STRATEGY:
1. Find the SIR generation job submission code (likely in the indexer/observer
   module that processes parsed symbols)
2. Before submitting a SIR generation job for a symbol, check if a SIR file
   already exists for that symbol_id:
   - Check .aether/sir/{symbol_hash}.json exists
   - OR query the SIR store for an existing entry
3. Skip the generation job if SIR already exists AND the source hash matches
4. Log at debug level: "Skipping SIR generation for {symbol_name}: already exists"
5. Add --force flag support: when --force is passed, skip the dedup check and
   regenerate all SIR (already exists as CLI flag per --help output)

ACCEPTANCE:
- First run with mock: generates SIR for all ~60 symbols
- Second run with mock (no source changes): generates SIR for 0 symbols, logs "skipping"
- Edit one .rs file, re-run: only regenerates SIR for symbols in that file
- --force flag: regenerates all SIR regardless

========================================================================
TASK 2: PLAIN ENGLISH LAYER
========================================================================

The dashboard currently shows raw technical metrics with no explanation. This
task adds a plain English layer to every existing page — NO new pages, NO new
API endpoints, just additive HTML/CSS/JS on top of the existing dashboard.

TECHNOLOGY: HTMX + D3.js + Tailwind CSS (all from CDN). NO React, NO Node.js.
Maud for server-side HTML fragments. rust-embed for static files.

=== CHANGE 1: Welcome Banner on Overview ===

Add a welcome section at the top of /dashboard/frag/overview, BEFORE any charts.

Content (generated server-side from SharedState data):
- Headline: "Welcome to [project_name]"
- 2-sentence summary: "[project_name] contains [N] components across [M] files.
  AETHER has analyzed [P]% of them and built a map of how they connect."
- Navigation cards (HTMX links, show as "Coming Soon" until target phases land):
  1. "📖 Understand This Project" → /dashboard/frag/anatomy
  2. "🗺️ Take a Guided Tour" → /dashboard/frag/tour
  3. "💬 Build a Question" → /dashboard/frag/prompts

=== CHANGE 2: Plain English Metric Labels ===

Replace or supplement technical labels throughout:

| Current | New | Where |
|---------|-----|-------|
| "Symbols" / "Symbol Count" | "Components" | Overview |
| "SIR Coverage" | "Understanding Coverage" + subtitle | Overview |
| "Files Indexed" | "Source Files" | Overview |
| "Graph Nodes" | "Connected Components" | Overview/Graph |
| "Coupling Score: 0.73" | "Connection Strength: Strong (0.73)" | Coupling |
| "PageRank: 0.12" | "Importance Score: 0.12" | Graph detail |
| "Drift Score" | "Change Risk" | Drift |

Qualitative coupling labels: 0–0.3 "Weak", 0.3–0.6 "Moderate", 0.6–0.8 "Strong", 0.8–1.0 "Very Strong"

=== CHANGE 3: Explanation Headers on Every Page ===

Add a header block at the top of each page fragment with:
- Plain English title (h2)
- 1-2 sentence explanation
- Styled: bg-slate-50 rounded-lg p-4 mb-6

GRAPH: "How This Project's Pieces Connect" / "Each dot is a component. Lines
mean one depends on another. Bigger dots are more important."

HEALTH: "Project Health Check" / "AETHER measures your codebase across several
dimensions. Higher scores are better."

COUPLING: "Which Files Change Together" / "When two files frequently change at
the same time, they're coupled."

DRIFT: "What's Changed Since Last Analysis" / "Drift means the code changed but
AETHER's understanding hasn't been updated yet."

=== CHANGE 4: "What Does This Mean?" Tooltips ===

Add Tippy.js from CDN (~3KB):
  https://cdnjs.cloudflare.com/ajax/libs/tippy.js/6.3.7/tippy-bundle.umd.min.js
  https://cdnjs.cloudflare.com/ajax/libs/tippy.js/6.3.7/tippy.css

Add ⓘ icon with data-tippy-content="..." next to every metric.
Initialize: tippy('[data-tippy-content]') in htmx:afterSwap handler.

Tooltip examples:

"SIR Coverage" → "SIR is AETHER's understanding of what each piece of code does.
87% means AETHER has analyzed 87 out of every 100 components."

"Coupling Score" → "0 to 1 measuring how strongly two files are connected. High
scores mean they change together and share dependencies."

"PageRank" → "How central a component is. Higher = more things depend on it.
Like measuring road importance by counting routes that use it."

"Confidence" → "How sure AETHER is about its analysis. Below 0.5 = should be verified."

Add tooltips to ALL metrics. For any without a specific definition above,
generate one following the pattern: what it measures, what the number means,
why you should care.

=== CHANGE 5: Complexity Level Selector ===

Global toggle in sidebar:
🟢 "I'm new here" (default) — all explanations visible, simplified terms
🟡 "I know some code" — condensed explanations, mixed terminology
🔴 "Show me the data" — headers hidden, tooltips muted, full technical terms

Implementation:
- localStorage key "aether-complexity", default "beginner"
- CSS classes on <body>: complexity-beginner, complexity-intermediate, complexity-expert
- .complexity-expert .beginner-only { display: none; }
- .complexity-beginner .expert-only { display: none; }
- 3 radio-style buttons in sidebar, persists across HTMX navigations

=== CHANGE 6: Health Page Recommendations ===

After radar chart, add "What to Work on First" card. Find lowest-scoring
dimension, select matching recommendation:

SIR Coverage < 70%: "🔍 Many components haven't been analyzed. Run --index-once."
Test Coverage < 50%: "🧪 Less than half your components have tests."
Graph Connectivity < 60%: "🔗 Some components are isolated. May indicate dead code."
Staleness > 24h: "⏰ Analysis is stale. Re-run indexing."

=== CHANGE 7: Coupling Page Summary ===

"Key Connections" card above heatmap. Auto-generate 3-5 bullet points for
strongest coupling pairs:
"{file_a} and {file_b} are {qualitative_label} ({score}) — {reason from signal types}"

========================================================================
VALIDATION (covers both tasks)
========================================================================

BUG FIXES:
cd /home/rephu/mini-redis
rm -rf .aether

# Test Bug 1: no sqlite error on cold start
aetherd --workspace . --index-once --inference-provider mock 2>&1 | grep -i "sqlite error"
# Expected: no output (no error)

# Test Bug 3: re-run should skip all symbols
aetherd --workspace . --index-once --inference-provider mock 2>&1 | grep -i "skipping\|already exists\|processed.*0"
# Expected: all symbols skipped

# Test Bug 2: dashboard pages don't deadlock
aetherd --workspace . --lsp --index --inference-provider mock &
sleep 3
curl -s -m 5 http://127.0.0.1:9720/api/v1/overview | head -c 200
curl -s -m 15 http://127.0.0.1:9720/dashboard/frag/architecture | head -c 200
curl -s -m 15 http://127.0.0.1:9720/dashboard/frag/causal | head -c 200
curl -s -m 5 http://127.0.0.1:9720/api/v1/overview | head -c 200
kill %1

PLAIN ENGLISH:
1. Welcome banner with project name and counts
2. Plain English metric labels
3. ⓘ tooltips on all metrics
4. Complexity selector toggles content visibility
5. Health recommendations card
6. Coupling summary bullets
7. Explanation headers on all pages
8. D3 visualizations still render
9. HTMX navigation still works
10. No regressions on API endpoints

cargo fmt --all --check
cargo clippy --workspace --features dashboard -- -D warnings
cargo test -p aether-dashboard
cargo test -p aether-store
cargo test -p aether-mcp
cargo test -p aetherd --features dashboard
```
