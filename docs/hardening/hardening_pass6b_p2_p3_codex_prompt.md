# AETHER Hardening Pass 6b — P2/P3 Performance & Polish Fixes

You are working on the AETHER project at the repository root. This prompt contains
verified P2/P3 fixes from a Gemini deep code review. Apply ALL fixes below, then
run the validation gate.

**CRITICAL: Do NOT change any public API signatures, struct field names, or trait
method signatures. All fixes are internal implementation changes only.**

**Read `docs/hardening/hardening_pass6_session_context.md` before starting.**

**Prerequisite:** Hardening Pass 6a (P0/P1 fixes) MUST be merged to main first.

---

## Preflight

```bash
git status --porcelain
# Must be clean. If not, stop and report dirty files.

git fetch origin
git pull --ff-only origin main
```

## Branch + Worktree

```bash
git worktree add ../aether-hardening-pass6b feature/hardening-pass6b -b feature/hardening-pass6b
cd ../aether-hardening-pass6b
```

## Build Environment

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

---

## Fix B1: Analyzers Bypass SharedState and Read-Only Mode (P2)

**The bug:** Dashboard API handlers instantiate analyzers via
`DriftAnalyzer::new(&workspace)`, `CausalAnalyzer::new(&workspace)`, and
`HealthAnalyzer::new(&workspace)`. These analyzers open fresh `SqliteStore::open()`
(read-write) and `CozoGraphStore::open()` connections on each method call, bypassing
SharedState connection pooling. When `aether-query` is running (read-only mode),
these endpoints crash with `attempt to write a readonly database`.

**The fix:** Wrap the synchronous analyzer calls in `tokio::task::spawn_blocking`
and use `SqliteStore::open_readonly()` instead of `SqliteStore::open()`. The
full refactor (passing shared store references into analyzers) is deferred to
Phase 8.

### File: `crates/aether-analysis/src/drift.rs`

**Step B1a:** Find the three methods that call `SqliteStore::open()`:

1. `report()` (around line 242): `let store = SqliteStore::open(&self.workspace)?;`
2. `timeline()` (around line 407): `let store = SqliteStore::open(&self.workspace)?;`
3. `communities()` (around line 455): `let store = SqliteStore::open(&self.workspace)?;`

Change all three to use `open_readonly`:

```rust
// BEFORE:
let store = SqliteStore::open(&self.workspace)?;

// AFTER:
let store = SqliteStore::open_readonly(&self.workspace)?;
```

**Verify:** Confirm that `SqliteStore::open_readonly` exists. If it doesn't exist,
check if there's an `open_read_only` or a method that takes a `read_only: bool`
parameter. If no readonly variant exists at all, skip this sub-step and report
what you found.

**Step B1b:** Do the same for `HealthAnalyzer` and `CausalAnalyzer` if they also
call `SqliteStore::open()`:

```bash
grep -rn "SqliteStore::open(" crates/aether-analysis/src/
```

Change every `SqliteStore::open(&self.workspace)` to `SqliteStore::open_readonly(&self.workspace)`.

**Step B1c:** In the dashboard API handlers, wrap analyzer calls in
`run_blocking_with_timeout` (which already uses `spawn_blocking`):

### File: `crates/aether-dashboard/src/api/architecture.rs`

Find the `load_architecture_data` function (around line 67). The call to
`DriftAnalyzer::new` and `analyzer.communities()` is synchronous and blocks the
Axum async worker. Wrap it:

```rust
// BEFORE (around line 74-77):
let analyzer = DriftAnalyzer::new(shared.workspace.as_path()).map_err(|e| e.to_string())?;
let result = analyzer
    .communities(CommunitiesRequest { format: None })
    .map_err(|e| e.to_string())?;

// AFTER:
let workspace = shared.workspace.clone();
let result = crate::support::run_blocking_with_timeout(move || {
    let analyzer = DriftAnalyzer::new(workspace.as_path()).map_err(|e| e.to_string())?;
    analyzer
        .communities(CommunitiesRequest { format: None })
        .map_err(|e| e.to_string())
})
.await?;
```

Note: This requires making `load_architecture_data` async. If it's already async,
just wrap the analyzer call. If it's not async, the change is larger — check
whether the Axum handler calling it is async (it should be). If making it async
is not straightforward, just wrap the analyzer call in
`tokio::task::spawn_blocking` inline.

