//! Tests for the git-aware watcher: classifying events, the debounce, git triggers and
//! what each reindex records for freshness.

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
    crate::freshness::let_writes_settle();
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

    // The watcher's debounce lets writes settle before it reads.
    crate::freshness::let_writes_settle();
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
    let after = crate::freshness::assess_index_freshness(workspace, &store).expect("assess after");
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
        crate::freshness::let_writes_settle();
        run_initial_index_once(&config).expect("index once on main");
        // The daemon's state from before the branch switch.
        let runtime = WatcherRuntimeConfig::from_workspace(&config).expect("watcher runtime");
        let mut observer = ObserverState::new(workspace.to_path_buf()).expect("observer");
        observer.seed_from_disk().expect("seed observer");
        Self {
            _watcher_lock: crate::freshness::take_watcher_lock(workspace).expect("watcher lock"),
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
        // The loop fires only once the git event settles.
        crate::freshness::let_writes_settle();
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
    crate::freshness::let_writes_settle();
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

    // The watcher's debounce lets writes settle before it reads.
    crate::freshness::let_writes_settle();
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
    let after = crate::freshness::assess_index_freshness(workspace, &store).expect("assess after");
    assert_eq!(after.new_files, 0, "{after:?}");
    assert_eq!(
        after.verdict,
        crate::freshness::FreshnessVerdict::Fresh,
        "{after:?}"
    );
}

/// A checkout while a daemon's startup walk runs lands before the watches exist, so
/// no event reports it. The sync must keep the HEAD from before the walk as the
/// watcher's base, so that the next git trigger diffs from it and indexes the switch.
#[test]
fn a_head_change_during_the_startup_walk_is_covered_by_the_next_trigger() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    let mut repo = WatchedRepo::new(workspace, "trigger_on_branch_switch = true");

    // A daemon starts: its walk reads `main`, then HEAD moves to `feature` before the
    // snapshot is synced.
    let mut observer = ObserverState::new(workspace.to_path_buf()).expect("observer");
    observer.seed_from_disk().expect("seed observer");
    run_git(workspace, &["checkout", "-q", "feature"]);
    let (events, symbols_by_id, _) = collect_initial_snapshot(&observer);
    sync_snapshot(
        workspace,
        &repo.store,
        &mut repo.structural,
        &SnapshotScope::of(&observer),
        &events,
        &symbols_by_id,
        |_| Ok(()),
    )
    .expect("sync snapshot");

    assert_eq!(
        read_persisted_head_state(workspace)
            .expect("watcher base")
            .sha
            .as_deref(),
        Some(repo.main.as_str()),
        "the watcher's base is the commit the walk read"
    );
    assert_eq!(
        crate::freshness::read_index_sync_marker(workspace)
            .head
            .as_deref(),
        Some(repo.main.as_str())
    );
    assert!(!repo.indexes("pkg/added.py"));

    // The first git event after the watches exist covers the missed switch.
    repo.observer = observer;
    repo.fire_git_trigger();
    assert!(repo.indexes("pkg/added.py"));
    assert_eq!(
        crate::freshness::read_index_sync_marker(workspace)
            .head
            .as_deref(),
        Some(repo.feature.as_str())
    );
}

/// An indexed file that grows past the parse size limit is skipped, not read, so its
/// stored symbols may describe older text. The watcher must not count it as read.
#[test]
fn a_watched_file_skipped_for_its_size_still_reads_as_modified() {
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
    crate::freshness::let_writes_settle();
    run_initial_index_once(&config).expect("index once");
    let _watcher_lock = crate::freshness::take_watcher_lock(workspace).expect("watcher lock");
    let mut observer = ObserverState::new(workspace.to_path_buf()).expect("observer");
    observer.seed_from_disk().expect("seed observer");
    let mut structural =
        StructuralIndexer::new(workspace.to_path_buf()).expect("structural indexer");
    let store = SqliteStore::open(workspace).expect("open store");
    let queue_state = SharedQueueState::new(HashMap::new());
    let read_before = store.file_read_at_ms("pkg/a.py").expect("read time");

    // Past the 2 MiB parse limit: a new body, then enough comment lines.
    let mut grown = String::from("def alpha(x):\n    return x * 2\n");
    while grown.len() <= 2 * 1024 * 1024 {
        grown.push_str("# padding that pushes the file past the parse size limit\n");
    }
    fs::write(workspace.join("pkg/a.py"), grown).expect("grow a");
    crate::freshness::let_writes_settle();
    process_debounced_paths(
        &config,
        &mut observer,
        &mut structural,
        &store,
        &queue_state,
        vec![workspace.join("pkg/a.py")],
    );

    assert!(observer.skipped_as_oversized(&workspace.join("pkg/a.py")));
    assert_eq!(
        store.file_read_at_ms("pkg/a.py").expect("read time"),
        read_before,
        "a skipped file was not read"
    );
    let freshness = crate::freshness::assess_index_freshness(workspace, &store).expect("assess");
    assert_eq!(
        freshness.changed_file_samples,
        ["M pkg/a.py"],
        "{freshness:?}"
    );
}

