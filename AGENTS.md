# AETHER Codex Context

## Repo structure
Rust workspace with 9 crates under `crates/`.
VS Code extension under `vscode-extension/`.

## Validation gates (always run these before committing)
- cargo fmt --all --check
- cargo clippy --workspace -- -D warnings  
- cargo test --workspace

## Key patterns
- Store trait in crates/aether-store abstracts all persistence
- InferenceProvider and EmbeddingProvider traits abstract AI backends
- Config loaded from .aether/config.toml via crates/aether-config
- MCP tools are in crates/aether-mcp, handler methods on AetherMcpRouter
- SIR is canonical JSON, sorted keys, BLAKE3 hashed

## Do NOT
- Create new crates without explicit instruction
- Modify VS Code extension unless the stage doc says to
- Use unwrap() in library code (use anyhow/thiserror)
- Add dependencies not listed in the stage doc
