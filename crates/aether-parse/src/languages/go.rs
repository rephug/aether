use std::collections::{HashMap, HashSet};

use aether_core::{
    EdgeKind, Language, Symbol, SymbolEdge, SymbolKind, content_hash, file_source_id,
    signature_fingerprint, stable_symbol_id,
};
use tree_sitter::{Node, Query};

use crate::parser::{
    TestIntent, build_symbol, enclosing_function_symbol_id, named_child_text, node_range,
    node_text, normalize_intent_text,
};
use crate::registry::{LanguageConfig, LanguageHooks, QueryCaptures};

pub fn config() -> LanguageConfig {
    let ts_language = tree_sitter_go::LANGUAGE.into();
    let symbol_query = Query::new(&ts_language, include_str!("../queries/go_symbols.scm"))
        .expect("invalid go symbol query");
    let edge_query = Query::new(&ts_language, include_str!("../queries/go_edges.scm"))
        .expect("invalid go edge query");
    let test_intent_query =
        Query::new(&ts_language, include_str!("../queries/go_test_intents.scm"))
            .expect("invalid go test intent query");

    LanguageConfig {
        id: "go",
        extensions: &["go"],
        ts_language,
        symbol_query,
        edge_query,
        test_intent_query: Some(test_intent_query),
        module_markers: &["go.mod"],
        hooks: Some(Box::new(GoHooks)),
    }
}

struct GoHooks;

/// Predeclared Go types. References to them never resolve to workspace symbols.
const BUILTIN_TYPES: &[&str] = &[
    "any",
    "bool",
    "byte",
    "comparable",
    "complex64",
    "complex128",
    "error",
    "float32",
    "float64",
    "int",
    "int8",
    "int16",
    "int32",
    "int64",
    "rune",
    "string",
    "uint",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "uintptr",
];

/// Predeclared Go functions.
const BUILTIN_FUNCTIONS: &[&str] = &[
    "append", "cap", "clear", "close", "complex", "copy", "delete", "imag", "len", "make", "max",
    "min", "new", "panic", "print", "println", "real", "recover",
];

/// First element of every standard-library import path, plus cgo's pseudo-package `C`.
/// Calls and receiver types from these packages are spelled the Go way (`fmt.Errorf`,
/// `testing.T::Run`), which no qualified name contains, so they can't resolve to a workspace
/// package that happens to share the name (`log`, `errors`). Their types are not recorded
/// as type references at all.
const STDLIB_ROOTS: &[&str] = &[
    "C",
    "archive",
    "bufio",
    "bytes",
    "cmp",
    "compress",
    "container",
    "context",
    "crypto",
    "database",
    "debug",
    "embed",
    "encoding",
    "errors",
    "expvar",
    "flag",
    "fmt",
    "go",
    "hash",
    "html",
    "image",
    "index",
    "io",
    "iter",
    "log",
    "maps",
    "math",
    "mime",
    "net",
    "os",
    "path",
    "plugin",
    "reflect",
    "regexp",
    "runtime",
    "slices",
    "sort",
    "strconv",
    "strings",
    "structs",
    "sync",
    "syscall",
    "testing",
    "text",
    "time",
    "unicode",
    "unique",
    "unsafe",
    "weak",
];

/// What a Go file can name: its own package, the packages it imports and the functions it
/// declares at package level.
struct FileScope {
    package: Option<String>,
    /// Local name in this file → the imported package.
    imports: HashMap<String, ImportedPackage>,
    functions: HashSet<String>,
}

struct ImportedPackage {
    /// The package name its own declarations are qualified with.
    name: String,
    is_stdlib: bool,
}

impl ImportedPackage {
    /// A member of the package as a call target or type: `db::Open`, or `fmt.Errorf` for the
    /// standard library.
    fn member(&self, member: &str) -> String {
        if self.is_stdlib {
            format!("{}.{member}", self.name)
        } else {
            format!("{}::{member}", self.name)
        }
    }
}

