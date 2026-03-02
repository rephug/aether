# AETHER Dashboard Revision — Codex Prompts v3, Part 2 of 3

**Phases covered:** E (Narrative Engine: Deep Dives + Flow), F (What Changed + Ask AETHER)
**Date:** March 2026
**Companion files:**
- `DASHBOARD_REVISIONS_SESSION_CONTEXT_v3.md` — full context
- `DASHBOARD_REVISION_CODEX_PROMPTS_v3_part1.md` — Phases A–D
- `DASHBOARD_REVISION_CODEX_PROMPTS_v3_part3.md` — Phases G–H

---

## Phase E: Narrative Engine — Symbol Deep Dive, File Deep Dive, Flow Narrative

```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- export CARGO_TARGET_DIR=/home/rephu/aether-target
- export CARGO_BUILD_JOBS=1
- export PROTOC=$(which protoc)
- export TMPDIR=/home/rephu/aether-target/tmp
- mkdir -p $TMPDIR

PREREQUISITE: Phases A, B, and C must be merged to main first. Phase D is
helpful (Glossary links) but not strictly required.

TECHNOLOGY: HTMX + D3.js + Tailwind CSS (all from CDN). Maud for fragments.

=== CONTEXT ===

This phase adds the "click anything and understand it" narrative layer. Three
features that transform the dashboard from a data display into a storytelling
tool. ALL narrative text is composed from existing SIR data + dependency graph +
layer categorization using the narrative module created in Phase C. No new
inference calls.

After this phase, every symbol name anywhere in the dashboard becomes a
clickable link to a full narrative report.

=== IMPORTANT: WIRE UP SYMBOL LINKS ===

Phase C created "symbol-link" styled spans with data-symbol attributes.
Phase D added more of them in Tour and Glossary. This phase must:

1. Add a global JavaScript handler in index.html that intercepts clicks on
   any element with class "symbol-link":

   document.body.addEventListener('click', function(e) {
     const link = e.target.closest('.symbol-link');
     if (link) {
       const symbol = link.dataset.symbol;
       htmx.ajax('GET', '/dashboard/frag/symbol/' + encodeURIComponent(symbol),
         { target: '#main-content' });
     }
   });

2. Convert ALL existing symbol-link spans in Phase C and D code to use this
   pattern consistently.

3. Going forward, every symbol name rendered anywhere in the dashboard must
   use this pattern:
   span class="symbol-link text-blue-600 hover:underline cursor-pointer"
     data-symbol=(name) { (name) }

=== FEATURE 1: SYMBOL DEEP DIVE ===

PURPOSE: The single most important narrative feature. Click any symbol name
anywhere in the dashboard and get a full story — not just a definition, but
a complete narrative of what it is, how it fits, who uses it, what it depends
on, what could go wrong, and how an LLM would approach building it.

=== API ENDPOINT: GET /api/v1/symbol/{name} ===

Given a symbol name (URL-encoded), return a comprehensive narrative report.

{
  "data": {
    "name": "Db",
    "kind": "struct",
    "file": "src/db.rs",
    "layer": "Data",
    "layer_icon": "💾",

    "role": "Manages the shared database state including key-value storage and pub/sub channels",

    "context": "Db is the heart of mini-redis — it's the shared state that every part of the system touches. It sits in the Data layer and is the most depended-upon component in the entire project.",

    "creation_narrative": "The server creates a single Db instance at startup in src/server.rs. It's designed to be cheaply cloned — each incoming client connection gets its own handle to the same shared data through Arc.",

    "dependents": {
      "count": 12,
      "narrative": "Db is central to the project — 12 components depend on it, including all 7 command handlers in the Core Logic layer (Get, Set, Publish, Subscribe, Ping, Unknown, Command), the server's connection handler in the Interface layer, and the blocking client in the Connectors layer.",
      "by_layer": [
        { "layer": "Core Logic", "symbols": ["Get", "Set", "Publish", "Subscribe", "Ping", "Unknown", "Command"] },
        { "layer": "Interface", "symbols": ["Handler"] },
        { "layer": "Connectors", "symbols": ["BlockingClient"] }
      ]
    },

    "dependencies": {
      "count": 3,
      "narrative": "Db depends on tokio for its async runtime and background cleanup task, bytes for efficient value storage, and the standard library's HashMap for the underlying key-value and channel storage.",
      "items": [
        { "name": "tokio", "reason": "Async runtime and background task spawning" },
        { "name": "bytes", "reason": "Efficient byte value storage" },
        { "name": "HashMap", "reason": "Underlying key-value and channel storage" }
      ]
    },

    "side_effects": {
      "narrative": "Db has significant side effects to be aware of: it spawns a background task that runs continuously cleaning up expired keys, dropping the last clone triggers shutdown of that background task, and publish operations fan out to all active subscribers — a write to Db can cause network I/O across multiple connections.",
      "items": [
        "Spawns background cleanup task for expired keys",
        "Last-clone drop triggers background task shutdown",
        "Publish fans out to all subscribers (cross-connection I/O)"
      ]
    },

    "error_modes": {
      "narrative": "The main failure modes are: key not found (returns None, not an error), channel send failures when subscribers disconnect (logged but not fatal), and potential deadlock if the cleanup task and a write operation contend on the same lock.",
      "items": ["Key not found returns None", "Channel send failure on disconnect", "Potential lock contention with cleanup task"]
    },

    "blast_radius": {
      "risk_level": "High",
      "narrative": "If you change Db, 12 components across 8 files would be affected. The highest-risk changes are to the pub/sub interface, since Subscribe and Publish both depend on the exact channel management API. Changes to the key-value get/set interface are lower risk since they use a simpler API surface.",
      "affected_files": 8,
      "affected_symbols": 12
    },

    "centrality": 0.34,
    "centrality_rank": 1,
    "centrality_narrative": "Db is the most central component in this project (rank 1 of 60). It has the highest number of dependents and sits at the intersection of nearly every feature."
  },
  "meta": { "generated_at": "...", "stale": false }
}

=== IMPLEMENTING SYMBOL DEEP DIVE ===

Create: crates/aether-dashboard/src/api/symbol.rs

For each section, use the narrative module from Phase C:

ROLE: First sentence of SIR intent.

CONTEXT: compose using layer assignment + centrality rank + dependent count.
Template patterns:
  High centrality (top 10%): "{name} is the heart of {project} — it's the
    {layer_description} and is the most depended-upon component."
  Medium centrality: "{name} is an important {kind} in the {layer} layer.
    {sir_intent_first_sentence}."
  Low centrality: "{name} is a {kind} in the {layer} layer that
    {sir_intent_first_sentence}."

CREATION NARRATIVE: Trace upstream in the dependency graph. Find symbols
whose SIR explicitly references this symbol in their intent or dependencies.
Compose: "The {upstream_layer} creates/calls {name} in {file}.
{how_it_gets_used_context}."
- Use get_dependents() or get_callers() from GraphStore
- If no upstream found: "{name} is a foundational component with no upstream callers."

DEPENDENTS NARRATIVE: Call compose_dependents_narrative() from narrative module.
Group by layer for readability.

DEPENDENCIES NARRATIVE: Call compose_dependencies_narrative() from narrative module.
For each dependency, include the reason from SIR dependency annotations.

SIDE EFFECTS: From SIR side_effects field. Compose as a narrative paragraph
with "be aware of" framing.
- 0 side effects: "This is a pure component with no side effects."
- 1-2: "{name} has a side effect to be aware of: {effect}."
- 3+: "{name} has significant side effects: {narrative list}."

ERROR MODES: From SIR error_modes field. Similar composition.
- 0: "No documented failure modes."
- 1-2: "The main failure mode is: {mode}."
- 3+: "The main failure modes are: {narrative list}."

BLAST RADIUS: Compute via BFS from this symbol in the dependency graph.
Count affected symbols and files. Compose risk level:
- 0-2 affected: "Low" — "Changes to {name} are well-contained."
- 3-8 affected: "Medium" — "Changes affect {N} components."
- 9+ affected: "High" — "Changes to {name} ripple across {N} components in {M} files."
Include which specific areas are highest risk (based on layer of affected symbols).

CENTRALITY: From PageRank. Rank among all symbols.
- Top 10%: "most central component"
- Top 25%: "highly central"
- Top 50%: "moderately connected"
- Bottom 50%: "relatively independent"

=== HTMX FRAGMENT: GET /dashboard/frag/symbol/{name} ===

Full narrative page in maud. Layout:

HEADER CARD (bg-white rounded-lg shadow p-6):
  Name (large, h1) + Kind badge (struct/enum/fn/trait) + Layer badge with icon
  File path (small, clickable → File Deep Dive)
  One-line role (from SIR first sentence)

CONTEXT SECTION:
  "How It Fits" (h2)
  Context paragraph
  Centrality narrative

CREATION SECTION:
  "How It Gets Used" (h2)
  Creation narrative paragraph

CONNECTIONS SECTION (two columns):
  Left: "What Depends on This" (h3) + dependents narrative + grouped list
  Right: "What This Depends On" (h3) + dependencies narrative + list

RISKS SECTION:
  "Side Effects & Risks" (h2)
  Side effects narrative
  Error modes narrative
  Blast radius card with risk level badge (Low=green, Medium=yellow, High=red)

BOTTOM CARDS (horizontal row, links to future phases):
  "📋 Generate Spec" → /dashboard/frag/spec/{name} (Phase G)
  "🎓 Prompt Advisor" → /dashboard/frag/advisor/{name} (Phase H)
  "🔄 Trace Flow" → /dashboard/frag/flow?start={name} (Flow Narrative below)

  If Phase G/H not yet built, render these as disabled gray buttons with tooltips.

ALL symbol names in the deep dive are themselves symbol-links (clickable to
their own deep dive). This creates a Wikipedia-like browsing experience.

=== FEATURE 2: FILE DEEP DIVE ===

PURPOSE: When someone clicks a file path anywhere in the dashboard, show the
full story of that file — not just a list of its symbols, but how they relate
to each other and how the file connects to the project.

=== API ENDPOINT: GET /api/v1/file/{path} ===

Path is URL-encoded (e.g., /api/v1/file/src%2Fdb.rs).

{
  "data": {
    "path": "src/db.rs",
    "layer": "Data",
    "layer_icon": "💾",
    "symbol_count": 6,

    "summary": "src/db.rs is the data backbone of mini-redis. It defines the shared state that every client connection accesses...",

    "internal_narrative": "The file is organized around two key types: Db (the public-facing handle) wraps DbState (the internal storage) through an Arc for shared access. DbDropGuard ensures cleanup when the last handle is dropped. The purge_expired_keys function runs as a background task, periodically scanning for and removing expired entries. State holds the raw HashMap storage and broadcast channels.",

    "external_narrative": "This file is depended upon by 8 other files in the project. Every command handler in src/cmd/ receives a Db reference to execute operations. The server in src/server.rs creates the initial instance and clones it for each connection. The blocking client in src/blocking/ wraps Db for synchronous access.",

    "symbols": [
      {
        "name": "Db",
        "kind": "struct",
        "sir_intent": "Manages shared database state...",
        "centrality": 0.34,
        "dependents_count": 12,
        "internal_connections": ["DbState", "DbDropGuard"],
        "role_in_file": "Primary public interface — the handle other files use"
      },
      {
        "name": "DbState",
        "kind": "struct",
        "sir_intent": "Internal storage holding the actual data...",
        "centrality": 0.05,
        "dependents_count": 1,
        "internal_connections": ["Db"],
        "role_in_file": "Internal implementation detail — only accessed through Db"
      }
    ],

    "connections_to_project": {
      "depended_on_by": ["src/server.rs", "src/cmd/get.rs", "src/cmd/set.rs", "src/cmd/publish.rs", "src/cmd/subscribe.rs"],
      "depends_on": ["tokio", "bytes"]
    }
  }
}

=== IMPLEMENTING FILE DEEP DIVE ===

Create: crates/aether-dashboard/src/api/file.rs

SUMMARY: Call compose_file_summary() from narrative module, but use the
extended version (2-3 sentences for file deep dive vs 1 sentence for anatomy).

INTERNAL NARRATIVE: This is new. Describe how the symbols WITHIN the file
relate to each other.
1. Find all symbols in the file from SqliteStore
2. Find internal dependencies (symbol A in this file depends on symbol B in this file)
3. Identify the "primary" symbol (highest centrality) and "supporting" symbols
4. Compose:
   "The file is organized around {primary_type}: {primary_name} ({primary_role}).
   {supporting_narrative}. {relationship_narrative}."

   Template for supporting symbols:
   1 supporting: "{name} {relationship_to_primary}."
   2-3 supporting: "{name1} and {name2} {relationship}."
   4+: "Supporting types include {name1} ({role}), {name2} ({role}), and {N} others."

EXTERNAL NARRATIVE: How this file connects to the rest of the project.
1. Find all files that depend on symbols in this file
2. Group by layer
3. Compose: "This file is depended upon by {N} other files. {grouped_narrative}."

ROLE IN FILE: For each symbol, describe its role within the file:
- Highest centrality: "Primary public interface"
- Only used internally: "Internal implementation detail"
- Used by primary: "Supporting type for {primary_name}"
- Test: "Test for {tested_symbol}"

=== HTMX FRAGMENT: GET /dashboard/frag/file/{path} ===

Layout:

HEADER:
  File path (large) + Layer badge + Symbol count badge
  Summary paragraph (2-3 sentences)

HOW THIS FILE WORKS (h2):
  Internal narrative paragraph
  Visual: small D3 graph showing only symbols in this file and their
  internal connections (optional — skip if too complex for this phase)

HOW THIS FILE CONNECTS (h2):
  External narrative paragraph
  Two lists: "Depended on by" (files with layer badges) and "Depends on" (deps)

ALL COMPONENTS IN THIS FILE (h2):
  Card list of symbols, each showing:
  Name (symbol-link), Kind badge, Role in file badge, SIR intent,
  Centrality if top 20%, Internal connections

FILE PATH LINKS: Add click handler similar to symbol links. Any file path
rendered in the dashboard becomes clickable:

  document.body.addEventListener('click', function(e) {
    const link = e.target.closest('.file-link');
    if (link) {
      const path = link.dataset.path;
      htmx.ajax('GET', '/dashboard/frag/file/' + encodeURIComponent(path),
        { target: '#main-content' });
    }
  });

=== FEATURE 3: FLOW NARRATIVE ===

PURPOSE: Trace a data path through the codebase and narrate each step.
"How does a client request become a response?" answered as a numbered story.

=== API ENDPOINT: GET /api/v1/flow?start={symbol}&end={symbol} ===

If only start is provided, trace downstream (follow dependencies of dependencies)
up to 10 hops or until a leaf node.

If both start and end are provided, find the shortest path between them in
the dependency graph.

{
  "data": {
    "start": "run",
    "end": "Db",
    "step_count": 4,
    "steps": [
      {
        "number": 1,
        "symbol": "run",
        "file": "src/server.rs",
        "layer": "Interface",
        "layer_icon": "🌐",
        "narrative": "A TCP connection arrives at the server's run function in the Interface layer. It accepts the connection and spawns a new Handler task to process it.",
        "sir_intent": "Accepts incoming TCP connections and spawns per-connection handlers",
        "transition": "The Handler is created with a clone of the shared Db"
      },
      {
        "number": 2,
        "symbol": "Handler",
        "file": "src/server.rs",
        "layer": "Interface",
        "layer_icon": "🌐",
        "narrative": "The Handler reads a Frame from the Connection, which parses the raw bytes into a structured Redis command.",
        "sir_intent": "Per-connection handler that reads commands and writes responses",
        "transition": "The parsed frame is passed to Command::from_frame"
      },
      {
        "number": 3,
        "symbol": "Command",
        "file": "src/cmd/mod.rs",
        "layer": "Core Logic",
        "layer_icon": "⚙️",
        "narrative": "The Command enum identifies the command type (Get, Set, Publish, etc.) and delegates to the specific handler's apply method.",
        "sir_intent": "Dispatches parsed commands to their specific implementations",
        "transition": "The command's apply method receives the shared Db"
      },
      {
        "number": 4,
        "symbol": "Db",
        "file": "src/db.rs",
        "layer": "Data",
        "layer_icon": "💾",
        "narrative": "The Db processes the operation — reading a value, storing a value, or managing a pub/sub subscription — and returns the result.",
        "sir_intent": "Manages shared database state including key-value and pub/sub",
        "transition": null
      }
    ],
    "summary": "This flow shows how a client request travels from the network interface through command parsing to the data layer. The Interface layer handles connection management, the Core Logic layer identifies and dispatches the command, and the Data layer executes the actual operation."
  }
}

=== IMPLEMENTING FLOW NARRATIVE ===

Create: crates/aether-dashboard/src/api/flow.rs

PATH FINDING:
- If start + end provided: Use BFS or Dijkstra from aether-graph-algo to find
  shortest path. If no path exists, return error: "No connection found between
  {start} and {end}."
- If start only: Use BFS from start, following dependency edges (outgoing).
  Stop at depth 10 or when reaching a node with 0 outgoing edges.
  Select the "most interesting" path — prefer paths that cross layer boundaries
  and include high-centrality nodes. If multiple paths exist, pick the one
  that visits the most distinct layers.

NARRATIVE PER STEP:
For each symbol on the path:
1. Get layer assignment (from cached layer data)
2. Get SIR intent
3. Compose step narrative using template:
   First step: "{description of what initiates the flow} at {name} in the {layer} layer."
   Middle steps: "{name} {sir_intent_reworded_as_action}."
   Last step: "{name} {sir_intent_as_conclusion} and returns the result."

TRANSITION TEXT:
Between each step, compose how data moves from one to the next:
1. Check the dependency type (direct call, passed as parameter, shared state)
2. Template: "The {output} is {verb} to {next_name}"
   Verbs by relationship: "passed to", "received by", "accessed through", "triggers"

SUMMARY:
Compose from the layers visited:
"This flow shows how {start_description} travels from the {first_layer} through
{middle_layers} to the {last_layer}. {layer_role_summaries}."

=== HTMX FRAGMENT: GET /dashboard/frag/flow ===

QUERY PARAMS: ?start={symbol} and optionally ?end={symbol}

If no params, show the flow builder UI:
- "Start from" symbol typeahead (reuse search endpoint)
- "End at" symbol typeahead (optional)
- "Trace Flow" button
- HTMX: hx-get="/dashboard/frag/flow?start={start}&end={end}"

If params provided, show the flow visualization:

Layout:
- Summary paragraph at top (highlighted card)
- Vertical timeline (like a git log visualization):
  Each step is a card connected by a vertical line
  Card shows: Step number, Symbol name (symbol-link), Layer badge,
  Narrative paragraph, SIR intent (smaller, italic)
  Between cards: Transition text in a connector element
- Layer color coding: each step's left border matches its layer color

SUGGESTED FLOWS: Below the flow builder, show 3-4 suggested starting points
based on entry-point symbols (Interface layer, main functions):
"Try tracing from: run (server entry), main (program start), Command (request dispatch)"
Each is a clickable link that pre-fills the start field.

=== SIDEBAR ===

No new sidebar entry for Deep Dives — they're accessed by clicking symbol/file
names throughout the dashboard.

Add "🔄 Trace Flow" to sidebar (between Tour and Glossary):
  📖 Anatomy
  🗺️ Tour
  🔄 Trace Flow
  📚 Glossary

=== VALIDATION ===

SYMBOL DEEP DIVE:
1. Click any symbol name in Glossary → full narrative report loads
2. Click symbol in Anatomy key actors → same report
3. Click symbol in Tour → same report
4. Report has all sections: role, context, creation, dependents, dependencies,
   side effects, error modes, blast radius, centrality
5. Dependents grouped by layer with narrative paragraph
6. All symbol names within the deep dive are themselves clickable
7. File path is clickable → goes to File Deep Dive
8. Disabled buttons for Spec (Phase G) and Advisor (Phase H) visible

FILE DEEP DIVE:
9. Click file path anywhere → full file narrative loads
10. Internal narrative explains how symbols relate within the file
11. External narrative explains connections to rest of project
12. Symbol list shows role-in-file badges
13. All symbol names are clickable → Symbol Deep Dive

FLOW NARRATIVE:
14. /dashboard/flow loads with builder UI and suggested starting points
15. Enter a start symbol → trace renders as vertical timeline
16. Each step has narrative paragraph + layer badge + transition text
17. Summary paragraph reads as coherent English
18. Enter start + end → shows shortest path between them
19. "No connection found" for unrelated symbols
20. All symbol names in flow steps are clickable

CROSS-CUTTING:
21. Symbol-link click handler works from ALL pages (Anatomy, Tour, Glossary,
    Graph, Health, Coupling, Overview, Deep Dive itself)
22. File-link click handler works from Deep Dive and Anatomy
23. Browser back/forward works with HTMX history (hx-push-url)

cargo fmt --all --check
cargo clippy --workspace --features dashboard -- -D warnings
cargo test -p aether-dashboard
cargo test -p aetherd --features dashboard
```

