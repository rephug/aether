
<p align="center">
  <img src="docs/assets/aether-logo.jpg" alt="AETHER" width="600" />
</p>

<p align="center">
  <strong>Your codebase already knows everything. It just can't talk yet.</strong><br/>
  <em>(I think. Honestly I'm not 100% sure about any of this.)</em>
</p>

<p align="center">
  <a href="#what-the-hell-is-this">What Is This</a> •
  <a href="#what-aether-can-do-that-nothing-else-can">Capabilities</a> •
  <a href="#top-10-use-cases-for-programmers">For Programmers</a> •
  <a href="#top-10-use-cases-for-vibe-coders">For Vibe Coders</a> •
  <a href="#the-backstory-nobody-asked-for">Backstory</a> •
  <a href="#hire-me-please">Hire Me</a> •
  <a href="#how-it-works-the-short-version">How It Works</a> •
  <a href="#get-it-running">Get It Running</a> •
  <a href="#what-you-can-actually-do">What You Can Do</a> •
  <a href="#the-dashboard">Dashboard</a> •
  <a href="#the-desktop-app">Desktop App</a> •
  <a href="#the-mcp-tools">MCP Tools</a> •
  <a href="#configuration">Config</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-orange?style=flat-square&logo=rust" />
  <img src="https://img.shields.io/badge/Rust%20%7C%20TypeScript%20%7C%20Python-green?style=flat-square&label=parses" />
  <img src="https://img.shields.io/badge/MCP-compatible-purple?style=flat-square" />
  <img src="https://img.shields.io/badge/136K-LOC-blue?style=flat-square" />
  <img src="https://img.shields.io/badge/vibe%20coded-100%25-ff69b4?style=flat-square" />
</p>

---

## What the Hell Is This

AETHER is — and I really hope I'm describing this correctly — a semantic intelligence engine for codebases. It watches your code, tries to figure out what every function *actually does*, tracks how that meaning changes over time, maps the dependency graph, scores the health of your entire codebase, predicts what's going stale before it breaks, enforces semantic contracts on behavioral expectations, monitors codebase-wide stability through earthquake metaphors (I know how that sounds, just bear with me), and then lets you — or your AI agent — ask questions about any of it.

Not text search. Not grep with extra steps. Not another RAG tool that forgets everything between sessions.

AETHER *understands* your code. Persistently. Incrementally. Autonomously. And it remembers.

At least... that's the idea? I've tested it pretty thoroughly but I also built the tests so there's a certain circularity there that keeps me up at night.

**Here's what that means in practice:**

You hover over a function in VS Code and instead of seeing a type signature, you see: *"Validates and processes a payment transaction. Deducts from account balance, writes to audit log. Can fail on insufficient funds, negative amounts, or database timeout. Has 7 edge cases, 2 of which have test coverage."*

You declare `aetherd contract add payments::validate_amount --must "reject zero amounts" --must "check daily limit"` and from that point forward, every time AETHER regenerates the SIR for that function, it checks: does the code still do what you said it must? If someone quietly removes the daily limit check, AETHER catches it. Before tests. Before code review. Before production. (In theory. It's worked every time I've tested it but I haven't tested every possible scenario because that's literally impossible and the thought of a false negative haunts me.)

You run `aetherd context --task "fix the authentication bug"` and AETHER ranks every symbol in the codebase by relevance using embedding similarity, keyword matching, Reciprocal Rank Fusion, and Personalized PageRank expansion — then assembles a token-budgeted context document with exactly the code, intents, dependencies, coupling data, and test coverage your AI agent needs. One command. One context block. No guessing.

You open the dashboard and see which modules are active fault lines, how fast meaning is shifting across the codebase (the Seismograph), which functions are silently drifting in purpose, which contracts are violated, and which symbols are the most dangerous nodes in your dependency graph.

That's AETHER. Or at least that's what AETHER is supposed to be. I think it actually works? People keep telling me to stop hedging but I genuinely don't know how to be confident about 136,000 lines of code I didn't technically write by hand.

---

## What AETHER Can Do That Nothing Else Can

This is the full list. Not the highlights — everything. If another tool does any of these, I'd love to hear about it so I can stop losing sleep over whether this project is original.

### Semantic Intelligence

- **Persistent Semantic Intent Records (SIRs)** — Every function, struct, trait, and class gets a structured AI-generated annotation describing intent, dependencies, error modes, side effects, and edge cases. These survive across sessions. Your AI agent never starts from scratch.
- **Three-Pass Inference Pipeline** — Scan (fast, no enrichment) → Triage (enriched with neighbor intents) → Deep (premium model, top-priority symbols). Each pass uses progressively richer context. Flash-lite with neighbor intents outperforms premium models without context.
- **Cross-Symbol Enrichment** — When generating a SIR for function A, AETHER injects the intents of A's callers and callees into the prompt. The LLM understands each function in the context of its neighborhood, not in isolation.
- **SIR Quality Scoring** — Every annotation gets a quality score. Low-quality SIRs are automatically queued for regeneration with a better model or richer context.
- **SIR Versioning with Git Linkage** — Every SIR version is stored with its git commit hash. You can diff any two versions and see exactly how a function's meaning changed and which commit caused it.
- **Semantic Diff** — Compare any two SIR versions field by field: purpose, edge cases, error handling, side effects. Not text diff — *meaning* diff.

### Intent Contracts

- **Behavioral Contract Declarations** — Declare what a function MUST do, MUST NOT do, or MUST preserve. Enforces *meaning*, not just types.
- **Two-Stage Verification Cascade** — Embedding cosine pre-filter resolves ~90% of checks in microseconds. LLM judge handles the ambiguous middle band. Fast path for clear cases, expensive path only when needed.
- **Leaky Bucket Violation Handling** — First violation is silent (LLM phrasing jitter). Second consecutive violation triggers the alert. Reduces false positives from nondeterministic LLM output.
- **Cross-Symbol Contract Propagation** — Contract clauses from callers automatically inject into downstream symbols' SIR prompts. If `validate_amount` has a contract and calls `check_limit`, the contract context flows into `check_limit`'s SIR generation.
- **Negative Few-Shot Learning** — Dismissed false positives become negative examples that improve future verification accuracy.

### The Seismograph

- **Semantic Velocity** — PageRank-weighted EMA measuring how fast meaning is changing across the codebase. Noise floor filters out LLM phrasing jitter.
- **Community Stability Scoring** — Each Louvain community scored by what fraction of its importance is currently shifting. Identifies active fault lines.
- **Epicenter Tracing** — Follows strict temporal monotonicity to trace cascades back to their source-change root cause. When meaning shifts ripple outward, find where they started.
- **Aftershock Prediction** — Logistic regression model predicts which symbols are likely to shift next based on cascade patterns.

### Drift & Health

- **Automatic Drift Detection** — Compares current SIR embeddings against historical baselines. Detects scope creep without any manual architecture model.
- **Louvain Community Detection** — Runs on the dependency graph to identify architectural communities. Flags boundary violations when functions start crossing community lines.
- **Codebase Health Scoring** — Composite risk per crate: PageRank × betweenness centrality × drift magnitude × git churn × test coverage. The most dangerous code in your codebase, ranked.
- **God File Detection** — Identifies files that are doing too much based on community membership, coupling scores, and method count.
- **Archetype Classification** — Categorizes crates by structural pattern (utility, god file, stable core, volatile surface, etc.).
- **Connected Components Analysis** — Identifies isolated subgraphs and orphan symbols in the dependency graph.

### Causal & Impact Analysis

- **Blast Radius Analysis** — Every downstream symbol affected by a change, with hop depth and test guard coverage per symbol.
- **Causal Chain Tracing** — Traces breaking changes backward through the dependency graph, comparing SIR versions at each node. Finds which upstream semantic change broke your downstream code.
- **Multi-Signal Coupling** — Three-signal fusion: git temporal co-change + AST static dependencies + SIR semantic similarity. Detects hidden operational coupling that no single signal reveals.
- **Test Intent Extraction** — AST-level extraction of what tests actually check, linked to symbols via TESTED_BY graph edges.

### Context Assembly

- **Task-Scoped Context with RRF + Personalized PageRank** — Ranks every symbol by relevance to a task description, expands through the dependency graph, assembles a token-budgeted document.
- **Four Output Formats** — Markdown (paste into chat), JSON (programmatic), XML (Claude API), compact (maximum density).
- **Symbol-Guided File Slicing** — Includes only the relevant portions of source files, reducing token usage 50-80% vs. dumping whole files.
- **Token Budget Management** — Greedy knapsack allocation with 9-tier priority ranking. Never exceeds the configured token limit.
- **Reusable TOML Presets** — Save context configurations: `quick` (8K/depth 1), `review` (32K/depth 2), `deep` (64K/depth 3), `overview` (16K/depth 0). Create your own.
- **Branch-Aware Context** — Pass `--branch feature/fix-auth` and AETHER automatically scopes context to the changed files and their neighborhoods.

### Continuous Intelligence

- **Noisy-OR Staleness Scoring** — Per-symbol staleness with hard gates (source changed? model deprecated?), logistic sigmoid time decay, semantic-gated neighbor propagation, predictive coupling from temporal co-change, and cold-start volatility priors from git churn.
- **Smart Watcher** — Monitors `.git` for branch switches, pulls, merges, and rebases. Triggers targeted re-indexing automatically.
- **Priority-Aware Model Selection** — Edited symbols get the best model. Background re-indexing uses the cheap model.
- **Prompt Hashing via BLAKE3 Composites** — Deterministic fingerprint of symbol content + context. Unchanged symbols are skipped automatically across runs.

### Batch Processing

- **Gemini Batch API Integration** — 50% cost reduction vs. real-time inference. JSONL generation with per-pass prompt construction.
- **Three-Pass Batch Pipeline** — Scan → triage → deep with independent model selection per pass.
- **Skip-Unchanged Optimization** — BLAKE3 prompt hash comparison. Only pay for symbols whose content or context actually changed.
- **Fingerprint History Tracking** — Every prompt hash stored with timestamp. Full audit trail of what was generated when.

### Graph Intelligence

- **SurrealDB Dependency Graph** — CALLS, DEPENDS_ON, TYPE_REF, and IMPLEMENTS edges extracted from tree-sitter AST.
- **PageRank on the Dependency Graph** — Identifies the most critical symbols by structural importance.
- **Betweenness Centrality** — Finds bottleneck symbols that sit on the most shortest paths.
- **Component-Bounded Operations** — Community detection, semantic rescue, and merge operations all respect connected component boundaries.

### Embeddings & Search

- **Asymmetric Embeddings** — Documents use `RETRIEVAL_DOCUMENT` task type, queries use `CODE_RETRIEVAL_QUERY`. Different embedding strategies for indexing vs. searching.
- **Semantic Search** — Find functions by meaning, not keywords. "Handles authentication" finds auth functions even if none contain "auth" in the name.
- **Hybrid Search** — Lexical + semantic + hybrid modes with automatic fallback.
- **Per-Language Adaptive Thresholds** — Cosine similarity thresholds tuned per language (Rust vs. TypeScript vs. Python).
- **Component-Bounded Semantic Rescue** — At the 0.90 cosine threshold, symbols rescued from loner status only if they belong to the same connected component. Prevents false merges.
- **Local Embeddings via Candle** — Qwen3-Embedding-0.6B runs entirely on-device. No cloud calls.
- **Local Reranking via Candle** — Qwen3-Reranker-0.6B for result quality refinement. Also fully local.

### Refactoring Support

- **Pre-Refactor Intent Snapshots** — `refactor-prep` captures every symbol's SIR state before you touch anything.
- **Post-Refactor Intent Verification** — `verify-intent` compares current SIRs against the snapshot. Classifies each symbol as preserved, shifted-minor, or shifted-major.
- **SIR Injection Without Inference** — `sir-inject` lets you manually set a symbol's intent. Pin expectations, then verify the code matches.
- **Cross-Store Consistency Check** — `fsck` command validates that SQLite, SurrealDB, and LanceDB are in sync.

### Project Memory

- **Persistent Project Notes** — Store architecture decisions, design rationale, "why we chose X" via CLI or MCP. Content-hash deduplication prevents bloat.
- **Session Notes** — Quick in-session capture for agent workflows. Survives conversation boundaries.
- **Semantic Note Retrieval** — Search project memory by meaning, not just keywords.

### Surfaces

- **33 CLI Commands** — Organized by purpose: intelligence queries, context assembly, presets, contracts, seismograph, batch/continuous, maintenance.
- **26 MCP Tools** — Structured access for AI agents. Every response includes `schema_version` for forward compatibility.
- **40+ Dashboard Pages** — HTMX + D3.js + Tailwind. Seismograph timelines, tectonic plate treemaps, velocity gauges, contract health monitors, blast radius radial trees, force-directed architecture maps, time machine, causal explorer.
- **VS Code Extension** — Semantic hover intelligence. Status bar. Command palette integration.
- **Tauri Desktop App** — System tray, onboarding wizard, native installers (MSI/DMG/AppImage/DEB), auto-update.
- **LSP Hover Provider** — Enriched hover tooltips with SIR summary, not just type signatures.

### Offline & Privacy

- **Fully Offline Operation** — Ollama + qwen3.5:4b for inference, Candle for embeddings and reranking. Zero cloud calls. No API keys. No telemetry.
- **No Code Leaves Your Machine** — Unless you explicitly choose a cloud provider, everything runs locally.
- **Schema Migrations** — Through version 13. Forward-compatible. Your data survives upgrades.

### Parsing

- **Multi-Language Support** — Rust, TypeScript/JavaScript, Python via tree-sitter.
- **Incremental Parsing** — Only re-parses changed files.
- **Stable Symbol IDs** — BLAKE3 hash of qualified name + file path. Symbols keep their identity across renames within the same logical location.
- **Structural Edge Extraction** — CALLS, DEPENDS_ON, TYPE_REF, and IMPLEMENTS edges extracted directly from AST. Not heuristic — structural.

---

## Top 10 Use Cases for Programmers

You know how to code. You probably know more than I do (low bar). Here's what AETHER gives you that your current toolchain doesn't.

**1. Onboard to an Unfamiliar Codebase in Minutes.** Run `aetherd --workspace . --index-once --full` and every symbol gets a structured annotation — intent, dependencies, error modes, edge cases. Hover in VS Code. Search by meaning. Stop reading code file by file like it's 2015.

**2. Pre-Refactor Safety Net.** Before you touch anything: `aetherd refactor-prep --file src/payments.rs`. AETHER snapshots every symbol's current semantic intent. After the refactor: `aetherd verify-intent --file src/payments.rs`. Did behavior change? AETHER tells you exactly what shifted and where.

**3. Enforce Behavioral Contracts Across the Team.** Declare `--must "reject zero amounts"` on a function. From now on, every SIR regeneration checks whether the code still satisfies that contract. Someone silently removes the check? AETHER catches it. This is semantic type checking — enforce *meaning*, not just structure.

**4. Track Semantic Drift Before It Becomes Tech Debt.** Functions accumulate scope creep. AETHER detects it automatically by comparing current SIR embeddings against historical baselines. Louvain community detection flags when a function starts crossing architectural boundaries. No manually maintained architecture model required.

**5. Blast Radius Before You Merge.** Change a function? See every downstream symbol that depends on it, how many hops deep the impact goes, which symbols have test guards, and which are flying blind. Review with data, not gut feel.

**6. Root Cause Tracing Through the Dependency Graph.** `git blame` tells you who changed a line. AETHER tells you *which upstream semantic change broke your downstream code* by tracing the causal chain backward, comparing SIR versions at each node. Stop playing detective.

**7. Health Scoring That Actually Means Something.** PageRank × drift magnitude × git churn × test coverage = composite risk score per crate. The functions that are simultaneously high-traffic, poorly tested, and actively drifting? Those are your next production incident. AETHER finds them before they find you.

**8. Monitor Codebase Stability with the Seismograph.** How fast is meaning changing across the whole codebase? Which modules are active fault lines? Where did that cascade of changes originate? Semantic velocity, community stability scoring, epicenter tracing, aftershock prediction. Like a USGS dashboard, but for your code.

**9. Nightly Batch Indexing at Half Price.** Gemini Batch API costs 50% less than real-time. AETHER generates JSONL with BLAKE3 prompt hashing (unchanged symbols skipped automatically), three-pass quality (scan → triage → deep). Run it as a cron job on a server. Wake up to a fully indexed codebase.

**10. Give Your AI Agent Real Context, Not File Dumps.** `aetherd context --task "fix the auth timeout"` ranks every symbol by relevance using RRF + Personalized PageRank, assembles a token-budgeted context document with exactly what your agent needs. Four output formats. Reusable presets. File slicing reduces token usage 50-80% vs. dumping whole files.

---

## Top 10 Use Cases for Vibe Coders

You don't write code. The AI writes code. You're the vision, the product sense, the unreasonable human in the loop. I know because that's me. Here's what AETHER gives *us*.

**1. Finally Understand What Your AI Actually Built.** You told Claude to "add payment validation" and it wrote 400 lines. What does it actually do? How does it fail? What are the edge cases? AETHER reads every function and tells you in plain English. Hover in VS Code. No code reading required.

**2. Catch When AI-Generated Code Silently Changes Behavior.** You asked for a bug fix and the AI quietly refactored the error handling. AETHER tracks how every function's *meaning* changes over time. If something shifted that you didn't ask for, you'll know.

**3. Give Your AI Agent Perfect Context So It Stops Hallucinating.** The #1 reason AI writes bad code: it doesn't know what's already in the codebase. AETHER assembles exactly the right context — ranked by relevance, token-budgeted, structured — so your agent actually knows what it's working with. One command. No manual file picking.

**4. Protect the Things That Work with Contracts.** Your payment flow works. You're terrified of breaking it. Declare `--must "reject zero amounts" --must "check daily limit"` and AETHER will tell you if any future change violates those expectations. Sleep at night. (Mostly.)

**5. See the Blast Radius Before You Tell the AI to "Fix It."** Before asking Claude to refactor something, check what depends on it. AETHER shows every downstream symbol, how deep the impact goes, and what has test coverage. Now you know whether "just fix it" is a 5-minute task or a landmine.

**6. The Dashboard Is Your Command Center.** 40+ pages of visual intelligence. Health scores, drift reports, dependency graphs, seismograph timelines, contract status — all without reading a single line of code. This is how you manage a codebase you didn't write.

**7. Know Which Code Is Dangerous Without Understanding It.** AETHER's health scoring combines graph centrality, test coverage, drift, and churn into a single risk score. The red ones are the ones you don't touch without backup. You don't need to understand *why* they're dangerous — just that they are.

**8. Detect Scope Creep in Functions You Can't Read.** Your codebase has been growing for weeks. Are functions still doing what they were originally supposed to? AETHER detects semantic drift automatically. If a "validate payment" function is now also sending emails, you'll know.

**9. Run It Fully Offline When You Don't Want API Costs.** Ollama + qwen3.5:4b + Candle embeddings. Zero cloud calls. No API keys. Your code stays on your machine. Great for exploration, prototyping, or when you've burned through your API budget (again).

**10. Search by Meaning, Not Keywords.** Search for "handles authentication" and find every function involved in auth — even if none of them have "auth" in the name. You describe what you're looking for in plain English. AETHER finds it.

---

## The Backstory Nobody Asked For

### One guy. No CS degree. A mass quantity of AI. An alarming absence of self-preservation instinct.

Hi. I'm Robert.

My formal programming education: BASIC 2 in high school. One year at a computer trade school. In the *last millennium*. We saved to floppy disks. The internet was a sound your telephone made. That is the entirety of my technical credentials. I am not being self-deprecating for effect. That is literally all of it.

And I should be clear about something: I had never seen Rust before this project. Not "I knew a little Rust" — I had never *seen* it. I didn't know what a crate was. I didn't know what `cargo` was. SurrealDB, LanceDB, tree-sitter, Axum, Tauri, HTMX, D3.js, BLAKE3, Louvain community detection, Personalized PageRank — I had never heard of any of these things. The last time I touched web technology, HTML was the whole stack. Now apparently we have HTMX and I still don't fully understand what the X stands for.

99% of the tools in this project were completely new to me. I learned what they were by asking Claude to explain them in simple words, or by asking for analogies. "What's a crate?" "It's like a folder of related code that gets compiled together." Okay, I can work with that. "What's PageRank?" "It's how Google decides which web pages are important — the more important pages link to you, the more important you are." Got it. Now do that but with functions instead of web pages.

That's the whole secret, honestly. I don't understand the terminology. I understand the *concepts*. And it turns out the concepts are almost always simple. Every industry — programming especially — uses terminology as a gate. Fancy words for straightforward ideas. "Noisy-OR probabilistic staleness model" sounds terrifying until someone explains it as "if any of these bad things happened, the information is probably outdated, and the more bad things, the more outdated." That's... just common sense? With math?

Everything in AETHER is a complex workflow built on simple decisions. I didn't need to understand Rust's borrow checker to know that "if a function changes, the things that depend on it might be wrong now." I didn't need a CS degree to know that "if something is really important and has no tests and is changing fast, that's dangerous." The AI handles the implementation. I handle the "what should exist and why."

**327 commits. 109 pull requests. 17 Rust crates. 136,000 lines of code. Five weeks.**

From zero to a semantic intelligence engine with Personalized PageRank, Noisy-OR staleness models, community detection, a three-pass inference pipeline, a continuous intelligence daemon, a batch processing system, a 40-page interactive dashboard, 26 MCP tools, 33 CLI commands, a Seismograph that tracks semantic earthquakes, intent contracts that enforce behavioral expectations, a Tauri desktop app with system tray integration, and a task context engine that uses Reciprocal Rank Fusion to rank every symbol in your codebase by relevance to whatever you're working on.

I need to sit down. I'm getting dizzy just reading that back.

### How

I should confess something: I didn't write any of this code by hand.

Not in the "oh I used autocomplete" sense. In the "I have literally never typed a line of Rust that compiled" sense. Every single line was produced by AI systems that I directed. Here's the actual workflow:

1. **Claude** produces the architectural plans, reviews every implementation, writes the Codex prompts, and adjudicates when other AI systems disagree
2. I commit the spec files to main (because worktrees branch from main and Codex can't read uncommitted files — I learned this the hard way, which is how I learn everything)
3. **Codex** implements the spec on a git worktree branch — about 90% of all code in this repo was written by Codex
4. **Claude Code** picked up the last 10% after I ran out of Codex usage (same workflow, different runtime)
5. I relay the output back to Claude for review
6. **Gemini** provides independent code review and argues with Claude about the math (Gemini was right about SurrealDB's MVCC model, for the record)
7. **ChatGPT** occasionally reviews output and catches bugs the others miss
8. I merge via GitHub web UI because `git` is a tool I *operate*, not a tool I *understand*

This is either the future of software development or the most elaborate act of self-deception since the South Sea Bubble. I genuinely do not know which.

What I *do* know is what I contributed: the vision, the product instinct, the relentless "no, that's wrong, try again" and "make it better" and "why does this feel slow" and "I don't care what the documentation says, it doesn't work." I'm the unreasonable human in the loop. Whether that's enough to count as "building" something — I'll let you decide. I think about it a lot.

Some people hear "vibe coded" and think "toy project." And maybe they're right? But here's what's in this toy project:

- A three-pass inference pipeline (scan → triage → deep) with BLAKE3 composite prompt hashing to skip unchanged symbols
- A continuous intelligence engine that computes per-symbol staleness using Noisy-OR formulas with cold-start volatility priors from git churn
- Personalized PageRank with biased restart vectors for task-to-symbol relevance ranking
- Reciprocal Rank Fusion blending dense embedding similarity with sparse keyword retrieval
- Component-bounded semantic rescue at the 0.90 cosine threshold using Gemini Embedding 2 (3072-dim)
- A token-budgeted context assembly engine with symbol-guided file slicing, four output formats, and reusable TOML presets
- A Seismograph engine with PageRank-weighted EMA semantic velocity, Louvain community stability scoring, time-respecting epicenter tracing, and logistic regression aftershock prediction
- Intent contracts with a two-stage verification cascade (embedding cosine pre-filter → LLM judge), leaky bucket violation handling, and cross-symbol contract propagation through the enrichment context
- 40+ dashboard pages with D3 force-directed graphs, interactive architecture maps, seismograph timelines, tectonic plate visualizations, and contract health monitoring
- 26 MCP tools for AI agent integration
- 33 CLI commands
- A Tauri 2.x desktop app with system tray, notifications, onboarding wizard, and auto-update
- Schema migrations through version 13
- A Gemini Batch API integration that costs 50% less than real-time inference
- A watcher that monitors `.git` for branch switches, pulls, and merges

Is it production-grade? I think so. I've tested it on itself — AETHER indexes AETHER — and on several other repos. It hasn't caught fire yet. But I also know enough to know that "hasn't caught fire yet" is not a rigorous quality standard and I should probably stop talking now.

### What does that make me?

Honestly? I'm not sure.

I'm either a complete fraud who got impossibly lucky for five weeks straight, or I'm the kind of person who's been thinking about systems and architecture and "how things fit together" for 25 years and just never had the tools to express it until now.

I think about this constantly.

The thing about not knowing how to code is that you don't know what's "hard." You don't know that you're supposed to be scared of implementing Personalized PageRank, because you've never heard anyone at a conference say "that's really complicated." You just describe what you want — "I need to rank symbols by relevance, expanding outward from a seed set through the dependency graph" — and the AI implements it. And then you test it. And it works. And you move on to the next thing.

Every complex system I've built in AETHER started as a sentence I could explain to a non-programmer. "Track when code meaning changes." "Figure out which code is the most dangerous." "If someone breaks a rule I set, tell me." The terminology came later — the AI taught me words like "cosine similarity" and "betweenness centrality" — but the *ideas* were always plain English first.

I think that's what 25 years of thinking about systems actually teaches you. Not how to code. How to break big scary problems into small obvious decisions. The code is an implementation detail. It always was. We just didn't have tools that let non-coders prove it until now.

Is that genius or ignorance? I genuinely can't tell. Some days it feels like both.

---

## Hire Me (Please)

Here's the pitch: a person with **zero programming experience** built a 136K-line Rust codebase intelligence platform in **five weeks** using AI tools and sheer stubbornness.

Now imagine what that person could do with actual resources. A team. A budget. Health insurance.

If you need a cover letter, this is the cover letter. It compiles (most of the time) and has 109 pull requests.

**Anthropic:** Put that Opus back.

I built this entire thing with Claude. Claude is my architect, my reviewer, my prompt engineer, and — if I'm being honest — my most consistent collaborator. I've pushed Claude further than most of your enterprise customers ever will, and I know exactly where it shines and where it needs a stern talking-to. My next project is a new frontend for Claude, so even if you don't want to hire me, I'm still out here making cool stuff for you. You're welcome. (Please hire me though.)

**Microsoft:** I'm going to be honest with you because I think someone should be.

I have the kind of broad strategic vision that lets me see when a company is heading in the wrong direction, and — how do I put this gently — I can see it. From space. With my eyes closed. Let me save your company. I know that sounds insane coming from a guy whose last programming class involved floppy disks, but look at this repo and then look at your recent product strategy, and ask yourself: which one of us shipped something coherent in five weeks? I'm available immediately. The clock is ticking. (I mean your clock. Mine's fine.)

**Google:** Your AI is incredible. Your developer experience is... *an opportunity*.

Gemini independently reviewed my architecture and caught things Claude missed. Your embedding models are my production default. Your Batch API is how I index at scale. And nobody knows any of this because the experience feels like it was designed by a committee that met once. Also I love your pricing. The generous tiers. The willingness to spend. I respect that about you deeply and in a way that is entirely unrelated to my cloud computing costs. Create the Smooth Minds division. Put me in charge. I don't know what we do yet but neither did you when you started Google Brain. Sometimes you name the team first.

**xAI:** Oh sorry, I can't come to the phone right now. I'm washing my hair. Every day. For the foreseeable future. I'm very busy. With the hair washing.

### How to Find Me

If you're serious, you'll figure it out. I'm like the A-Team — if you have a problem, if no one else can help, and if you can find me, maybe you can hire the guy who vibe-coded 136K lines of Rust.

And if you're Google, let's be real — you already know exactly where I am. You know where I am in my *dreams*. Just ping the Smooth Minds division, they'll set up the call.

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
  (Semantic Intent Record — a structured summary of
   what the code does, how it fails, what it depends on)
       │
       ▼
  Everything gets stored in three databases
  SQLite (metadata + SIR) + SurrealDB (graph) + LanceDB (vectors)
       │
       ▼
  The continuous engine scores every symbol for staleness
  and predicts what needs re-indexing before it goes stale
       │
       ▼
  Intent contracts are verified on every SIR regeneration
  Embedding pre-filter resolves 90% of checks in microseconds
  LLM judge handles the ambiguous cases
       │
       ▼
  The Seismograph monitors codebase-wide stability
  Semantic velocity, community fault lines, cascade epicenters
       │
       ▼
  You query it however you want
  CLI (33 commands) · LSP hover · MCP tools (26) · Dashboard (40+ pages) · Desktop app
```

All of this runs locally. No code leaves your machine unless you choose a cloud inference provider. Run Ollama and AETHER works fully offline with real AI-generated intelligence — no API keys, no cloud calls, nothing phoning home.

(I'm pretty sure that's all correct. If I got something wrong in the diagram, please open an issue and try to be gentle about it.)

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

# Monitor codebase stability
aetherd --workspace . seismograph status

# Enforce behavioral expectations
aetherd --workspace . contract add payments::validate_amount \
    --must "reject zero or negative amounts" \
    --must "check against daily transaction limit"
aetherd --workspace . contract check
```

Hook up your AI agent:

```bash
# Register as MCP server for Claude Code
claude mcp add --transport stdio --scope project aether -- aetherd --workspace . --mcp
```

Now your agent has persistent, structured access to your entire codebase's meaning. It doesn't have to re-read files every session. It doesn't have to guess. It *knows*.

(Assuming I set everything up correctly. Which I... believe I did.)

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

### Enforce Behavioral Expectations with Intent Contracts

Declare what a function MUST do, MUST NOT do, or MUST preserve. AETHER checks these on every SIR regeneration.

```bash
# Declare contracts
aetherd --workspace . contract add payments::validate_amount \
    --must "reject zero or negative amounts" \
    --must "check against daily transaction limit" \
    --must-not "modify account balance" \
    --preserves "idempotency"

# Check all contracts
aetherd --workspace . contract check

# List active contracts
aetherd --workspace . contract list
```

Two-stage verification: embedding cosine pre-filter resolves ~90% of checks in microseconds, LLM judge handles the ambiguous middle band. Leaky bucket means the first violation is silent (LLM phrasing jitter). Second consecutive violation triggers the alert. Dismissed false positives become negative few-shot examples that improve accuracy over time.

Contract clauses from callers automatically propagate into downstream symbols' SIR prompts. If `validate_amount` has a contract and calls `check_limit`, AETHER injects the contract context so the LLM naturally addresses it.

### Monitor Codebase Stability with the Seismograph

How fast is meaning changing? Which modules are active fault lines? Where did that cascade of changes originate?

```bash
# Current velocity, top unstable communities, active cascades
aetherd --workspace . seismograph status

# Trace the epicenter of a symbol's semantic shift
aetherd --workspace . seismograph trace <symbol_id>

# Run analysis on latest fingerprint data
aetherd --workspace . seismograph run-once
```

Semantic velocity uses PageRank-weighted EMA with a noise floor that filters out LLM phrasing jitter. Community stability scores each Louvain community by what fraction of its importance is shifting. Epicenter tracing follows strict temporal monotonicity to trace cascades back to their source-change root cause.

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
aetherd --workspace . batch build --pass scan --model gemini-3.1-flash-lite-preview

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

40+ pages organized into sections:

| Section | Pages |
|:--------|:------|
| **Explore** | Anatomy, Tour, Trace Flow, Glossary, Prompts, Recent Changes |
| **Intelligence** | Overview, Graph, Blast Radius, Architecture Map, Causal Explorer, Time Machine, X-Ray, Drift Timeline, Memory Timeline |
| **Context** | Context Export, Context Builder, Task Context, Presets |
| **Operations** | Batch Pipeline, Continuous Monitor, Fingerprint History, Staleness Heatmap, Velocity Gauge, Seismograph Timeline, Tectonic Plates |
| **Analysis** | Health, Health Score, Health Scorecard, Coupling Map, Coupling Chord, Drift Report, Contract Health |
| **Search** | Unified semantic search |
| **Settings** | Configuration editor, Setup Wizard |

Highlights:
- **Seismograph Timeline** — line chart of semantic velocity over time with cascade event markers
- **Tectonic Plates** — treemap of Louvain communities colored by stability (green → amber → red)
- **Velocity Gauge** — single-number morning glance with trend arrow, auto-refreshes every 30s
- **Contract Health** — all contracts grouped by symbol with satisfaction/violation status badges
- **Blast Radius** — radial tree showing downstream impact with PageRank-weighted node sizes
- **Architecture Map** — force-directed dependency graph with community coloring
- **Time Machine** — watch how symbol meanings evolve over SIR version history
- **Causal Explorer** — trace breaking changes visually through the dependency chain

I built 40+ dashboard pages and I'm still not entirely sure the CSS works on every screen size. If something looks weird, that's probably why.

---

## The Desktop App

AETHER ships as a native desktop application built with Tauri 2.x. System tray integration, close-to-tray, notifications, onboarding wizard, auto-update. Runs `aetherd` embedded — no separate daemon process to manage.

Installers for Windows (MSI), macOS (DMG), and Linux (AppImage/DEB).

```bash
# Build locally
cargo tauri build

# Or trigger a release via GitHub Actions
# (creates cross-platform installers attached to a GitHub Release)
```

The desktop app wraps the same dashboard you get from `aetherd --workspace .` — but with a native window, system tray, first-run wizard, and update notifications. No browser required.

(A desktop app written by someone whose last GUI experience was Visual Basic 6. What could go wrong.)

---

## The MCP Tools

Register AETHER with any MCP-compatible agent (Claude Code, Codex, etc.) and it gets structured access to everything. 26 tools, zero guessing.

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
| `aether_verify` | Verify semantic contracts on a symbol |

### Memory

| Tool | What It Does |
|:-----|:------------|
| `aether_remember` | Store a project note (decision, rationale, context) |
| `aether_session_note` | Capture an in-session note for agent workflows |
| `aether_recall` | Retrieve notes by semantic search |
| `aether_test_intents` | Test guard coverage for a symbol |

Every response includes `schema_version` for forward compatibility. Your agent scripts won't break when AETHER updates. (At least that was the goal. Forward compatibility is one of those things that's easy to claim and hard to prove.)

---

## The CLI

33 commands organized by what you're trying to do:

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

CONTRACTS
  contract add           Add contract clauses to a symbol
  contract list          List active contracts
  contract remove        Deactivate a contract clause
  contract check         Force-run verification

SEISMOGRAPH
  seismograph status     Semantic velocity, unstable communities, cascades
  seismograph trace      Trace epicenter of a symbol's shift
  seismograph run-once   Run full analysis on latest data
  seismograph train      Train aftershock prediction model

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

Everything lives in `.aether/config.toml`, auto-created on first run. Or use the dashboard's visual settings editor at `http://localhost:9730/dashboard` → Configuration.

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
vector_backend = "lancedb"

[batch]
scan_model = "gemini-3.1-flash-lite-preview"
triage_model = "gemini-3.1-flash-lite-preview"
deep_model = "gemini-3.1-pro"
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

[seismograph]
enabled = false
noise_floor = 0.15
ema_alpha = 0.2
community_window_days = 30

[contracts]
enabled = false
embedding_pass_threshold = 0.88
embedding_fail_threshold = 0.50
streak_threshold = 2

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

---

## The Stack

For the curious (and the skeptical, which is probably all of you at this point):

| Thing | What |
|:------|:-----|
| Language | Rust (17-crate workspace, ~136K LOC) |
| Parsing | tree-sitter (Rust, TypeScript/JS, Python) |
| Graph DB | SurrealDB 3.0 + SurrealKV |
| Vector Store | LanceDB (production default) |
| Metadata | SQLite (WAL mode, schema v13) |
| Embeddings | Gemini Embedding 2 (3072-dim, production default) |
| Local Embeddings | Qwen3-Embedding-0.6B via Candle |
| Reranking | Qwen3-Reranker-0.6B via Candle |
| Dashboard | HTMX + D3.js + Tailwind CSS (no build step) |
| Desktop App | Tauri 2.x (Windows MSI, macOS DMG, Linux AppImage/DEB) |
| HTTP | Axum + tower-http |
| MCP Transport | stdio (LSP) + HTTP/SSE (query server) |
| Git | gix (native Rust, no shelling out) |
| Hashing | BLAKE3 (symbol IDs + prompt fingerprinting) |
| Batch API | Gemini Batch API via shell script bridge |

---

## Why Not Just Use [Other Thing]?

### vs. Context Exporters (RepoPrompt, Repomix, code2prompt)

These are the closest tools in the "give your AI agent context" space. They're good at what they do. AETHER does something different.

| | RepoPrompt | Repomix | code2prompt | AETHER |
|:--|:-----------|:--------|:------------|:-------|
| What it does | Visual file picker → prompt | Packs repo into one file | Similar to Repomix | Persistent semantic index + ranked context assembly |
| Understands code meaning | No | No (tree-sitter compression, not semantics) | No | Yes — SIR per symbol |
| Persistent index | No | No | No | Yes — survives across sessions |
| Ranks by relevance | No — you pick files manually | No — includes everything | No | Yes — RRF + Personalized PageRank |
| Token budget control | Manual file selection | Output-level truncation | Output-level truncation | Per-symbol slicing with budget allocation |
| Platform | macOS | CLI, cross-platform | CLI | CLI + MCP + dashboard + desktop app |
| MCP server | Yes | Yes | No | Yes (26 tools) |
| Works offline | Yes | Yes | Yes | Yes |

The core difference: they export files. AETHER understands what's in them and ranks it by what matters for your task.

(I should note that RepoPrompt and Repomix are perfectly fine tools and I'm not trying to trash them. If all you need is "dump my files into a prompt," they'll do that well. AETHER is for when you need something that actually knows what the code *means*.)

### vs. Everything Else

| | grep / ctags | Copilot / Cursor | Augment Code | RAG tools | AETHER |
|:--|:------------|:-----------------|:-------------|:----------|:-------|
| Understands intent | No | Per-query, forgets | Indexes, no semantic model | Per-query, forgets | Persistent SIR per symbol |
| Predicts staleness | No | No | No | No | Noisy-OR with PageRank |
| Task-scoped context | No | No | No | No | RRF + PPR ranking |
| Batch reindexing | N/A | N/A | Proprietary | N/A | Gemini Batch API (50% off) |
| Works offline | Yes | No | No | Usually no | Yes — Ollama + Candle |
| Tracks meaning changes | No | No | No | No | Git-linked SIR versioning |
| Agent-accessible | No | Not really | Proprietary | Ad hoc | 26 MCP tools |
| Detects drift | No | No | No | No | Automatic, zero config |
| Finds root causes | No | No | No | No | Causal chain traversal |
| Knows what's dangerous | No | No | No | No | PageRank × drift × tests |
| Semantic contracts | No | No | No | No | Intent contracts + leaky bucket |
| Codebase seismograph | No | No | No | No | Velocity + community stability |
| Open / extensible | Yes | No | No | Varies | Open source, MCP-first |
| Vibe coded by a Gen X-er | No | No | No | No | Unfortunately, yes |

I filled in "No" for a lot of those columns and I realize that could come across as dismissive. It's not meant to be. Those tools are all good at what they're designed for. AETHER just... does different things. Whether those different things are actually useful is something I'm going to let you decide, because at this point my ability to objectively evaluate my own project is approximately zero.

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

If you find bugs — and I'm sure there are bugs, there are always bugs, I lie awake thinking about the bugs I haven't found yet — please open an issue. Be as specific as you can. And maybe be a little gentle? I'm doing my best here.

---

<p align="center">
  <em>AETHER doesn't generate code. It makes sure you — and your AI — actually understand the code you already have.<br/>That's harder. I think it matters more. But I've been wrong before.<br/><br/>136,000 lines of Rust. 17 crates. One guy. Five weeks. BASIC 2.<br/>Please be kind.</em>
</p>