Apply the same pattern to:
- `crates/aether-dashboard/src/api/causal_chain.rs` (around line 120)
- `crates/aether-dashboard/src/api/health.rs` (around line 100)

---

## Fix B2: Unfixed `normalize_rename_path` in drift.rs (P2)

**The bug:** In hardening pass 5a, `normalize_rename_path` was fixed in
`coupling.rs` to handle brace-enclosed git renames (`crates/{old => new}/src/lib.rs`).
However, the **exact same broken function** is duplicated in `drift.rs` and was
missed. Semantic Drift analysis will drop or misattribute symbols on file renames.

### File: `crates/aether-analysis/src/drift.rs`

Find `normalize_rename_path` (around line 1211). Replace the entire function
body with the fixed version from `coupling.rs` (around line 804):

```rust
// BEFORE (around line 1211):
fn normalize_rename_path(path: &str) -> String {
    let value = path.trim();
    if let Some((_, right)) = value.rsplit_once("=>") {
        return right
            .trim()
            .trim_start_matches('{')
            .trim_end_matches('}')
            .trim()
            .to_owned();
    }
    value.to_owned()
}

// AFTER (copy from coupling.rs):
fn normalize_rename_path(path: &str) -> String {
    let value = path.trim();

    // Handle brace-enclosed renames: prefix{old => new}suffix
    // Example: crates/{old => new}/src/lib.rs -> crates/new/src/lib.rs
    if let (Some(brace_start), Some(brace_end)) = (value.find('{'), value.find('}'))
        && brace_start < brace_end
    {
        let prefix = &value[..brace_start];
        let inner = &value[brace_start + 1..brace_end];
        let suffix = &value[brace_end + 1..];

        if let Some((_, new_part)) = inner.split_once("=>") {
            return format!("{}{}{}", prefix, new_part.trim(), suffix);
        }
    }

    // Handle simple renames: old_path => new_path
    if let Some((_, right)) = value.rsplit_once("=>") {
        return right.trim().to_owned();
    }

    value.to_owned()
}
```

### Verification

Add a test in the existing test module of `drift.rs` (or create one if none exists):

```rust
#[test]
fn normalize_rename_path_brace_enclosed() {
    assert_eq!(
        normalize_rename_path("crates/{old => new}/src/lib.rs"),
        "crates/new/src/lib.rs"
    );
}

#[test]
fn normalize_rename_path_simple() {
    assert_eq!(
        normalize_rename_path("old/path.rs => new/path.rs"),
        "new/path.rs"
    );
}
```

---

## Fix B3: SSE Semaphore Permit Drops Early (P2)

**The bug:** In `aether-query/src/server.rs`, the rate-limiting semaphore permit
is stored in `response.extensions_mut()`. When Axum begins streaming an SSE
response, the `Response` object is consumed to serialize headers, which drops the
`Extensions` map (and the permit) while the SSE stream is still generating.

### File: `crates/aether-query/src/server.rs`

Find the permit holding logic (around line 155-162):

```rust
// BEFORE (around line 155-162):
let response = mcp_response.into_response();
// Hold the permit in an Arc that lives as long as the response.
// For SSE, dropping the Arc (and thus the permit) happens when the
// response body is fully consumed or the connection closes.
let permit = Arc::new(permit);
let _hold = permit.clone();
let mut response = response;
response.extensions_mut().insert(permit);
response

// AFTER:
let response = mcp_response.into_response();
// Tie the permit lifetime to the response body, not the extensions.
// Extensions are dropped when headers are serialized for SSE streams.
let (parts, body) = response.into_parts();
let permit = Arc::new(permit);
let held_permit = permit.clone();
let body = axum::body::Body::new(http_body_util::combinators::BoxBody::new(
    body.map_frame(move |frame| {
        let _keep = &held_permit;
        frame
    })
));
Response::from_parts(parts, body)
```

**IMPORTANT:** This fix requires `http_body_util` to be in the dependency tree.
Check if it's already a dependency:

```bash
grep "http-body-util\|http_body_util" Cargo.toml crates/aether-query/Cargo.toml
```

If `http-body-util` is NOT available, use this simpler alternative that wraps
the body in a stream that holds the permit:

```rust
// SIMPLER ALTERNATIVE (if http-body-util is not available):
use futures_util::StreamExt;

let response = mcp_response.into_response();
let (parts, body) = response.into_parts();
let permit = Arc::new(permit);
let held_permit = permit.clone();
let wrapped = axum::body::Body::from_stream(
    http_body_util::BodyStream::new(body).map(move |chunk| {
        let _keep = &held_permit;
        chunk
    })
);
Response::from_parts(parts, wrapped)
```

