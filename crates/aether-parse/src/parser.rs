use std::collections::HashMap;
use std::path::Path;

use aether_core::{
    EdgeKind, Language, Position, SourceRange, Symbol, SymbolEdge, SymbolKind, content_hash,
    file_source_id, normalize_for_fingerprint, normalize_path, signature_fingerprint,
    stable_symbol_id,
};
use anyhow::{Result, anyhow};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, Parser, Point, QueryCursor};

use crate::registry::{LanguageConfig, LanguageRegistry, QueryCaptures, default_registry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedFile {
    pub symbols: Vec<Symbol>,
    pub edges: Vec<SymbolEdge>,
    pub test_intents: Vec<TestIntent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestIntent {
    pub file_path: String,
    pub test_name: String,
    pub intent_text: String,
    pub group_label: Option<String>,
    pub language: Language,
    pub symbol_id: Option<String>,
}

pub struct SymbolExtractor {
    registry: LanguageRegistry,
    parsers: HashMap<&'static str, Parser>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RustUsePrefix {
    Crate,
    Self_,
    Super,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustUsePathAtCursor {
    pub prefix: RustUsePrefix,
    pub segments: Vec<String>,
    pub cursor_segment_index: Option<usize>,
}

impl SymbolExtractor {
    pub fn new() -> Result<Self> {
        let registry = default_registry();
        let mut parsers = HashMap::new();

        for config in registry.configs() {
            let mut parser = Parser::new();
            parser
                .set_language(&config.ts_language)
                .map_err(|_| anyhow!("failed to load {} tree-sitter grammar", config.id))?;
            parsers.insert(config.id, parser);
        }

        Ok(Self { registry, parsers })
    }

    pub fn extract_from_path(&mut self, path: &Path, source: &str) -> Result<Vec<Symbol>> {
        Ok(self.extract_with_edges_from_path(path, source)?.symbols)
    }

    pub fn extract_from_source(
        &mut self,
        language: Language,
        file_path: &str,
        source: &str,
    ) -> Result<Vec<Symbol>> {
        Ok(self
            .extract_with_edges_from_source(language, file_path, source)?
            .symbols)
    }

    pub fn extract_with_edges_from_path(
        &mut self,
        path: &Path,
        source: &str,
    ) -> Result<ExtractedFile> {
        let language = language_for_path(path)
            .ok_or_else(|| anyhow!("unsupported file extension: {}", path.display()))?;
        let file_path = normalize_path(&path.to_string_lossy());
        let config_id = self
            .registry
            .get_by_path(path)
            .map(|config| config.id)
            .ok_or_else(|| anyhow!("no parser config for extension: {}", path.display()))?;

        extract_with_config(
            &self.registry,
            &mut self.parsers,
            language,
            &file_path,
            source,
            config_id,
        )
    }

    pub fn extract_with_edges_from_source(
        &mut self,
        language: Language,
        file_path: &str,
        source: &str,
    ) -> Result<ExtractedFile> {
        let normalized_file_path = normalize_path(file_path);
        let config_id = self
            .config_for_language_and_path(language, &normalized_file_path)
            .map(|config| config.id)
            .ok_or_else(|| anyhow!("unsupported file extension for {normalized_file_path}"))?;

        extract_with_config(
            &self.registry,
            &mut self.parsers,
            language,
            &normalized_file_path,
            source,
            config_id,
        )
    }

    fn config_for_language_and_path(
        &self,
        language: Language,
        file_path: &str,
    ) -> Option<&LanguageConfig> {
        let by_path = self.registry.get_by_path(Path::new(file_path));
        if by_path.is_some() {
            return by_path;
        }

        let fallback_id = match language {
            Language::Rust => "rust",
            Language::TypeScript => "typescript",
            Language::Tsx | Language::JavaScript | Language::Jsx => "tsx_js",
            Language::Python => "python",
        };
        self.registry.get_by_id(fallback_id)
    }
}

fn extract_with_config(
    registry: &LanguageRegistry,
    parsers: &mut HashMap<&'static str, Parser>,
    language: Language,
    file_path: &str,
    source: &str,
    config_id: &'static str,
) -> Result<ExtractedFile> {
    let config = registry
        .get_by_id(config_id)
        .ok_or_else(|| anyhow!("missing language config {config_id}"))?;
    let parser = parsers
        .get_mut(config.id)
        .ok_or_else(|| anyhow!("missing parser for language config {}", config.id))?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("tree-sitter parser produced no syntax tree"))?;

    let root = tree.root_node();
    let source_bytes = source.as_bytes();

    let mut symbols = extract_symbols(language, file_path, source_bytes, root, config);
    symbols.sort_by(|a, b| a.id.cmp(&b.id));

    let mut edges = extract_edges(language, file_path, source_bytes, root, config, &symbols);
    sort_and_dedupe_edges(&mut edges);

    let mut test_intents =
        extract_test_intents(language, file_path, source_bytes, root, config, &symbols);
    sort_and_dedupe_test_intents(&mut test_intents);

    Ok(ExtractedFile {
        symbols,
        edges,
        test_intents,
    })
}

fn extract_symbols(
    language: Language,
    file_path: &str,
    source: &[u8],
    root: Node<'_>,
    config: &LanguageConfig,
) -> Vec<Symbol> {
    let mut cursor = QueryCursor::new();
    let mut symbols = Vec::new();

    let mut query_matches = cursor.matches(&config.symbol_query, root, source);
    while {
        query_matches.advance();
        query_matches.get().is_some()
    } {
        let matched = query_matches.get().expect("query match should exist");
        let captures = QueryCaptures::new(
            &config.symbol_query,
            matched.captures,
            matched.pattern_index,
        );

        if let Some(hooks) = config.hooks.as_ref()
            && let Some(symbol) = hooks.map_symbol(language, &captures, source, file_path)
        {
            symbols.push(symbol);
            continue;
        }

        if let Some(symbol) = default_map_symbol(language, file_path, source, config, &captures) {
            symbols.push(symbol);
        }
    }

    symbols
}

fn extract_edges(
    language: Language,
    file_path: &str,
    source: &[u8],
    root: Node<'_>,
    config: &LanguageConfig,
    symbols: &[Symbol],
) -> Vec<SymbolEdge> {
    let mut cursor = QueryCursor::new();
    let mut edges = Vec::new();

    let mut query_matches = cursor.matches(&config.edge_query, root, source);
    while {
        query_matches.advance();
        query_matches.get().is_some()
    } {
        let matched = query_matches.get().expect("query match should exist");
        let captures =
            QueryCaptures::new(&config.edge_query, matched.captures, matched.pattern_index);

        if let Some(hooks) = config.hooks.as_ref()
            && let Some(mapped) = hooks.map_edge(language, &captures, source, file_path, symbols)
        {
            edges.extend(mapped);
            continue;
        }

        if let Some(mapped) = default_map_edge(language, file_path, source, &captures, symbols) {
            edges.extend(mapped);
        }
    }

    edges
}

fn extract_test_intents(
    language: Language,
    file_path: &str,
    source: &[u8],
    root: Node<'_>,
    config: &LanguageConfig,
    symbols: &[Symbol],
) -> Vec<TestIntent> {
    let Some(query) = config.test_intent_query.as_ref() else {
        return Vec::new();
    };

    let mut cursor = QueryCursor::new();
    let mut intents = Vec::new();
    let mut query_matches = cursor.matches(query, root, source);
    while {
        query_matches.advance();
        query_matches.get().is_some()
    } {
        let matched = query_matches.get().expect("query match should exist");
        let captures = QueryCaptures::new(query, matched.captures, matched.pattern_index);
        if let Some(hooks) = config.hooks.as_ref()
            && let Some(mapped) =
                hooks.map_test_intent(language, &captures, source, file_path, symbols)
        {
            intents.extend(mapped);
        }
    }

    intents
}

fn default_map_symbol(
    language: Language,
    file_path: &str,
    source: &[u8],
    config: &LanguageConfig,
    captures: &QueryCaptures<'_, '_>,
) -> Option<Symbol> {
    let name = captures.capture_text("name", source)?;
    let name = sanitize_name(name);
    if name.is_empty() {
        return None;
    }

    let kind = captures
        .capture_text("kind", source)
        .and_then(|text| parse_symbol_kind(text.trim()))
        .or_else(|| {
            captures
                .first_capture_name_with_prefix("symbol.")
                .and_then(symbol_kind_from_capture_name)
        })?;

    let node = captures
        .node("body")
        .or_else(|| captures.node_with_prefix("symbol."))
        .or_else(|| captures.node("name"))?;

    let parent = captures.capture_text("parent", source).map(sanitize_name);

    let qualified_name = config
        .hooks
        .as_ref()
        .and_then(|hooks| hooks.qualify_name(file_path, &name, parent.as_deref()))
        .unwrap_or_else(|| default_qualify_name(file_path, &name, parent.as_deref()));

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

fn default_map_edge(
    _language: Language,
    file_path: &str,
    source: &[u8],
    captures: &QueryCaptures<'_, '_>,
    symbols: &[Symbol],
) -> Option<Vec<SymbolEdge>> {
    let target = captures.capture_text("target", source).or_else(|| {
        captures
            .node_with_prefix("edge.")
            .map(|node| node_text(node, source))
    })?;

    let target = sanitize_name(target);
    if target.is_empty() {
        return None;
    }

    let edge_kind = captures
        .capture_text("edge_kind", source)
        .and_then(|value| parse_edge_kind(value.trim()))
        .or_else(|| {
            captures
                .first_capture_name_with_prefix("edge.")
                .and_then(edge_kind_from_capture_name)
        })
        .unwrap_or(EdgeKind::Calls);

    let source_id = captures
        .capture_text("source_id", source)
        .map(sanitize_name)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            if edge_kind == EdgeKind::Calls {
                captures
                    .node_with_prefix("edge.")
                    .and_then(|node| enclosing_function_symbol_id(symbols, node))
            } else {
                None
            }
        })
        .unwrap_or_else(|| file_source_id(file_path));

    Some(vec![SymbolEdge {
        source_id,
        target_qualified_name: target,
        edge_kind,
        file_path: file_path.to_owned(),
    }])
}

fn default_qualify_name(file_path: &str, symbol_name: &str, parent: Option<&str>) -> String {
    let module = Path::new(file_path)
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "module".to_owned());

    match parent {
        Some(parent) if !parent.trim().is_empty() => {
            format!("{module}::{parent}::{symbol_name}")
        }
        _ => format!("{module}::{symbol_name}"),
    }
}