/// A source file without symbols (imports and top-level code only) still has edges.
/// The watcher must extract them before it counts the file as read.
#[test]
fn a_watched_symbol_free_file_has_its_edges_indexed_before_it_counts_as_read() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
    fs::write(
        workspace.join(".aether/config.toml"),
        "[inference]\nprovider = \"mock\"\n\n[storage]\ngraph_backend = \"sqlite\"\n\n[embeddings]\nenabled = false\nvector_backend = \"sqlite\"\n",
    )
    .expect("write config");
    fs::create_dir_all(workspace.join("src")).expect("create src");
    fs::write(
        workspace.join("src/a.ts"),
        "export function alpha(): number {\n  return 1;\n}\n",
    )
    .expect("write a");
    let config = mock_config(workspace);
    crate::freshness::let_writes_settle();
    run_initial_index_once(&config).expect("index once");
    let _watcher_lock = crate::freshness::take_watcher_lock(workspace).expect("watcher lock");
    let mut observer = ObserverState::new(workspace.to_path_buf()).expect("observer");
    observer.seed_from_disk().expect("seed observer");
    let mut structural =
        StructuralIndexer::new(workspace.to_path_buf()).expect("structural indexer");
    let store = SqliteStore::open(workspace).expect("open store");
    let queue_state = SharedQueueState::new(HashMap::new());
    let edges_of = |file_path: &str| -> i64 {
        rusqlite::Connection::open(workspace.join(".aether/meta.sqlite"))
            .and_then(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM symbol_edges WHERE file_path = ?1",
                    [file_path],
                    |row| row.get(0),
                )
            })
            .expect("count edges")
    };

    fs::write(
        workspace.join("src/main.ts"),
        "import { alpha } from \"./a\";\n\nalpha();\n",
    )
    .expect("write main");
    crate::freshness::let_writes_settle();
    process_debounced_paths(
        &config,
        &mut observer,
        &mut structural,
        &store,
        &queue_state,
        vec![workspace.join("src/main.ts")],
    );

    assert!(
        store
            .list_symbols_for_file("src/main.ts")
            .expect("list symbols")
            .is_empty()
    );
    assert!(edges_of("src/main.ts") > 0, "the file's edges are indexed");
    let freshness = crate::freshness::assess_index_freshness(workspace, &store).expect("assess");
    assert_eq!(freshness.new_files, 0, "{freshness:?}");
}

/// A workspace whose watcher has indexed `src/main.ts`, a source file with imports and
/// a top-level call but no symbols.
struct SymbolFreeFile {
    config: IndexerConfig,
    observer: ObserverState,
    structural: StructuralIndexer,
    store: SqliteStore,
    queue_state: SharedQueueState,
    _watcher_lock: crate::freshness::WatcherLock,
}

