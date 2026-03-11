use std::collections::{HashMap, HashSet};

use aether_core::{EdgeKind, Language, Symbol, SymbolEdge, SymbolKind, file_source_id};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, Query, QueryCursor};

use crate::parser::{
    TestIntent, build_symbol, collect_scoped_identifier_segments, has_ancestor_kind,
    humanize_test_name, node_text, normalize_intent_text, rust_call_target, rust_qualified_name,
    rust_source_function_id, rust_use_target,
};
use crate::registry::{LanguageConfig, LanguageHooks, QueryCaptures};

pub fn config() -> LanguageConfig {
    let ts_language = tree_sitter_rust::LANGUAGE.into();
    let symbol_query = Query::new(&ts_language, include_str!("../queries/rust_symbols.scm"))
        .expect("invalid rust symbol query");
    let edge_query = Query::new(&ts_language, include_str!("../queries/rust_edges.scm"))
        .expect("invalid rust edge query");
    let test_intent_query = Query::new(
        &ts_language,
        include_str!("../queries/rust_test_intents.scm"),
    )
    .expect("invalid rust test intent query");

    LanguageConfig {
        id: "rust",
        extensions: &["rs"],
        ts_language,
        symbol_query,
        edge_query,
        test_intent_query: Some(test_intent_query),
        module_markers: &["Cargo.toml"],
        hooks: Some(Box::new(RustHooks)),
    }
}

struct RustHooks;

const STDLIB_TYPES: &[&str] = &[
    "String",
    "str",
    "Vec",
    "HashMap",
    "HashSet",
    "BTreeMap",
    "BTreeSet",
    "Option",
    "Result",
    "Box",
    "Arc",
    "Rc",
    "Mutex",
    "RwLock",
    "Cell",
    "RefCell",
    "Pin",
    "Cow",
    "PhantomData",
    "bool",
    "u8",
    "u16",
    "u32",
    "u64",
    "u128",
    "usize",
    "i8",
    "i16",
    "i32",
    "i64",
    "i128",
    "isize",
    "f32",
    "f64",
    "char",
    "Self",
    "Infallible",
    "Duration",
    "Instant",
    "PathBuf",
    "Path",
    "OsStr",
    "OsString",
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct TypeCandidate {
    leaf: String,
    path_segments: Vec<String>,
}

struct RustTypeResolver<'a> {
    leaf_to_symbols: HashMap<String, Vec<&'a Symbol>>,
    type_symbols: Vec<&'a Symbol>,
    use_paths_by_visible: HashMap<String, Vec<Vec<String>>>,
}

impl LanguageHooks for RustHooks {
    fn map_symbol(
        &self,
        language: Language,
        captures: &QueryCaptures<'_, '_>,
        source: &[u8],
        file_path: &str,
    ) -> Option<Symbol> {
        let capture_name = captures.first_capture_name_with_prefix("symbol.")?;
        let node = captures.node_with_prefix("symbol.")?;
        let name = captures.capture_text("name", source)?;
        let name = name.trim().to_owned();
        if name.is_empty() {
            return None;
        }

        let kind = match capture_name {
            "symbol.struct" => SymbolKind::Struct,
            "symbol.enum" => SymbolKind::Enum,
            "symbol.trait" => SymbolKind::Trait,
            "symbol.type_alias" => SymbolKind::TypeAlias,
            "symbol.function" => {
                if has_ancestor_kind(node, "impl_item") || has_ancestor_kind(node, "trait_item") {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                }
            }
            _ => return None,
        };

        let qualified_name = rust_qualified_name(node, &name, source);
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

    fn map_edge(
        &self,
        language: Language,
        captures: &QueryCaptures<'_, '_>,
        source: &[u8],
        file_path: &str,
        _symbols: &[Symbol],
    ) -> Option<Vec<SymbolEdge>> {
        let capture_name = captures.first_capture_name_with_prefix("edge.")?;
        let node = captures.node_with_prefix("edge.")?;

        match capture_name {
            "edge.call" => {
                let target = rust_call_target(node, source)?;
                let source_id = rust_source_function_id(language, file_path, source, node)?;
                Some(vec![SymbolEdge {
                    source_id,
                    target_qualified_name: target,
                    edge_kind: EdgeKind::Calls,
                    file_path: file_path.to_owned(),
                }])
            }
            "edge.depends_on" => {
                let target = rust_use_target(node, source)?;
                Some(vec![SymbolEdge {
                    source_id: file_source_id(file_path),
                    target_qualified_name: target,
                    edge_kind: EdgeKind::DependsOn,
                    file_path: file_path.to_owned(),
                }])
            }
            _ => None,
        }
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
        if node.kind() != "function_item" {
            return None;
        }
        if !has_test_attribute(node, source) {
            return None;
        }

        let test_name = captures.capture_text("name", source)?.trim().to_owned();
        if test_name.is_empty() {
            return None;
        }

        let symbol_id = rust_source_function_id(language, file_path, source, node);
        let intent_text = rust_doc_comment(node, source)
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| humanize_test_name(test_name.as_str()));
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
}

impl<'a> RustTypeResolver<'a> {
    fn new(root: Node<'_>, source: &[u8], symbols: &'a [Symbol]) -> Self {
        let type_symbols = symbols
            .iter()
            .filter(|symbol| {
                matches!(
                    symbol.kind,
                    SymbolKind::Struct
                        | SymbolKind::Enum
                        | SymbolKind::Trait
                        | SymbolKind::TypeAlias
                )
            })
            .collect::<Vec<_>>();
        let mut leaf_to_symbols = HashMap::<String, Vec<&'a Symbol>>::new();
        for symbol in &type_symbols {
            leaf_to_symbols
                .entry(symbol.name.clone())
                .or_default()
                .push(*symbol);
        }

        Self {
            leaf_to_symbols,
            type_symbols,
            use_paths_by_visible: collect_use_paths(root, source),
        }
    }

