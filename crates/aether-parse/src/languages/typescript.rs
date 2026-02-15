use aether_core::{EdgeKind, Language, Symbol, SymbolEdge, SymbolKind, file_source_id};
use tree_sitter::Query;

use crate::parser::{
    build_symbol, typescript_call_target, typescript_qualified_name, typescript_source_function_id,
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

    LanguageConfig {
        id: "typescript",
        extensions: &["ts"],
        ts_language,
        symbol_query,
        edge_query,
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

    LanguageConfig {
        id: "tsx_js",
        extensions: &["tsx", "js", "jsx"],
        ts_language,
        symbol_query,
        edge_query,
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
}
