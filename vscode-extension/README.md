# AETHER VS Code Extension

This extension starts the AETHER LSP server over stdio and enables SIR hover in Rust/TypeScript/JavaScript files.

## What It Is Useful For

- Quick symbol understanding while reading unfamiliar code.
- Seeing continuously updated intent summaries as code changes.
- Switching inference providers from VS Code settings without editing CLI scripts.

## Prerequisites

From repo root, build `aetherd`:

```bash
cargo build -p aetherd
```

## Run in Extension Development Host

1. Open `vscode-extension/` in VS Code.
2. Install/build:

```bash
npm install
npm run build
```

3. Press `F5`.

The extension launches:

```text
aetherd -- --workspace <workspaceRoot> --lsp --index --inference-provider ... --inference-model ... --inference-endpoint ... --inference-api-key-env ...
```

## Configuration Screen

Open VS Code Settings and search for `AETHER`, or use Command Palette:

- `AETHER: Select Inference Provider`

Available settings:
- `aether.inferenceProvider`: `auto | mock | gemini | qwen3_local`
- `aether.inferenceModel`: model name (includes qwen3 embedding presets)
- `aether.inferenceEndpoint`: local endpoint for `qwen3_local` (default `http://127.0.0.1:11434`)
- `aether.geminiApiKeyEnv`: env var name for Gemini key (default `GEMINI_API_KEY`)

## Provider Notes

- `auto`: Gemini when key exists, otherwise Mock.
- `mock`: deterministic local behavior; no key needed.
- `gemini`: set your key in `aether.geminiApiKeyEnv` env var.
- `qwen3_local`: requires a local server at `aether.inferenceEndpoint`; no key required.

## Verify It Works

Open a Rust/TS/JS file and hover a function. After indexing runs, hover should show SIR text such as:

`Mock summary for ...`