    fn resolve_type_targets(&self, node: Node<'_>, source: &[u8]) -> Vec<&'a Symbol> {
        let mut candidates = Vec::new();
        let mut seen_candidates = HashSet::<String>::new();
        collect_type_candidates(node, source, &mut candidates, &mut seen_candidates);

        let mut resolved = Vec::new();
        let mut seen_symbols = HashSet::<String>::new();
        for candidate in candidates {
            let Some(symbol) = self.resolve_candidate(&candidate) else {
                continue;
            };
            if seen_symbols.insert(symbol.id.clone()) {
                resolved.push(symbol);
            }
        }
        resolved
    }

    fn resolve_endpoint_symbol(&self, node: Node<'_>, source: &[u8]) -> Option<&'a Symbol> {
        let candidate = primary_type_candidate(node, source)?;
        self.resolve_candidate(&candidate)
    }

    fn resolve_candidate(&self, candidate: &TypeCandidate) -> Option<&'a Symbol> {
        if candidate.leaf.is_empty() || STDLIB_TYPES.contains(&candidate.leaf.as_str()) {
            return None;
        }

        let mut explicit_matches = Vec::<&'a Symbol>::new();
        let mut seen = HashSet::<String>::new();

        if let Some(paths) = self.use_paths_by_visible.get(candidate.leaf.as_str()) {
            for path in paths {
                self.push_path_matches(path.as_slice(), &mut explicit_matches, &mut seen);
            }
        }
        if !candidate.path_segments.is_empty() {
            let normalized = normalize_path_segments(candidate.path_segments.as_slice());
            self.push_path_matches(normalized.as_slice(), &mut explicit_matches, &mut seen);
        }

        match explicit_matches.len() {
            1 => return explicit_matches.into_iter().next(),
            len if len > 1 => return None,
            _ => {}
        }

        let symbols = self.leaf_to_symbols.get(candidate.leaf.as_str())?;
        (symbols.len() == 1).then_some(symbols[0])
    }

    fn push_path_matches(
        &self,
        path: &[String],
        resolved: &mut Vec<&'a Symbol>,
        seen: &mut HashSet<String>,
    ) {
        if path.is_empty() {
            return;
        }

        for symbol in &self.type_symbols {
            if !qualified_name_matches_path(symbol.qualified_name.as_str(), path) {
                continue;
            }
            if seen.insert(symbol.id.clone()) {
                resolved.push(*symbol);
            }
        }
    }
}

pub(crate) fn extract_rust_edges(
    language: Language,
    file_path: &str,
    source: &[u8],
    root: Node<'_>,
    edge_query: &Query,
    symbols: &[Symbol],
) -> Vec<SymbolEdge> {
    let resolver = RustTypeResolver::new(root, source, symbols);
    let mut cursor = QueryCursor::new();
    let mut edges = Vec::new();
    let mut query_matches = cursor.matches(edge_query, root, source);

    while {
        query_matches.advance();
        query_matches.get().is_some()
    } {
        let matched = query_matches.get().expect("query match should exist");
        let captures = QueryCaptures::new(edge_query, matched.captures, matched.pattern_index);
        let Some(capture_name) = captures.first_capture_name_with_prefix("edge.") else {
            continue;
        };

        match capture_name {
            "edge.call" => {
                let Some(node) = captures.node("edge.call") else {
                    continue;
                };
                let Some(target) = rust_call_target(node, source) else {
                    continue;
                };
                let Some(source_id) = rust_source_function_id(language, file_path, source, node)
                else {
                    continue;
                };
                edges.push(SymbolEdge {
                    source_id,
                    target_qualified_name: target,
                    edge_kind: EdgeKind::Calls,
                    file_path: file_path.to_owned(),
                });
            }
            "edge.depends_on" => {
                let Some(node) = captures.node("edge.depends_on") else {
                    continue;
                };
                let Some(target) = rust_use_target(node, source) else {
                    continue;
                };
                edges.push(SymbolEdge {
                    source_id: file_source_id(file_path),
                    target_qualified_name: target,
                    edge_kind: EdgeKind::DependsOn,
                    file_path: file_path.to_owned(),
                });
            }
            "edge.type_ref" => {
                let Some(node) = captures.node("edge.type_ref") else {
                    continue;
                };
                let Some(source_id) = rust_source_function_id(language, file_path, source, node)
                else {
                    continue;
                };
                for symbol in resolver.resolve_type_targets(node, source) {
                    edges.push(SymbolEdge {
                        source_id: source_id.clone(),
                        target_qualified_name: symbol.qualified_name.clone(),
                        edge_kind: EdgeKind::TypeRef,
                        file_path: file_path.to_owned(),
                    });
                }
            }
            "edge.implements" => {
                let Some(self_type) = captures.node("self_type") else {
                    continue;
                };
                let Some(trait_node) = captures.node("trait") else {
                    continue;
                };
                let Some(source_symbol) = resolver.resolve_endpoint_symbol(self_type, source)
                else {
                    continue;
                };
                let Some(target_symbol) = resolver.resolve_endpoint_symbol(trait_node, source)
                else {
                    continue;
                };
                edges.push(SymbolEdge {
                    source_id: source_symbol.id.clone(),
                    target_qualified_name: target_symbol.qualified_name.clone(),
                    edge_kind: EdgeKind::Implements,
                    file_path: file_path.to_owned(),
                });
            }
            _ => {}
        }
    }

    edges
}

fn collect_type_candidates(
    node: Node<'_>,
    source: &[u8],
    candidates: &mut Vec<TypeCandidate>,
    seen: &mut HashSet<String>,
) {
    match node.kind() {
        "type_identifier" | "identifier" => {
            let text = node_text(node, source).trim().to_owned();
            if text.is_empty() {
                return;
            }
            let key = text.clone();
            if seen.insert(key) {
                candidates.push(TypeCandidate {
                    leaf: text.clone(),
                    path_segments: vec![text],
                });
            }
            return;
        }
        "scoped_identifier" | "scoped_type_identifier" => {
            let path_segments = collect_type_path_segments(node, source);
            if let Some(leaf) = path_segments.last().cloned() {
                let key = path_segments.join("::");
                if !key.is_empty() && seen.insert(key) {
                    candidates.push(TypeCandidate {
                        leaf,
                        path_segments,
                    });
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_type_candidates(child, source, candidates, seen);
    }
}

fn primary_type_candidate(node: Node<'_>, source: &[u8]) -> Option<TypeCandidate> {
    match node.kind() {
        "type_identifier" | "identifier" => {
            let text = node_text(node, source).trim().to_owned();
            (!text.is_empty()).then(|| TypeCandidate {
                leaf: text.clone(),
                path_segments: vec![text],
            })
        }
        "scoped_identifier" | "scoped_type_identifier" => {
            let path_segments = collect_type_path_segments(node, source);
            let leaf = path_segments.last().cloned()?;
            Some(TypeCandidate {
                leaf,
                path_segments,
            })
        }
        "generic_type" => node
            .child_by_field_name("type")
            .and_then(|child| primary_type_candidate(child, source)),
        "reference_type" => node
            .child_by_field_name("type")
            .and_then(|child| primary_type_candidate(child, source)),
        "dynamic_type" => node
            .child_by_field_name("trait")
            .and_then(|child| primary_type_candidate(child, source)),
        _ => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if let Some(candidate) = primary_type_candidate(child, source) {
                    return Some(candidate);
                }
            }
            None
        }
    }
}

fn collect_type_path_segments(node: Node<'_>, source: &[u8]) -> Vec<String> {
    match node.kind() {
        "scoped_identifier" => collect_scoped_identifier_segments(node, source)
            .into_iter()
            .map(|(segment, _)| segment)
            .collect(),
        "scoped_type_identifier" => {
            let mut segments = Vec::new();
            if let Some(path) = node.child_by_field_name("path") {
                segments.extend(collect_type_path_segments(path, source));
            }
            if let Some(name) = node.child_by_field_name("name") {
                segments.extend(collect_type_path_segments(name, source));
            }
            segments
        }
        "generic_type" => node
            .child_by_field_name("type")
            .map(|child| collect_type_path_segments(child, source))
            .unwrap_or_default(),
        "type_identifier" | "identifier" | "crate" | "self" | "super" => {
            let text = node_text(node, source).trim().to_owned();
            if text.is_empty() {
                Vec::new()
            } else {
                vec![text]
            }
        }
        _ => Vec::new(),
    }
}

fn normalize_path_segments(path: &[String]) -> Vec<String> {
    let mut segments = path.to_vec();
    while matches!(
        segments.first().map(String::as_str),
        Some("crate" | "self" | "super")
    ) {
        segments.remove(0);
    }
    segments
}

fn qualified_name_matches_path(qualified_name: &str, path: &[String]) -> bool {
    let symbol_segments = qualified_name.split("::").collect::<Vec<_>>();
    let path_len = path.len();
    if path_len == 0 || path_len > symbol_segments.len() {
        return false;
    }

    symbol_segments[symbol_segments.len() - path_len..]
        .iter()
        .zip(path.iter())
        .all(|(left, right)| *left == right)
}

fn collect_use_paths(root: Node<'_>, source: &[u8]) -> HashMap<String, Vec<Vec<String>>> {
    let mut paths = HashMap::<String, Vec<Vec<String>>>::new();
    let mut stack = vec![root];

    while let Some(node) = stack.pop() {
        if node.kind() == "use_declaration" {
            if let Some(argument) = node.child_by_field_name("argument") {
                collect_use_path_entries(argument, &[], source, &mut paths);
            }
            continue;
        }

        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }

    paths
}

fn collect_use_path_entries(
    node: Node<'_>,
    prefix: &[String],
    source: &[u8],
    out: &mut HashMap<String, Vec<Vec<String>>>,
) {
    match node.kind() {
        "identifier" | "crate" | "self" | "super" | "scoped_identifier" => {
            let mut path = prefix.to_vec();
            path.extend(collect_type_path_segments(node, source));
            insert_use_path(path, out);
        }
        "use_as_clause" => {
            let Some(path_node) = node.child_by_field_name("path") else {
                return;
            };
            let Some(alias) = node
                .child_by_field_name("alias")
                .map(|alias| node_text(alias, source))
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty())
            else {
                return;
            };

            let mut path = prefix.to_vec();
            path.extend(collect_type_path_segments(path_node, source));
            if path.is_empty() {
                return;
            }
            out.entry(alias).or_default().push(path);
        }
        "scoped_use_list" => {
            let mut next_prefix = prefix.to_vec();
            if let Some(path_node) = node.child_by_field_name("path") {
                next_prefix.extend(collect_type_path_segments(path_node, source));
            }
            if let Some(list) = node.child_by_field_name("list") {
                collect_use_path_entries(list, next_prefix.as_slice(), source, out);
            }
        }
        "use_list" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_use_path_entries(child, prefix, source, out);
            }
        }
        "use_wildcard" => {}
        _ => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_use_path_entries(child, prefix, source, out);
            }
        }
    }
}

