//! How far the index has drifted from the workspace, for `aether_status`, `aetherd status`
//! and the freshness line of `aether_sir_context`.
//!
//! `.aether/index_sync.json` records the commit the index reflects and when it caught up
//! with it. A full sync writes it, but only when it read every path. A git-triggered
//! reindex moves it only when it reindexed every change since the commit the marker names.
//! A skipped or failed reindex leaves it alone, so status keeps reporting the gap. The
//! watcher's own `.aether/last_indexed_head`, the base it diffs the next HEAD change
//! against, advances even when a trigger is skipped, so it can't stand in for this
//! marker.
//!
//! Each symbol also records when its file was last read (`last_seen_at`). An indexed file
//! modified after that, an indexed file that is gone, and a source file the index lacks
//! that changed since the last full sync have all changed since the index. A daemon that
//! watches the workspace holds `.aether/watcher.lock` while it runs.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::time::UNIX_EPOCH;

use aether_core::{GitContext, normalize_path};
use aether_parse::language_for_path;
use aether_store::{SirFreshnessCounts, SqliteStore, SymbolRecord};
use anyhow::{Context, Result, anyhow};
use fs2::FileExt;
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};

use crate::observer::{is_ignored_path, relative_workspace_path};

pub const INDEX_SYNC_FILE: &str = "index_sync.json";
pub const WATCHER_LOCK_FILE: &str = "watcher.lock";

/// Most changed paths a freshness report names.
const MAX_CHANGED_FILE_SAMPLES: usize = 10;

/// Files larger than this are never parsed (see the observer), so they are not "new".
const MAX_INDEXED_FILE_SIZE: u64 = 2 * 1024 * 1024;

/// The commit the index reflects, and when it caught up with it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexSyncMarker {
    /// Every change committed up to this commit is indexed. `None` outside git.
    pub head: Option<String>,
    /// When the index caught up with `head` (unix seconds), by a full sync or a
    /// git-triggered reindex.
    pub synced_at: Option<i64>,
    /// When a full sync last read every path of the workspace (unix seconds, taken when
    /// its walk started).
    pub full_sync_at: Option<i64>,
    /// The same moment in milliseconds. A source file the index lacks that changed after
    /// it is new; a file written earlier in the same second is not.
    #[serde(default)]
    pub full_sync_at_ms: Option<i64>,
}

