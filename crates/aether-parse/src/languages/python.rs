use std::path::Path;

use aether_core::{
    EdgeKind, Language, Symbol, SymbolEdge, SymbolKind, file_source_id, normalize_path,
};
use tree_sitter::{Node, Query};

use crate::parser::{TestIntent, humanize_test_name, normalize_intent_text};
use crate::parser::{build_symbol, enclosing_function_symbol_id, named_child_text, node_text};
use crate::registry::{LanguageConfig, LanguageHooks, QueryCaptures};

pub fn config() -> LanguageConfig {
    let ts_language = tree_sitter_python::LANGUAGE.into();
    let symbol_query = Query::new(&ts_language, include_str!("../queries/python_symbols.scm"))
        .expect("invalid python symbol query");
    let edge_query = Query::new(&ts_language, include_str!("../queries/python_edges.scm"))
        .expect("invalid python edge query");
    let test_intent_query = Query::new(
        &ts_language,
        include_str!("../queries/python_test_intents.scm"),
    )
    .expect("invalid python test intent query");

    LanguageConfig {
        id: "python",
        extensions: &["py", "pyi"],
        ts_language,
        symbol_query,
        edge_query,
        test_intent_query: Some(test_intent_query),
        module_markers: &["__init__.py", "pyproject.toml", "setup.py", "setup.cfg"],
        hooks: Some(Box::new(PythonHooks)),
    }
}

struct PythonHooks;

impl LanguageHooks for PythonHooks {
    fn qualify_name(
        &self,
        file_path: &str,
        symbol_name: &str,
        parent: Option<&str>,
    ) -> Option<String> {
        let module = python_module_path(file_path);
        Some(match parent {
            Some(parent) if !parent.trim().is_empty() => {
                format!("{module}::{parent}::{symbol_name}")
            }
            _ => format!("{module}::{symbol_name}"),
        })
    }

