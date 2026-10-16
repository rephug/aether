//! How far the index has drifted from the workspace, for `aether_status`, `aetherd status`
//! and the freshness line of `aether_sir_context`.
//!
//! The index records when it last synced and at which git HEAD (`.aether/last_indexed_at`
//! and `.aether/last_indexed_head`, written by every full sync and every git-triggered
//! reindex), and each symbol records when its file was last read (`last_seen_at`). A
//! file whose modification time is later than that, an indexed file that is gone, and a
//! source file the index has never seen that appeared after the last sync have all
//! changed since the index. A daemon that watches the workspace holds
//! `.aether/watcher.lock` while it runs.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::time::UNIX_EPOCH;

use aether_core::{GitContext, normalize_path};
use aether_parse::language_for_path;
use aether_store::{SirFreshnessCounts, SqliteStore, SymbolRecord};
use anyhow::{Context, Result};
use fs2::FileExt;
use ignore::WalkBuilder;

use crate::observer::{is_ignored_path, relative_workspace_path};

pub const LAST_INDEXED_AT_FILE: &str = "last_indexed_at";
pub const LAST_INDEXED_HEAD_FILE: &str = "last_indexed_head";
pub const WATCHER_LOCK_FILE: &str = "watcher.lock";

/// Most changed paths a freshness report names.
const MAX_CHANGED_FILE_SAMPLES: usize = 10;

/// Files larger than this are never parsed (see the observer), so they are not "new".
const MAX_INDEXED_FILE_SIZE: u64 = 2 * 1024 * 1024;

/// When the index last synced with the workspace, and at which HEAD.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexSyncMarker {
    /// Unix seconds of the last full sync or git-triggered reindex.
    pub synced_at: Option<i64>,
    /// The commit HEAD pointed at when that sync started.
    pub head: Option<String>,
}

pub fn read_index_sync_marker(workspace: &Path) -> IndexSyncMarker {
    let aether_dir = workspace.join(".aether");
    let read = |name: &str| {
        fs::read_to_string(aether_dir.join(name))
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    IndexSyncMarker {
        synced_at: read(LAST_INDEXED_AT_FILE).and_then(|value| value.parse().ok()),
        head: read(LAST_INDEXED_HEAD_FILE),
    }
}

pub(crate) fn write_index_synced_at(workspace: &Path, synced_at: i64) -> Result<()> {
    let path = workspace.join(".aether").join(LAST_INDEXED_AT_FILE);
    fs::write(&path, format!("{synced_at}\n"))
        .with_context(|| format!("failed to write index sync time {}", path.display()))
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
    /// `None` when another process already holds the lock.
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
        if file.try_lock_exclusive().is_err() {
            return Ok(None);
        }
        file.set_len(0)
            .and_then(|()| writeln!(file, "{}", std::process::id()))
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(Some(Self { _file: file }))
    }
}

