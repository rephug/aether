# Phase 4 - Stage 4.7: LSP Rust Import-Hover for File-Level SIR

## Purpose
Extend the LSP import-hover path (added in 4.6 for TS/JS) to support Rust `use` statements. Hovering a Rust `use` import resolves the module path to a file and shows its file-level SIR summary in the editor.

## Dependency
Requires Stage 4.6 merged to main. Uses `FileSir`, `synthetic_file_sir_id`, stored file-level SIR records, and the import-hover formatting helpers added in 4.6.

## Current implementation (after 4.6)
- LSP hover serves leaf symbol SIR for any hovered symbol
- TS/JS relative import-hover resolves `import ... from "./path"` to file-level SIR
- Rust `use` statements are **not resolved** — hovering them falls through to leaf hover or no result
- File-level SIR exists in the store for indexed Rust files

## Target implementation
- Hovering `use crate::config::loader` shows the `FileSir` for `src/config/loader.rs`
- Hovering `use self::helper` resolves relative to the current module
- Hovering `use super::utils` resolves relative to the parent module
- If resolution fails or no `FileSir` exists, fall through to existing leaf hover (no error)

## In scope

### Rust `use` path resolution
- Support three prefix styles:
  - `crate::` — resolve from crate root (`src/lib.rs` or `src/main.rs` parent directory)
  - `self::` — resolve relative to the current file's module directory
  - `super::` — resolve relative to the parent module directory
- For each path segment, check both `{segment}.rs` and `{segment}/mod.rs`
- When both exist, prefer `mod.rs` (Rust convention)
- Resolve to the deepest segment that maps to an actual file
- Remaining segments after file resolution are assumed to be symbols — fall through to leaf hover

### Hover behavior
- Detect `use_declaration` AST node at cursor position via tree-sitter
- Extract path segments from `scoped_identifier` child
- Run resolution algorithm to find target file
- Compute synthetic file ID via `synthetic_file_sir_id("rust", normalized_path)`
- Read `FileSir` from store
- Format using the same file rollup markdown helper added in 4.6
- If any step fails, fall through silently to existing behavior

## Out of scope
- `#[path = "..."]` attribute overrides (rare, document as known limitation)
- External crate imports (`use serde::Deserialize`) — no cross-crate resolution
- Workspace cross-crate imports (`use other_crate::thing`)
- `pub use` re-export chasing (show SIR for the direct resolved file, not re-export targets)
- Module-level SIR in hover (file-level only, matches 4.6 TS/JS behavior)
- Any changes to MCP tools

## Implementation notes

### Resolution algorithm
```
use crate::config::loader::parse_toml
         ^^^^^^  ^^^^^^  ^^^^^^^^^^
         dir?    file?   symbol (leaf hover)
```

1. Determine prefix and compute base directory:
   - `crate::` → crate root (directory containing `lib.rs` or `main.rs`)
   - `self::` → directory of the current file (or parent if current file is `mod.rs`)
   - `super::` → parent of the `self::` base
2. Walk segments left to right. For each segment:
   - Check `{base}/{segment}.rs` — if exists, file found
   - Check `{base}/{segment}/mod.rs` — if exists, set base = `{base}/{segment}`, continue
   - Neither exists → resolution fails, fall through
3. Once a file is found, any remaining segments are symbols within that file → fall through to leaf hover for those

### Crate root detection
- Walk upward from the current file looking for `Cargo.toml`
- Crate root is the `src/` directory under that `Cargo.toml`'s parent
- If no `Cargo.toml` found, resolution fails silently

### Edge cases

| Scenario | Behavior |
|----------|----------|
| `use crate::config` where `src/config.rs` and `src/config/mod.rs` both exist | Prefer `src/config/mod.rs` |
| `use self::helper` in `src/config/mod.rs` | Resolve to `src/config/helper.rs` |
| `use self::helper` in `src/config.rs` | Resolve to `src/config/helper.rs` (same directory) |
| `use super::utils` in `src/config/loader.rs` | Resolve to `src/utils.rs` or `src/utils/mod.rs` |
| `use super::utils` in `src/config/mod.rs` | Resolve to `src/utils.rs` or `src/utils/mod.rs` |
| Hovering the symbol segment (`parse_toml`) | Fall through to leaf hover |
| `use crate::deeply::nested::module::func` | Resolve directories until a `.rs` file is found |
| File exists but has no `FileSir` | Fall through to leaf hover |
| `use serde::Deserialize` (external crate) | No `crate::`/`self::`/`super::` prefix → skip, fall through |
| `use crate::config` where config is a re-exporting `mod.rs` | Show `FileSir` for `mod.rs` itself, don't chase re-exports |

## Pass criteria
1. Hovering `use crate::config::loader` shows `FileSir` for `src/config/loader.rs`.
2. Hovering `use crate::config` resolves to `src/config/mod.rs` when it exists.
3. Hovering `use self::helper` resolves relative to current module.
4. Hovering `use super::utils` resolves relative to parent module.
5. Hovering a Rust `use` that can't be resolved falls through to leaf hover (no error).
6. Hovering a non-import Rust symbol continues to show leaf SIR (no regression).
7. Existing TS/JS import-hover is unaffected (no regression).
8. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Tests

| # | Location | Scenario | Assertion |
|---|----------|----------|-----------|
| 1 | `crates/aether-lsp` | Hover `use crate::config::loader` → `src/config/loader.rs` has `FileSir` | Returns file-level hover markdown |
| 2 | `crates/aether-lsp` | Hover `use crate::config` → `src/config/mod.rs` has `FileSir` | Returns file-level hover markdown |
| 3 | `crates/aether-lsp` | Hover `use super::utils` from nested module | Resolves to correct parent-relative file |
| 4 | `crates/aether-lsp` | Hover `use crate::nonexistent` | Falls through, no error |
| 5 | `crates/aether-lsp` | Hover `use crate::config::loader` with no `FileSir` for loader.rs | Falls through to leaf hover |
| 6 | `crates/aether-lsp` | Hover leaf symbol (non-import) | Existing leaf SIR hover, no regression |
| 7 | `crates/aether-lsp` | Hover TS/JS import (regression check) | Still works as before |

## Exact Codex prompt(s)
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_4_stage_4_7_lsp_import_hover.md for full spec.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase4-stage4-7-rust-import-hover off main.
3) Create worktree ../aether-phase4-stage4-7-rust-hover for that branch and switch into it.
4) In crates/aether-lsp/src/lib.rs, add Rust use-declaration detection:
   - Detect use_declaration AST node at cursor via tree-sitter.
   - Extract path segments from scoped_identifier.
   - Identify prefix (crate::, self::, super::).
5) Implement Rust module path resolution:
   - crate:: resolves from crate root (find Cargo.toml, use src/ dir).
   - self:: resolves from current file's module directory.
   - super:: resolves from parent module directory.
   - Walk segments checking segment.rs and segment/mod.rs.
   - Prefer mod.rs when both exist.
6) On successful resolution:
   - Compute synthetic file ID via synthetic_file_sir_id("rust", path).
   - Read FileSir from store.
   - Format using existing file rollup markdown helper from 4.6.
7) Ensure graceful fallthrough: if resolution fails or no FileSir exists, fall through to existing leaf hover.
8) Add tests per spec (7 scenarios covering crate::, super::, fallthrough, no regression).
9) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
10) Commit with message: "Add LSP Rust use import-hover for file-level SIR".
```

## Expected commit
`Add LSP Rust use import-hover for file-level SIR`
