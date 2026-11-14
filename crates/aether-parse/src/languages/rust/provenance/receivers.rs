//! Explicit nominal receiver evidence only; no return-type or trait inference.
use std::collections::HashMap;

use aether_core::Symbol;
use tree_sitter::Node;

use super::{
    attributed, children, collect_use_path_entries, contains_identifier, extern_binding,
    lexical_scopes, local_item, name, node_text, scope_macros, shadowed, simple_path,
    symbol_for_node,
};

pub(super) struct Receiver<'tree> {
    pub(super) at: Node<'tree>,
    pub(super) path: Vec<String>,
    pub(super) require_unit: bool,
    pub(super) reference: bool,
}

pub(super) fn local_receiver_target<'a>(
    call: Node<'_>,
    source: &[u8],
    symbols: &'a [Symbol],
) -> Option<&'a Symbol> {
    let receiver = receiver_nominal(call, source)?;
    if receiver.require_unit && shadowed(receiver.at, &receiver.path[0], source, false) {
        return None;
    }
    let nominal = local_item(receiver.at, &receiver.path, source, true, 0)?;
    if !nominal_struct(nominal, receiver.require_unit) {
        return None;
    }
    let method_name = call
        .child_by_field_name("function")?
        .child_by_field_name("field")?;
    let method_name = node_text(method_name, source);
    if !dispatch_scope_safe(call, &method_name, &receiver.path, source) {
        return None;
    }
    let method = inherent_method(nominal, &method_name, source, true)?;
    if receiver.reference && !self_reference(method, source) {
        return None;
    }
    symbol_for_node(method, symbols)
}

pub(super) fn nominal_struct(item: Node<'_>, require_unit: bool) -> bool {
    item.kind() == "struct_item"
        && item.child_by_field_name("type_parameters").is_none()
        && !attributed(item)
        && item.parent().is_some_and(|scope| !scope_macros(scope))
        && (!require_unit || item.child_by_field_name("body").is_none())
}

/// Select only one non-generic, immutable-self inherent method in the nominal
/// declaration's own file/scope. Trait implementations never supply a target.
pub(super) fn inherent_method<'tree>(
    nominal: Node<'tree>,
    wanted: &str,
    source: &[u8],
    receiver_call: bool,
) -> Option<Node<'tree>> {
    if !nominal_struct(nominal, false) {
        return None;
    }
    let type_name = name(nominal, source)?;
    let mut methods = Vec::new();
    for implementation in children(nominal.parent()?)
        .into_iter()
        .filter(|item| item.kind() == "impl_item")
    {
        if implementation
            .child_by_field_name("type")
            .map(|ty| node_text(ty, source))
            .as_deref()
            != Some(type_name.as_str())
        {
            continue;
        }
        let Some(body) = implementation.child_by_field_name("body") else {
            continue;
        };
        if scope_macros(body) {
            return None;
        }
        for method in children(body).into_iter().filter(|item| {
            item.kind() == "function_item" && name(*item, source).as_deref() == Some(wanted)
        }) {
            if implementation.child_by_field_name("trait").is_some()
                || implementation
                    .child_by_field_name("type_parameters")
                    .is_some()
                || method.child_by_field_name("type_parameters").is_some()
                || attributed(method)
            {
                return None;
            }
            let parameters = method.child_by_field_name("parameters")?;
            let self_parameters = children(parameters)
                .into_iter()
                .filter(|parameter| parameter.kind() == "self_parameter")
                .collect::<Vec<_>>();
            if receiver_call && (self_parameters.len() != 1 || mutable(self_parameters[0])) {
                return None;
            }
            methods.push(method);
        }
    }
    (methods.len() == 1).then(|| methods[0])
}

