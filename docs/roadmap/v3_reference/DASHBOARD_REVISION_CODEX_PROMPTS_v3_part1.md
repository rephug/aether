# AETHER Dashboard Revision — Codex Prompts v3, Part 1 of 3

**Phases covered:** A (Bug Fixes), B (Plain English), C (Anatomy + Layers), D (Tour + Glossary)
**Date:** March 2026
**Companion files:**
- `DASHBOARD_REVISIONS_SESSION_CONTEXT_v3.md` — full context
- `DASHBOARD_REVISION_CODEX_PROMPTS_v3_part2.md` — Phases E–F
- `DASHBOARD_REVISION_CODEX_PROMPTS_v3_part3.md` — Phases G–H

---

## Git Workflow for Each Phase

```bash
# Before starting — create worktree from main:
cd /home/rephu/projects/aether
git switch main && git pull --ff-only
git worktree add ../aether-dash-revisions feature/dashboard-revisions

# After Codex finishes each phase:
cargo fmt --all --check
cargo clippy --workspace --features dashboard -- -D warnings
cargo test -p aether-dashboard
cargo test -p aetherd --features dashboard

# Push + PR:
git push -u origin feature/dashboard-revisions --force-with-lease
gh pr create --base main --head feature/dashboard-revisions \
  --title "Dashboard revisions: [phase description]" \
  --body "See DASHBOARD_REVISIONS_v1.md for full spec"

# After PR merges:
git switch main && git pull --ff-only
git worktree remove ../aether-dash-revisions
git branch -d feature/dashboard-revisions
```

---

## Phase A: Bug Fixes (Ship-Blocking)

```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- export CARGO_TARGET_DIR=/home/rephu/aether-target
- export CARGO_BUILD_JOBS=1
- export PROTOC=$(which protoc)
- export TMPDIR=/home/rephu/aether-target/tmp
- mkdir -p $TMPDIR
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.
- The repo uses mold linker via .cargo/config.toml — ensure mold and clang are installed.

=== CONTEXT ===

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
  consider switching to new_multi_thread() with worker_threads(2) so blocking
  one thread doesn't kill everything. However, spawn_blocking is the primary fix.
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

=== VALIDATION ===

After fixing all three bugs, run this full validation sequence:

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
curl -s -m 5 http://127.0.0.1:9730/api/v1/overview | head -c 200
curl -s -m 15 http://127.0.0.1:9730/dashboard/frag/architecture | head -c 200
curl -s -m 15 http://127.0.0.1:9730/dashboard/frag/causal | head -c 200
curl -s -m 5 http://127.0.0.1:9730/api/v1/overview | head -c 200
kill %1

cargo fmt --all --check
cargo clippy --workspace --features dashboard -- -D warnings
cargo test -p aether-dashboard
cargo test -p aether-store
cargo test -p aetherd --features dashboard
```

---

## Phase B: Plain English Layer

```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- export CARGO_TARGET_DIR=/home/rephu/aether-target
- export CARGO_BUILD_JOBS=1
- export PROTOC=$(which protoc)
- export TMPDIR=/home/rephu/aether-target/tmp
- mkdir -p $TMPDIR

PREREQUISITE: Phase A bug fixes must be merged to main first.

=== CONTEXT ===

The dashboard currently shows raw technical metrics with no explanation. This
phase adds a plain English layer to every existing page — NO new pages, NO new
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

=== VALIDATION ===

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
cargo test -p aetherd --features dashboard
```

---

## Phase C: Project Anatomy + Layer Narratives

