use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;

use aether_core::{
    EdgeKind, Language, Symbol, SymbolEdge, SymbolKind, file_source_id, normalize_path,
};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, Query, QueryCursor};

use crate::parser::{TestIntent, humanize_test_name, normalize_intent_text};
use crate::parser::{build_symbol, named_child_text, node_text};
use crate::registry::{LanguageConfig, LanguageHooks, QueryCaptures};

pub fn config() -> LanguageConfig {
    let ts_language = tree_sitter_python::LANGUAGE.into();
    let symbol_query = Query::new(&ts_language, include_str!("../queries/python_symbols.scm"))
        .expect("invalid python symbol query");
    let edge_query = Query::new(&ts_language, include_str!("../queries/python_edges.scm"))
        .expect("invalid python edge query");
    let test_intent_query = Query::new(
        &ts_language,
        include_str!("../queries/python_test_intents.scm"),
    )
    .expect("invalid python test intent query");

    LanguageConfig {
        id: "python",
        extensions: &["py", "pyi"],
        ts_language,
        symbol_query,
        edge_query,
        test_intent_query: Some(test_intent_query),
        module_markers: &["__init__.py", "pyproject.toml", "setup.py", "setup.cfg"],
        hooks: Some(Box::new(PythonHooks)),
    }
}

struct PythonHooks;

impl LanguageHooks for PythonHooks {
    fn qualify_name(
        &self,
        file_path: &str,
        symbol_name: &str,
        parent: Option<&str>,
    ) -> Option<String> {
        Some(qualify(file_path, symbol_name, parent))
    }

    fn map_symbol(
        &self,
        language: Language,
        captures: &QueryCaptures<'_, '_>,
        source: &[u8],
        file_path: &str,
    ) -> Option<Symbol> {
        let capture_name = captures.first_capture_name_with_prefix("symbol.")?;
        let node = captures.node_with_prefix("symbol.")?;

        match capture_name {
            "symbol.function" => {
                if is_directly_decorated(node) {
                    return None;
                }
                self.map_function_symbol(language, file_path, source, node)
            }
            "symbol.class" => {
                if is_directly_decorated(node) {
                    return None;
                }
                self.map_class_symbol(language, file_path, source, node)
            }
            "symbol.decorated" => self.map_decorated_symbol(language, file_path, source, node),
            "symbol.variable" => self.map_variable_symbol(language, file_path, source, node),
            "symbol.type_alias" => self.map_type_alias_symbol(language, file_path, source, node),
            _ => None,
        }
    }

    fn map_edge(
        &self,
        _language: Language,
        captures: &QueryCaptures<'_, '_>,
        source: &[u8],
        file_path: &str,
        symbols: &[Symbol],
    ) -> Option<Vec<SymbolEdge>> {
        let capture_name = captures.first_capture_name_with_prefix("edge.")?;
        let node = captures.node_with_prefix("edge.")?;

        // `extract_python_edges` maps whole files. This maps a single node, finding its
        // enclosing nodes by walking up. Always answer with a list, even an empty one, so the
        // generic mapper never turns a skipped node into an edge built from its raw text.
        let edges = match capture_name {
            "edge.call" => {
                let mut ancestors = Vec::new();
                let mut current = node.parent();
                while let Some(parent) = current {
                    ancestors.push(parent);
                    current = parent.parent();
                }
                ancestors.reverse();
                let scopes = (0..ancestors.len())
                    .filter(|&index| SCOPE_KINDS.contains(&ancestors[index].kind()))
                    .collect::<Vec<_>>();
                let enclosing = Enclosing {
                    nodes: &ancestors,
                    scopes: &scopes,
                };
                Resolver::new(file_path, source, symbols)
                    .call_edge(node, enclosing)
                    .into_iter()
                    .collect()
            }
            "edge.depends_on" => import_edges(node, source, file_path),
            _ => Vec::new(),
        };
        Some(edges)
    }

    fn map_test_intent(
        &self,
        language: Language,
        captures: &QueryCaptures<'_, '_>,
        source: &[u8],
        file_path: &str,
        _symbols: &[Symbol],
    ) -> Option<Vec<TestIntent>> {
        let node = captures.node_with_prefix("test.")?;
        if node.kind() != "function_definition" {
            return None;
        }

        let test_name = named_child_text(node, "name", source)?;
        if !test_name.starts_with("test_") {
            return None;
        }

        let symbol_id = self
            .map_function_symbol(language, file_path, source, node)
            .map(|symbol| symbol.id);
        let intent_text = python_docstring(node, source)
            .unwrap_or_else(|| humanize_test_name(test_name.as_str()));
        let intent_text = normalize_intent_text(intent_text.as_str());
        if intent_text.is_empty() {
            return None;
        }

        Some(vec![TestIntent {
            file_path: file_path.to_owned(),
            test_name,
            intent_text,
            group_label: None,
            language,
            symbol_id,
        }])
    }

    fn is_module_root(&self, dir_path: &Path) -> Option<bool> {
        Some(dir_path.join("__init__.py").exists())
    }
}

impl PythonHooks {
    fn map_function_symbol(
        &self,
        language: Language,
        file_path: &str,
        source: &[u8],
        node: Node<'_>,
    ) -> Option<Symbol> {
        let name = named_child_text(node, "name", source)?;
        let kind = if is_method_definition(node) {
            SymbolKind::Method
        } else {
            SymbolKind::Function
        };
        let qualified_name = function_qualified_name(node, &name, source, file_path);
        Some(build_symbol(
            language,
            file_path,
            kind,
            &name,
            &qualified_name,
            node,
            source,
        ))
    }

    fn map_class_symbol(
        &self,
        language: Language,
        file_path: &str,
        source: &[u8],
        node: Node<'_>,
    ) -> Option<Symbol> {
        let name = named_child_text(node, "name", source)?;
        let qualified_name = class_qualified_name(node, &name, source, file_path);
        Some(build_symbol(
            language,
            file_path,
            SymbolKind::Class,
            &name,
            &qualified_name,
            node,
            source,
        ))
    }

    fn map_decorated_symbol(
        &self,
        language: Language,
        file_path: &str,
        source: &[u8],
        node: Node<'_>,
    ) -> Option<Symbol> {
        let inner = node
            .child_by_field_name("definition")
            .or_else(|| find_inner_definition(node))?;
        match inner.kind() {
            "function_definition" => self.map_function_symbol(language, file_path, source, inner),
            "class_definition" => self.map_class_symbol(language, file_path, source, inner),
            _ => None,
        }
    }

    fn map_variable_symbol(
        &self,
        language: Language,
        file_path: &str,
        source: &[u8],
        node: Node<'_>,
    ) -> Option<Symbol> {
        if has_ancestor_kind(node, "function_definition")
            || has_ancestor_kind(node, "class_definition")
        {
            return None;
        }
        let name_node = node
            .child_by_field_name("left")
            .or_else(|| node.child_by_field_name("name"))
            .or_else(|| node.named_child(0))?;
        let name = node_text(name_node, source).trim().to_owned();
        if name.is_empty() {
            return None;
        }
        if node.kind() == "assignment" {
            let has_type = node.child_by_field_name("type").is_some();
            if !has_type && name != "__all__" {
                return None;
            }
        }
        let qualified_name = self
            .qualify_name(file_path, &name, None)
            .unwrap_or_else(|| format!("{}::{name}", python_module_path(file_path)));
        Some(build_symbol(
            language,
            file_path,
            SymbolKind::Variable,
            &name,
            &qualified_name,
            node,
            source,
        ))
    }

