//! A deliberately bounded import resolver: explicit named relative imports whose
//! target file directly exports a function or class declaration. No package,
//! tsconfig alias, CommonJS, export-list, re-export or dynamic import resolution.
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};

use aether_core::normalize_path;
use tree_sitter::{Node, Parser};

use crate::languages::typescript_owners::contains_potential_direct_eval;
use crate::parser::{named_child_text, node_text};

const EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];
const MAX_MODULE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug)]
pub(super) struct ImportTarget {
    pub(super) file_path: String,
    pub(super) qualified_name: String,
}

#[derive(Default)]
pub(super) struct Imports {
    targets: HashMap<String, Option<ImportTarget>>,
    dependencies: BTreeSet<String>,
}

impl Imports {
    pub(super) fn new(
        root: Node<'_>,
        source: &[u8],
        file_path: &str,
        workspace_root: Option<&Path>,
    ) -> Self {
        let mut imports = Self::default();
        let mut exports = HashMap::new();
        let mut cursor = root.walk();
        for statement in root.named_children(&mut cursor) {
            if statement.kind() != "import_statement" {
                continue;
            }
            if has_keyword(statement, "type") || has_keyword(statement, "typeof") {
                continue;
            }
            let Some(module) = statement.child_by_field_name("source") else {
                continue;
            };
            let module = node_text(module, source);
            let Some(module) = string_literal_contents(&module) else {
                continue;
            };
            let candidates = import_candidates(file_path, module);
            imports.dependencies.extend(candidates.iter().cloned());
            let target_file = workspace_root.and_then(|workspace| {
                unique_module(workspace, &candidates).map(|path| (workspace, path))
            });
            let mut pending = vec![statement];
            while let Some(node) = pending.pop() {
                if node.kind() == "import_specifier" {
                    let Some(name) = named_child_text(node, "name", source) else {
                        continue;
                    };
                    let alias =
                        named_child_text(node, "alias", source).unwrap_or_else(|| name.clone());
                    let target = if has_keyword(node, "type") || has_keyword(node, "typeof") {
                        None
                    } else {
                        target_file.as_ref().and_then(|(workspace, path)| {
                            exports
                                .entry(path.clone())
                                .or_insert_with(|| direct_exports(workspace, path))
                                .contains(&name)
                                .then(|| ImportTarget {
                                    file_path: path.clone(),
                                    qualified_name: name.clone(),
                                })
                        })
                    };
                    imports
                        .targets
                        .entry(alias)
                        .and_modify(|old| *old = None)
                        .or_insert(target);
                    continue;
                }
                let mut walk = node.walk();
                pending.extend(node.named_children(&mut walk));
            }
        }
        imports
    }

    pub(super) fn target(&self, name: &str) -> Option<&ImportTarget> {
        self.targets.get(name).and_then(Option::as_ref)
    }

    pub(super) fn dependencies(&self) -> Vec<String> {
        self.dependencies.iter().cloned().collect()
    }
}

/// Remove exactly the string token's matching outer delimiters, preserving
/// quote characters and whitespace which belong to the module specifier itself.
pub(super) fn string_literal_contents(text: &str) -> Option<&str> {
    let quote = text.chars().next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    text.strip_prefix(quote)?.strip_suffix(quote)
}

/// Modifiers are anonymous grammar tokens. Comments and identifiers spelled
/// type must not be mistaken for the keyword, regardless of token spacing.
fn has_keyword(node: Node<'_>, keyword: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| !child.is_named() && child.kind() == keyword)
}

fn import_candidates(file_path: &str, module: &str) -> Vec<String> {
    if !(module.starts_with("./") || module.starts_with("../")) || module.contains(['\\', '?', '#'])
    {
        return Vec::new();
    }
    let parent = Path::new(file_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let mut normalized = PathBuf::new();
    for component in parent.join(module).components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Vec::new();
                }
            }
            Component::Prefix(_) | Component::RootDir => return Vec::new(),
        }
    }
    let Some(path) = normalized.to_str() else {
        return Vec::new();
    };
    let path = normalize_path(path);
    let directory_only = module.ends_with('/')
        || module
            .rsplit('/')
            .next()
            .is_some_and(|part| matches!(part, "." | ".."));
    if directory_only {
        let index = if path.is_empty() {
            "index".to_owned()
        } else {
            format!("{path}/index")
        };
        return EXTENSIONS
            .iter()
            .map(|extension| normalize_path(&format!("{index}.{extension}")))
            .collect();
    }
    match normalized
        .extension()
        .and_then(|extension| extension.to_str())
    {
        Some(extension) if EXTENSIONS.contains(&extension) => vec![path],
        Some(_) => Vec::new(),
        None => EXTENSIONS
            .iter()
            .flat_map(|extension| {
                [
                    normalize_path(&format!("{path}.{extension}")),
                    normalize_path(&format!("{path}/index.{extension}")),
                ]
            })
            .collect(),
    }
}