impl FileScope {
    fn of(node: Node<'_>, source: &[u8]) -> Self {
        let mut root = node;
        while let Some(parent) = root.parent() {
            root = parent;
        }

        let mut scope = Self {
            package: None,
            imports: HashMap::new(),
            functions: HashSet::new(),
        };
        let mut cursor = root.walk();
        for child in root.named_children(&mut cursor) {
            match child.kind() {
                "package_clause" => {
                    let mut inner = child.walk();
                    scope.package = child
                        .named_children(&mut inner)
                        .find(|name| name.kind() == "package_identifier")
                        .map(|name| node_text(name, source).trim().to_owned())
                        .filter(|name| !name.is_empty());
                }
                "import_declaration" => scope.add_imports(child, source),
                "function_declaration" => {
                    if let Some(name) = named_child_text(child, "name", source) {
                        scope.functions.insert(name);
                    }
                }
                _ => {}
            }
        }
        scope
    }

    fn add_imports(&mut self, declaration: Node<'_>, source: &[u8]) {
        let mut stack = vec![declaration];
        while let Some(node) = stack.pop() {
            if node.kind() != "import_spec" {
                let mut cursor = node.walk();
                stack.extend(node.named_children(&mut cursor));
                continue;
            }
            let Some(path) = node
                .child_by_field_name("path")
                .and_then(|path| import_path(path, source))
            else {
                continue;
            };
            let Some((names, declared)) = import_package_names(&path) else {
                continue;
            };
            let is_stdlib = STDLIB_ROOTS.contains(&path.split('/').next().unwrap_or_default());
            let alias = node
                .child_by_field_name("name")
                .map(|name| node_text(name, source).trim().to_owned());
            match alias.as_deref() {
                // Blank and dot imports bind no qualifier.
                Some("_" | ".") => {}
                Some(alias) if !alias.is_empty() => {
                    self.imports.insert(
                        alias.to_owned(),
                        ImportedPackage {
                            name: declared,
                            is_stdlib,
                        },
                    );
                }
                _ => {
                    // Without an alias the qualifier is the package's declared name itself.
                    for name in names {
                        self.imports
                            .insert(name.clone(), ImportedPackage { name, is_stdlib });
                    }
                }
            }
        }
    }

    /// Qualifies a declaration of this file's package: `store::Store::Get`.
    fn qualify(&self, path: &[&str]) -> String {
        let joined = path.join("::");
        match &self.package {
            Some(package) => format!("{package}::{joined}"),
            None => joined,
        }
    }
}

