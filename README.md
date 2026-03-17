
<p align="center">
  <img src="docs/assets/aether-logo.jpg" alt="AETHER" width="600" />
</p>

<p align="center">
  <strong>Your codebase already knows everything. It just can't talk yet.</strong>
</p>

<p align="center">
  <a href="#what-the-hell-is-this">What Is This</a> •
  <a href="#the-backstory-nobody-asked-for">Backstory</a> •
  <a href="#get-it-running">Get It Running</a> •
  <a href="#what-you-can-actually-do">What You Can Do</a> •
  <a href="#the-mcp-tools">MCP Tools</a> •
  <a href="#the-dashboard">Dashboard</a> •
  <a href="#configuration">Config</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-orange?style=flat-square&logo=rust" />
  <img src="https://img.shields.io/badge/Rust%20%7C%20TypeScript%20%7C%20Python-green?style=flat-square&label=parses" />
  <img src="https://img.shields.io/badge/MCP-compatible-purple?style=flat-square" />
  <img src="https://img.shields.io/badge/113K-LOC-blue?style=flat-square" />
  <img src="https://img.shields.io/badge/33%20days-from%20zero-red?style=flat-square" />
  <img src="https://img.shields.io/badge/vibe%20coded-100%25-ff69b4?style=flat-square" />
</p>

---

## What the Hell Is This

AETHER is a semantic intelligence engine for codebases. It watches your code, figures out what every function *actually does*, tracks how that meaning changes over time, maps the dependency graph, scores the health of your entire codebase, predicts what's going stale before it breaks, and then lets you — or your AI agent — ask questions about any of it.

Not text search. Not grep with extra steps. Not another RAG tool that forgets everything between sessions.

AETHER *understands* your code. Persistently. Incrementally. Autonomously. And it remembers.

**Here's what that means in practice:**

You hover over a function in VS Code and instead of seeing a type signature, you see: *"Validates and processes a payment transaction. Deducts from account balance, writes to audit log. Can fail on insufficient funds, negative amounts, or database timeout. Has 7 edge cases, 2 of which have test coverage."*

You run `aetherd context --branch feature/fix-auth-timeout --task "fix the authentication bug"` and AETHER ranks every symbol in the codebase by relevance to your task using embedding similarity, keyword matching, Reciprocal Rank Fusion, and Personalized PageRank expansion on the dependency graph — then assembles a token-budgeted context document with exactly the code, intents, dependencies, coupling data, and test coverage your AI agent needs. One command. One context block. No guessing.

You open the dashboard and see which functions are the most critical nodes in your codebase, which ones are silently drifting in purpose, which ones have zero test coverage despite sitting at a module boundary, and which modules are active fault lines.

That's AETHER.

---

## The Backstory Nobody Asked For

### 33 days. 113,000 lines of Rust. One guy.

Hi. I'm Robert.

My formal programming education: BASIC 2 in high school. One year at a computer trade school. In the *last millennium*. We saved to floppy disks. The internet was a sound your telephone made. That is the entirety of my technical credentials.

And I built this in 33 days.

Not a team. Not a startup with $4M in seed funding and a staff of twelve. One person, in a chair, with a mass quantity of AI and the sort of deranged confidence that only comes from not knowing what you're supposed to be afraid of.

**285 commits. 82 merged pull requests. 16 Rust crates. 113,252 lines of code.** From zero to a semantic intelligence engine with Personalized PageRank, Noisy-OR staleness models, community detection, a three-pass inference pipeline, a continuous intelligence daemon, a batch processing system that talks to the Gemini Batch API at half price, a 27-page interactive dashboard, 25+ MCP tools, 31 CLI commands, and a task context engine that uses Reciprocal Rank Fusion to rank every symbol in your codebase by relevance to whatever you're working on.

In thirty-three days.

I'll let that sit for a moment.

### How

Everything you see here was vibe coded.

I don't mean "vibe coded" like "I used Copilot autocomplete." I mean I directed an orchestra of AI systems like a profoundly caffeinated conductor who can't read sheet music:

