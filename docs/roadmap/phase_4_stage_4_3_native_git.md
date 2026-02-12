# Phase 4 - Stage 4.3: Native Git via `gix`

## Purpose
Replace the shell-out to `git rev-parse --verify HEAD` with the pure-Rust `gix` library. The current code uses `std::process::Command` to call git, which is fragile (requires git on PATH, no error typing, subprocess overhead per call). The prospectus specifies `gix (gitoxide)` (§5 Tech Stack) for pure-Rust git operations.

## Current implementation (what we're replacing)
```rust
// crates/aetherd/src/sir_pipeline.rs
fn resolve_head_commit(workspace: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(workspace)
        .output()
        .ok()?;
    // ... parse stdout
}
```

## Target implementation
- `GitContext` struct in `crates/aether-core` that wraps a `gix::Repository`
- Methods: `head_commit() -> Option<String>`, `blame_file(path) -> Vec<BlameLine>`, `log_for_file(path, limit) -> Vec<CommitInfo>`
- Used by `sir_pipeline.rs` for commit hash resolution
- Used by future historian enhancements for richer git context

## In scope
- Add `gix = { version = "0.76", default-features = false, features = ["max-performance-safe"] }` to workspace deps
- Create `GitContext` in `crates/aether-core/src/git.rs`
- Replace `resolve_head_commit()` in `crates/aetherd/src/sir_pipeline.rs`
- Add `blame_file()` and `log_for_file()` for future use by historian queries
- Handle non-git workspaces gracefully (return None, don't crash)

## Out of scope
- Full async blame walker on background threads (Phase 5)
- PR/ticket resolution from commit messages (Phase 5)
- Git hook integration

## Implementation notes

### GitContext API shape
```rust
pub struct GitContext {
    repo: gix::Repository,
}

impl GitContext {
    pub fn open(workspace: &Path) -> Option<Self>;
    pub fn head_commit_hash(&self) -> Option<String>;
    pub fn file_log(&self, path: &Path, limit: usize) -> Vec<CommitInfo>;
    pub fn blame_lines(&self, path: &Path) -> Vec<BlameLine>;
}

pub struct CommitInfo {
    pub hash: String,
    pub author: String,
    pub message: String,
    pub timestamp: i64,
}

pub struct BlameLine {
    pub line_number: u32,
    pub commit_hash: String,
    pub author: String,
}
```

### Non-git workspace handling
- `GitContext::open()` returns `None` if `.git/` doesn't exist or gix can't open it
- All callers already handle `Option<String>` for commit hash — no behavior change

## Pass criteria
1. `resolve_head_commit` uses gix, not `std::process::Command`.
2. `grep -r 'Command::new.*git' crates/` returns zero matches.
3. `head_commit_hash()` returns same SHA as `git rev-parse HEAD` in a test repo.
4. Non-git workspaces return `None` without panic.
5. `blame_lines()` returns correct attribution for a test file.
6. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_4_stage_4_3_native_git.md for full spec.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase4-stage4-3-gix off main.
3) Create worktree ../aether-phase4-stage4-3-gix for that branch and switch into it.
4) Add workspace dependency:
   - gix = { version = "0.76", default-features = false, features = ["max-performance-safe"] }
5) Create crates/aether-core/src/git.rs with GitContext struct.
6) Implement head_commit_hash(), file_log(), blame_lines().
7) Replace std::process::Command("git") in crates/aetherd/src/sir_pipeline.rs with GitContext.
8) Add tests:
   - head_commit_hash matches expected SHA in a temp git repo
   - Non-git workspace returns None
   - blame_lines returns correct line attribution
9) Run:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test --workspace
10) Commit with message: "Replace git shell-out with native gix library".
```

## Expected commit
`Replace git shell-out with native gix library`
