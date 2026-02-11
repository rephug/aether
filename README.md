# AETHER

AETHER is a local-first code intelligence toolkit for Rust and TypeScript/JavaScript projects.

It watches your workspace, extracts stable symbols, generates per-symbol SIR (Structured Intermediate Representation) summaries, stores everything in `.aether/`, and exposes results through:
- LSP hover (editor UX)
- MCP tools (agent UX, e.g. Claude Code)

## What It Does

- Watches file changes with debounce and incremental symbol diffing.
- Extracts symbols via tree-sitter (`rs`, `ts`, `tsx`, `js`, `jsx`).
- Computes stable symbol IDs so IDs survive line-shift edits.
- Generates SIR for changed symbols using configurable inference providers.
- Stores symbol metadata + canonical SIR in SQLite (`.aether/meta.sqlite`) with optional file mirrors under `.aether/sir/*.json`.
- Stores optional per-symbol embeddings in SQLite for semantic retrieval.
- Searches local symbols by name/path/language from CLI and MCP, with optional semantic/hybrid ranking.
- Serves hover summaries through the AETHER LSP server.
- Serves local lookup/explain tools through the AETHER MCP server.

## What You Can Use It For

- Onboard faster in unfamiliar codebases by hovering functions/classes for intent summaries.
- Keep lightweight, continuously refreshed code intent docs without manual writing.
- Give coding agents structured local context through MCP (status, lookup, explain, get_sir).
- Inspect changed code behavior quickly during refactors and reviews.
- Build local developer tooling on top of stable symbol IDs + local SIR storage.

## Current Scope and Limits

- Languages: Rust and TypeScript/JavaScript family only.
- Focus: understanding and retrieval, not autonomous code edits.
- Inference can run fully without cloud keys using `mock` provider.
- No secrets are stored in this repo; use local env vars.

## Architecture Overview

Main crates:
- `crates/aetherd`: observer/indexer daemon + CLI + LSP launch path
- `crates/aether-parse`: tree-sitter extraction
- `crates/aether-core`: symbol model, stable ID strategy, diffs
- `crates/aether-sir`: SIR schema, validation, canonical JSON, hash
- `crates/aether-store`: SQLite canonical storage + optional local SIR mirror files under `.aether/`
- `crates/aether-infer`: provider traits + `mock`, `gemini`, `qwen3_local` for SIR and embeddings
- `crates/aether-lsp`: stdio LSP hover server
- `crates/aether-mcp`: stdio MCP server exposing local AETHER tools
- `crates/aether-config`: `.aether/config.toml` loader/defaults

## Roadmap

- `docs/roadmap/README.md`: phase/stage plan with scoped deliverables, pass criteria, and copy/paste Codex prompts.

## Quickstart (Build from Source)

```bash
cargo build -p aetherd -p aether-mcp
```

## Getting Started in 5 Minutes

1. Build once:
   - `cargo build -p aetherd -p aether-mcp`
2. Index your current workspace:
   - `cargo run -p aetherd -- --workspace . --print-sir`
3. Start LSP with background indexing:
   - `cargo run -p aetherd -- --workspace . --lsp --index`
4. In VS Code extension dev host, hover a Rust/TS/JS symbol to see SIR.
5. For Claude Code, register MCP:
   - `claude mcp add --transport stdio --scope project aether -- ./target/debug/aether-mcp --workspace .`

### Indexing and SIR generation

```bash
cargo run -p aetherd -- --workspace . --print-sir
```

Run one deterministic indexing pass and exit:

```bash
cargo run -p aetherd -- --workspace . --index-once --print-sir
```

Useful debug flags:

```bash
cargo run -p aetherd -- --workspace . --print-events --print-sir
```

### Run LSP with background indexing

```bash
cargo run -p aetherd -- --workspace . --lsp --index
```

LSP hover rendering is sectioned for readability (`Intent`, `Inputs`, `Outputs`, `Side Effects`, `Dependencies`, `Error Modes`) and includes a stale warning when the latest SIR metadata reports stale status.

### Search symbols

```bash
cargo run -p aetherd -- --workspace . --search "alpha"
```