/// The recorded marker; empty when none was written or it can't be read.
pub fn read_index_sync_marker(workspace: &Path) -> IndexSyncMarker {
    fs::read_to_string(workspace.join(".aether").join(INDEX_SYNC_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write_index_sync_marker(workspace: &Path, marker: &IndexSyncMarker) -> Result<()> {
    let aether_dir = workspace.join(".aether");
    fs::create_dir_all(&aether_dir)
        .with_context(|| format!("failed to create {}", aether_dir.display()))?;
    let path = aether_dir.join(INDEX_SYNC_FILE);
    // Written beside the marker and renamed over it, so a reader never sees half of it.
    let staged = aether_dir.join(format!("{INDEX_SYNC_FILE}.tmp"));
    let body = serde_json::to_string_pretty(marker).context("failed to encode the sync marker")?;
    fs::write(&staged, format!("{body}\n"))
        .with_context(|| format!("failed to write {}", staged.display()))?;
    fs::rename(&staged, &path).with_context(|| format!("failed to replace {}", path.display()))
}

/// A full sync read every path of the workspace, as it was when its walk started
/// (`walk_started_at` seconds, `walk_started_at_ms` milliseconds), when HEAD was `head`.
pub(crate) fn record_full_sync(
    workspace: &Path,
    head: Option<&str>,
    walk_started_at: i64,
    walk_started_at_ms: i64,
) -> Result<()> {
    write_index_sync_marker(
        workspace,
        &IndexSyncMarker {
            head: head.map(str::to_owned),
            synced_at: Some(walk_started_at),
            full_sync_at: Some(walk_started_at),
            full_sync_at_ms: Some(walk_started_at_ms),
        },
    )
}

/// A git-triggered reindex that started at `started_at` indexed every change between the
/// marker's commit and `head`. Files outside git history keep the last full sync's time.
pub(crate) fn record_head_caught_up(workspace: &Path, head: &str, started_at: i64) -> Result<()> {
    let previous = read_index_sync_marker(workspace);
    write_index_sync_marker(
        workspace,
        &IndexSyncMarker {
            head: Some(head.to_owned()),
            synced_at: Some(started_at),
            full_sync_at: previous.full_sync_at,
            full_sync_at_ms: previous.full_sync_at_ms,
        },
    )
}

/// The commit HEAD points at, if the workspace is in a git repository.
pub fn workspace_head(workspace: &Path) -> Option<String> {
    GitContext::open(workspace).and_then(|context| context.head_commit_hash())
}

/// Held by a daemon for as long as it watches the workspace.
#[derive(Debug)]
pub struct WatcherLock {
    _file: File,
}

impl WatcherLock {
    /// `None` when another process already holds the lock. Any other failure to lock (a
    /// filesystem without advisory locks, say) is an error, not a running daemon.
    pub fn acquire(workspace: &Path) -> Result<Option<Self>> {
        let aether_dir = workspace.join(".aether");
        fs::create_dir_all(&aether_dir)
            .with_context(|| format!("failed to create {}", aether_dir.display()))?;
        let path = aether_dir.join(WATCHER_LOCK_FILE);
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => {}
            Err(err) if lock_contended(&err) => return Ok(None),
            Err(err) => {
                return Err(err).with_context(|| format!("failed to lock {}", path.display()));
            }
        }
        file.set_len(0)
            .and_then(|()| writeln!(file, "{}", std::process::id()))
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(Some(Self { _file: file }))
    }
}

/// The lock a daemon must hold to watch `workspace`. A second watcher is refused rather
/// than run unlocked: once the first exits, status would read the free lock as "nothing
/// watching" while the second still indexes.
pub fn take_watcher_lock(workspace: &Path) -> Result<WatcherLock> {
    WatcherLock::acquire(workspace)
        .context("failed to take the watcher lock")?
        .ok_or_else(|| {
            let holder = fs::read_to_string(workspace.join(".aether").join(WATCHER_LOCK_FILE))
                .ok()
                .and_then(|raw| raw.trim().parse::<u32>().ok())
                .map(|pid| format!(" (pid {pid})"))
                .unwrap_or_default();
            anyhow!(
                "another aetherd{holder} is already watching {}; stop it before starting another",
                workspace.display()
            )
        })
}

/// Whether a lock attempt failed because another holder has the lock, rather than
/// because locking itself failed.
fn lock_contended(err: &std::io::Error) -> bool {
    let contended = fs2::lock_contended_error();
    err.kind() == ErrorKind::WouldBlock
        || (err.raw_os_error().is_some() && err.raw_os_error() == contended.raw_os_error())
}

/// Whether a daemon holds the watcher lock. A probe takes a shared lock for an instant,
/// so it never blocks a daemon that holds or wants the exclusive one for long. A probe
/// that can't lock at all says nothing about a daemon; it is logged and read as "not
/// watching", which errs toward telling the user to resync.
pub fn watcher_running(workspace: &Path) -> bool {
    let path = workspace.join(".aether").join(WATCHER_LOCK_FILE);
    let Ok(file) = File::open(&path) else {
        return false;
    };
    match FileExt::try_lock_shared(&file) {
        Ok(()) => {
            let _ = FileExt::unlock(&file);
            false
        }
        Err(err) if lock_contended(&err) => true,
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "could not probe the watcher lock"
            );
            false
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshnessVerdict {
    /// Nothing changed since the index, as far as file times and HEAD tell: symbols and
    /// edges are current. A SIR without a source hash may still be out of date, so the
    /// summary then says the structural index is fresh and SIR staleness unverifiable.
    Fresh,
    /// Files or HEAD changed since the index.
    Stale,
    /// No symbols are indexed.
    Empty,
}

impl FreshnessVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Empty => "empty",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexFreshness {
    pub verdict: FreshnessVerdict,
    /// One line saying how fresh the index is.
    pub summary: String,
    /// What makes it stale or uncertain, one sentence each.
    pub notes: Vec<String>,
    /// When the index last caught up with `indexed_head` (see [`IndexSyncMarker`]).
    pub last_sync_at: Option<i64>,
    /// When a full sync last read every path of the workspace.
    pub last_full_sync_at: Option<i64>,
    /// The latest time any symbol was written.
    pub last_symbol_update_at: Option<i64>,
    pub last_sir_update_at: Option<i64>,
    pub indexed_head: Option<String>,
    pub workspace_head: Option<String>,
    /// `None` when either HEAD is unknown.
    pub head_matches: Option<bool>,
    pub indexed_files: u64,
    /// Indexed files modified after they were last read, by modification time: a file
    /// touched or checked out again without a content change counts too.
    pub modified_files: u64,
    /// Indexed files that no longer exist, symbol-free ones the index read included.
    pub deleted_files: u64,
    /// Source files without indexed symbols that appeared or changed since the last full
    /// sync started and that no reindex has read since.
    pub new_files: u64,
    /// Indexed files, and paths of the workspace walk, whose metadata couldn't be read
    /// (permissions, a filesystem error): they may have changed, so the index can't be
    /// called fresh while they are.
    pub unreadable_files: u64,
    /// Up to ten changed paths, each prefixed `M `, `D `, `A ` or `? ` (unreadable).
    pub changed_file_samples: Vec<String>,
    /// SIR leaves of modified or deleted files: they may describe code that is gone.
    pub sirs_in_changed_files: u64,
    pub sir: SirFreshnessCounts,
    pub watcher_running: bool,
}

pub fn assess_index_freshness(workspace: &Path, store: &SqliteStore) -> Result<IndexFreshness> {
    let now = crate::time::current_unix_timestamp_secs();
    let marker = read_index_sync_marker(workspace);
    let workspace_head = workspace_head(workspace);
    let watcher_running = watcher_running(workspace);
    let indexed = store
        .list_indexed_files()
        .context("failed to list indexed files")?;
    let sir = store
        .sir_freshness_counts()
        .context("failed to count SIR freshness")?;
    let last_symbol_update_at = indexed.iter().map(|file| file.last_indexed_at).max();

    let mut modified = Vec::new();
    let mut deleted = Vec::new();
    let mut unreadable = Vec::new();
    let mut sirs_in_changed_files = 0u64;
    for file in &indexed {
        match file_time(&workspace.join(&file.file_path)) {
            FileTime::Modified(mtime_ms)
                if modified_since_read(mtime_ms, file.read_at_ms, file.last_indexed_at) =>
            {
                modified.push(file.file_path.clone());
                sirs_in_changed_files += file.symbols_with_sir;
            }
            FileTime::Modified(_) => {}
            FileTime::Unreadable => unreadable.push(file.file_path.clone()),
            FileTime::Missing => {
                deleted.push(file.file_path.clone());
                sirs_in_changed_files += file.symbols_with_sir;
            }
        }
    }

    // A source file with no symbols may simply declare none (an empty `__init__.py`),
    // so only one that changed since the last full sync started counts as new. Only a full
    // sync walks every path; a git-triggered reindex reads the files of the commits.
    // Without a marker, the newest symbol write stands in, to the end of its second.
    let reference_ms = marker
        .full_sync_at_ms
        .or_else(|| marker.full_sync_at.map(|secs| secs.saturating_mul(1000)))
        .or_else(|| last_symbol_update_at.map(|secs| secs.saturating_mul(1000) + 999));
    // A file the watcher read after its last write is not new either, symbols or not.
    let new_files = match reference_ms {
        Some(reference) if !indexed.is_empty() => {
            let indexed_paths = indexed
                .iter()
                .map(|file| file.file_path.as_str())
                .collect::<std::collections::HashSet<_>>();
            let reads = store
                .list_file_reads()
                .context("failed to list file read times")?;
            let walk = source_files(workspace);
            unreadable.extend(walk.unreadable);
            walk.files
                .into_iter()
                .filter(|(path, mtime)| {
                    written_after(*mtime, reference)
                        && !indexed_paths.contains(path.as_str())
                        && reads
                            .get(path)
                            .is_none_or(|read_at_ms| written_after(*mtime, *read_at_ms))
                })
                .map(|(path, _)| path)
                .collect::<Vec<_>>()
        }
        _ => Vec::new(),
    };
    unreadable.sort();
    unreadable.dedup();

    // A file the index read that is gone, with no symbols for the loop above to find, can
    // still have edges and test intents stored until a reindex or full sync clears them.
    let indexed_paths = indexed
        .iter()
        .map(|file| file.file_path.as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut gone_without_symbols = store
        .list_file_reads()
        .context("failed to list file read times")?
        .into_keys()
        .filter(|path| {
            !indexed_paths.contains(path.as_str())
                && matches!(file_time(&workspace.join(path)), FileTime::Missing)
        })
        .collect::<Vec<_>>();
    gone_without_symbols.sort();
    for path in gone_without_symbols {
        deleted.push(path);
    }

    let head_matches = match (marker.head.as_deref(), workspace_head.as_deref()) {
        (Some(indexed), Some(current)) => Some(indexed.eq_ignore_ascii_case(current)),
        _ => None,
    };
    let changed = modified.len() + deleted.len() + new_files.len() + unreadable.len();
    let verdict = if indexed.is_empty() {
        FreshnessVerdict::Empty
    } else if changed > 0 || head_matches == Some(false) {
        FreshnessVerdict::Stale
    } else {
        FreshnessVerdict::Fresh
    };

    let changed_file_samples = modified
        .iter()
        .map(|path| format!("M {path}"))
        .chain(deleted.iter().map(|path| format!("D {path}")))
        .chain(new_files.iter().map(|path| format!("A {path}")))
        .chain(unreadable.iter().map(|path| format!("? {path}")))
        .take(MAX_CHANGED_FILE_SAMPLES)
        .collect::<Vec<_>>();

    let mut freshness = IndexFreshness {
        verdict,
        summary: String::new(),
        notes: Vec::new(),
        last_sync_at: marker.synced_at,
        last_full_sync_at: marker.full_sync_at,
        last_symbol_update_at,
        last_sir_update_at: sir.last_sir_update_at,
        indexed_head: marker.head,
        workspace_head,
        head_matches,
        indexed_files: indexed.len() as u64,
        modified_files: modified.len() as u64,
        deleted_files: deleted.len() as u64,
        new_files: new_files.len() as u64,
        unreadable_files: unreadable.len() as u64,
        changed_file_samples,
        sirs_in_changed_files,
        sir,
        watcher_running,
    };
    freshness.summary = summary_line(&freshness, now);
    freshness.notes = notes(&freshness);
    Ok(freshness)
}

/// Whether `file_path` was modified after the index last read it, by the rule status uses
/// for `modified_files` ([`modified_since_read`]). A source file without symbols has only
/// its `file_reads` time to go by; one the index has never read has changed.
pub(crate) fn file_changed_since_read(
    workspace: &Path,
    store: &SqliteStore,
    file_path: &str,
) -> Result<bool> {
    let last_seen_at = store
        .file_last_read_at(file_path)
        .with_context(|| format!("failed to read when {file_path} was last indexed"))?;
    let read_at_ms = store
        .file_read_at_ms(file_path)
        .with_context(|| format!("failed to read when {file_path} was last read"))?;
    let FileTime::Modified(mtime_ms) = file_time(&workspace.join(file_path)) else {
        return Ok(false);
    };
    Ok(match (last_seen_at, read_at_ms) {
        (Some(last_seen_at), read_at_ms) => modified_since_read(mtime_ms, read_at_ms, last_seen_at),
        (None, Some(read_at_ms)) => written_after(mtime_ms, read_at_ms),
        (None, None) => true,
    })
}

enum FileTime {
    /// Last modified at these unix milliseconds.
    Modified(i64),
    Missing,
    /// It exists, but its modification time can't be read.
    Unreadable,
}

fn file_time(path: &Path) -> FileTime {
    match fs::metadata(path) {
        Ok(metadata) => {
            metadata_mtime_millis(&metadata).map_or(FileTime::Unreadable, FileTime::Modified)
        }
        Err(err) if err.kind() == ErrorKind::NotFound => FileTime::Missing,
        Err(_) => FileTime::Unreadable,
    }
}

/// How much earlier than the moment of a write its modification time can read. Linux
/// stamps writes from a coarse clock that runs up to a tick (1 ms at HZ=1000, 10 ms at
/// HZ=100) behind the fine clock the indexer takes its read times from: on a 6.8 kernel
/// at HZ=1000, 1,999 of 2,000 writes read up to 1.3 ms earlier than a clock reading taken
/// before them. The margin is twice the slowest common tick.
const FILE_CLOCK_MARGIN_MS: i64 = 20;

/// Whether a write whose modification time reads `mtime_ms` may have come after `at_ms`,
/// a moment read from the system clock: an equal time, or one up to the clock margin
/// earlier, may still be a later write.
fn written_after(mtime_ms: i64, at_ms: i64) -> bool {
    mtime_ms >= at_ms.saturating_sub(FILE_CLOCK_MARGIN_MS)
}

/// Waits long enough after writing files that a read starting now is unambiguously later
/// than those writes, as the watcher's debounce (300 ms by default) guarantees in use.
/// Tests that write a file and expect the next read to see it unchanged call this first.
#[cfg(test)]
pub(crate) fn let_writes_settle() {
    std::thread::sleep(std::time::Duration::from_millis(
        (FILE_CLOCK_MARGIN_MS * 2 + 10) as u64,
    ));
}

/// Whether a file last modified at `mtime_ms` changed after the indexer last read it.
/// With its recorded read start (`read_at_ms`: the walk start for a full sync, the moment
/// before the read for a reindex), any write that may be later ([`written_after`]) is a
/// change, including one made while a long walk was still running. A file read only by a
/// binary that didn't record that time has `last_seen_at`, whole seconds written after the
/// read, so a write in that same second may have come after the read and counts as a
/// change too.
fn modified_since_read(mtime_ms: i64, read_at_ms: Option<i64>, last_seen_at: i64) -> bool {
    match read_at_ms {
        Some(read_at_ms) => written_after(mtime_ms, read_at_ms),
        None => mtime_ms.div_euclid(1000) >= last_seen_at,
    }
}

fn metadata_mtime_millis(metadata: &fs::Metadata) -> Option<i64> {
    let modified = metadata.modified().ok()?;
    let millis = modified.duration_since(UNIX_EPOCH).ok()?.as_millis();
    i64::try_from(millis).ok()
}

/// What a walk of the workspace found.
struct SourceWalk {
    /// Every file the indexer would read, as a workspace-relative path with its
    /// modification time in milliseconds.
    files: Vec<(String, i64)>,
    /// Paths the walk, or a file's metadata, couldn't be read for; `.` when the error
    /// names no path.
    unreadable: Vec<String>,
}

fn source_files(workspace: &Path) -> SourceWalk {
    let relative =
        |path: &Path| normalize_path(&relative_workspace_path(workspace, path).to_string_lossy());
    let mut walk = SourceWalk {
        files: Vec::new(),
        unreadable: Vec::new(),
    };
    for entry in WalkBuilder::new(workspace).standard_filters(true).build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                walk.unreadable.push(
                    walk_error_path(&err)
                        .map(relative)
                        .unwrap_or_else(|| ".".to_owned()),
                );
                continue;
            }
        };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        if is_ignored_path(path) || language_for_path(path).is_none() {
            continue;
        }
        let Some(metadata) = entry.metadata().ok() else {
            walk.unreadable.push(relative(path));
            continue;
        };
        if metadata.len() > MAX_INDEXED_FILE_SIZE {
            continue;
        }
        let Some(mtime) = metadata_mtime_millis(&metadata) else {
            walk.unreadable.push(relative(path));
            continue;
        };
        walk.files.push((relative(path), mtime));
    }
    walk
}

/// The path a walk error is about, if it names one.
fn walk_error_path(err: &ignore::Error) -> Option<&Path> {
    match err {
        ignore::Error::WithPath { path, .. } => Some(path),
        ignore::Error::WithDepth { err, .. } | ignore::Error::WithLineNumber { err, .. } => {
            walk_error_path(err)
        }
        ignore::Error::Partial(errs) => errs.iter().find_map(walk_error_path),
        _ => None,
    }
}

fn summary_line(freshness: &IndexFreshness, now: i64) -> String {
    let synced = freshness
        .last_sync_at
        .or(freshness.last_symbol_update_at)
        .map(|at| format!("last synced {}", describe_time(at, now)))
        .unwrap_or_else(|| "never synced".to_owned());
    let head = match (
        freshness.indexed_head.as_deref(),
        freshness.workspace_head.as_deref(),
    ) {
        (Some(indexed), Some(_)) if freshness.head_matches == Some(true) => {
            format!("at HEAD {} (workspace HEAD matches)", short_sha(indexed))
        }
        (Some(indexed), Some(current)) => format!(
            "at HEAD {}; workspace HEAD is {}",
            short_sha(indexed),
            short_sha(current)
        ),
        (Some(indexed), None) => format!("at HEAD {}", short_sha(indexed)),
        (None, Some(current)) => format!(
            "at an unrecorded HEAD; workspace HEAD is {}",
            short_sha(current)
        ),
        (None, None) => "outside git".to_owned(),
    };
    match freshness.verdict {
        FreshnessVerdict::Empty => "Index empty: no symbols are indexed.".to_owned(),
        FreshnessVerdict::Fresh if freshness.sir.leaves_without_source_hash > 0 => format!(
            "Structural index fresh: {synced} {head}; no source file changed since. SIR \
             staleness unverifiable for {} SIRs with no source hash.",
            freshness.sir.leaves_without_source_hash
        ),
        FreshnessVerdict::Fresh => {
            format!("Index fresh: {synced} {head}; no source file changed since.")
        }
        FreshnessVerdict::Stale => format!(
            "Index STALE: {synced} {head}; since then {} modified, {} deleted, {} new source files{}.",
            freshness.modified_files,
            freshness.deleted_files,
            freshness.new_files,
            match freshness.unreadable_files {
                0 => String::new(),
                count => format!(", and {count} paths that could not be checked"),
            }
        ),
    }
}

fn notes(freshness: &IndexFreshness) -> Vec<String> {
    let mut notes = Vec::new();
    if freshness.verdict == FreshnessVerdict::Empty {
        notes.push("Run `aetherd --workspace . --index-once` to build the index.".to_owned());
        return notes;
    }
    if !freshness.changed_file_samples.is_empty() {
        let changed = freshness.modified_files
            + freshness.deleted_files
            + freshness.new_files
            + freshness.unreadable_files;
        let more = changed.saturating_sub(freshness.changed_file_samples.len() as u64);
        notes.push(format!(
            "Changed since the index (M modified, D deleted, A new, ? unreadable): {}{}.",
            freshness.changed_file_samples.join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            }
        ));
    }
    if freshness.unreadable_files > 0 {
        notes.push(format!(
            "{} paths could not be checked (permissions or a filesystem error), so they may \
             have changed; the index can't be called fresh until they can be read.",
            freshness.unreadable_files
        ));
    }
    if freshness.sirs_in_changed_files > 0 {
        notes.push(format!(
            "{} SIRs belong to files that changed or disappeared since they were indexed; \
             they may describe code that is no longer there.",
            freshness.sirs_in_changed_files
        ));
    }
    if freshness.indexed_head.is_none() && freshness.workspace_head.is_some() {
        notes.push(
            "The commit this index reflects is not recorded; the next full sync that reads \
             every path records it."
                .to_owned(),
        );
    }
    if freshness.head_matches == Some(false) {
        notes.push(
            "HEAD moved since the index caught up with it, and no reindex has covered the \
             change: a daemon may still be on it, or skipped it because its [watcher] trigger \
             is off, or the reindex failed."
                .to_owned(),
        );
    }
    if freshness.sir.stale_leaves > 0 {
        notes.push(format!(
            "{} SIRs are marked stale: their symbol changed after they were written.",
            freshness.sir.stale_leaves
        ));
    }
    if freshness.sir.leaves_without_source_hash > 0 {
        notes.push(format!(
            "{} of {} SIRs have no recorded source hash, so an edit made while no daemon \
             watched can't mark them stale; regenerating a SIR records its hash.",
            freshness.sir.leaves_without_source_hash, freshness.sir.leaf_rows
        ));
    }
    if freshness.watcher_running {
        if freshness.verdict == FreshnessVerdict::Stale {
            notes.push(
                "A daemon is watching this workspace; recent changes may still be queued."
                    .to_owned(),
            );
        }
    } else if freshness.verdict == FreshnessVerdict::Stale {
        notes.push(
            "No aetherd daemon is watching this workspace. Run `aetherd --workspace . \
             --index-once` to resync before trusting symbols, edges or SIRs of changed files."
                .to_owned(),
        );
    } else {
        notes.push(
            "No aetherd daemon is watching this workspace: edits from now on won't reach the \
             index until the next sync."
                .to_owned(),
        );
    }
    notes
}

