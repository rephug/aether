use std::path::Path;

use aether_core::{
    EdgeKind, Language, Position, SourceRange, Symbol, SymbolEdge, SymbolKind, content_hash,
    file_source_id, normalize_for_fingerprint, normalize_path, signature_fingerprint,
    stable_symbol_id,
};
use anyhow::{Result, anyhow};
use tree_sitter::{Node, Parser, Point};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedFile {
    pub symbols: Vec<Symbol>,
    pub edges: Vec<SymbolEdge>,
}

pub struct SymbolExtractor {
    rust_parser: Parser,
    ts_parser: Parser,
    tsx_parser: Parser,
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
        let mut rust_parser = Parser::new();
        rust_parser
            .set_language(&rust_language())
            .map_err(|_| anyhow!("failed to load Rust tree-sitter grammar"))?;

        let mut ts_parser = Parser::new();
        ts_parser
            .set_language(&typescript_language())
            .map_err(|_| anyhow!("failed to load TypeScript tree-sitter grammar"))?;

        let mut tsx_parser = Parser::new();
        tsx_parser
            .set_language(&tsx_language())
            .map_err(|_| anyhow!("failed to load TSX tree-sitter grammar"))?;

        Ok(Self {
            rust_parser,
            ts_parser,
            tsx_parser,
        })
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
        self.extract_with_edges_from_source(language, &file_path, source)
    }

    pub fn extract_with_edges_from_source(
        &mut self,
        language: Language,
        file_path: &str,
        source: &str,
    ) -> Result<ExtractedFile> {
        let parser = match language {
            Language::Rust => &mut self.rust_parser,
            Language::TypeScript => &mut self.ts_parser,
            Language::Tsx | Language::JavaScript | Language::Jsx => &mut self.tsx_parser,
        };

        let tree = parser
            .parse(source, None)
            .ok_or_else(|| anyhow!("tree-sitter parser produced no syntax tree"))?;

        let root = tree.root_node();
        let normalized_file_path = normalize_path(file_path);
        let mut state = ParseState::new(language, &normalized_file_path, source.as_bytes());

        match language {
            Language::Rust => visit_rust(root, &mut state),
            Language::TypeScript | Language::Tsx | Language::JavaScript | Language::Jsx => {
                visit_typescript(root, &mut state)
            }
        }

        state.symbols.sort_by(|a, b| a.id.cmp(&b.id));
        sort_and_dedupe_edges(&mut state.edges);

        Ok(ExtractedFile {
            symbols: state.symbols,
            edges: state.edges,
        })
    }
}

pub fn language_for_path(path: &Path) -> Option<Language> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    match ext.as_str() {
        "rs" => Some(Language::Rust),
        "ts" => Some(Language::TypeScript),
        "tsx" => Some(Language::Tsx),
        "js" => Some(Language::JavaScript),
        "jsx" => Some(Language::Jsx),
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

struct ParseState<'a> {
    language: Language,
    file_path: &'a str,
    source: &'a [u8],
    context: Vec<String>,
    function_stack: Vec<String>,
    symbols: Vec<Symbol>,
    edges: Vec<SymbolEdge>,
}

impl<'a> ParseState<'a> {
    fn new(language: Language, file_path: &'a str, source: &'a [u8]) -> Self {
        Self {
            language,
            file_path,
            source,
            context: Vec::new(),
            function_stack: Vec::new(),
            symbols: Vec::new(),
            edges: Vec::new(),
        }
    }

    fn push_symbol_from_node(
        &mut self,
        kind: SymbolKind,
        name: &str,
        node: Node<'_>,
    ) -> Option<String> {
        if name.is_empty() {
            return None;
        }

        let symbol_text = node_text(node, self.source);
        let signature_text = declaration_prefix(node, self.source);
        let range = node_range(node);
        let qualified_name = qualify(&self.context, name);
        let sig_fingerprint = signature_fingerprint(&signature_text);
        let id = stable_symbol_id(
            self.language,
            self.file_path,
            kind,
            &qualified_name,
            &sig_fingerprint,
        );

        self.symbols.push(Symbol {
            id: id.clone(),
            language: self.language,
            file_path: self.file_path.to_owned(),
            kind,
            name: name.to_owned(),
            qualified_name,
            signature_fingerprint: sig_fingerprint,
            content_hash: content_hash(&symbol_text),
            range,
        });

        Some(id)
    }

