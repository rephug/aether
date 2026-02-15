use aether_core::{EdgeKind, Language, Symbol, SymbolEdge, SymbolKind, file_source_id};
use tree_sitter::Query;

use crate::parser::{
    build_symbol, has_ancestor_kind, rust_call_target, rust_qualified_name,
    rust_source_function_id, rust_use_target,
};
use crate::registry::{LanguageConfig, LanguageHooks, QueryCaptures};

pub fn config() -> LanguageConfig {
    let ts_language = tree_sitter_rust::LANGUAGE.into();
    let symbol_query = Query::new(&ts_language, include_str!("../queries/rust_symbols.scm"))
        .expect("invalid rust symbol query");
    let edge_query = Query::new(&ts_language, include_str!("../queries/rust_edges.scm"))
        .expect("invalid rust edge query");

    LanguageConfig {
        id: "rust",
        extensions: &["rs"],
        ts_language,
        symbol_query,
        edge_query,
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
}