If neither approach compiles cleanly, **skip this fix** and report what
dependencies are available. This can be revisited when the crate's dependency
tree is updated.

---

## Fix B4: LSP UTF-16 vs UTF-8 Column Mismatch (P2)

**The bug:** LSP defines `Position.character` in **UTF-16 code units**. Tree-sitter
uses **UTF-8 byte offsets**. The code casts `position.character as usize` directly
to a byte column. On lines containing emoji, CJK, or other multi-byte characters,
the hover offset decouples and hover fails.

### File: `crates/aether-lsp/src/lib.rs`

Find where the LSP position is extracted (around line 200-201):

```rust
// BEFORE (around line 200-201):
let cursor_line = (position.line as usize) + 1;
let cursor_column = (position.character as usize) + 1;

// AFTER:
let cursor_line = (position.line as usize) + 1;
let cursor_column = utf16_offset_to_byte_offset(&source, position.line, position.character) + 1;
```

Add this helper function near the end of the file (before the tests module):

```rust
/// Convert an LSP UTF-16 character offset to a UTF-8 byte offset for the given line.
fn utf16_offset_to_byte_offset(source: &str, line: u32, character: u32) -> usize {
    let target_line = line as usize;
    let utf16_offset = character as usize;

    let line_start = source
        .lines()
        .take(target_line)
        .map(|l| l.len() + 1) // +1 for newline
        .sum::<usize>();

    let line_text = source.lines().nth(target_line).unwrap_or("");

    let mut utf16_count = 0usize;
    for (byte_idx, ch) in line_text.char_indices() {
        if utf16_count >= utf16_offset {
            return byte_idx;
        }
        utf16_count += ch.len_utf16();
    }
    // If offset is past the end of the line, return line length
    line_text.len()
}
```

### Verification

Add a test:

```rust
#[test]
fn utf16_to_byte_offset_with_emoji() {
    // "😀hello" — emoji is 4 UTF-8 bytes but 2 UTF-16 code units
    let source = "😀hello";
    // UTF-16 offset 2 (past the emoji) = byte offset 4
    assert_eq!(utf16_offset_to_byte_offset(source, 0, 2), 4);
    // UTF-16 offset 0 = byte offset 0
    assert_eq!(utf16_offset_to_byte_offset(source, 0, 0), 0);
}

#[test]
fn utf16_to_byte_offset_ascii_only() {
    let source = "fn hello()";
    // For pure ASCII, UTF-16 offset == byte offset
    assert_eq!(utf16_offset_to_byte_offset(source, 0, 3), 3);
}
```

---

## Fix B5: Double-Mutex in SqliteVectorStore (P3)

**The bug:** `SqliteVectorStore` wraps `SqliteStore` in `std::sync::Mutex`, but
`SqliteStore` already internally wraps its `rusqlite::Connection` in a `Mutex`.
Every fallback vector operation acquires two mutexes unnecessarily.

### File: `crates/aether-store/src/vector.rs`

**Step B5a:** Change the struct (around line 116):

```rust
// BEFORE:
pub struct SqliteVectorStore {
    store: std::sync::Mutex<SqliteStore>,
}

// AFTER:
pub struct SqliteVectorStore {
    store: SqliteStore,
}
```

**Step B5b:** Update the constructor (around line 120):

```rust
// BEFORE:
impl SqliteVectorStore {
    pub fn new(workspace_root: &Path) -> Result<Self, StoreError> {
        Ok(Self {
            store: std::sync::Mutex::new(SqliteStore::open(workspace_root)?),
        })
    }
}

// AFTER:
impl SqliteVectorStore {
    pub fn new(workspace_root: &Path) -> Result<Self, StoreError> {
        Ok(Self {
            store: SqliteStore::open(workspace_root)?,
        })
    }
}
```

**Step B5c:** Update all `self.store.lock().unwrap()` calls in the `VectorStore`
trait implementation to just `&self.store`:

```bash
grep -n "self.store.lock()" crates/aether-store/src/vector.rs
```

For each occurrence:

```rust
// BEFORE:
let store = self.store.lock().unwrap();
store.some_method(...);

// AFTER:
self.store.some_method(...);
```

