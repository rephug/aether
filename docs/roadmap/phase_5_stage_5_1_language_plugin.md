# Phase 5 - Stage 5.1: Language Plugin Abstraction

## Purpose
Refactor `aether-parse` so that language-specific behavior (symbol queries, edge queries, module resolution, name qualification) is driven by a modular data structure with optional trait overrides, rather than inline `match language { ... }` branches. This makes adding Python (Stage 5.2) and future languages mechanical instead of surgical.

## Current implementation (what we're refactoring)
- `aether-parse` has Rust and TypeScript tree-sitter grammars linked in
- Symbol extraction uses language-specific `.scm` query files or inline query strings
- Edge extraction (Stage 4.4) walks AST nodes with per-language branching
- Language selection happens via file extension matching
- Qualified name construction has per-language logic scattered across functions
- Module boundary detection is implicit (Cargo.toml, package.json)

## Target implementation
- A `LanguageConfig` data struct defines everything a language needs: extensions, grammar, query files, module markers
- An optional `LanguageHooks` trait allows override of behaviors that can't be captured in data (e.g., Python's `__init__.py` module semantics)
- A `LanguageRegistry` maps file extensions → `LanguageConfig` instances, populated at startup
- The parser driver becomes generic: receives a `LanguageConfig` and calls through it
- Existing Rust and TypeScript logic is moved into per-language modules
- All existing tests pass with identical output — this is a pure refactor

## In scope
- Define `LanguageConfig` struct in `crates/aether-parse/src/registry.rs`:
  ```rust
  pub struct LanguageConfig {
      /// Unique language identifier (e.g., "rust", "typescript")
      pub id: &'static str,
      /// File extensions this language handles (e.g., ["rs"], ["ts", "tsx", "js", "jsx"])
      pub extensions: &'static [&'static str],
      /// Tree-sitter Language grammar
      pub ts_language: tree_sitter::Language,
      /// Compiled tree-sitter query for symbol extraction
      pub symbol_query: tree_sitter::Query,
      /// Compiled tree-sitter query for edge extraction
      pub edge_query: tree_sitter::Query,
      /// Files that mark module boundaries (e.g., ["Cargo.toml"], ["package.json"])
      pub module_markers: &'static [&'static str],
      /// Optional hooks for language-specific behavior
      pub hooks: Option<Box<dyn LanguageHooks>>,
  }
  ```
- Define `LanguageHooks` trait for optional overrides:
  ```rust
  pub trait LanguageHooks: Send + Sync {
      /// Build qualified name from file path + symbol name + optional parent
      /// Default: "{file_stem}::{parent}::{name}" (works for most languages)
      fn qualify_name(&self, file_path: &str, symbol_name: &str, parent: Option<&str>) -> String;

      /// Map a tree-sitter query match to a SymbolRecord
      /// Default: standard capture-name-based mapping (works for most languages)
      fn map_symbol(&self, captures: &QueryCaptures, source: &[u8], file_path: &str) -> Option<SymbolRecord> {
          None // None means "use default mapping"
      }

      /// Map a tree-sitter query match to SymbolEdge(s)
      /// Default: standard capture-name-based mapping
      fn map_edge(&self, captures: &QueryCaptures, source: &[u8], file_path: &str) -> Option<Vec<SymbolEdge>> {
          None // None means "use default mapping"
      }

      /// Determine if a directory is a module root
      /// Default: check for module_markers files
      fn is_module_root(&self, dir_path: &Path) -> Option<bool> {
          None // None means "use default module_markers check"
      }
  }
  ```
- Create `LanguageRegistry` that builds `Vec<LanguageConfig>` at startup
- Create per-language modules:
  ```
  crates/aether-parse/src/languages/
  ├── mod.rs
  ├── rust.rs          // fn rust_config() -> LanguageConfig
  └── typescript.rs    // fn typescript_config() -> LanguageConfig
  ```
- Move existing `.scm` query files (or inline strings) into:
  ```
  crates/aether-parse/src/queries/
  ├── rust_symbols.scm
  ├── rust_edges.scm
  ├── typescript_symbols.scm
  └── typescript_edges.scm
  ```
- Refactor the parser driver in `crates/aether-parse/src/parser.rs` to accept `&LanguageConfig` instead of branching on language enums
- Refactor edge extraction to use `LanguageConfig.edge_query` + default mapping logic
- Implement `RustHooks` and `TypeScriptHooks` where the default mapping is insufficient
- Update `crates/aetherd/src/sir_pipeline.rs` to obtain `LanguageConfig` from the registry

