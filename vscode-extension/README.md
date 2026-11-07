# AETHER VS Code Extension

This extension starts the AETHER LSP server over stdio and adds command/status UX for indexing and symbol search in Rust/TypeScript/JavaScript/Python workspaces.

In a multi-root workspace the extension starts one AETHER language server per folder,
and each answers hovers only for files its folder owns (with nested folders, the
innermost one). Adding or removing a folder starts or stops just that folder's server.
The index and search commands still act on the first folder.

Hover explanations use the saved file on disk. While a document has unsaved edits,
AETHER asks you to save the file before showing an explanation. Responses are
also discarded if the document changes or closes while the request is running.
An open AETHER hover is dismissed when its active document changes, and a
save-before-hover message is dismissed when that document is saved. This clears
VS Code's cached popup; the next hover reads the current saved explanation.
AETHER does not reopen the popup or move your focus when generation finishes.

Explanations lead with what the code does, its inputs, result, and cautions.
Dependencies, model confidence, and explanation history appear below those sections.
Empty fields mean the model did not record a finding; they do not prove the absence
of side effects or failures. A source fingerprint mismatch warns that the explanation
may be out of date, even before background indexing notices the saved edit. Older
explanations without a source fingerprint disclose that freshness could not be checked.

When an explanation is missing, hover requests it and says it is waiting for the result. That request does not prove that a background worker has started it.
Generation errors, when recorded, appear as a failed last update. Hover reuses saved
explanations; it does not run a new LLM call for each hover.

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

For an existing Oh My Pi subscription login, select `omp` as the inference provider,
set the model to the full route (for example `openai-codex/gpt-6-luna`), and set the
endpoint to the gateway API base (for example `http://127.0.0.1:4000/v1`). Custom
model names are accepted. Supply the gateway credential through the daemon's
`OMP_GATEWAY_TOKEN` or `OMP_GATEWAY_TOKEN_FILE` environment; the legacy setting
`aether.geminiApiKeyEnv` can select a different credential variable. Run
`AETHER: Restart Server` after changing provider settings. VS Code passes these
settings as daemon overrides; they take precedence over `.aether/config.toml`.
Configure embeddings separately in that file if generation should avoid embedding calls.

## Status Bar

The extension shows a status bar item:

- `AETHER: indexing`: active index task, startup window, or recent `.aether/meta.sqlite` write detected.
- `AETHER: idle`: no active indexing signal.
- `AETHER: idle (outdated:N)`: out-of-date hover warnings observed during this VS Code session. This count persists when explanations refresh.
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

Open a saved Rust/TS/JS/Python file and hover a function. After indexing runs, hover should show SIR text such as:

`Mock summary for ...`

Run extension verification scripts:

```bash
npm run build
npm run smoke
npm test
```

`npm run verify` runs all three.

An optional real VS Code check uses an existing daemon binary built from the same
source, a separate editor profile, synthetic Python code, and mock inference only:

```text
node test/run-live-hover.cjs <aetherd-binary> <new-output-directory>
```

The check leaves workspace trust unchanged. An untrusted test workspace stops the
check before the extension starts. After the owner has trusted that synthetic
workspace in its test profile, `--resume` reuses the profile and preserves earlier
result files. This check verifies editor/LSP behavior; mock text cannot establish
the quality of real model-generated explanations.
