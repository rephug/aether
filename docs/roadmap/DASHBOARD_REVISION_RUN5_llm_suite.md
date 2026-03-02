# AETHER Dashboard Revision — Run 5: LLM Collaboration Suite

**Phases combined:** G (Difficulty Radar + Prompt Builder + Context Advisor) + H (Decomposer + Checkpoints + Autopsy)
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

PREREQUISITES: Runs 1-3 must be merged to main:
- Run 1: Bug fixes + plain English layer
- Run 2: Anatomy + narrative module + layer caching
- Run 3: Tour + Glossary + Symbol Deep Dive + File Deep Dive + Flow Narrative
Run 4 (Changes + Ask) is independent and not required.

TECHNOLOGY: HTMX + D3.js + Tailwind CSS (all from CDN). NO React, NO Node.js.
Maud for server-side HTML fragments. rust-embed for static files.

Read docs/roadmap/DASHBOARD_REVISIONS_SESSION_CONTEXT_v3.md for full project context.

=== OVERVIEW ===

This run builds the complete LLM collaboration teaching suite — six features
that teach users how to work effectively with AI coding agents using their own
codebase as the textbook.

CRITICAL: ALL output is template-composed from existing SIR data, dependency
graph, and layer categorization. NO new inference calls. The "teaching" is
generated from data patterns, not from asking an LLM to write teaching content.

  TASK 1: LLM Difficulty Radar — score every component on LLM generation difficulty
  TASK 2: Enhanced Prompt Builder + Code-to-Spec
  TASK 3: Context Window Advisor — minimal sufficient context for LLM tasks
  TASK 4: Prompt Decomposer — "Build This in N Steps"
  TASK 5: Verification Checkpoints — what to verify after each step
  TASK 6: Prompt Autopsy — "What Would Work and What Wouldn't"

========================================================================
TASK 1: LLM DIFFICULTY RADAR
========================================================================

PURPOSE: Score every component on how hard it would be for an LLM to generate
correctly, and explain WHY. Helps users prioritize where to invest
prompting effort vs where they can fire-and-forget.

=== DIFFICULTY SCORING ALGORITHM ===

Add to the narrative module (crates/aether-dashboard/src/narrative.rs):

fn compute_difficulty(symbol: &SymbolData) -> DifficultyScore {
    let mut score: f64 = 0.0;
    let mut reasons: Vec<String> = Vec::new();

    // Factor 1: Error mode complexity (0-30 points)
    let error_count = symbol.sir.error_modes.len();
    if error_count == 0 {
        // No documented errors — simple
    } else if error_count <= 2 {
        score += 10.0;
        reasons.push(format!("{} failure modes to handle", error_count));
    } else {
        score += 30.0;
        reasons.push(format!("{} failure modes — LLMs often miss edge cases", error_count));
    }

    // Factor 2: Side effect count (0-25 points)
    let side_effect_count = symbol.sir.side_effects.len();
    if side_effect_count == 0 {
        // Pure — easy for LLMs
    } else if side_effect_count <= 1 {
        score += 10.0;
        reasons.push("has side effects that must be handled correctly".into());
    } else {
        score += 25.0;
        reasons.push(format!("{} side effects — LLMs frequently miss cleanup and notifications", side_effect_count));
    }

    // Factor 3: Dependency count (0-20 points)
    let dep_count = symbol.sir.dependencies.len();
    if dep_count <= 2 {
        // Few deps — easy context
    } else if dep_count <= 5 {
        score += 10.0;
        reasons.push(format!("{} dependencies require context in the prompt", dep_count));
    } else {
        score += 20.0;
        reasons.push(format!("{} dependencies — large context window needed", dep_count));
    }

    // Factor 4: Async/concurrent patterns (0-25 points)
    let intent_lower = symbol.sir.intent.to_lowercase();
    let is_async = intent_lower.contains("async") || intent_lower.contains("concurrent")
        || intent_lower.contains("spawn") || intent_lower.contains("lock")
        || intent_lower.contains("channel") || intent_lower.contains("select!")
        || intent_lower.contains("mutex") || intent_lower.contains("arc");
    if is_async {
        score += 25.0;
        reasons.push("async/concurrent patterns — LLMs struggle with races and lifetimes".into());
    }

    // Classify
    let (emoji, label, guidance) = if score <= 15.0 {
        ("🟢", "Easy", "Minimal prompting needed. A brief description usually produces correct code.")
    } else if score <= 40.0 {
        ("🟡", "Moderate", "Provide context and verify output. Include type signatures and key constraints.")
    } else if score <= 65.0 {
        ("🔴", "Hard", "Decompose into steps and specify edge cases explicitly. Verify each step before proceeding.")
    } else {
        ("⛔", "Very Hard", "Break into small pieces, specify control flow, enumerate all failure modes. Manual review essential.")
    };

    DifficultyScore { score, emoji, label, guidance, reasons }
}

