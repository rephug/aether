# Phase 5 - Stage 5.2: Python Language Support

## Purpose
Add Python as AETHER's third supported language with full parity: parsing, symbol extraction, edge extraction, SIR generation, and search (lexical + semantic). Python is the most-requested language and validates that the Stage 5.1 language plugin abstraction works for real.

## Current implementation (what exists)
- Stage 5.1 provides `LanguageConfig` + `LanguageHooks` + `LanguageRegistry`
- Rust and TypeScript are registered via per-language modules
- Parser driver is generic — accepts `&LanguageConfig`
- tree-sitter-python crate exists in the ecosystem (`tree-sitter-python`)

## Target implementation
- `crates/aether-parse/src/languages/python.rs` defines `PythonLanguageConfig` + `PythonHooks`
- tree-sitter-python grammar linked and registered
- Python symbol extraction: functions, classes, methods, module-level variables
- Python edge extraction: function calls, imports (absolute and relative)
- Python-specific qualified name resolution handles `__init__.py` packages
- SIR generation works for Python symbols (same inference pipeline)
- Search returns Python symbols alongside Rust and TypeScript results

## In scope
- Add `tree-sitter-python` to workspace dependencies
- Create `crates/aether-parse/src/languages/python.rs`:
  ```rust
  pub fn config() -> LanguageConfig {
      LanguageConfig {
          id: "python",
          extensions: &["py", "pyi"],
          ts_language: tree_sitter_python::LANGUAGE.into(),
          symbol_query: /* compiled from python_symbols.scm */,
          edge_query: /* compiled from python_edges.scm */,
          module_markers: &["__init__.py", "pyproject.toml", "setup.py", "setup.cfg"],
          hooks: Some(Box::new(PythonHooks)),
      }
  }
  ```
- Create tree-sitter query files:
  - `crates/aether-parse/src/queries/python_symbols.scm`
  - `crates/aether-parse/src/queries/python_edges.scm`
- Implement `PythonHooks` with custom `qualify_name` for Python's package semantics
- Register Python in `default_registry()`
- Add Python test fixtures in `crates/aether-parse/tests/fixtures/`
- Verify end-to-end: index a Python file → symbols in SQLite → SIR generated → searchable

## Out of scope
- Type inference or flow analysis
- Dynamic dispatch resolution (`getattr`, `__getattr__`, `eval`)
- Notebook (`.ipynb`) support
- Virtual environment or installed package analysis
- Python 2 compatibility
- AST-based import resolution beyond static `import` / `from ... import` statements

## Implementation notes

### Python symbols to extract

| tree-sitter node type | AETHER symbol kind | Notes |
|----------------------|-------------------|-------|
| `function_definition` | Function | Top-level and nested functions |
| `class_definition` | Class | |
| `function_definition` inside `class_definition` | Method | Parent = class name |
| `decorated_definition` | (unwrap decorator) | Extract the inner function/class, note decorators in metadata |
| `assignment` at module level | Variable | Only typed annotations or `__all__` assignments |
| `type_alias_statement` | TypeAlias | Python 3.12+ `type X = ...` |

### Symbol query sketch (`python_symbols.scm`)
```scheme
;; Functions (top-level and nested)
(function_definition
  name: (identifier) @name
  parameters: (parameters) @signature
  body: (block) @body) @function

;; Classes
(class_definition
  name: (identifier) @name
  body: (block) @body) @class

;; Methods (functions inside classes)
(class_definition
  name: (identifier) @parent
  body: (block
    (function_definition
      name: (identifier) @name
      parameters: (parameters) @signature
      body: (block) @body) @method))

;; Decorated definitions (unwrap to inner)
(decorated_definition
  (decorator) @decorator
  definition: (_) @inner)
```

### Edge query sketch (`python_edges.scm`)
```scheme
;; Function calls
(call
  function: (identifier) @callee) @call

;; Method calls
(call
  function: (attribute
    object: (_) @object
    attribute: (identifier) @callee)) @call

;; Import statements
(import_statement
  name: (dotted_name) @import_path) @import

;; From-import statements
(import_from_statement
  module_name: (dotted_name) @import_path
  name: (dotted_name) @imported_name) @from_import

;; Relative imports
(import_from_statement
  module_name: (relative_import) @import_path
  name: (dotted_name) @imported_name) @relative_import
```

