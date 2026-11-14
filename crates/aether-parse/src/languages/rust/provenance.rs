//! Conservative Rust target proofs. General receiver inference, glob imports,
//! reexports, macro-generated items, cfg selection and custom module paths remain
//! raw unresolved references rather than guessing from a globally unique name.
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use aether_core::{EdgeKind, Language, Symbol, SymbolEdge, normalize_path};
use tree_sitter::{Node, Parser};

use super::collect_use_path_entries;
use crate::parser::{node_text, rust_call_target, rust_qualified_name, rust_source_function_id};

mod receivers;
use receivers::{
    dispatch_scope_safe, inherent_method, local_receiver_target, nominal_struct, receiver_nominal,
};

pub(crate) fn local_call_target<'a>(
    call: Node<'_>,
    source: &[u8],
    symbols: &'a [Symbol],
) -> Option<&'a Symbol> {
    if !syntax_supported(call) {
        return None;
    }
    let callee = call.child_by_field_name("function")?;
    if callee.kind() == "field_expression" {
        return local_receiver_target(call, source, symbols);
    }
    if !matches!(callee.kind(), "identifier" | "scoped_identifier") {
        return None;
    }
    let text = node_text(callee, source);
    let path = simple_path(&text)?;
    let declaration = local_item(call, &path, source, false, 0)?;
    symbol_for_node(declaration, symbols)
}

pub(crate) fn local_type_target<'a>(
    at: Node<'_>,
    path: &[String],
    source: &[u8],
    symbols: &[&'a Symbol],
) -> Option<&'a Symbol> {
    if !syntax_supported(at) {
        return None;
    }
    let declaration = local_item(at, path, source, true, 0)?;
    let start = declaration.start_byte();
    symbols
        .iter()
        .copied()
        .find(|symbol| symbol.range.start_byte == Some(start))
}

fn symbol_for_node<'a>(node: Node<'_>, symbols: &'a [Symbol]) -> Option<&'a Symbol> {
    symbols
        .iter()
        .find(|symbol| symbol.range.start_byte == Some(node.start_byte()))
}

fn simple_path(text: &str) -> Option<Vec<String>> {
    let parts = text
        .trim()
        .split("::")
        .map(str::to_owned)
        .collect::<Vec<_>>();
    (!parts.is_empty()
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_alphanumeric() || c == '_')))
    .then_some(parts)
}

/// A statement/item macro can introduce an unmodelled binding in this scope.
/// Macros nested in another expression cannot inject a sibling declaration.
fn scope_macros(scope: Node<'_>) -> bool {
    children(scope).into_iter().any(|item| {
        item.kind() == "macro_invocation"
            || (item.kind() == "expression_statement"
                && item
                    .named_child(0)
                    .is_some_and(|expression| expression.kind() == "macro_invocation"))
    })
}

fn syntax_supported(mut at: Node<'_>) -> bool {
    if attributed(at) {
        return false;
    }
    while let Some(parent) = at.parent() {
        at = parent;
    }
    !at.has_error()
}