=== DIFFICULTY DISPLAY LOCATIONS ===

1. GLOSSARY (Run 3): Add "Difficulty" column/badge to each term card.
   Show emoji + label. Hover tooltip shows reasons.

2. SYMBOL DEEP DIVE (Run 3): Add "LLM Difficulty" section before the
   bottom action buttons. Show:
   - Emoji + label (large)
   - Guidance sentence
   - Bulleted reasons list
   - "See the Prompt Advisor for guidance →" link (Task 6 below)

3. GRAPH PAGE (existing): Add difficulty as node color overlay.
   Green=Easy, Yellow=Moderate, Red=Hard, Dark Red=Very Hard.
   Toggle button: "Color by: Layer | Difficulty"
   Implement as data attribute on graph nodes, D3 color scale swap.

4. ANATOMY KEY ACTORS: Add difficulty badge next to each key actor.

5. NEW: DIFFICULTY OVERVIEW — top-level summary on a new section of the
   Overview page (or as a card on the Anatomy page):

   "LLM Difficulty Analysis"
   "🟢 Easy: {N} components ({P}%) — safe to prompt directly"
   "🟡 Moderate: {N} components ({P}%) — provide context and verify"
   "🔴 Hard: {N} components ({P}%) — decompose and specify edge cases"
   "⛔ Very Hard: {N} components ({P}%) — requires careful step-by-step prompting"

=== API ENDPOINT: GET /api/v1/difficulty ===

Returns difficulty scores for all symbols (for bulk display).

{
  "data": {
    "summary": {
      "easy": { "count": 25, "percentage": 42 },
      "moderate": { "count": 20, "percentage": 33 },
      "hard": { "count": 12, "percentage": 20 },
      "very_hard": { "count": 3, "percentage": 5 }
    },
    "symbols": [
      {
        "name": "Db",
        "difficulty": {
          "score": 72,
          "emoji": "⛔",
          "label": "Very Hard",
          "guidance": "Break into small pieces...",
          "reasons": [
            "3 failure modes — LLMs often miss edge cases",
            "3 side effects — LLMs frequently miss cleanup",
            "async/concurrent patterns — LLMs struggle with races"
          ]
        }
      }
    ]
  }
}

========================================================================
TASK 2: ENHANCED PROMPT BUILDER + CODE-TO-SPEC
========================================================================

PURPOSE: Help users construct effective questions for AI coding agents AND
generate buildable specifications from existing code.

=== PROMPT BUILDER UI ===

GET /dashboard/frag/prompts

Layout:

STEP 1: Choose a Goal — 8 cards in 2x4 grid:

| Goal | Icon | Description | MCP Tool |
|------|------|-------------|----------|
| Understand a component | 🔍 | "What does [X] do and why?" | aether_explain |
| Understand a flow | 🔄 | "How does [X] connect to [Y]?" | aether_dependencies |
| Find related code | 🔗 | "What else is related to [X]?" | aether_search |
| Assess change risk | ⚡ | "What breaks if I change [X]?" | aether_blast_radius |
| Debug a problem | 🐛 | "Why might [X] be failing?" | aether_explain + deps |
| Plan a refactor | 🏗️ | "How should I restructure [X]?" | blast_radius + coupling |
| Health check | 📊 | "What needs attention?" | aether_health |
| Understand history | 📜 | "Why was [X] built this way?" | aether_ask |

HTMX: Click card → hx-get="/dashboard/frag/prompts?goal=understand_component"

STEP 2: Select Target (except Health Check)
- Typeahead symbol search
- hx-get="/dashboard/frag/prompts/search?q={value}" hx-trigger="input changed delay:200ms"
- Suggestions: name, kind, file, SIR intent one-liner, difficulty badge

STEP 3: Generated Prompt + Copy Button
- Ready-to-copy prompt text
- Target MCP tool name
- "Copy to Clipboard" button (navigator.clipboard.writeText)

PROMPT TEMPLATES:

understand_component:
  "Explain what {symbol_name} in {file_path} does. Use the AETHER explain tool
  to get its semantic analysis, then describe its purpose, what it depends on,
  and what depends on it, in plain English."

understand_flow:
  "Trace the data flow starting from {symbol_name}. Use AETHER's dependency
  tool to show what it calls and what calls it, then explain the complete flow."

find_related:
  "Search AETHER for everything related to {symbol_name}. Show semantically
  similar symbols and dependency connections."

assess_risk:
  "What's the blast radius if I change {symbol_name} in {file_path}? Explain
  the risk level and what tests to run."

