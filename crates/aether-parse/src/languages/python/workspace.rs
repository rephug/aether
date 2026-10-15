//! Python call targets resolved through the workspace's other files.
//!
//! A file alone can't always tell where a name it imports is defined:
//! - `from pkg import f`, where `pkg/__init__.py` re-exports `f` from a submodule
//!   (`from .impl import f`, or `from .impl import *`), targets `pkg::f`, while the symbol is
//!   `pkg.impl::f`;
//! - in a `src/` layout, an absolute import from outside `src/` (from `tests/`, or from a
//!   sibling package under `src/`) targets `pkg.mod::f`, while the symbol is
//!   `src.pkg.mod::f`.
//!
//! [`PythonWorkspace`] reads the modules a call target names from disk and follows the target
//! through their imports to the module that defines it, so that the edge names the defining
//! symbol. Symbol ids and qualified names don't change, only call targets. Where Python could
//! bind the name more than one way, the target keeps its spelling: a wrong edge is worse than
//! a missing one.
//!
//! It remembers what each extraction read (directory listings and modules) and which names it
//! looked up in them, so that a caller who sees a file change can re-extract just the files
//! whose targets the change may move.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aether_core::{EdgeKind, SymbolEdge, normalize_path};
use tree_sitter::{Node, Parser};

use super::{
    Binding, FileContext, Resolver, STATEMENT_CONTAINERS, module_segments, node_text,
    python_directory_segments, python_module_path,
};

/// Files larger than this aren't indexed (the observer skips them), so none of their symbols
/// can be a target.
const MAX_MODULE_FILE_SIZE: u64 = 2 * 1024 * 1024;

/// How many imports one target is followed through, which bounds import cycles.
const MAX_HOPS: usize = 32;

/// How a module binds a name at its top level, as another module importing it sees it.
#[derive(Debug, PartialEq, Eq)]
enum Export {
    /// A `def` or `class` of the module: a target that reaches it is spelled with this module.
    Defined,
    /// `from module import name [as alias]`, from a module outside the standard library.
    Imported { module: String, name: String },
    /// Anything else: an assignment, a module (`import pkg.mod as mod`), a standard-library
    /// name, a name a function rebinds through `global`, or a name bound in more than one
    /// way.
    Opaque,
}

/// What `from module import *` takes, as the module's `__all__` says.
#[derive(Debug, PartialEq, Eq)]
enum All {
    /// No `__all__`: the public names.
    Absent,
    /// Every name of the list and tuple literals assigned or added to `__all__`, which
    /// nothing else in the file touches. A name in only one branch of an `if` counts.
    Literal(BTreeSet<String>),
    /// Built or changed some other way (`__all__.append(…)`, `__all__ = other.__all__`).
    Dynamic,
}

/// What a module binds at its top level.
#[derive(Debug, PartialEq, Eq)]
struct ModuleSummary {
    names: BTreeMap<String, Export>,
    /// The modules `from module import *` takes names from. `None` for the standard library
    /// and for a module the file can't spell (a relative import past the top-level package),
    /// which may hold any name.
    star_imports: Vec<Option<String>>,
    all: All,
    /// Names the file assigns attributes on (`Widget.build = …`), which may replace or add
    /// the members a target names.
    patched: BTreeSet<String>,
}

/// A module file as last read.
#[derive(Clone)]
enum FileState {
    Missing,
    /// A file whose contents aren't summarized: unreadable, not UTF-8, unparsable or too
    /// large to index.
    Unparsed,
    Module {
        source_hash: blake3::Hash,
        summary: Arc<ModuleSummary>,
    },
}

/// Something an extraction read.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Read {
    /// The entries of a directory.
    Listing(Arc<str>),
    /// A module file.
    Module(Arc<str>),
}

/// The names whose lookups a change may have moved.
enum Changed {
    All,
    Names(BTreeSet<String>),
}

/// The Python modules of a workspace, read as call targets need them.
pub(crate) struct PythonWorkspace {
    root: PathBuf,
    /// The entries of each directory read; `None` for a path that isn't a directory.
    listings: HashMap<Arc<str>, Option<Arc<BTreeSet<String>>>>,
    modules: HashMap<Arc<str>, FileState>,
    /// The names any extraction looked up in each listing or module.
    queried: HashMap<Read, HashSet<String>>,
    /// What each extracted file's targets were resolved through, by the extracted file.
    reads: HashMap<Arc<str>, BTreeSet<Read>>,
    /// The extracted files that read each listing or module.
    readers: HashMap<Read, BTreeSet<Arc<str>>>,
}