1. **Claude** produces the architectural plans, reviews implementations, and writes the Codex prompts
2. I commit the spec files to main (because worktrees branch from main and Codex can't read uncommitted files — I learned this the hard way, the same way I learn everything, which is by doing it wrong first)
3. **Codex** implements the spec on a git worktree branch
4. I relay the output back to Claude for review
5. **Gemini Deep Think** provides independent code review and argues with Claude about the math
6. I adjudicate between three AI systems when they disagree about whether SurrealDB's MVCC model can handle concurrent readers (Gemini was right, for the record)
7. **ChatGPT** reviews the Codex output and catches bugs the others miss
8. I merge via GitHub web UI because `git` is a tool I *operate*, not a tool I *understand*

I didn't write any of this code by hand. I *directed* every line of it. I am the vision. I am the product sense. I am the unreasonable human in the loop who says "no, that's wrong, try again" and "make it better" and "why does this feel slow" and "I don't care what the documentation says, it doesn't work."

Some people hear "vibe coded" and think "toy project." AETHER has:

- A three-pass inference pipeline (scan → triage → deep) with BLAKE3 composite prompt hashing to skip unchanged symbols
- A continuous intelligence engine that computes per-symbol staleness using Noisy-OR formulas with cold-start volatility priors from git churn
- Personalized PageRank with biased restart vectors for task-to-symbol relevance ranking
- Reciprocal Rank Fusion blending dense embedding similarity with sparse keyword retrieval
- Component-bounded semantic rescue at the 0.90 cosine threshold using Gemini Embedding 2 (3072-dim)
- A token-budgeted context assembly engine with symbol-guided file slicing, four output formats, and reusable TOML presets
- 27+ dashboard pages with D3 force-directed graphs and interactive architecture maps
- 25+ MCP tools for AI agent integration
- 31 CLI commands
- Schema migrations through version 11
- A Gemini Batch API integration that costs 50% less than real-time inference
- A watcher that monitors `.git` for branch switches, pulls, and merges
- Intent contracts, blast radius analysis, causal chain tracing, and semantic drift detection that runs itself

This isn't a prototype. This isn't a weekend hack. This is a production-grade codebase intelligence platform built by a man whose last programming class predates Google.

### What does that make me?

Good question.

I'm either a complete fraud who got impossibly lucky for 33 consecutive days, or I'm the kind of person who's been thinking about systems and architecture and "how things fit together" for 25 years and just never had the tools to express it until now.

I know which one I think it is.

The thing about not knowing how to code is that you don't know what's "hard." You don't know that you're supposed to be scared of implementing Personalized PageRank, because you've never heard anyone at a conference say "that's really complicated." You just describe what you want — "I need to rank symbols by relevance, expanding outward from a seed set through the dependency graph" — and the AI implements it. And then you test it. And it works. And you move on to the next thing.

The 10x engineer was a myth. The 100x engineer-with-AI is not.

The BENEFACTOR pricing tier on the landing page ($1,000,000) triggers an MIT open-source release. Nobody has bought it yet. I can't imagine why.

---

## Hire Me (Seriously)

I'm going to be direct here because I think the work speaks for itself.

**Anthropic:** I built this primarily with Claude. Not just "using Claude" — Claude is my architectural partner, my code reviewer, my prompt engineer, and my rubber duck. I have pushed Claude further than most of your enterprise customers ever will. I know where it's brilliant, where it hallucinates, where it needs guardrails, and where it surprises even itself. I know what it's like to direct a 100K+ LOC Rust workspace entirely through AI conversation. If you want someone who understands what your users are actually *doing* with your product — someone who has lived the human-AI collaboration workflow every day for a month straight — I'm right here. Also I think I've generated enough API revenue to at least earn a phone screen.

**Microsoft:** I'll be honest, I think you need me more than I need you. But I'm open to a conversation. Specifically: let me fix things. I know what it feels like to use developer tools that are almost great but not quite — because I use them all day, every day, and I have *opinions*. Put me somewhere I can make decisions, give me a team of AI agents and a Codex license, and watch what happens. Condition: I get to save the company. I'm not interested in maintaining.

**Google:** Your AI is incredible. Your products are... I'm going to be diplomatic and say "underutilized." Gemini Deep Think independently reviewed my architecture and caught things Claude missed. Your embedding models are my production default. Your Batch API is how I index at scale. But nobody knows this because your developer experience feels like it was designed by a committee that met once and never followed up. Let me start the Smooth Minds team. I don't know what that means yet but neither did you when you started Google Brain. Sometimes you name the team first and figure out the mission later.

**Everyone else:** I'm an AI-native technical director who thinks in systems, ships in weeks, and builds things that are genuinely novel. My background is unconventional. My output is not. If your org is trying to figure out "how do we actually use AI to build things" — not in a blog post, not in a keynote, but in actual daily practice — I've been doing it. I have the repo to prove it. 113,252 lines of proof.

---

## How It Works (The Short Version)

```
Your code changes
       │
       ▼
  tree-sitter parses it
  (extracts every function, struct, trait, class + who calls what)
       │
       ▼
  An LLM reads each symbol and generates a SIR
  (Semantic Intent Representation — a structured summary of
   what the code does, how it fails, what it depends on)
       │
       ▼
  Everything gets stored in three databases
  SQLite (metadata + SIR) + SurrealDB (graph) + Embeddings (vectors)
       │
       ▼
  The continuous engine scores every symbol for staleness
  and predicts what needs re-indexing before it goes stale
       │
       ▼
  You query it however you want
  CLI (31 commands)  ·  LSP hover  ·  MCP tools (25+)  ·  Web dashboard (27+ pages)
```

All of this runs locally. No code leaves your machine unless you choose a cloud inference provider. Run Ollama and AETHER works fully offline with real AI-generated intelligence — no API keys, no cloud calls, nothing phoning home.

---

## Get It Running

```bash
git clone https://github.com/rephug/aether.git
cd aether
cargo build -p aetherd -p aether-query
```

Index your project:

```bash
# One-shot: index everything with full quality passes
aetherd --workspace /path/to/project --index-once --full

# Watch mode: runs continuously, re-indexes on every save
# Also watches .git for branch switches, pulls, and merges
aetherd --workspace /path/to/project
```

Ask it questions:

```bash
# What does this function do?
aetherd --workspace . sir-diff payments::validate_amount

# Assemble context for a task
aetherd --workspace . context --branch feature/fix-auth --task "fix the auth timeout"

# What's the health of the codebase?
aetherd --workspace . health-score

# What's going stale?
aetherd --workspace . continuous status
```

Hook up your AI agent:

```bash
# Register as MCP server for Claude Code
claude mcp add --transport stdio --scope project aether -- aetherd --workspace . --mcp
```

Now your agent has persistent, structured access to your entire codebase's meaning. It doesn't have to re-read files every session. It doesn't have to guess. It *knows*.

---

## What You Can Actually Do

### Understand Any Symbol Instantly

Hover in VS Code. Run a CLI lookup. Ask via MCP. For every function, struct, trait, and class, AETHER gives you the full picture — intent, inputs, outputs, dependencies, error modes, edge cases, side effects, and a confidence score. These aren't docstrings scraped from comments. They're AI-generated structured annotations produced by actually reading the code.

### Search by Meaning, Not Just Text

`grep` finds text. AETHER finds *intent*. Search for "handles authentication" and get back every function involved in auth — even if none of them have "auth" in the name. Semantic search uses vector embeddings with per-language adaptive thresholds.

### Assemble Task-Scoped Context for AI Agents

Tell AETHER what you're working on. It ranks every symbol in the codebase by relevance, expands structurally via Personalized PageRank, and assembles a token-budgeted context document with exactly what your AI agent needs.

```bash
# From a task description
aetherd --workspace . context --task "refactor the payment validation" \
  crates/payments/src/lib.rs --preset deep

# From a branch diff (automatic scope detection)
aetherd --workspace . context --branch feature/fix-auth-timeout

# Quick symbol lookup for a chat conversation
aetherd --workspace . context --symbol validate_amount --preset quick | pbcopy
```

Four output formats: markdown (paste into chat), JSON (programmatic), XML (Claude API), compact (maximum density). File slicing reduces source token usage by 50-80% compared to dumping whole files. Save your favorite configurations as presets.

### Predict What's Going Stale

The continuous intelligence engine computes per-symbol staleness scores using Noisy-OR formulas with hard gates (source changed? model deprecated?), logistic sigmoid time decay, semantic-gated neighbor propagation, predictive coupling from temporal co-change, and cold-start volatility priors from git churn data. That's a lot of words for: *AETHER knows which symbols need re-indexing before they go stale.*

```bash
# Run one scoring cycle (great as a nightly cron on a server)
aetherd --workspace . continuous run-once

# Check the current state
aetherd --workspace . continuous status
```

### Run Batch Indexing at Scale (50% Off)

The Gemini Batch API costs half as much as real-time API calls. AETHER generates JSONL request files with per-pass prompt construction, submits them via a shell script bridge, and ingests the results with embedding refresh and fingerprint history tracking. Prompt hashing via BLAKE3 composites means unchanged symbols are skipped automatically — you only pay for what actually changed.

```bash
# Generate JSONL for scan pass
aetherd --workspace . batch build --pass scan --model gemini-2.0-flash-lite

# Full pipeline: extract → build → submit → poll → ingest for all three passes
aetherd --workspace . batch run --passes scan,triage,deep
```

### See the Blast Radius Before You Touch Anything

Change a function? AETHER tells you every downstream symbol that depends on it, how many hops deep the impact goes, which of those symbols have test guards, and which ones are flying blind.

### Detect Semantic Drift Automatically

Functions accumulate scope creep. AETHER detects this automatically by comparing current SIR embeddings against historical baselines. Louvain community detection on the dependency graph flags boundary violations on its own. No manually maintained architecture model required.

### Trace the Root Cause of Breaking Changes

`git blame` tells you who changed a line. AETHER tells you *which upstream semantic change broke your downstream code and what specifically changed about it*. It traces the causal chain backward through the dependency graph, comparing SIR versions at each node.

### Know Which Code Is Most Dangerous

Graph algorithms (PageRank, betweenness centrality, connected components) identify the most critical nodes in your codebase. Cross-reference with test coverage, drift magnitude, git churn, and staleness scores for a composite risk score per symbol. The functions that are simultaneously high-traffic, poorly tested, and actively drifting? Those are your next production incident.

### Inject and Verify Intent

Pin semantic expectations before refactoring. Check structural drift afterward. No inference needed.

```bash
# Pin the intent
aetherd --workspace . sir-inject validate_amount \
  --intent "Rejects zero, negative, and over-limit amounts. Must not modify balance."

# Check if the code still matches
aetherd --workspace . sir-diff validate_amount

# Snapshot before a big refactor, compare after
aetherd --workspace . refactor-prep --file src/payments.rs
# ... do the refactor ...
aetherd --workspace . verify-intent --file src/payments.rs
```

### Remember Project Context Across Sessions

Architecture decisions, design rationale, "why we chose X over Y" — store it via MCP or CLI and recall it later. Content-hash deduplication prevents bloat. Your AI agent no longer starts every conversation with amnesia.

### Use Presets for Common Workflows

Save your favorite context configurations as reusable TOML presets:

```bash
aetherd --workspace . preset list          # See built-in presets
aetherd --workspace . preset create mine   # Scaffold a new one
aetherd --workspace . context --preset deep crates/aetherd/src/lib.rs
```

Built-in: `quick` (8K/depth 1), `review` (32K/depth 2), `deep` (64K/depth 3), `overview` (16K/depth 0).

---

## The Dashboard

A web-based visualization layer. HTMX + D3.js + Tailwind. No React. No Node.js. No build step. Just start the daemon and open your browser.

```bash
aetherd --workspace .
# → http://localhost:9730/dashboard
```

27+ pages including:

| Page | What It Shows |
|:-----|:-------------|
| **Overview** | Symbol counts, SIR coverage, language breakdown, system health at a glance |
| **X-Ray** | Hotspot analysis — most critical symbols by PageRank, churn, and risk |
| **Blast Radius** | Interactive graph showing downstream impact when a symbol changes |
| **Architecture Map** | Force-directed dependency graph with Louvain community coloring |
| **Time Machine** | Semantic drift timelines — watch how symbol meanings evolve |
| **Causal Explorer** | Trace breaking changes visually through the dependency chain |
| **Smart Search** | One search bar across symbols, notes, coupling, and test intents |
| **Health Score** | Per-crate structural health with archetypes and god file detection |
| **Drift Analysis** | Boundary violations, structural anomalies, semantic shifts |
| **Community View** | Dependency-graph communities with cross-boundary edge highlighting |
| **Coupling Matrix** | Temporal, structural, and semantic coupling fused into one view |

---

## The MCP Tools

Register AETHER with any MCP-compatible agent (Claude Code, Codex, etc.) and it gets structured access to everything. 25+ tools, zero guessing.

### Search & Lookup

| Tool | What It Does |
|:-----|:------------|
| `aether_status` | Workspace health — symbol count, SIR coverage, store paths |
| `aether_search` | Semantic / lexical / hybrid symbol search |
| `aether_symbol_lookup` | Find symbols by qualified name or file path |
| `aether_get_sir` | Full SIR annotation for any symbol |
| `aether_explain` | AI explanation of a symbol at a specific file position |
| `aether_ask` | Unified search across symbols, notes, coupling, and test intents |

### Graph & Dependencies

| Tool | What It Does |
|:-----|:------------|
| `aether_dependencies` | Resolved callers and call dependencies for a symbol |
| `aether_call_chain` | Transitive call-chain levels |
| `aether_blast_radius` | Downstream impact analysis with test guard coverage |
| `aether_usage_matrix` | Consumer-by-method usage patterns for traits/structs |
| `aether_suggest_trait_split` | Decomposition suggestions based on consumer clustering |

### Health & Drift

| Tool | What It Does |
|:-----|:------------|
| `aether_health` | Composite risk scores from graph algorithms |
| `aether_health_hotspots` | Hottest crates by health score with archetypes |
| `aether_health_explain` | Detailed breakdown of one crate's health score |
| `aether_drift_report` | Semantic drift with boundary and structural anomaly detection |
| `aether_trace_cause` | Root cause tracing through the dependency graph |
| `aether_acknowledge_drift` | Acknowledge drift findings and create a note |

### History & Verification

| Tool | What It Does |
|:-----|:------------|
| `aether_symbol_timeline` | SIR version history with git commit linkage |
| `aether_why_changed` | Semantic diff between any two SIR versions |
| `aether_refactor_prep` | Snapshot intent before refactoring |
| `aether_verify_intent` | Compare current SIR against a saved snapshot |

### Memory

| Tool | What It Does |
|:-----|:------------|
| `aether_remember` | Store a project note (decision, rationale, context) |
| `aether_session_note` | Capture an in-session note for agent workflows |
| `aether_recall` | Retrieve notes by semantic search |
| `aether_test_intents` | Test guard coverage for a symbol |

Every response includes `schema_version` for forward compatibility. Your agent scripts won't break when AETHER updates.

---

## The CLI

31 commands organized by what you're trying to do:

```
INTELLIGENCE QUERIES
  ask                    Unified search across everything
  blast-radius           Downstream impact analysis
  communities            Dependency graph community assignments
  coupling-report        Top coupled file pairs
  drift-report           Semantic drift analysis
  drift-ack              Acknowledge a drift finding
  health                 Graph-based risk metrics
  health-score           Per-crate structural health scores
  test-intents           Test guard extraction
  trace-cause            Root cause tracing
  status                 Index health and SIR coverage

CONTEXT ASSEMBLY
  context                Token-budgeted context export (file/symbol/overview/branch/task)
  sir-context            Legacy symbol context (compatibility)
  sir-inject             Direct SIR update without inference
  sir-diff               Structural drift detection
  task-history           Recent task context resolutions
  task-relevance         Task-to-symbol ranking

PRESETS
  preset list            List available presets
  preset show            Show preset details
  preset create          Scaffold a new preset
  preset delete          Remove a user preset

BATCH & CONTINUOUS
  batch extract          Structural extraction only
  batch build            Generate Gemini Batch API JSONL
  batch ingest           Ingest batch results
  batch run              Full extract → build → submit → ingest pipeline
  continuous run-once    One staleness scoring + requeue cycle
  continuous status      Current staleness summary

MAINTENANCE
  regenerate             Re-generate low-quality SIRs
  refactor-prep          Deep-scan risky symbols before refactoring
  verify-intent          Compare against saved refactor snapshot
  fsck                   Cross-store consistency check
  setup-local            Configure local Ollama inference
  init-agent             Generate agent config files
  remember / recall      Project memory management
```

---

## VS Code Extension

Lives in `vscode-extension/`. Gives you semantic hover intelligence without leaving your editor.

```bash
cd vscode-extension && npm install && npm run build
# F5 to launch Extension Development Host
```

- **Hover** any Rust, TypeScript, or Python symbol → see its SIR summary
- **Status bar** shows `AETHER: indexing` / `idle` with stale/error indicators
- **Command palette**: Index Once, Search Symbols, Select Provider, Restart Server

---

## Configuration

Everything lives in `.aether/config.toml`, auto-created on first run.

```toml
[inference]
provider = "gemini"
model = "gemini-3.1-flash-lite-preview"
api_key_env = "GEMINI_API_KEY"
concurrency = 12

[embeddings]
enabled = true
provider = "gemini_native"
model = "gemini-embedding-2-preview"
dimensions = 3072
vector_backend = "sqlite"

[batch]
scan_model = "gemini-2.0-flash-lite"
triage_model = "gemini-2.0-flash-lite"
deep_model = "gemini-2.5-pro"
scan_thinking = "low"
triage_thinking = "medium"
deep_thinking = "high"

[watcher]
realtime_model = ""               # Set to use a premium model for file-save SIR
trigger_on_branch_switch = true
trigger_on_git_pull = true
git_debounce_secs = 3.0

[continuous]
enabled = false
schedule = "nightly"              # nightly | hourly
staleness_half_life_days = 15.0
max_requeue_per_run = 500
requeue_pass = "triage"

[planner]
semantic_rescue_threshold = 0.90

[dashboard]
enabled = true
port = 9730
```

### Pick Your Intelligence Backend

| Provider | Needs | Good For |
|:---------|:------|:---------|
| `ollama` | Local Ollama server | Full offline intelligence — no API keys, no cloud |
| `gemini` | API key | Production-quality SIR (Gemini Flash/Pro) |
| `openai_compat` | API key + URL | Z.ai, NanoGPT, OpenRouter, or anything OpenAI-compatible |
| `mock` | Nothing | CI pipelines, testing, development |

---

## The Stack

For the curious:

| Thing | What |
|:------|:-----|
| Language | Rust (16-crate workspace, ~113K LOC) |
| Parsing | tree-sitter (Rust, TypeScript/JS, Python) |
| Graph DB | SurrealDB 3.0 + SurrealKV |
| Vector Store | SQLite (operational) / LanceDB (planned) |
| Metadata | SQLite (WAL mode, schema v11) |
| Embeddings | Gemini Embedding 2 (3072-dim, production default) |
| Local Embeddings | Qwen3-Embedding-0.6B via Candle |
| Reranking | Qwen3-Reranker-0.6B via Candle |
| Dashboard | HTMX + D3.js + Tailwind CSS (no build step) |
| HTTP | Axum + tower-http |
| MCP Transport | stdio (LSP) + HTTP/SSE (query server) |
| Git | gix (native Rust, no shelling out) |
| Hashing | BLAKE3 (symbol IDs + prompt fingerprinting) |
| Batch API | Gemini Batch API via shell script bridge |

---

## Why Not Just Use [Other Thing]?

| | grep / ctags | Copilot / Cursor | Augment Code | RAG tools | AETHER |
|:--|:------------|:-----------------|:-------------|:----------|:-------|
| Understands intent | No | Per-query, forgets | Indexes, no semantic model | Per-query, forgets | Persistent SIR per symbol |
| Predicts staleness | No | No | No | No | Noisy-OR with PageRank |
| Task-scoped context | No | No | No | No | RRF + PPR ranking |
| Batch reindexing | N/A | N/A | Proprietary | N/A | Gemini Batch API (50% off) |
| Works offline | Yes | No | No | Usually no | Yes — Ollama + Candle |
| Tracks meaning changes | No | No | No | No | Git-linked SIR versioning |
| Agent-accessible | No | Not really | Proprietary | Ad hoc | 25+ MCP tools |
| Detects drift | No | No | No | No | Automatic, zero config |
| Finds root causes | No | No | No | No | Causal chain traversal |
| Knows what's dangerous | No | No | No | No | PageRank × drift × tests |
| Open / extensible | Yes | No | No | Varies | Open source, MCP-first |
| Vibe coded by a Gen X-er | No | No | No | No | Absolutely yes |

---

## Contributing

```bash
# Build
cargo build -p aetherd

# Test (per-crate to avoid OOM on constrained machines)
cargo test -p aether-core
cargo test -p aether-parse
cargo test -p aether-store
cargo test -p aether-mcp
cargo test -p aetherd

# Lint
cargo fmt --all --check
cargo clippy --workspace -- -D warnings
```

---

<p align="center">
  <em>AETHER doesn't generate code. It makes sure you — and your AI — actually understand the code you already have.<br/>That's harder, and it matters more.<br/><br/>113,252 lines of Rust. 33 days. One guy. BASIC 2.<br/>The future is already here. It just doesn't know how to code.</em>
</p>