fn parse_symbol_kind(value: &str) -> Option<SymbolKind> {
    match value {
        "function" => Some(SymbolKind::Function),
        "method" => Some(SymbolKind::Method),
        "class" => Some(SymbolKind::Class),
        "variable" => Some(SymbolKind::Variable),
        "struct" => Some(SymbolKind::Struct),
        "enum" => Some(SymbolKind::Enum),
        "trait" => Some(SymbolKind::Trait),
        "interface" => Some(SymbolKind::Interface),
        "type_alias" => Some(SymbolKind::TypeAlias),
        _ => None,
    }
}

fn symbol_kind_from_capture_name(capture_name: &str) -> Option<SymbolKind> {
    match capture_name.strip_prefix("symbol.")? {
        "function" => Some(SymbolKind::Function),
        "method" => Some(SymbolKind::Method),
        "class" => Some(SymbolKind::Class),
        "variable" => Some(SymbolKind::Variable),
        "struct" => Some(SymbolKind::Struct),
        "enum" => Some(SymbolKind::Enum),
        "trait" => Some(SymbolKind::Trait),
        "interface" => Some(SymbolKind::Interface),
        "type_alias" => Some(SymbolKind::TypeAlias),
        _ => None,
    }
}

fn parse_edge_kind(value: &str) -> Option<EdgeKind> {
    match value {
        "calls" => Some(EdgeKind::Calls),
        "depends_on" => Some(EdgeKind::DependsOn),
        _ => None,
    }
}