fn lexical_scopes(at: Node<'_>) -> Vec<Node<'_>> {
    let mut visible = Vec::new();
    for scope in scopes(at) {
        // Associated item names are not bare lexical bindings: method bodies
        // require Self::member or Type::member to name their sibling items.
        if scope
            .parent()
            .is_some_and(|parent| matches!(parent.kind(), "impl_item" | "trait_item"))
        {
            continue;
        }
        visible.push(scope);
        if scope
            .parent()
            .is_some_and(|parent| parent.kind() == "mod_item")
        {
            break;
        }
    }
    visible
}

fn scopes(at: Node<'_>) -> Vec<Node<'_>> {
    let mut out = Vec::new();
    let mut current = at.parent();
    while let Some(node) = current {
        if matches!(node.kind(), "block" | "declaration_list" | "source_file") {
            out.push(node);
        }
        current = node.parent();
    }
    out
}

fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

fn name(node: Node<'_>, source: &[u8]) -> Option<String> {
    node.child_by_field_name("name")
        .map(|node| node_text(node, source))
}

/// Foreign declarations bind names in their enclosing scope without providing
/// an indexed definition. An extern-crate alias similarly owns its namespace.
fn extern_binding(scope: Node<'_>, wanted: &str, source: &[u8]) -> bool {
    children(scope).into_iter().any(|item| match item.kind() {
        "extern_crate_declaration" => item
            .child_by_field_name("alias")
            .or_else(|| item.child_by_field_name("name"))
            .is_some_and(|binding| node_text(binding, source) == wanted),
        "foreign_mod_item" => item.child_by_field_name("body").is_some_and(|body| {
            children(body)
                .into_iter()
                .any(|declaration| name(declaration, source).as_deref() == Some(wanted))
        }),
        _ => false,
    })
}

fn contains_identifier(node: Node<'_>, wanted: &str, source: &[u8]) -> bool {
    (matches!(node.kind(), "identifier" | "type_identifier") && node_text(node, source) == wanted)
        || children(node)
            .into_iter()
            .any(|child| contains_identifier(child, wanted, source))
}

fn parameter_binding(parameters: Node<'_>, wanted: &str, source: &[u8]) -> bool {
    children(parameters).into_iter().any(|parameter| {
        let pattern = if matches!(parameter.kind(), "parameter" | "variadic_parameter") {
            parameter.child_by_field_name("pattern")
        } else if parameters.kind() == "closure_parameters" {
            Some(parameter)
        } else {
            None
        };
        pattern.is_some_and(|pattern| contains_identifier(pattern, wanted, source))
    })
}

/// Conservative shadow check: any binding in an enclosing function/block may
/// hide the item. Ignoring flow order deliberately loses uncertain links.
fn shadowed(at: Node<'_>, wanted: &str, source: &[u8], type_name: bool) -> bool {
    let mut node = Some(at);
    while let Some(current) = node {
        if matches!(current.kind(), "function_item" | "closure_expression")
            && let Some(parameters) = current.child_by_field_name("parameters")
            && !type_name
            && parameter_binding(parameters, wanted, source)
        {
            return true;
        }
        // Type parameters on impls/traits/types hide outer concrete types too.
        if let Some(parameters) = current.child_by_field_name("type_parameters")
            && children(parameters).into_iter().any(|parameter| {
                parameter
                    .child_by_field_name("name")
                    .is_some_and(|binding| node_text(binding, source) == wanted)
            })
        {
            return true;
        }
        if current.kind() == "block" {
            for child in children(current) {
                if child.kind() == "let_declaration"
                    && let Some(pattern) = child.child_by_field_name("pattern")
                    && contains_identifier(pattern, wanted, source)
                {
                    return true;
                }
            }
        }
        if matches!(
            current.kind(),
            "for_expression" | "if_expression" | "while_expression" | "match_arm"
        ) && let Some(pattern) = current.child_by_field_name("pattern")
            && contains_identifier(pattern, wanted, source)
        {
            return true;
        }
        // if-let/while-let expose their pattern as a let_condition, not as a
        // field on the surrounding expression.
        if matches!(current.kind(), "if_expression" | "while_expression")
            && children(current).into_iter().any(|child| {
                child.kind() == "let_condition"
                    && child
                        .child_by_field_name("pattern")
                        .is_some_and(|pattern| contains_identifier(pattern, wanted, source))
            })
        {
            return true;
        }
        node = current.parent();
    }
    false
}

fn visible_import(at: Node<'_>, wanted: &str, source: &[u8]) -> Option<Vec<String>> {
    for scope in lexical_scopes(at) {
        if scope_macros(scope) || extern_binding(scope, wanted, source) {
            return None;
        }
        let mut bindings = HashMap::new();
        let mut has_glob = false;
        if children(scope).into_iter().any(|item| {
            name(item, source).as_deref() == Some(wanted)
                && matches!(
                    item.kind(),
                    "function_item"
                        | "struct_item"
                        | "enum_item"
                        | "trait_item"
                        | "type_item"
                        | "mod_item"
                        | "const_item"
                        | "static_item"
                )
        }) {
            return None;
        }
        for item in children(scope)
            .into_iter()
            .filter(|item| item.kind() == "use_declaration")
        {
            let text = node_text(item, source);
            has_glob |= text.contains('*');
            if let Some(argument) = item.child_by_field_name("argument") {
                collect_use_path_entries(argument, &[], source, &mut bindings);
            }
        }
        if let Some(paths) = bindings.get(wanted) {
            return (paths.len() == 1).then(|| paths[0].clone());
        }
        // A glob can introduce an unmodelled binding; don't seek a farther one.
        if has_glob {
            return None;
        }
    }
    None
}

fn local_item<'tree>(
    at: Node<'tree>,
    path: &[String],
    source: &[u8],
    type_name: bool,
    depth: usize,
) -> Option<Node<'tree>> {
    if depth > 8 || path.is_empty() || shadowed(at, &path[0], source, type_name) {
        return None;
    }
    let mut visible = scopes(at);
    let mut remaining = path;
    match path[0].as_str() {
        "crate" => {
            // File-relative parsing cannot prove which file is a crate root.
            // Workspace resolution handles crate paths with the manifest.
            return None;
        }
        "self" => {
            visible.retain(|scope| {
                scope.kind() == "source_file"
                    || scope
                        .parent()
                        .is_some_and(|parent| parent.kind() == "mod_item")
            });
            remaining = &path[1..];
            visible.truncate(1);
        }
        "super" => {
            visible.retain(|scope| {
                scope.kind() == "source_file"
                    || scope
                        .parent()
                        .is_some_and(|parent| parent.kind() == "mod_item")
            });
            let count = path
                .iter()
                .take_while(|part| part.as_str() == "super")
                .count();
            visible = visible.into_iter().skip(count).take(1).collect();
            remaining = &path[count..];
        }
        "Self" => {
            let mut current = at.parent();
            while let Some(item) = current {
                if item.kind() == "impl_item" && item.child_by_field_name("trait").is_none() {
                    let body = item.child_by_field_name("body")?;
                    return descend(body, &path[1..], source, type_name);
                }
                current = item.parent();
            }
            return None;
        }
        _ => visible = lexical_scopes(at),
    }
    let first = remaining.first()?;
    for scope in visible {
        if scope_macros(scope) || extern_binding(scope, first, source) {
            return None;
        }
        let items = children(scope);
        let matches = items
            .iter()
            .copied()
            .filter(|item| {
                name(*item, source).as_deref() == Some(first.as_str())
                    && matches!(
                        item.kind(),
                        "function_item"
                            | "struct_item"
                            | "enum_item"
                            | "trait_item"
                            | "type_item"
                            | "mod_item"
                            | "const_item"
                            | "static_item"
                    )
            })
            .collect::<Vec<_>>();
        if matches.len() > 1 {
            return None;
        }
        if let Some(item) = matches.first().copied() {
            if attributed(item) {
                return None;
            }
            if remaining.len() == 1 {
                return allowed(item, type_name).then_some(item);
            }
            if item.kind() == "mod_item" {
                return descend(
                    item.child_by_field_name("body")?,
                    &remaining[1..],
                    source,
                    type_name,
                );
            }
            if matches!(item.kind(), "struct_item" | "enum_item") {
                let mut methods = Vec::new();
                for implementation in items.into_iter().filter(|item| item.kind() == "impl_item") {
                    if implementation.child_by_field_name("trait").is_some()
                        || implementation
                            .child_by_field_name("type")
                            .map(|ty| node_text(ty, source))
                            .as_deref()
                            != Some(first)
                    {
                        continue;
                    }
                    if let Some(body) = implementation.child_by_field_name("body")
                        && let Some(method) = descend(body, &remaining[1..], source, type_name)
                    {
                        methods.push(method);
                    }
                }
                return (methods.len() == 1).then(|| methods[0]);
            }
            return None;
        }
        let mut imports = HashMap::new();
        let mut glob = false;
        for item in items
            .into_iter()
            .filter(|item| item.kind() == "use_declaration")
        {
            glob |= node_text(item, source).contains('*');
            if let Some(argument) = item.child_by_field_name("argument") {
                collect_use_path_entries(argument, &[], source, &mut imports);
            }
        }
        if let Some(paths) = imports.get(first) {
            if paths.len() != 1
                || !matches!(
                    paths[0].first().map(String::as_str),
                    Some("crate" | "self" | "super")
                )
            {
                // Bare use paths differ by edition and crate-root context.
                // File-local extraction has no evidence for either.
                return None;
            }
            let mut imported = paths[0].clone();
            imported.extend_from_slice(&remaining[1..]);
            // Imported module paths resolve in the same scope, not from the call
            // site's nested blocks.
            return local_item(
                scope.named_child(0)?,
                &imported,
                source,
                type_name,
                depth + 1,
            );
        }
        if glob {
            return None;
        }
    }
    None
}