impl LanguageHooks for GoHooks {
    fn map_symbol(
        &self,
        language: Language,
        captures: &QueryCaptures<'_, '_>,
        source: &[u8],
        file_path: &str,
    ) -> Option<Symbol> {
        if is_testdata(file_path) {
            return None;
        }
        let capture_name = captures.first_capture_name_with_prefix("symbol.")?;
        let node = captures.node_with_prefix("symbol.")?;
        let name = named_child_text(node, "name", source)?;
        if name == "_" {
            return None;
        }
        let scope = FileScope::of(node, source);
        // A file without a package clause is not Go package code (a snippet or a file
        // mid-edit), and its names would have no package to qualify them.
        scope.package.as_ref()?;

        match capture_name {
            "symbol.go_function" => Some(build_symbol(
                language,
                file_path,
                SymbolKind::Function,
                &name,
                &scope.qualify(&[&name]),
                node,
                source,
            )),
            "symbol.go_method" => {
                let qualified_name = match receiver_type_name(node, source) {
                    Some(receiver) => scope.qualify(&[&receiver, &name]),
                    None => scope.qualify(&[&name]),
                };
                Some(build_symbol(
                    language,
                    file_path,
                    SymbolKind::Method,
                    &name,
                    &qualified_name,
                    node,
                    source,
                ))
            }
            "symbol.go_type" | "symbol.go_type_alias" => {
                // Types declared inside a function body are local helpers, not package API.
                if is_inside_function(node) {
                    return None;
                }
                Some(build_type_symbol(
                    language,
                    file_path,
                    capture_name == "symbol.go_type_alias",
                    &name,
                    &scope.qualify(&[&name]),
                    node,
                    source,
                ))
            }
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
        // Always answer with a list, even an empty one, so the generic mapper never turns a
        // skipped node into an edge built from its raw text.
        if is_testdata(file_path) {
            return Some(Vec::new());
        }
        let capture_name = captures.first_capture_name_with_prefix("edge.")?;
        let node = captures.node_with_prefix("edge.")?;
        if !has_package_clause(node) {
            return Some(Vec::new());
        }

        let edges = match capture_name {
            "edge.call" => call_edge(node, source, file_path, symbols)
                .into_iter()
                .collect(),
            "edge.depends_on" => captures
                .node("source")
                .and_then(|path| import_path(path, source))
                .map(|target| SymbolEdge {
                    source_id: file_source_id(file_path),
                    target_qualified_name: target,
                    edge_kind: EdgeKind::DependsOn,
                    file_path: file_path.to_owned(),
                })
                .into_iter()
                .collect(),
            "edge.type_ref" => type_ref_edges(node, source, file_path, symbols),
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
        symbols: &[Symbol],
    ) -> Option<Vec<TestIntent>> {
        if !file_path.ends_with("_test.go") || is_testdata(file_path) {
            return None;
        }
        let node = captures.node_with_prefix("test.")?;
        let test_name = named_child_text(node, "name", source)?;
        if !has_package_clause(node)
            || !is_test_function_name(&test_name)
            || !takes_testing_t(node, source)
        {
            return None;
        }

        let intent_text =
            doc_comment(node, source, &test_name).unwrap_or_else(|| humanize_test_name(&test_name));
        let intent_text = normalize_intent_text(&intent_text);
        if intent_text.is_empty() {
            return None;
        }

        Some(vec![TestIntent {
            file_path: file_path.to_owned(),
            test_name,
            intent_text,
            group_label: None,
            language,
            symbol_id: symbol_for_node(symbols, node).map(|symbol| symbol.id.clone()),
        }])
    }
}

fn has_package_clause(node: Node<'_>) -> bool {
    let mut root = node;
    while let Some(parent) = root.parent() {
        root = parent;
    }
    let mut cursor = root.walk();
    root.named_children(&mut cursor)
        .any(|child| child.kind() == "package_clause")
}

/// The go tool ignores `testdata` directories; their files are fixtures, often deliberately
/// broken, not package code.
fn is_testdata(file_path: &str) -> bool {
    file_path.split('/').any(|segment| segment == "testdata")
}

/// Builds a type symbol whose signature stops before the struct or interface body, so adding
/// a field or method keeps the symbol id stable, as it does for Rust structs and TypeScript
/// classes.
fn build_type_symbol(
    language: Language,
    file_path: &str,
    is_alias: bool,
    name: &str,
    qualified_name: &str,
    spec: Node<'_>,
    source: &[u8],
) -> Symbol {
    let definition = spec.child_by_field_name("type");
    let kind = match definition.map(|node| node.kind()) {
        Some("struct_type") if !is_alias => SymbolKind::Struct,
        Some("interface_type") if !is_alias => SymbolKind::Interface,
        _ => SymbolKind::TypeAlias,
    };

    // A lone `type X ...` declaration keeps its `type` keyword; specs inside a grouped
    // `type ( ... )` block stand alone, however many the block holds.
    let node = spec
        .parent()
        .filter(|parent| parent.kind() == "type_declaration" && !has_token(*parent, "("))
        .unwrap_or(spec);
    // Structs and interfaces sign up to their `struct` / `interface` keyword token.
    let signature_end = definition
        .filter(|_| kind != SymbolKind::TypeAlias)
        .and_then(|definition| definition.child(0))
        .map_or(node.end_byte(), |keyword| keyword.end_byte())
        .clamp(node.start_byte(), node.end_byte());
    let signature_text = source
        .get(node.start_byte()..signature_end)
        .map(String::from_utf8_lossy)
        .unwrap_or_default();
    let signature = signature_fingerprint(&signature_text);

    Symbol {
        id: stable_symbol_id(language, file_path, kind, qualified_name, &signature),
        language,
        file_path: file_path.to_owned(),
        kind,
        name: name.to_owned(),
        qualified_name: qualified_name.to_owned(),
        signature_fingerprint: signature,
        content_hash: content_hash(&node_text(node, source)),
        range: node_range(node),
    }
}

fn has_token(node: Node<'_>, kind: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| !child.is_named() && child.kind() == kind)
}

fn call_edge(
    call: Node<'_>,
    source: &[u8],
    file_path: &str,
    symbols: &[Symbol],
) -> Option<SymbolEdge> {
    let source_id = enclosing_function_symbol_id(symbols, call)?;
    let target = call_target(call, source, &FileScope::of(call, source))?;
    Some(SymbolEdge {
        source_id,
        target_qualified_name: target,
        edge_kind: EdgeKind::Calls,
        file_path: file_path.to_owned(),
    })
}

/// Spells a call target the way Go symbols are qualified:
/// - `helper()` → `store::helper`, in the file's own package;
/// - `pkg.Func()` → `pkg::Func` for an imported package (under its declared name, not an
///   alias), or `fmt.Errorf` for the standard library;
/// - `x.Method()`, where `x` is a receiver or parameter of a named type → `pkg::Type::Method`;
/// - any other selector keeps the bare method name, which no Go symbol is qualified as.
///
/// Calls through a local variable, parameter or type parameter (`next(w, r)`, `cancel()`)
/// have no static target and are skipped, as are builtins and conversions to builtin types,
/// unless the file declares a function that shadows the builtin.
fn call_target(call: Node<'_>, source: &[u8], scope: &FileScope) -> Option<String> {
    let callee = call.child_by_field_name("function")?;
    match callee.kind() {
        "identifier" => {
            let name = node_text(callee, source).trim().to_owned();
            let builtin = BUILTIN_TYPES.contains(&name.as_str())
                || (BUILTIN_FUNCTIONS.contains(&name.as_str()) && !scope.functions.contains(&name));
            let dynamic = declares_local(call, &name, source)
                || enclosing_declaration(call).is_some_and(|declaration| {
                    generic_type_names(declaration, source).contains(&name)
                });
            (!name.is_empty() && !builtin && !dynamic).then(|| scope.qualify(&[&name]))
        }
        "selector_expression" => {
            let field = callee.child_by_field_name("field")?;
            let field = node_text(field, source).trim().to_owned();
            if field.is_empty() {
                return None;
            }
            let Some(operand) = callee
                .child_by_field_name("operand")
                .filter(|operand| operand.kind() == "identifier")
            else {
                return Some(field);
            };

            let operand = node_text(operand, source).trim().to_owned();
            if let Some(binding) = declared_binding_type(call, &operand, source, scope) {
                return Some(format!("{binding}::{field}"));
            }
            // A local variable may shadow an imported package's name.
            if declares_local(call, &operand, source) {
                return Some(field);
            }
            if let Some(package) = scope.imports.get(&operand) {
                return Some(package.member(&field));
            }
            Some(field)
        }
        _ => None,
    }
}

/// Whether `name` is a variable in scope at `node`: a receiver or parameter of an enclosing
/// function or closure, or a local declared earlier in an enclosing block or in the header
/// of an enclosing `if`, `for` or `switch`.
fn declares_local(node: Node<'_>, name: &str, source: &[u8]) -> bool {
    let mut child = node;
    let mut current = node.parent();
    while let Some(scope) = current {
        let declared =
            match scope.kind() {
                "function_declaration" | "method_declaration" | "func_literal" => {
                    ["receiver", "parameters"].iter().any(|field| {
                        scope.child_by_field_name(field).is_some_and(|list| {
                            let mut cursor = list.walk();
                            list.named_children(&mut cursor)
                                .any(|parameter| declares_name(parameter, name, source))
                        })
                    })
                }
                "block" => {
                    let mut cursor = scope.walk();
                    scope
                        .named_children(&mut cursor)
                        .take_while(|statement| statement.start_byte() < child.start_byte())
                        .any(|statement| declaration_binds(statement, name, source))
                }
                "if_statement"
                | "expression_switch_statement"
                | "type_switch_statement"
                | "for_statement" => {
                    let mut cursor = scope.walk();
                    scope
                        .named_children(&mut cursor)
                        .filter(|header| *header != child)
                        .any(|header| match header.kind() {
                            "for_clause" => header.child_by_field_name("initializer").is_some_and(
                                |initializer| declaration_binds(initializer, name, source),
                            ),
                            _ => declaration_binds(header, name, source),
                        })
                }
                _ => false,
            };
        if declared {
            return true;
        }
        if matches!(scope.kind(), "function_declaration" | "method_declaration") {
            return false;
        }
        child = scope;
        current = scope.parent();
    }
    false
}

/// Whether a `:=`, `var` or `range` declaration binds `name`.
fn declaration_binds(node: Node<'_>, name: &str, source: &[u8]) -> bool {
    let names_match = |list: Node<'_>| {
        let mut cursor = list.walk();
        list.named_children(&mut cursor)
            .any(|declared| node_text(declared, source).trim() == name)
    };
    match node.kind() {
        "short_var_declaration" | "range_clause" => {
            node.child_by_field_name("left").is_some_and(names_match)
        }
        "var_declaration" => {
            let mut stack = vec![node];
            while let Some(current) = stack.pop() {
                if current.kind() == "var_spec" {
                    let mut cursor = current.walk();
                    if current
                        .children_by_field_name("name", &mut cursor)
                        .any(|declared| node_text(declared, source).trim() == name)
                    {
                        return true;
                    }
                    continue;
                }
                let mut cursor = current.walk();
                stack.extend(current.named_children(&mut cursor));
            }
            false
        }
        _ => false,
    }
}

/// Finds the declared type of the receiver or parameter called `name` in the functions that
/// enclose `node`, innermost first, spelled as a qualified name. Local variables are not
/// tracked.
fn declared_binding_type(
    node: Node<'_>,
    name: &str,
    source: &[u8],
    scope: &FileScope,
) -> Option<String> {
    let generics = enclosing_declaration(node)
        .map(|declaration| generic_type_names(declaration, source))
        .unwrap_or_default();

    let mut current = node.parent();
    while let Some(function) = current {
        if matches!(
            function.kind(),
            "function_declaration" | "method_declaration" | "func_literal"
        ) {
            for field in ["receiver", "parameters"] {
                let Some(list) = function.child_by_field_name(field) else {
                    continue;
                };
                let mut cursor = list.walk();
                for parameter in list.named_children(&mut cursor) {
                    if !declares_name(parameter, name, source) {
                        continue;
                    }
                    // A variadic parameter is a slice, so it has no methods of its own.
                    if parameter.kind() != "parameter_declaration" {
                        return None;
                    }
                    let declared = parameter.child_by_field_name("type")?;
                    return named_type(declared, source, &generics, scope);
                }
            }
        }
        current = function.parent();
    }
    None
}

fn declares_name(parameter: Node<'_>, name: &str, source: &[u8]) -> bool {
    if !matches!(
        parameter.kind(),
        "parameter_declaration" | "variadic_parameter_declaration"
    ) {
        return false;
    }
    let mut cursor = parameter.walk();
    parameter
        .children_by_field_name("name", &mut cursor)
        .any(|declared| node_text(declared, source).trim() == name)
}

/// Resolves a declared type to the qualified name of the named type behind pointers,
/// parentheses and type arguments: `store::Store` for `*Store[K]` in package `store`,
/// `db::Conn` for `*db.Conn`. Builtins and type parameters have no declaration.
fn named_type(
    node: Node<'_>,
    source: &[u8],
    generics: &HashSet<String>,
    scope: &FileScope,
) -> Option<String> {
    match node.kind() {
        "type_identifier" => {
            let name = node_text(node, source).trim().to_owned();
            let skip = name.is_empty()
                || BUILTIN_TYPES.contains(&name.as_str())
                || generics.contains(&name);
            (!skip).then(|| scope.qualify(&[&name]))
        }
        "pointer_type" | "parenthesized_type" => node
            .named_child(0)
            .and_then(|inner| named_type(inner, source, generics, scope)),
        "generic_type" => node
            .child_by_field_name("type")
            .and_then(|inner| named_type(inner, source, generics, scope)),
        "qualified_type" => {
            let (package, name) = qualified_type_parts(node, source, scope)?;
            Some(package.member(&name))
        }
        _ => None,
    }
}

/// The imported package and type name of `pkg.Name`. A qualifier that matches no import is
/// taken as the package's own name.
fn qualified_type_parts(
    node: Node<'_>,
    source: &[u8],
    scope: &FileScope,
) -> Option<(ImportedPackage, String)> {
    let qualifier = node_text(node.child_by_field_name("package")?, source)
        .trim()
        .to_owned();
    let name = node_text(node.child_by_field_name("name")?, source)
        .trim()
        .to_owned();
    if qualifier.is_empty() || name.is_empty() {
        return None;
    }
    let package = match scope.imports.get(&qualifier) {
        Some(package) => ImportedPackage {
            name: package.name.clone(),
            is_stdlib: package.is_stdlib,
        },
        None => ImportedPackage {
            name: qualifier,
            is_stdlib: false,
        },
    };
    Some((package, name))
}

/// Emits one type-reference edge per type named in a function's parameters or results.
/// Builtins, standard-library types and type parameters are skipped; so is the receiver,
/// which is already the method's parent.
fn type_ref_edges(
    declaration: Node<'_>,
    source: &[u8],
    file_path: &str,
    symbols: &[Symbol],
) -> Vec<SymbolEdge> {
    let Some(source_symbol) = symbol_for_node(symbols, declaration) else {
        return Vec::new();
    };
    let generics = generic_type_names(declaration, source);
    let scope = FileScope::of(declaration, source);

    let mut targets = Vec::new();
    for field in ["parameters", "result"] {
        if let Some(node) = declaration.child_by_field_name(field) {
            collect_type_refs(node, source, &generics, &scope, &mut targets);
        }
    }
    targets.sort();
    targets.dedup();

    targets
        .into_iter()
        .map(|target| SymbolEdge {
            source_id: source_symbol.id.clone(),
            target_qualified_name: target,
            edge_kind: EdgeKind::TypeRef,
            file_path: file_path.to_owned(),
        })
        .collect()
}

fn collect_type_refs(
    node: Node<'_>,
    source: &[u8],
    generics: &HashSet<String>,
    scope: &FileScope,
    out: &mut Vec<String>,
) {
    match node.kind() {
        "type_identifier" => {
            let name = node_text(node, source).trim().to_owned();
            if !name.is_empty()
                && !BUILTIN_TYPES.contains(&name.as_str())
                && !generics.contains(&name)
            {
                out.push(scope.qualify(&[&name]));
            }
        }
        "qualified_type" => {
            if let Some((package, name)) = qualified_type_parts(node, source, scope)
                && !package.is_stdlib
            {
                out.push(format!("{}::{name}", package.name));
            }
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_type_refs(child, source, generics, scope, out);
            }
        }
    }
}

