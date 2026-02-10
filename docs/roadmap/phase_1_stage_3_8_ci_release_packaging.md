# Phase 1 - Stage 3.8: CI and Release Packaging

## Purpose
Ensure contributors and users can rely on consistent CI gates and downloadable release artifacts.

## In scope
- CI workflow maintenance in `.github/workflows/ci.yml`
- Release workflow maintenance in `.github/workflows/release.yml`
- Packaging/docs updates in root `README.md` and related docs

## Out of scope
- Distribution via package managers (Homebrew, winget, apt)
- Long-term support policy changes

## Pass criteria
1. CI workflow enforces `fmt`, `clippy`, and workspace tests.
2. Release workflow builds `aetherd` and `aether-mcp` for linux/macos/windows and packages artifacts.
3. Workflow files validate syntactically and repo tests pass locally.
4. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
You are working in the repo root of https://github.com/rephug/aether.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase1-stage3-8-ci-release off main.
3) Create worktree ../aether-phase1-stage3-8-ci-release for that branch and switch into it.
4) Implement in existing files only:
   - tighten/adjust .github/workflows/ci.yml
   - tighten/adjust .github/workflows/release.yml
   - update README.md release/CI notes if behavior changes
5) Validate and test:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
   - quick grep/sanity check on workflow triggers and job names
6) Commit with message: "Finalize CI and release packaging workflow".
```