---

## Phase F: What Changed Recently + Ask AETHER

```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- export CARGO_TARGET_DIR=/home/rephu/aether-target
- export CARGO_BUILD_JOBS=1
- export PROTOC=$(which protoc)
- export TMPDIR=/home/rephu/aether-target/tmp
- mkdir -p $TMPDIR

PREREQUISITE: Phase C must be merged (for layer data). Independent of D and E.
TECHNOLOGY: HTMX + D3.js + Tailwind CSS (all from CDN). Maud for fragments.

=== FEATURE 1: WHAT CHANGED RECENTLY ===

PURPOSE: First thing a returning user wants to know: "what happened since I
last looked?" Frames changes as a narrative timeline, not a technical diff.

=== API ENDPOINT: GET /api/v1/changes ===

Parameters:
- ?since=24h (default; supports 1h, 24h, 7d, 30d)
- ?limit=20

Data sources (combine all available):
- File modification times: stat indexed files for mtime
- SIR generation timestamps: mtime of .aether/sir/*.json files
- Git log (if available):
  std::process::Command::new("git")
    .args(["log", "--since=24 hours ago", "--format=%H|%s|%an|%aI", "--name-only"])
  Gracefully handle non-git workspaces (git rev-parse returns error → skip)
- Drift data from Phase 6 analytics if available

Response:
{
  "data": {
    "period": "24h",
    "change_count": 5,
    "changes": [
      {
        "timestamp": "2026-03-01T14:30:00Z",
        "type": "file_modified",
        "file": "src/server.rs",
        "layer": "Interface",
        "layer_icon": "🌐",
        "summary": "Connection handling updated — affects 4 components in the Interface layer",
        "symbols_affected": ["Handler", "Handler::run", "Listener", "run"],
        "git_message": "fix: handle connection timeout gracefully",
        "git_author": "alice"
      }
    ],
    "file_summary": {
      "files_changed": 3,
      "symbols_affected": 12,
      "layers_touched": ["Interface", "Data"]
    }
  }
}

Summary templates per change type:
- file_modified: "{file} updated — affects {N} components in the {layer} layer"
- sir_generated: "AETHER analyzed {N} components in {file} for the first time"
- sir_updated: "AETHER's understanding of {N} components in {file} was refreshed"
- file_added: "New file {file} added to the {layer} layer with {N} components"
- file_deleted: "{file} removed — {N} components no longer tracked"

=== HTMX FRAGMENT: GET /dashboard/frag/changes ===

Layout:
- Time selector: pill buttons (1h, 24h, 7d, 30d)
  hx-get="/dashboard/frag/changes?since=7d" hx-target="#changes-content"
- Summary card: "{N} files changed affecting {M} components across {K} layers"
- Timeline (newest first):
  Each entry: timestamp (relative), layer badge, file name (file-link),
  summary sentence, git commit message (quote block if available),
  expandable symbol list (symbol-links)
- Empty state: "No changes in the last {period}. The codebase is stable. ✨"

PLACEMENT:
1. Section on Overview page between welcome banner and stats (most prominent)
2. Dedicated sidebar link: "🕐 Recent Changes" → full page version

=== FEATURE 2: ASK AETHER — Direct Query Box ===

PURPOSE: Instead of building prompts for external tools, ask questions
directly from the dashboard. The query calls the same logic as `aetherd ask`.

=== API ENDPOINT: POST /api/v1/ask ===

Request: { "question": "how does pub/sub work?" }

Server-side logic:
1. Parse question
2. Call the same search/ask function as `aetherd ask "..."`
   This is a LOCAL function call, NOT MCP protocol.
   Find: crates/aetherd/src/commands/ask.rs or wherever run_ask_command lives
3. Format results as JSON

Response:
{
  "data": {
    "question": "how does pub/sub work?",
    "answer_type": "search_results",
    "results": [
      {
        "symbol": "Subscribe",
        "file": "src/cmd/subscribe.rs",
        "layer": "Core Logic",
        "relevance": 0.92,
        "sir_intent": "Subscribes a client to one or more channels...",
        "sir_dependencies": ["Connection", "Db", "Shutdown"],
        "sir_error_modes": ["Channel send failure", "Connection dropped"]
      }
    ],
    "summary": "Pub/sub in this project is implemented through the Subscribe and Publish commands. Subscribe registers a client connection to receive messages on specified channels. Publish sends a message to all active subscribers. The shared Db struct manages the subscriber registry."
  }
}

Summary composition:
- Take top 3-5 results by relevance
- Use their SIR intents to compose 2-4 sentences using narrative module
- Template: "[Topic] is implemented through [top results]. [First SIR sentence].
  [Second SIR sentence]. [Key dependency] manages [state]."

=== HTMX FRAGMENT: POST /dashboard/frag/ask ===

Returns answer as HTML fragment:
- Summary paragraph in highlighted card
- "Related Components" list: compact cards with symbol name (symbol-link),
  file (file-link), layer badge, relevance score, SIR intent first sentence
- Each card links to Symbol Deep Dive via symbol-link pattern

=== ASK BOX PLACEMENT ===

GLOBAL: Top of main content area on EVERY page. Rendered in the index.html
shell, not in individual fragments. Prominent search bar:

<div id="ask-container" class="mb-6 px-6 pt-4">
  <div class="relative">
    <input type="text" id="ask-input" name="question"
      placeholder="Ask about this codebase..."
      class="w-full p-4 text-lg border-2 border-blue-200 rounded-lg
             focus:border-blue-500 focus:ring-2 focus:ring-blue-200"
      hx-post="/dashboard/frag/ask"
      hx-trigger="keyup[key=='Enter']"
      hx-target="#ask-results"
      hx-indicator="#ask-spinner">
    <div id="ask-spinner" class="htmx-indicator absolute right-4 top-4">
      <!-- loading spinner -->
    </div>
  </div>
  <div id="ask-results" class="mt-4"></div>
</div>

Position: ABOVE the #main-content div so it persists across page navigations.
Results appear below the search bar, pushing page content down.

=== MCP NOT AVAILABLE HANDLING ===

If the ask function is unavailable (the search modules aren't loaded,
aetherd started in minimal mode):
- Ask box still visible but grayed out with reduced opacity
- Placeholder text: "Direct questions require indexing — run aetherd --index first"
- On submission, return fragment: "AETHER needs to index this project before
  it can answer questions. Run: aetherd --workspace . --index-once"

Check: The dashboard's SharedState has access to SqliteStore and VectorStore.
The ask function likely needs at least symbol search capability. If SIR count
is 0, show: "No analysis data yet. Index the project first."

=== SIDEBAR ===

Add: 🕐 Recent Changes (between Glossary and existing pages)

Ask box is NOT a sidebar item — it's always visible globally.

=== VALIDATION ===

CHANGES:
1. /api/v1/changes returns valid JSON
2. Overview shows "What Changed Recently" section
3. Time selector filters results
4. Each entry has layer badge, file, narrative summary
5. Git messages appear when available
6. Symbol names and file names are clickable
7. "🕐 Recent Changes" sidebar → full page timeline
8. Empty state message when no changes

ASK:
1. Ask box visible on every page
2. Type question + Enter → results appear inline
3. Summary paragraph is readable English
4. Related components listed with symbol-links
5. Multiple questions: new results replace previous
6. Empty query → graceful message
7. Navigating pages doesn't lose the ask box
8. If no index data → appropriate message

cargo fmt --all --check
cargo clippy --workspace --features dashboard -- -D warnings
cargo test -p aether-dashboard
cargo test -p aetherd --features dashboard
```