    fn map_type_alias_symbol(
        &self,
        language: Language,
        file_path: &str,
        source: &[u8],
        node: Node<'_>,
    ) -> Option<Symbol> {
        let name = named_child_text(node, "name", source).or_else(|| {
            node.named_child(0)
                .map(|child| node_text(child, source).trim().to_owned())
        })?;
        if name.is_empty() {
            return None;
        }
        let qualified_name = self
            .qualify_name(file_path, &name, None)
            .unwrap_or_else(|| format!("{}::{name}", python_module_path(file_path)));
        Some(build_symbol(
            language,
            file_path,
            SymbolKind::TypeAlias,
            &name,
            &qualified_name,
            node,
            source,
        ))
    }
}

fn qualify(file_path: &str, symbol_name: &str, parent: Option<&str>) -> String {
    let module = python_module_path(file_path);
    match parent {
        Some(parent) if !parent.trim().is_empty() => {
            format!("{module}::{parent}::{symbol_name}")
        }
        _ => format!("{module}::{symbol_name}"),
    }
}

/// The qualified name of a `def`: a method is qualified by its enclosing classes alone, any
/// other function by its enclosing classes and functions.
fn function_qualified_name(node: Node<'_>, name: &str, source: &[u8], file_path: &str) -> String {
    let parent = if is_method_definition(node) {
        nearest_ancestor_name(node, source, &["class_definition"])
    } else {
        nearest_ancestor_name(node, source, &["class_definition", "function_definition"])
    };
    qualify(file_path, name, parent.as_deref())
}

fn class_qualified_name(node: Node<'_>, name: &str, source: &[u8], file_path: &str) -> String {
    let parent = nearest_ancestor_name(node, source, &["class_definition", "function_definition"]);
    qualify(file_path, name, parent.as_deref())
}

fn python_module_path(file_path: &str) -> String {
    let normalized = normalize_path(file_path);
    let path = Path::new(&normalized);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("module");
    let is_init = stem == "__init__";

    let mut segments = python_directory_segments(file_path);

    if !is_init && !stem.trim().is_empty() {
        segments.push(stem.to_owned());
    }

    if segments.is_empty() {
        if is_init {
            "module".to_owned()
        } else {
            stem.to_owned()
        }
    } else {
        segments.join(".")
    }
}