## Out of scope
- Adding Python (that's Stage 5.2)
- Changing symbol ID generation (BLAKE3 scheme is unchanged)
- Changing SIR schema or storage
- Changing any MCP tool interfaces
- Adding new symbol kinds
- Compile-time or config-time language selection (all built-in languages are always available)

## Implementation notes

### Default mapping convention
The default `map_symbol` implementation should work from standardized tree-sitter query capture names:
- `@name` → symbol name
- `@kind` → symbol kind (function, class, struct, etc.)
- `@body` → symbol body text
- `@signature` → signature text for ID hashing
- `@parent` → parent symbol name (for nested symbols)

Languages that follow this capture naming convention don't need `LanguageHooks` overrides. The `.scm` query files are the primary extension point.

### Default `qualify_name`
```
{module_path}::{parent}::{name}
```
Where `module_path` is derived from the file path relative to the nearest module root. This works for Rust (`crate::module::Type::method`) and TypeScript (`module/file::Class.method`). Python will need a custom `qualify_name` (Stage 5.2) to handle `__init__.py` and dotted package paths.

### Registry initialization
```rust
pub fn default_registry() -> LanguageRegistry {
    let mut registry = LanguageRegistry::new();
    registry.register(languages::rust::config());
    registry.register(languages::typescript::config());
    registry
}
```

### Backward compatibility guarantee
After this refactor, the following must be byte-identical to pre-refactor:
- Symbol IDs for all existing test fixtures
- Edge records for all existing test fixtures
- SIR content hash for all existing test fixtures

This is verified by running the existing test suite without modification.

## Edge cases

| Scenario | Behavior |
|----------|----------|
| Unknown file extension | Skip file, log at debug level |
| Multiple languages claim same extension | First registered wins, log warning |
| `.scm` query file has syntax error | Panic at startup (fail-fast, not runtime) |
| Language has no edge query | Empty edge set (edge extraction is optional) |
| Language has no hooks | All default implementations used |

## Pass criteria
1. `LanguageConfig` struct and `LanguageHooks` trait exist in `crates/aether-parse`.
2. `LanguageRegistry` maps extensions to configs and is used by the parser driver.
3. Rust and TypeScript each have their own module in `languages/` and query files in `queries/`.
4. The parser driver accepts `&LanguageConfig` — no `match language` branches remain in `parser.rs`.
5. All existing tests pass with identical symbol IDs, edges, and SIR hashes.
6. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read these files for context first:
- docs/roadmap/phase_5_stage_5_1_language_plugin.md (this file)
- crates/aether-parse/src/lib.rs (current parser module)
- crates/aether-parse/src/parser.rs (current parser driver, if exists)
- crates/aether-core/src/types.rs (SymbolRecord, SymbolEdge types)
- crates/aetherd/src/sir_pipeline.rs (how parsing is invoked)

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase5-stage5-1-language-plugin off main.
3) Create worktree ../aether-phase5-stage5-1-language-plugin for that branch and switch into it.
4) Define LanguageConfig struct and LanguageHooks trait in crates/aether-parse/src/registry.rs.
   - LanguageConfig holds: id, extensions, ts_language, symbol_query, edge_query, module_markers, optional hooks.
   - LanguageHooks has default methods: qualify_name, map_symbol, map_edge, is_module_root.
   - Return Option from hooks — None means "use default behavior".
5) Create LanguageRegistry (Vec<LanguageConfig> with extension lookup).
6) Create crates/aether-parse/src/languages/ directory with:
   - mod.rs (pub mod rust; pub mod typescript;)
   - rust.rs (pub fn config() -> LanguageConfig)
   - typescript.rs (pub fn config() -> LanguageConfig)
7) Extract tree-sitter query strings into .scm files under crates/aether-parse/src/queries/:
   - rust_symbols.scm, rust_edges.scm
   - typescript_symbols.scm, typescript_edges.scm
   Include queries via include_str!() in the config functions.
8) Refactor the parser driver to accept &LanguageConfig instead of branching on language.
   - Symbol extraction uses config.symbol_query + default mapping (or hooks override).
   - Edge extraction uses config.edge_query + default mapping (or hooks override).
9) Update sir_pipeline.rs to get LanguageConfig from LanguageRegistry by file extension.
10) Run ALL existing tests to verify identical output:
    - Symbol IDs must not change
    - Edge records must not change
    - SIR hashes must not change
11) Run:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test --workspace
12) Commit with message: "Refactor parser into language plugin abstraction".
```

## Expected commit
`Refactor parser into language plugin abstraction`