impl SymbolFreeFile {
    fn new(workspace: &Path) -> Self {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            "[inference]\nprovider = \"mock\"\n\n[storage]\ngraph_backend = \"sqlite\"\n\n[embeddings]\nenabled = false\nvector_backend = \"sqlite\"\n",
        )
        .expect("write config");
        fs::create_dir_all(workspace.join("src")).expect("create src");
        fs::write(
            workspace.join("src/a.ts"),
            "export function alpha(): number {\n  return 1;\n}\n",
        )
        .expect("write a");
        let config = mock_config(workspace);
        crate::freshness::let_writes_settle();
        run_initial_index_once(&config).expect("index once");
        let mut repo = Self {
            _watcher_lock: crate::freshness::take_watcher_lock(workspace).expect("watcher lock"),
            observer: {
                let mut observer = ObserverState::new(workspace.to_path_buf()).expect("observer");
                observer.seed_from_disk().expect("seed observer");
                observer
            },
            structural: StructuralIndexer::new(workspace.to_path_buf())
                .expect("structural indexer"),
            store: SqliteStore::open(workspace).expect("open store"),
            queue_state: SharedQueueState::new(HashMap::new()),
            config,
        };
        fs::write(
            workspace.join("src/main.ts"),
            "import { alpha } from \"./a\";\n\nalpha();\n",
        )
        .expect("write main");
        repo.watch("src/main.ts");
        assert!(repo.edges("src/main.ts") > 0);
        repo
    }

    fn watch(&mut self, file_path: &str) {
        crate::freshness::let_writes_settle();
        let path = self.config.workspace.join(file_path);
        process_debounced_paths(
            &self.config,
            &mut self.observer,
            &mut self.structural,
            &self.store,
            &self.queue_state,
            vec![path],
        );
    }

    fn edges(&self, file_path: &str) -> i64 {
        rusqlite::Connection::open(self.config.workspace.join(".aether/meta.sqlite"))
            .and_then(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM symbol_edges WHERE file_path = ?1",
                    [file_path],
                    |row| row.get(0),
                )
            })
            .expect("count edges")
    }
}

/// Deleting a symbol-free file removes no symbol, so no symbol event clears what it
/// left: the watcher must clear its edges itself and forget its read.
#[test]
fn a_watched_deletion_of_a_symbol_free_file_clears_its_edges() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    let mut repo = SymbolFreeFile::new(workspace);

    fs::remove_file(workspace.join("src/main.ts")).expect("delete main");
    repo.watch("src/main.ts");

    assert_eq!(repo.edges("src/main.ts"), 0);
    assert_eq!(
        repo.store.file_read_at_ms("src/main.ts").expect("read"),
        None
    );
    let freshness =
        crate::freshness::assess_index_freshness(workspace, &repo.store).expect("assess");
    assert_eq!(
        freshness.verdict,
        crate::freshness::FreshnessVerdict::Fresh,
        "{freshness:?}"
    );
}

/// Deleted while nothing watched, the file's edges stay stored, so status must call the
/// index stale until a full sync clears them.
#[test]
fn an_unwatched_deletion_of_a_symbol_free_file_is_stale_until_a_full_sync() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    let repo = SymbolFreeFile::new(workspace);

    fs::remove_file(workspace.join("src/main.ts")).expect("delete main");
    let stale = crate::freshness::assess_index_freshness(workspace, &repo.store).expect("assess");
    assert_eq!(stale.changed_file_samples, ["D src/main.ts"], "{stale:?}");
    assert_eq!(stale.verdict, crate::freshness::FreshnessVerdict::Stale);

    let SymbolFreeFile {
        config,
        store,
        _watcher_lock,
        ..
    } = repo;
    drop(_watcher_lock);
    run_initial_index_once(&config).expect("full sync");
    let edges = rusqlite::Connection::open(workspace.join(".aether/meta.sqlite"))
        .and_then(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM symbol_edges WHERE file_path = 'src/main.ts'",
                [],
                |row| row.get::<_, i64>(0),
            )
        })
        .expect("count edges");
    assert_eq!(edges, 0, "the full sync cleared the deleted file's edges");
    assert_eq!(store.file_read_at_ms("src/main.ts").expect("read"), None);
    let fresh = crate::freshness::assess_index_freshness(workspace, &store).expect("assess");
    assert_eq!(fresh.deleted_files, 0, "{fresh:?}");
}