/// The directories of a file's path, as its module path spells them.
fn python_directory_segments(file_path: &str) -> Vec<String> {
    let normalized = normalize_path(file_path);
    Path::new(&normalized)
        .parent()
        .map(|parent| {
            parent
                .iter()
                .filter_map(|part| {
                    let part = part.to_string_lossy().trim().to_owned();
                    (!part.is_empty() && part != ".").then_some(part)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn python_package_path(file_path: &str) -> String {
    let module = python_module_path(file_path);
    let normalized = normalize_path(file_path);
    let stem = Path::new(&normalized)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if stem == "__init__" {
        module
    } else {
        module
            .split('.')
            .filter(|segment| !segment.is_empty())
            .collect::<Vec<_>>()
            .split_last()
            .map(|(_, head)| head.join("."))
            .unwrap_or_default()
    }
}

/// Top-level modules of the Python standard library (3.12's `sys.stdlib_module_names`, without
/// private modules and the regression-test package `test`, a common name for a project's own
/// tests). Their members are spelled with dots, as Python writes them (`os.path.join`), so a
/// workspace module, or another language's module, of the same name can't match them.
const STDLIB_MODULES: &[&str] = &[
    "abc",
    "aifc",
    "antigravity",
    "argparse",
    "array",
    "ast",
    "asyncio",
    "atexit",
    "audioop",
    "base64",
    "bdb",
    "binascii",
    "bisect",
    "builtins",
    "bz2",
    "cProfile",
    "calendar",
    "cgi",
    "cgitb",
    "chunk",
    "cmath",
    "cmd",
    "code",
    "codecs",
    "codeop",
    "collections",
    "colorsys",
    "compileall",
    "concurrent",
    "configparser",
    "contextlib",
    "contextvars",
    "copy",
    "copyreg",
    "crypt",
    "csv",
    "ctypes",
    "curses",
    "dataclasses",
    "datetime",
    "dbm",
    "decimal",
    "difflib",
    "dis",
    "doctest",
    "email",
    "encodings",
    "ensurepip",
    "enum",
    "errno",
    "faulthandler",
    "fcntl",
    "filecmp",
    "fileinput",
    "fnmatch",
    "fractions",
    "ftplib",
    "functools",
    "gc",
    "genericpath",
    "getopt",
    "getpass",
    "gettext",
    "glob",
    "graphlib",
    "grp",
    "gzip",
    "hashlib",
    "heapq",
    "hmac",
    "html",
    "http",
    "idlelib",
    "imaplib",
    "imghdr",
    "importlib",
    "inspect",
    "io",
    "ipaddress",
    "itertools",
    "json",
    "keyword",
    "lib2to3",
    "linecache",
    "locale",
    "logging",
    "lzma",
    "mailbox",
    "mailcap",
    "marshal",
    "math",
    "mimetypes",
    "mmap",
    "modulefinder",
    "msilib",
    "msvcrt",
    "multiprocessing",
    "netrc",
    "nis",
    "nntplib",
    "nt",
    "ntpath",
    "nturl2path",
    "numbers",
    "opcode",
    "operator",
    "optparse",
    "os",
    "ossaudiodev",
    "pathlib",
    "pdb",
    "pickle",
    "pickletools",
    "pipes",
    "pkgutil",
    "platform",
    "plistlib",
    "poplib",
    "posix",
    "posixpath",
    "pprint",
    "profile",
    "pstats",
    "pty",
    "pwd",
    "py_compile",
    "pyclbr",
    "pydoc",
    "pydoc_data",
    "pyexpat",
    "queue",
    "quopri",
    "random",
    "re",
    "readline",
    "reprlib",
    "resource",
    "rlcompleter",
    "runpy",
    "sched",
    "secrets",
    "select",
    "selectors",
    "shelve",
    "shlex",
    "shutil",
    "signal",
    "site",
    "smtplib",
    "sndhdr",
    "socket",
    "socketserver",
    "spwd",
    "sqlite3",
    "sre_compile",
    "sre_constants",
    "sre_parse",
    "ssl",
    "stat",
    "statistics",
    "string",
    "stringprep",
    "struct",
    "subprocess",
    "sunau",
    "symtable",
    "sys",
    "sysconfig",
    "syslog",
    "tabnanny",
    "tarfile",
    "telnetlib",
    "tempfile",
    "termios",
    "textwrap",
    "this",
    "threading",
    "time",
    "timeit",
    "tkinter",
    "token",
    "tokenize",
    "tomllib",
    "trace",
    "traceback",
    "tracemalloc",
    "tty",
    "turtle",
    "turtledemo",
    "types",
    "typing",
    "unicodedata",
    "unittest",
    "urllib",
    "uu",
    "uuid",
    "venv",
    "warnings",
    "wave",
    "weakref",
    "webbrowser",
    "winreg",
    "winsound",
    "wsgiref",
    "xdrlib",
    "xml",
    "xmlrpc",
    "zipapp",
    "zipfile",
    "zipimport",
    "zlib",
    "zoneinfo",
];

/// Where a Python file sits, which is what its imports are resolved against.
struct FileContext<'a> {
    file_path: &'a str,
    directories: Vec<String>,
    /// The package relative imports start from; `None` for a top-level module, which is in
    /// no package. A top-level `__init__.py` is the package of the workspace root, whose
    /// modules are named without a prefix.
    package: Option<Vec<String>>,
}

/// A module an import names.
#[derive(Clone, PartialEq)]
struct ModulePath {
    path: String,
    stdlib: bool,
}

impl ModulePath {
    /// A member of the module as a call target: `pkg.mod::f`, or `os.path.join` for the
    /// standard library.
    fn member(&self, member: &str) -> Option<String> {
        if self.path.is_empty() {
            return None;
        }
        Some(if self.stdlib {
            format!("{}.{member}", self.path)
        } else {
            format!("{}::{member}", self.path)
        })
    }
}

impl<'a> FileContext<'a> {
    fn new(file_path: &'a str) -> Self {
        let directories = python_directory_segments(file_path);
        let normalized = normalize_path(file_path);
        let is_init = Path::new(&normalized)
            .file_stem()
            .is_some_and(|stem| stem == "__init__");
        let package = (is_init || !directories.is_empty()).then(|| directories.clone());
        Self {
            file_path,
            directories,
            package,
        }
    }

    /// The module an import names, spelled the way this workspace's module paths are.
    ///
    /// Relative imports resolve against the file's package, and one that climbs to or past
    /// the top-level package names nothing. In a `src` layout, where `src/pkg/mod.py` is the
    /// module `src.pkg.mod`, an absolute import of the file's own top-level package
    /// (`pkg.mod`) gains the same `src` prefix. Any other absolute import keeps its path, and
    /// is marked when it names the standard library.
    fn module(&self, written: &str) -> Option<ModulePath> {
        let written = compact(written);
        let dots = written.chars().take_while(|ch| *ch == '.').count();
        if dots > 0 {
            let package = self.package.as_ref()?;
            let ascend = dots - 1;
            if ascend > package.len() || (ascend == package.len() && !package.is_empty()) {
                return None;
            }
            let mut segments = package[..package.len() - ascend].to_vec();
            segments.extend(module_segments(&written[dots..]));
            return Some(ModulePath {
                path: segments.join("."),
                stdlib: false,
            });
        }
        let head = written.split('.').next().unwrap_or_default();
        let src_layout_root = (1..self.directories.len())
            .rev()
            .find(|&index| self.directories[index] == head && self.directories[index - 1] == "src");
        Some(match src_layout_root {
            Some(index) => ModulePath {
                path: format!("{}.{written}", self.directories[..index].join(".")),
                stdlib: false,
            },
            // A file inside a workspace package named like a standard-library module imports
            // that package.
            None => ModulePath {
                stdlib: STDLIB_MODULES.binary_search(&head).is_ok()
                    && self.directories.first().map(String::as_str) != Some(head),
                path: written,
            },
        })
    }
}

/// What a name refers to where it is used.
#[derive(Clone, PartialEq)]
enum Binding<'tree> {
    /// A `def` or `class` in this file.
    Definition(Node<'tree>),
    /// A module: `import pkg.mod` binds `pkg`, and `import pkg.mod as m` binds `m`.
    Module(ModulePath),
    /// A name taken from a module: `from module import name [as alias]`.
    Imported { module: ModulePath, name: String },
    /// A method's first parameter, which holds an instance of the class that declares the
    /// method, or the class itself in a `@classmethod`.
    Receiver(Node<'tree>),
    /// Anything whose value the source doesn't pin down: a parameter, a variable, a loop or
    /// pattern target, or a name bound in more than one way.
    Value,
}

/// How one scope binds a name.
#[derive(Default)]
struct ScopeBindings<'tree> {
    definitions: Vec<Node<'tree>>,
    imports: Vec<Binding<'tree>>,
    assigned: bool,
    global: bool,
    nonlocal: bool,
}

impl<'tree> ScopeBindings<'tree> {
    fn is_bound(&self) -> bool {
        !self.definitions.is_empty() || !self.imports.is_empty() || self.assigned
    }

    /// What the name holds when the scope binds it in one way only. A name bound in more
    /// than one way (a `def` beside the import it stands in for, a `def` rebound by
    /// `f = cache(f)` or by a loop, imports of different things) may hold any of them at the
    /// call, so it has no static target.
    fn binding(self) -> Option<Binding<'tree>> {
        let ways = usize::from(!self.definitions.is_empty())
            + usize::from(!self.imports.is_empty())
            + usize::from(self.assigned);
        match ways {
            0 => None,
            1 if !self.definitions.is_empty() => Some(Binding::Definition(self.definitions[0])),
            1 if !self.imports.is_empty()
                && self.imports.iter().all(|import| *import == self.imports[0]) =>
            {
                self.imports.into_iter().next()
            }
            _ => Some(Binding::Value),
        }
    }
}

/// One way a scope binds a name.
#[derive(Clone)]
enum BindingEvent<'tree> {
    Definition(Node<'tree>),
    Import(Binding<'tree>),
    Assigned,
    Global,
    Nonlocal,
}

/// The names one scope binds, with the byte offset where each binding is complete.
#[derive(Default)]
struct ScopeIndex<'tree> {
    names: HashMap<String, Vec<(usize, BindingEvent<'tree>)>>,
}

/// Statements whose children are statements of the same scope.
const STATEMENT_CONTAINERS: &[&str] = &[
    "module",
    "block",
    "expression_statement",
    "decorated_definition",
    "if_statement",
    "elif_clause",
    "else_clause",
    "for_statement",
    "while_statement",
    "try_statement",
    "except_clause",
    "except_group_clause",
    "finally_clause",
    "with_statement",
    "with_clause",
    "with_item",
    "match_statement",
];

impl<'tree> ScopeIndex<'tree> {
    /// Indexes the statements of `body`. Nested functions and classes have scopes of their
    /// own, so only their names count. Expressions are not walked; assignment expressions
    /// (`:=`) are found by scanning the scope's text outside the bodies of nested functions
    /// and classes, which may also count one in a lambda or a string, so it errs towards
    /// treating a name as a local.
    fn build(body: Node<'tree>, source: &[u8], file: &FileContext<'_>) -> Self {
        let mut index = Self::default();
        let mut nested = Vec::new();
        let mut stack = vec![body];
        while let Some(node) = stack.pop() {
            match node.kind() {
                "function_definition" | "class_definition" => {
                    // Its header (default values, base classes) runs in this scope; its body
                    // doesn't.
                    if let Some(nested_body) = node.child_by_field_name("body") {
                        nested.push((nested_body.start_byte(), nested_body.end_byte()));
                    }
                    if let Some(name) = node.child_by_field_name("name") {
                        index.add(
                            name,
                            source,
                            node.end_byte(),
                            BindingEvent::Definition(node),
                        );
                    }
                    continue;
                }
                "global_statement" | "nonlocal_statement" => {
                    let event = if node.kind() == "global_statement" {
                        BindingEvent::Global
                    } else {
                        BindingEvent::Nonlocal
                    };
                    let mut cursor = node.walk();
                    for declared in node.named_children(&mut cursor) {
                        index.add(declared, source, node.end_byte(), event.clone());
                    }
                    continue;
                }
                "import_statement" | "import_from_statement" => {
                    for (bound, binding) in import_bindings(node, source, file) {
                        let event = match binding {
                            Binding::Value => BindingEvent::Assigned,
                            binding => BindingEvent::Import(binding),
                        };
                        index.add(bound, source, node.end_byte(), event);
                    }
                    continue;
                }
                "assignment" | "augmented_assignment" => {
                    if let Some(left) = node.child_by_field_name("left") {
                        index.add_pattern(left, source, node.end_byte());
                    }
                    // `a = b = value` nests the second assignment on the right.
                    if let Some(right) = node.child_by_field_name("right").filter(|right| {
                        matches!(right.kind(), "assignment" | "augmented_assignment")
                    }) {
                        stack.push(right);
                    }
                    continue;
                }
                "for_statement" => {
                    // The iterable is evaluated before the target is bound.
                    if let Some(left) = node.child_by_field_name("left") {
                        let complete_at = node
                            .child_by_field_name("right")
                            .map_or(left.end_byte(), |right| right.end_byte());
                        index.add_pattern(left, source, complete_at);
                    }
                }
                "type_alias_statement" => {
                    if let Some(left) = node.child_by_field_name("left") {
                        index.add_pattern(left, source, node.end_byte());
                    }
                    continue;
                }
                // `del name` makes `name` local to the scope, as an assignment does.
                "delete_statement" => {
                    let mut cursor = node.walk();
                    for target in node.named_children(&mut cursor) {
                        index.add_pattern(target, source, node.end_byte());
                    }
                    continue;
                }
                // `with … as name` and `except … as name`.
                "as_pattern" => {
                    if let Some(alias) = node.child_by_field_name("alias") {
                        index.add_pattern(alias, source, alias.end_byte());
                    }
                    continue;
                }
                // A `match` block's cases: a capture binds its name for the whole scope.
                "case_clause" => {
                    let mut cursor = node.walk();
                    for pattern in node
                        .named_children(&mut cursor)
                        .filter(|pattern| pattern.kind() == "case_pattern")
                    {
                        for capture in case_pattern_captures(pattern) {
                            index.add(capture, source, pattern.end_byte(), BindingEvent::Assigned);
                        }
                    }
                    if let Some(consequence) = node.child_by_field_name("consequence") {
                        stack.push(consequence);
                    }
                    continue;
                }
                kind if !STATEMENT_CONTAINERS.contains(&kind) => continue,
                _ => {}
            }
            let mut cursor = node.walk();
            stack.extend(node.named_children(&mut cursor));
        }
        for (name, complete_at) in walrus_targets(body, source, &nested) {
            index
                .names
                .entry(name)
                .or_default()
                .push((complete_at, BindingEvent::Assigned));
        }
        index
    }

    fn add(
        &mut self,
        name: Node<'_>,
        source: &[u8],
        complete_at: usize,
        event: BindingEvent<'tree>,
    ) {
        let name = node_text(name, source);
        if !name.is_empty() {
            self.names
                .entry(name)
                .or_default()
                .push((complete_at, event));
        }
    }

    /// Adds the names an assignment target binds. Attributes and subscripts bind nothing.
    fn add_pattern(&mut self, pattern: Node<'_>, source: &[u8], complete_at: usize) {
        for name in pattern_names(pattern) {
            self.add(name, source, complete_at, BindingEvent::Assigned);
        }
    }

    /// How the scope binds `name`, counting only the bindings complete by byte `before` when
    /// it is given.
    fn lookup(&self, name: &str, before: Option<usize>) -> ScopeBindings<'tree> {
        let mut found = ScopeBindings::default();
        for (complete_at, event) in self.names.get(name).into_iter().flatten() {
            match event {
                BindingEvent::Global => found.global = true,
                BindingEvent::Nonlocal => found.nonlocal = true,
                _ if before.is_some_and(|position| *complete_at > position) => {}
                BindingEvent::Definition(definition) => found.definitions.push(*definition),
                BindingEvent::Import(binding) => found.imports.push(binding.clone()),
                BindingEvent::Assigned => found.assigned = true,
            }
        }
        found
    }
}

/// A node the edge query captures.
enum EdgeSite {
    Call,
    Import,
}

/// Nodes that open a scope of their own.
const SCOPE_KINDS: &[&str] = &[
    "module",
    "function_definition",
    "lambda",
    "class_definition",
    "list_comprehension",
    "set_comprehension",
    "dictionary_comprehension",
    "generator_expression",
];

/// The nodes enclosing an edge site, outermost first, and the positions among them of the
/// ones that open a scope.
#[derive(Clone, Copy)]
struct Enclosing<'s, 'tree> {
    nodes: &'s [Node<'tree>],
    scopes: &'s [usize],
}

/// The call and import edges of a Python file.
///
/// The edge query picks the nodes that make edges, and one walk of the tree reaches each of
/// them with its enclosing nodes and scopes at hand. Each scope's bindings are indexed the
/// first time a lookup reaches it, so resolving a call costs a walk up its enclosing scopes
/// rather than a scan of their statements.
pub(crate) fn extract_python_edges(
    file_path: &str,
    source: &[u8],
    root: Node<'_>,
    edge_query: &Query,
    symbols: &[Symbol],
) -> Vec<SymbolEdge> {
    let mut sites = HashMap::new();
    let mut query_cursor = QueryCursor::new();
    let mut query_matches = query_cursor.matches(edge_query, root, source);
    while {
        query_matches.advance();
        query_matches.get().is_some()
    } {
        let matched = query_matches.get().expect("query match should exist");
        let captures = QueryCaptures::new(edge_query, matched.captures, matched.pattern_index);
        let site = match captures.first_capture_name_with_prefix("edge.") {
            Some("edge.call") => EdgeSite::Call,
            Some("edge.depends_on") => EdgeSite::Import,
            _ => continue,
        };
        if let Some(node) = captures.node_with_prefix("edge.") {
            sites.insert(node.id(), site);
        }
    }

    let resolver = Resolver::new(file_path, source, symbols);
    let mut edges = Vec::new();
    let mut ancestors = Vec::new();
    let mut scopes = Vec::new();
    let mut cursor = root.walk();
    loop {
        let node = cursor.node();
        match sites.get(&node.id()) {
            Some(EdgeSite::Call) => {
                let enclosing = Enclosing {
                    nodes: &ancestors,
                    scopes: &scopes,
                };
                edges.extend(resolver.call_edge(node, enclosing));
            }
            Some(EdgeSite::Import) => edges.extend(import_edges(node, source, file_path)),
            None => {}
        }
        if cursor.goto_first_child() {
            if SCOPE_KINDS.contains(&node.kind()) {
                scopes.push(ancestors.len());
            }
            ancestors.push(node);
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return edges;
            }
            ancestors.pop();
            if scopes.last() == Some(&ancestors.len()) {
                scopes.pop();
            }
        }
    }
}

fn import_edges(node: Node<'_>, source: &[u8], file_path: &str) -> Vec<SymbolEdge> {
    python_import_targets(node, source, file_path)
        .into_iter()
        .map(|target| SymbolEdge {
            source_id: file_source_id(file_path),
            target_qualified_name: target,
            edge_kind: EdgeKind::DependsOn,
            file_path: file_path.to_owned(),
        })
        .collect()
}

/// Resolves the calls of one file, keeping what it learns about the file's scopes.
struct Resolver<'a, 'tree> {
    file: FileContext<'a>,
    source: &'a [u8],
    /// Function and method symbols by the byte range of their `def`.
    functions: HashMap<(usize, usize), &'a str>,
    scopes: RefCell<HashMap<usize, Rc<ScopeIndex<'tree>>>>,
    /// Names a function rebinds in an enclosing scope, by that scope's body.
    rebound: OnceCell<HashMap<usize, HashSet<String>>>,
    /// The names a class body calls while it runs, by the class body.
    class_body_calls: RefCell<HashMap<usize, Rc<HashSet<String>>>>,
    qualified_names: RefCell<HashMap<usize, Option<String>>>,
}

impl<'a, 'tree> Resolver<'a, 'tree> {
    fn new(file_path: &'a str, source: &'a [u8], symbols: &'a [Symbol]) -> Self {
        let functions = symbols
            .iter()
            .filter(|symbol| matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method))
            .filter_map(|symbol| {
                Some((
                    (symbol.range.start_byte?, symbol.range.end_byte?),
                    symbol.id.as_str(),
                ))
            })
            .collect();
        Self {
            file: FileContext::new(file_path),
            source,
            functions,
            scopes: RefCell::default(),
            rebound: OnceCell::new(),
            class_body_calls: RefCell::default(),
            qualified_names: RefCell::default(),
        }
    }

    /// The edge of a call. It comes from the innermost enclosing function, or from the file
    /// for module-level code.
    fn call_edge(&self, call: Node<'tree>, enclosing: Enclosing<'_, 'tree>) -> Option<SymbolEdge> {
        let target = self.call_target(call, enclosing)?;
        let source_id = enclosing
            .scopes
            .iter()
            .rev()
            .map(|&index| enclosing.nodes[index])
            .filter(|node| node.kind() == "function_definition")
            .find_map(|node| self.functions.get(&(node.start_byte(), node.end_byte())))
            .map_or_else(
                || file_source_id(self.file.file_path),
                |id| (*id).to_owned(),
            );
        Some(SymbolEdge {
            source_id,
            target_qualified_name: target,
            edge_kind: EdgeKind::Calls,
            file_path: self.file.file_path.to_owned(),
        })
    }

    /// Spells a call's target the way Python symbols are qualified (`module::name`,
    /// `module::Class::method`) whenever the file shows where the callee lives:
    /// - `f()` for a `def` or `class` in this file, or a name imported with
    ///   `from module import f [as g]`;
    /// - `self.m()` and `cls.m()` in a method of a class that defines `m`, and `C.m()` for a
    ///   class `C` in this file that defines `m`;
    /// - `mod.f()` for a module imported with `import pkg.mod [as mod]` or
    ///   `from pkg import mod`, and `C.m()` for a class imported with `from pkg.mod import C`.
    ///
    /// Members of the standard library keep Python's dotted spelling (`os.path.join`), which
    /// no workspace symbol is qualified as. Any other call has no target: a builtin, a
    /// parameter, a local, anything the file doesn't define or import, a method on a value
    /// of unknown type, and a call of an expression that isn't a name. A bare method name
    /// can't name a Python symbol, but the store resolves targets across languages, where it
    /// could name an unrelated function.
    fn call_target(&self, call: Node<'tree>, enclosing: Enclosing<'_, 'tree>) -> Option<String> {
        let callee = call.child_by_field_name("function")?;
        match callee.kind() {
            "identifier" => {
                let name = node_text(callee, self.source);
                match self.resolve_name(call, enclosing, &name)? {
                    Binding::Definition(definition) => self.definition_qualified_name(definition),
                    Binding::Imported { module, name } => module.member(&name),
                    Binding::Module(_) | Binding::Receiver(_) | Binding::Value => None,
                }
            }
            "attribute" => self.member_call_target(call, callee, enclosing),
            _ => None,
        }
    }

    /// The qualified target of `receiver.member()` when the receiver is a dotted name whose
    /// first segment the file binds to a class or a module.
    fn member_call_target(
        &self,
        call: Node<'tree>,
        callee: Node<'tree>,
        enclosing: Enclosing<'_, 'tree>,
    ) -> Option<String> {
        let member = node_text(callee.child_by_field_name("attribute")?, self.source);
        let path = dotted_path(callee.child_by_field_name("object")?, self.source)?;
        let (head, rest) = path.split_first()?;
        match self.resolve_name(call, enclosing, head)? {
            Binding::Definition(class) | Binding::Receiver(class)
                if class.kind() == "class_definition" =>
            {
                let mut class = class;
                for segment in rest {
                    class = self
                        .class_member(class, segment)
                        .filter(|nested| nested.kind() == "class_definition")?;
                }
                let definition = self.class_member(class, &member)?;
                self.definition_qualified_name(definition)
            }
            Binding::Module(module) if module.stdlib => {
                Some(stdlib_member(&module.path, rest, &member))
            }
            Binding::Imported { module, name } if module.stdlib => Some(stdlib_member(
                &format!("{}.{name}", module.path),
                rest,
                &member,
            )),
            Binding::Module(module) => {
                module_member(module_segments(&module.path), Vec::new(), rest, &member)
            }
            Binding::Imported { module, name } => {
                let mut modules = module_segments(&module.path);
                let mut classes = Vec::new();
                if is_class_name(&name) {
                    classes.push(name);
                } else {
                    modules.push(name);
                }
                module_member(modules, classes, rest, &member)
            }
            _ => None,
        }
    }

    /// What `name` refers to at `at`, following Python's scoping rules: the enclosing
    /// functions, lambdas and comprehensions innermost first, the class body for code
    /// directly inside one, then the module. `None` when nothing in the file binds the name:
    /// a builtin, a name from a `*` import, or an undefined one.
    fn resolve_name(
        &self,
        at: Node<'tree>,
        enclosing: Enclosing<'_, 'tree>,
        name: &str,
    ) -> Option<Binding<'tree>> {
        let root = *enclosing.nodes.first()?;
        // A class body is visible to the code directly inside it, but not to the functions,
        // lambdas, comprehensions and classes it contains.
        let mut class_visible = true;
        // Code directly in a module or class body runs while that body runs, so it sees only
        // the names bound before it. Code in a function or lambda runs once they are
        // complete.
        let mut runs_inline = true;
        // A name a class body binds, though not yet, is looked up in the module next: the
        // functions around the class are skipped.
        let mut module_next = false;
        for &index in enclosing.scopes.iter().rev() {
            let scope = enclosing.nodes[index];
            let child = enclosing.nodes.get(index + 1).copied().unwrap_or(at);
            // Default values, decorators and base classes are evaluated in the enclosing
            // scope.
            let in_body = scope.child_by_field_name("body") == Some(child);
            match scope.kind() {
                "function_definition" if in_body => {
                    if !module_next {
                        let found = self.bindings(child, root, name, None);
                        if found.global {
                            return self.bindings(root, root, name, None).binding();
                        }
                        if !found.nonlocal {
                            let function_enclosing = &enclosing.nodes[..index];
                            let parameter = self.parameter_binding(scope, function_enclosing, name);
                            let binding = match parameter {
                                Some(_) if found.is_bound() => Some(Binding::Value),
                                Some(parameter) => Some(parameter),
                                None => found.binding(),
                            };
                            if binding.is_some() {
                                return binding;
                            }
                        }
                    }
                    class_visible = false;
                    runs_inline = false;
                }
                "lambda" if in_body => {
                    if !module_next
                        && scope
                            .child_by_field_name("parameters")
                            .is_some_and(|parameters| {
                                parameters_bind(parameters, name, self.source)
                            })
                    {
                        return Some(Binding::Value);
                    }
                    class_visible = false;
                    runs_inline = false;
                }
                "list_comprehension"
                | "set_comprehension"
                | "dictionary_comprehension"
                | "generator_expression" => {
                    // The first iterable is evaluated in the enclosing scope.
                    let mut cursor = scope.walk();
                    let clauses = scope
                        .named_children(&mut cursor)
                        .filter(|clause| clause.kind() == "for_in_clause")
                        .collect::<Vec<_>>();
                    let in_first_iterable = clauses.first().is_some_and(|first| {
                        first.child_by_field_name("right").is_some_and(|right| {
                            right.start_byte() <= at.start_byte()
                                && at.end_byte() <= first.end_byte()
                        })
                    });
                    if !in_first_iterable {
                        if !module_next
                            && clauses.iter().any(|clause| {
                                clause.child_by_field_name("left").is_some_and(|left| {
                                    pattern_names(left)
                                        .into_iter()
                                        .any(|bound| text_is(bound, self.source, name))
                                })
                            })
                        {
                            return Some(Binding::Value);
                        }
                        class_visible = false;
                    }
                }
                "class_definition" if in_body => {
                    if class_visible && !module_next {
                        let index = self.scope(child);
                        let before = runs_inline.then(|| at.start_byte());
                        if let Some(binding) = index.lookup(name, before).binding() {
                            return Some(binding);
                        }
                        module_next = index.lookup(name, None).is_bound();
                    }
                    class_visible = false;
                }
                "module" => {
                    let before = runs_inline.then(|| at.start_byte());
                    return self.bindings(scope, root, name, before).binding();
                }
                _ => {}
            }
        }
        None
    }

    /// The bindings of a scope's body, indexed on first use.
    fn scope(&self, body: Node<'tree>) -> Rc<ScopeIndex<'tree>> {
        if let Some(index) = self.scopes.borrow().get(&body.id()) {
            return Rc::clone(index);
        }
        let index = Rc::new(ScopeIndex::build(body, self.source, &self.file));
        self.scopes
            .borrow_mut()
            .insert(body.id(), Rc::clone(&index));
        index
    }

    /// How a module or function body binds `name`, counting the rebindings made from inner
    /// functions through `global` or `nonlocal`, which may happen at any time.
    fn bindings(
        &self,
        body: Node<'tree>,
        root: Node<'tree>,
        name: &str,
        before: Option<usize>,
    ) -> ScopeBindings<'tree> {
        let mut found = self.scope(body).lookup(name, before);
        if self
            .rebound(root)
            .get(&body.id())
            .is_some_and(|names| names.contains(name))
        {
            found.assigned = true;
        }
        found
    }

    /// Names that functions rebind in an enclosing scope, by that scope's body. A function
    /// that declares `global name` and binds it rebinds the module's `name`; one that
    /// declares `nonlocal name` and binds it rebinds `name` in the nearest enclosing function
    /// that binds it.
    fn rebound(&self, root: Node<'tree>) -> &HashMap<usize, HashSet<String>> {
        self.rebound.get_or_init(|| {
            let mut rebound = HashMap::<usize, HashSet<String>>::new();
            let mut ancestors = Vec::new();
            let mut cursor = root.walk();
            loop {
                let node = cursor.node();
                let is_global = node.kind() == "global_statement";
                if is_global || node.kind() == "nonlocal_statement" {
                    let bodies = ancestors
                        .iter()
                        .rev()
                        .filter(|ancestor: &&Node<'tree>| ancestor.kind() == "function_definition")
                        .filter_map(|function| function.child_by_field_name("body"))
                        .collect::<Vec<_>>();
                    if let Some((&declaring, outer)) = bodies.split_first() {
                        let mut names = node.walk();
                        for declared in node.named_children(&mut names) {
                            let name = node_text(declared, self.source);
                            if !self.scope(declaring).lookup(&name, None).is_bound() {
                                continue;
                            }
                            let target = if is_global {
                                Some(root)
                            } else {
                                outer
                                    .iter()
                                    .copied()
                                    .find(|body| self.scope(*body).lookup(&name, None).is_bound())
                            };
                            if let Some(target) = target {
                                rebound.entry(target.id()).or_default().insert(name);
                            }
                        }
                    }
                }
                if cursor.goto_first_child() {
                    ancestors.push(node);
                    continue;
                }
                while !cursor.goto_next_sibling() {
                    if !cursor.goto_parent() {
                        return rebound;
                    }
                    ancestors.pop();
                }
            }
        })
    }

    /// What a function's parameter list binds `name` to: the receiver for the first
    /// parameter of a method, a plain value for any other parameter. `enclosing` holds the
    /// function's enclosing nodes.
    fn parameter_binding(
        &self,
        function: Node<'tree>,
        enclosing: &[Node<'tree>],
        name: &str,
    ) -> Option<Binding<'tree>> {
        let parameters = function.child_by_field_name("parameters")?;
        let mut cursor = parameters.walk();
        let entries = parameters
            .named_children(&mut cursor)
            .filter(|entry| entry.kind() != "comment")
            .collect::<Vec<_>>();
        let position = entries.iter().position(|entry| {
            parameter_pattern(*entry).is_some_and(|pattern| {
                pattern_names(pattern)
                    .into_iter()
                    .any(|bound| text_is(bound, self.source, name))
            })
        })?;
        // `def m(*, store)` has no receiver: a separator comes first.
        if position == 0
            && entries[0].kind() != "list_splat_pattern"
            && entries[0].kind() != "dictionary_splat_pattern"
            && parameter_pattern(entries[0]).is_some_and(|pattern| pattern.kind() == "identifier")
            && let Some(class) = self.receiver_class(function, enclosing)
        {
            return Some(Binding::Receiver(class));
        }
        Some(Binding::Value)
    }

    /// The class whose instance, or whose class object in a `@classmethod`, a method's first
    /// parameter receives. `None` for a function that isn't a method, for a `@staticmethod`,
    /// for a method whose name the class body also binds another way
    /// (`m = staticmethod(m)`), and for a helper the class body calls while it runs, such as
    /// a decorator applied to its other methods.
    fn receiver_class(
        &self,
        function: Node<'tree>,
        enclosing: &[Node<'tree>],
    ) -> Option<Node<'tree>> {
        // The nearest enclosing class, unless a function comes first, as
        // `is_method_definition` decides.
        let class = enclosing.iter().rev().find_map(|node| match node.kind() {
            "class_definition" => Some(Some(*node)),
            "function_definition" => Some(None),
            _ => None,
        })??;
        let is_static = enclosing
            .last()
            .filter(|parent| parent.kind() == "decorated_definition")
            .is_some_and(|decorated| {
                let mut cursor = decorated.walk();
                decorated
                    .named_children(&mut cursor)
                    .filter(|child| child.kind() == "decorator")
                    .filter_map(|decorator| {
                        let mut cursor = decorator.walk();
                        decorator
                            .named_children(&mut cursor)
                            .find(|expression| expression.kind() != "comment")
                    })
                    .any(|expression| {
                        let text = compact(&node_text(expression, self.source));
                        text == "staticmethod" || text.ends_with(".staticmethod")
                    })
            });
        if is_static {
            return None;
        }
        let name = node_text(function.child_by_field_name("name")?, self.source);
        let body = class.child_by_field_name("body")?;
        let rebound = self.scope(body).lookup(&name, None);
        let used_by_body = self.class_body_calls(body).contains(&name);
        (rebound.imports.is_empty() && !rebound.assigned && !used_by_body).then_some(class)
    }

    /// The names a class body calls or applies as decorators while it runs, in its
    /// statements, its methods' decorators and default values, and its nested classes' base
    /// classes. A function used that way is a plain function, not a method.
    fn class_body_calls(&self, body: Node<'tree>) -> Rc<HashSet<String>> {
        if let Some(names) = self.class_body_calls.borrow().get(&body.id()) {
            return Rc::clone(names);
        }
        let callee_name = |expression: Node<'tree>| match expression.kind() {
            "identifier" => Some(node_text(expression, self.source)),
            "call" => expression
                .child_by_field_name("function")
                .filter(|function| function.kind() == "identifier")
                .map(|function| node_text(function, self.source)),
            _ => None,
        };
        let mut names = HashSet::new();
        let mut stack = vec![body];
        while let Some(node) = stack.pop() {
            match node.kind() {
                "decorator" => {
                    let mut cursor = node.walk();
                    names.extend(
                        node.named_children(&mut cursor)
                            .find(|expression| expression.kind() != "comment")
                            .and_then(callee_name),
                    );
                }
                "call" => names.extend(callee_name(node)),
                "function_definition" => {
                    if let Some(parameters) = node.child_by_field_name("parameters") {
                        let mut cursor = parameters.walk();
                        stack.extend(
                            parameters
                                .named_children(&mut cursor)
                                .filter_map(|parameter| parameter.child_by_field_name("value")),
                        );
                    }
                    continue;
                }
                "class_definition" => {
                    stack.extend(node.child_by_field_name("superclasses"));
                    continue;
                }
                "lambda" => continue,
                _ => {}
            }
            let mut cursor = node.walk();
            stack.extend(node.named_children(&mut cursor));
        }
        let names = Rc::new(names);
        self.class_body_calls
            .borrow_mut()
            .insert(body.id(), Rc::clone(&names));
        names
    }

    /// The `def` or nested `class` a class body binds `name` to.
    fn class_member(&self, class: Node<'tree>, name: &str) -> Option<Node<'tree>> {
        let body = class.child_by_field_name("body")?;
        match self.scope(body).lookup(name, None).binding()? {
            Binding::Definition(definition) => Some(definition),
            _ => None,
        }
    }

    /// The qualified name the symbol mapper gives a `def` or `class`. `None` for a method of
    /// a class declared inside a function: its name leaves the function out (`def f(): class
    /// C: def m` is `C::m`), so it may be shared with an unrelated method.
    fn definition_qualified_name(&self, definition: Node<'tree>) -> Option<String> {
        if let Some(name) = self.qualified_names.borrow().get(&definition.id()) {
            return name.clone();
        }
        let qualified = named_child_text(definition, "name", self.source).and_then(|name| {
            match definition.kind() {
                "function_definition" => (!(is_method_definition(definition)
                    && has_ancestor_kind(definition, "function_definition")))
                .then(|| {
                    function_qualified_name(definition, &name, self.source, self.file.file_path)
                }),
                "class_definition" => Some(class_qualified_name(
                    definition,
                    &name,
                    self.source,
                    self.file.file_path,
                )),
                _ => None,
            }
        });
        self.qualified_names
            .borrow_mut()
            .insert(definition.id(), qualified.clone());
        qualified
    }
}