    fn map_symbol(
        &self,
        language: Language,
        captures: &QueryCaptures<'_, '_>,
        source: &[u8],
        file_path: &str,
    ) -> Option<Symbol> {
        let capture_name = captures.first_capture_name_with_prefix("symbol.")?;
        let node = captures.node_with_prefix("symbol.")?;

        match capture_name {
            "symbol.function" => {
                if is_directly_decorated(node) {
                    return None;
                }
                self.map_function_symbol(language, file_path, source, node)
            }
            "symbol.class" => {
                if is_directly_decorated(node) {
                    return None;
                }
                self.map_class_symbol(language, file_path, source, node)
            }
            "symbol.decorated" => self.map_decorated_symbol(language, file_path, source, node),
            "symbol.variable" => self.map_variable_symbol(language, file_path, source, node),
            "symbol.type_alias" => self.map_type_alias_symbol(language, file_path, source, node),
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
        let capture_name = captures.first_capture_name_with_prefix("edge.")?;
        let node = captures.node_with_prefix("edge.")?;

        match capture_name {
            "edge.call" => {
                let target = python_call_target(node, source)?;
                let source_id = enclosing_function_symbol_id(symbols, node)
                    .unwrap_or_else(|| file_source_id(file_path));
                Some(vec![SymbolEdge {
                    source_id,
                    target_qualified_name: target,
                    edge_kind: EdgeKind::Calls,
                    file_path: file_path.to_owned(),
                }])
            }
            "edge.depends_on" => {
                let targets = python_import_targets(node, source, file_path);
                if targets.is_empty() {
                    return None;
                }
                Some(
                    targets
                        .into_iter()
                        .map(|target| SymbolEdge {
                            source_id: file_source_id(file_path),
                            target_qualified_name: target,
                            edge_kind: EdgeKind::DependsOn,
                            file_path: file_path.to_owned(),
                        })
                        .collect(),
                )
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
        if node.kind() != "function_definition" {
            return None;
        }

        let test_name = named_child_text(node, "name", source)?;
        if !test_name.starts_with("test_") {
            return None;
        }

        let symbol_id = self
            .map_function_symbol(language, file_path, source, node)
            .map(|symbol| symbol.id);
        let intent_text = python_docstring(node, source)
            .unwrap_or_else(|| humanize_test_name(test_name.as_str()));
        let intent_text = normalize_intent_text(intent_text.as_str());
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

    fn is_module_root(&self, dir_path: &Path) -> Option<bool> {
        Some(dir_path.join("__init__.py").exists())
    }
}

impl PythonHooks {
    fn map_function_symbol(
        &self,
        language: Language,
        file_path: &str,
        source: &[u8],
        node: Node<'_>,
    ) -> Option<Symbol> {
        let name = named_child_text(node, "name", source)?;
        let kind = if is_method_definition(node) {
            SymbolKind::Method
        } else {
            SymbolKind::Function
        };
        let parent = if kind == SymbolKind::Method {
            nearest_ancestor_name(node, source, &["class_definition"])
        } else {
            nearest_ancestor_name(node, source, &["class_definition", "function_definition"])
        };
        let qualified_name = self
            .qualify_name(file_path, &name, parent.as_deref())
            .unwrap_or_else(|| format!("{}::{name}", python_module_path(file_path)));
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

    fn map_class_symbol(
        &self,
        language: Language,
        file_path: &str,
        source: &[u8],
        node: Node<'_>,
    ) -> Option<Symbol> {
        let name = named_child_text(node, "name", source)?;
        let parent =
            nearest_ancestor_name(node, source, &["class_definition", "function_definition"]);
        let qualified_name = self
            .qualify_name(file_path, &name, parent.as_deref())
            .unwrap_or_else(|| format!("{}::{name}", python_module_path(file_path)));
        Some(build_symbol(
            language,
            file_path,
            SymbolKind::Class,
            &name,
            &qualified_name,
            node,
            source,
        ))
    }

    fn map_decorated_symbol(
        &self,
        language: Language,
        file_path: &str,
        source: &[u8],
        node: Node<'_>,
    ) -> Option<Symbol> {
        let inner = node
            .child_by_field_name("definition")
            .or_else(|| find_inner_definition(node))?;
        match inner.kind() {
            "function_definition" => self.map_function_symbol(language, file_path, source, inner),
            "class_definition" => self.map_class_symbol(language, file_path, source, inner),
            _ => None,
        }
    }

    fn map_variable_symbol(
        &self,
        language: Language,
        file_path: &str,
        source: &[u8],
        node: Node<'_>,
    ) -> Option<Symbol> {
        if has_ancestor_kind(node, "function_definition")
            || has_ancestor_kind(node, "class_definition")
        {
            return None;
        }
        let name_node = node
            .child_by_field_name("left")
            .or_else(|| node.child_by_field_name("name"))
            .or_else(|| node.named_child(0))?;
        let name = node_text(name_node, source).trim().to_owned();
        if name.is_empty() {
            return None;
        }
        if node.kind() == "assignment" {
            let has_type = node.child_by_field_name("type").is_some();
            if !has_type && name != "__all__" {
                return None;
            }
        }
        let qualified_name = self
            .qualify_name(file_path, &name, None)
            .unwrap_or_else(|| format!("{}::{name}", python_module_path(file_path)));
        Some(build_symbol(
            language,
            file_path,
            SymbolKind::Variable,
            &name,
            &qualified_name,
            node,
            source,
        ))
    }

    fn map_type_alias_symbol(
        &self,
        language: Language,
        file_path: &str,
        source: &[u8],
        node: Node<'_>,
    ) -> Option<Symbol> {
        let name = named_child_text(node, "name", source).or_else(|| {
            node.named_child(0)
                .map(|child| node_text(child, source).trim().to_owned())
        })?;
        if name.is_empty() {
            return None;
        }
        let qualified_name = self
            .qualify_name(file_path, &name, None)
            .unwrap_or_else(|| format!("{}::{name}", python_module_path(file_path)));
        Some(build_symbol(
            language,
            file_path,
            SymbolKind::TypeAlias,
            &name,
            &qualified_name,
            node,
            source,
        ))
    }
}

fn python_module_path(file_path: &str) -> String {
    let normalized = normalize_path(file_path);
    let path = Path::new(&normalized);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("module");
    let is_init = stem == "__init__";

    let mut segments = path
        .parent()
        .map(|parent| {
            parent
                .iter()
                .filter_map(|part| {
                    let part = part.to_string_lossy().trim().to_owned();
                    (!part.is_empty() && part != ".").then_some(part)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if !is_init && !stem.trim().is_empty() {
        segments.push(stem.to_owned());
    }

    if segments.is_empty() {
        if is_init {
            "module".to_owned()
        } else {
            stem.to_owned()
        }
    } else {
        segments.join(".")
    }
}

fn python_package_path(file_path: &str) -> String {
    let module = python_module_path(file_path);
    let normalized = normalize_path(file_path);
    let stem = Path::new(&normalized)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if stem == "__init__" {
        module
    } else {
        module
            .split('.')
            .filter(|segment| !segment.is_empty())
            .collect::<Vec<_>>()
            .split_last()
            .map(|(_, head)| head.join("."))
            .unwrap_or_default()
    }
}

fn python_call_target(node: Node<'_>, source: &[u8]) -> Option<String> {
    let callee = node
        .child_by_field_name("function")
        .or_else(|| node.named_child(0))?;
    let target = node_text(callee, source).trim().to_owned();
    (!target.is_empty()).then_some(target)
}

fn python_import_targets(node: Node<'_>, source: &[u8], file_path: &str) -> Vec<String> {
    let text = node_text(node, source);
    match node.kind() {
        "import_statement" => parse_import_statement(&text),
        "import_from_statement" => parse_import_from_statement(&text, file_path),
        _ => Vec::new(),
    }
}

fn python_docstring(node: Node<'_>, source: &[u8]) -> Option<String> {
    let body = node.child_by_field_name("body")?;
    let first_statement = body.named_child(0)?;
    if first_statement.kind() != "expression_statement" {
        return None;
    }
    let literal = first_statement.named_child(0)?;
    if literal.kind() != "string" {
        return None;
    }

    let raw = node_text(literal, source);
    let stripped = raw
        .trim()
        .trim_start_matches("r\"\"\"")
        .trim_start_matches("u\"\"\"")
        .trim_start_matches("f\"\"\"")
        .trim_start_matches("\"\"\"")
        .trim_start_matches("'''")
        .trim_end_matches("\"\"\"")
        .trim_end_matches("'''")
        .trim();
    (!stripped.is_empty()).then(|| stripped.to_owned())
}

fn parse_import_statement(text: &str) -> Vec<String> {
    let Some(remainder) = text.trim().strip_prefix("import") else {
        return Vec::new();
    };
    remainder
        .split(',')
        .map(str::trim)
        .filter_map(|entry| {
            entry
                .split_once(" as ")
                .map_or(Some(entry), |(name, _)| Some(name))
        })
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_import_from_statement(text: &str, file_path: &str) -> Vec<String> {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("from") else {
        return Vec::new();
    };
    let rest = rest.trim();
    let Some((module_raw, names_raw)) = rest.split_once(" import ") else {
        return Vec::new();
    };

    let module = resolve_import_module(module_raw.trim(), file_path);
    let names = names_raw.trim();
    if names == "*" {
        return (!module.is_empty()).then_some(module).into_iter().collect();
    }

    names
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            entry
                .split_once(" as ")
                .map_or(entry, |(name, _)| name)
                .trim()
        })
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            if module.is_empty() {
                entry.to_owned()
            } else {
                format!("{module}.{entry}")
            }
        })
        .collect()
}

fn resolve_import_module(raw_module: &str, file_path: &str) -> String {
    let module = raw_module.trim();
    if module.is_empty() {
        return String::new();
    }
    if !module.starts_with('.') {
        return module.to_owned();
    }

    let dot_count = module.chars().take_while(|ch| *ch == '.').count();
    let tail = module[dot_count..].trim_matches('.');
    let package = python_package_path(file_path);

    let mut segments = package
        .split('.')
        .filter(|segment| !segment.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let ascend = dot_count.saturating_sub(1);
    if ascend >= segments.len() {
        segments.clear();
    } else {
        let remaining = segments.len() - ascend;
        segments.truncate(remaining);
    }

    if !tail.is_empty() {
        segments.extend(
            tail.split('.')
                .filter(|segment| !segment.is_empty())
                .map(ToOwned::to_owned),
        );
    }

    segments.join(".")
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

fn is_directly_decorated(node: Node<'_>) -> bool {
    node.parent()
        .is_some_and(|parent| parent.kind() == "decorated_definition")
}

fn is_method_definition(node: Node<'_>) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        match parent.kind() {
            "class_definition" => return true,
            "function_definition" => return false,
            _ => current = parent.parent(),
        }
    }
    false
}

fn nearest_ancestor_name(node: Node<'_>, source: &[u8], kinds: &[&str]) -> Option<String> {
    let mut current = node.parent();
    while let Some(parent) = current {
        if kinds.iter().any(|kind| *kind == parent.kind()) {
            return named_child_text(parent, "name", source);
        }
        current = parent.parent();
    }
    None
}

fn find_inner_definition(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == "function_definition" || child.kind() == "class_definition")
}
