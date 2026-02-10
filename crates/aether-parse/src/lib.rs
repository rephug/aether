use std::path::Path;

use aether_core::{
    Language, Position, SourceRange, Symbol, SymbolKind, content_hash, normalize_for_fingerprint,
    normalize_path, signature_fingerprint, stable_symbol_id,
};
use anyhow::{Result, anyhow};
use tree_sitter::{Node, Parser, Point};

pub struct SymbolExtractor {
    rust_parser: Parser,
    ts_parser: Parser,
    tsx_parser: Parser,
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
        let language = language_for_path(path)
            .ok_or_else(|| anyhow!("unsupported file extension: {}", path.display()))?;
        let file_path = normalize_path(&path.to_string_lossy());
        self.extract_from_source(language, &file_path, source)
    }

    pub fn extract_from_source(
        &mut self,
        language: Language,
        file_path: &str,
        source: &str,
    ) -> Result<Vec<Symbol>> {
        let parser = match language {
            Language::Rust => &mut self.rust_parser,
            Language::TypeScript => &mut self.ts_parser,
            Language::Tsx | Language::JavaScript | Language::Jsx => &mut self.tsx_parser,
        };

        let tree = parser
            .parse(source, None)
            .ok_or_else(|| anyhow!("tree-sitter parser produced no syntax tree"))?;

        let root = tree.root_node();
        let source_bytes = source.as_bytes();
        let normalized_file_path = normalize_path(file_path);

        let mut symbols = Vec::new();
        let mut context = Vec::new();

        match language {
            Language::Rust => {
                visit_rust(
                    root,
                    source_bytes,
                    &normalized_file_path,
                    language,
                    &mut context,
                    &mut symbols,
                );
            }
            Language::TypeScript | Language::Tsx | Language::JavaScript | Language::Jsx => {
                visit_typescript(
                    root,
                    source_bytes,
                    &normalized_file_path,
                    language,
                    &mut context,
                    &mut symbols,
                );
            }
        }

        symbols.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(symbols)
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

fn visit_rust(
    node: Node<'_>,
    source: &[u8],
    file_path: &str,
    language: Language,
    context: &mut Vec<String>,
    symbols: &mut Vec<Symbol>,
) {
    match node.kind() {
        "mod_item" => {
            if let Some(name) = named_child_text(node, "name", source) {
                context.push(name);
                visit_children(
                    node, source, file_path, language, context, symbols, visit_rust,
                );
                context.pop();
                return;
            }
        }
        "impl_item" => {
            let target = node
                .child_by_field_name("type")
                .map(|ty| normalize_for_fingerprint(&node_text(ty, source)))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "impl".to_owned());
            context.push(target);
            visit_children(
                node, source, file_path, language, context, symbols, visit_rust,
            );
            context.pop();
            return;
        }
        "struct_item" => {
            if let Some(name) = named_child_text(node, "name", source) {
                push_symbol(
                    symbols,
                    language,
                    file_path,
                    SymbolKind::Struct,
                    &name,
                    context,
                    node_text(node, source),
                    declaration_prefix(node, source),
                    node,
                );
                context.push(name);
                visit_children(
                    node, source, file_path, language, context, symbols, visit_rust,
                );
                context.pop();
                return;
            }
        }
        "enum_item" => {
            if let Some(name) = named_child_text(node, "name", source) {
                push_symbol(
                    symbols,
                    language,
                    file_path,
                    SymbolKind::Enum,
                    &name,
                    context,
                    node_text(node, source),
                    declaration_prefix(node, source),
                    node,
                );
                context.push(name);
                visit_children(
                    node, source, file_path, language, context, symbols, visit_rust,
                );
                context.pop();
                return;
            }
        }
        "trait_item" => {
            if let Some(name) = named_child_text(node, "name", source) {
                push_symbol(
                    symbols,
                    language,
                    file_path,
                    SymbolKind::Trait,
                    &name,
                    context,
                    node_text(node, source),
                    declaration_prefix(node, source),
                    node,
                );
                context.push(name);
                visit_children(
                    node, source, file_path, language, context, symbols, visit_rust,
                );
                context.pop();
                return;
            }
        }
        "type_item" => {
            if let Some(name) = named_child_text(node, "name", source) {
                push_symbol(
                    symbols,
                    language,
                    file_path,
                    SymbolKind::TypeAlias,
                    &name,
                    context,
                    node_text(node, source),
                    declaration_prefix(node, source),
                    node,
                );
            }
        }
        "function_item" => {
            if let Some(name) = named_child_text(node, "name", source) {
                let symbol_kind = if has_ancestor_kind(node, "impl_item") {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                };
                push_symbol(
                    symbols,
                    language,
                    file_path,
                    symbol_kind,
                    &name,
                    context,
                    node_text(node, source),
                    declaration_prefix(node, source),
                    node,
                );
            }
        }
        _ => {}
    }

    visit_children(
        node, source, file_path, language, context, symbols, visit_rust,
    );
}