fn edge_kind_from_capture_name(capture_name: &str) -> Option<EdgeKind> {
    match capture_name.strip_prefix("edge.")? {
        "call" => Some(EdgeKind::Calls),
        "depends_on" => Some(EdgeKind::DependsOn),
        _ => None,
    }
}

fn sanitize_name(value: String) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_end_matches(';')
        .trim()
        .to_owned()
}

pub fn language_for_path(path: &Path) -> Option<Language> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    match ext.as_str() {
        "rs" => Some(Language::Rust),
        "ts" => Some(Language::TypeScript),
        "tsx" => Some(Language::Tsx),
        "js" => Some(Language::JavaScript),
        "jsx" => Some(Language::Jsx),
        "py" | "pyi" => Some(Language::Python),
        _ => None,
    }
}

pub fn rust_use_path_at_cursor(
    source: &str,
    cursor_line_0: usize,
    cursor_col_0: usize,
) -> Option<RustUsePathAtCursor> {
    let mut parser = Parser::new();
    parser.set_language(&rust_language()).ok()?;
    let tree = parser.parse(source, None)?;
    let root = tree.root_node();
    let cursor = Point {
        row: cursor_line_0,
        column: cursor_col_0,
    };
    let mut node = root.named_descendant_for_point_range(cursor, cursor)?;
    while node.kind() != "use_declaration" {
        node = node.parent()?;
    }

    let scoped = find_smallest_scoped_identifier_at_point(node, cursor)?;
    let mut segments = collect_scoped_identifier_segments(scoped, source.as_bytes());
    if segments.len() < 2 {
        return None;
    }

    let prefix = match segments[0].0.as_str() {
        "crate" => RustUsePrefix::Crate,
        "self" => RustUsePrefix::Self_,
        "super" => RustUsePrefix::Super,
        _ => return None,
    };

    let cursor_segment_index = segments
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(index, (_, node))| point_in_node(*node, cursor).then_some(index - 1));
    let tail = segments
        .drain(1..)
        .map(|(text, _)| text)
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if tail.is_empty() {
        return None;
    }

    Some(RustUsePathAtCursor {
        prefix,
        segments: tail,
        cursor_segment_index,
    })
}

