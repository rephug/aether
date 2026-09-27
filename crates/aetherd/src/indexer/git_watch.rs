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
    let previous_head = read_persisted_head_state(config.workspace.as_path())?;
    let current_head = read_current_head_state(config.workspace.as_path(), git_watch_dir)?;
    let mut paths = dirty_paths.into_iter().collect::<BTreeSet<_>>();

    if previous_head.sha != current_head.sha {
        let trigger_kind =
            classify_git_trigger_kind(config.workspace.as_path(), &previous_head, &current_head);
        if trigger_kind.is_some_and(|kind| git_trigger_enabled(&watcher_runtime.watcher, kind)) {
            let git_paths = if !watcher_runtime.watcher.git_trigger_changed_files_only
                || previous_head.sha.is_none()
            {
                collect_full_reindex_paths(config.workspace.as_path(), observer)
            } else if let (Some(old_sha), Some(new_sha)) =
                (previous_head.sha.as_deref(), current_head.sha.as_deref())
            {
                match changed_paths_between_heads(config.workspace.as_path(), old_sha, new_sha) {
                    Ok(paths) => paths,
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
    for path in paths {
        process_reindex_path(config, observer, structural, store, queue_state, &path)?;
    }
    Ok(())
}

pub(super) fn process_reindex_path(
    config: &IndexerConfig,
    observer: &mut ObserverState,
    structural: &mut StructuralIndexer,
    store: &SqliteStore,
    queue_state: &SharedQueueState,
    path: &Path,
) -> Result<()> {
    match observer.process_path(path) {
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
        }
        Ok(None) => {}
        Err(err) => {
            return Err(err).with_context(|| format!("failed to process {}", path.display()));
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

pub(super) fn collect_full_reindex_paths(
    workspace: &Path,
    observer: &ObserverState,
) -> Vec<PathBuf> {
    let mut paths = observer
        .tracked_paths()
        .into_iter()
        .collect::<BTreeSet<_>>();
    for entry in WalkBuilder::new(workspace).standard_filters(true).build() {
        let Ok(entry) = entry else {
            continue;
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
    paths.into_iter().collect()
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
}
