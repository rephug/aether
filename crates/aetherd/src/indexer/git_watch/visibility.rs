//! Apply the same ignore rules as the startup walk to an individual live path.
//! WalkBuilder deliberately does not filter its root, so walking an event directory
//! directly is insufficient: an ignored generated directory becomes a new root.
use ignore::WalkBuilder;
use std::path::Path;

/// Check visibility from the workspace root, pruning every unrelated subtree.
/// This reuses ignore's ordering for .ignore, nested .gitignore, global excludes,
/// repository boundaries and hidden paths rather than maintaining a second matcher.
/// Rules are reread each time, so edits to ignore files cannot leave a stale cache.
pub(super) fn is_visible(workspace: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(workspace) else {
        return false;
    };
    if relative.as_os_str().is_empty() {
        return true;
    }
    // Missing paths must reach reconciliation to retire old catalog rows. They
    // cannot introduce generated source or incur a new inference call.
    if !path.exists() {
        return true;
    }
    // Startup only indexes real files/directories and does not follow symlinks.
    if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return false;
    }
    let target = path.to_path_buf();
    WalkBuilder::new(workspace)
        .standard_filters(true)
        .max_depth(Some(relative.components().count()))
        .filter_entry(move |entry| target.starts_with(entry.path()))
        .build()
        .filter_map(Result::ok)
        .any(|entry| entry.path() == path)
}

/// Evaluate a batch with one ancestry-pruned walk, rather than listing the same
/// parent directory for every source during overflow recovery or a git update.
pub(super) fn retain_visible_paths(
    workspace: &Path,
    paths: Vec<std::path::PathBuf>,
) -> Result<Vec<std::path::PathBuf>, ignore::Error> {
    use std::collections::HashSet;
    if paths.is_empty() {
        return Ok(paths);
    }
    let mut ancestry = HashSet::new();
    for path in &paths {
        if path.starts_with(workspace) {
            for parent in path.ancestors() {
                if !parent.starts_with(workspace) {
                    break;
                }
                ancestry.insert(parent.to_path_buf());
            }
        }
    }
    let visible: HashSet<_> = WalkBuilder::new(workspace)
        .standard_filters(true)
        .filter_entry(move |entry| ancestry.contains(entry.path()))
        .build()
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|entry| {
            entry
                .file_type()
                .is_some_and(|kind| kind.is_file() || kind.is_dir())
        })
        .map(|entry| entry.into_path())
        .collect();
    Ok(paths
        .into_iter()
        .filter(|path| path.starts_with(workspace) && (!path.exists() || visible.contains(path)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn visibility_matches_seed_for_nested_rules_and_ignored_event_root() {
        let temp = tempdir().expect("temp");
        let root = temp.path();
        fs::create_dir_all(root.join(".git/info")).expect("git");
        fs::create_dir_all(root.join("lib")).expect("lib");
        fs::create_dir_all(root.join("src")).expect("src");
        fs::write(root.join(".gitignore"), "lib/\n*.generated.js\n").expect("ignore");
        fs::write(root.join("src/.gitignore"), "!keep.generated.js\n").expect("nested");
        fs::write(root.join("src/.ignore"), "private.js\n").expect("ignore");
        fs::write(root.join(".git/info/exclude"), "excluded.js\n").expect("exclude");
        for file in [
            "lib/index.js",
            "src/no.generated.js",
            "src/keep.generated.js",
            "src/private.js",
            "src/excluded.js",
            "src/main.js",
        ] {
            fs::write(root.join(file), "function test() {}").expect("source");
        }
        assert!(!is_visible(root, &root.join("lib")));
        assert!(!is_visible(root, &root.join("lib/index.js")));
        assert!(!is_visible(root, &root.join("src/no.generated.js")));
        assert!(is_visible(root, &root.join("src/keep.generated.js")));
        assert!(!is_visible(root, &root.join("src/private.js")));
        assert!(!is_visible(root, &root.join("src/excluded.js")));
        assert!(is_visible(root, &root.join("src/main.js")));
        fs::write(root.join(".gitignore"), "").expect("change rules");
        assert!(is_visible(root, &root.join("lib/index.js")));
    }

    #[test]
    fn deleted_source_reaches_reconciliation_and_outside_paths_do_not() {
        let temp = tempdir().expect("temp");
        assert!(is_visible(temp.path(), &temp.path().join("deleted.rs")));
        assert!(!is_visible(temp.path(), Path::new("/outside.rs")));
    }
}

#[cfg(all(test, unix))]
mod symlink_tests {
    use super::*;
    #[test]
    fn source_symlink_is_not_visible() {
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("real.rs");
        std::fs::write(&source, "fn real() {}").expect("source");
        let link = temp.path().join("link.rs");
        std::os::unix::fs::symlink(&source, &link).expect("symlink");
        assert!(!is_visible(temp.path(), &link));
        assert!(is_visible(temp.path(), &source));
    }
}

#[cfg(test)]
mod batch_tests {
    use super::*;
    #[test]
    fn unreadable_workspace_is_not_silently_accepted() {
        let temp = tempfile::tempdir().expect("temp");
        let missing = temp.path().join("missing");
        assert!(retain_visible_paths(&missing, vec![missing.join("source.rs")]).is_err());
    }

    #[test]
    fn batched_visibility_matches_single_checks_and_keeps_deletions() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        std::fs::create_dir_all(root.join(".git")).expect("git");
        std::fs::create_dir_all(root.join("src")).expect("src");
        std::fs::write(root.join(".gitignore"), "*.generated.rs\n").expect("ignore");
        let mut paths = Vec::new();
        for index in 0..64 {
            let file = root.join(format!("src/file{index}.rs"));
            std::fs::write(&file, "fn test() {}").expect("file");
            paths.push(file);
        }
        let ignored = root.join("src/x.generated.rs");
        std::fs::write(&ignored, "fn generated() {}").expect("generated");
        paths.push(ignored);
        paths.push(root.join("deleted.rs"));
        let expected: Vec<_> = paths
            .iter()
            .filter(|path| is_visible(root, path))
            .cloned()
            .collect();
        assert_eq!(
            retain_visible_paths(root, paths).expect("visibility walk"),
            expected
        );
    }
}