impl PythonWorkspace {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            listings: HashMap::new(),
            modules: HashMap::new(),
            queried: HashMap::new(),
            reads: HashMap::new(),
            readers: HashMap::new(),
        }
    }

    /// Rewrites the call targets of `file_path` that name a module of the workspace through
    /// which the callee is only re-exported, or a `src/`-layout module without its `src`
    /// prefix, to name the module that defines the callee. Targets in the file's own module
    /// were resolved with the file's scopes at hand, and targets that can't be followed to a
    /// single definition keep their spelling.
    pub(crate) fn resolve_calls(
        &mut self,
        parser: &mut Parser,
        file_path: &str,
        edges: &mut [SymbolEdge],
    ) {
        let own_module = python_module_path(file_path);
        let mut lookup = Lookup {
            root: &self.root,
            listings: &mut self.listings,
            modules: &mut self.modules,
            queried: &mut self.queried,
            parser,
            read: BTreeSet::new(),
        };
        for edge in edges
            .iter_mut()
            .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        {
            let Some((module, rest)) = edge.target_qualified_name.split_once("::") else {
                continue;
            };
            if module == own_module {
                continue;
            }
            let names = rest.split("::").map(str::to_owned).collect::<Vec<_>>();
            if let Some(target) = lookup.resolve(module, &names, file_path, 0) {
                edge.target_qualified_name = target;
            }
        }
        let read = lookup.read;
        self.record_reads(file_path, read);
    }

    /// Re-reads `path` (a file or a directory) after it changed, appeared or went away,
    /// with the directories above it. Returns the extracted files that looked up, in what
    /// changed, a name whose binding or presence changed: their targets may now resolve
    /// differently. Nothing that no extraction read is re-read.
    pub(crate) fn refresh(&mut self, parser: &mut Parser, path: &str) -> Vec<String> {
        let path = normalize_path(path);
        let path = path.trim_matches('/');
        let mut stale = BTreeSet::new();

        if let Some((key, previous)) = self
            .modules
            .get_key_value(path)
            .map(|(key, state)| (Arc::clone(key), state.clone()))
        {
            let current = read_module(&self.root, path, parser, Some(&previous));
            let changed = changed_names(&previous, &current);
            self.modules.insert(Arc::clone(&key), current);
            stale.extend(self.affected(&Read::Module(key), &changed));
        }

        let mut directories = vec![path];
        let mut rest = path;
        while let Some((parent, _)) = rest.rsplit_once('/') {
            directories.push(parent);
            rest = parent;
        }
        if !path.is_empty() {
            directories.push("");
        }
        for directory in directories {
            let Some((key, previous)) = self
                .listings
                .get_key_value(directory)
                .map(|(key, entries)| (Arc::clone(key), entries.clone()))
            else {
                continue;
            };
            let current = list_directory(&self.root, directory);
            let changed = match (&previous, &current) {
                (Some(before), Some(after)) => Changed::Names(
                    before
                        .symmetric_difference(after)
                        .cloned()
                        .collect::<BTreeSet<_>>(),
                ),
                (None, None) => Changed::Names(BTreeSet::new()),
                _ => Changed::All,
            };
            if matches!(&changed, Changed::Names(names) if names.is_empty()) {
                continue;
            }
            self.listings.insert(Arc::clone(&key), current);
            // What sat under an entry that came or went is read afresh when next needed.
            let gone = match (&changed, &previous) {
                (Changed::Names(names), _) => names.iter().cloned().collect::<Vec<_>>(),
                (Changed::All, Some(entries)) => entries.iter().cloned().collect(),
                (Changed::All, None) => Vec::new(),
            };
            for name in gone {
                self.forget_under(&join(directory, &name));
            }
            stale.extend(self.affected(&Read::Listing(key), &changed));
        }

        // A file that went away reads nothing any more.
        if !self.root.join(path).exists() {
            self.record_reads(path, BTreeSet::new());
        }
        stale.into_iter().map(|reader| reader.to_string()).collect()
    }

    /// The files that read `read` and looked up a name `changed` covers in it.
    fn affected(&self, read: &Read, changed: &Changed) -> Vec<Arc<str>> {
        let touched = match changed {
            Changed::All => true,
            Changed::Names(names) => self
                .queried
                .get(read)
                .is_some_and(|queried| names.iter().any(|name| queried.contains(name))),
        };
        if !touched {
            return Vec::new();
        }
        self.readers
            .get(read)
            .into_iter()
            .flatten()
            .cloned()
            .collect()
    }

    /// Drops what was read at or under `path`.
    fn forget_under(&mut self, path: &str) {
        let under = |key: &str| {
            key == path
                || key
                    .strip_prefix(path)
                    .is_some_and(|rest| rest.starts_with('/'))
        };
        self.listings.retain(|key, _| !under(key));
        self.modules.retain(|key, _| !under(key));
    }

    fn record_reads(&mut self, file_path: &str, read: BTreeSet<Read>) {
        let reader = self
            .reads
            .get_key_value(file_path)
            .map_or_else(|| Arc::from(file_path), |(key, _)| Arc::clone(key));
        for previous in self.reads.remove(file_path).into_iter().flatten() {
            if let Some(readers) = self.readers.get_mut(&previous) {
                readers.remove(file_path);
                if readers.is_empty() {
                    self.readers.remove(&previous);
                }
            }
        }
        for item in &read {
            self.readers
                .entry(item.clone())
                .or_default()
                .insert(Arc::clone(&reader));
        }
        if !read.is_empty() {
            self.reads.insert(reader, read);
        }
    }
}