/// Type parameter names in scope for a function or method declaration, including the ones a
/// method's receiver binds (`func (s *Set[K]) ...`).
fn generic_type_names(declaration: Node<'_>, source: &[u8]) -> HashSet<String> {
    let mut names = HashSet::new();
    if let Some(list) = declaration.child_by_field_name("type_parameters") {
        let mut cursor = list.walk();
        for parameter in list.named_children(&mut cursor) {
            let mut name_cursor = parameter.walk();
            for name in parameter.children_by_field_name("name", &mut name_cursor) {
                names.insert(node_text(name, source).trim().to_owned());
            }
        }
    }
    if let Some(receiver) = declaration.child_by_field_name("receiver") {
        let mut stack = vec![receiver];
        while let Some(node) = stack.pop() {
            if node.kind() == "type_arguments" {
                let mut cursor = node.walk();
                for argument in node.named_children(&mut cursor) {
                    let text = node_text(argument, source).trim().to_owned();
                    if !text.is_empty() {
                        names.insert(text);
                    }
                }
                continue;
            }
            let mut cursor = node.walk();
            stack.extend(node.named_children(&mut cursor));
        }
    }
    names
}

/// The names an unaliased import may be referred to by, and the best guess at the package's
/// declared name for an aliased one. A trailing `/vN` is either a module's major version
/// (`go-chi/chi/v5` declares `package chi`) or a versioned API package (`api/v1` declares
/// `package v1`), so both are accepted.
fn import_package_names(path: &str) -> Option<(Vec<String>, String)> {
    let segments = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let (&last, rest) = segments.split_last()?;
    let is_major_version = last.len() > 1
        && last.starts_with('v')
        && last[1..].bytes().all(|byte| byte.is_ascii_digit());
    if is_major_version && let Some(&parent) = rest.last() {
        let mut names = vec![last.to_owned()];
        names.extend(package_name_candidates(parent));
        return Some((names, last.to_owned()));
    }
    let names = package_name_candidates(last);
    let declared = names.last()?.clone();
    Some((names, declared))
}

