# AETHER VS Code Extension

## Build aetherd

```bash
cargo build -p aetherd
```

## Run the extension in Extension Development Host (F5)

1. Open `vscode-extension/` in VS Code.
2. Run:

```bash
npm install
npm run build
```

3. Press `F5` to launch the Extension Development Host.

The extension starts:

```text
aetherd -- --workspace <workspaceRoot> --lsp --index --inference-provider ... --inference-model ... --inference-endpoint ...
```

## Provider selection

Use VS Code Settings (`AETHER`) or Command Palette:

- `AETHER: Select Inference Provider`

Settings:
- `aether.inferenceProvider`: `auto | mock | gemini | qwen3_local`
- `aether.inferenceModel`: `qwen3-embeddings-0.6B | qwen3-embeddings-4B | qwen3-embeddings-8B | gemini-2.0-flash`
- `aether.inferenceEndpoint`: local endpoint for `qwen3_local` (default `http://127.0.0.1:11434`)
- `aether.geminiApiKeyEnv`: env var name for Gemini API key (default `GEMINI_API_KEY`)

Gemini:
- Set your key in the environment variable configured by `aether.geminiApiKeyEnv`.

qwen3_local:
- Requires a local HTTP server running at `aether.inferenceEndpoint`.
- No API key is required.

## Confirm it works

Open a Rust or TypeScript file and hover a function. Once indexing has produced SIR, you should see hover text containing:

`Mock summary for ...`