debug:
  "I'm debugging an issue with {symbol_name}. Explain what it does, its error
  modes, and trace dependencies to find likely failure points."

plan_refactor:
  "I want to refactor {symbol_name}. Show what would be affected and suggest
  a safe refactoring plan."

health_check:
  "Run an AETHER health check. Show the overall score, weakest areas, and
  what to work on first."

understand_history:
  "Why was {symbol_name} built this way? What design decisions led to its
  current structure?"

=== API ENDPOINTS ===

GET /api/v1/prompts/search?q=term — Symbol typeahead (reuse search logic)
GET /api/v1/prompts/generate?goal=X&symbol=Y — Generated prompt + MCP tool

=== CODE-TO-SPEC ===

GET /api/v1/spec/{symbol}

Generate a buildable specification from SIR data:

{
  "data": {
    "symbol": "Db",
    "kind": "struct",
    "file": "src/db.rs",
    "spec": {
      "purpose": "Manages shared database state for key-value storage and pub/sub",
      "requirements": [
        "MUST support concurrent access from multiple client connections",
        "MUST store key-value pairs with optional expiration timestamps",
        "MUST support publish/subscribe messaging across channels",
        "MUST clean up expired keys periodically"
      ],
      "inputs": ["Key-value pairs", "Channel names", "Expiration durations"],
      "outputs": ["Stored values", "Subscription messages", "Expiration notifications"],
      "dependencies": ["tokio (async runtime)", "bytes (value storage)"],
      "error_handling": ["Key not found returns None", "Channel send failure logged"]
    }
  }
}

Implementation: Compose from SIR fields:
- intent → purpose
- intent sentences split on periods → "MUST" requirements
- dependencies → dependencies with reasons
- error_modes → error_handling

Enable "📋 Spec" buttons in Glossary (Run 3 left them disabled).
Add "📋 Generate Spec" as action in Symbol Deep Dive bottom cards.

GET /dashboard/frag/spec/{symbol} — HTMX fragment rendering the spec as a
formatted card with copy button.

========================================================================
TASK 3: CONTEXT WINDOW ADVISOR
========================================================================

PURPOSE: Given a target symbol or task, compute the minimal code context an
LLM needs to generate or modify it correctly. Teaches users that context
selection is a skill.

=== API ENDPOINT: GET /api/v1/context/{symbol} ===

{
  "data": {
    "symbol": "Subscribe",
    "context_type": "generation",

    "required": [
      {
        "file": "src/cmd/mod.rs",
        "symbols": ["Command"],
        "reason": "The dispatch pattern Subscribe must integrate with",
        "estimated_lines": 12,
        "priority": "essential"
      },
      {
        "file": "src/connection.rs",
        "symbols": ["Connection"],
        "reason": "Public methods for reading/writing frames",
        "estimated_lines": 8,
        "priority": "essential"
      },
      {
        "file": "src/db.rs",
        "symbols": ["Db"],
        "reason": "Shared state API for subscribe/publish",
        "estimated_lines": 6,
        "priority": "essential"
      },
      {
        "file": "src/frame.rs",
        "symbols": ["Frame"],
        "reason": "Wire format for building responses",
        "estimated_lines": 15,
        "priority": "essential"
      }
    ],

    "helpful_but_optional": [
      {
        "file": "src/cmd/publish.rs",
        "symbols": ["Publish"],
        "reason": "Example of a similar command implementation",
        "estimated_lines": 30,
        "priority": "helpful"
      }
    ],

    "not_needed": [
      {
        "reason": "Other command implementations — useful examples but crowd the context",
        "files": ["src/cmd/get.rs", "src/cmd/set.rs", "src/cmd/ping.rs"]
      },
      {
        "reason": "Internal Db state management — only the public API matters",
        "files": ["(internal Db types)"]
      },
      {
        "reason": "Test files — not relevant for generation",
        "files": ["tests/"]
      }
    ],

    "total_required_lines": 41,
    "total_with_optional_lines": 71,
    "full_codebase_lines": 2400,
    "context_reduction": "98% smaller than providing everything",

    "teaching_note": "Context selection is a core prompting skill. The {total_required_lines} lines identified here contain everything an LLM needs to correctly generate {symbol}. The remaining {full_codebase_lines - total_required_lines} lines would dilute the signal without adding useful information. When in doubt, include only direct dependencies and their public interfaces."
  }
}

=== IMPLEMENTING CONTEXT ADVISOR ===

Create: crates/aether-dashboard/src/api/context.rs