    fn push_calls_edge(&mut self, target_qualified_name: String) {
        let Some(source_id) = self.function_stack.last() else {
            return;
        };

        if target_qualified_name.trim().is_empty() {
            return;
        }

        self.edges.push(SymbolEdge {
            source_id: source_id.clone(),
            target_qualified_name,
            edge_kind: EdgeKind::Calls,
            file_path: self.file_path.to_owned(),
        });
    }

    fn push_depends_on_edge(&mut self, target_qualified_name: String) {
        if target_qualified_name.trim().is_empty() {
            return;
        }

        self.edges.push(SymbolEdge {
            source_id: file_source_id(self.file_path),
            target_qualified_name,
            edge_kind: EdgeKind::DependsOn,
            file_path: self.file_path.to_owned(),
        });
    }
}

fn visit_rust(node: Node<'_>, state: &mut ParseState<'_>) {
    match node.kind() {
        "mod_item" => {
            if let Some(name) = named_child_text(node, "name", state.source) {
                state.context.push(name);
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    visit_rust(child, state);
                }
                state.context.pop();
                return;
            }
        }
        "impl_item" => {
            let target = node
                .child_by_field_name("type")
                .map(|ty| normalize_for_fingerprint(&node_text(ty, state.source)))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "impl".to_owned());
            state.context.push(target);
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                visit_rust(child, state);
            }
            state.context.pop();
            return;
        }
        "struct_item" => {
            if let Some(name) = named_child_text(node, "name", state.source) {
                let _ = state.push_symbol_from_node(SymbolKind::Struct, &name, node);
                state.context.push(name);
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    visit_rust(child, state);
                }
                state.context.pop();
                return;
            }
        }
        "enum_item" => {
            if let Some(name) = named_child_text(node, "name", state.source) {
                let _ = state.push_symbol_from_node(SymbolKind::Enum, &name, node);
                state.context.push(name);
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    visit_rust(child, state);
                }
                state.context.pop();
                return;
            }
        }
        "trait_item" => {
            if let Some(name) = named_child_text(node, "name", state.source) {
                let _ = state.push_symbol_from_node(SymbolKind::Trait, &name, node);
                state.context.push(name);
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    visit_rust(child, state);
                }
                state.context.pop();
                return;
            }
        }
        "type_item" => {
            if let Some(name) = named_child_text(node, "name", state.source) {
                let _ = state.push_symbol_from_node(SymbolKind::TypeAlias, &name, node);
            }
        }
        "function_item" => {
            if let Some(name) = named_child_text(node, "name", state.source) {
                let symbol_kind = if has_ancestor_kind(node, "impl_item") {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                };
                if let Some(symbol_id) = state.push_symbol_from_node(symbol_kind, &name, node) {
                    state.function_stack.push(symbol_id);
                    let mut cursor = node.walk();
                    for child in node.children(&mut cursor) {
                        visit_rust(child, state);
                    }
                    state.function_stack.pop();
                    return;
                }
            }
        }
        "call_expression" => {
            if let Some(target) = rust_call_target(node, state.source) {
                state.push_calls_edge(target);
            }
        }
        "use_declaration" => {
            if let Some(target) = rust_use_target(node, state.source) {
                state.push_depends_on_edge(target);
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit_rust(child, state);
    }
}

