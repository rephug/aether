//! Git-aware watching for the indexing loop: classifying watcher events, debouncing
//! source edits behind a git operation, tracking HEAD and turning a commit, checkout or
//! merge into the set of paths to re-index.

use super::structural::StructuralIndexer;
use super::worker::{SharedQueueState, enqueue_changed_symbols};
use super::*;

#[derive(Debug, Default)]
pub(super) struct GitDebounceState {
    pub(super) last_git_event_at: Option<Instant>,
    pub(super) dirty_paths: HashSet<PathBuf>,
}

impl GitDebounceState {
    pub(super) fn mark_git_event(&mut self, now: Instant) {
        self.last_git_event_at = Some(now);
    }

    pub(super) fn has_pending(&self) -> bool {
        self.last_git_event_at.is_some()
    }

    pub(super) fn extend_dirty<I>(&mut self, paths: I)
    where
        I: IntoIterator<Item = PathBuf>,
    {
        self.dirty_paths.extend(paths);
    }

    pub(super) fn should_fire(&self, now: Instant, debounce_window: Duration) -> bool {
        self.last_git_event_at
            .is_some_and(|last_seen| now.saturating_duration_since(last_seen) >= debounce_window)
    }

    pub(super) fn take_dirty_paths(&mut self) -> HashSet<PathBuf> {
        self.last_git_event_at = None;
        std::mem::take(&mut self.dirty_paths)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct HeadState {
    pub(super) sha: Option<String>,
    pub(super) marker: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GitTriggerKind {
    BranchSwitch,
    GitPull,
    Merge,
}

#[derive(Debug, Default)]
pub(super) struct ClassifiedWatchEvent {
    pub(super) git_event: bool,
    pub(super) source_paths: Vec<PathBuf>,
    pub(super) watch_dirs: Vec<PathBuf>,
}

pub(super) fn handle_watch_result(
    workspace: &Path,
    git_watch_dir: Option<&Path>,
    watcher: &mut RecommendedWatcher,
    result: notify::Result<Event>,
    debounce_queue: &mut DebounceQueue,
    git_debounce_state: &mut GitDebounceState,
) -> Result<()> {
    let event = result.context("notify error")?;
    let classified = classify_watch_event(workspace, git_watch_dir, event);
    for path in &classified.watch_dirs {
        let _ = watcher.watch(path, RecursiveMode::NonRecursive);
    }
    enqueue_classified_watch_event(classified, debounce_queue, git_debounce_state);
    Ok(())
}

pub(super) fn classify_watch_event(
    workspace: &Path,
    git_watch_dir: Option<&Path>,
    event: Event,
) -> ClassifiedWatchEvent {
    let mut classified = ClassifiedWatchEvent::default();
    for path in event.paths {
        if git_watch_dir.is_some_and(|git_dir| path.starts_with(git_dir)) {
            classified.git_event = true;
            continue;
        }

        if path.is_dir() {
            if !is_ignored_path(&path) {
                classified.watch_dirs.push(path);
            }
            continue;
        }

        if is_ignored_path(&path) {
            continue;
        }
        if let Ok(relative) = path.strip_prefix(workspace)
            && is_ignored_path(relative)
        {
            continue;
        }

        classified.source_paths.push(path);
    }

    classified
}

pub(super) fn enqueue_classified_watch_event(
    classified: ClassifiedWatchEvent,
    debounce_queue: &mut DebounceQueue,
    git_debounce_state: &mut GitDebounceState,
) {
    let now = Instant::now();
    if classified.git_event {
        git_debounce_state.mark_git_event(now);
    }

    if git_debounce_state.has_pending() {
        git_debounce_state.extend_dirty(classified.source_paths);
    } else {
        for path in classified.source_paths {
            debounce_queue.mark(path, now);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn process_git_trigger(
    config: &IndexerConfig,
    watcher_runtime: &WatcherRuntimeConfig,
    git_watch_dir: Option<&Path>,
    observer: &mut ObserverState,
    structural: &mut StructuralIndexer,
    store: &SqliteStore,
    queue_state: &SharedQueueState,
    dirty_paths: HashSet<PathBuf>,
) -> Result<()> {
    let started_at = crate::time::current_unix_timestamp_secs();
    let previous_head = read_persisted_head_state(config.workspace.as_path())?;
    let current_head = read_current_head_state(config.workspace.as_path(), git_watch_dir)?;
    let indexed_commit = crate::freshness::read_index_sync_marker(config.workspace.as_path()).head;
    let mut paths = dirty_paths.into_iter().collect::<BTreeSet<_>>();
    // Whether this reindex covers every change between the commit the freshness marker
    // names and the new HEAD. The watcher's own base advances even past a skipped
    // trigger, so a diff from it covers the marker's commit only when the two agree.
    let mut catches_up = false;

    if previous_head.sha != current_head.sha {
        let trigger_kind =
            classify_git_trigger_kind(config.workspace.as_path(), &previous_head, &current_head);
        if trigger_kind.is_some_and(|kind| git_trigger_enabled(&watcher_runtime.watcher, kind)) {
            let (git_paths, covers_indexed_commit) = if !watcher_runtime
                .watcher
                .git_trigger_changed_files_only
                || previous_head.sha.is_none()
            {
                collect_full_reindex_paths(config.workspace.as_path(), observer)
            } else if let (Some(old_sha), Some(new_sha)) =
                (previous_head.sha.as_deref(), current_head.sha.as_deref())
            {
                match changed_paths_between_heads(config.workspace.as_path(), old_sha, new_sha) {
                    Ok(paths) => (paths, indexed_commit.as_deref() == Some(old_sha)),
                    Err(err) => {
                        tracing::warn!(
                            old_sha,
                            new_sha,
                            error = %err,
                            "failed to diff git heads; falling back to full reindex"
                        );
                        collect_full_reindex_paths(config.workspace.as_path(), observer)
                    }
                }
            } else {
                collect_full_reindex_paths(config.workspace.as_path(), observer)
            };
            catches_up = covers_indexed_commit;
            paths.extend(git_paths);
        }
    }

    if !paths.is_empty() {
        tracing::info!(path_count = paths.len(), "processing watcher reindex batch");
        process_reindex_paths(
            config,
            observer,
            structural,
            store,
            queue_state,
            paths.into_iter().collect::<Vec<_>>(),
        )?;
    }

    write_persisted_head_state(config.workspace.as_path(), &current_head)?;
    // A skipped trigger, a diff from a base the marker doesn't name, or a failed reindex
    // (returned above) leaves the freshness marker on the commit it already names.
    if catches_up
        && let Some(head) = current_head.sha.as_deref()
        && let Err(err) =
            crate::freshness::record_head_caught_up(config.workspace.as_path(), head, started_at)
    {
        tracing::warn!(error = %err, "failed to record the index sync marker");
    }
    Ok(())
}

pub(super) fn process_reindex_paths(
    config: &IndexerConfig,
    observer: &mut ObserverState,
    structural: &mut StructuralIndexer,
    store: &SqliteStore,
    queue_state: &SharedQueueState,
    paths: Vec<PathBuf>,
) -> Result<()> {
    refresh_paths(config, structural, &paths);
    let processed = paths.iter().try_for_each(|path| {
        process_reindex_path(config, observer, structural, store, queue_state, path)
    });
    structural.reindex_stale_files(store);
    processed
}

/// Re-indexes the paths the watcher's debounce released. A path that fails is logged and
/// the rest go on.
pub(super) fn process_debounced_paths(
    config: &IndexerConfig,
    observer: &mut ObserverState,
    structural: &mut StructuralIndexer,
    store: &SqliteStore,
    queue_state: &SharedQueueState,
    paths: Vec<PathBuf>,
) {
    refresh_paths(config, structural, &paths);
    for path in &paths {
        if let Err(err) =
            process_reindex_path(config, observer, structural, store, queue_state, path)
        {
            tracing::error!(path = %path.display(), error = %err, "process error");
        }
    }
    structural.reindex_stale_files(store);
}

/// Re-reads a batch of changed paths for the Python call targets resolved through them,
/// symbols or not (a package's `__init__.py` that only re-exports has none), before any
/// file of the batch is re-extracted. The files whose targets they may move are
/// re-extracted once the batch is done (`StructuralIndexer::reindex_stale_files`), also
/// when it stops early.
fn refresh_paths(config: &IndexerConfig, structural: &mut StructuralIndexer, paths: &[PathBuf]) {
    for path in paths {
        structural.refresh_file(&normalize_path(
            &relative_workspace_path(&config.workspace, path).to_string_lossy(),
        ));
    }
}

/// A source file on disk the observer holds no symbols for, as its workspace-relative path
/// and language.
fn symbol_free_source(
    config: &IndexerConfig,
    path: &Path,
) -> Option<(String, aether_core::Language)> {
    if !path.is_file() {
        return None;
    }
    let language = language_for_path(path)?;
    let file_path =
        normalize_path(&relative_workspace_path(&config.workspace, path).to_string_lossy());
    Some((file_path, language))
}

fn process_reindex_path(
    config: &IndexerConfig,
    observer: &mut ObserverState,
    structural: &mut StructuralIndexer,
    store: &SqliteStore,
    queue_state: &SharedQueueState,
    path: &Path,
) -> Result<()> {
    // Taken before the read, so an edit landing while the file is read counts as newer.
    let read_started_ms = crate::time::current_unix_timestamp_millis();
    let read = match observer.process_path(path) {
        Ok(Some(event)) => {
            if config.print_events {
                let line = serde_json::to_string(&event)
                    .context("failed to serialize symbol-change event")?;
                println!("{line}");
            }

            structural.process_event(store, &event)?;
            for removed in &event.removed {
                queue_state.remove_symbol(&removed.id);
            }
            let mut changed = Vec::new();
            collect_changed_symbols(&event, &mut changed);
            for symbol in &changed {
                queue_state.upsert_symbol(symbol.clone());
            }
            if let Err(err) =
                enqueue_changed_symbols(config.workspace.as_path(), store, queue_state, &changed)
            {
                tracing::warn!(
                    file_path = %event.file_path,
                    error = %err,
                    "failed to enqueue changed symbols"
                );
            }
            true
        }
        // A file skipped for its size was not read: its stored symbols may describe older
        // text, so nothing may count it as read.
        Ok(None) if observer.skipped_as_oversized(path) => false,
        // A source file that is gone and had no symbols (so no event removes any) can still
        // have edges and test intents stored. An empty event clears them.
        Ok(None) if !path.exists() => {
            if let Some(language) = language_for_path(path) {
                structural.process_event(
                    store,
                    &SymbolChangeEvent {
                        file_path: normalize_path(
                            &relative_workspace_path(&config.workspace, path).to_string_lossy(),
                        ),
                        language,
                        added: Vec::new(),
                        removed: Vec::new(),
                        updated: Vec::new(),
                    },
                )?;
            }
            false
        }
        // An edit that changes no symbol (a comment, blank lines, an import) still changes
        // the file and maybe its edges. Re-extract them, or status would report the file
        // as modified until the next full sync. A source file without symbols (imports
        // only, top-level code) has edges too, so it is handled the same way. A file the
        // index read since it last changed, as a branch switch leaves most, is skipped.
        Ok(None) => match observer
            .symbols_of(path)
            .map(|(file_path, language, _)| (file_path, language))
            .or_else(|| symbol_free_source(config, path))
        {
            Some((file_path, language))
                if crate::freshness::file_changed_since_read(
                    &config.workspace,
                    store,
                    &file_path,
                )? =>
            {
                structural.process_event(
                    store,
                    &SymbolChangeEvent {
                        file_path,
                        language,
                        added: Vec::new(),
                        removed: Vec::new(),
                        updated: Vec::new(),
                    },
                )?;
                true
            }
            _ => false,
        },
        Err(err) => {
            return Err(err).with_context(|| format!("failed to process {}", path.display()));
        }
    };

    // Every symbol the file still declares now matches its row, the unchanged ones too, so
    // the whole file counts as read; only changed symbols were rewritten above.
    if read && let Some((_, _, symbols)) = observer.symbols_of(path) {
        let symbol_ids = symbols
            .iter()
            .map(|symbol| symbol.id.clone())
            .collect::<Vec<_>>();
        store
            .mark_symbols_read(&symbol_ids, unix_timestamp_secs())
            .with_context(|| format!("failed to record reading {}", path.display()))?;
    }

    let file_path =
        normalize_path(&relative_workspace_path(&config.workspace, path).to_string_lossy());
    if path.is_file() && language_for_path(path).is_some() && !observer.skipped_as_oversized(path) {
        if let Err(err) = store.record_file_reads(&[file_path], read_started_ms) {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "failed to record when the file was read"
            );
        }
    } else if !path.exists()
        && let Err(err) = store.forget_file_reads(&[file_path])
    {
        tracing::warn!(
            path = %path.display(),
            error = %err,
            "failed to forget the read of a removed file"
        );
    }
    Ok(())
}

pub(super) fn resolve_git_watch_dir(workspace: &Path) -> Option<PathBuf> {
    let dot_git = workspace.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }

    let raw = fs::read_to_string(&dot_git).ok()?;
    let git_dir = raw.strip_prefix("gitdir:")?.trim();
    let path = PathBuf::from(git_dir);
    Some(if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    })
}

pub(super) fn read_persisted_head_state(workspace: &Path) -> Result<HeadState> {
    let aether_dir = workspace.join(".aether");
    Ok(HeadState {
        sha: read_optional_trimmed_file(aether_dir.join("last_indexed_head"))?,
        marker: read_optional_trimmed_file(aether_dir.join("last_indexed_head_ref"))?,
    })
}

pub(super) fn write_persisted_head_state(workspace: &Path, head_state: &HeadState) -> Result<()> {
    let aether_dir = workspace.join(".aether");
    fs::create_dir_all(&aether_dir).with_context(|| {
        format!(
            "failed to create watcher state directory {}",
            aether_dir.display()
        )
    })?;
    write_optional_trimmed_file(
        aether_dir.join("last_indexed_head"),
        head_state.sha.as_deref(),
    )?;
    write_optional_trimmed_file(
        aether_dir.join("last_indexed_head_ref"),
        head_state.marker.as_deref(),
    )?;
    Ok(())
}

/// HEAD now, as the watcher reads it: the commit and what `.git/HEAD` says.
pub(crate) fn current_head(workspace: &Path) -> Result<(Option<String>, Option<String>)> {
    let head = read_current_head_state(workspace, resolve_git_watch_dir(workspace).as_deref())?;
    Ok((head.sha, head.marker))
}

pub(super) fn read_current_head_state(
    workspace: &Path,
    git_watch_dir: Option<&Path>,
) -> Result<HeadState> {
    let marker = git_watch_dir
        .map(|git_dir| git_dir.join("HEAD"))
        .map(read_optional_trimmed_file)
        .transpose()?
        .flatten();
    let sha = GitContext::open(workspace)
        .and_then(|context| context.head_commit_hash())
        .or_else(|| {
            marker
                .as_deref()
                .filter(|value| !value.starts_with("ref:"))
                .map(str::to_owned)
        });
    Ok(HeadState { sha, marker })
}

pub(super) fn read_optional_trimmed_file(path: PathBuf) -> Result<Option<String>> {
    match fs::read_to_string(&path) {
        Ok(raw) => Ok({
            let trimmed = raw.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_owned())
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err)
            .with_context(|| format!("failed to read watcher state file {}", path.display())),
    }
}

pub(super) fn write_optional_trimmed_file(path: PathBuf, value: Option<&str>) -> Result<()> {
    if let Some(value) = value {
        fs::write(&path, format!("{}\n", value.trim()))
            .with_context(|| format!("failed to write watcher state file {}", path.display()))?;
    } else if let Err(err) = fs::remove_file(&path)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        return Err(err)
            .with_context(|| format!("failed to remove watcher state file {}", path.display()));
    }
    Ok(())
}

pub(super) fn classify_git_trigger_kind(
    workspace: &Path,
    previous_head: &HeadState,
    current_head: &HeadState,
) -> Option<GitTriggerKind> {
    let previous_sha = previous_head.sha.as_deref()?;
    let current_sha = current_head.sha.as_deref()?;
    if previous_sha == current_sha {
        return None;
    }
    if previous_head.marker.as_deref() != current_head.marker.as_deref() {
        return Some(GitTriggerKind::BranchSwitch);
    }
    match current_commit_parent_count(workspace, current_sha) {
        Ok(parent_count) if parent_count > 1 => Some(GitTriggerKind::Merge),
        Ok(_) => Some(GitTriggerKind::GitPull),
        Err(err) => {
            tracing::warn!(
                current_sha,
                error = %err,
                "failed to classify git head advance; treating as git pull"
            );
            Some(GitTriggerKind::GitPull)
        }
    }
}

pub(super) fn current_commit_parent_count(workspace: &Path, commit_hash: &str) -> Result<usize> {
    let repo = gix::discover(workspace).context("failed to open git repo")?;
    let commit_id = repo
        .rev_parse_single(commit_hash)
        .with_context(|| format!("failed to resolve commit {commit_hash}"))?;
    let commit = repo
        .find_commit(commit_id.detach())
        .with_context(|| format!("failed to load commit {commit_hash}"))?;
    Ok(commit.parent_ids().count())
}

pub(super) fn git_trigger_enabled(watcher: &WatcherConfig, trigger: GitTriggerKind) -> bool {
    match trigger {
        GitTriggerKind::BranchSwitch => watcher.trigger_on_branch_switch,
        GitTriggerKind::GitPull => watcher.trigger_on_git_pull,
        GitTriggerKind::Merge => watcher.trigger_on_merge,
    }
}

pub(super) fn changed_paths_between_heads(
    workspace: &Path,
    old_sha: &str,
    new_sha: &str,
) -> Result<Vec<PathBuf>> {
    let repo = gix::discover(workspace).context("failed to open git repo for diff")?;
    let old_id = repo
        .rev_parse_single(old_sha)
        .with_context(|| format!("failed to resolve previous head {old_sha}"))?;
    let new_id = repo
        .rev_parse_single(new_sha)
        .with_context(|| format!("failed to resolve current head {new_sha}"))?;
    let old_commit = repo
        .find_commit(old_id.detach())
        .with_context(|| format!("failed to load previous head commit {old_sha}"))?;
    let new_commit = repo
        .find_commit(new_id.detach())
        .with_context(|| format!("failed to load current head commit {new_sha}"))?;
    let old_tree = old_commit
        .tree()
        .with_context(|| format!("failed to load previous head tree {old_sha}"))?;
    let new_tree = new_commit
        .tree()
        .with_context(|| format!("failed to load current head tree {new_sha}"))?;

    let mut diff_options = gix::diff::Options::default();
    diff_options.track_rewrites(None);
    let changes = repo
        .diff_tree_to_tree(Some(&old_tree), Some(&new_tree), Some(diff_options))
        .with_context(|| format!("failed to diff git heads {old_sha}..{new_sha}"))?;

    let mut paths = BTreeSet::new();
    for change in changes {
        let raw_path = change.location().to_str_lossy();
        let normalized = normalize_path(normalize_git_rename_path(raw_path.as_ref()).as_str());
        if normalized.is_empty() || is_ignored_path(Path::new(&normalized)) {
            continue;
        }
        paths.insert(workspace.join(normalized));
    }

    Ok(paths.into_iter().collect())
}

pub(super) fn normalize_git_rename_path(path: &str) -> String {
    let value = path.trim();
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
    if let Some((_, right)) = value.rsplit_once("=>") {
        return right.trim().to_owned();
    }
    value.to_owned()
}

/// Every path a full reindex reads: those the observer tracks and every source file on
/// disk. The flag says whether the walk read the whole workspace; a path it couldn't read
/// is logged and left out, so a reindex of the rest doesn't cover the workspace.
pub(super) fn collect_full_reindex_paths(
    workspace: &Path,
    observer: &ObserverState,
) -> (Vec<PathBuf>, bool) {
    let mut paths = observer
        .tracked_paths()
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut complete = true;
    for entry in WalkBuilder::new(workspace).standard_filters(true).build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                complete = false;
                tracing::warn!(
                    workspace = %workspace.display(),
                    error = %err,
                    "full reindex could not read a path; it is left out"
                );
                continue;
            }
        };
        if !entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        if is_ignored_path(entry.path()) || language_for_path(entry.path()).is_none() {
            continue;
        }
        paths.insert(entry.path().to_path_buf());
    }
    (paths.into_iter().collect(), complete)
}

#[cfg(test)]
mod tests;