/// Whether a `*` import may bind a name.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Exports {
    Yes,
    /// The workspace can't tell: a module it doesn't hold, a dynamic `__all__`, or a
    /// package's submodule.
    Maybe,
    No,
}

/// Where a target goes next from the module it names.
enum Next {
    /// The module defines it.
    Here,
    /// The module imports it under this name from another module.
    Import { module: String, name: String },
    /// The module takes it from another module with `*`.
    Star(String),
}

/// One extraction's lookups, with what they read.
struct Lookup<'w> {
    root: &'w Path,
    listings: &'w mut HashMap<Arc<str>, Option<Arc<BTreeSet<String>>>>,
    modules: &'w mut HashMap<Arc<str>, FileState>,
    queried: &'w mut HashMap<Read, HashSet<String>>,
    parser: &'w mut Parser,
    read: BTreeSet<Read>,
}

impl Lookup<'_> {
    /// `module::names…`, spelled with the module that defines `names[0]`, as `from_file`'s
    /// imports see `module`. `None` when the target can't be followed to one definition.
    fn resolve(
        &mut self,
        module: &str,
        names: &[String],
        from_file: &str,
        hops: usize,
    ) -> Option<String> {
        if hops > MAX_HOPS {
            return None;
        }
        let (module, file) = self.locate(module, from_file)?;
        let summary = self.summary(&file)?;
        let (first, rest) = names.split_first()?;
        if !rest.is_empty() {
            // `Widget.build = …` replaces or adds the member the target names.
            self.query(Read::Module(Arc::from(file.as_str())), first);
            if summary.patched.contains(first) {
                return None;
            }
        }
        match self.next(&summary, &file, first)? {
            Next::Here => Some(format!("{module}::{}", names.join("::"))),
            Next::Import {
                module: source,
                name,
            } => {
                let mut names = vec![name];
                names.extend_from_slice(rest);
                self.resolve(&source, &names, &file, hops + 1)
            }
            Next::Star(source) => self.resolve(&source, names, &file, hops + 1),
        }
    }

    /// How the module summarized by `summary` binds `name`. A name it binds itself and that a
    /// `*` import may also bind, or that more than one `*` import may bind, has no single
    /// source: which one holds depends on the order and conditions they run in.
    fn next(&mut self, summary: &ModuleSummary, file: &str, name: &str) -> Option<Next> {
        self.query(Read::Module(Arc::from(file)), name);
        let stars = summary
            .star_imports
            .iter()
            .filter_map(|star| {
                let verdict = self.exports(star.as_deref(), file, name, &mut HashSet::new());
                (verdict != Exports::No).then_some((star.as_deref(), verdict))
            })
            .collect::<Vec<_>>();
        match summary.names.get(name) {
            Some(Export::Defined) if stars.is_empty() => Some(Next::Here),
            Some(Export::Imported {
                module,
                name: imported,
            }) if stars.is_empty()
                // `from .core import *` beside `from .core import name` binds one thing
                // twice; beside `from .core import other as name`, two things.
                || (imported == name
                    && stars.iter().all(|(star, verdict)| {
                        *star == Some(module.as_str()) && *verdict == Exports::Yes
                    })) =>
            {
                Some(Next::Import {
                    module: module.clone(),
                    name: imported.clone(),
                })
            }
            Some(_) => None,
            None => match stars.as_slice() {
                [(Some(module), Exports::Yes)] => Some(Next::Star((*module).to_owned())),
                _ => None,
            },
        }
    }

    /// Whether `from module import *`, written in `from_file`, binds `name`. Answering "yes"
    /// or "maybe" for a name it doesn't bind only leaves a target unresolved; answering "no"
    /// for one it binds could send a target to another binding, so "no" needs certainty.
    fn exports(
        &mut self,
        module: Option<&str>,
        from_file: &str,
        name: &str,
        visited: &mut HashSet<String>,
    ) -> Exports {
        let public = !name.starts_with('_');
        let unknown = if public { Exports::Maybe } else { Exports::No };
        let Some((_, file)) = module.and_then(|module| self.locate(module, from_file)) else {
            return unknown;
        };
        let Some(summary) = self.summary(&file) else {
            return unknown;
        };
        self.query(Read::Module(Arc::from(file.as_str())), name);
        let bound = summary.names.contains_key(name);
        match &summary.all {
            All::Literal(names) => {
                return if names.contains(name) {
                    Exports::Yes
                } else {
                    Exports::No
                };
            }
            All::Dynamic => return if bound { Exports::Yes } else { Exports::Maybe },
            All::Absent => {}
        }
        if !public {
            return Exports::No;
        }
        if bound {
            return Exports::Yes;
        }
        // A package's submodule is one of its attributes once anything imports it.
        if let Some(package) = file.strip_suffix("__init__.py")
            && self
                .module_file(package.trim_end_matches('/'), &[name.to_owned()])
                .is_some()
        {
            return Exports::Maybe;
        }
        // A module that takes `*` from itself, or from a module that does, adds nothing.
        if !visited.insert(file.clone()) {
            return Exports::No;
        }
        let mut verdict = Exports::No;
        for star in &summary.star_imports {
            match self.exports(star.as_deref(), &file, name, visited) {
                Exports::Yes => return Exports::Yes,
                Exports::Maybe => verdict = Exports::Maybe,
                Exports::No => {}
            }
        }
        verdict
    }

    /// The file of `module` and the module's path as the workspace's symbols spell it, as an
    /// import in `from_file` names it: `pkg/__init__.py` or `pkg/mod.py` from the workspace
    /// root for `pkg.mod`, or the same under the `src/` directory of the nearest directory
    /// above `from_file` that has one (`src.pkg.mod`). `None` when Python could load more
    /// than one of them, or one in `from_file`'s own directory, which comes first on
    /// `sys.path` when the file runs as a script and in pytest's default `prepend` mode.
    fn locate(&mut self, module: &str, from_file: &str) -> Option<(String, String)> {
        let segments = module_segments(module);
        let head = segments.first()?.clone();
        let directories = python_directory_segments(from_file);
        if !directories.is_empty()
            && self
                .module_file(&directories.join("/"), &segments)
                .is_some()
        {
            return None;
        }
        let at_root = self.module_file("", &segments);
        // The file's own top-level directory is the root's (relative imports, a package
        // importing itself), never a `src/` package.
        let under_src = if directories.first() == Some(&head) {
            None
        } else {
            self.src_module(&directories, &segments)
        };
        match (at_root, under_src) {
            (Some(file), None) => Some((segments.join("."), file)),
            (None, Some((src, file))) => Some((
                format!("{}.{}", src.replace('/', "."), segments.join(".")),
                file,
            )),
            _ => None,
        }
    }

    /// The `src/` directory nearest above `directories` that holds the module, and its file.
    fn src_module(
        &mut self,
        directories: &[String],
        segments: &[String],
    ) -> Option<(String, String)> {
        let mut directories = directories.to_vec();
        loop {
            let directory = directories.join("/");
            if self.has(&directory, "src") {
                let src = join(&directory, "src");
                if let Some(file) = self.module_file(&src, segments) {
                    return Some((src, file));
                }
            }
            directories.pop()?;
        }
    }

    /// The file of the module `segments` names under `base`: a package's `__init__.py`, else
    /// a module's `.py` file, in the order Python's import system looks for them.
    fn module_file(&mut self, base: &str, segments: &[String]) -> Option<String> {
        let (last, parents) = segments.split_last()?;
        let mut directory = base.to_owned();
        for segment in parents {
            if !self.has(&directory, segment) {
                return None;
            }
            directory = join(&directory, segment);
        }
        let package = join(&directory, last);
        if self.has(&directory, last) && self.has(&package, "__init__.py") {
            return Some(join(&package, "__init__.py"));
        }
        let module = format!("{last}.py");
        self.has(&directory, &module)
            .then(|| join(&directory, &module))
    }

    /// Whether `directory` has an entry `name`.
    fn has(&mut self, directory: &str, name: &str) -> bool {
        let key = self
            .listings
            .get_key_value(directory)
            .map(|(key, _)| Arc::clone(key));
        let key = match key {
            Some(key) => key,
            None => {
                let key = Arc::<str>::from(directory);
                let entries = list_directory(self.root, directory);
                self.listings.insert(Arc::clone(&key), entries);
                key
            }
        };
        let read = Read::Listing(Arc::clone(&key));
        self.query(read, name);
        self.listings
            .get(&key)
            .and_then(Option::as_ref)
            .is_some_and(|entries| entries.contains(name))
    }

    fn summary(&mut self, file: &str) -> Option<Arc<ModuleSummary>> {
        let key = self
            .modules
            .get_key_value(file)
            .map(|(key, _)| Arc::clone(key));
        let key = match key {
            Some(key) => key,
            None => {
                let key = Arc::<str>::from(file);
                let state = read_module(self.root, file, self.parser, None);
                self.modules.insert(Arc::clone(&key), state);
                key
            }
        };
        self.read.insert(Read::Module(Arc::clone(&key)));
        match self.modules.get(&key)? {
            FileState::Module { summary, .. } => Some(Arc::clone(summary)),
            FileState::Missing | FileState::Unparsed => None,
        }
    }

    /// Records that this extraction read `read` and looked up `name` in it.
    fn query(&mut self, read: Read, name: &str) {
        let names = self.queried.entry(read.clone()).or_default();
        if !names.contains(name) {
            names.insert(name.to_owned());
        }
        self.read.insert(read);
    }
}