fn visit_typescript(node: Node<'_>, state: &mut ParseState<'_>) {
    match node.kind() {
        "class_declaration" => {
            if let Some(name) = named_child_text(node, "name", state.source) {
                let _ = state.push_symbol_from_node(SymbolKind::Class, &name, node);
                state.context.push(name);
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    visit_typescript(child, state);
                }
                state.context.pop();
                return;
            }
        }
        "interface_declaration" => {
            if let Some(name) = named_child_text(node, "name", state.source) {
                let _ = state.push_symbol_from_node(SymbolKind::Interface, &name, node);
                state.context.push(name);
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    visit_typescript(child, state);
                }
                state.context.pop();
                return;
            }
        }
        "function_declaration" => {
            if let Some(name) = named_child_text(node, "name", state.source)
                && let Some(symbol_id) =
                    state.push_symbol_from_node(SymbolKind::Function, &name, node)
            {
                state.function_stack.push(symbol_id);
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    visit_typescript(child, state);
                }
                state.function_stack.pop();
                return;
            }
        }
        "method_definition" => {
            if let Some(name) = named_child_text(node, "name", state.source)
                && let Some(symbol_id) =
                    state.push_symbol_from_node(SymbolKind::Method, &name, node)
            {
                state.function_stack.push(symbol_id);
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    visit_typescript(child, state);
                }
                state.function_stack.pop();
                return;
            }
        }
        "type_alias_declaration" => {
            if let Some(name) = named_child_text(node, "name", state.source) {
                let _ = state.push_symbol_from_node(SymbolKind::TypeAlias, &name, node);
            }
        }
        "call_expression" | "new_expression" => {
            if let Some(target) = typescript_call_target(node, state.source) {
                state.push_calls_edge(target);
            }
        }
        "import_declaration" | "import_statement" => {
            if let Some(target) = named_child_text(node, "source", state.source) {
                state.push_depends_on_edge(target);
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit_typescript(child, state);
    }
}

fn rust_call_target(node: Node<'_>, source: &[u8]) -> Option<String> {
    let callee = node
        .child_by_field_name("function")
        .or_else(|| node.named_child(0))?;
    let text = node_text(callee, source);
    let target = text.trim();
    (!target.is_empty()).then(|| target.to_owned())
}

fn rust_use_target(node: Node<'_>, source: &[u8]) -> Option<String> {
    let text = node_text(node, source);
    let text = text.trim();
    let text = text
        .strip_prefix("use")
        .map(str::trim_start)
        .unwrap_or(text);
    let text = text.trim_end_matches(';').trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn typescript_call_target(node: Node<'_>, source: &[u8]) -> Option<String> {
    let callee = node
        .child_by_field_name("function")
        .or_else(|| node.child_by_field_name("constructor"))
        .or_else(|| node.named_child(0))?;
    let text = node_text(callee, source);
    let target = text.trim();
    (!target.is_empty()).then(|| target.to_owned())
}

fn sort_and_dedupe_edges(edges: &mut Vec<SymbolEdge>) {
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

fn node_range(node: Node<'_>) -> SourceRange {
    SourceRange {
        start: point_to_position(node.start_position()),
        end: point_to_position(node.end_position()),
    }
}

fn named_child_text(node: Node<'_>, field_name: &str, source: &[u8]) -> Option<String> {
    let child = node.child_by_field_name(field_name)?;
    let text = node_text(child, source);
    let trimmed = text.trim().trim_matches('"').trim_matches('\'').to_owned();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
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

fn declaration_prefix(node: Node<'_>, source: &[u8]) -> String {
    let start = node.start_byte();
    let end = node
        .child_by_field_name("body")
        .map(|body| body.start_byte())
        .unwrap_or_else(|| node.end_byte());
    byte_range_text(source, start, end)
}

fn node_text(node: Node<'_>, source: &[u8]) -> String {
    byte_range_text(source, node.start_byte(), node.end_byte())
}

fn byte_range_text(source: &[u8], start: usize, end: usize) -> String {
    if start >= end || end > source.len() {
        return String::new();
    }
    String::from_utf8_lossy(&source[start..end]).into_owned()
}

fn qualify(context: &[String], name: &str) -> String {
    if context.is_empty() {
        name.to_owned()
    } else {
        format!("{}::{}", context.join("::"), name)
    }
}

fn point_to_position(point: Point) -> Position {
    Position {
        line: point.row + 1,
        column: point.column + 1,
    }
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

fn rust_language() -> tree_sitter::Language {
    tree_sitter_rust::LANGUAGE.into()
}

fn typescript_language() -> tree_sitter::Language {
    tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
}

fn tsx_language() -> tree_sitter::Language {
    tree_sitter_typescript::LANGUAGE_TSX.into()
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
