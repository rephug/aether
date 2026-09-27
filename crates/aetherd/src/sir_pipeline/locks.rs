//! Coordination shared by every SIR and embedding writer against one workspace: the
//! daemon's indexer and regenerate passes, `aetherd sir inject`, and each `aether-mcp`
//! process serving `aether_sir_inject`. Each lock has an in-process half (a keyed wait
//! set, so two tasks in one process never both hold it) and a cross-process half (an
//! exclusive lock on a file under `.aether/`), acquired in that order and released
//! together when the guard drops.
//!
//! - The inject lock covers reading a leaf's prior state, the confidence guard, the leaf
//!   write and the file-rollup rebuild or persist, so a rollup is never persisted over
//!   leaves that changed after it was computed.
//! - The per-symbol embedding lock covers every write or delete of a symbol's vector,
//!   so an embedding computed for an older SIR can never land on top of a newer one, and
//!   a writer's own take-back never removes another writer's vector.
//!
//! A holder of one lock never acquires another kind while holding it, and batches take
//! their per-symbol locks in sorted order, so the locks cannot deadlock.

use std::collections::BTreeSet;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};

use anyhow::{Context, Result};

const INJECT_LOCK_FILE: &str = "inject.lock";
const EMBED_LOCK_DIR: &str = "embed-locks";

/// Keys held by this process (the in-process half of every lock).
static HELD: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
static HELD_CHANGED: Condvar = Condvar::new();

/// Holds one lock; both halves are released when it drops.
#[derive(Debug)]
pub struct WriteGuard {
    key: String,
    _file: File,
}

impl Drop for WriteGuard {
    fn drop(&mut self) {
        release(&self.key);
    }
}

fn release(key: &str) {
    let mut held = HELD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    held.remove(key);
    HELD_CHANGED.notify_all();
}

fn acquire(key: String, path: PathBuf) -> Result<WriteGuard> {
    {
        let mut held = HELD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        while held.contains(&key) {
            held = HELD_CHANGED
                .wait(held)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        held.insert(key.clone());
    }
    match lock_file(&path) {
        Ok(file) => Ok(WriteGuard { key, _file: file }),
        Err(err) => {
            release(&key);
            Err(err)
        }
    }
}

fn lock_file(path: &Path) -> Result<File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    file.lock()
        .with_context(|| format!("failed to lock {}", path.display()))?;
    Ok(file)
}

/// Exclusive lock over leaf-SIR and file-rollup writes for the workspace
/// (`.aether/inject.lock`).
pub fn acquire_inject_write_lock(workspace: &Path) -> Result<WriteGuard> {
    acquire(
        "inject".to_owned(),
        workspace.join(".aether").join(INJECT_LOCK_FILE),
    )
}

fn embed_lock_path(workspace: &Path, symbol_id: &str) -> PathBuf {
    // Named by the BLAKE3 hash of the symbol id, so arbitrary ids map to safe, bounded
    // file names.
    workspace.join(".aether").join(EMBED_LOCK_DIR).join(format!(
        "{}.lock",
        blake3::hash(symbol_id.as_bytes()).to_hex()
    ))
}

/// Exclusive lock over one symbol's vector (`.aether/embed-locks/<blake3(id)>.lock`).
pub fn acquire_embed_write_lock(workspace: &Path, symbol_id: &str) -> Result<WriteGuard> {
    acquire(
        format!("embed:{symbol_id}"),
        embed_lock_path(workspace, symbol_id),
    )
}

/// Exclusive locks over several symbols' vectors, taken in sorted order (duplicates
/// collapsed) so concurrent batches cannot deadlock against each other.
pub fn acquire_embed_write_locks<'a>(
    workspace: &Path,
    symbol_ids: impl IntoIterator<Item = &'a str>,
) -> Result<Vec<WriteGuard>> {
    let ids: BTreeSet<&str> = symbol_ids.into_iter().collect();
    ids.into_iter()
        .map(|symbol_id| acquire_embed_write_lock(workspace, symbol_id))
        .collect()
}
