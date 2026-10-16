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
        // An edit that changes no symbol (a comment, blank lines, an import) still changes
        // the file and maybe its edges. Re-extract them, or status would report the file
        // as modified until the next full sync. A file the index read since it last
        // changed, as a branch switch leaves most, is skipped.
        Ok(None) => match observer.symbols_of(path) {
            Some((file_path, language, _))
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

    if path.is_file() && language_for_path(path).is_some() {
        let file_path =
            normalize_path(&relative_workspace_path(&config.workspace, path).to_string_lossy());
        if let Err(err) = store.record_file_reads(&[file_path], read_started_ms) {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "failed to record when the file was read"
            );
        }
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
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn resolve_git_watch_dir_follows_worktree_gitdir_file() {
        let temp = tempdir().expect("tempdir");
        let git_admin = temp.path().join("git-admin/worktrees/demo");
        fs::create_dir_all(&git_admin).expect("create git admin dir");
        fs::write(
            temp.path().join(".git"),
            format!("gitdir: {}\n", git_admin.display()),
        )
        .expect("write gitdir pointer");

        let resolved = resolve_git_watch_dir(temp.path()).expect("resolve git watch dir");
        assert_eq!(resolved, git_admin);
    }

    #[test]
    fn classify_watch_event_recognizes_git_paths_outside_workspace() {
        let temp = tempdir().expect("tempdir");
        let git_admin = temp.path().join("git-admin/worktrees/demo");
        let source_file = temp.path().join("src/lib.rs");
        fs::create_dir_all(source_file.parent().expect("source parent")).expect("create src dir");
        let event = Event {
            kind: notify::EventKind::Modify(notify::event::ModifyKind::Any),
            paths: vec![git_admin.join("HEAD"), source_file.clone()],
            attrs: Default::default(),
        };

        let classified = classify_watch_event(temp.path(), Some(&git_admin), event);
        assert!(classified.git_event);
        assert_eq!(classified.source_paths, vec![source_file]);
    }

    #[test]
    fn git_events_suppress_normal_file_debounce_until_settled() {
        let mut debounce_queue = DebounceQueue::default();
        let mut git_state = GitDebounceState::default();
        let source_a = PathBuf::from("src/lib.rs");
        let source_b = PathBuf::from("src/main.rs");

        enqueue_classified_watch_event(
            ClassifiedWatchEvent {
                git_event: true,
                source_paths: vec![source_a.clone()],
                watch_dirs: Vec::new(),
            },
            &mut debounce_queue,
            &mut git_state,
        );
        enqueue_classified_watch_event(
            ClassifiedWatchEvent {
                git_event: false,
                source_paths: vec![source_b.clone()],
                watch_dirs: Vec::new(),
            },
            &mut debounce_queue,
            &mut git_state,
        );

        assert!(git_state.has_pending());
        assert!(
            debounce_queue
                .drain_due(Instant::now(), Duration::ZERO)
                .is_empty()
        );

        let mut dirty = git_state.take_dirty_paths().into_iter().collect::<Vec<_>>();
        dirty.sort();
        assert_eq!(dirty, vec![source_a, source_b]);
    }

    #[test]
    fn a_reexport_change_reextracts_the_python_callers_resolved_through_it() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        crate::indexer::tests::write_default_config(workspace);
        for (path, source) in [
            ("pkg/__init__.py", "from .old import f\n"),
            ("pkg/old.py", "def f():\n    return 1\n"),
            ("pkg/new.py", "def f():\n    return 2\n"),
            (
                "tests/test_pkg.py",
                "from pkg import f\n\n\ndef test_f():\n    f()\n",
            ),
        ] {
            let full_path = workspace.join(path);
            fs::create_dir_all(full_path.parent().expect("parent")).expect("create dirs");
            fs::write(full_path, source).expect("write source");
        }
        let config = IndexerConfig {
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
            inference_provider: None,
            inference_model: None,
            inference_endpoint: None,
            inference_api_key_env: None,
            pause_flag: None,
        };
        let mut observer = ObserverState::new(workspace.to_path_buf()).expect("observer");
        observer.seed_from_disk().expect("seed observer");
        let store = SqliteStore::open(workspace).expect("open store");
        let mut structural =
            StructuralIndexer::new(workspace.to_path_buf()).expect("structural indexer");
        for event in observer.initial_symbol_events() {
            structural
                .process_event(&store, &event)
                .expect("index file");
        }
        let queue_state = SharedQueueState::new(HashMap::new());

        let id = |file: &str, qualified_name: &str| {
            store
                .list_symbols_for_file(file)
                .expect("list symbols")
                .into_iter()
                .find(|symbol| symbol.qualified_name == qualified_name)
                .unwrap_or_else(|| panic!("missing symbol {qualified_name}"))
                .id
        };
        let test_f = id("tests/test_pkg.py", "tests.test_pkg::test_f");
        let callees = || {
            store
                .list_graph_dependency_edges()
                .expect("list edges")
                .into_iter()
                .filter(|edge| edge.edge_kind == "calls" && edge.source_symbol_id == test_f)
                .map(|edge| edge.target_symbol_id)
                .collect::<Vec<_>>()
        };
        assert_eq!(callees(), vec![id("pkg/old.py", "pkg.old::f")]);

        // The package re-exports `f` from another module. Its `__init__.py` has no symbols,
        // so the edit is no symbol change, and the test file itself is unchanged.
        fs::write(workspace.join("pkg/__init__.py"), "from .new import f\n").expect("edit");
        process_reindex_paths(
            &config,
            &mut observer,
            &mut structural,
            &store,
            &queue_state,
            vec![workspace.join("pkg/__init__.py")],
        )
        .expect("reindex the package");
        assert_eq!(callees(), vec![id("pkg/new.py", "pkg.new::f")]);

        // The same through the watcher's debounce path.
        fs::write(workspace.join("pkg/__init__.py"), "from .old import f\n").expect("edit");
        process_debounced_paths(
            &config,
            &mut observer,
            &mut structural,
            &store,
            &queue_state,
            vec![workspace.join("pkg/__init__.py")],
        );
        assert_eq!(callees(), vec![id("pkg/old.py", "pkg.old::f")]);
    }

    /// A watched edit that changes no symbol (a comment, an import) must not leave the
    /// file "modified" until the next full sync, and an edit to one symbol must not leave
    /// its unchanged neighbours looking stale.
    #[test]
    fn a_watched_edit_that_changes_no_symbol_leaves_the_index_fresh() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
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
            "def beta(x):\n    return x\n\n\ndef gamma(x):\n    return x\n",
        )
        .expect("write b");
        let config = mock_config(workspace);
        run_initial_index_once(&config).expect("index once");

        // Held as the daemon that runs the debounce path holds it.
        let _watcher_lock = crate::freshness::take_watcher_lock(workspace).expect("watcher lock");
        let mut observer = ObserverState::new(workspace.to_path_buf()).expect("observer");
        observer.seed_from_disk().expect("seed observer");
        let mut structural =
            StructuralIndexer::new(workspace.to_path_buf()).expect("structural indexer");
        let store = SqliteStore::open(workspace).expect("open store");
        let queue_state = SharedQueueState::new(HashMap::new());

        // The index read both files two minutes ago, so edits made now come after it.
        for file in ["pkg/a.py", "pkg/b.py"] {
            for record in store.list_symbols_for_file(file).expect("list symbols") {
                store
                    .upsert_symbol(SymbolRecord {
                        last_seen_at: record.last_seen_at - 120,
                        ..record
                    })
                    .expect("backdate symbol");
            }
        }
        let alpha_before = store.list_symbols_for_file("pkg/a.py").expect("list a");

        fs::write(
            workspace.join("pkg/a.py"),
            "import os\n\n# A note that changes no symbol.\ndef alpha(x):\n    return x + 1\n",
        )
        .expect("edit a");
        fs::write(
            workspace.join("pkg/b.py"),
            "def beta(x):\n    return x\n\n\ndef gamma(x):\n    return x * 2\n",
        )
        .expect("edit b");
        let before =
            crate::freshness::assess_index_freshness(workspace, &store).expect("assess before");
        assert_eq!(
            before.verdict,
            crate::freshness::FreshnessVerdict::Stale,
            "{before:?}"
        );
        assert_eq!(before.modified_files, 2, "{before:?}");

        process_debounced_paths(
            &config,
            &mut observer,
            &mut structural,
            &store,
            &queue_state,
            vec![workspace.join("pkg/a.py"), workspace.join("pkg/b.py")],
        );

        // No symbol of a.py changed, and its ids are the ones indexed before.
        let alpha_after = store.list_symbols_for_file("pkg/a.py").expect("list a");
        assert_eq!(
            alpha_after
                .iter()
                .map(|symbol| &symbol.id)
                .collect::<Vec<_>>(),
            alpha_before
                .iter()
                .map(|symbol| &symbol.id)
                .collect::<Vec<_>>()
        );
        let after =
            crate::freshness::assess_index_freshness(workspace, &store).expect("assess after");
        assert_eq!(
            after.verdict,
            crate::freshness::FreshnessVerdict::Fresh,
            "{after:?}"
        );
        assert_eq!(after.modified_files, 0, "{after:?}");

        // beta didn't change, but the watcher read its file and found it as stored.
        let beta = store
            .list_symbols_for_file("pkg/b.py")
            .expect("list b")
            .into_iter()
            .find(|symbol| symbol.qualified_name.ends_with("beta"))
            .expect("beta");
        let note = crate::freshness::symbol_freshness_note(workspace, &store, &beta);
        assert!(
            note.starts_with("Freshness: file unchanged since indexed "),
            "{note}"
        );
    }

    fn run_git(workspace: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
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

    fn mock_config(workspace: &Path) -> IndexerConfig {
        IndexerConfig {
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
        }
    }

    /// The watcher's state for one git workspace: indexed on `main`, while a `feature`
    /// branch adds `pkg/added.py`. Returns the two commits.
    struct WatchedRepo {
        /// Held as the daemon that runs these triggers holds it.
        _watcher_lock: crate::freshness::WatcherLock,
        config: IndexerConfig,
        runtime: WatcherRuntimeConfig,
        observer: ObserverState,
        structural: StructuralIndexer,
        store: SqliteStore,
        queue_state: SharedQueueState,
        main: String,
        feature: String,
    }

    impl WatchedRepo {
        fn new(workspace: &Path, watcher_toml: &str) -> Self {
            fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
            fs::write(
                workspace.join(".aether/config.toml"),
                format!(
                    "[inference]\nprovider = \"mock\"\n\n[storage]\ngraph_backend = \"sqlite\"\n\n[embeddings]\nenabled = false\nvector_backend = \"sqlite\"\n\n[watcher]\n{watcher_toml}\n"
                ),
            )
            .expect("write config");
            fs::create_dir_all(workspace.join("pkg")).expect("create pkg");
            fs::write(
                workspace.join("pkg/a.py"),
                "def alpha(x):\n    return x + 1\n",
            )
            .expect("write a");
            run_git(workspace, &["init", "-q", "-b", "main"]);
            run_git(workspace, &["config", "user.email", "test@example.com"]);
            run_git(workspace, &["config", "user.name", "Test"]);
            run_git(workspace, &["add", "pkg"]);
            run_git(workspace, &["commit", "-q", "-m", "init"]);
            let main = crate::freshness::workspace_head(workspace).expect("main head");
            run_git(workspace, &["checkout", "-q", "-b", "feature"]);
            fs::write(
                workspace.join("pkg/added.py"),
                "def added(x):\n    return x\n",
            )
            .expect("write added");
            run_git(workspace, &["add", "pkg"]);
            run_git(workspace, &["commit", "-q", "-m", "add a module"]);
            let feature = crate::freshness::workspace_head(workspace).expect("feature head");
            run_git(workspace, &["checkout", "-q", "main"]);
            assert!(!workspace.join("pkg/added.py").exists());

            let config = mock_config(workspace);
            run_initial_index_once(&config).expect("index once on main");
            // The daemon's state from before the branch switch.
            let runtime = WatcherRuntimeConfig::from_workspace(&config).expect("watcher runtime");
            let mut observer = ObserverState::new(workspace.to_path_buf()).expect("observer");
            observer.seed_from_disk().expect("seed observer");
            Self {
                _watcher_lock: crate::freshness::take_watcher_lock(workspace)
                    .expect("watcher lock"),
                runtime,
                observer,
                structural: StructuralIndexer::new(workspace.to_path_buf())
                    .expect("structural indexer"),
                store: SqliteStore::open(workspace).expect("open store"),
                queue_state: SharedQueueState::new(HashMap::new()),
                config,
                main,
                feature,
            }
        }

        /// What the loop does once a git event settles. No file event is passed along, as
        /// when the new file lands in a directory the watcher doesn't see.
        fn fire_git_trigger(&mut self) {
            let workspace = self.config.workspace.clone();
            process_git_trigger(
                &self.config,
                &self.runtime,
                resolve_git_watch_dir(&workspace).as_deref(),
                &mut self.observer,
                &mut self.structural,
                &self.store,
                &self.queue_state,
                HashSet::new(),
            )
            .expect("git trigger");
        }

        fn indexes(&self, file_path: &str) -> bool {
            !self
                .store
                .list_symbols_for_file(file_path)
                .expect("list symbols")
                .is_empty()
        }
    }

    /// A branch switch the watcher is configured to skip indexes nothing, so the freshness
    /// marker must keep the commit it named: status reports the switch and the file it
    /// brought instead of calling the index fresh.
    #[test]
    fn a_skipped_branch_switch_leaves_the_index_sync_marker_unchanged() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let mut repo = WatchedRepo::new(workspace, "trigger_on_branch_switch = false");
        let before = crate::freshness::read_index_sync_marker(workspace);
        assert_eq!(before.head.as_deref(), Some(repo.main.as_str()));

        run_git(workspace, &["checkout", "-q", "feature"]);
        repo.fire_git_trigger();

        assert!(!repo.indexes("pkg/added.py"), "the switch was skipped");
        assert_eq!(crate::freshness::read_index_sync_marker(workspace), before);
        let freshness =
            crate::freshness::assess_index_freshness(workspace, &repo.store).expect("assess");
        assert_eq!(
            freshness.verdict,
            crate::freshness::FreshnessVerdict::Stale,
            "{freshness:?}"
        );
        assert_eq!(freshness.head_matches, Some(false));
        assert_eq!(
            freshness.workspace_head.as_deref(),
            Some(repo.feature.as_str())
        );
        assert_eq!(freshness.new_files, 1, "{freshness:?}");
        assert_eq!(freshness.changed_file_samples, ["A pkg/added.py"]);
    }

    /// An enabled branch switch that brings a new source file indexes it, and only then
    /// moves the marker to the new commit.
    #[test]
    fn a_branch_switch_that_adds_a_source_file_indexes_it_before_the_marker_moves() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let mut repo = WatchedRepo::new(workspace, "trigger_on_branch_switch = true");
        let full_sync_at = crate::freshness::read_index_sync_marker(workspace).full_sync_at;

        run_git(workspace, &["checkout", "-q", "feature"]);
        repo.fire_git_trigger();

        assert!(repo.indexes("pkg/added.py"));
        let marker = crate::freshness::read_index_sync_marker(workspace);
        assert_eq!(marker.head.as_deref(), Some(repo.feature.as_str()));
        // Only a full sync walks every path, so its time stays.
        assert_eq!(marker.full_sync_at, full_sync_at);
        let freshness =
            crate::freshness::assess_index_freshness(workspace, &repo.store).expect("assess");
        assert_eq!(
            freshness.verdict,
            crate::freshness::FreshnessVerdict::Fresh,
            "{freshness:?}"
        );
        assert_eq!(freshness.head_matches, Some(true));
    }

    /// After a skipped switch the watcher's own base moves on, so a later reindex diffs
    /// from it and misses the skipped change: the marker must not claim that reindex.
    #[test]
    fn a_reindex_after_a_skipped_switch_does_not_claim_the_skipped_change() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let mut repo = WatchedRepo::new(workspace, "trigger_on_branch_switch = false");
        run_git(workspace, &["checkout", "-q", "feature"]);
        repo.fire_git_trigger();

        // A commit on the same branch is a pull-style advance, which is enabled.
        fs::write(
            workspace.join("pkg/more.py"),
            "def more(x):\n    return x\n",
        )
        .expect("write more");
        run_git(workspace, &["add", "pkg"]);
        run_git(workspace, &["commit", "-q", "-m", "more"]);
        repo.fire_git_trigger();

        assert!(
            repo.indexes("pkg/more.py"),
            "the diff from the watcher's base ran"
        );
        assert!(
            !repo.indexes("pkg/added.py"),
            "the skipped change stays unindexed"
        );
        let marker = crate::freshness::read_index_sync_marker(workspace);
        assert_eq!(marker.head.as_deref(), Some(repo.main.as_str()));
        let freshness =
            crate::freshness::assess_index_freshness(workspace, &repo.store).expect("assess");
        assert_eq!(freshness.head_matches, Some(false));
        assert!(
            freshness
                .changed_file_samples
                .contains(&"A pkg/added.py".to_owned()),
            "{freshness:?}"
        );
    }

    /// A full reindex whose walk can't read part of the workspace hasn't covered it, so the
    /// marker must keep its commit even though every path it did read succeeded.
    #[cfg(unix)]
    #[test]
    fn a_full_reindex_that_could_not_walk_everything_leaves_the_marker_unchanged() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let mut repo = WatchedRepo::new(workspace, "git_trigger_changed_files_only = false");
        let before = crate::freshness::read_index_sync_marker(workspace);
        let locked = workspace.join("pkg/locked");
        fs::create_dir_all(&locked).expect("create locked dir");
        fs::write(locked.join("hidden.py"), "def hidden():\n    return 0\n").expect("write");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("lock dir");
        if fs::read_dir(&locked).is_ok() {
            // Running as root: permissions can't make the walk fail here.
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).expect("unlock");
            return;
        }

        run_git(workspace, &["checkout", "-q", "feature"]);
        repo.fire_git_trigger();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).expect("unlock dir");

        assert!(
            repo.indexes("pkg/added.py"),
            "the readable paths were reindexed"
        );
        assert_eq!(crate::freshness::read_index_sync_marker(workspace), before);
    }

    /// A source file without symbols (a comment-only module, an empty `__init__.py`) has
    /// no row whose time a read could advance. Once the watcher has read it, it must stop
    /// counting as new.
    #[test]
    fn a_watched_source_file_without_symbols_stops_counting_as_new_once_read() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
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
        let config = mock_config(workspace);
        run_initial_index_once(&config).expect("index once");
        let _watcher_lock = crate::freshness::take_watcher_lock(workspace).expect("watcher lock");
        let mut observer = ObserverState::new(workspace.to_path_buf()).expect("observer");
        observer.seed_from_disk().expect("seed observer");
        let mut structural =
            StructuralIndexer::new(workspace.to_path_buf()).expect("structural indexer");
        let store = SqliteStore::open(workspace).expect("open store");
        let queue_state = SharedQueueState::new(HashMap::new());

        fs::write(
            workspace.join("pkg/notes.py"),
            "# Only a comment: no symbols.\n",
        )
        .expect("write notes");
        let before =
            crate::freshness::assess_index_freshness(workspace, &store).expect("assess before");
        assert_eq!(
            before.changed_file_samples,
            ["A pkg/notes.py"],
            "{before:?}"
        );

        process_debounced_paths(
            &config,
            &mut observer,
            &mut structural,
            &store,
            &queue_state,
            vec![workspace.join("pkg/notes.py")],
        );

        assert!(
            store
                .list_symbols_for_file("pkg/notes.py")
                .expect("list symbols")
                .is_empty()
        );
        let after =
            crate::freshness::assess_index_freshness(workspace, &store).expect("assess after");
        assert_eq!(after.new_files, 0, "{after:?}");
        assert_eq!(
            after.verdict,
            crate::freshness::FreshnessVerdict::Fresh,
            "{after:?}"
        );
    }
}