Algorithm:
1. Get the target symbol's direct dependencies from the graph
2. For each dependency:
   a. Get its file path and symbol kind
   b. If it's a type (struct/enum/trait): include its definition (public fields/methods)
   c. If it's a function: include its signature
   d. Estimate line count from SIR or symbol metadata
   e. Mark as "essential"
3. Get ONE example peer (same kind, same layer, similar dependency pattern):
   Mark as "helpful_but_optional"
4. Everything else: categorize into "not_needed" groups with reasons
5. Compute totals and context_reduction percentage

Line estimation: If exact line counts aren't in metadata, estimate:
- struct/enum definition: 5-15 lines
- trait definition: 10-20 lines
- function signature: 2-5 lines
- full implementation: look at SIR complexity indicators

=== HTMX FRAGMENT: GET /dashboard/frag/context/{symbol} ===

Layout:
- Header: "What an LLM Needs to Build {symbol}"
- Stats bar: "Required: {N} lines | Optional: {M} lines | Full project: {K} lines | {P}% reduction"

REQUIRED section (green left border):
  Each file as a card: file path (file-link), symbol names, reason, line estimate
  "📋 Copy All Required Context" button — copies all required file paths as a list

HELPFUL section (yellow left border):
  Same card format, with "optional" tag

NOT NEEDED section (red left border, collapsed by default):
  Group cards: reason + file list
  Click to expand

TEACHING NOTE (bottom card, subtle styling):
  The teaching_note text from the API response

========================================================================
TASK 4: PROMPT DECOMPOSER — "Build This in N Steps"
========================================================================

PURPOSE: The #1 mistake with LLMs is asking for too much at once. AETHER's
dependency graph IS the optimal prompt decomposition — build the things with
no dependencies first, then layer up.

=== API ENDPOINT: GET /api/v1/decompose/{symbol} ===

For a given symbol (or file), generate an ordered sequence of
prompts that build it from the ground up.

{
  "data": {
    "target": "Db",
    "target_kind": "struct",
    "target_file": "src/db.rs",
    "step_count": 5,
    "difficulty": { "emoji": "⛔", "label": "Very Hard" },

    "preamble": "Db is rated Very Hard for LLM generation because of concurrent shared state, background tasks, and multiple access patterns. Breaking it into 5 steps ensures each prompt has clear scope and the LLM can focus on getting one thing right at a time.",

    "steps": [
      {
        "number": 1,
        "title": "The Foundation",
        "subtitle": "Define the data shape (no dependencies)",
        "symbol_target": "State",
        "difficulty": "🟢 Easy",
        "prompt": "Create a Rust struct called `State` with two fields: a `HashMap<String, Entry>` for key-value storage and a `HashMap<String, broadcast::Sender<Bytes>>` for pub/sub channels. `Entry` should hold a `Bytes` value and an optional `Instant` for expiration. Both `State` and `Entry` are internal types (not public).",
        "why_this_order": "State has no internal dependencies — it's pure data. Starting here gives the LLM a clean slate with no integration concerns.",
        "context_needed": ["bytes crate (Bytes type)", "tokio broadcast channel"],
        "expected_output": "Two structs: State and Entry, with appropriate field types",
        "checkpoints": []
      },
      {
        "number": 2,
        "title": "The Wrapper",
        "subtitle": "Create the public interface (depends on Step 1)",
        "symbol_target": "Db",
        "difficulty": "🟡 Moderate",
        "prompt": "Create a `Db` struct that wraps `Arc<SharedState>` where SharedState contains the State from step 1 plus a `Notify` for shutdown signaling. Implement `Clone` for Db (via Arc clone). Add a constructor `new()` that initializes the state and starts a background task for key expiration cleanup.",
        "why_this_order": "Db wraps State, so State must exist first. The Arc + Clone pattern is standard but the background task adds complexity — better to add it after the basic shape is right.",
        "context_needed": ["State from Step 1", "tokio (spawn, Notify)"],
        "expected_output": "Db struct with new(), Clone, and background task spawn",
        "checkpoints": []
      },
      {
        "number": 3,
        "title": "The Core Operations",
        "subtitle": "Key-value read/write (depends on Steps 1-2)",
        "symbol_target": "Db methods",
        "difficulty": "🟡 Moderate",
        "prompt": "Add methods to Db: `get(&self, key: &str) -> Option<Bytes>` reads a value if it exists and hasn't expired. `set(&mut self, key: String, value: Bytes, expire: Option<Duration>)` stores a value with optional expiration. Both methods lock the shared state, operate on the HashMap, and release the lock.",
        "why_this_order": "Get/set are the simplest operations and don't involve pub/sub. Getting these right first establishes the locking pattern.",
        "context_needed": ["Db and State from Steps 1-2"],
        "expected_output": "get() and set() methods with locking, expiration support",
        "checkpoints": []
      },
      {
        "number": 4,
        "title": "The Pub/Sub System",
        "subtitle": "Messaging layer (depends on Steps 1-3)",
        "symbol_target": "Db pub/sub methods",
        "difficulty": "🔴 Hard",
        "prompt": "Add pub/sub methods to Db: `subscribe(&self, channel: String) -> broadcast::Receiver<Bytes>` subscribes to a channel (creating the broadcast channel if it doesn't exist). `publish(&self, channel: &str, message: Bytes) -> usize` publishes to all subscribers on a channel and returns the subscriber count. Handle the case where a channel has no subscribers.",
        "why_this_order": "Pub/sub is more complex than get/set because it involves broadcast channels and must handle the no-subscribers edge case. The locking pattern from Step 3 carries over.",
        "context_needed": ["Db from Steps 1-3", "tokio broadcast"],
        "expected_output": "subscribe() and publish() methods with channel management",
        "checkpoints": []
      },
      {
        "number": 5,
        "title": "The Cleanup",
        "subtitle": "Graceful shutdown and expiration (depends on Steps 1-4)",
        "symbol_target": "purge_expired_keys + DbDropGuard",
        "difficulty": "🔴 Hard",
        "prompt": "Implement a `purge_expired_keys` async function that runs in a loop: sleep for a short duration, then lock the state and remove all entries whose expiration has passed. Also create a `DbDropGuard` wrapper that signals the background task to shut down when the last clone of Db is dropped. Use `Notify` for the shutdown signal.",
        "why_this_order": "Cleanup and shutdown depend on the full Db being correct. This step coordinates the background task lifecycle — the most error-prone part. Isolating it to the final step means earlier steps can be verified first.",
        "context_needed": ["Full Db from Steps 1-4", "tokio Notify, sleep"],
        "expected_output": "Background purge loop, DbDropGuard with Drop impl",
        "checkpoints": []
      }
    ],

    "teaching_summary": "This decomposition follows the dependency graph: data shapes first, then wrappers, then operations from simple to complex, then lifecycle management last. This pattern works for any complex type — identify the pieces with zero dependencies, build them first, then add each layer that depends on only what's already been built. The LLM gets focused scope at each step and you can verify before moving on."
  }
}