/// A standard-library member as Python writes it: `os.path.join`.
fn stdlib_member(module: &str, rest: &[String], member: &str) -> String {
    let mut path = vec![module.to_owned()];
    path.extend(rest.iter().cloned());
    path.push(member.to_owned());
    path.join(".")
}

/// `module.Class.member` spelled `module::Class::member`. Lowercase segments extend the
/// module path and the first capitalized one starts the class path, as PEP 8 names them.
/// `None` for a method of a class attribute (`module.Class.attr.method`).
fn module_member(
    mut modules: Vec<String>,
    mut classes: Vec<String>,
    rest: &[String],
    member: &str,
) -> Option<String> {
    for segment in rest {
        if is_class_name(segment) {
            classes.push(segment.clone());
        } else if classes.is_empty() {
            modules.push(segment.clone());
        } else {
            return None;
        }
    }
    if modules.is_empty() {
        return None;
    }
    classes.push(member.to_owned());
    Some(format!("{}::{}", modules.join("."), classes.join("::")))
}

/// The names bound by `name :=` in the text of `scope`, with the offset where each binding
/// is complete, leaving out the `nested` byte ranges.
fn walrus_targets(
    scope: Node<'_>,
    source: &[u8],
    nested: &[(usize, usize)],
) -> Vec<(String, usize)> {
    let start = scope.start_byte();
    let Some(text) = source.get(start..scope.end_byte().min(source.len())) else {
        return Vec::new();
    };
    let is_identifier = |byte: u8| byte == b'_' || byte.is_ascii_alphanumeric() || byte >= 0x80;
    let mut targets = Vec::new();
    for index in 1..text.len() {
        if text[index - 1] != b':' || text[index] != b'=' {
            continue;
        }
        let offset = start + index;
        if nested
            .iter()
            .any(|(nested_start, nested_end)| (*nested_start..*nested_end).contains(&offset))
        {
            continue;
        }
        let mut stop = index - 1;
        while stop > 0 && text[stop - 1].is_ascii_whitespace() {
            stop -= 1;
        }
        let mut begin = stop;
        while begin > 0 && is_identifier(text[begin - 1]) {
            begin -= 1;
        }
        if begin < stop {
            targets.push((
                String::from_utf8_lossy(&text[begin..stop]).into_owned(),
                offset + 1,
            ));
        }
    }
    targets
}

