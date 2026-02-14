<p align="center">
  <h1 align="center">⚡ AETHER</h1>
  <p align="center"><strong>Your codebase, understood.</strong></p>
  <p align="center">
    A local-first code intelligence engine that watches your code, understands what it means,<br>and tells you <em>why it changed</em> — all without leaving your editor.
  </p>
</p>

<p align="center">
  <a href="https://github.com/rephug/aether/actions/workflows/ci.yml"><img src="https://github.com/rephug/aether/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rephug/aether/releases"><img src="https://img.shields.io/github/v/release/rephug/aether?include_prereleases&label=release" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
  <img src="https://img.shields.io/badge/rust-edition%202024-orange" alt="Rust Edition 2024">
  <img src="https://img.shields.io/badge/languages-Rust%20%7C%20TypeScript%20%7C%20JS-blueviolet" alt="Languages">
</p>

<p align="center">
  <a href="#quickstart">Quickstart</a> •
  <a href="#how-it-works">How It Works</a> •
  <a href="#features">Features</a> •
  <a href="#vs-code-extension">VS Code</a> •
  <a href="#mcp-server">MCP for AI Agents</a> •
  <a href="#roadmap">Roadmap</a>
</p>

---

## The Problem

You're onboarding to a 200k-line codebase. You hover over `reconcile_ledger()` in your editor. You get... a type signature. Cool. But what does it *do*? Why was it rewritten last sprint? What side effects does it trigger?

Your AI coding agent asks to understand a module. It gets raw text. No structure. No history. No semantic context. It hallucinates.

**Code intelligence today gives you *what*. AETHER gives you *why*.**

## What AETHER Does

AETHER is a **local-first code intelligence engine** built in Rust. It continuously watches your workspace and builds a living semantic model of your code:

1. **Parses** — Extracts every function, class, struct, trait, and type using tree-sitter
2. **Understands** — Generates structured semantic summaries (SIR) for each symbol via AI inference
3. **Remembers** — Tracks how symbol *meaning* evolves over time, linked to git commits
4. **Explains** — Answers "what does this do?" and "why did it change?" from your editor or AI agent

Everything stays local. Your code never leaves your machine unless you choose a cloud inference provider.

```
┌─────────────────────────────────────────────────────────────────┐
│  Your Code                                                       │
│  ┌──────┐  ┌──────────┐  ┌──────────┐  ┌────────────────────┐  │
│  │ .rs  │  │   .ts    │  │  .tsx    │  │   .js / .jsx       │  │
│  └──┬───┘  └────┬─────┘  └────┬─────┘  └─────────┬──────────┘  │
│     └───────────┴──────────────┴──────────────────┘              │
│                          │                                       │
│                    tree-sitter parse                              │
│                          │                                       │
│                   Symbol Extraction                              │
│                   (stable IDs survive refactors)                 │
│                          │                                       │
│              ┌───────────┴───────────┐                           │
│              ▼                       ▼                           │
│     AI Inference (SIR)        Embedding Index                   │
│     intent · inputs ·        semantic search                    │
│     outputs · side effects    lexical + hybrid                  │
│              │                       │                           │
│              └───────────┬───────────┘                           │
│                          ▼                                       │
│                  .aether/meta.sqlite                             │
│                  (version history + git linkage)                 │
│                          │                                       │
│              ┌───────────┴───────────┐                           │
│              ▼                       ▼                           │
│         LSP Server              MCP Server                      │
│      (hover in editor)      (AI agent tools)                    │
│         VS Code               Claude Code                       │
└─────────────────────────────────────────────────────────────────┘
```

## Quickstart

### Download a Binary

Grab the latest release for your platform:

```bash
# Linux (x86_64)
curl -LO https://github.com/rephug/aether/releases/latest/download/aether-linux-x86_64.tar.gz
tar xzf aether-linux-x86_64.tar.gz
```