=== IMPLEMENTING THE DECOMPOSER ===

Create: crates/aether-dashboard/src/api/decompose.rs

DECOMPOSITION ALGORITHM:
1. Get the target symbol and all symbols in its file
2. Build a local dependency graph of just the symbols in this file + their
   direct internal dependencies
3. Topological sort this local graph (use aether-graph-algo if available,
   otherwise implement Kahn's algorithm)
4. Group the sorted nodes into logical steps:
   a. Symbols with 0 internal dependencies → Step 1 "The Foundation"
   b. Symbols that depend only on Step 1 → Step 2 "The Wrapper"
   c. Simple methods/functions → Step 3 "Core Operations"
   d. Complex methods (high difficulty score) → Step 4+
   e. Lifecycle/cleanup symbols → Last step "The Cleanup"
5. If the target is a single function (not a type with methods), decompose
   along its internal control flow instead:
   a. Input validation → Step 1
   b. Main logic → Step 2
   c. Error handling → Step 3
   d. Output formatting → Step 4

PROMPT GENERATION PER STEP:
For each step:
1. Get the SIR intent for the symbol(s) in that step
2. Get their dependencies (which should all be from previous steps)
3. Compose the prompt using template:
   "Create/Implement/Add {description} {details_from_sir}.
   {constraint_1}. {constraint_2}."

   Details come from SIR: field types, method signatures, return types
   Constraints come from SIR: "must be Clone", "must handle expiration",
   "must be async", etc.

4. Compose "why_this_order" from dependency relationship:
   "{target} has no dependencies — it's pure data."
   "{target} wraps {dep}, so {dep} must exist first."
   "{target} is more complex than {previous} because {reason}."

5. List context_needed from the dependency graph (what from previous steps
   the LLM needs to see)

STEP NAMING CONVENTIONS:
- Step 1 always: "The Foundation" / "Define the data shape"
- Last step for types with lifecycle: "The Cleanup" / "Graceful shutdown"
- Middle steps: name by what they add ("Core Operations", "The Pub/Sub System",
  "Error Handling", "The Connector", etc.)

=== ALSO SUPPORT FILE-LEVEL DECOMPOSITION ===

GET /api/v1/decompose/file/{path}

Same concept but decompose an entire file. Steps map to individual symbols
or groups of related symbols within the file, ordered by internal dependencies.

=== HTMX FRAGMENT: GET /dashboard/frag/decompose/{symbol} ===

Layout:

HEADER:
  "Build {symbol} in {N} Steps" (h1)
  Difficulty badge, file path
  Preamble paragraph

STEP CARDS (vertical, numbered):
  Each card has:
  - Step number (large circle) + Title + Subtitle
  - Difficulty badge for this step
  - PROMPT BOX: The actual prompt text in a code-like box with copy button
    "📋 Copy This Prompt"
  - "Why this order" explanation (italic, smaller)
  - "Context needed" list (what to include from previous steps)
  - "Expected output" description

TEACHING SUMMARY (bottom card):
  The teaching_summary paragraph

NAVIGATION: Link from Symbol Deep Dive ("🔨 See Build Steps" button) and
from the Prompt Builder (new goal: "Build this component step by step").

========================================================================
TASK 5: VERIFICATION CHECKPOINTS
========================================================================

PURPOSE: After each decomposer step, what should the user check before
feeding the output into the next prompt? Derived from SIR invariants.

Add a "checkpoints" field to each decomposer step (already shown in the
schema above as empty arrays — now populate them).

=== GENERATING CHECKPOINTS ===

For each step, derive checkpoints from:

1. TRAIT REQUIREMENTS: If dependents use this symbol via a trait (Clone, Send,
   Sync, Display, etc.), checkpoint: "Must implement {Trait}. Required by
   {dependent_names}."
   Source: SIR dependency annotations + graph dependents

2. VISIBILITY CONSTRAINTS: If a symbol is only used internally, checkpoint:
   "Should not be public." If used externally, "Must be public."
   Source: Graph dependents (internal vs external file)

3. SIDE EFFECT INVARIANTS: For each side effect in SIR, checkpoint:
   "Must {handle/spawn/signal} {side_effect}."
   Source: SIR side_effects field

4. ERROR MODE COVERAGE: For each error mode in SIR, checkpoint:
   "Must handle {error_mode}."
   Source: SIR error_modes field

5. DEPENDENCY CONTRACTS: For each dependency, checkpoint:
   "Must use {dep} correctly — {usage_pattern}."
   Source: SIR dependency annotations

Severity:
- "critical" — compilation or correctness failure if missed
- "warning" — code smell or maintainability issue
- "info" — best practice suggestion

Checkpoint example:
{
  "check": "Db implements Clone",
  "why": "Required — 8 other components clone it. Without Clone, every command handler fails to compile.",
  "source": "dependency_graph (8 dependents that receive Db by clone)",
  "severity": "critical"
}

=== HTMX DISPLAY ===

Checkpoints are rendered WITHIN each decomposer step card, in a collapsible
section below the prompt:

"✅ Verify Before Moving On" (click to expand)
  Checklist items with checkboxes (client-side only, not persisted):
  [ ] Db implements Clone — Required, 8 components clone it
  [ ] Arc wraps SharedState — Background task handle must be shared
  [ ] State is not pub — Internal implementation detail

Color coding: critical=red left border, warning=yellow, info=gray

========================================================================
TASK 6: PROMPT AUTOPSY — "What Would Work and What Wouldn't"
========================================================================

PURPOSE: For a given symbol, show three prompts at different specificity
levels and explain why each succeeds or fails. Teaches users to calibrate
their prompting to code complexity.

=== API ENDPOINT: GET /api/v1/autopsy/{symbol} ===

{
  "data": {
    "symbol": "Subscribe",
    "kind": "struct + impl",
    "file": "src/cmd/subscribe.rs",
    "difficulty": { "emoji": "🔴", "label": "Hard" },

    "prompts": [
      {
        "level": "good",
        "emoji": "✅",
        "label": "This Would Work",
        "prompt": "Implement an async `apply` method on Subscribe that takes a `Db`, a `Connection`, and a `Shutdown` signal. For each channel name, subscribe via `db.subscribe(channel)`. Then loop: select! between receiving a message (forward it as a Bulk frame to the connection), receiving a shutdown signal (break), or the connection sending an unsubscribe command (remove that subscription and if no subscriptions remain, return).",
        "why_it_works": "This prompt succeeds because it specifies three critical things: (1) the exact input types, (2) the control flow structure (loop with select!), and (3) all edge cases (shutdown, unsubscribe, empty subscriptions). For async stateful operations, the LLM needs the control flow spelled out.",
        "key_elements": [
          "Exact parameter types (Db, Connection, Shutdown)",
          "Control flow structure (loop + select!)",
          "All three exit conditions (message, shutdown, unsubscribe)",
          "Edge case: empty subscription list"
        ]
      },
      {
        "level": "partial",
        "emoji": "⚠️",
        "label": "This Would Have Bugs",
        "prompt": "Implement Subscribe::apply that subscribes to channels and forwards messages to the client.",
        "what_goes_wrong": "The LLM will produce code that handles the happy path — subscribing and forwarding messages — but will miss: the shutdown signal handling (the subscribe loop runs forever), the per-channel unsubscribe logic (no way to stop individual subscriptions), and the 'return when all subscriptions gone' edge case (the function never terminates cleanly).",
        "missing_elements": [
          "No mention of Shutdown signal → infinite loop on shutdown",
          "No mention of unsubscribe → can't remove individual channels",
          "No mention of empty-subscription exit → function hangs",
          "No mention of select! → LLM may use sequential awaits instead"
        ]
      },
      {
        "level": "bad",
        "emoji": "❌",
        "label": "This Would Fail",
        "prompt": "Build the subscribe command for my Redis server.",
        "what_goes_wrong": "Without seeing the Connection, Db, and Frame types, the LLM will invent its own abstractions that don't integrate with the existing codebase. The generated code will have different type signatures, different error handling patterns, and won't match the command dispatch interface. You'd spend more time adapting the output than writing it yourself.",
        "missing_elements": [
          "No type information → LLM invents incompatible types",
          "No existing patterns → doesn't match Command dispatch",
          "No context about Frame protocol → wrong serialization",
          "No dependency information → can't call Db correctly"
        ]
      }
    ],

    "teaching_summary": "The working prompt succeeded because it specified the control flow and edge cases explicitly. When prompting for async operations, always describe: the loop/select structure, all exit conditions, and what happens in each branch. The partial prompt missed these because it described WHAT but not HOW. The bad prompt failed because it provided no context at all. The general rule: the harder the difficulty rating, the more control flow detail you need to include.",

    "pattern_name": "Async State Machine",
    "pattern_rule": "For async operations with multiple event sources: always specify the select!/loop structure, enumerate all branches, and describe termination conditions."
  }
}

=== IMPLEMENTING PROMPT AUTOPSY ===

Create: crates/aether-dashboard/src/api/autopsy.rs

GOOD PROMPT GENERATION:
1. Get SIR intent, dependencies, error_modes, side_effects
2. For the prompt text, compose from SIR:
   - Start with action: "Implement/Create/Add"
   - Include exact parameter types from SIR dependencies
   - Include control flow from SIR intent (look for "loop", "select", "match",
     "iterate", "await", "spawn" keywords)
   - Include edge cases from SIR error_modes
   - Include termination conditions from SIR intent
3. For why_it_works, enumerate the key elements that make it work
4. For key_elements, list: types specified, control flow specified,
   edge cases specified, context included

PARTIAL PROMPT GENERATION:
1. Take the SIR intent first sentence only
2. Strip type information and control flow details
3. Keep the high-level goal but remove specifics
4. For what_goes_wrong, identify what was removed:
   - Each error_mode not mentioned → "No mention of {error} → {consequence}"
   - Control flow not specified → "No mention of {pattern} → LLM may use {wrong_alternative}"
   - Side effects not mentioned → "No mention of {effect} → {consequence}"

BAD PROMPT GENERATION:
1. Reduce to a single vague sentence with no context
2. Remove all type names, all specifics
3. For what_goes_wrong, explain the complete absence of context
4. Focus on: type invention, pattern mismatch, serialization mismatch

TEACHING SUMMARY:
Select from pattern templates based on the symbol's characteristics:

Pattern: "Async State Machine"
  Trigger: SIR contains "loop", "select", "channel", "spawn"
  Rule: "For async operations with multiple event sources, always specify the
    select/loop structure, enumerate all branches, and describe termination."

Pattern: "Concurrent Shared State"
  Trigger: SIR contains "Arc", "Mutex", "lock", "shared", "concurrent"
  Rule: "For shared state, specify the wrapping pattern (Arc<Mutex>), what
    operations need locks, and lifecycle (who creates, who drops last)."

Pattern: "Data Transformation"
  Trigger: Symbol kind is function, low side effects, low error modes
  Rule: "For pure data transformations, specify input types, output types,
    and the transformation logic. These are easiest for LLMs."

Pattern: "Error-Heavy Operations"
  Trigger: error_modes count > 3
  Rule: "For operations with many failure modes, enumerate every error case
    explicitly. LLMs default to happy-path code."

Pattern: "Protocol Implementation"
  Trigger: Wire Format layer, SIR contains "parse", "frame", "serialize"
  Rule: "For protocol code, always provide the format specification and
    at least one example input/output pair."

=== HTMX FRAGMENT: GET /dashboard/frag/autopsy/{symbol} ===

Layout:

HEADER:
  "Prompt Autopsy: {symbol}" (h1)
  Difficulty badge + Pattern name badge ("Async State Machine")
  File path

THREE PROMPT CARDS (side by side or stacked):

✅ Card (green border):
  "This Would Work" (h2)
  Prompt text in highlighted box with copy button
  "Why it works:" paragraph
  "Key elements:" checklist (all checked)

⚠️ Card (yellow border):
  "This Would Have Bugs" (h2)
  Prompt text in highlighted box
  "What goes wrong:" paragraph
  "Missing:" checklist (all unchecked, with consequences)

❌ Card (red border):
  "This Would Fail" (h2)
  Prompt text in highlighted box
  "What goes wrong:" paragraph
  "Missing:" list of fatal gaps

TEACHING SUMMARY (bottom):
  Pattern rule paragraph
  "The general principle:" sentence

NAVIGATION: Access from Symbol Deep Dive ("🎓 Prompt Advisor" button,
previously disabled — enable it now). Also from Glossary ("🎓" button,
also previously disabled — enable now).

=== SIDEBAR ===

Add: 💬 Prompts (between Glossary and Recent Changes)

Prompt Builder, Code-to-Spec, and Context Advisor are all accessible from
the Prompts page via tabs or sub-navigation within the page.

The Prompts page now has sub-navigation tabs:
  💬 Build a Prompt | 📋 Generate Spec | 🧠 Context Advisor | 🎓 Learn to Prompt

The "🎓 Learn to Prompt" tab shows:
- Difficulty overview (from Task 1)
- Link to any symbol's Decomposer
- Link to any symbol's Autopsy
- Teaching summary: "How to Prompt Effectively" general guidance paragraph

No new sidebar entries for Decomposer/Autopsy. They're accessed from:
- Symbol Deep Dive (bottom action buttons)
- Glossary (term action buttons)
- Prompt Builder page ("🎓 Learn" tab)

========================================================================
VALIDATION (covers all six tasks)
========================================================================

DIFFICULTY RADAR:
1. Glossary shows difficulty badge on each term
2. Symbol Deep Dive shows difficulty section with reasons
3. Graph page has "Color by Difficulty" toggle
4. Anatomy key actors show difficulty badges
5. Overview or Anatomy shows difficulty summary stats
6. Db rated Hard or Very Hard (concurrent, side effects, errors)
7. Simple types rated Easy
8. Reasons are specific to each symbol, not generic

PROMPT BUILDER:
9. /dashboard/prompts shows 8 goal cards
10. Click goal → typeahead appears (except Health Check)
11. Select symbol → prompt generated
12. Copy button works
13. All prompts well-formed English

CODE-TO-SPEC:
14. "📋 Spec" buttons in Glossary now enabled and functional
15. Spec shows purpose, requirements, inputs, outputs, deps, errors
16. Requirements prefixed with "MUST"
17. Copy button on spec

CONTEXT ADVISOR:
18. /dashboard/frag/context/{symbol} shows required/optional/not-needed
19. Required files match direct dependencies
20. Line estimates are reasonable
21. Context reduction percentage shown
22. Teaching note present
23. "Copy All Required Context" button works

DECOMPOSER:
24. /dashboard/frag/decompose/Db shows 4-5 steps
25. Steps ordered by dependency (foundation → wrapper → operations → cleanup)
26. Each step has a concrete, copy-paste-ready prompt
27. "Why this order" explains the dependency reasoning
28. Context needed lists only previous steps
29. Steps for simple symbols (Frame, Entry) have 1-2 steps
30. Steps for complex symbols (Db, Subscribe) have 4-5 steps

CHECKPOINTS:
31. Each decomposer step has verification checkpoints
32. Checkpoints are specific (not generic "does it compile")
33. Critical checks reference actual dependents ("8 components clone this")
34. Checkbox UI works (client-side, no persistence needed)

AUTOPSY:
35. /dashboard/frag/autopsy/Subscribe shows three prompt levels
36. Good prompt includes types, control flow, edge cases
37. Partial prompt is recognizably incomplete (shows what was removed)
38. Bad prompt is genuinely vague
39. Teaching summary identifies a named pattern
40. Pattern rule is specific and actionable
41. "🎓 Prompt Advisor" buttons in Glossary and Deep Dive are now enabled and functional
42. Prompts page "🎓 Learn" tab shows difficulty overview + navigation

INTEGRATION:
43. All symbol-links throughout all features work
44. Navigation between Decomposer, Autopsy, Context Advisor, and Deep Dive
    forms a connected exploration experience
45. Complexity selector from Run 1 applies to all new features
    (expert mode hides teaching notes, beginner mode shows them all)

cargo fmt --all --check
cargo clippy --workspace --features dashboard -- -D warnings
cargo test -p aether-dashboard
cargo test -p aetherd --features dashboard
```