pub(crate) fn build_symbol(
    language: Language,
    file_path: &str,
    kind: SymbolKind,
    name: &str,
    qualified_name: &str,
    node: Node<'_>,
    source: &[u8],
) -> Symbol {
    let symbol_text = node_text(node, source);
    let signature_text = declaration_prefix(node, source);
    let signature = signature_fingerprint(&signature_text);
    let id = stable_symbol_id(language, file_path, kind, qualified_name, &signature);

    Symbol {
        id,
        language,
        file_path: file_path.to_owned(),
        kind,
        name: name.to_owned(),
        qualified_name: qualified_name.to_owned(),
        signature_fingerprint: signature,
        content_hash: content_hash(&symbol_text),
        range: node_range(node),
    }
}

pub(crate) fn declaration_prefix(node: Node<'_>, source: &[u8]) -> String {
    let start = node.start_byte();
    let end = node
        .child_by_field_name("body")
        .map(|body| body.start_byte())
        .unwrap_or_else(|| node.end_byte());
    byte_range_text(source, start, end)
}

pub(crate) fn has_ancestor_kind(node: Node<'_>, kind: &str) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        if parent.kind() == kind {
            return true;
        }
        current = parent.parent();
    }
    false
}

pub(crate) fn named_child_text(node: Node<'_>, field_name: &str, source: &[u8]) -> Option<String> {
    let child = node.child_by_field_name(field_name)?;
    let text = node_text(child, source);
    let trimmed = sanitize_name(text);
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

pub(crate) fn node_text(node: Node<'_>, source: &[u8]) -> String {
    byte_range_text(source, node.start_byte(), node.end_byte())
}

pub(crate) fn node_range(node: Node<'_>) -> SourceRange {
    SourceRange {
        start: point_to_position(node.start_position()),
        end: point_to_position(node.end_position()),
    }
}

pub(crate) fn rust_call_target(node: Node<'_>, source: &[u8]) -> Option<String> {
    let callee = node
        .child_by_field_name("function")
        .or_else(|| node.named_child(0))?;
    let text = node_text(callee, source);
    let target = text.trim();
    (!target.is_empty()).then(|| target.to_owned())
}

pub(crate) fn rust_use_target(node: Node<'_>, source: &[u8]) -> Option<String> {
    let text = node_text(node, source);
    let text = text.trim();
    let text = text
        .strip_prefix("use")
        .map(str::trim_start)
        .unwrap_or(text);
    let text = text.trim_end_matches(';').trim();
    (!text.is_empty()).then(|| text.to_owned())
}

pub(crate) fn sort_and_dedupe_edges(edges: &mut Vec<SymbolEdge>) {
    edges.sort_by(|left, right| {
        left.source_id
            .cmp(&right.source_id)
            .then_with(|| left.target_qualified_name.cmp(&right.target_qualified_name))
            .then_with(|| left.edge_kind.cmp(&right.edge_kind))
            .then_with(|| left.file_path.cmp(&right.file_path))
    });

    edges.dedup_by(|left, right| {
        left.source_id == right.source_id
            && left.target_qualified_name == right.target_qualified_name
            && left.edge_kind == right.edge_kind
            && left.file_path == right.file_path
    });
}

pub(crate) fn sort_and_dedupe_test_intents(intents: &mut Vec<TestIntent>) {
    intents.sort_by(|left, right| {
        left.file_path
            .cmp(&right.file_path)
            .then_with(|| left.test_name.cmp(&right.test_name))
            .then_with(|| left.intent_text.cmp(&right.intent_text))
            .then_with(|| left.group_label.cmp(&right.group_label))
            .then_with(|| left.language.cmp(&right.language))
            .then_with(|| left.symbol_id.cmp(&right.symbol_id))
    });
    intents.dedup_by(|left, right| {
        left.file_path == right.file_path
            && left.test_name == right.test_name
            && left.intent_text == right.intent_text
            && left.group_label == right.group_label
            && left.language == right.language
            && left.symbol_id == right.symbol_id
    });
}

pub(crate) fn humanize_test_name(name: &str) -> String {
    let normalized = name.trim();
    if normalized.is_empty() {
        return String::new();
    }

    let lowered = normalized.to_ascii_lowercase();
    let without_prefix = if let Some(rest) = lowered.strip_prefix("test_") {
        rest
    } else if let Some(rest) = lowered.strip_prefix("test") {
        rest.trim_start_matches('_')
    } else {
        lowered.as_str()
    };

    without_prefix
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn normalize_intent_text(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .replace('\n', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn typescript_call_target(node: Node<'_>, source: &[u8]) -> Option<String> {
    let callee = node
        .child_by_field_name("function")
        .or_else(|| node.child_by_field_name("constructor"))
        .or_else(|| node.named_child(0))?;
    let text = node_text(callee, source);
    let target = text.trim();
    (!target.is_empty()).then(|| target.to_owned())
}

pub(crate) fn enclosing_function_symbol_id(symbols: &[Symbol], node: Node<'_>) -> Option<String> {
    let start = node.start_position();
    let end = node.end_position();

    symbols
        .iter()
        .filter(|symbol| matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method))
        .filter(|symbol| {
            let symbol_start = point_from_position(symbol.range.start);
            let symbol_end = point_from_position(symbol.range.end);
            (symbol_start.row, symbol_start.column) <= (start.row, start.column)
                && (end.row, end.column) <= (symbol_end.row, symbol_end.column)
        })
        .min_by_key(|symbol| {
            let span = symbol
                .range
                .end
                .line
                .saturating_sub(symbol.range.start.line);
            let width = symbol
                .range
                .end
                .column
                .saturating_sub(symbol.range.start.column);
            (span, width)
        })
        .map(|symbol| symbol.id.clone())
}

pub(crate) fn rust_qualified_name(node: Node<'_>, name: &str, source: &[u8]) -> String {
    qualify(&collect_rust_context(node, source), name)
}

pub(crate) fn typescript_qualified_name(node: Node<'_>, name: &str, source: &[u8]) -> String {
    qualify(&collect_typescript_context(node, source), name)
}

pub(crate) fn rust_source_function_id(
    language: Language,
    file_path: &str,
    source: &[u8],
    node: Node<'_>,
) -> Option<String> {
    let function_node = nearest_ancestor(node, "function_item")?;
    let name = named_child_text(function_node, "name", source)?;
    let kind = if has_ancestor_kind(function_node, "impl_item") {
        SymbolKind::Method
    } else {
        SymbolKind::Function
    };
    let qualified_name = rust_qualified_name(function_node, &name, source);
    let signature = signature_fingerprint(&declaration_prefix(function_node, source));
    Some(stable_symbol_id(
        language,
        file_path,
        kind,
        &qualified_name,
        &signature,
    ))
}

pub(crate) fn typescript_source_function_id(
    language: Language,
    file_path: &str,
    source: &[u8],
    node: Node<'_>,
) -> Option<String> {
    let function_node = nearest_typescript_function_ancestor(node)?;
    let kind = if function_node.kind() == "method_definition" {
        SymbolKind::Method
    } else {
        SymbolKind::Function
    };
    let name = named_child_text(function_node, "name", source)?;
    let qualified_name = typescript_qualified_name(function_node, &name, source);
    let signature = signature_fingerprint(&declaration_prefix(function_node, source));
    Some(stable_symbol_id(
        language,
        file_path,
        kind,
        &qualified_name,
        &signature,
    ))
}

fn nearest_typescript_function_ancestor(node: Node<'_>) -> Option<Node<'_>> {
    let mut current = Some(node);
    while let Some(cursor) = current {
        if cursor.kind() == "function_declaration" || cursor.kind() == "method_definition" {
            return Some(cursor);
        }
        current = cursor.parent();
    }
    None
}

fn nearest_ancestor<'tree>(mut node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    loop {
        if node.kind() == kind {
            return Some(node);
        }
        node = node.parent()?;
    }
}

fn collect_rust_context(node: Node<'_>, source: &[u8]) -> Vec<String> {
    let mut context = Vec::new();
    let mut current = node.parent();

    while let Some(cursor) = current {
        match cursor.kind() {
            "mod_item" => {
                if let Some(name) = named_child_text(cursor, "name", source) {
                    context.push(name);
                }
            }
            "impl_item" => {
                let target = cursor
                    .child_by_field_name("type")
                    .map(|ty| normalize_for_fingerprint(&node_text(ty, source)))
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| "impl".to_owned());
                context.push(target);
            }
            "trait_item" => {
                if let Some(name) = named_child_text(cursor, "name", source) {
                    context.push(name);
                }
            }
            _ => {}
        }

        current = cursor.parent();
    }

    context.reverse();
    context
}

fn collect_typescript_context(node: Node<'_>, source: &[u8]) -> Vec<String> {
    let mut context = Vec::new();
    let mut current = node.parent();

    while let Some(cursor) = current {
        match cursor.kind() {
            "class_declaration" | "interface_declaration" => {
                if let Some(name) = named_child_text(cursor, "name", source) {
                    context.push(name);
                }
            }
            _ => {}
        }
        current = cursor.parent();
    }

    context.reverse();
    context
}

fn qualify(context: &[String], name: &str) -> String {
    if context.is_empty() {
        name.to_owned()
    } else {
        format!("{}::{}", context.join("::"), name)
    }
}

fn point_from_position(position: Position) -> Point {
    Point {
        row: position.line.saturating_sub(1),
        column: position.column.saturating_sub(1),
    }
}

fn byte_range_text(source: &[u8], start: usize, end: usize) -> String {
    if start >= end || end > source.len() {
        return String::new();
    }
    String::from_utf8_lossy(&source[start..end]).into_owned()
}

fn point_to_position(point: Point) -> Position {
    Position {
        line: point.row + 1,
        column: point.column + 1,
    }
}

fn rust_language() -> tree_sitter::Language {
    tree_sitter_rust::LANGUAGE.into()
}

fn point_in_node(node: Node<'_>, point: Point) -> bool {
    let start = node.start_position();
    let end = node.end_position();
    (start.row, start.column) <= (point.row, point.column)
        && (point.row, point.column) < (end.row, end.column)
}

fn find_smallest_scoped_identifier_at_point<'a>(node: Node<'a>, point: Point) -> Option<Node<'a>> {
    let mut best: Option<Node<'a>> = None;
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if current.kind() == "scoped_identifier" && point_in_node(current, point) {
            best = match best {
                Some(existing) if existing.byte_range().len() >= current.byte_range().len() => {
                    Some(existing)
                }
                _ => Some(current),
            };
        }

        let mut cursor = current.walk();
        for child in current.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    best
}