/// Names an unaliased import may be referred to by: `yaml` for `gopkg.in/yaml.v3`,
/// `sqlite3` for `go-sqlite3`, `client` for `client-go`. The last entry is the best guess at
/// the package's declared name.
fn package_name_candidates(last_segment: &str) -> Vec<String> {
    let mut names = vec![last_segment.to_owned()];
    if let Some((base, version)) = last_segment.rsplit_once(".v")
        && !version.is_empty()
        && version.bytes().all(|byte| byte.is_ascii_digit())
    {
        names.push(base.to_owned());
    }
    if let Some(rest) = last_segment.strip_prefix("go-") {
        names.push(rest.to_owned());
    }
    if let Some(rest) = last_segment.strip_suffix("-go") {
        names.push(rest.to_owned());
    }
    names.retain(|name| !name.is_empty());
    names.dedup();
    names
}

fn import_path(node: Node<'_>, source: &[u8]) -> Option<String> {
    let text = node_text(node, source);
    let path = text.trim().trim_matches(['"', '`']).trim();
    (!path.is_empty()).then(|| path.to_owned())
}

/// The receiver's type name without pointers or type arguments: `Store` for
/// `func (s *Store[K]) Get()`.
fn receiver_type_name(method: Node<'_>, source: &[u8]) -> Option<String> {
    let receiver = method.child_by_field_name("receiver")?;
    let mut cursor = receiver.walk();
    let parameter = receiver
        .named_children(&mut cursor)
        .find(|child| child.kind() == "parameter_declaration")?;
    let mut node = parameter.child_by_field_name("type")?;
    loop {
        match node.kind() {
            "type_identifier" => {
                let name = node_text(node, source).trim().to_owned();
                return (!name.is_empty()).then_some(name);
            }
            "pointer_type" | "parenthesized_type" => node = node.named_child(0)?,
            "generic_type" => node = node.child_by_field_name("type")?,
            _ => return None,
        }
    }
}

