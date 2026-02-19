use aether_core::{EdgeKind, Language, Symbol, SymbolEdge, SymbolKind, file_source_id};
use tree_sitter::{Node, Query};

use crate::parser::{
    TestIntent, build_symbol, has_ancestor_kind, humanize_test_name, normalize_intent_text,
    rust_call_target, rust_qualified_name, rust_source_function_id, rust_use_target,
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
                if has_ancestor_kind(node, "impl_item") {
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
