# AETHER VS Code Extension

This extension starts the AETHER LSP server over stdio and adds command/status UX for indexing and symbol search in Rust/TypeScript/JavaScript workspaces.

In a multi-root workspace the extension starts one AETHER language server per folder,
and each answers hovers only for files its folder owns (with nested folders, the
innermost one). Adding or removing a folder starts or stops just that folder's server.
The index and search commands still act on the first folder.

## What It Is Useful For

- Quick symbol understanding while reading unfamiliar code.
- Seeing continuously updated intent summaries as code changes.
- Switching inference providers from VS Code settings without editing CLI scripts.
- Running one-shot indexing and symbol search directly from the command palette.

## Prerequisites

From repo root, build `aetherd`:

```bash
cargo build -p aetherd
```

The extension uses the first `aetherd` it finds:

1. the `aether.aetherdPath` setting, when set (user settings only);
2. `$CARGO_TARGET_DIR/debug/aetherd`, when `CARGO_TARGET_DIR` is set;
3. `<repo>/target/debug/aetherd`;
4. `aetherd` on `PATH`.

If none exists, it runs `cargo build -p aetherd` in the repo root and looks again.

## Run in Extension Development Host

1. Open `vscode-extension/` in VS Code.
2. Install/build:

```bash
npm install
npm run build
npm run smoke
```

3. Press `F5`.

The extension launches:

```text
aetherd -- --workspace <workspaceRoot> --lsp --index --inference-provider ... --inference-model ... --inference-endpoint ... --inference-api-key-env ...
```

## Status Bar

The extension shows a status bar item:

- `AETHER: indexing`: active index task, startup window, or recent `.aether/meta.sqlite` write detected.
- `AETHER: idle`: no active indexing signal.
- `AETHER: idle (stale:N)`: stale hover warnings observed in this VS Code session.
- `AETHER: idle (error)`: the latest extension-triggered action failed (binary build/startup/index/search/open).

Clicking the status bar item runs `AETHER: Search Symbols`.

## Command Palette

- `AETHER: Restart Server`
- `AETHER: Select Inference Provider`
- `AETHER: Index Once`
- `AETHER: Search Symbols`
- `AETHER: Open Symbol Result`

`AETHER: Search Symbols` prompts for a query, runs `aetherd --search ... --output json`, shows a quick pick, and opens the selected file result.

## Configuration Screen

Open VS Code Settings and search for `AETHER`, or use Command Palette:

- `AETHER: Select Inference Provider`

Available settings:
- `aether.inferenceProvider`: `auto | mock | gemini | qwen3_local`
- `aether.inferenceModel`: model name (includes qwen3 embedding presets)
- `aether.inferenceEndpoint`: local endpoint for `qwen3_local` (default `http://127.0.0.1:11434`)
- `aether.geminiApiKeyEnv`: env var name for Gemini key (default `GEMINI_API_KEY`)
- `aether.searchMode`: `lexical | semantic | hybrid` (default `lexical`)
- `aether.aetherdPath`: absolute path of the `aetherd` binary (default empty: search as described under Prerequisites)

## Provider Notes

- `auto`: Gemini when key exists, otherwise Mock.
- `mock`: deterministic local behavior; no key needed.
- `gemini`: set your key in `aether.geminiApiKeyEnv` env var.
- `qwen3_local`: requires a local server at `aether.inferenceEndpoint`; no key required.

## Verify It Works

Open a Rust/TS/JS file and hover a function. After indexing runs, hover should show SIR text such as:

`Mock summary for ...`

Run extension verification scripts:

```bash
npm run build
npm run smoke
```