```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- export CARGO_TARGET_DIR=/home/rephu/aether-target
- export CARGO_BUILD_JOBS=1
- export PROTOC=$(which protoc)
- export TMPDIR=/home/rephu/aether-target/tmp
- mkdir -p $TMPDIR

PREREQUISITE: Phases A and B must be merged to main first.
TECHNOLOGY: HTMX + D3.js + Tailwind CSS (all from CDN). Maud for fragments.

=== CONTEXT ===

The Project Anatomy page is the highest-value new page. It's the "ingredients
list" for a codebase. This phase also introduces the LAYER NARRATIVE engine
that composes plain English paragraphs explaining how each layer works as a
unit. Layer narratives are reused by Tour (Phase D), Deep Dives (Phase E),
and the LLM suite (Phases G-H), so the narrative composition logic should
be implemented as reusable functions, not inline in handlers.

=== NARRATIVE COMPOSITION MODULE ===

IMPORTANT: Create a shared narrative module that will be reused across many
future phases. This is NOT throwaway code — it's the narrative engine.

Create: crates/aether-dashboard/src/narrative.rs

This module contains reusable functions for composing plain English from data.
Every function follows the same pattern:
1. Gather raw data (SIR intents, graph edges, layer assignments)
2. Group and categorize
3. Select template based on data shape (count, type, complexity)
4. Fill template with specific data
5. Join into coherent sentences/paragraphs

REQUIRED FUNCTIONS (all will be reused in later phases):

fn compose_project_summary(sir_intents: &[SirIntent], lang: &str, deps: &[Dep]) -> String
  — 3-5 sentence project summary from aggregated SIR data

fn compose_layer_narrative(layer: &Layer, files: &[FileInfo], symbols: &[SymbolInfo]) -> String
  — Paragraph explaining how a layer works as a unit (see below)

fn compose_file_summary(file: &str, symbols: &[SymbolInfo]) -> String
  — 1-2 sentence summary of a file from its symbols' SIR intents

fn compose_dependents_narrative(name: &str, dependents: &[Dependent], layers: &LayerMap) -> String
  — Plain English description of who depends on a symbol, grouped by layer

fn compose_dependencies_narrative(name: &str, deps: &[Dependency]) -> String
  — Plain English description of what a symbol depends on

fn qualify_coupling(score: f64) -> &'static str
  — Returns "Weak"/"Moderate"/"Strong"/"Very Strong" (reuse from Phase B)

fn qualify_difficulty(error_count: usize, side_effect_count: usize, dep_count: usize, is_async: bool) -> (&'static str, &'static str)
  — Returns (emoji, label) for LLM difficulty: ("🟢", "Easy") etc. (used in Phase G)

Template patterns for compose_dependents_narrative:
  0 dependents: "Nothing else in the project directly uses {name}."
  1-3 dependents: "{dep1}, {dep2}, and {dep3} depend on {name}."
  4+ dependents: "{name} is central to the project — {count} components depend
    on it, including {grouped_by_layer}."

  Layer grouping for 4+:
    "all {N} command handlers in the Core Logic layer ({names}), the server's
    connection handler in the Interface layer, and the blocking client in the
    Connectors layer"

Template patterns for compose_layer_narrative:

  INTERFACE LAYER:
  "The Interface layer contains {file_count} files with {symbol_count} components
  that handle how the project communicates with the outside world.
  {top_file_summary}. {second_file_summary}. All interface components ultimately
  connect to the {most_depended_layer} layer for processing."

  CORE LOGIC LAYER:
  "The Core Logic layer is the heart of the project with {symbol_count} components
  across {file_count} files. {if has_command_pattern: 'It follows a command pattern
  where each command ({command_names}) processes a specific operation.'}
  {top_file_summary}. {relationship_to_data_layer}."

  DATA LAYER:
  "The Data layer manages the project's state through {symbol_count} components
  in {file_count} files. {top_symbol_narrative}. {side_effects_summary}."

  WIRE FORMAT LAYER:
  "The Wire Format layer handles data serialization and parsing with {symbol_count}
  components. {top_file_summary}. These components are used by both the Interface
  layer (for incoming data) and the Connectors layer (for outgoing data)."

  CONNECTOR LAYER:
  "The Connectors layer provides {symbol_count} components for communicating with
  external systems. {file_summaries}."

  TEST LAYER:
  "The test suite contains {symbol_count} test components across {file_count}
  files. {coverage_narrative}."

  UTILITIES LAYER:
  "The project includes {symbol_count} utility components for common operations.
  {file_summaries}."

  Generic fallback for any layer:
  "This layer contains {symbol_count} components across {file_count} files.
  {top_3_file_summaries}."

=== API ENDPOINT: GET /api/v1/anatomy ===

Returns JSON with these sections:

{
  "data": {
    "project_name": "mini-redis",
    "summary": "mini-redis is a lightweight Redis server implementation...",
    "maturity": {
      "dominant_phase": "Implementation",
      "icon": "⚙️",
      "description": "Focused on concrete functionality with solid test coverage"
    },
    "tech_stack": [ { "category": "...", "items": [...] } ],
    "layers": [
      {
        "name": "Interface",
        "icon": "🌐",
        "description": "Accepts external input from network, CLI, or HTTP",
        "narrative": "The Interface layer contains 3 files with 8 components...",
        "files": [
          {
            "path": "src/server.rs",
            "symbol_count": 4,
            "summary": "TCP listener that accepts client connections and dispatches commands",
            "symbols": ["run", "Handler", "Handler::run", "Listener"]
          }
        ],
        "total_symbol_count": 8
      }
    ],
    "key_actors": [
      {
        "name": "Db",
        "kind": "struct",
        "file": "src/db.rs",
        "layer": "Data",
        "description": "Manages the shared database state...",
        "centrality": 0.34,
        "dependents_count": 12
      }
    ],
    "simplified_graph": {
      "nodes": [ { "id": "Interface", "symbol_count": 8 } ],
      "edges": [ { "source": "Interface", "target": "Core Logic", "weight": 5 } ]
    }
  },
  "meta": { "generated_at": "...", "index_age_seconds": 120, "stale": false }
}

=== IMPLEMENTING THE ANATOMY ENDPOINT ===

Create: crates/aether-dashboard/src/api/anatomy.rs

SECTION 1 — Project Summary:
- Call compose_project_summary() from narrative module
- Cache result: only regenerate when SIR count changes

SECTION 2 — Maturity Badge:
- Count symbols per lifecycle category (Architecture/Implementation/Integration/Testing/Operations)
- Pick dominant. One-line description as pill badge.
- Detection: traits→Architecture, Core Logic functions→Implementation,
  Connector symbols→Integration, Test layer→Testing, config/logging→Operations

SECTION 3 — Tech Stack Discovery:
- Parse Cargo.toml for dependencies
- Hardcoded lookup table:
  tokio→("Language & Runtime", "Async runtime"), serde→("Serialization", "Data conversion"),
  clap→("CLI & Config", "Argument parsing"), tracing→("Observability", "Logging"),
  axum→("Networking", "HTTP framework"), bytes→("Wire Format", "Byte buffers"),
  anyhow→("Error Handling", "Error propagation"), thiserror→("Error Handling", "Custom errors"),
  sqlx/rusqlite/surrealdb→("Data Storage", ...), reqwest/hyper→("Networking", ...), etc.

SECTION 4 — Project Layers + Layer Narratives + File Drill-Down:
- Categorize every symbol into a layer:
  1. tests/→🧪Tests, 2. bin/main.rs/cli→🌐Interface, 3. server/handler/route/api→🌐Interface,
  4. client/connector/provider→🔌Connectors, 5. db/store/repo/cache/state→💾Data,
  6. frame/parse/codec/wire/proto→📦Wire Format, 7. cmd/command/service→⚙️Core Logic,
  8. SIR "utility"/"helper"→🔧Utilities, 9. Default→⚙️Core Logic
- For each layer: call compose_layer_narrative() from narrative module
- For each file in layer: call compose_file_summary()
- CACHE layer assignments in SharedState or a dashmap — Tour, Glossary, Deep Dives,
  Difficulty Radar, and Decomposer all need this data. Cache key: SIR count.

SECTION 5 — Key Actors:
- PageRank via aether-graph-algo (spawn_blocking!)
- Top 5-10 by centrality with SIR intents

SECTION 6 — Simplified Graph:
- Aggregate dependencies to layer level, 5-8 nodes

=== HTMX FRAGMENTS ===

GET /dashboard/frag/anatomy — Full page
GET /dashboard/frag/anatomy/layer?name=Interface — Expanded file list with layer narrative
GET /dashboard/frag/anatomy/file?path=src/server.rs — Symbol list for a file

IMPORTANT — SYMBOL LINKS: Every symbol name rendered in the anatomy page
(in key actors, in file drill-downs, anywhere) must be a clickable link to
the Symbol Deep Dive page (Phase E). For now, render them as styled spans
with a data-symbol attribute and a class "symbol-link". Phase E will add
the click handler. Pattern:

  span class="symbol-link text-blue-600 cursor-pointer" data-symbol="Db" { "Db" }

=== SIDEBAR ===

Add "📖 Anatomy" FIRST in sidebar, above Overview.
Update welcome banner links from Phase B.

=== VALIDATION ===

1. /api/v1/anatomy returns valid JSON with all sections
2. Anatomy page renders, no deadlock
3. Project summary is readable English
4. Maturity badge shows (e.g., "⚙️ Implementation Project")
5. Tech stack cards categorized from Cargo.toml
6. Each layer has a narrative paragraph (not just a list)
7. Click layer → file list with file summaries
8. Click file → symbol list with SIR intents
9. Key actors show centrality symbols
10. Simplified graph renders with 5-8 nodes
11. Layer narrative reads as a coherent paragraph, not bullet points
12. File summaries are 1-2 sentences, not raw SIR dump
13. Symbol names are styled as clickable (even if handler not yet wired)

cargo fmt --all --check
cargo clippy --workspace --features dashboard -- -D warnings
cargo test -p aether-dashboard
cargo test -p aetherd --features dashboard
```