// By-value methods of traits automatically in the Rust 1.94 standard
// preludes. On an owned T these can beat an inherent method taking &self.
// Keeping these dot references unresolved avoids implicit trait dispatch.
const PRELUDE_VALUE_METHODS: &[&str] = &[
    "array_chunks",
    "async_call_once",
    "call_once",
    "chain",
    "clamp",
    "cloned",
    "cmp",
    "cmp_by",
    "collect",
    "collect_into",
    "copied",
    "count",
    "cycle",
    "enumerate",
    "eq",
    "eq_by",
    "filter",
    "filter_map",
    "flat_map",
    "flatten",
    "fold",
    "for_each",
    "fuse",
    "ge",
    "gt",
    "inspect",
    "intersperse",
    "intersperse_with",
    "into",
    "into_future",
    "into_iter",
    "is_partitioned",
    "is_sorted",
    "is_sorted_by",
    "is_sorted_by_key",
    "last",
    "le",
    "lt",
    "map",
    "map_while",
    "map_windows",
    "max",
    "max_by",
    "max_by_key",
    "min",
    "min_by",
    "min_by_key",
    "ne",
    "partial_cmp",
    "partial_cmp_by",
    "partition",
    "partition_in_place",
    "peekable",
    "product",
    "reduce",
    "rev",
    "rfold",
    "scan",
    "skip",
    "skip_while",
    "step_by",
    "sum",
    "take",
    "take_while",
    "try_into",
    "unzip",
    "zip",
];

pub(super) fn dispatch_scope_safe(
    call: Node<'_>,
    method: &str,
    nominal_path: &[String],
    source: &[u8],
) -> bool {
    if PRELUDE_VALUE_METHODS.contains(&method) {
        return false;
    }
    for scope in lexical_scopes(call) {
        if scope_macros(scope) {
            return false;
        }
        for item in children(scope) {
            if item.kind() == "extern_crate_declaration" {
                return false;
            }
            if item.kind() == "trait_item"
                && item.child_by_field_name("body").is_some_and(|body| {
                    scope_macros(body)
                        || children(body)
                            .into_iter()
                            .any(|member| name(member, source).as_deref() == Some(method))
                })
            {
                return false;
            }
            if item.kind() != "use_declaration" {
                continue;
            }
            if node_text(item, source).contains('*') || attributed(item) {
                return false;
            }
            let mut imports = HashMap::new();
            if let Some(argument) = item.child_by_field_name("argument") {
                collect_use_path_entries(argument, &[], source, &mut imports);
            }
            if imports.is_empty() {
                return false;
            }
            for (visible, paths) in imports {
                if paths.len() != 1 {
                    return false;
                }
                // The receiver's own type import is verified by local_item or
                // the workspace resolver. Other imports must prove a local
                // concrete type; an unknown import could introduce a trait.
                if nominal_path.first().is_some_and(|first| first == &visible) {
                    continue;
                }
                if !local_item(item, &[visible], source, true, 0)
                    .is_some_and(|nominal| nominal_struct(nominal, false))
                {
                    return false;
                }
            }
        }
    }
    true
}

fn self_reference(method: Node<'_>, source: &[u8]) -> bool {
    method
        .child_by_field_name("parameters")
        .is_some_and(|parameters| {
            children(parameters).into_iter().any(|parameter| {
                parameter.kind() == "self_parameter" && node_text(parameter, source).contains('&')
            })
        })
}

fn mutable(node: Node<'_>) -> bool {
    children(node)
        .into_iter()
        .any(|child| child.kind() == "mutable_specifier")
}

fn nominal_path(node: Node<'_>, source: &[u8]) -> Option<Vec<String>> {
    match node.kind() {
        "identifier" | "type_identifier" | "scoped_identifier" | "scoped_type_identifier" => {
            simple_path(&node_text(node, source))
        }
        _ => None,
    }
}

fn from_binding<'tree>(
    binding: Node<'tree>,
    wanted: &str,
    source: &[u8],
) -> Option<Receiver<'tree>> {
    let pattern = binding.child_by_field_name("pattern")?;
    if pattern.kind() != "identifier"
        || node_text(pattern, source) != wanted
        || mutable(binding)
        || attributed(binding)
    {
        return None;
    }
    if let Some(ty) = binding.child_by_field_name("type") {
        return Some(Receiver {
            at: binding,
            path: nominal_path(ty, source)?,
            require_unit: false,
            reference: false,
        });
    }
    if binding.kind() != "let_declaration" {
        return None;
    }
    let value = binding.child_by_field_name("value")?;
    Some(Receiver {
        at: binding,
        path: nominal_path(value, source)?,
        require_unit: true,
        reference: false,
    })
}