/// The names a `case` pattern captures: `case name`, `[name, *name]`, `{"k": name}`,
/// `Point(x=name)`, `… as name`. Dotted names are values, and a class pattern's class and a
/// keyword pattern's keyword are not captures.
fn case_pattern_captures(pattern: Node<'_>) -> Vec<Node<'_>> {
    let mut captures = Vec::new();
    let mut stack = vec![pattern];
    while let Some(node) = stack.pop() {
        let mut cursor = node.walk();
        let children = node.named_children(&mut cursor).collect::<Vec<_>>();
        match node.kind() {
            "identifier" => captures.push(node),
            "dotted_name" => {
                if let [part] = children.as_slice() {
                    captures.push(*part);
                }
            }
            "class_pattern" | "keyword_pattern" => stack.extend(children.into_iter().skip(1)),
            _ => stack.extend(children),
        }
    }
    captures
}

/// The names an `import` or `from … import` statement binds, each with what it binds it to.
/// A name imported from a module the file can't spell (a relative import past the top-level
/// package) is a plain value.
fn import_bindings<'tree>(
    statement: Node<'tree>,
    source: &[u8],
    file: &FileContext<'_>,
) -> Vec<(Node<'tree>, Binding<'tree>)> {
    let from_module = (statement.kind() == "import_from_statement").then(|| {
        statement
            .child_by_field_name("module_name")
            .and_then(|module| file.module(&node_text(module, source)))
    });
    let mut cursor = statement.walk();
    statement
        .children_by_field_name("name", &mut cursor)
        .filter_map(|item| {
            let (bound, path) = match item.kind() {
                "aliased_import" => (
                    item.child_by_field_name("alias")?,
                    item.child_by_field_name("name")?,
                ),
                // `import pkg.mod` binds `pkg`; `from module import name` binds `name`.
                "dotted_name" => (item.named_child(0)?, item),
                _ => return None,
            };
            let binding = match &from_module {
                Some(module) => module.clone().map(|module| Binding::Imported {
                    module,
                    name: compact(&node_text(path, source)),
                }),
                None if bound.parent() == Some(path) => {
                    file.module(&node_text(bound, source)).map(Binding::Module)
                }
                None => file.module(&node_text(path, source)).map(Binding::Module),
            };
            Some((bound, binding.unwrap_or(Binding::Value)))
        })
        .collect()
}