/// One line on how far `record` and its SIR can be trusted, for `aether_sir_context`.
pub fn symbol_freshness_note(
    workspace: &Path,
    store: &SqliteStore,
    record: &SymbolRecord,
) -> String {
    use aether_store::SirStateStore;

    let now = crate::time::current_unix_timestamp_secs();
    let marker = read_index_sync_marker(workspace);
    let watching = watcher_running(workspace);
    let indexed = describe_time(record.last_seen_at, now);
    // Without a readable read time, the conservative whole-second comparison applies.
    let read_at_ms = store.file_read_at_ms(&record.file_path).unwrap_or_default();
    let file = match file_time(&workspace.join(&record.file_path)) {
        FileTime::Missing => format!("file missing from the workspace (indexed {indexed})"),
        FileTime::Modified(mtime_ms)
            if modified_since_read(mtime_ms, read_at_ms, record.last_seen_at) =>
        {
            format!("file modified after it was indexed ({indexed})")
        }
        FileTime::Modified(_) => format!("file unchanged since indexed {indexed}"),
        FileTime::Unreadable => format!("file indexed {indexed}, modification time unreadable"),
    };

    let sir = match (
        store.get_sir_meta(record.id.as_str()),
        store.get_sir_source_hash(record.id.as_str()),
    ) {
        (Ok(None), _) => "no SIR".to_owned(),
        (Ok(Some(meta)), Ok(source_hash)) => {
            let status = meta.sir_status.trim().to_ascii_lowercase();
            let status = if status.is_empty() {
                "unknown".to_owned()
            } else {
                status
            };
            if source_hash
                .as_deref()
                .is_none_or(|hash| hash.trim().is_empty())
            {
                format!(
                    "SIR {status} (no source hash, so edits made while no daemon watched go undetected)"
                )
            } else {
                format!("SIR {status}")
            }
        }
        (Err(err), _) | (_, Err(err)) => format!("SIR state unreadable: {err}"),
    };

    let index = match (marker.head.as_deref(), workspace_head(workspace).as_deref()) {
        (Some(indexed), Some(current)) if indexed.eq_ignore_ascii_case(current) => {
            format!("index at workspace HEAD {}", short_sha(current))
        }
        (Some(indexed), Some(current)) => format!(
            "index at HEAD {}, workspace at {}",
            short_sha(indexed),
            short_sha(current)
        ),
        (None, Some(current)) => format!(
            "indexed HEAD unrecorded, workspace at {}",
            short_sha(current)
        ),
        (Some(indexed), None) => format!("index at HEAD {}", short_sha(indexed)),
        (None, None) => "no git HEAD".to_owned(),
    };
    let daemon = if watching {
        "daemon watching"
    } else {
        "no daemon watching"
    };
    format!("Freshness: {file}; {sir}; {index}; {daemon}.")
}