fn join(directory: &str, name: &str) -> String {
    if directory.is_empty() {
        name.to_owned()
    } else {
        format!("{directory}/{name}")
    }
}

fn list_directory(root: &Path, directory: &str) -> Option<Arc<BTreeSet<String>>> {
    let entries = fs::read_dir(root.join(directory)).ok()?;
    Some(Arc::new(
        entries
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect(),
    ))
}

/// The names whose lookups in a module may differ between two reads of it.
fn changed_names(previous: &FileState, current: &FileState) -> Changed {
    match (previous, current) {
        (FileState::Missing, FileState::Missing) | (FileState::Unparsed, FileState::Unparsed) => {
            Changed::Names(BTreeSet::new())
        }
        (
            FileState::Module {
                summary: before, ..
            },
            FileState::Module { summary: after, .. },
        ) => {
            if before.star_imports != after.star_imports || before.all != after.all {
                return Changed::All;
            }
            let mut names = before
                .names
                .iter()
                .filter(|(name, export)| after.names.get(*name) != Some(*export))
                .chain(
                    after
                        .names
                        .iter()
                        .filter(|(name, _)| !before.names.contains_key(*name)),
                )
                .map(|(name, _)| name.clone())
                .collect::<BTreeSet<_>>();
            names.extend(before.patched.symmetric_difference(&after.patched).cloned());
            Changed::Names(names)
        }
        _ => Changed::All,
    }
}

