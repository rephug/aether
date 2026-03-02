# AETHER Dashboard Revision — Run 4: What Changed Recently + Ask AETHER

**Phase:** F (What Changed Recently + Ask AETHER)
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

PREREQUISITE: Run 2 (Anatomy — layer data) must be merged to main.
             Run 3 (Tour, Glossary, Deep Dives) is helpful but not required.
TECHNOLOGY: HTMX + D3.js + Tailwind CSS (all from CDN). NO React, NO Node.js.
Maud for server-side HTML fragments. rust-embed for static files.

Read docs/roadmap/DASHBOARD_REVISIONS_SESSION_CONTEXT_v3.md for full project context.

=== OVERVIEW ===

Two independent features:
  FEATURE 1: What Changed Recently — timeline of changes with semantic context
  FEATURE 2: Ask AETHER — global search bar using the same logic as `aetherd ask`

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

Add: 🕐 Recent Changes (after Glossary, before existing pages)

Ask box is NOT a sidebar item — it's always visible globally.

========================================================================
VALIDATION
========================================================================

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
9. Ask box visible on every page
10. Type question + Enter → results appear inline
11. Summary paragraph is readable English
12. Related components listed with symbol-links
13. Multiple questions: new results replace previous
14. Empty query → graceful message
15. Navigating pages doesn't lose the ask box
16. If no index data → appropriate message

cargo fmt --all --check
cargo clippy --workspace --features dashboard -- -D warnings
cargo test -p aether-dashboard
cargo test -p aetherd --features dashboard
```
