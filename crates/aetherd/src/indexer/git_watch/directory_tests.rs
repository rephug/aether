use super::*;
use tempfile::tempdir;

#[test]
fn moved_in_tree_registers_nested_watches_and_existing_sources() {
    let temp = tempdir().expect("workspace");
    let workspace = temp.path();
    let incoming = workspace.join("incoming");
    fs::create_dir_all(incoming.join("nested")).expect("nested source directory");
    fs::write(incoming.join("nested/lib.rs"), "pub fn moved() {}\n").expect("source");
    fs::create_dir_all(workspace.join(".git")).expect("git marker");
    fs::write(workspace.join(".gitignore"), "incoming/skip.rs\n").expect("ignore rule");
    fs::write(incoming.join("skip.rs"), "fn ignored_by_git() {}\n").expect("ignored source");
    fs::create_dir_all(incoming.join("target/deep")).expect("ignored directory");
    fs::write(incoming.join("target/deep/ignored.rs"), "fn ignored() {}\n")
        .expect("ignored source");
    let classified = classify_watch_event(
        workspace,
        None,
        Event {
            kind: notify::EventKind::Create(notify::event::CreateKind::Folder),
            paths: vec![incoming.clone()],
            attrs: Default::default(),
        },
    );
    let watches = classified.watch_dirs.into_iter().collect::<BTreeSet<_>>();
    assert_eq!(
        watches,
        BTreeSet::from([incoming.clone(), incoming.join("nested")])
    );
    let sources = classified.source_paths.into_iter().collect::<BTreeSet<_>>();
    assert_eq!(
        sources,
        BTreeSet::from([incoming.clone(), incoming.join("nested/lib.rs")])
    );
}

#[test]
fn removed_directory_includes_prior_symbol_free_files_without_prefix_siblings() {
    let temp = tempdir().expect("workspace");
    let workspace = temp.path();
    let removed = workspace.join("pkg");
    let symbol_file = removed.join("nested/model.py");
    let imports_only = removed.join("__init__.py");
    let sibling = workspace.join("pkg2/keep.py");
    let expanded = expand_known_descendants(
        vec![removed.clone(), symbol_file.clone()],
        [symbol_file.clone(), imports_only.clone(), sibling],
    );
    assert_eq!(
        expanded.into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([removed, symbol_file, imports_only])
    );
}

#[test]
fn ordinary_existing_file_event_does_not_expand_into_unrelated_files() {
    let temp = tempdir().expect("workspace");
    let path = temp.path().join("lib.rs");
    fs::write(&path, "pub fn edited() {}\n").expect("source");
    assert_eq!(
        expand_known_descendants(vec![path.clone()], [temp.path().join("other.rs")]),
        vec![path]
    );
}