If any method requires `&mut self` on `SqliteStore`, this fix won't work because
the `VectorStore` trait uses `&self`. In that case, skip this fix and report which
method requires mutability.

---

## Fix B6: DRY `percent_encode` Across Dashboard Fragments (P3)

**The bug:** The `percent_encode` utility function is duplicated identically
across 6 files in the dashboard fragments.

### Step B6a: Add the canonical implementation

### File: `crates/aether-dashboard/src/support.rs`

Add this function at the end of the file (before any closing braces):

```rust
/// Percent-encode a string for safe use in URL path segments and query parameters.
pub(crate) fn percent_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len() * 2);
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    encoded
}
```

**Verify** that this matches the body of the duplicated functions. If the
implementations differ across files, use the most common version and report
any differences.

### Step B6b: Remove duplicates and use the shared version

In each of these 6 files, delete the local `fn percent_encode` function and
replace calls with `crate::support::percent_encode` (or `support::percent_encode`
depending on module path):

1. `crates/aether-dashboard/src/fragments/anatomy.rs` (around line 340)
2. `crates/aether-dashboard/src/fragments/symbol.rs` (around line 271)
3. `crates/aether-dashboard/src/fragments/changes.rs` (around line 191)
4. `crates/aether-dashboard/src/fragments/flow.rs` (around line 257)
5. `crates/aether-dashboard/src/fragments/glossary.rs` (around line 284)
6. `crates/aether-dashboard/src/fragments/prompts.rs` (around line 444)

For each file:
- Delete the local `fn percent_encode(input: &str) -> String { ... }` function
- Add `use crate::support::percent_encode;` at the top if not already imported
- Verify all call sites still compile

---

## Scope Guard

- Do NOT modify any MCP tool schemas, CLI argument shapes, or public API contracts.
- Do NOT add new crates or new workspace dependencies (except `http-body-util` IF
  needed for Fix B3 AND it's already in the transitive dependency tree).
- Do NOT touch SQLite schema migrations or LanceDB table schemas.
- Do NOT rename any public functions or types.
- If any fix cannot be applied because the code structure differs from what's
  described, report exactly what you found and skip that fix.

---

## Validation Gate

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR

cargo fmt --all --check
cargo clippy --workspace -- -D warnings
cargo test -p aether-core
cargo test -p aether-config
cargo test -p aether-store
cargo test -p aether-parse
cargo test -p aether-sir
cargo test -p aether-infer
cargo test -p aether-lsp
cargo test -p aether-analysis
cargo test -p aether-mcp
cargo test -p aether-query
cargo test -p aetherd
cargo test -p aether-dashboard
cargo test -p aether-document
cargo test -p aether-memory
cargo test -p aether-graph-algo
```

All tests MUST pass. The new `normalize_rename_path_brace_enclosed` test in
Fix B2 and the `utf16_to_byte_offset_with_emoji` test in Fix B4 MUST pass.

If `cargo clippy` warns about unused imports after removing duplicate functions,
remove them.

---

## Commit Message

```
fix: P2/P3 hardening pass 6b — analyzer read-only, rename path, SSE permit, LSP UTF-16, vector mutex, DRY percent_encode

- Switch analyzer SqliteStore::open to open_readonly + spawn_blocking (P2)
- Copy fixed normalize_rename_path from coupling.rs to drift.rs (P2)
- Tie SSE semaphore permit to response body lifetime (P2)
- Convert LSP UTF-16 character offset to UTF-8 byte offset (P2)
- Remove unnecessary outer Mutex from SqliteVectorStore (P3)
- Consolidate 6 duplicate percent_encode functions into support.rs (P3)
```

---

## Post-Fix Commands

```bash
git add -A
git commit -m "fix: P2/P3 hardening pass 6b — analyzer read-only, rename path, SSE permit, LSP UTF-16, vector mutex, DRY percent_encode"
git push origin feature/hardening-pass6b
gh pr create --base main --head feature/hardening-pass6b \
  --title "Hardening pass 6b: P2/P3 polish fixes from Gemini deep review" \
  --body "6 fixes: analyzer read-only mode, drift rename path, SSE permit lifetime, LSP UTF-16 columns, vector store double-mutex, percent_encode dedup."
```

After merge:

```bash
cd /home/rephu/projects/aether
git switch main
git pull --ff-only
git worktree remove ../aether-hardening-pass6b
git branch -d feature/hardening-pass6b
git worktree prune
```