fn visit_typescript(
    node: Node<'_>,
    source: &[u8],
    file_path: &str,
    language: Language,
    context: &mut Vec<String>,
    symbols: &mut Vec<Symbol>,
) {
    match node.kind() {
        "class_declaration" => {
            if let Some(name) = named_child_text(node, "name", source) {
                push_symbol(
                    symbols,
                    language,
                    file_path,
                    SymbolKind::Class,
                    &name,
                    context,
                    node_text(node, source),
                    declaration_prefix(node, source),
                    node,
                );
                context.push(name);
                visit_children(
                    node,
                    source,
                    file_path,
                    language,
                    context,
                    symbols,
                    visit_typescript,
                );
                context.pop();
                return;
            }
        }
        "interface_declaration" => {
            if let Some(name) = named_child_text(node, "name", source) {
                push_symbol(
                    symbols,
                    language,
                    file_path,
                    SymbolKind::Interface,
                    &name,
                    context,
                    node_text(node, source),
                    declaration_prefix(node, source),
                    node,
                );
                context.push(name);
                visit_children(
                    node,
                    source,
                    file_path,
                    language,
                    context,
                    symbols,
                    visit_typescript,
                );
                context.pop();
                return;
            }
        }
        "function_declaration" => {
            if let Some(name) = named_child_text(node, "name", source) {
                push_symbol(
                    symbols,
                    language,
                    file_path,
                    SymbolKind::Function,
                    &name,
                    context,
                    node_text(node, source),
                    declaration_prefix(node, source),
                    node,
                );
            }
        }
        "method_definition" => {
            if let Some(name) = named_child_text(node, "name", source) {
                push_symbol(
                    symbols,
                    language,
                    file_path,
                    SymbolKind::Method,
                    &name,
                    context,
                    node_text(node, source),
                    declaration_prefix(node, source),
                    node,
                );
            }
        }
        "type_alias_declaration" => {
            if let Some(name) = named_child_text(node, "name", source) {
                push_symbol(
                    symbols,
                    language,
                    file_path,
                    SymbolKind::TypeAlias,
                    &name,
                    context,
                    node_text(node, source),
                    declaration_prefix(node, source),
                    node,
                );
            }
        }
        _ => {}
    }

    visit_children(
        node,
        source,
        file_path,
        language,
        context,
        symbols,
        visit_typescript,
    );
}

fn push_symbol(
    output: &mut Vec<Symbol>,
    language: Language,
    file_path: &str,
    kind: SymbolKind,
    name: &str,
    context: &[String],
    symbol_text: String,
    signature_text: String,
    node: Node<'_>,
) {
    if name.is_empty() {
        return;
    }

    let qualified_name = qualify(context, name);
    let sig_fingerprint = signature_fingerprint(&signature_text);
    let id = stable_symbol_id(language, file_path, kind, &qualified_name, &sig_fingerprint);

    output.push(Symbol {
        id,
        language,
        file_path: file_path.to_owned(),
        kind,
        name: name.to_owned(),
        qualified_name,
        signature_fingerprint: sig_fingerprint,
        content_hash: content_hash(&symbol_text),
        range: SourceRange {
            start: point_to_position(node.start_position()),
            end: point_to_position(node.end_position()),
        },
    });
}

fn visit_children(
    node: Node<'_>,
    source: &[u8],
    file_path: &str,
    language: Language,
    context: &mut Vec<String>,
    symbols: &mut Vec<Symbol>,
    visitor: fn(Node<'_>, &[u8], &str, Language, &mut Vec<String>, &mut Vec<Symbol>),
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visitor(child, source, file_path, language, context, symbols);
    }
}

fn named_child_text(node: Node<'_>, field_name: &str, source: &[u8]) -> Option<String> {
    let child = node.child_by_field_name(field_name)?;
    let text = node_text(child, source);
    let trimmed = text.trim().trim_matches('\"').to_owned();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
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
}