fn enclosing_declaration(node: Node<'_>) -> Option<Node<'_>> {
    let mut current = node.parent();
    while let Some(parent) = current {
        if matches!(parent.kind(), "function_declaration" | "method_declaration") {
            return Some(parent);
        }
        current = parent.parent();
    }
    None
}

fn is_inside_function(node: Node<'_>) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        if matches!(
            parent.kind(),
            "function_declaration" | "method_declaration" | "func_literal"
        ) {
            return true;
        }
        current = parent.parent();
    }
    false
}

fn symbol_for_node<'a>(symbols: &'a [Symbol], node: Node<'_>) -> Option<&'a Symbol> {
    symbols.iter().find(|symbol| {
        symbol.range.start_byte == Some(node.start_byte())
            && symbol.range.end_byte == Some(node.end_byte())
    })
}

/// `go test` runs `func TestXxx(t *testing.T)` where `Xxx` does not start with a lowercase
/// letter.
fn is_test_function_name(name: &str) -> bool {
    name.strip_prefix("Test")
        .is_some_and(|rest| !rest.starts_with(|ch: char| ch.is_lowercase()))
}

fn takes_testing_t(function: Node<'_>, source: &[u8]) -> bool {
    let Some(parameters) = function.child_by_field_name("parameters") else {
        return false;
    };
    let mut cursor = parameters.walk();
    let declarations = parameters
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "parameter_declaration")
        .collect::<Vec<_>>();
    let [parameter] = declarations.as_slice() else {
        return false;
    };
    parameter
        .child_by_field_name("type")
        .filter(|declared| declared.kind() == "pointer_type")
        .and_then(|pointer| pointer.named_child(0))
        .filter(|inner| inner.kind() == "qualified_type")
        .and_then(|qualified| qualified.child_by_field_name("name"))
        .is_some_and(|name| node_text(name, source).trim() == "T")
}

