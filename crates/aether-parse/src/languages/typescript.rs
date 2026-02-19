use aether_core::{EdgeKind, Language, Symbol, SymbolEdge, SymbolKind, file_source_id};
use tree_sitter::{Node, Query};

use crate::parser::{
    TestIntent, build_symbol, normalize_intent_text, typescript_call_target,
    typescript_qualified_name, typescript_source_function_id,
};
use crate::registry::{LanguageConfig, LanguageHooks, QueryCaptures};

pub fn config() -> LanguageConfig {
    let ts_language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
    let symbol_query = Query::new(
        &ts_language,
        include_str!("../queries/typescript_symbols.scm"),
    )
    .expect("invalid typescript symbol query");
    let edge_query = Query::new(
        &ts_language,
        include_str!("../queries/typescript_edges.scm"),
    )
    .expect("invalid typescript edge query");
    let test_intent_query = Query::new(
        &ts_language,
        include_str!("../queries/typescript_test_intents.scm"),
    )
    .expect("invalid typescript test intent query");

    LanguageConfig {
        id: "typescript",
        extensions: &["ts"],
        ts_language,
        symbol_query,
        edge_query,
        test_intent_query: Some(test_intent_query),
        module_markers: &["package.json"],
        hooks: Some(Box::new(TypeScriptHooks)),
    }
}

pub fn tsx_js_config() -> LanguageConfig {
    let ts_language = tree_sitter_typescript::LANGUAGE_TSX.into();
    let symbol_query = Query::new(
        &ts_language,
        include_str!("../queries/typescript_symbols.scm"),
    )
    .expect("invalid typescript symbol query");
    let edge_query = Query::new(
        &ts_language,
        include_str!("../queries/typescript_edges.scm"),
    )
    .expect("invalid typescript edge query");
    let test_intent_query = Query::new(
        &ts_language,
        include_str!("../queries/typescript_test_intents.scm"),
    )
    .expect("invalid typescript test intent query");

    LanguageConfig {
        id: "tsx_js",
        extensions: &["tsx", "js", "jsx"],
        ts_language,
        symbol_query,
        edge_query,
        test_intent_query: Some(test_intent_query),
        module_markers: &["package.json"],
        hooks: Some(Box::new(TypeScriptHooks)),
    }
}

struct TypeScriptHooks;

impl LanguageHooks for TypeScriptHooks {
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
            "symbol.class" => SymbolKind::Class,
            "symbol.interface" => SymbolKind::Interface,
            "symbol.function" => SymbolKind::Function,
            "symbol.method" => SymbolKind::Method,
            "symbol.type_alias" => SymbolKind::TypeAlias,
            _ => return None,
        };

        let qualified_name = typescript_qualified_name(node, &name, source);
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
                let target = typescript_call_target(node, source)?;
                let source_id = typescript_source_function_id(language, file_path, source, node)?;
                Some(vec![SymbolEdge {
                    source_id,
                    target_qualified_name: target,
                    edge_kind: EdgeKind::Calls,
                    file_path: file_path.to_owned(),
                }])
            }
            "edge.depends_on" => {
                let target = captures.capture_text("source", source)?;
                let target = target
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_owned();
                if target.is_empty() {
                    return None;
                }
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
        if node.kind() != "call_expression" {
            return None;
        }

        let function = node.child_by_field_name("function")?;
        let callee = call_name(function, source)?;
        if !matches!(callee.as_str(), "it" | "test" | "describe") {
            return None;
        }

        let label = first_string_argument(node, source)?;
        let intent_text = normalize_intent_text(label.as_str());
        if intent_text.is_empty() {
            return None;
        }

        let group_label = if callee == "describe" {
            None
        } else {
            nearest_describe_label(node, source)
        };

        Some(vec![TestIntent {
            file_path: file_path.to_owned(),
            test_name: callee,
            intent_text,
            group_label,
            language,
            symbol_id: None,
        }])
    }
}

fn call_name(function: Node<'_>, source: &[u8]) -> Option<String> {
    match function.kind() {
        "identifier" => Some(crate::parser::node_text(function, source)),
        "member_expression" => function
            .child_by_field_name("property")
            .or_else(|| function.named_child(1))
            .map(|node| crate::parser::node_text(node, source)),
        _ => None,
    }
    .map(|value| value.trim().to_owned())
    .filter(|value| !value.is_empty())
}

fn first_string_argument(call: Node<'_>, source: &[u8]) -> Option<String> {
    let arguments = call.child_by_field_name("arguments")?;
    let arg = arguments.named_child(0)?;
    match arg.kind() {
        "string" => Some(crate::parser::node_text(arg, source)),
        "template_string" => arg
            .named_child(0)
            .filter(|node| node.kind() == "string_fragment")
            .map(|node| crate::parser::node_text(node, source)),
        _ => None,
    }
}

fn nearest_describe_label(node: Node<'_>, source: &[u8]) -> Option<String> {
    let mut current = node.parent();
    while let Some(parent) = current {
        if parent.kind() == "call_expression"
            && let Some(function) = parent.child_by_field_name("function")
            && call_name(function, source).as_deref() == Some("describe")
        {
            return first_string_argument(parent, source)
                .map(|value| normalize_intent_text(&value));
        }
        current = parent.parent();
    }
    None
}