/// Reads and summarizes the module file at `path`, reusing `previous`'s summary when the
/// source is unchanged.
fn read_module(
    root: &Path,
    path: &str,
    parser: &mut Parser,
    previous: Option<&FileState>,
) -> FileState {
    let full_path = root.join(path);
    match fs::metadata(&full_path) {
        Ok(metadata) if !metadata.is_file() => return FileState::Missing,
        Ok(metadata) if metadata.len() > MAX_MODULE_FILE_SIZE => return FileState::Unparsed,
        Ok(_) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => return FileState::Missing,
        Err(_) => return FileState::Unparsed,
    }
    let Ok(source) = fs::read_to_string(&full_path) else {
        return FileState::Unparsed;
    };
    let source_hash = blake3::hash(source.as_bytes());
    if let Some(FileState::Module {
        source_hash: previous_hash,
        summary,
    }) = previous
        && *previous_hash == source_hash
    {
        return FileState::Module {
            source_hash,
            summary: Arc::clone(summary),
        };
    }
    match summarize(path, &source, parser) {
        Some(summary) => FileState::Module {
            source_hash,
            summary: Arc::new(summary),
        },
        None => FileState::Unparsed,
    }
}

/// The top-level bindings of the module in `source`, by the same rules the call resolver
/// applies to a module's scope.
fn summarize(file_path: &str, source: &str, parser: &mut Parser) -> Option<ModuleSummary> {
    let tree = parser.parse(source, None)?;
    let root = tree.root_node();
    let source = source.as_bytes();
    let resolver = Resolver::new(file_path, source, &[]);
    let mut names = BTreeMap::new();
    for name in resolver.scope(root).names.keys() {
        let export = match resolver.bindings(root, root, name, None).binding() {
            None => continue,
            Some(Binding::Definition(_)) => Export::Defined,
            Some(Binding::Imported { module, name }) if !module.stdlib => Export::Imported {
                module: module.path,
                name,
            },
            Some(_) => Export::Opaque,
        };
        names.insert(name.clone(), export);
    }
    // A function that declares `global name` and binds it creates the module's `name`.
    for name in resolver.rebound(root).get(&root.id()).into_iter().flatten() {
        names.insert(name.clone(), Export::Opaque);
    }
    let (star_imports, all, patched) =
        module_statements(root, source, &FileContext::new(file_path));
    Some(ModuleSummary {
        names,
        star_imports,
        all,
        patched,
    })
}