/// Reads the comment block directly above a declaration, dropping compiler directives such as
/// `//go:build` and the conventional leading repeat of the declaration's name. A comment that
/// trails code on its own line belongs to that code, so it ends the block.
fn doc_comment(node: Node<'_>, source: &[u8], name: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut next_row = node.start_position().row;
    let mut current = node.prev_sibling();
    while let Some(comment) = current {
        if comment.kind() != "comment" || comment.end_position().row + 1 != next_row {
            break;
        }
        let trails_code = comment.prev_sibling().is_some_and(|previous| {
            previous.kind() != "comment"
                && previous.end_position().row == comment.start_position().row
        });
        if trails_code {
            break;
        }
        lines.push(node_text(comment, source));
        next_row = comment.start_position().row;
        current = comment.prev_sibling();
    }
    lines.reverse();

    let text = lines
        .iter()
        .filter_map(|line| comment_prose(line))
        .collect::<Vec<_>>()
        .join(" ");
    let text = text.trim();
    let text = text
        .strip_prefix(name)
        .filter(|rest| rest.starts_with(char::is_whitespace))
        .unwrap_or(text);
    let normalized = normalize_intent_text(text);
    (!normalized.is_empty()).then_some(normalized)
}

fn comment_prose(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix("//") {
        let is_directive = rest.starts_with(|ch: char| ch.is_ascii_alphanumeric())
            && rest
                .split_whitespace()
                .next()
                .is_some_and(|word| word.contains(':'));
        return (!is_directive).then(|| rest.trim().to_owned());
    }
    raw.strip_prefix("/*")
        .map(|rest| rest.trim_end_matches("*/").trim().to_owned())
}

