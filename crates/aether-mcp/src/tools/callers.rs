//! Which call edges really reach a selected symbol.
//!
//! Rust qualified names usually carry no module path (only inline `mod` blocks add
//! one), and the Rust parser records method calls such as `x.alpha()` with the bare
//! method name as their target. A call edge's target can therefore name several
//! symbols: same-named methods of different types, a free function, or the same type
//! declared twice. A caller counts for the selected symbol only when the edge's target
//! narrows to that one symbol or to its overloads. TypeScript records calls by their
//! callee text (`obj.alpha`, `this.alpha`) and Python by the callee's qualified name, so
//! the bare-name expansion applies to Rust only.
//!
//! Overloads share a qualified name, language and file but not an ID: Java and C#
//! overloaded methods (`Type::method`), or a Rust method of a trait a type implements
//! twice (`impl From<u8> for Wrap` and `impl From<u16> for Wrap` both declare
//! `Wrap::from`). A call edge can't tell them apart, so a call to that name counts for
//! every overload. Same-named symbols in different files stay ambiguous, C# partial
//! classes included, since neither Java nor C# puts the package or namespace in the
//! qualified name.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use aether_store::{SqliteStore, SymbolRecord, SymbolRelationStore};

use super::common::symbol_leaf_name;
use crate::AetherMcpError;

/// Files whose directory is the root of a package.
const PACKAGE_MANIFESTS: [&str; 3] = ["Cargo.toml", "package.json", "pyproject.toml"];

pub(crate) struct CallerAttribution<'a> {
    store: &'a SqliteStore,
    packages: PackageRoots<'a>,
    candidates_by_target: HashMap<String, Vec<SymbolRecord>>,
    records_by_id: HashMap<String, Option<SymbolRecord>>,
}

impl<'a> CallerAttribution<'a> {
    pub(crate) fn new(store: &'a SqliteStore, workspace_root: &'a Path) -> Self {
        Self {
            store,
            packages: PackageRoots::new(workspace_root),
            candidates_by_target: HashMap::new(),
            records_by_id: HashMap::new(),
        }
    }

