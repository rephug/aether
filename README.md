# AETHER

AETHER is a local code observer that extracts symbols, stores SIR annotations, serves hover via LSP, and exposes lookup tools through MCP.

## Quickstart (Build from Source)

```bash
cargo build -p aetherd -p aether-mcp
```

Run indexing in your workspace:

```bash
cargo run -p aetherd -- --workspace . --print-sir
```

`aetherd` also supports LSP + background indexing together:

```bash
cargo run -p aetherd -- --workspace . --lsp --index
```

## Indexing

Indexing parses files, diffs symbol changes, and stores symbol + SIR data in `.aether/`.

Primary command:

```bash
cargo run -p aetherd -- --workspace . --print-sir
```

## VS Code Extension

The extension is in `vscode-extension/`.

```bash
cd vscode-extension
npm install
npm run build
```

Then press `F5` in VS Code to run the Extension Development Host.

## MCP Server (Claude Code)

Build the MCP binary:

```bash
cargo build -p aether-mcp
```

Register it in Claude Code (project scope):

```bash
claude mcp add --transport stdio --scope project aether -- <path-to-aether-mcp> --workspace .
```

Example binary path from this repo:
- Linux/macOS: `./target/debug/aether-mcp`
- Windows: `./target/debug/aether-mcp.exe`

## API Keys and Environment Variables

- Do not put keys in the repository.
- Set keys locally via environment variables only.
- `GEMINI_API_KEY` is optional; if unset, AETHER can run with mock inference.

## Prebuilt Binaries

Prebuilt binaries are published on GitHub Releases:

- https://github.com/OWNER/REPO/releases
