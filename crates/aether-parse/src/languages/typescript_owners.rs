use crate::parser::{declaration_prefix, named_child_text, node_text};
use aether_core::{
    Language, SymbolKind, normalize_for_fingerprint, signature_fingerprint, stable_symbol_id,
};
use std::collections::HashMap;
use tree_sitter::Node;

pub(crate) fn typescript_qualified_name(
    node: Node<'_>,
    name: &str,
    source: &[u8],
    occurrences: &CallOccurrences,
) -> String {
    qualify(&collect_typescript_context(node, source, occurrences), name)
}

pub(crate) fn typescript_source_function_id(
    language: Language,
    file_path: &str,
    source: &[u8],
    node: Node<'_>,
    occurrences: &CallOccurrences,
) -> Option<String> {
    let function_node = nearest_typescript_function_ancestor(node)?;
    let kind = if function_node.kind() == "method_definition" {
        SymbolKind::Method
    } else {
        SymbolKind::Function
    };
    let name = named_child_text(function_node, "name", source)?;
    let qualified_name = typescript_qualified_name(function_node, &name, source, occurrences);
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

// Ownership text is token-normalized, not whitespace-stripped source: spaces
// inside quoted names and template literals are semantic, not formatting.
fn typescript_owner_text(node: Node<'_>, source: &[u8]) -> String {
    if node.kind() == "comment" {
        return String::new();
    }
    if matches!(node.kind(), "string" | "template_string" | "regex") {
        return node_text(node, source).trim().to_owned();
    }
    if node.child_count() == 0 {
        return normalize_for_fingerprint(&node_text(node, source));
    }
    let mut walk = node.walk();
    node.children(&mut walk)
        .map(|child| typescript_owner_text(child, source))
        .collect::<Vec<_>>()
        .join("")
}

fn collect_typescript_context(
    node: Node<'_>,
    source: &[u8],
    occurrences: &CallOccurrences,
) -> Vec<String> {
    let mut context = Vec::new();
    let mut child = node;
    let mut current = node.parent();

    while let Some(cursor) = current {
        match cursor.kind() {
            "class_declaration"
            | "interface_declaration"
            | "function_declaration"
            | "function_expression"
            | "method_definition"
            | "variable_declarator" => {
                if let Some(name) = named_child_text(cursor, "name", source) {
                    context.push(name);
                }
            }
            "pair" => {
                if let Some(name) = named_child_text(cursor, "key", source) {
                    context.push(name);
                }
            }
            "object"
                if cursor
                    .parent()
                    .is_some_and(|parent| parent.kind() == "return_statement") =>
            {
                context.push("$return".to_owned());
            }
            // A callback, anonymous class, or object argument belongs to its call
            // site. Scalar arguments identify test names and property keys without
            // incorporating any callback or method body into the stable identity.
            "array" => {
                let mut walk = cursor.walk();
                if let Some(ordinal) = cursor
                    .named_children(&mut walk)
                    .filter(|element| element.kind() != "comment")
                    .position(|element| element.id() == child.id())
                {
                    context.push(format!("$element({ordinal})"));
                }
            }
            "arguments" => {
                let call = cursor
                    .parent()
                    .and_then(|call| {
                        call.child_by_field_name("function")
                            .or_else(|| call.child_by_field_name("constructor"))
                    })
                    .map(|callee| typescript_owner_text(callee, source))
                    .unwrap_or_else(|| "$call".to_owned());
                let mut walk = cursor.walk();
                let args = cursor
                    .named_children(&mut walk)
                    .filter(|arg| arg.kind() != "comment")
                    .collect::<Vec<_>>();
                let ordinal = args
                    .iter()
                    .position(|arg| arg.id() == child.id())
                    .unwrap_or(0);
                let labels = args
                    .iter()
                    .filter(|arg| {
                        matches!(
                            arg.kind(),
                            "string"
                                | "number"
                                | "true"
                                | "false"
                                | "null"
                                | "identifier"
                                | "binary_expression"
                                | "template_string"
                        )
                    })
                    .map(|arg| typescript_owner_text(*arg, source))
                    .collect::<Vec<_>>()
                    .join(",");
                let site = cursor
                    .parent()
                    .map(|call| occurrences.get(call))
                    .unwrap_or_default();
                context.push(if labels.is_empty() {
                    format!("$arg({call},{ordinal}{site})")
                } else {
                    format!("$arg({call},{ordinal};{labels}{site})")
                });
            }
            "if_statement" | "ternary_expression" => {
                let condition = cursor
                    .child_by_field_name("condition")
                    .map(|condition| typescript_owner_text(condition, source))
                    .unwrap_or_default();
                let branch = if cursor
                    .child_by_field_name("alternative")
                    .is_some_and(|alternative| alternative.id() == child.id())
                {
                    "else"
                } else {
                    "then"
                };
                context.push(format!("$if({condition},{branch})"));
            }
            _ => {}
        }
        child = cursor;
        current = cursor.parent();
    }

    context.reverse();
    context
}

// Repeated anonymous objects in the same lexical block can have identical call
// headers (e.g. two synthetic events with preventDefault methods). Their source
// order is the remaining declaration discriminator. Count matching call headers,
// not byte offsets, so whitespace, unrelated statements and method-body edits do
// not perturb identity. Inserting another identical call may renumber this group.
fn header(node: Node<'_>, source: &[u8]) -> String {
    let callee = node
        .child_by_field_name("function")
        .or_else(|| node.child_by_field_name("constructor"))
        .map(|callee| typescript_owner_text(callee, source))
        .unwrap_or_default();
    let labels = node
        .child_by_field_name("arguments")
        .map(|args| {
            let mut walk = args.walk();
            args.named_children(&mut walk)
                .filter(|arg| {
                    matches!(
                        arg.kind(),
                        "string"
                            | "number"
                            | "true"
                            | "false"
                            | "null"
                            | "identifier"
                            | "binary_expression"
                            | "template_string"
                    )
                })
                .map(|arg| typescript_owner_text(arg, source))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    format!("{callee};{labels}")
}

#[derive(Default)]
pub(crate) struct CallOccurrences {
    suffixes: HashMap<usize, String>,
    #[cfg(test)]
    scope_scans: usize,
    #[cfg(test)]
    headers_computed: usize,
}

impl CallOccurrences {
    pub(crate) fn new(root: Node<'_>, source: &[u8]) -> Self {
        let mut scopes = HashMap::new();
        let mut pending = vec![root];
        while let Some(node) = pending.pop() {
            if matches!(node.kind(), "call_expression" | "new_expression") {
                let scope = call_scope(node);
                scopes.insert(scope.id(), scope);
            }
            let mut walk = node.walk();
            pending.extend(node.named_children(&mut walk));
        }
        let mut result = Self::default();
        for scope in scopes.into_values() {
            #[cfg(test)]
            {
                result.scope_scans += 1;
            }
            let mut groups: HashMap<String, Vec<Node<'_>>> = HashMap::new();
            let mut pending = vec![scope];
            while let Some(node) = pending.pop() {
                if matches!(node.kind(), "call_expression" | "new_expression") {
                    #[cfg(test)]
                    {
                        result.headers_computed += 1;
                    }
                    groups.entry(header(node, source)).or_default().push(node);
                }
                if node.id() != scope.id()
                    && matches!(
                        node.kind(),
                        "function_declaration"
                            | "function_expression"
                            | "arrow_function"
                            | "method_definition"
                            | "class_declaration"
                            | "class"
                    )
                {
                    continue;
                }
                let mut walk = node.walk();
                pending.extend(node.named_children(&mut walk));
            }
            for mut calls in groups.into_values().filter(|calls| calls.len() > 1) {
                calls.sort_by_key(|node| node.start_byte());
                for (ordinal, call) in calls.into_iter().enumerate() {
                    // The legacy traversal includes nested statement blocks.
                    // Retain its ordinal, but only publish the call's nearest-scope result.
                    if call_scope(call).id() == scope.id() {
                        result.suffixes.insert(call.id(), format!("@{ordinal}"));
                    }
                }
            }
        }
        result
    }

    fn get(&self, call: Node<'_>) -> String {
        self.suffixes.get(&call.id()).cloned().unwrap_or_default()
    }
}

fn call_scope(mut node: Node<'_>) -> Node<'_> {
    while let Some(parent) = node.parent() {
        node = parent;
        if matches!(
            node.kind(),
            "statement_block" | "program" | "arrow_function"
        ) {
            break;
        }
    }
    node
}
fn qualify(context: &[String], name: &str) -> String {
    if context.is_empty() {
        name.to_owned()
    } else {
        format!("{}::{}", context.join("::"), name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::SymbolExtractor;

    #[test]
    fn occurrence_cache_scans_each_scope_once_and_is_reused() {
        let source = (0..256)
            .map(|i| format!("test('case_{i}', {{ run() {{ target(); }} }});\n"))
            .collect::<String>();
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_typescript::LANGUAGE_TSX.into())
            .expect("grammar");
        let tree = parser.parse(&source, None).expect("tree");
        let cache = CallOccurrences::new(tree.root_node(), source.as_bytes());
        assert_eq!(cache.scope_scans, 257);
        assert_eq!(cache.headers_computed, 512);
        let mut pending = vec![tree.root_node()];
        while let Some(node) = pending.pop() {
            if node.kind() == "method_definition" {
                for _ in 0..4 {
                    assert!(
                        typescript_qualified_name(node, "run", source.as_bytes(), &cache)
                            .starts_with("$arg(test,1;'case_")
                    );
                }
            }
            let mut walk = node.walk();
            pending.extend(node.named_children(&mut walk));
        }
        assert_eq!(
            cache.headers_computed, 512,
            "qualification must not rescan scopes"
        );
    }

    #[test]
    fn nested_block_ordinals_match_their_own_scope_and_cache_does_not_cross_files() {
        let mut extractor = SymbolExtractor::new().expect("extractor");
        let first = extractor
            .extract_with_edges_from_source(
                aether_core::Language::JavaScript,
                "scope.mjs",
                "send({run(){ outer(); }}); if(flag){send({run(){ inner(); }});}",
            )
            .expect("extract");
        let names = first
            .symbols
            .iter()
            .map(|s| s.qualified_name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"$arg(send,0@0)::run"));
        assert!(names.contains(&"$if((flag),then)::$arg(send,0)::run"));
        for edge in first
            .edges
            .iter()
            .filter(|e| ["outer", "inner"].contains(&e.target_qualified_name.as_str()))
        {
            assert!(first.symbols.iter().any(|s| s.id == edge.source_id));
        }
        let second = extractor
            .extract_from_source(
                aether_core::Language::JavaScript,
                "scope.mjs",
                "send({run(){ outer(); }});",
            )
            .expect("second extraction");
        assert_eq!(second[0].qualified_name, "$arg(send,0)::run");
    }
}
