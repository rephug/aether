# AETHER Build Environment

## When to use

Before running ANY cargo command (build, clippy, test, run, bench), ensure the build environment variables are active in your shell. This skill fires automatically when Claude Code runs Rust toolchain commands.

## Required environment

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

If using `.envrc` (direnv), these are sourced automatically when you `cd` into the project. If not, run them manually or `source .envrc` before any cargo command.

## Why these settings matter

- `CARGO_TARGET_DIR=/home/rephu/aether-target` — build artifacts on native Linux FS (ext4), NOT on tmpfs (`/tmp/` — OOM) or 9P (`/mnt/` — 10x slower)
- `CARGO_BUILD_JOBS=2` — prevent OOM on 16GB RAM (12GB WSL2 allocation). AETHER has heavy dependencies (SurrealDB, LanceDB, tree-sitter, gix) that consume ~2GB per parallel rustc instance
- `PROTOC=$(which protoc)` — LanceDB's build script needs protobuf compiler
- `RUSTC_WRAPPER=sccache` — compilation cache, cuts rebuild times by 60%+
- `TMPDIR` — redirects temporary files to the target dir (away from RAM-backed tmpfs)

## System dependencies

These must be installed in WSL2:

```bash
sudo apt-get install -y protobuf-compiler liblzma-dev poppler-utils
```

- `protobuf-compiler` — required by LanceDB build script
- `liblzma-dev` — required by LanceDB transitive dependency
- `poppler-utils` — provides `pdftotext` for document extraction

Linker and cache:

```bash
# mold linker (faster than default ld)
sudo apt-get install -y mold

# sccache
cargo install sccache --locked
```

## Per-crate commands (NEVER use --workspace)

```bash
# Build one crate
cargo build -p aetherd

# Release build
cargo build -p aetherd --release

# With dashboard feature
cargo build -p aetherd --features dashboard --release

# Test one crate
cargo test -p aether-core

# Clippy one crate
cargo clippy -p aether-store -- -D warnings
```

## Phase 9 specific: Tauri builds

Phase 9 introduces `aether-desktop` which depends on Tauri 2.x. Additional deps:

```bash
# Tauri system dependencies (Ubuntu/Debian)
sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev

# Tauri CLI
cargo install tauri-cli --version "^2"
```

Build the desktop app:

```bash
cargo tauri build -p aether-desktop
```

## Disk space management

Build artifacts can grow to 20-30GB. To reclaim:

```bash
rm -rf /home/rephu/aether-target
```

Source code is safe in `.aether/` and the git repo. Only cached build artifacts are deleted.

## SurrealKV lock cleanup

Before running any `aetherd` CLI command, kill existing daemon processes:

```bash
pkill -f aetherd
rm -f .aether/graph/LOCK
```

This avoids "sending into a closed channel" errors from SurrealKV lock contention.