---

## Phase D: Dynamic Guided Tour + Glossary

```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- export CARGO_TARGET_DIR=/home/rephu/aether-target
- export CARGO_BUILD_JOBS=1
- export PROTOC=$(which protoc)
- export TMPDIR=/home/rephu/aether-target/tmp
- mkdir -p $TMPDIR

PREREQUISITE: Phases A, B, and C must be merged to main first.
TECHNOLOGY: HTMX + D3.js + Tailwind CSS (all from CDN). Maud for fragments.

=== PAGE 1: DYNAMIC GUIDED TOUR (/dashboard/tour) ===

PURPOSE: Step-by-step walkthrough like a museum audio guide. CRITICAL: stops
are DYNAMICALLY generated based on which layers actually exist. A library with
no main() has no "Front Door" stop.

=== TOUR STOP TEMPLATES ===

Each included only if condition is true:

"The Front Door" (Entry Point)
  Condition: Interface layer has main/bin symbols
  Show: Entry point symbols with SIR intents

"What It Accepts" (Input Processing)
  Condition: Interface layer has server/handler symbols (separate from entry points)
  Show: Non-entry-point Interface symbols

"How It Thinks" (Core Logic)
  Condition: Core Logic layer exists (almost always true)
  Show: Core Logic symbols grouped by function

"Where It Stores Things" (Data)
  Condition: Data layer exists
  Show: Data layer symbols

"How It Talks" (Wire Format + Connectors)
  Condition: Wire Format OR Connector layer exists
  Show: Combined Wire Format + Connector symbols

"How It Handles Problems" (Error Handling)
  Condition: ANY symbol has non-empty error_modes in SIR
  Show: Symbols with rich error_modes, grouped by file

"How It Gets Tested" (Testing)
  Condition: Tests layer exists
  Show: Test layer symbols

"The Utilities"
  Condition: Utilities layer has >= 3 symbols
  Show: Utility symbols

GENERATION RULES:
1. Evaluate each condition against cached layer data from Phase C
2. Include only templates whose condition is true
3. Number sequentially
4. Minimum 2 stops (fallback: Core Logic + Error Handling)
5. Maximum 8 stops (drop Utilities first, then merge What It Accepts into Front Door)
6. Order: entry → input → logic → data → output → errors → tests → utilities

Each stop DESCRIPTION is composed using narrative module functions:
- Take top 2-3 symbols for that stop
- Call compose_file_summary() for their files
- Compose 2-3 sentences: what this part does, which components are involved

=== TOUR API: GET /api/v1/tour ===

{
  "data": {
    "stop_count": 5,
    "stops": [
      {
        "number": 1,
        "title": "The Front Door",
        "subtitle": "Entry Point",
        "description": "This is where mini-redis starts. The main function...",
        "symbols": [ { "name": "main", "file": "...", "sir_intent": "..." } ],
        "layer": "Interface",
        "file_count": 2, "symbol_count": 3
      }
    ],
    "skipped_stops": ["The Utilities"]
  }
}

=== TOUR HTMX: GET /dashboard/frag/tour ===

Layout:
- Left: numbered stop list (only generated stops), clickable
- Main: current stop content
- Each stop: number badge, title, subtitle, narrative description, symbol list
  with SIR intents, file links
- Previous/Next buttons (hide at boundaries)
- Progress bar (filled segments for included stops only)
- HTMX: hx-get="/dashboard/frag/tour?stop=N" hx-target="#tour-content"

EDGE CASE: < 2 qualifying stops → single "Overview" stop listing all symbols.

SYMBOL LINKS: All symbol names in tour stops use the "symbol-link" pattern
from Phase C. Phase E will wire the click handler.

=== PAGE 2: GLOSSARY (/dashboard/glossary) ===

Auto-generated dictionary. Every type, trait, function with SIR intent as definition.

=== GLOSSARY API: GET /api/v1/glossary ===

Parameters: ?search=term, ?layer=Core+Logic, ?kind=struct, ?page=1&per_page=50

{
  "data": {
    "terms": [
      {
        "name": "Command",
        "kind": "enum",
        "file": "src/cmd/mod.rs",
        "layer": "Core Logic",
        "layer_icon": "⚙️",
        "definition": "Dispatches the command to its specific implementation...",
        "related": ["Get", "Set", "Publish", "Subscribe"],
        "dependents_count": 8
      }
    ],
    "total": 60, "page": 1, "per_page": 50
  }
}

=== GLOSSARY HTMX: GET /dashboard/frag/glossary ===

Layout:
- Search bar with HTMX search-as-you-type:
  hx-get="/dashboard/frag/glossary?search={value}" hx-trigger="input changed delay:300ms"
- Layer filter buttons, kind filter buttons
- Alphabetized card list:
  Name (large), Kind badge, Layer badge, File path, Definition, Related terms
  Disabled "📋 Spec" button (Phase E enables)
  Disabled "🎓 Advisor" button (Phase H enables)
- Pagination

SYMBOL LINKS: Each term name is a "symbol-link". Related term names are also
symbol-links. Phase E wires the click handler to Symbol Deep Dive.

=== SIDEBAR ===

  📖 Anatomy
  🗺️ Tour
  📚 Glossary
  --- separator ---
  Overview, Graph, Health, Coupling, Drift (existing)

=== VALIDATION ===

TOUR:
1. /dashboard/tour loads with dynamic stops
2. mini-redis: expect 5-7 stops
3. Library crate without main(): no "Front Door" stop
4. Each stop has narrative description (sentences, not bullet lists)
5. Next/Previous work, progress bar updates
6. Symbol names are clickable-styled

GLOSSARY:
1. /dashboard/glossary loads all terms alphabetically
2. Search filters in real-time
3. Layer/kind filters work
4. Each term shows definition from SIR intent
5. Pagination works
6. "Spec" and "Advisor" buttons visible but disabled

cargo fmt --all --check
cargo clippy --workspace --features dashboard -- -D warnings
cargo test -p aether-dashboard
cargo test -p aetherd --features dashboard
```