Semantic/hybrid search modes:

```bash
cargo run -p aetherd -- --workspace . --search "alpha behavior" --search-mode semantic
cargo run -p aetherd -- --workspace . --search "alpha behavior" --search-mode hybrid
```

When semantic/hybrid cannot run (for example embeddings are disabled), AETHER falls back to lexical search and prints the fallback reason to stderr.

Stable JSON output for scripting:

```bash
cargo run -p aetherd -- --workspace . --search "alpha behavior" --search-mode hybrid --output json
```

JSON envelope fields:
- `mode_requested`
- `mode_used`
- `fallback_reason`
- `matches`

Output columns:
- `symbol_id`
- `qualified_name`
- `file_path`
- `language`
- `kind`

## VS Code Extension

The extension lives in `vscode-extension/` and starts AETHER over stdio.

```bash
cd vscode-extension
npm install
npm run build
```

Then open `vscode-extension/` in VS Code and press `F5` to launch an Extension Development Host.

## MCP Server (Claude Code)

Build:

```bash
cargo build -p aether-mcp
```

Register in Claude Code (project scope):

```bash
claude mcp add --transport stdio --scope project aether -- <path-to-aether-mcp> --workspace .
```

Typical paths:
- Linux/macOS: `./target/debug/aether-mcp`
- Windows: `./target/debug/aether-mcp.exe`

MCP tools exposed:
- `aether_status`
- `aether_symbol_lookup`
- `aether_search`
- `aether_get_sir`
- `aether_explain`

Stable MCP response fields for scripting/agents:
- `aether_status`:
  - `schema_version`
  - `generated_at`
  - `workspace`
  - `store_present`
  - `sqlite_path`
  - `sir_dir`
  - `symbol_count`
  - `sir_count`
- `aether_symbol_lookup`:
  - `query`
  - `limit`
  - `mode_requested` (`lexical`)
  - `mode_used` (`lexical`)
  - `fallback_reason` (`null`)
  - `result_count`
  - `matches`
- `aether_search`:
  - `query`
  - `limit`
  - `mode_requested`
  - `mode_used`
  - `fallback_reason`
  - `result_count`
  - `matches`

Search fallback reasons are intentionally aligned between CLI and MCP.

## Configuration

AETHER uses project-local config at:

- `<workspace>/.aether/config.toml`

If missing, it is created automatically.

Current config schema:

```toml
[inference]
provider = "auto" # auto | mock | gemini | qwen3_local
# model = "..."
# endpoint = "..."
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true # optional file mirrors under .aether/sir/

[embeddings]
enabled = false # optional semantic retrieval index
provider = "mock" # mock | qwen3_local
# model = "qwen3-embeddings-0.6B"
# endpoint = "http://127.0.0.1:11434/api/embeddings"
```

Provider behavior:
- `auto`: if `api_key_env` is set -> `gemini`, else `mock`
- `mock`: deterministic local summaries
- `gemini`: requires API key env var
- `qwen3_local`: local HTTP endpoint (default `http://127.0.0.1:11434`)

CLI overrides (optional):

```bash
--inference-provider <auto|mock|gemini|qwen3_local>
--inference-model <name>
--inference-endpoint <url>
--inference-api-key-env <ENV_VAR_NAME>
--search-mode <lexical|semantic|hybrid>
--output <table|json>
```

Override precedence is CLI > config file > built-in defaults.

## Local Data Layout

AETHER writes to `.aether/` under the workspace:
- `.aether/config.toml`
- `.aether/meta.sqlite`
- `.aether/sir/<symbol_id>.json` (optional mirror files)

## Security and Keys

- Never commit API keys.
- Keep `.env` local only.
- Use env vars for secrets (`GEMINI_API_KEY`, or your configured `api_key_env`).
- `mock` works without any key.

## Prebuilt Binaries

GitHub Releases provides prebuilt binaries for Linux, macOS, and Windows:

- https://github.com/rephug/aether/releases

## Development CI/Release

- CI runs `fmt`, `clippy`, and `test` on push/PR.
- Tagging `v*.*.*` triggers cross-platform release builds and uploads binaries to GitHub Releases.
