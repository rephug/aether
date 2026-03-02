# AETHER Dashboard Revision — Run 2: Project Anatomy + Layer Narratives

**Phase:** C (Project Anatomy + Layer Narratives)
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

PREREQUISITE: Run 1 (bug fixes + plain English layer) must be merged to main first.
TECHNOLOGY: HTMX + D3.js + Tailwind CSS (all from CDN). NO React, NO Node.js.
Maud for server-side HTML fragments. rust-embed for static files.

Read docs/roadmap/DASHBOARD_REVISIONS_SESSION_CONTEXT_v3.md for full project context.

=== CONTEXT ===

The Project Anatomy page is the highest-value new page. It's the "ingredients
list" for a codebase. This phase also introduces the LAYER NARRATIVE engine
that composes plain English paragraphs explaining how each layer works as a
unit. Layer narratives are reused by Tour (Run 3), Deep Dives (Run 3),
and the LLM suite (Run 5), so the narrative composition logic should
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
  — Returns "Weak"/"Moderate"/"Strong"/"Very Strong" (reuse from Run 1)

fn qualify_difficulty(error_count: usize, side_effect_count: usize, dep_count: usize, is_async: bool) -> (&'static str, &'static str)
  — Returns (emoji, label) for LLM difficulty: ("🟢", "Easy") etc. (used in Run 5)

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
the Symbol Deep Dive page (Run 3). For now, render them as styled spans
with a data-symbol attribute and a class "symbol-link". Run 3 will add
the click handler. Pattern:

  span class="symbol-link text-blue-600 cursor-pointer" data-symbol="Db" { "Db" }

=== SIDEBAR ===

Add "📖 Anatomy" FIRST in sidebar, above Overview.
Update welcome banner links from Run 1.

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