/// An index of symbol-free files alone has no symbols, yet holds their edges: deleting
/// one while nothing watches must read as stale, not as an empty index.
#[test]
fn a_deletion_in_an_index_of_symbol_free_files_alone_reads_as_stale() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
    fs::write(
        workspace.join(".aether/config.toml"),
        "[inference]\nprovider = \"mock\"\n\n[storage]\ngraph_backend = \"sqlite\"\n\n[embeddings]\nenabled = false\nvector_backend = \"sqlite\"\n",
    )
    .expect("write config");
    let config = mock_config(workspace);
    run_initial_index_once(&config).expect("index once");
    let mut observer = ObserverState::new(workspace.to_path_buf()).expect("observer");
    observer.seed_from_disk().expect("seed observer");
    let mut structural =
        StructuralIndexer::new(workspace.to_path_buf()).expect("structural indexer");
    let store = SqliteStore::open(workspace).expect("open store");
    let queue_state = SharedQueueState::new(HashMap::new());

    fs::create_dir_all(workspace.join("src")).expect("create src");
    fs::write(
        workspace.join("src/main.ts"),
        "import { alpha } from \"./a\";\n\nalpha();\n",
    )
    .expect("write main");
    crate::freshness::let_writes_settle();
    process_debounced_paths(
        &config,
        &mut observer,
        &mut structural,
        &store,
        &queue_state,
        vec![workspace.join("src/main.ts")],
    );
    assert!(
        store
            .file_read_at_ms("src/main.ts")
            .expect("read")
            .is_some()
    );
    assert!(
        store
            .list_indexed_files()
            .expect("indexed files")
            .is_empty()
    );

    fs::remove_file(workspace.join("src/main.ts")).expect("delete main");
    let freshness = crate::freshness::assess_index_freshness(workspace, &store).expect("assess");
    assert_eq!(
        freshness.verdict,
        crate::freshness::FreshnessVerdict::Stale,
        "{freshness:?}"
    );
    assert_eq!(freshness.changed_file_samples, ["D src/main.ts"]);
}

/// An index of symbol-free files alone has no symbols, yet holds their edges: editing one
/// while nothing watches must read as modified, not leave the index looking empty.
#[test]
fn an_unwatched_edit_in_an_index_of_symbol_free_files_alone_reads_as_modified() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
    fs::write(
        workspace.join(".aether/config.toml"),
        "[inference]\nprovider = \"mock\"\n\n[storage]\ngraph_backend = \"sqlite\"\n\n[embeddings]\nenabled = false\nvector_backend = \"sqlite\"\n",
    )
    .expect("write config");
    let config = mock_config(workspace);
    run_initial_index_once(&config).expect("index once");
    let mut observer = ObserverState::new(workspace.to_path_buf()).expect("observer");
    observer.seed_from_disk().expect("seed observer");
    let mut structural =
        StructuralIndexer::new(workspace.to_path_buf()).expect("structural indexer");
    let store = SqliteStore::open(workspace).expect("open store");
    let queue_state = SharedQueueState::new(HashMap::new());

    fs::create_dir_all(workspace.join("src")).expect("create src");
    fs::write(
        workspace.join("src/main.ts"),
        "import { alpha } from \"./a\";\n\nalpha();\n",
    )
    .expect("write main");
    crate::freshness::let_writes_settle();
    process_debounced_paths(
        &config,
        &mut observer,
        &mut structural,
        &store,
        &queue_state,
        vec![workspace.join("src/main.ts")],
    );
    assert!(
        store
            .list_indexed_files()
            .expect("indexed files")
            .is_empty()
    );
    let read = crate::freshness::assess_index_freshness(workspace, &store).expect("assess");
    assert_eq!(
        read.verdict,
        crate::freshness::FreshnessVerdict::Empty,
        "{read:?}"
    );

    // Edited after the watcher stopped: the stored edges are of the old text.
    crate::freshness::let_writes_settle();
    fs::write(
        workspace.join("src/main.ts"),
        "import { beta } from \"./b\";\n\nbeta();\n",
    )
    .expect("edit main");
    let freshness = crate::freshness::assess_index_freshness(workspace, &store).expect("assess");
    assert_eq!(
        freshness.verdict,
        crate::freshness::FreshnessVerdict::Stale,
        "{freshness:?}"
    );
    assert_eq!(freshness.changed_file_samples, ["M src/main.ts"]);
}