fn parameters_bind(parameters: Node<'_>, name: &str, source: &[u8]) -> bool {
    let mut cursor = parameters.walk();
    parameters
        .named_children(&mut cursor)
        .filter_map(parameter_pattern)
        .flat_map(pattern_names)
        .any(|bound| text_is(bound, source, name))
}

/// The part of a parameter that names it, without its type annotation or default value.
fn parameter_pattern(parameter: Node<'_>) -> Option<Node<'_>> {
    match parameter.kind() {
        "default_parameter" | "typed_default_parameter" => parameter.child_by_field_name("name"),
        "typed_parameter" => {
            let mut cursor = parameter.walk();
            parameter
                .named_children(&mut cursor)
                .find(|child| !matches!(child.kind(), "type" | "comment"))
        }
        "keyword_separator" | "positional_separator" | "comment" => None,
        _ => Some(parameter),
    }
}

/// The identifiers an assignment target binds. Attributes and subscripts bind nothing.
fn pattern_names(pattern: Node<'_>) -> Vec<Node<'_>> {
    let mut names = Vec::new();
    let mut stack = vec![pattern];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "identifier" => names.push(node),
            "attribute" | "subscript" => {}
            _ => {
                let mut cursor = node.walk();
                stack.extend(node.named_children(&mut cursor));
            }
        }
    }
    names
}