/// `TestParseConfig_EmptyInput` becomes "parse config empty input".
fn humanize_test_name(name: &str) -> String {
    split_identifier_words(name.strip_prefix("Test").unwrap_or(name)).join(" ")
}

fn split_identifier_words(value: &str) -> Vec<String> {
    let chars = value.chars().collect::<Vec<_>>();
    let mut words = Vec::new();
    let mut current = String::new();
    for (index, &ch) in chars.iter().enumerate() {
        if ch == '_' || ch == '-' || ch.is_whitespace() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            continue;
        }
        if !current.is_empty() {
            let previous = chars[index - 1];
            let next = chars.get(index + 1).copied();
            let starts_word = (ch.is_uppercase()
                && (previous.is_lowercase() || previous.is_ascii_digit()))
                || (ch.is_uppercase()
                    && previous.is_uppercase()
                    && next.is_some_and(char::is_lowercase))
                || (ch.is_ascii_digit() && previous.is_alphabetic());
            if starts_word {
                words.push(std::mem::take(&mut current));
            }
        }
        current.extend(ch.to_lowercase());
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::{
        humanize_test_name, import_package_names, is_test_function_name, package_name_candidates,
    };

    #[test]
    fn humanizes_camel_case_and_underscored_test_names() {
        assert_eq!(humanize_test_name("TestGetWorks"), "get works");
        assert_eq!(
            humanize_test_name("TestParseConfig_EmptyInput"),
            "parse config empty input"
        );
        assert_eq!(
            humanize_test_name("TestHTTPServerReturns404"),
            "http server returns 404"
        );
    }

    #[test]
    fn recognizes_go_test_function_names() {
        assert!(is_test_function_name("Test"));
        assert!(is_test_function_name("TestParse"));
        assert!(is_test_function_name("Test_parse"));
        assert!(!is_test_function_name("Testparse"));
        assert!(!is_test_function_name("helperTest"));
    }

    #[test]
    fn derives_package_names_from_import_paths() {
        let names = |path: &str| import_package_names(path).expect("names");
        assert_eq!(names("fmt"), (vec!["fmt".to_owned()], "fmt".to_owned()));
        assert_eq!(
            names("net/http"),
            (vec!["http".to_owned()], "http".to_owned())
        );
        // A trailing `/vN` may be a module's major version or a versioned API package.
        assert_eq!(
            names("github.com/go-chi/chi/v5"),
            (vec!["v5".to_owned(), "chi".to_owned()], "v5".to_owned())
        );
        assert_eq!(
            names("gopkg.in/yaml.v3"),
            (
                vec!["yaml.v3".to_owned(), "yaml".to_owned()],
                "yaml".to_owned()
            )
        );
        assert_eq!(package_name_candidates("yaml.v3"), vec!["yaml.v3", "yaml"]);
        assert_eq!(
            package_name_candidates("go-sqlite3"),
            vec!["go-sqlite3", "sqlite3"]
        );
        assert_eq!(
            package_name_candidates("client-go"),
            vec!["client-go", "client"]
        );
    }
}