Or [browse all releases](https://github.com/rephug/aether/releases) for macOS and Windows builds.

### Build from Source

```bash
git clone https://github.com/rephug/aether.git
cd aether
cargo build -p aetherd -p aether-mcp
```

### Try It in 60 Seconds

```bash
# Index your project and see semantic summaries
cargo run -p aetherd -- --workspace /path/to/your/project --index-once --print-sir

# Search your codebase by meaning, not just text
cargo run -p aetherd -- --workspace . --search "error handling" --search-mode hybrid

# Start the LSP for editor hover intelligence
cargo run -p aetherd -- --workspace . --lsp --index
```

## How It Works

### Structured Intermediate Representation (SIR)

Every symbol in your codebase gets a **SIR annotation** — a structured semantic summary that captures what the code *means*, not just what it *says*:

```json
{
  "intent": "Validates user credentials against the auth store and returns a signed JWT on success",
  "confidence": 0.92,
  "inputs": ["username: string", "password: string"],
  "outputs": ["Result<AuthToken, AuthError>"],
  "side_effects": ["Increments failed_attempts counter on failure", "Writes audit log entry"],
  "dependencies": ["auth_store", "jwt_signer", "audit_logger"],
  "error_modes": ["InvalidCredentials", "AccountLocked", "StoreUnavailable"]
}
```

SIR annotations are **versioned** and **linked to git commits**, so you can trace how a function's *meaning* evolved over time — not just how its text changed.

### Stable Symbol IDs

AETHER computes symbol identities using a combination of language, file path, kind, qualified name, and signature fingerprint — hashed with BLAKE3. This means:

- Reformatting code? **Same ID.**
- Adding blank lines above a function? **Same ID.**
- Renaming a function? **New ID** (correctly tracked as a removal + addition).

This is what makes incremental indexing and version history reliable.

## Features

### 🔍 Semantic Search

Go beyond `grep`. Search your codebase by *intent*, not just text:

```bash
# Lexical (fast, name/path matching)
aetherd --workspace . --search "validate"

# Semantic (meaning-based, uses embeddings)
aetherd --workspace . --search "check if user is allowed" --search-mode semantic

# Hybrid (combines both, best results)
aetherd --workspace . --search "authentication logic" --search-mode hybrid
```

When semantic search can't run (e.g., embeddings disabled), AETHER gracefully falls back to lexical and tells you why.

### 📜 Symbol Timeline & "Why Changed?"

Track how any symbol's meaning evolves:

```bash
# See version history for a symbol
aetherd --workspace . --history <symbol_id>

# Ask why a symbol changed between two points
aetherd --workspace . --why-changed <symbol_id> --from <timestamp> --to <timestamp>
```

Each version entry includes the git commit hash, so you can cross-reference with `git log`.

### ✅ Verification Runner

Run configurable verification commands against your workspace — from the CLI, MCP, or editor:

```toml
# .aether/config.toml
[verify]
mode = "host"  # or "container" for Docker isolation
commands = ["cargo test", "cargo clippy -- -D warnings"]
```

Container mode automatically falls back to host execution when Docker isn't available.

### 🧩 Pluggable Inference

Choose your intelligence backend:

| Provider | Requires | Best For |
|----------|----------|----------|
| `mock` | Nothing | Testing, offline development |
| `gemini` | API key | Production-quality SIR generation |
| `qwen3_local` | Local server | Air-gapped / privacy-first environments |

```toml
# .aether/config.toml
[inference]
provider = "auto"  # uses gemini if key is available, else mock
api_key_env = "GEMINI_API_KEY"
```

## VS Code Extension

AETHER ships with a VS Code extension that gives you semantic hover intelligence directly in your editor.

```bash
cd vscode-extension
npm install && npm run build
# Press F5 in VS Code to launch Extension Development Host
```

**Command Palette:**
- `AETHER: Index Once` — trigger a one-shot indexing pass
- `AETHER: Search Symbols` — semantic symbol search with quick-pick results
- `AETHER: Select Inference Provider` — switch providers on the fly
- `AETHER: Restart Server` — restart the LSP server

The status bar shows real-time indexing state and stale warnings.

## MCP Server

AETHER exposes a full MCP (Model Context Protocol) server, giving AI coding agents structured access to your codebase intelligence.

### Register with Claude Code

```bash
cargo build -p aether-mcp
claude mcp add --transport stdio --scope project aether -- ./target/debug/aether-mcp --workspace .
```

### Available Tools

| Tool | Description |
|------|-------------|
| `aether_status` | Workspace health: symbol count, SIR coverage, store paths |
| `aether_search` | Semantic/lexical/hybrid symbol search |
| `aether_symbol_lookup` | Direct symbol lookup by name |
| `aether_get_sir` | Retrieve full SIR annotation for a symbol |
| `aether_explain` | AI-powered explanation of a symbol |
| `aether_symbol_timeline` | Version history with git commit linkage |
| `aether_why_changed` | Semantic diff between SIR versions |
| `aether_verify` | Run verification commands with structured results |

Every MCP response includes a `schema_version` field for forward compatibility.

## Configuration

AETHER auto-generates a config file at `<workspace>/.aether/config.toml`:

```toml
[inference]
provider = "auto"          # auto | mock | gemini | qwen3_local
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true    # write .aether/sir/*.json alongside SQLite

[embeddings]
enabled = false            # enable for semantic search
provider = "mock"          # mock | qwen3_local

[verify]
mode = "host"              # host | container
commands = ["cargo test"]
```

CLI flags override config values: `--inference-provider`, `--search-mode`, `--output json`, etc.

## Architecture

AETHER is a Rust workspace with clean crate boundaries:

```
crates/
├── aetherd          # Daemon: CLI, observer, indexer, LSP launcher
├── aether-core      # Symbol model, stable IDs, diffing, shared types
├── aether-parse     # tree-sitter extraction (Rust, TS, TSX, JS, JSX)
├── aether-sir       # SIR schema, validation, canonical JSON, hashing
├── aether-store     # SQLite storage, migrations, embeddings, search
├── aether-infer     # Provider traits + mock/gemini/qwen3 implementations
├── aether-lsp       # LSP hover server (stdio)
├── aether-mcp       # MCP server with full tool suite (stdio)
└── aether-config    # Config loader, defaults, validation
```

**Key design decisions:**
- **Local-first**: Everything in `.aether/` — no external services required
- **Stable IDs**: BLAKE3 hash of (lang, path, kind, name, signature) — survives reformatting
- **Incremental**: Only re-processes symbols whose content hash actually changed
- **Graceful degradation**: Semantic → lexical fallback, container → host fallback
- **Dual interface**: LSP for humans, MCP for AI agents — same intelligence, different presentations

## Roadmap

AETHER follows a three-phase roadmap:

| Phase | Name | Status | Description |
|-------|------|--------|-------------|
| **1** | **Observer** | ✅ Complete | Parse, index, SIR, search, LSP, MCP, VS Code, CI/CD |
| **2** | **Historian** | ✅ Complete | SIR versioning, git linkage, "why changed?" queries |
| **3** | **Ghost** | 🔧 In Progress | Verification runners (host ✅, container ✅, microVM 🔜) |

Full roadmap with implementation details and executable prompts: [`docs/roadmap/`](docs/roadmap/README.md)

## Development

```bash
# Format
cargo fmt --all

# Lint
cargo clippy --workspace -- -D warnings

# Test (99 tests across all crates)
cargo test --workspace
```

CI runs on every push and PR. Releases are built for 6 targets (Linux/macOS/Windows × x86_64/ARM64).

## Local Data Layout

```
.aether/
├── config.toml              # Project configuration
├── meta.sqlite              # Canonical store (symbols, SIR, history, embeddings)
└── sir/
    └── <symbol_id>.json     # Optional human-readable SIR mirrors
```

## Security

- No secrets stored in the repo — use environment variables
- `mock` provider works fully offline with zero API keys
- Container verification mode provides isolation for untrusted workspaces
- All data stays local in `.aether/`

## License

[MIT](LICENSE)

---

<p align="center">
  <strong>AETHER doesn't just index your code. It understands it.</strong><br>
  <a href="https://github.com/rephug/aether/releases">Download</a> •
  <a href="https://github.com/rephug/aether/issues">Report a Bug</a> •
  <a href="docs/roadmap/README.md">Roadmap</a>
</p>