/// `a.b.c` as `[a, b, c]`; `None` for any expression other than a dotted name.
fn dotted_path(node: Node<'_>, source: &[u8]) -> Option<Vec<String>> {
    let mut path = Vec::new();
    let mut current = node;
    while current.kind() == "attribute" {
        path.push(node_text(current.child_by_field_name("attribute")?, source));
        current = current.child_by_field_name("object")?;
    }
    if current.kind() != "identifier" {
        return None;
    }
    path.push(node_text(current, source));
    path.reverse();
    Some(path)
}

fn module_segments(module: &str) -> Vec<String> {
    module
        .split('.')
        .filter(|segment| !segment.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// PEP 8 names classes in CapWords and modules in lowercase.
fn is_class_name(name: &str) -> bool {
    name.trim_start_matches('_')
        .starts_with(|ch: char| ch.is_uppercase())
}

fn text_is(node: Node<'_>, source: &[u8], text: &str) -> bool {
    source.get(node.start_byte()..node.end_byte()) == Some(text.as_bytes())
}

/// A dotted name as written, without the whitespace Python allows around its dots.
fn compact(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_whitespace()).collect()
}

fn python_import_targets(node: Node<'_>, source: &[u8], file_path: &str) -> Vec<String> {
    let text = node_text(node, source);
    match node.kind() {
        "import_statement" => parse_import_statement(&text),
        "import_from_statement" => parse_import_from_statement(&text, file_path),
        _ => Vec::new(),
    }
}

fn python_docstring(node: Node<'_>, source: &[u8]) -> Option<String> {
    let body = node.child_by_field_name("body")?;
    let first_statement = body.named_child(0)?;
    if first_statement.kind() != "expression_statement" {
        return None;
    }
    let literal = first_statement.named_child(0)?;
    if literal.kind() != "string" {
        return None;
    }

    let raw = node_text(literal, source);
    let stripped = raw
        .trim()
        .trim_start_matches("r\"\"\"")
        .trim_start_matches("u\"\"\"")
        .trim_start_matches("f\"\"\"")
        .trim_start_matches("\"\"\"")
        .trim_start_matches("'''")
        .trim_end_matches("\"\"\"")
        .trim_end_matches("'''")
        .trim();
    (!stripped.is_empty()).then(|| stripped.to_owned())
}

fn parse_import_statement(text: &str) -> Vec<String> {
    let Some(remainder) = text.trim().strip_prefix("import") else {
        return Vec::new();
    };
    remainder
        .split(',')
        .map(str::trim)
        .filter_map(|entry| {
            entry
                .split_once(" as ")
                .map_or(Some(entry), |(name, _)| Some(name))
        })
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_import_from_statement(text: &str, file_path: &str) -> Vec<String> {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("from") else {
        return Vec::new();
    };
    let rest = rest.trim();
    let Some((module_raw, names_raw)) = rest.split_once(" import ") else {
        return Vec::new();
    };

    let module = resolve_import_module(module_raw.trim(), file_path);
    let names = names_raw.trim();
    // Strip surrounding parentheses from multi-line imports:
    // from module import (a, b) -> a, b
    let names = names
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))
        .unwrap_or(names)
        .trim();
    if names == "*" {
        return (!module.is_empty()).then_some(module).into_iter().collect();
    }

    names
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            entry
                .split_once(" as ")
                .map_or(entry, |(name, _)| name)
                .trim()
        })
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            if module.is_empty() {
                entry.to_owned()
            } else {
                format!("{module}.{entry}")
            }
        })
        .collect()
}