    /// The callers whose call edges resolve to `symbol` or its overloads alone, ordered
    /// by qualified name and ID.
    ///
    /// An edge counts when its target is the symbol's qualified name or, for a Rust
    /// method, its bare name. The symbols that target could name (in the caller's
    /// language family) are narrowed to the caller's own file; if none are there, to
    /// the caller's package; otherwise all are kept. The caller is attributed only when
    /// every remaining candidate is `symbol` or one of its overloads.
    pub(crate) fn callers_of(
        &mut self,
        symbol: &SymbolRecord,
    ) -> Result<Vec<SymbolRecord>, AetherMcpError> {
        let mut targets = vec![symbol.qualified_name.clone()];
        let leaf = symbol_leaf_name(symbol.qualified_name.as_str());
        if symbol.language == "rust"
            && leaf != symbol.qualified_name
            && matches!(symbol.kind.as_str(), "method" | "function")
        {
            targets.push(leaf.to_owned());
        }

        let mut callers = HashMap::<String, SymbolRecord>::new();
        for target in &targets {
            for edge in SymbolRelationStore::get_callers(self.store, target)? {
                if callers.contains_key(&edge.source_id) {
                    continue;
                }
                let Some(caller) = self.record(edge.source_id.as_str())? else {
                    continue;
                };
                if self.resolves_to(&caller, edge.target_qualified_name.as_str(), symbol)? {
                    callers.insert(caller.id.clone(), caller);
                }
            }
        }

        let mut callers = callers.into_values().collect::<Vec<_>>();
        callers.sort_by(|left, right| {
            left.qualified_name
                .cmp(&right.qualified_name)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(callers)
    }

    fn resolves_to(
        &mut self,
        caller: &SymbolRecord,
        target: &str,
        symbol: &SymbolRecord,
    ) -> Result<bool, AetherMcpError> {
        // Only Rust records a call by the bare name of a member; elsewhere a target
        // names exactly the symbol with that qualified name.
        let bare_member_calls = caller.language == "rust";
        let caller_family = language_family(caller.language.as_str());
        let candidates = self
            .candidates(target)?
            .iter()
            .filter(|candidate| {
                language_family(candidate.language.as_str()) == caller_family
                    && (bare_member_calls || candidate.qualified_name == target)
            })
            .cloned()
            .collect::<Vec<_>>();

        let in_caller_file = candidates
            .iter()
            .filter(|candidate| candidate.file_path == caller.file_path)
            .collect::<Vec<_>>();
        let narrowed = if !in_caller_file.is_empty() {
            in_caller_file
        } else {
            let caller_package = self.packages.package_of(caller.file_path.as_str());
            let in_caller_package = match caller_package {
                Some(package) => candidates
                    .iter()
                    .filter(|candidate| {
                        self.packages
                            .package_of(candidate.file_path.as_str())
                            .as_ref()
                            == Some(&package)
                    })
                    .collect::<Vec<_>>(),
                None => Vec::new(),
            };
            if in_caller_package.is_empty() {
                candidates.iter().collect()
            } else {
                in_caller_package
            }
        };

        Ok(!narrowed.is_empty()
            && narrowed
                .iter()
                .all(|candidate| is_same_or_overload(candidate, symbol)))
    }

    fn candidates(&mut self, target: &str) -> Result<&Vec<SymbolRecord>, AetherMcpError> {
        if !self.candidates_by_target.contains_key(target) {
            let candidates = self.store.list_symbols_for_call_target(target)?;
            self.candidates_by_target
                .insert(target.to_owned(), candidates);
        }
        Ok(&self.candidates_by_target[target])
    }

    fn record(&mut self, symbol_id: &str) -> Result<Option<SymbolRecord>, AetherMcpError> {
        if let Some(record) = self.records_by_id.get(symbol_id) {
            return Ok(record.clone());
        }
        let record = self.store.get_symbol_record(symbol_id)?;
        self.records_by_id
            .insert(symbol_id.to_owned(), record.clone());
        Ok(record)
    }
}

/// Whether `candidate` is `symbol` or an overload of it: a symbol with the same
/// qualified name and language declared in the same file.
fn is_same_or_overload(candidate: &SymbolRecord, symbol: &SymbolRecord) -> bool {
    candidate.id == symbol.id
        || (candidate.qualified_name == symbol.qualified_name
            && candidate.language == symbol.language
            && candidate.file_path == symbol.file_path)
}

/// Languages that can call each other's symbols share a family.
fn language_family(language: &str) -> &str {
    match language {
        "typescript" | "tsx" | "javascript" | "jsx" => "ecmascript",
        other => other,
    }
}

/// Maps workspace-relative files to their package: the nearest directory from the
/// file's own up to the workspace root that holds a package manifest, or the
/// workspace root when none does.
struct PackageRoots<'a> {
    workspace_root: &'a Path,
    by_dir: HashMap<PathBuf, PathBuf>,
}

impl<'a> PackageRoots<'a> {
    fn new(workspace_root: &'a Path) -> Self {
        Self {
            workspace_root,
            by_dir: HashMap::new(),
        }
    }

    /// The workspace-relative package directory of `file_path`, or `None` when the
    /// path isn't a plain relative path inside the workspace.
    fn package_of(&mut self, file_path: &str) -> Option<PathBuf> {
        let file_path = Path::new(file_path);
        if !file_path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        {
            return None;
        }
        let dir = file_path.parent().unwrap_or(Path::new("")).to_path_buf();
        if let Some(package) = self.by_dir.get(&dir) {
            return Some(package.clone());
        }
        let package = dir
            .ancestors()
            .find(|ancestor| {
                PACKAGE_MANIFESTS
                    .iter()
                    .any(|manifest| self.workspace_root.join(ancestor).join(manifest).is_file())
            })
            .unwrap_or(Path::new(""))
            .to_path_buf();
        self.by_dir.insert(dir, package.clone());
        Some(package)
    }
}