fn has_attached_attribute(item: Node<'_>) -> bool {
    let mut previous = item.prev_named_sibling();
    while let Some(node) = previous {
        match node.kind() {
            "line_comment" | "block_comment" => previous = node.prev_named_sibling(),
            "attribute_item" => return true,
            _ => return false,
        }
    }
    false
}

fn attributed(item: Node<'_>) -> bool {
    let mut current = Some(item);
    while let Some(node) = current {
        if has_attached_attribute(node) {
            return true;
        }
        current = node.parent();
    }
    false
}

fn allowed(item: Node<'_>, type_name: bool) -> bool {
    if attributed(item) {
        return false;
    }
    if type_name {
        matches!(
            item.kind(),
            "struct_item" | "enum_item" | "trait_item" | "type_item"
        )
    } else {
        item.kind() == "function_item"
            || (item.kind() == "struct_item"
                && item.child_by_field_name("type_parameters").is_none()
                && item
                    .child_by_field_name("body")
                    .is_some_and(|body| body.kind() == "ordered_field_declaration_list"))
    }
}

fn descend<'tree>(
    scope: Node<'tree>,
    path: &[String],
    source: &[u8],
    type_name: bool,
) -> Option<Node<'tree>> {
    if scope_macros(scope) {
        return None;
    }
    let first = path.first()?;
    let matches = children(scope)
        .into_iter()
        .filter(|item| name(*item, source).as_deref() == Some(first.as_str()))
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return None;
    }
    let item = matches[0];
    if path.len() == 1 {
        return allowed(item, type_name).then_some(item);
    }
    if item.kind() == "mod_item" {
        return descend(
            item.child_by_field_name("body")?,
            &path[1..],
            source,
            type_name,
        );
    }
    None
}