/// The modules a module's `*` imports name, its `__all__`, and the names the file assigns
/// attributes on.
fn module_statements(
    root: Node<'_>,
    source: &[u8],
    file: &FileContext<'_>,
) -> (Vec<Option<String>>, All, BTreeSet<String>) {
    let mut star_imports = Vec::new();
    let mut all_literals = Vec::new();
    // `__all__` assigned or added to at the module's top level by something other than a
    // literal.
    let mut all_dynamic = false;
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "import_from_statement" => {
                let mut cursor = node.walk();
                if node
                    .named_children(&mut cursor)
                    .any(|child| child.kind() == "wildcard_import")
                {
                    let module = node
                        .child_by_field_name("module_name")
                        .and_then(|module| file.module(&node_text(module, source)))
                        .filter(|module| !module.stdlib)
                        .map(|module| module.path);
                    star_imports.push((node.start_byte(), module));
                }
                continue;
            }
            "assignment" | "augmented_assignment" => {
                if node.child_by_field_name("left").is_some_and(|left| {
                    left.kind() == "identifier" && node_text(left, source) == "__all__"
                }) {
                    match node
                        .child_by_field_name("right")
                        .and_then(|right| string_sequence(right, source))
                    {
                        Some(names) => all_literals.push(names),
                        None => all_dynamic = true,
                    }
                }
                continue;
            }
            "function_definition" | "class_definition" => continue,
            kind if !STATEMENT_CONTAINERS.contains(&kind) => continue,
            _ => {}
        }
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
    star_imports.sort_by_key(|(position, _)| *position);

    // Every other mention of `__all__`, anywhere in the file (`__all__.append(…)` in a
    // decorator, `global __all__`, an import of it), may change it.
    let mut patched = BTreeSet::new();
    let mut all_mentions = 0usize;
    let mut cursor = root.walk();
    loop {
        let node = cursor.node();
        if node.kind() == "identifier" && node_text(node, source) == "__all__" {
            all_mentions += 1;
        }
        if matches!(node.kind(), "assignment" | "augmented_assignment")
            && let Some(object) = node
                .child_by_field_name("left")
                .filter(|left| left.kind() == "attribute")
                .and_then(|left| left.child_by_field_name("object"))
                .filter(|object| object.kind() == "identifier")
        {
            patched.insert(node_text(object, source));
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                break;
            }
        }
        if cursor.node() == root {
            break;
        }
    }

    let all = if all_mentions == 0 {
        All::Absent
    } else if all_dynamic || all_mentions != all_literals.len() {
        All::Dynamic
    } else {
        All::Literal(all_literals.into_iter().flatten().collect())
    };
    (
        star_imports.into_iter().map(|(_, module)| module).collect(),
        all,
        patched,
    )
}

/// The strings of a list or tuple of string literals; `None` for anything else.
fn string_sequence(node: Node<'_>, source: &[u8]) -> Option<BTreeSet<String>> {
    if !matches!(node.kind(), "list" | "tuple") {
        return None;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .map(|item| {
            if item.kind() != "string" {
                return None;
            }
            let mut parts = item.walk();
            let mut text = String::new();
            for part in item.named_children(&mut parts) {
                match part.kind() {
                    "string_start" | "string_end" => {}
                    "string_content" => text.push_str(&node_text(part, source)),
                    _ => return None,
                }
            }
            Some(text)
        })
        .collect()
}