/// Whether a daemon holds the watcher lock. A probe takes a shared lock for an instant,
/// so it never blocks a daemon that holds or wants the exclusive one for long.
pub fn watcher_running(workspace: &Path) -> bool {
    let path = workspace.join(".aether").join(WATCHER_LOCK_FILE);
    let Ok(file) = File::open(&path) else {
        return false;
    };
    match file.try_lock_shared() {
        Ok(()) => {
            let _ = FileExt::unlock(&file);
            false
        }
        Err(_) => true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshnessVerdict {
    /// Nothing changed since the index, as far as file times and HEAD tell.
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
    pub last_sync_at: Option<i64>,
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
    /// Indexed files that no longer exist.
    pub deleted_files: u64,
    /// Source files without indexed symbols that appeared or changed after the last sync.
    pub new_files: u64,
    /// Up to ten changed paths, each prefixed `M `, `D ` or `A `.
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
    let mut sirs_in_changed_files = 0u64;
    for file in &indexed {
        match file_time(&workspace.join(&file.file_path)) {
            FileTime::Modified(mtime) if mtime > file.last_indexed_at => {
                modified.push(file.file_path.clone());
                sirs_in_changed_files += file.symbols_with_sir;
            }
            FileTime::Modified(_) | FileTime::Unreadable => {}
            FileTime::Missing => {
                deleted.push(file.file_path.clone());
                sirs_in_changed_files += file.symbols_with_sir;
            }
        }
    }

    // A source file with no symbols may simply declare none (an empty `__init__.py`),
    // so only one that changed after the last sync counts as new.
    let new_files = match marker.synced_at.or(last_symbol_update_at) {
        Some(reference) if !indexed.is_empty() => {
            let indexed_paths = indexed
                .iter()
                .map(|file| file.file_path.as_str())
                .collect::<std::collections::HashSet<_>>();
            source_files(workspace)
                .into_iter()
                .filter(|(path, mtime)| {
                    *mtime > reference && !indexed_paths.contains(path.as_str())
                })
                .map(|(path, _)| path)
                .collect::<Vec<_>>()
        }
        _ => Vec::new(),
    };

    let head_matches = match (marker.head.as_deref(), workspace_head.as_deref()) {
        (Some(indexed), Some(current)) => Some(indexed.eq_ignore_ascii_case(current)),
        _ => None,
    };
    let changed = modified.len() + deleted.len() + new_files.len();
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
        .take(MAX_CHANGED_FILE_SAMPLES)
        .collect::<Vec<_>>();

    let mut freshness = IndexFreshness {
        verdict,
        summary: String::new(),
        notes: Vec::new(),
        last_sync_at: marker.synced_at,
        last_symbol_update_at,
        last_sir_update_at: sir.last_sir_update_at,
        indexed_head: marker.head,
        workspace_head,
        head_matches,
        indexed_files: indexed.len() as u64,
        modified_files: modified.len() as u64,
        deleted_files: deleted.len() as u64,
        new_files: new_files.len() as u64,
        changed_file_samples,
        sirs_in_changed_files,
        sir,
        watcher_running,
    };
    freshness.summary = summary_line(&freshness, now);
    freshness.notes = notes(&freshness);
    Ok(freshness)
}

enum FileTime {
    /// Last modified at these unix seconds.
    Modified(i64),
    Missing,
    /// It exists, but its modification time can't be read.
    Unreadable,
}

fn file_time(path: &Path) -> FileTime {
    match fs::metadata(path) {
        Ok(metadata) => metadata_mtime(&metadata).map_or(FileTime::Unreadable, FileTime::Modified),
        Err(err) if err.kind() == ErrorKind::NotFound => FileTime::Missing,
        Err(_) => FileTime::Unreadable,
    }
}

fn metadata_mtime(metadata: &fs::Metadata) -> Option<i64> {
    let modified = metadata.modified().ok()?;
    let secs = modified.duration_since(UNIX_EPOCH).ok()?.as_secs();
    i64::try_from(secs).ok()
}

/// Every file the indexer would read, as a workspace-relative path with its
/// modification time.
fn source_files(workspace: &Path) -> Vec<(String, i64)> {
    let mut files = Vec::new();
    for entry in WalkBuilder::new(workspace)
        .standard_filters(true)
        .build()
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        if is_ignored_path(path) || language_for_path(path).is_none() {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.len() > MAX_INDEXED_FILE_SIZE {
            continue;
        }
        let Some(mtime) = metadata_mtime(&metadata) else {
            continue;
        };
        let relative = normalize_path(&relative_workspace_path(workspace, path).to_string_lossy());
        files.push((relative, mtime));
    }
    files
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
        FreshnessVerdict::Fresh => {
            format!("Index fresh: {synced} {head}; no source file changed since.")
        }
        FreshnessVerdict::Stale => format!(
            "Index STALE: {synced} {head}; since then {} modified, {} deleted, {} new source files.",
            freshness.modified_files, freshness.deleted_files, freshness.new_files
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
        let changed = freshness.modified_files + freshness.deleted_files + freshness.new_files;
        let more = changed.saturating_sub(freshness.changed_file_samples.len() as u64);
        notes.push(format!(
            "Changed since the index (M modified, D deleted, A new): {}{}.",
            freshness.changed_file_samples.join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            }
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
            "The HEAD this index was built at is not recorded; the next sync records it."
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
    let indexed = describe_time(record.last_seen_at, now);
    let file = match file_time(&workspace.join(&record.file_path)) {
        FileTime::Missing => format!("file missing from the workspace (indexed {indexed})"),
        FileTime::Modified(mtime) if mtime > record.last_seen_at => {
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

    let marker = read_index_sync_marker(workspace);
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
    let daemon = if watcher_running(workspace) {
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
        FreshnessVerdict, WatcherLock, assess_index_freshness, describe_age,
        read_index_sync_marker, symbol_freshness_note, watcher_running, workspace_head,
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

    #[test]
    fn ages_read_naturally() {
        assert_eq!(describe_age(30), "just now");
        assert_eq!(describe_age(5 * 60), "5 minutes ago");
        assert_eq!(describe_age(3 * 3600), "3 hours ago");
        assert_eq!(describe_age(86_400 * 3), "3 days ago");
        assert_eq!(describe_age(86_400 * 210), "7 months ago");
    }
}