fn unique_module(workspace: &Path, candidates: &[String]) -> Option<String> {
    let workspace = workspace.canonicalize().ok()?;
    let mut found = None;
    for path in candidates {
        let candidate = workspace.join(path);
        if !candidate.is_file() {
            continue;
        }
        // Do not prove an edge through a symlink out of the indexed workspace.
        if !candidate.canonicalize().ok()?.starts_with(&workspace) {
            return None;
        }
        if found.is_some() {
            return None;
        }
        found = Some(path.clone());
    }
    found
}

fn direct_exports(workspace: &Path, path: &str) -> BTreeSet<String> {
    let full_path = workspace.join(path);
    let Ok(metadata) = fs::metadata(&full_path) else {
        return BTreeSet::new();
    };
    if metadata.len() > MAX_MODULE_BYTES {
        return BTreeSet::new();
    }
    let Ok(source) = fs::read_to_string(&full_path) else {
        return BTreeSet::new();
    };
    let mut parser = Parser::new();
    let tsx = Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext, "tsx" | "jsx" | "js" | "mjs" | "cjs"));
    let language = if tsx {
        tree_sitter_typescript::LANGUAGE_TSX.into()
    } else {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    };
    if parser.set_language(&language).is_err() {
        return BTreeSet::new();
    }
    let Some(tree) = parser.parse(&source, None) else {
        return BTreeSet::new();
    };
    let root = tree.root_node();
    if root.has_error() || contains_potential_direct_eval(root, source.as_bytes()) {
        return BTreeSet::new();
    }
    let mut exported = BTreeSet::new();
    let mut cursor = root.walk();
    for statement in root.named_children(&mut cursor) {
        if statement.kind() != "export_statement"
            || statement.child_by_field_name("source").is_some()
        {
            continue;
        }
        let Some(declaration) = statement.child_by_field_name("declaration") else {
            continue;
        };
        if has_keyword(statement, "default") {
            continue;
        }
        if matches!(
            declaration.kind(),
            "function_declaration" | "class_declaration"
        ) && declaration.child_by_field_name("body").is_some()
            && let Some(name) = named_child_text(declaration, "name", source.as_bytes())
        {
            exported.insert(name);
        }
    }
    // Writes can replace a directly exported function or class. Be conservative
    // even when the write might refer to a nested binding of the same spelling.
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        if matches!(
            node.kind(),
            "assignment_expression"
                | "augmented_assignment_expression"
                | "update_expression"
                | "for_in_statement"
                | "variable_declarator"
        ) && let Some(left) = node
            .child_by_field_name("left")
            .or_else(|| node.child_by_field_name("argument"))
            .or_else(|| node.child_by_field_name("name"))
        {
            remove_written_exports(left, source.as_bytes(), &mut exported);
        }
        let mut cursor = node.walk();
        pending.extend(node.named_children(&mut cursor));
    }
    exported
}

fn remove_written_exports(pattern: Node<'_>, source: &[u8], exported: &mut BTreeSet<String>) {
    match pattern.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => {
            exported.remove(&node_text(pattern, source));
        }
        "pair_pattern" => {
            if let Some(value) = pattern.child_by_field_name("value") {
                remove_written_exports(value, source, exported);
            }
        }
        "assignment_pattern" | "object_assignment_pattern" => {
            if let Some(left) = pattern.child_by_field_name("left") {
                remove_written_exports(left, source, exported);
            }
        }
        "array_pattern" | "object_pattern" | "rest_pattern" => {
            let mut cursor = pattern.walk();
            for child in pattern.named_children(&mut cursor) {
                remove_written_exports(child, source, exported);
            }
        }
        _ => {}
    }
}