fn assigned(owner: Node<'_>, wanted: &str, source: &[u8]) -> bool {
    let mut stack = vec![owner];
    while let Some(node) = stack.pop() {
        if matches!(
            node.kind(),
            "assignment_expression" | "compound_assignment_expr"
        ) && node
            .child_by_field_name("left")
            .is_some_and(|left| contains_identifier(left, wanted, source))
        {
            return true;
        }
        stack.extend(children(node));
    }
    false
}

pub(super) fn receiver_nominal<'tree>(call: Node<'tree>, source: &[u8]) -> Option<Receiver<'tree>> {
    let callee = call.child_by_field_name("function")?;
    if callee.kind() != "field_expression" {
        return None;
    }
    let value = callee.child_by_field_name("value")?;
    if !matches!(value.kind(), "identifier" | "self") {
        return None;
    }
    let wanted = node_text(value, source);
    let mut owner = call.parent();
    while let Some(node) = owner {
        if node.kind() == "function_item" {
            if assigned(node, &wanted, source) {
                return None;
            }
            break;
        }
        owner = node.parent();
    }
    let mut current = call.parent();
    while let Some(node) = current {
        if matches!(
            node.kind(),
            "for_expression" | "match_arm" | "let_condition"
        ) && node
            .child_by_field_name("pattern")
            .is_some_and(|pattern| contains_identifier(pattern, &wanted, source))
        {
            return None;
        }
        if matches!(node.kind(), "if_expression" | "while_expression")
            && children(node).into_iter().any(|condition| {
                condition.kind() == "let_condition"
                    && condition
                        .child_by_field_name("pattern")
                        .is_some_and(|pattern| contains_identifier(pattern, &wanted, source))
            })
        {
            return None;
        }
        if node.kind() == "block" {
            if extern_binding(node, &wanted, source) {
                return None;
            }
            if children(node).into_iter().any(|item| {
                matches!(item.kind(), "const_item" | "static_item" | "function_item")
                    && name(item, source).as_deref() == Some(wanted.as_str())
            }) {
                return None;
            }
            let bindings = children(node)
                .into_iter()
                .filter(|item| {
                    item.kind() == "let_declaration"
                        && item
                            .child_by_field_name("pattern")
                            .is_some_and(|pattern| contains_identifier(pattern, &wanted, source))
                })
                .collect::<Vec<_>>();
            if !bindings.is_empty() {
                if bindings.len() != 1 || bindings[0].end_byte() > call.start_byte() {
                    return None;
                }
                return from_binding(bindings[0], &wanted, source);
            }
        }
        if matches!(node.kind(), "function_item" | "closure_expression") {
            let parameters = node.child_by_field_name("parameters")?;
            if wanted == "self" {
                let self_parameters = children(parameters)
                    .into_iter()
                    .filter(|parameter| parameter.kind() == "self_parameter")
                    .collect::<Vec<_>>();
                if self_parameters.len() != 1 || mutable(self_parameters[0]) {
                    return None;
                }
                let mut implementation = node.parent();
                while let Some(item) = implementation {
                    if item.kind() == "impl_item" {
                        if item.child_by_field_name("trait").is_some()
                            || item.child_by_field_name("type_parameters").is_some()
                            || attributed(item)
                        {
                            return None;
                        }
                        return Some(Receiver {
                            at: call,
                            path: nominal_path(item.child_by_field_name("type")?, source)?,
                            require_unit: false,
                            reference: node_text(self_parameters[0], source).contains('&'),
                        });
                    }
                    implementation = item.parent();
                }
                return None;
            }
            let bindings = children(parameters)
                .into_iter()
                .filter(|parameter| {
                    if let Some(pattern) = parameter.child_by_field_name("pattern") {
                        contains_identifier(pattern, &wanted, source)
                    } else {
                        node.kind() == "closure_expression"
                            && contains_identifier(*parameter, &wanted, source)
                    }
                })
                .collect::<Vec<_>>();
            if !bindings.is_empty() {
                return (bindings.len() == 1)
                    .then(|| from_binding(bindings[0], &wanted, source))
                    .flatten();
            }
            if node.kind() == "function_item" {
                return None;
            }
        }
        current = node.parent();
    }
    None
}