/// Files read to verify explicit module imports. Definitions are re-read on each
/// extraction; no stale summary is used after an edit.
pub(crate) struct RustWorkspace {
    root: PathBuf,
    reads: HashMap<String, BTreeSet<String>>,
}

impl RustWorkspace {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            reads: HashMap::new(),
        }
    }

    pub(crate) fn refresh(&mut self, path: &str) -> Vec<String> {
        let path = normalize_path(path);
        let path = path.trim_matches('/');
        self.reads
            .iter()
            .filter(|(_, reads)| {
                reads.iter().any(|read| {
                    path.is_empty()
                        || read == path
                        || read.starts_with(&format!("{path}/"))
                        || path.starts_with(&format!("{read}/"))
                })
            })
            .map(|(caller, _)| caller.clone())
            .collect()
    }

    pub(crate) fn resolve_calls(
        &mut self,
        file_path: &str,
        source: &str,
        edges: &mut [SymbolEdge],
    ) {
        let mut reads = BTreeSet::new();
        let mut parser = Parser::new();
        if parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .is_err()
        {
            return;
        }
        let Some(tree) = parser.parse(source, None) else {
            return;
        };
        if tree.root_node().has_error() {
            return;
        }
        let mut resolved = HashMap::<(String, String), Option<(String, String)>>::new();
        let mut stack = vec![tree.root_node()];
        while let Some(call) = stack.pop() {
            stack.extend(children(call));
            if call.kind() != "call_expression" {
                continue;
            }
            let Some(raw) = rust_call_target(call, source.as_bytes()) else {
                continue;
            };
            let Some(source_id) =
                rust_source_function_id(Language::Rust, file_path, source.as_bytes(), call)
            else {
                continue;
            };
            let key = (source_id, raw.clone());
            let target = self.import_target(
                &mut parser,
                file_path,
                call,
                &raw,
                source.as_bytes(),
                &mut reads,
            );
            resolved
                .entry(key)
                .and_modify(|previous| {
                    if *previous != target {
                        *previous = None;
                    }
                })
                .or_insert(target);
        }
        for edge in edges
            .iter_mut()
            .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.target_symbol_id.is_none())
        {
            if let Some(Some((path, name))) =
                resolved.get(&(edge.source_id.clone(), edge.target_qualified_name.clone()))
            {
                edge.target_file_path = Some(path.clone());
                edge.target_qualified_name = name.clone();
            }
        }
        self.reads.insert(file_path.to_owned(), reads);
    }

    fn import_target(
        &self,
        parser: &mut Parser,
        file_path: &str,
        call: Node<'_>,
        raw: &str,
        source: &[u8],
        reads: &mut BTreeSet<String>,
    ) -> Option<(String, String)> {
        if !syntax_supported(call) {
            return None;
        }
        let callee = call.child_by_field_name("function")?;
        let receiver = (callee.kind() == "field_expression")
            .then(|| receiver_nominal(call, source))
            .flatten();
        let (mut path, context, member) = if let Some(receiver) = &receiver {
            let method = callee
                .child_by_field_name("field")
                .map(|field| node_text(field, source))?;
            if receiver.reference || !dispatch_scope_safe(call, &method, &receiver.path, source) {
                return None;
            }
            (
                receiver.path.clone(),
                receiver.at,
                Some((method, receiver.require_unit)),
            )
        } else if matches!(callee.kind(), "identifier" | "scoped_identifier") {
            (simple_path(raw)?, call, None)
        } else {
            return None;
        };
        if shadowed(
            context,
            &path[0],
            source,
            receiver
                .as_ref()
                .is_some_and(|receiver| !receiver.require_unit),
        ) {
            return None;
        }
        if !matches!(path[0].as_str(), "crate" | "self" | "super") {
            let import = visible_import(context, &path[0], source)?;
            path.splice(0..1, import);
        }
        // Inline modules and block-local imports require a complete module
        // context. The bounded external-file resolver does not guess one.
        let mut ancestor = call.parent();
        while let Some(node) = ancestor {
            if node.kind() == "mod_item" {
                return None;
            }
            ancestor = node.parent();
        }
        let own = Path::new(file_path);
        if own.is_absolute()
            || own
                .components()
                .any(|part| part == std::path::Component::ParentDir)
        {
            return None;
        }
        let crate_dir = own.ancestors().skip(1).find(|directory| {
            let manifest = directory.join("Cargo.toml");
            reads.insert(normalize_path(&manifest.to_string_lossy()));
            self.root.join(&manifest).is_file()
        })?;
        let manifest = fs::read_to_string(self.root.join(crate_dir).join("Cargo.toml")).ok()?;
        if !conventional_manifest(&manifest) {
            return None;
        }
        let src = crate_dir.join("src");
        if !own.starts_with(&src) {
            return None;
        }
        // A source file belongs to lib.rs or main.rs; when both exist, a
        // non-root module could be compiled by either, so its crate path is
        // deliberately unsupported.
        let roots = ["lib.rs", "main.rs"]
            .iter()
            .map(|name| src.join(name))
            .filter(|path| {
                reads.insert(normalize_path(&path.to_string_lossy()));
                self.root.join(path).is_file()
            })
            .collect::<Vec<_>>();
        let root_file = if roots.iter().any(|root| root == own) {
            own.to_path_buf()
        } else if roots.len() == 1 {
            roots[0].clone()
        } else {
            return None;
        };
        let own_module = module_segments(own, &src)?;
        if !self.verify_caller(
            parser,
            &root_file,
            &own_module,
            own,
            (file_path, source),
            reads,
        ) {
            return None;
        }
        let mut absolute = match path.first()?.as_str() {
            "crate" => {
                path.remove(0);
                Vec::new()
            }
            "self" => {
                path.remove(0);
                own_module
            }
            "super" => {
                let count = path
                    .iter()
                    .take_while(|part| part.as_str() == "super")
                    .count();
                if count > own_module.len() {
                    return None;
                }
                path.drain(0..count);
                own_module[..own_module.len() - count].to_vec()
            }
            _ => return None,
        };
        absolute.extend(path);
        if absolute.is_empty() {
            return None;
        }
        self.follow(
            parser,
            root_file,
            (
                absolute.as_slice(),
                member
                    .as_ref()
                    .map(|(method, unit)| (method.as_str(), *unit)),
            ),
            file_path,
            source,
            reads,
        )
    }

    fn verify_caller(
        &self,
        parser: &mut Parser,
        root_file: &Path,
        modules: &[String],
        own: &Path,
        caller: (&str, &[u8]),
        reads: &mut BTreeSet<String>,
    ) -> bool {
        let mut file = root_file.to_path_buf();
        for module in modules {
            let Some(source) = self.read_source(&file, caller.0, caller.1, reads) else {
                return false;
            };
            let Some(tree) = parser.parse(&source, None) else {
                return false;
            };
            if tree.root_node().has_error() || scope_macros(tree.root_node()) {
                return false;
            }
            let declarations = children(tree.root_node())
                .into_iter()
                .filter(|item| {
                    item.kind() == "mod_item"
                        && name(*item, source.as_bytes()).as_deref() == Some(module.as_str())
                })
                .collect::<Vec<_>>();
            if declarations.len() != 1 {
                return false;
            }
            let item = declarations[0];
            if item.child_by_field_name("body").is_some() || has_attached_attribute(item) {
                return false;
            }
            let Some(next) = self.external_module_file(&file, module, reads) else {
                return false;
            };
            file = next;
        }
        file == own
    }

    fn read_source(
        &self,
        file: &Path,
        caller_path: &str,
        caller_source: &[u8],
        reads: &mut BTreeSet<String>,
    ) -> Option<String> {
        let path = normalize_path(&file.to_string_lossy());
        reads.insert(path.clone());
        if path == caller_path {
            return std::str::from_utf8(caller_source).ok().map(str::to_owned);
        }
        let absolute = self.root.join(file);
        if fs::metadata(&absolute).ok()?.len() > 2 * 1024 * 1024 {
            return None;
        }
        fs::read_to_string(absolute).ok()
    }

    fn external_module_file(
        &self,
        file: &Path,
        module: &str,
        reads: &mut BTreeSet<String>,
    ) -> Option<PathBuf> {
        let parent = file.parent()?;
        let directory = if matches!(file.file_name()?.to_str()?, "lib.rs" | "main.rs" | "mod.rs") {
            parent.to_path_buf()
        } else {
            parent.join(file.file_stem()?)
        };
        let candidates = [
            directory.join(format!("{module}.rs")),
            directory.join(module).join("mod.rs"),
        ];
        for candidate in &candidates {
            reads.insert(normalize_path(&candidate.to_string_lossy()));
        }
        let existing = candidates
            .into_iter()
            .filter(|candidate| self.root.join(candidate).is_file())
            .collect::<Vec<_>>();
        (existing.len() == 1).then(|| existing[0].clone())
    }

    fn follow(
        &self,
        parser: &mut Parser,
        mut file: PathBuf,
        request: (&[String], Option<(&str, bool)>),
        caller_path: &str,
        caller_source: &[u8],
        reads: &mut BTreeSet<String>,
    ) -> Option<(String, String)> {
        let (mut remaining, member) = request;
        for hop in 0..32 {
            let source = self.read_source(&file, caller_path, caller_source, reads)?;
            let tree = parser.parse(&source, None)?;
            if tree.root_node().has_error() || scope_macros(tree.root_node()) {
                return None;
            }
            let items = children(tree.root_node());
            let first = remaining.first()?;
            let declarations = items
                .iter()
                .copied()
                .filter(|item| {
                    name(*item, source.as_bytes()).as_deref() == Some(first.as_str())
                        && matches!(
                            item.kind(),
                            "mod_item" | "function_item" | "struct_item" | "enum_item"
                        )
                })
                .collect::<Vec<_>>();
            if declarations.len() != 1 {
                return None;
            }
            let item = declarations[0];
            // Attribute expansion and cfg alternatives need the compiler.
            if has_attached_attribute(item) {
                return None;
            }
            if let Some((method_name, require_unit)) = member
                && remaining.len() == 1
                && nominal_struct(item, require_unit)
                && is_public(item, source.as_bytes())
            {
                let method = inherent_method(item, method_name, source.as_bytes(), true)?;
                if !is_public(method, source.as_bytes()) {
                    return None;
                }
                return Some((
                    normalize_path(&file.to_string_lossy()),
                    rust_qualified_name(method, method_name, source.as_bytes()),
                ));
            }
            if member.is_none()
                && remaining.len() == 2
                && nominal_struct(item, false)
                && is_public(item, source.as_bytes())
            {
                let method = inherent_method(item, &remaining[1], source.as_bytes(), false)?;
                if !is_public(method, source.as_bytes()) {
                    return None;
                }
                return Some((
                    normalize_path(&file.to_string_lossy()),
                    rust_qualified_name(method, &remaining[1], source.as_bytes()),
                ));
            }
            if member.is_none() && remaining.len() == 1 && item.kind() == "function_item" {
                if !is_public(item, source.as_bytes()) {
                    return None;
                }
                return Some((
                    normalize_path(&file.to_string_lossy()),
                    rust_qualified_name(item, first, source.as_bytes()),
                ));
            }
            if item.kind() != "mod_item"
                || remaining.len() < 2
                || item.child_by_field_name("body").is_some()
                || (hop > 0 && !is_public(item, source.as_bytes()))
            {
                return None;
            }
            remaining = &remaining[1..];
            file = self.external_module_file(&file, first, reads)?;
        }
        None
    }
}