fn insert_use_path(path: Vec<String>, out: &mut HashMap<String, Vec<Vec<String>>>) {
    let Some(visible_name) = path.last().cloned() else {
        return;
    };
    if visible_name.is_empty() {
        return;
    }

    let entry = out.entry(visible_name).or_default();
    if !entry.contains(&path) {
        entry.push(path);
    }
}

fn has_test_attribute(node: Node<'_>, source: &[u8]) -> bool {
    let text = String::from_utf8_lossy(source);
    let lines = text.lines().collect::<Vec<_>>();
    let mut row = node.start_position().row;
    if row == 0 {
        return false;
    }

    while row > 0 {
        row -= 1;
        let line = lines.get(row).copied().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("///") {
            continue;
        }
        if line.starts_with("#[") {
            let compact = line
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .collect::<String>();
            if compact.contains("#[test]") || compact.contains("::test]") {
                return true;
            }
            continue;
        }
        break;
    }

    false
}

fn rust_doc_comment(node: Node<'_>, source: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(source);
    let lines = text.lines().collect::<Vec<_>>();
    let mut row = node.start_position().row;
    if row == 0 {
        return None;
    }

    let mut doc_lines = Vec::new();
    while row > 0 {
        row -= 1;
        let line = lines.get(row).copied().unwrap_or_default().trim();
        if line.is_empty() {
            if doc_lines.is_empty() {
                continue;
            }
            break;
        }
        if line.starts_with("#[") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("///") {
            doc_lines.push(rest.trim().to_owned());
            continue;
        }
        break;
    }

    if doc_lines.is_empty() {
        None
    } else {
        doc_lines.reverse();
        let merged = doc_lines.join(" ");
        let normalized = normalize_intent_text(merged.as_str());
        (!normalized.is_empty()).then_some(normalized)
    }
}