### Python-specific `qualify_name`
Python uses dotted package paths derived from directory structure:
```
project/
├── mypackage/
│   ├── __init__.py      ← makes mypackage a package
│   ├── core.py          ← module: mypackage.core
│   │   └── def process()  ← mypackage.core::process
│   └── utils/
│       ├── __init__.py  ← makes mypackage.utils a package
│       └── helpers.py   ← module: mypackage.utils.helpers
│           └── class Helper  ← mypackage.utils.helpers::Helper
│               └── def run()  ← mypackage.utils.helpers::Helper::run
```

The `PythonHooks::qualify_name` implementation:
1. Walk up from file to find nearest `__init__.py` or `pyproject.toml`
2. Build dotted module path from directory names
3. Append `{parent}::{name}` for the symbol within the module
4. Files without a package parent use the filename as the module

### Decorator handling
Decorators modify symbol behavior but don't change identity:
- `@property` → symbol kind remains Method, add `is_property: true` metadata
- `@classmethod` → symbol kind remains Method, add `is_classmethod: true` metadata  
- `@staticmethod` → symbol kind remains Method, add `is_staticmethod: true` metadata
- Custom decorators → captured in SIR metadata but don't affect symbol extraction

### `__init__.py` module semantics
- An `__init__.py` file's symbols belong to the package (not `package.__init__`)
- Qualified names for symbols in `__init__.py` use the directory name as the module
- If `__init__.py` re-exports via `from .submodule import X`, that creates a DEPENDS_ON edge

## Edge cases

| Scenario | Behavior |
|----------|----------|
| No `__init__.py` (namespace package) | Treat directory as implicit package, warn at debug level |
| `__init__.py` with `__all__` | Extract `__all__` entries as DEPENDS_ON edges |
| Relative import `from . import X` | Edge target = `{current_package}.X` |
| Star import `from module import *` | Single DEPENDS_ON edge to the module, not individual names |
| Conditional import `if TYPE_CHECKING:` | Extract normally (static analysis, not runtime) |
| Nested function definition | Extract as symbol with parent = enclosing function |
| Lambda expressions | Skip — not named symbols |
| `__init__` method | Extract as Method with name `__init__` |
| `.pyi` stub files | Parse identically to `.py` — stubs define the same symbols |

## Pass criteria
1. Indexing a Python file with functions and classes produces correct symbols with stable BLAKE3 IDs.
2. Indexing a Python file with `import` and `from ... import` produces correct DEPENDS_ON edges.
3. Indexing a Python file with function calls produces correct CALLS edges.
4. Qualified names respect `__init__.py` package boundaries.
5. SIR generation works for Python symbols (mock provider in tests).
6. Lexical and semantic search return Python symbols.
7. Existing Rust and TypeScript tests still pass with identical output.
8. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read these files for context first:
- docs/roadmap/phase_5_stage_5_2_python_support.md (this file)
- crates/aether-parse/src/registry.rs (LanguageConfig, LanguageHooks, LanguageRegistry)
- crates/aether-parse/src/languages/rust.rs (reference implementation)
- crates/aether-parse/src/languages/typescript.rs (reference implementation)
- crates/aether-core/src/types.rs (SymbolRecord, SymbolEdge types)

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase5-stage5-2-python off main.
3) Create worktree ../aether-phase5-stage5-2-python for that branch and switch into it.
4) Add tree-sitter-python to workspace dependencies.
5) Create crates/aether-parse/src/queries/python_symbols.scm:
   - Functions (top-level, nested, methods inside classes)
   - Classes
   - Decorated definitions (unwrap to inner function/class)
   - Module-level typed assignments and type aliases
6) Create crates/aether-parse/src/queries/python_edges.scm:
   - Function calls (identifier and attribute/method calls)
   - Import statements (import X, from X import Y, relative imports)
7) Create crates/aether-parse/src/languages/python.rs:
   - pub fn config() -> LanguageConfig with Python grammar, queries, module_markers
   - PythonHooks implementing qualify_name with __init__.py-aware package path resolution
8) Register Python in the default_registry() in registry.rs.
9) Add test fixtures in crates/aether-parse/tests/fixtures/:
   - python_basic.py (functions, classes, methods, nested functions)
   - python_imports.py (absolute imports, from-imports, relative imports, star imports)
   - python_package/ directory with __init__.py to test package resolution
10) Add tests:
    - Symbol extraction: correct names, kinds, qualified names, stable IDs
    - Edge extraction: CALLS and DEPENDS_ON from calls and imports
    - Package resolution: __init__.py symbols use directory name as module
    - Integration: index → SIR generation (mock) → search returns Python results
11) Verify Rust and TypeScript tests still pass with identical output.
12) Run:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test --workspace
13) Commit with message: "Add Python language support with full parity".
```

## Expected commit
`Add Python language support with full parity`
