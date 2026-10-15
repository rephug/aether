//! The module a source file belongs to.

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use crate::registry::default_registry;

/// Maps workspace-relative source files to the root of their module (a Cargo crate, an
/// npm package, a Maven or Gradle module, a .NET project, a Go module or a Python
/// package): the nearest directory, from the file's own up to the workspace root, that
/// holds a module marker of any language the parser knows
/// ([`LanguageConfig::module_markers`](crate::LanguageConfig::module_markers)), or the
/// workspace root when none does. Markers of every language count, so a script inside a
/// Rust crate belongs to that crate.
pub struct ModuleRoots {
    workspace_root: PathBuf,
    /// Module root by file directory.
    by_dir: HashMap<PathBuf, PathBuf>,
    /// Whether a directory holds a module marker.
    is_root: HashMap<PathBuf, bool>,
}

impl ModuleRoots {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            by_dir: HashMap::new(),
            is_root: HashMap::new(),
        }
    }

    /// The workspace-relative module root of `file_path`, or `None` when the path isn't
    /// a plain relative path inside the workspace.
    pub fn module_of(&mut self, file_path: &str) -> Option<PathBuf> {
        let file_path = Path::new(file_path);
        if !file_path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        {
            return None;
        }
        let dir = file_path.parent().unwrap_or(Path::new("")).to_path_buf();
        if let Some(root) = self.by_dir.get(&dir) {
            return Some(root.clone());
        }
        let root = dir
            .ancestors()
            .find(|ancestor| self.is_module_root(ancestor))
            .unwrap_or(Path::new(""))
            .to_path_buf();
        self.by_dir.insert(dir, root.clone());
        Some(root)
    }

    fn is_module_root(&mut self, dir: &Path) -> bool {
        if let Some(&is_root) = self.is_root.get(dir) {
            return is_root;
        }
        let is_root = holds_marker(&self.workspace_root.join(dir), module_markers());
        self.is_root.insert(dir.to_path_buf(), is_root);
        is_root
    }
}

/// Every language's module markers, read from the parser registry once per process,
/// with the file names, checked by a single lookup each, before the patterns, which
/// read the directory.
fn module_markers() -> &'static [&'static str] {
    static MARKERS: OnceLock<Vec<&'static str>> = OnceLock::new();
    MARKERS.get_or_init(|| {
        let mut markers = default_registry()
            .configs()
            .iter()
            .flat_map(|config| config.module_markers.iter().copied())
            .collect::<Vec<_>>();
        markers.sort_unstable_by_key(|marker| (marker.starts_with('*'), *marker));
        markers.dedup();
        markers
    })
}

/// Whether `dir` holds a file named by one of `markers`. A marker `*<suffix>` names
/// any file whose name ends in `<suffix>`.
fn holds_marker(dir: &Path, markers: &[&str]) -> bool {
    markers.iter().any(|marker| match marker.strip_prefix('*') {
        Some(suffix) => fs::read_dir(dir).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.len() > suffix.len() && name.ends_with(suffix) && entry.path().is_file()
            })
        }),
        None => dir.join(marker).is_file(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, relative: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
        fs::write(path, "").expect("write file");
    }

    #[test]
    fn modules_follow_the_build_files_of_every_language() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        for marker in [
            "pom.xml",
            "billing/pom.xml",
            "orders/build.gradle.kts",
            "Directory.Build.props",
            "src/Api/Api.csproj",
            "src/Core/Core.csproj",
            "Cargo.toml",
            "crates/store/Cargo.toml",
            "web/package.json",
            "tools/pyproject.toml",
            "tools/pkg/__init__.py",
            "svc/go.mod",
        ] {
            write(root, marker);
        }
        let mut modules = ModuleRoots::new(root);
        let mut module_of = |file: &str| {
            modules
                .module_of(file)
                .map(|module| module.to_string_lossy().into_owned())
        };

        for (file, module) in [
            ("billing/src/main/java/com/acme/Invoice.java", "billing"),
            ("orders/src/main/java/com/acme/Order.java", "orders"),
            ("src/main/java/com/acme/App.java", ""),
            ("src/Api/Controllers/UsersController.cs", "src/Api"),
            ("src/Core/Users.cs", "src/Core"),
            ("src/Tools/Script.cs", ""),
            ("crates/store/src/lib.rs", "crates/store"),
            ("src/main.rs", ""),
            ("web/src/app.ts", "web"),
            ("web/src/view.jsx", "web"),
            ("tools/pkg/config.py", "tools/pkg"),
            ("tools/cli.py", "tools"),
            ("svc/internal/config/config.go", "svc"),
        ] {
            assert_eq!(module_of(file).as_deref(), Some(module), "{file}");
        }

        // Markers of every language count: a script in a Rust crate belongs to the crate,
        // and so does a file whose language the parser doesn't know.
        let mut modules = ModuleRoots::new(root);
        assert_eq!(
            modules.module_of("crates/store/static/app.js"),
            Some(PathBuf::from("crates/store"))
        );
        assert_eq!(
            modules.module_of("src/Api/notes.txt"),
            Some(PathBuf::from("src/Api"))
        );
        assert_eq!(modules.module_of("../outside.rs"), None);
        assert_eq!(modules.module_of("/abs/path.rs"), None);
    }
}