fn short_sha(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// `2026-03-19 22:52 UTC (7 months ago)`.
pub fn describe_time(at: i64, now: i64) -> String {
    let date = chrono::DateTime::<chrono::Utc>::from_timestamp(at, 0)
        .map(|time| time.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| format!("unix {at}"));
    format!("{date} ({})", describe_age(now.saturating_sub(at)))
}

fn describe_age(seconds: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    let plural = |count: i64, unit: &str| {
        if count == 1 {
            format!("1 {unit} ago")
        } else {
            format!("{count} {unit}s ago")
        }
    };
    match seconds {
        seconds if seconds < 2 * MINUTE => "just now".to_owned(),
        seconds if seconds < 2 * HOUR => plural(seconds / MINUTE, "minute"),
        seconds if seconds < 2 * DAY => plural(seconds / HOUR, "hour"),
        seconds if seconds < 60 * DAY => plural(seconds / DAY, "day"),
        seconds => plural(seconds / (30 * DAY), "month"),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use std::time::{Duration, SystemTime};

    use aether_config::InferenceProviderKind;
    use aether_store::{SqliteStore, SymbolCatalogStore};
    use tempfile::tempdir;

    use super::{
        FreshnessVerdict, WatcherLock, assess_index_freshness, describe_age, lock_contended,
        read_index_sync_marker, summary_line, symbol_freshness_note, take_watcher_lock,
        watcher_running, workspace_head,
    };
    use crate::indexer::{IndexerConfig, run_initial_index_once};

    fn write_workspace(workspace: &Path) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            "[inference]\nprovider = \"mock\"\n\n[storage]\ngraph_backend = \"sqlite\"\n\n[embeddings]\nenabled = false\nvector_backend = \"sqlite\"\n",
        )
        .expect("write config");
        fs::create_dir_all(workspace.join("pkg")).expect("create pkg");
        fs::write(
            workspace.join("pkg/a.py"),
            "def alpha(x):\n    return x + 1\n",
        )
        .expect("write a");
        fs::write(
            workspace.join("pkg/b.py"),
            "def beta(x):\n    return x - 1\n",
        )
        .expect("write b");
        // No symbols: never in the store, and not "new" either.
        fs::write(workspace.join("pkg/__init__.py"), "").expect("write init");
    }

    fn index_once(workspace: &Path) {
        super::let_writes_settle();
        run_initial_index_once(&IndexerConfig {
            workspace: workspace.to_path_buf(),
            debounce_ms: 300,
            print_events: false,
            print_sir: false,
            embeddings_only: false,
            force: false,
            full: false,
            deep: false,
            turbo_concurrency: None,
            dry_run: false,
            sir_concurrency: 1,
            lifecycle_logs: false,
            inference_provider: Some(InferenceProviderKind::Mock),
            inference_model: None,
            inference_endpoint: None,
            inference_api_key_env: None,
            pause_flag: None,
        })
        .expect("index once");
    }

    /// Writes `contents` with a modification time two minutes ahead, so it reads as
    /// changed after an index taken in the same second.
    fn write_later(path: &Path, contents: &str) {
        fs::write(path, contents).expect("write file");
        fs::File::options()
            .write(true)
            .open(path)
            .and_then(|file| file.set_modified(SystemTime::now() + Duration::from_secs(120)))
            .expect("set modified time");
    }

    fn run_git(workspace: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(workspace)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    #[test]
    fn freshness_reports_files_changed_since_the_index() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_workspace(workspace);
        index_once(workspace);

        let marker = read_index_sync_marker(workspace);
        assert!(marker.synced_at.is_some(), "a sync records its time");
        let store = SqliteStore::open(workspace).expect("open store");
        let fresh = assess_index_freshness(workspace, &store).expect("assess");
        assert_eq!(fresh.verdict, FreshnessVerdict::Fresh, "{fresh:?}");
        assert_eq!(fresh.indexed_files, 2);
        assert_eq!(
            (fresh.modified_files, fresh.deleted_files, fresh.new_files),
            (0, 0, 0)
        );
        assert!(
            fresh.summary.starts_with("Index fresh: last synced "),
            "{}",
            fresh.summary
        );
        assert!(!fresh.watcher_running);

        write_later(
            &workspace.join("pkg/a.py"),
            "def alpha(x):\n    return x + 2\n",
        );
        fs::remove_file(workspace.join("pkg/b.py")).expect("remove b");
        write_later(&workspace.join("pkg/c.py"), "def gamma(x):\n    return x\n");

        let stale = assess_index_freshness(workspace, &store).expect("assess");
        assert_eq!(stale.verdict, FreshnessVerdict::Stale, "{stale:?}");
        assert_eq!(
            (stale.modified_files, stale.deleted_files, stale.new_files),
            (1, 1, 1)
        );
        assert_eq!(
            stale.changed_file_samples,
            ["M pkg/a.py", "D pkg/b.py", "A pkg/c.py"]
        );
        assert!(
            stale
                .summary
                .ends_with("since then 1 modified, 1 deleted, 1 new source files."),
            "{}",
            stale.summary
        );
        assert!(
            stale
                .notes
                .iter()
                .any(|note| note.starts_with("No aetherd daemon is watching")),
            "{:?}",
            stale.notes
        );

        // Holding the watcher lock reads as a running daemon.
        let _lock = WatcherLock::acquire(workspace)
            .expect("lock")
            .expect("free");
        let watched = assess_index_freshness(workspace, &store).expect("assess");
        assert!(watched.watcher_running);

        // Resyncing clears it.
        drop(_lock);
        index_once(workspace);
        let resynced = assess_index_freshness(workspace, &store).expect("assess");
        assert_eq!(resynced.deleted_files, 0, "{resynced:?}");
        assert_eq!(resynced.new_files, 0, "{resynced:?}");
    }

    /// Without a source hash a SIR's staleness can't be checked, so a fresh verdict then
    /// vouches only for symbols and edges, and the summary says so.
    #[test]
    fn fresh_summary_vouches_only_for_the_structure_when_sirs_have_no_hash() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_workspace(workspace);
        index_once(workspace);
        let store = SqliteStore::open(workspace).expect("open store");
        let mut fresh = assess_index_freshness(workspace, &store).expect("assess");
        assert_eq!(fresh.verdict, FreshnessVerdict::Fresh, "{fresh:?}");

        fresh.sir.leaves_without_source_hash = 0;
        let hashed = summary_line(&fresh, 0);
        assert!(hashed.starts_with("Index fresh: "), "{hashed}");

        fresh.sir.leaves_without_source_hash = 2;
        let hashless = summary_line(&fresh, 0);
        assert!(
            hashless.starts_with("Structural index fresh: "),
            "{hashless}"
        );
        assert!(
            hashless.ends_with(
                "no source file changed since. SIR staleness unverifiable for 2 SIRs with no source hash."
            ),
            "{hashless}"
        );
    }

    #[test]
    fn freshness_compares_the_indexed_head_with_the_workspace_head() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_workspace(workspace);
        run_git(workspace, &["init", "-q"]);
        run_git(workspace, &["config", "user.email", "test@example.com"]);
        run_git(workspace, &["config", "user.name", "Test"]);
        run_git(workspace, &["add", "pkg"]);
        run_git(workspace, &["commit", "-q", "-m", "init"]);
        index_once(workspace);

        let head = workspace_head(workspace).expect("head");
        assert_eq!(
            read_index_sync_marker(workspace).head.as_deref(),
            Some(head.as_str())
        );
        let store = SqliteStore::open(workspace).expect("open store");
        let fresh = assess_index_freshness(workspace, &store).expect("assess");
        assert_eq!(fresh.head_matches, Some(true));
        assert_eq!(fresh.verdict, FreshnessVerdict::Fresh, "{fresh:?}");
        assert!(
            fresh.summary.contains("(workspace HEAD matches)"),
            "{}",
            fresh.summary
        );

        run_git(workspace, &["commit", "-q", "--allow-empty", "-m", "next"]);
        let moved = assess_index_freshness(workspace, &store).expect("assess");
        assert_eq!(moved.head_matches, Some(false));
        assert_eq!(moved.verdict, FreshnessVerdict::Stale, "{moved:?}");
        assert!(
            moved
                .summary
                .contains(&format!("at HEAD {}; workspace HEAD is ", &head[..7])),
            "{}",
            moved.summary
        );
    }

    #[test]
    fn symbol_note_says_when_its_file_changed_after_indexing() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_workspace(workspace);
        index_once(workspace);
        let store = SqliteStore::open(workspace).expect("open store");
        let record = store
            .list_symbols_for_file("pkg/a.py")
            .expect("list symbols")
            .into_iter()
            .next()
            .expect("alpha");

        let note = symbol_freshness_note(workspace, &store, &record);
        assert!(
            note.starts_with("Freshness: file unchanged since indexed "),
            "{note}"
        );
        assert!(note.contains("; no SIR; "), "{note}");
        assert!(
            note.ends_with("; no git HEAD; no daemon watching."),
            "{note}"
        );

        write_later(
            &workspace.join("pkg/a.py"),
            "def alpha(x):\n    return x * 2\n",
        );
        let note = symbol_freshness_note(workspace, &store, &record);
        assert!(
            note.starts_with("Freshness: file modified after it was indexed ("),
            "{note}"
        );
    }

    #[test]
    fn watcher_lock_is_seen_while_held() {
        let temp = tempdir().expect("tempdir");
        assert!(!watcher_running(temp.path()));
        let lock = WatcherLock::acquire(temp.path())
            .expect("acquire")
            .expect("lock free");
        assert!(watcher_running(temp.path()));
        assert!(
            WatcherLock::acquire(temp.path())
                .expect("second acquire")
                .is_none()
        );
        drop(lock);
        // A process another test forks meanwhile holds a copy of every open descriptor
        // until it execs, so the release can lag by a moment.
        let released = (0..200).any(|_| {
            let running = watcher_running(temp.path());
            if running {
                std::thread::sleep(Duration::from_millis(10));
            }
            !running
        });
        assert!(released, "the lock outlived its holder");
    }

    fn set_modified_millis(path: &Path, millis: i64) {
        let at = SystemTime::UNIX_EPOCH + Duration::from_millis(millis as u64);
        fs::File::options()
            .write(true)
            .open(path)
            .and_then(|file| file.set_modified(at))
            .expect("set modified time");
    }

    /// A file's recorded read start, not the whole-second `last_seen_at` the sync writes
    /// afterwards, decides whether an edit came later: an edit made while a long walk was
    /// running, or in the same second as the read, is a change, daemon or not.
    #[test]
    fn edits_after_a_file_was_read_count_however_close_to_the_read() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_workspace(workspace);
        index_once(workspace);
        let store = SqliteStore::open(workspace).expect("open store");
        let record = store
            .list_symbols_for_file("pkg/a.py")
            .expect("list symbols")
            .into_iter()
            .next()
            .expect("alpha");
        let walk_ms = read_index_sync_marker(workspace)
            .full_sync_at_ms
            .expect("walk start");
        assert_eq!(
            store.file_read_at_ms("pkg/a.py").expect("read time"),
            Some(walk_ms),
            "a full sync records its walk start as each file's read"
        );
        let path = workspace.join("pkg/a.py");
        let modified = || {
            assess_index_freshness(workspace, &store)
                .expect("assess")
                .modified_files
        };

        // Written well before the walk started: the sync read it.
        set_modified_millis(&path, walk_ms - 1_000);
        assert_eq!(modified(), 0);

        // A write stamped at the read start's own millisecond, or a little before it, may
        // still have come after: file times come from a coarser clock.
        set_modified_millis(&path, walk_ms);
        assert_eq!(modified(), 1);
        set_modified_millis(&path, walk_ms - 5);
        assert_eq!(modified(), 1);

        // Written just after the walk started, in the second the file was indexed.
        set_modified_millis(&path, record.last_seen_at * 1000 + 999);
        assert_eq!(modified(), 1);
        let note = symbol_freshness_note(workspace, &store, &record);
        assert!(
            note.starts_with("Freshness: file modified after it was indexed ("),
            "{note}"
        );
        // A watching daemon is no proof the edit was reread.
        let lock = WatcherLock::acquire(workspace)
            .expect("lock")
            .expect("free");
        assert_eq!(modified(), 1);
        drop(lock);

        // A walk that took seconds: the sync wrote `last_seen_at` well after it read the
        // file, so an edit made in between is older than `last_seen_at` yet newer than
        // the read.
        rusqlite::Connection::open(workspace.join(".aether/meta.sqlite"))
            .and_then(|conn| {
                conn.execute(
                    "UPDATE symbols SET last_seen_at = last_seen_at + 5 WHERE file_path = 'pkg/a.py'",
                    [],
                )
            })
            .expect("move last_seen_at later");
        set_modified_millis(&path, walk_ms + 1);
        assert_eq!(modified(), 1);
    }

    /// An indexed file whose metadata can't be read may have changed, so the index can't
    /// be called fresh while it can't be checked.
    #[cfg(unix)]
    #[test]
    fn an_indexed_file_that_cannot_be_checked_keeps_the_index_from_reading_fresh() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_workspace(workspace);
        let locked = workspace.join("pkg/locked");
        fs::create_dir_all(&locked).expect("create locked dir");
        fs::write(locked.join("x.py"), "def hidden(x):\n    return x\n").expect("write x");
        index_once(workspace);
        let store = SqliteStore::open(workspace).expect("open store");
        assert_eq!(
            assess_index_freshness(workspace, &store)
                .expect("assess")
                .verdict,
            FreshnessVerdict::Fresh
        );

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("lock dir");
        if fs::metadata(locked.join("x.py")).is_ok() {
            // Running as root: permissions can't hide the file here.
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).expect("unlock");
            return;
        }
        let freshness = assess_index_freshness(workspace, &store).expect("assess");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).expect("unlock dir");

        assert_eq!(freshness.verdict, FreshnessVerdict::Stale, "{freshness:?}");
        assert!(freshness.unreadable_files >= 1, "{freshness:?}");
        assert!(
            freshness
                .changed_file_samples
                .contains(&"? pkg/locked/x.py".to_owned()),
            "{freshness:?}"
        );
        assert!(
            freshness
                .summary
                .contains("paths that could not be checked"),
            "{}",
            freshness.summary
        );
    }

    #[test]
    fn only_contention_reads_as_another_lock_holder() {
        assert!(lock_contended(&fs2::lock_contended_error()));
        assert!(lock_contended(&std::io::Error::from(
            std::io::ErrorKind::WouldBlock
        )));
        assert!(!lock_contended(&std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no advisory locks on this filesystem"
        )));
    }

    #[test]
    fn a_second_watcher_is_refused_and_told_who_holds_the_lock() {
        let temp = tempdir().expect("tempdir");
        let _first = take_watcher_lock(temp.path()).expect("first watcher");
        let err = take_watcher_lock(temp.path()).expect_err("a second watcher must not start");
        let message = format!("{err:#}");
        assert!(message.contains("is already watching"), "{message}");
        assert!(
            message.contains(&format!("(pid {})", std::process::id())),
            "{message}"
        );
    }

    #[test]
    fn ages_read_naturally() {
        assert_eq!(describe_age(30), "just now");
        assert_eq!(describe_age(5 * 60), "5 minutes ago");
        assert_eq!(describe_age(3 * 3600), "3 hours ago");
        assert_eq!(describe_age(86_400 * 3), "3 days ago");
        assert_eq!(describe_age(86_400 * 210), "7 months ago");
    }
}