fn resolve_import_module(raw_module: &str, file_path: &str) -> String {
    let module = raw_module.trim();
    if module.is_empty() {
        return String::new();
    }
    if !module.starts_with('.') {
        return module.to_owned();
    }

    let dot_count = module.chars().take_while(|ch| *ch == '.').count();
    let tail = module[dot_count..].trim_matches('.');
    let package = python_package_path(file_path);

    let mut segments = package
        .split('.')
        .filter(|segment| !segment.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let ascend = dot_count.saturating_sub(1);
    if ascend >= segments.len() {
        segments.clear();
    } else {
        let remaining = segments.len() - ascend;
        segments.truncate(remaining);
    }

    if !tail.is_empty() {
        segments.extend(
            tail.split('.')
                .filter(|segment| !segment.is_empty())
                .map(ToOwned::to_owned),
        );
    }

    segments.join(".")
}

fn has_ancestor_kind(node: Node<'_>, kind: &str) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        if parent.kind() == kind {
            return true;
        }
        current = parent.parent();
    }
    false
}

fn is_directly_decorated(node: Node<'_>) -> bool {
    node.parent()
        .is_some_and(|parent| parent.kind() == "decorated_definition")
}

fn is_method_definition(node: Node<'_>) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        match parent.kind() {
            "class_definition" => return true,
            "function_definition" => return false,
            _ => current = parent.parent(),
        }
    }
    false
}

fn nearest_ancestor_name(node: Node<'_>, source: &[u8], kinds: &[&str]) -> Option<String> {
    let mut parts = Vec::new();
    let mut current = node.parent();
    while let Some(parent) = current {
        if kinds.iter().any(|kind| *kind == parent.kind())
            && let Some(name) = named_child_text(parent, "name", source)
        {
            parts.push(name);
        }
        current = parent.parent();
    }
    if parts.is_empty() {
        return None;
    }
    // Ancestors are collected inner-to-outer; reverse for outer-to-inner.
    parts.reverse();
    Some(parts.join("::"))
}

fn find_inner_definition(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == "function_definition" || child.kind() == "class_definition")
}

#[cfg(test)]
mod tests {
    use super::{nearest_ancestor_name, parse_import_from_statement};

    #[test]
    fn parenthesized_import() {
        let edges = parse_import_from_statement("from module import (a, b)", "src/main.py");
        assert_eq!(edges, vec!["module.a".to_owned(), "module.b".to_owned()]);
    }

    #[test]
    fn parenthesized_import_with_whitespace() {
        let edges =
            parse_import_from_statement("from module import (\n    a,\n    b,\n)", "src/main.py");
        assert_eq!(edges, vec!["module.a".to_owned(), "module.b".to_owned()]);
    }

    #[test]
    fn nested_class_method_qualified_name() {
        let source = r#"
class Outer:
    class Inner:
        def method(self):
            pass
"#;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .expect("set language");
        let tree = parser.parse(source, None).expect("parse");
        let root = tree.root_node();

        let outer = root.named_child(0).expect("Outer class");
        let body = outer.child_by_field_name("body").expect("outer body");
        let inner = body
            .named_children(&mut body.walk())
            .find(|n| n.kind() == "class_definition")
            .expect("Inner class");
        let inner_body = inner.child_by_field_name("body").expect("inner body");
        let method = inner_body
            .named_children(&mut inner_body.walk())
            .find(|n| n.kind() == "function_definition")
            .expect("method");

        let result = nearest_ancestor_name(method, source.as_bytes(), &["class_definition"]);
        assert_eq!(result, Some("Outer::Inner".to_owned()));
    }
}
