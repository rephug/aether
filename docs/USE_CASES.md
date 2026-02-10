# AETHER Use Cases

## 1) Onboard to a New Codebase

Goal: quickly understand core functions and modules.

1. Run indexing:
   - `cargo run -p aetherd -- --workspace . --print-sir`
2. Open files in editor with AETHER LSP/extension enabled.
3. Hover symbols to read intent, dependencies, and error modes.

## 2) Keep Intent Docs Fresh During Refactors

Goal: avoid stale docs while changing code.

1. Run continuous mode:
   - `cargo run -p aetherd -- --workspace . --print-events --print-sir`
2. Edit/save files.
3. AETHER updates only changed symbols and rewrites corresponding SIR blobs.

## 3) Give Claude Code Local Project Context

Goal: answer "what does this function do?" directly from local data.

1. Build MCP server:
   - `cargo build -p aether-mcp`
2. Register with Claude Code:
   - `claude mcp add --transport stdio --scope project aether -- ./target/debug/aether-mcp --workspace .`
3. Use tools:
   - `aether_symbol_lookup`
   - `aether_explain`
   - `aether_get_sir`

## 4) LSP + Auto-index During Development

Goal: hover answers keep up with edits without manual reruns.

1. Start combined mode:
   - `cargo run -p aetherd -- --workspace . --lsp --index`
2. Editor connects over stdio.
3. Background indexing updates `.aether/` while hover reads latest SIR.

## 5) Run Fully Without Cloud Keys

Goal: work in restricted/offline-like environments.

1. Use mock provider:
   - Config: `[inference] provider = "mock"`
   - or CLI: `--inference-provider mock`
2. Index and hover still function, with deterministic mock summaries.