fn collect_scoped_identifier_segments<'a>(
    node: Node<'a>,
    source: &[u8],
) -> Vec<(String, Node<'a>)> {
    match node.kind() {
        "scoped_identifier" => {
            let mut segments = Vec::new();
            if let Some(path) = node.child_by_field_name("path") {
                segments.extend(collect_scoped_identifier_segments(path, source));
            }
            if let Some(name) = node.child_by_field_name("name") {
                segments.extend(collect_scoped_identifier_segments(name, source));
            }
            segments
        }
        "identifier" | "self" | "super" | "crate" => {
            let text = node_text(node, source).trim().to_owned();
            if text.is_empty() {
                Vec::new()
            } else {
                vec![(text, node)]
            }
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract(language: Language, path: &str, source: &str) -> Vec<Symbol> {
        let mut extractor = SymbolExtractor::new().expect("extractor");
        extractor
            .extract_from_source(language, path, source)
            .expect("extract")
    }

    fn extract_with_edges(language: Language, path: &str, source: &str) -> ExtractedFile {
        let mut extractor = SymbolExtractor::new().expect("extractor");
        extractor
            .extract_with_edges_from_source(language, path, source)
            .expect("extract with edges")
    }

    #[test]
    fn extracts_rust_symbols_and_qualified_methods() {
        let source = r#"
struct Widget;

type WidgetId = u64;

impl Widget {
    fn run(&self) -> bool {
        true
    }
}

fn helper() -> i32 {
    1
}
"#;

        let symbols = extract(Language::Rust, "src/lib.rs", source);
        let names: Vec<String> = symbols.iter().map(|s| s.qualified_name.clone()).collect();

        assert!(names.contains(&"Widget".to_owned()));
        assert!(names.contains(&"WidgetId".to_owned()));
        assert!(names.contains(&"Widget::run".to_owned()));
        assert!(names.contains(&"helper".to_owned()));
    }

    #[test]
    fn extracts_typescript_symbols_and_methods() {
        let source = r#"
function topLevel(value: number): number {
  return value + 1;
}

class Greeter {
  greet(name: string): string {
    return `hello ${name}`;
  }
}

interface User {
  id: string;
}

type UserId = string;
"#;

        let symbols = extract(Language::TypeScript, "src/index.ts", source);
        let names: Vec<String> = symbols.iter().map(|s| s.qualified_name.clone()).collect();

        assert!(names.contains(&"topLevel".to_owned()));
        assert!(names.contains(&"Greeter".to_owned()));
        assert!(names.contains(&"Greeter::greet".to_owned()));
        assert!(names.contains(&"User".to_owned()));
        assert!(names.contains(&"UserId".to_owned()));
    }

    #[test]
    fn extracts_rust_test_intents_from_names_and_doc_comments() {
        let source = r#"
#[test]
fn test_handles_negative_balance() {}

/// returns none for missing symbol
#[test]
fn lookup_missing_symbol_returns_none() {}

fn helper() {}
"#;

        let extracted = extract_with_edges(Language::Rust, "src/lib.rs", source);
        let intents = extracted
            .test_intents
            .iter()
            .map(|intent| intent.intent_text.clone())
            .collect::<Vec<_>>();
        assert!(intents.contains(&"handles negative balance".to_owned()));
        assert!(intents.contains(&"returns none for missing symbol".to_owned()));
        assert!(
            extracted
                .test_intents
                .iter()
                .all(|intent| intent.symbol_id.is_some())
        );
    }

    #[test]
    fn extracts_typescript_test_intents_from_it_test_and_describe() {
        let source = r#"
describe("PaymentService", () => {
  it("handles negative balance", () => {});
  test("returns none for missing symbol", () => {});
});
"#;

        let extracted = extract_with_edges(Language::TypeScript, "src/payment.test.ts", source);
        let intents = extracted
            .test_intents
            .iter()
            .map(|intent| intent.intent_text.clone())
            .collect::<Vec<_>>();
        assert!(intents.contains(&"PaymentService".to_owned()));
        assert!(intents.contains(&"handles negative balance".to_owned()));
        assert!(intents.contains(&"returns none for missing symbol".to_owned()));
        assert!(
            extracted
                .test_intents
                .iter()
                .any(|intent| intent.group_label.as_deref() == Some("PaymentService"))
        );
    }

    #[test]
    fn extracts_python_test_intents_from_names_and_docstrings() {
        let source = r#"
def test_handles_negative_balance():
    """handles negative balance"""
    assert True

def test_returns_none_for_missing_symbol():
    assert True

def helper():
    return 1
"#;

        let extracted = extract_with_edges(Language::Python, "tests/test_payments.py", source);
        let intents = extracted
            .test_intents
            .iter()
            .map(|intent| intent.intent_text.clone())
            .collect::<Vec<_>>();
        assert!(intents.contains(&"handles negative balance".to_owned()));
        assert!(intents.contains(&"returns none for missing symbol".to_owned()));
    }

    #[test]
    fn extracts_rust_calls_edges() {
        let source = r#"
fn beta() {}

fn alpha() {
    beta();
}
"#;

        let extracted = extract_with_edges(Language::Rust, "src/lib.rs", source);
        let alpha = extracted
            .symbols
            .iter()
            .find(|symbol| symbol.qualified_name == "alpha")
            .expect("alpha symbol should exist");

        let calls = extracted
            .edges
            .iter()
            .filter(|edge| edge.edge_kind == EdgeKind::Calls)
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].source_id, alpha.id);
        assert_eq!(calls[0].target_qualified_name, "beta");
        assert_eq!(calls[0].file_path, "src/lib.rs");
    }

    #[test]
    fn extracts_typescript_dependency_edges_from_imports() {
        let source = r#"
import { run } from "./dep";

function alpha() {
    return run();
}
"#;

        let extracted = extract_with_edges(Language::TypeScript, "src/app.ts", source);
        let depends = extracted
            .edges
            .iter()
            .filter(|edge| edge.edge_kind == EdgeKind::DependsOn)
            .collect::<Vec<_>>();

        assert_eq!(depends.len(), 1);
        assert_eq!(depends[0].source_id, file_source_id("src/app.ts"));
        assert_eq!(depends[0].target_qualified_name, "./dep");
        assert_eq!(depends[0].file_path, "src/app.ts");
    }

    #[test]
    fn preserves_symbol_id_when_line_offsets_shift() {
        let source_a = "fn add(x: i32, y: i32) -> i32 { x + y }\n";
        let source_b = "\n\nfn add(x: i32, y: i32) -> i32 { x + y }\n";

        let symbols_a = extract(Language::Rust, "src/math.rs", source_a);
        let symbols_b = extract(Language::Rust, "src/math.rs", source_b);

        assert_eq!(symbols_a.len(), 1);
        assert_eq!(symbols_b.len(), 1);
        assert_eq!(symbols_a[0].id, symbols_b[0].id);
    }

    #[test]
    fn changes_symbol_id_when_renamed() {
        let source_a = "fn add(x: i32, y: i32) -> i32 { x + y }\n";
        let source_b = "fn sum(x: i32, y: i32) -> i32 { x + y }\n";

        let symbols_a = extract(Language::Rust, "src/math.rs", source_a);
        let symbols_b = extract(Language::Rust, "src/math.rs", source_b);

        assert_eq!(symbols_a.len(), 1);
        assert_eq!(symbols_b.len(), 1);
        assert_ne!(symbols_a[0].id, symbols_b[0].id);
    }

    #[test]
    fn rust_use_path_at_cursor_extracts_crate_path_and_cursor_segment() {
        let source = "use crate::config::loader;\n";
        let loader_col = source.find("loader").expect("loader segment");
        let path = rust_use_path_at_cursor(source, 0, loader_col).expect("rust use path");
        assert_eq!(path.prefix, RustUsePrefix::Crate);
        assert_eq!(
            path.segments,
            vec!["config".to_owned(), "loader".to_owned()]
        );
        assert_eq!(path.cursor_segment_index, Some(1));
    }

    #[test]
    fn rust_use_path_at_cursor_returns_none_for_non_use_cursor() {
        let source = "fn alpha() {}\n";
        assert!(rust_use_path_at_cursor(source, 0, 3).is_none());
    }

    #[test]
    fn rust_use_path_at_cursor_ignores_external_crate_imports() {
        let source = "use serde::Deserialize;\n";
        assert!(rust_use_path_at_cursor(source, 0, 8).is_none());
    }
}