fn is_public(node: Node<'_>, source: &[u8]) -> bool {
    children(node).into_iter().any(|child| {
        child.kind() == "visibility_modifier"
            && matches!(node_text(child, source).as_str(), "pub" | "pub(crate)")
    })
}

fn module_segments(file: &Path, src: &Path) -> Option<Vec<String>> {
    let relative = file.strip_prefix(src).ok()?;
    let mut segments = relative
        .parent()?
        .components()
        .map(|part| part.as_os_str().to_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()?;
    let stem = relative.file_stem()?.to_str()?;
    if !matches!(stem, "lib" | "main" | "mod") {
        segments.push(stem.to_owned());
    }
    Some(segments)
}

/// This slice understands Cargo's automatic conventional roots only. Explicit
/// target tables and auto-discovery overrides need real Cargo target metadata.
fn conventional_manifest(source: &str) -> bool {
    let mut has_package = false;
    for line in source.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        let compact = line
            .chars()
            .filter(|character| !character.is_whitespace() && !matches!(character, '\'' | '"'))
            .collect::<String>();
        if compact == "[package]" {
            has_package = true;
        }
        if compact.starts_with("[lib]") || compact.starts_with("[[bin]]") {
            return false;
        }
        if let Some((key, _)) = compact.split_once('=')
            && (matches!(key, "lib" | "bin" | "autolib" | "autobins")
                || key.starts_with("lib.")
                || key.starts_with("bin."))
        {
            return false;
        }
    }
    has_package
}
