use std::collections::{HashMap, HashSet};

use aether_core::{
    EdgeKind, Language, Symbol, SymbolEdge, SymbolKind, content_hash, file_source_id,
    signature_fingerprint, stable_symbol_id,
};
use tree_sitter::{Node, Query};

use crate::parser::{
    TestIntent, enclosing_function_symbol_id, named_child_text, node_range, node_text,
};
use crate::registry::{LanguageConfig, LanguageHooks, QueryCaptures};

pub fn config() -> LanguageConfig {
    let ts_language = tree_sitter_java::LANGUAGE.into();
    let symbol_query = Query::new(&ts_language, include_str!("../queries/java_symbols.scm"))
        .expect("invalid java symbol query");
    let edge_query = Query::new(&ts_language, include_str!("../queries/java_edges.scm"))
        .expect("invalid java edge query");
    let test_intent_query = Query::new(
        &ts_language,
        include_str!("../queries/java_test_intents.scm"),
    )
    .expect("invalid java test intent query");

    LanguageConfig {
        id: "java",
        extensions: &["java"],
        ts_language,
        symbol_query,
        edge_query,
        test_intent_query: Some(test_intent_query),
        module_markers: &["pom.xml", "build.gradle", "build.gradle.kts"],
        hooks: Some(Box::new(JavaHooks)),
    }
}

struct JavaHooks;

/// Declarations that introduce a named type.
const TYPE_DECLARATIONS: &[&str] = &[
    "class_declaration",
    "interface_declaration",
    "enum_declaration",
    "record_declaration",
    "annotation_type_declaration",
];

/// Nodes whose bodies hold local or anonymous code. Declarations nested inside them are not
/// addressable from outside, so they are not indexed.
const LOCAL_SCOPES: &[&str] = &[
    "block",
    "constructor_body",
    "lambda_expression",
    "object_creation_expression",
    "enum_constant",
];

/// `java.lang` types, which every file sees without an import. References to them never
/// resolve to workspace symbols.
const JAVA_LANG_TYPES: &[&str] = &[
    "Appendable",
    "ArithmeticException",
    "ArrayIndexOutOfBoundsException",
    "ArrayStoreException",
    "AssertionError",
    "AutoCloseable",
    "Boolean",
    "Byte",
    "CharSequence",
    "Character",
    "Class",
    "ClassCastException",
    "ClassLoader",
    "ClassNotFoundException",
    "CloneNotSupportedException",
    "Cloneable",
    "Comparable",
    "Deprecated",
    "Double",
    "Enum",
    "Error",
    "Exception",
    "ExceptionInInitializerError",
    "Float",
    "FunctionalInterface",
    "IllegalArgumentException",
    "IllegalStateException",
    "IllegalAccessException",
    "IllegalMonitorStateException",
    "IndexOutOfBoundsException",
    "InheritableThreadLocal",
    "InstantiationException",
    "Integer",
    "InterruptedException",
    "Iterable",
    "LinkageError",
    "Long",
    "Math",
    "Module",
    "NegativeArraySizeException",
    "NoSuchFieldException",
    "NoSuchMethodException",
    "NullPointerException",
    "Number",
    "NumberFormatException",
    "Object",
    "OutOfMemoryError",
    "Override",
    "Package",
    "Process",
    "ProcessBuilder",
    "Readable",
    "Record",
    "ReflectiveOperationException",
    "Runnable",
    "Runtime",
    "RuntimeException",
    "SafeVarargs",
    "SecurityException",
    "Short",
    "StackOverflowError",
    "StackTraceElement",
    "String",
    "StringBuffer",
    "StringBuilder",
    "StringIndexOutOfBoundsException",
    "SuppressWarnings",
    "System",
    "Thread",
    "ThreadLocal",
    "Throwable",
    "UnsupportedOperationException",
    "VirtualMachineError",
    "Void",
];

/// JUnit 4 and 5 (and TestNG) annotations that mark a test method.
const TEST_ANNOTATIONS: &[&str] = &[
    "Test",
    "ParameterizedTest",
    "RepeatedTest",
    "TestFactory",
    "TestTemplate",
];

impl LanguageHooks for JavaHooks {
    fn map_symbol(
        &self,
        language: Language,
        captures: &QueryCaptures<'_, '_>,
        source: &[u8],
        file_path: &str,
    ) -> Option<Symbol> {
        let capture_name = captures.first_capture_name_with_prefix("symbol.")?;
        let node = captures.node_with_prefix("symbol.")?;
        let name = named_child_text(node, "name", source)?;
        let declaring_types = declaring_types(node, source)?;

        let kind = match capture_name {
            "symbol.java_class" => SymbolKind::Class,
            "symbol.java_interface" => SymbolKind::Interface,
            "symbol.java_enum" => SymbolKind::Enum,
            "symbol.java_method" => {
                // Abstract and interface methods without a body are part of their type's text,
                // as Rust trait signatures and TypeScript interface members are.
                if declaring_types.is_empty() || node.child_by_field_name("body").is_none() {
                    return None;
                }
                SymbolKind::Method
            }
            _ => return None,
        };

        let qualified_name = qualify(&declaring_types, &name);
        let signature = signature_fingerprint(&signature_text(node, source));
        Some(Symbol {
            id: stable_symbol_id(language, file_path, kind, &qualified_name, &signature),
            language,
            file_path: file_path.to_owned(),
            kind,
            name,
            qualified_name,
            signature_fingerprint: signature,
            content_hash: content_hash(&node_text(node, source)),
            range: node_range(node),
        })
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

        // Always answer with a list, even an empty one, so the generic mapper never turns a
        // skipped node into an edge built from its raw text.
        let edges = match capture_name {
            "edge.call" => call_edge(node, source, file_path, symbols)
                .into_iter()
                .collect(),
            "edge.depends_on" => import_target(node, source)
                .map(|import| SymbolEdge {
                    source_id: file_source_id(file_path),
                    target_qualified_name: import.path,
                    edge_kind: EdgeKind::DependsOn,
                    file_path: file_path.to_owned(),
                })
                .into_iter()
                .collect(),
            "edge.type_ref" => type_ref_edges(node, source, file_path, symbols),
            "edge.implements" => implements_edges(node, source, file_path, symbols),
            _ => Vec::new(),
        };
        Some(edges)
    }

    fn map_test_intent(
        &self,
        language: Language,
        captures: &QueryCaptures<'_, '_>,
        source: &[u8],
        file_path: &str,
        symbols: &[Symbol],
    ) -> Option<Vec<TestIntent>> {
        let node = captures.node_with_prefix("test.")?;
        if !has_annotation(node, source, TEST_ANNOTATIONS) {
            return None;
        }
        let method_name = named_child_text(node, "name", source)?;
        // `@Nested` test classes often reuse method names, so a nested test is named by its
        // path below the file's outermost class.
        let test_name = match declaring_types(node, source) {
            Some(types) if types.len() > 1 => format!("{}::{method_name}", types[1..].join("::")),
            _ => method_name.clone(),
        };

        let intent_text = annotation_string_argument(node, source, "DisplayName")
            .or_else(|| javadoc(node, source))
            .unwrap_or_else(|| humanize_test_name(&method_name));
        let intent_text = collapse_whitespace(&intent_text);
        if intent_text.is_empty() {
            return None;
        }

        let group_label = enclosing_type(node).and_then(|class| {
            annotation_string_argument(class, source, "DisplayName")
                .or_else(|| named_child_text(class, "name", source))
                .map(|label| collapse_whitespace(&label))
        });

        Some(vec![TestIntent {
            file_path: file_path.to_owned(),
            test_name,
            intent_text,
            group_label,
            language,
            symbol_id: symbol_for_node(symbols, node).map(|symbol| symbol.id.clone()),
        }])
    }
}

/// A declaration's text up to its body, without annotations and comments. Adding
/// `@Override`, `@Disabled` or `@Deprecated` therefore keeps the symbol id, as adding an
/// attribute to a Rust item does.
fn signature_text(node: Node<'_>, source: &[u8]) -> String {
    let end = node
        .child_by_field_name("body")
        .map_or(node.end_byte(), |body| body.start_byte());
    let mut skipped = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.start_byte() >= end {
            break;
        }
        match child.kind() {
            "modifiers" => {
                let mut inner = child.walk();
                skipped.extend(
                    child
                        .children(&mut inner)
                        .filter(|modifier| {
                            matches!(
                                modifier.kind(),
                                "annotation"
                                    | "marker_annotation"
                                    | "line_comment"
                                    | "block_comment"
                            )
                        })
                        .map(|modifier| modifier.byte_range()),
                );
            }
            "line_comment" | "block_comment" => skipped.push(child.byte_range()),
            _ => {}
        }
    }

    let mut text = String::new();
    let mut position = node.start_byte();
    for range in skipped {
        if let Some(part) = source.get(position..range.start) {
            text.push_str(&String::from_utf8_lossy(part));
        }
        position = range.end;
    }
    if let Some(part) = source.get(position..end.max(position)) {
        text.push_str(&String::from_utf8_lossy(part));
    }
    text
}

/// Names of the types that declare `node`, outermost first. `None` when `node` sits in a
/// method body, lambda or anonymous class, where it can't be named from outside.
fn declaring_types(node: Node<'_>, source: &[u8]) -> Option<Vec<String>> {
    let mut names = Vec::new();
    let mut current = node.parent();
    while let Some(parent) = current {
        if LOCAL_SCOPES.contains(&parent.kind()) {
            return None;
        }
        if TYPE_DECLARATIONS.contains(&parent.kind()) {
            names.push(named_child_text(parent, "name", source)?);
        }
        current = parent.parent();
    }
    names.reverse();
    Some(names)
}

fn qualify(declaring_types: &[String], name: &str) -> String {
    if declaring_types.is_empty() {
        name.to_owned()
    } else {
        format!("{}::{name}", declaring_types.join("::"))
    }
}

/// The nearest type declaration around `node`, unless an anonymous class body (of a `new`
/// expression or an enum constant) comes first. Arguments of a `new` expression still belong
/// to the surrounding class.
fn enclosing_type(node: Node<'_>) -> Option<Node<'_>> {
    let mut current = node.parent();
    while let Some(parent) = current {
        if TYPE_DECLARATIONS.contains(&parent.kind()) {
            return Some(parent);
        }
        let is_anonymous_body = parent.kind() == "class_body"
            && parent.parent().is_some_and(|owner| {
                matches!(owner.kind(), "object_creation_expression" | "enum_constant")
            });
        if is_anonymous_body {
            return None;
        }
        current = parent.parent();
    }
    None
}

/// The qualified name of the type whose members `this` refers to at `node`.
fn enclosing_type_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    let class = enclosing_type(node)?;
    let name = named_child_text(class, "name", source)?;
    Some(qualify(&declaring_types(class, source)?, &name))
}

fn call_edge(
    call: Node<'_>,
    source: &[u8],
    file_path: &str,
    symbols: &[Symbol],
) -> Option<SymbolEdge> {
    let source_id = enclosing_function_symbol_id(symbols, call)?;
    let context = FileContext::of(call, source);
    let target = match call.kind() {
        "method_invocation" => method_call_target(call, source, &context)?,
        "object_creation_expression" => {
            let created = type_name(call.child_by_field_name("type")?, source)?;
            context.spell(&created, call, source)
        }
        _ => return None,
    };
    Some(SymbolEdge {
        source_id,
        target_qualified_name: target,
        edge_kind: EdgeKind::Calls,
        file_path: file_path.to_owned(),
    })
}

/// Spells a method call the way Java methods are qualified (`Type::method`) whenever the
/// receiver's type can be read from the source: the enclosing class for unqualified and
/// `this.` calls, the superclass for `super.` calls, a static import, a declared variable,
/// parameter or field, or a capitalized type name. Types imported from other projects keep
/// their fully qualified name. Anything else keeps the bare method name.
fn method_call_target(call: Node<'_>, source: &[u8], context: &FileContext) -> Option<String> {
    let name = node_text(call.child_by_field_name("name")?, source)
        .trim()
        .to_owned();
    if name.is_empty() {
        return None;
    }
    let owner = match call.child_by_field_name("object") {
        None => context
            .static_owner(&name)
            .or_else(|| context.method_owner(call, &name, source)),
        Some(object) => match object.kind() {
            "this" => enclosing_type_name(call, source),
            "super" => enclosing_type(call)
                .and_then(|class| class.child_by_field_name("superclass"))
                .and_then(|superclass| first_type_name(superclass, source))
                .map(|superclass| context.spell(&superclass, call, source)),
            "identifier" => {
                let receiver = node_text(object, source).trim().to_owned();
                match declared_type(call, &receiver, source) {
                    Some(declared) => {
                        declared.map(|declared| context.spell(&declared, call, source))
                    }
                    None if receiver.starts_with(char::is_uppercase) => {
                        Some(context.spell(&receiver, call, source))
                    }
                    None => None,
                }
            }
            "field_access" => this_field_type(object, call, source)
                .map(|declared| context.spell(&declared, call, source)),
            _ => None,
        },
    };
    Some(match owner {
        Some(owner) => format!("{owner}::{name}"),
        None => name,
    })
}

/// `this.repo` resolves through the enclosing class's field declarations.
fn this_field_type(field_access: Node<'_>, call: Node<'_>, source: &[u8]) -> Option<String> {
    let object = field_access.child_by_field_name("object")?;
    if object.kind() != "this" {
        return None;
    }
    let field = node_text(field_access.child_by_field_name("field")?, source);
    let class = enclosing_type(call)?;
    field_type(class, field.trim(), source)
}

/// Finds the declared type of `name` as seen from `node`: parameters, local variables,
/// loop and catch variables and resources in enclosing scopes, then fields of the enclosing
/// types. The outer `Option` says whether `name` was found; the inner one is `None` when it
/// was declared without a usable type (`var` without `new`, a primitive or an untyped lambda
/// parameter), so a shadowed name never falls through to an outer declaration.
fn declared_type(node: Node<'_>, name: &str, source: &[u8]) -> Option<Option<String>> {
    let mut child = node;
    let mut current = node.parent();
    while let Some(scope) = current {
        let found = match scope.kind() {
            "method_declaration"
            | "constructor_declaration"
            | "compact_constructor_declaration"
            | "lambda_expression"
            | "record_declaration" => parameter_type(scope, name, source),
            "block" | "constructor_body" => local_variable_type(scope, child, name, source),
            // `case Order order -> order.ship();`, then any locals of an old-style group.
            "switch_rule" | "switch_block_statement_group" => {
                let mut cursor = scope.walk();
                let labels = scope
                    .named_children(&mut cursor)
                    .filter(|label| label.kind() == "switch_label" && *label != child)
                    .collect::<Vec<_>>();
                labels
                    .into_iter()
                    .find_map(|label| pattern_binding_type(label, name, source))
                    .or_else(|| {
                        (scope.kind() == "switch_block_statement_group")
                            .then(|| local_variable_type(scope, child, name, source))
                            .flatten()
                    })
            }
            "enhanced_for_statement" => {
                (named_child_text(scope, "name", source).as_deref() == Some(name)).then(|| {
                    scope
                        .child_by_field_name("type")
                        .and_then(|declared| type_name(declared, source))
                })
            }
            "catch_clause" => catch_parameter_type(scope, name, source),
            "try_with_resources_statement" => resource_type(scope, name, source),
            "for_statement" => scope
                .child_by_field_name("init")
                .filter(|init| init.kind() == "local_variable_declaration" && *init != child)
                .and_then(|init| declarator_type(init, name, source)),
            // `if (x instanceof Order order) { order.total(); }`
            "if_statement" | "while_statement" => scope
                .child_by_field_name("condition")
                .filter(|condition| *condition != child)
                .and_then(|condition| pattern_binding_type(condition, name, source)),
            // `x instanceof Order order && order.isOpen()`
            "binary_expression" => scope
                .child_by_field_name("left")
                .filter(|left| *left != child)
                .and_then(|left| pattern_binding_type(left, name, source)),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
        if TYPE_DECLARATIONS.contains(&scope.kind())
            && let Some(declared) = field_type(scope, name, source)
        {
            return Some(Some(declared));
        }
        // An anonymous class's own fields come before the enclosing class's.
        let is_anonymous_body = scope.kind() == "class_body"
            && scope.parent().is_some_and(|owner| {
                matches!(owner.kind(), "object_creation_expression" | "enum_constant")
            });
        if is_anonymous_body && let Some(declared) = body_field_type(scope, name, source) {
            return Some(Some(declared));
        }
        child = scope;
        current = scope.parent();
    }
    None
}

fn parameter_type(scope: Node<'_>, name: &str, source: &[u8]) -> Option<Option<String>> {
    let parameters = scope.child_by_field_name("parameters")?;
    if parameters.kind() == "identifier" {
        // `x -> ...`: a single untyped lambda parameter.
        return (node_text(parameters, source).trim() == name).then_some(None);
    }
    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        match parameter.kind() {
            "formal_parameter" => {
                if named_child_text(parameter, "name", source).as_deref() == Some(name) {
                    return Some(
                        parameter
                            .child_by_field_name("type")
                            .and_then(|declared| type_name(declared, source)),
                    );
                }
            }
            "identifier" => {
                // `(a, b) -> ...`: inferred lambda parameters have no written type.
                if node_text(parameter, source).trim() == name {
                    return Some(None);
                }
            }
            "spread_parameter" => {
                let mut inner = parameter.walk();
                let declares = parameter
                    .named_children(&mut inner)
                    .filter(|child| child.kind() == "variable_declarator")
                    .any(|declarator| {
                        named_child_text(declarator, "name", source).as_deref() == Some(name)
                    });
                if declares {
                    // A varargs parameter is an array, which has no methods of its own here.
                    return Some(None);
                }
            }
            _ => {}
        }
    }
    None
}

/// Local variables declared in `block` before the statement that contains the call.
fn local_variable_type(
    block: Node<'_>,
    containing: Node<'_>,
    name: &str,
    source: &[u8],
) -> Option<Option<String>> {
    let mut found = None;
    let mut cursor = block.walk();
    for statement in block.named_children(&mut cursor) {
        if statement.start_byte() >= containing.start_byte() {
            break;
        }
        if statement.kind() == "local_variable_declaration"
            && let Some(declared) = declarator_type(statement, name, source)
        {
            found = Some(declared);
        }
    }
    found
}

/// The type a `local_variable_declaration` gives `name`, reading `var x = new T()` as `T`.
fn declarator_type(statement: Node<'_>, name: &str, source: &[u8]) -> Option<Option<String>> {
    let declared = statement.child_by_field_name("type");
    let mut declarators = statement.walk();
    let declarator = statement
        .children_by_field_name("declarator", &mut declarators)
        .find(|declarator| {
            named_child_text(*declarator, "name", source).as_deref() == Some(name)
        })?;
    let inferred = declared
        .filter(|declared| node_text(*declared, source).trim() == "var")
        .map(|_| {
            declarator
                .child_by_field_name("value")
                .filter(|value| value.kind() == "object_creation_expression")
                .and_then(|value| value.child_by_field_name("type"))
                .and_then(|created| type_name(created, source))
        });
    Some(inferred.unwrap_or_else(|| declared.and_then(|ty| type_name(ty, source))))
}

/// The type a pattern inside `node` binds `name` to: `x instanceof Order order` or a
/// `case Order order ->` type pattern.
fn pattern_binding_type(node: Node<'_>, name: &str, source: &[u8]) -> Option<Option<String>> {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        match current.kind() {
            "instanceof_expression" => {
                if named_child_text(current, "name", source).as_deref() == Some(name) {
                    return Some(
                        current
                            .child_by_field_name("right")
                            .and_then(|declared| type_name(declared, source)),
                    );
                }
            }
            "type_pattern" => {
                let mut cursor = current.walk();
                let children = current.named_children(&mut cursor).collect::<Vec<_>>();
                if let [declared, binding] = children.as_slice()
                    && binding.kind() == "identifier"
                    && node_text(*binding, source).trim() == name
                {
                    return Some(type_name(*declared, source));
                }
            }
            // Patterns inside a lambda don't bind names outside it.
            "lambda_expression" => continue,
            _ => {}
        }
        let mut cursor = current.walk();
        stack.extend(current.named_children(&mut cursor));
    }
    None
}

fn catch_parameter_type(clause: Node<'_>, name: &str, source: &[u8]) -> Option<Option<String>> {
    let mut cursor = clause.walk();
    let parameter = clause
        .named_children(&mut cursor)
        .find(|child| child.kind() == "catch_formal_parameter")?;
    if named_child_text(parameter, "name", source).as_deref() != Some(name) {
        return None;
    }
    let mut inner = parameter.walk();
    let catch_type = parameter
        .named_children(&mut inner)
        .find(|child| child.kind() == "catch_type");
    Some(catch_type.and_then(|caught| first_type_name(caught, source)))
}

fn resource_type(statement: Node<'_>, name: &str, source: &[u8]) -> Option<Option<String>> {
    let resources = statement.child_by_field_name("resources")?;
    let mut cursor = resources.walk();
    for resource in resources.named_children(&mut cursor) {
        if resource.kind() == "resource"
            && named_child_text(resource, "name", source).as_deref() == Some(name)
        {
            return Some(
                resource
                    .child_by_field_name("type")
                    .and_then(|declared| type_name(declared, source)),
            );
        }
    }
    None
}

/// The declared type of field `name` in a type's body, or of a record component.
fn field_type(type_declaration: Node<'_>, name: &str, source: &[u8]) -> Option<String> {
    if type_declaration.kind() == "record_declaration"
        && let Some(Some(component)) = parameter_type(type_declaration, name, source)
    {
        return Some(component);
    }
    body_field_type(type_declaration.child_by_field_name("body")?, name, source)
}

/// The declared type of field `name` in a class, interface or enum body.
fn body_field_type(body: Node<'_>, name: &str, source: &[u8]) -> Option<String> {
    let mut cursor = body.walk();
    let mut members = Vec::new();
    for member in body.named_children(&mut cursor) {
        // An enum's fields follow its constants, inside `enum_body_declarations`.
        if member.kind() == "enum_body_declarations" {
            let mut inner = member.walk();
            members.extend(member.named_children(&mut inner));
        } else {
            members.push(member);
        }
    }
    for member in members {
        if !matches!(member.kind(), "field_declaration" | "constant_declaration") {
            continue;
        }
        let mut declarators = member.walk();
        let declares = member
            .children_by_field_name("declarator", &mut declarators)
            .any(|declarator| {
                named_child_text(declarator, "name", source).as_deref() == Some(name)
            });
        if declares {
            return member
                .child_by_field_name("type")
                .and_then(|declared| type_name(declared, source));
        }
    }
    None
}

/// The written name of a class or interface type without type arguments: `Repo`,
/// `Order.Builder` or `java.util.List`. Primitives, arrays and `var` have none.
fn type_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "type_identifier" => {
            let name = node_text(node, source).trim().to_owned();
            (!name.is_empty() && name != "var").then_some(name)
        }
        "generic_type" => {
            let mut cursor = node.walk();
            let base = node.named_children(&mut cursor).find(|child| {
                matches!(child.kind(), "type_identifier" | "scoped_type_identifier")
            })?;
            type_name(base, source)
        }
        "scoped_type_identifier" => {
            let mut cursor = node.walk();
            let segments = node
                .named_children(&mut cursor)
                .filter(|child| {
                    matches!(
                        child.kind(),
                        "type_identifier" | "scoped_type_identifier" | "generic_type"
                    )
                })
                .map(|child| type_name(child, source))
                .collect::<Option<Vec<_>>>()?;
            (!segments.is_empty()).then(|| segments.join("."))
        }
        _ => None,
    }
}

/// The first class or interface type named anywhere inside `node`.
fn first_type_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    if let Some(name) = type_name(node, source) {
        return Some(name);
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find_map(|child| first_type_name(child, source))
}

struct Import {
    path: String,
    is_static: bool,
    is_wildcard: bool,
}

fn import_target(node: Node<'_>, source: &[u8]) -> Option<Import> {
    let text = node_text(node, source);
    let rest = text.trim().strip_prefix("import")?.trim_start();
    let (is_static, rest) = match rest.strip_prefix("static") {
        Some(after) if after.starts_with(char::is_whitespace) => (true, after.trim_start()),
        _ => (false, rest),
    };
    let path = rest
        .trim_end_matches(';')
        .split_whitespace()
        .collect::<String>();
    if path.is_empty() {
        return None;
    }
    Some(Import {
        is_wildcard: path.ends_with(".*"),
        path,
        is_static,
    })
}

/// What a file says about the types it names: its package, its imports and the named types
/// it declares.
struct FileContext {
    /// Simple name → fully qualified name, for single-type imports.
    imports: HashMap<String, String>,
    /// Statically imported member → fully qualified name of its class.
    static_members: HashMap<String, String>,
    /// The first two segments of the file's package (`com.acme`). Imports outside it come
    /// from other projects.
    project_prefix: Option<String>,
    /// Qualified names of the types the file declares: `Order`, `Order::Builder`.
    declared_types: HashSet<String>,
    /// Qualified names of the methods those types declare: `Order::total`.
    declared_methods: HashSet<String>,
}

impl FileContext {
    fn of(node: Node<'_>, source: &[u8]) -> Self {
        let mut root = node;
        while let Some(parent) = root.parent() {
            root = parent;
        }
        let mut context = Self {
            imports: HashMap::new(),
            static_members: HashMap::new(),
            project_prefix: None,
            declared_types: HashSet::new(),
            declared_methods: HashSet::new(),
        };
        let mut cursor = root.walk();
        for child in root.named_children(&mut cursor) {
            match child.kind() {
                "package_declaration" => {
                    let mut inner = child.walk();
                    context.project_prefix = child
                        .named_children(&mut inner)
                        .find(|name| matches!(name.kind(), "scoped_identifier" | "identifier"))
                        .map(|name| project_prefix(&node_text(name, source)))
                        .filter(|prefix| !prefix.is_empty());
                }
                "import_declaration" => {
                    let Some(import) = import_target(child, source) else {
                        continue;
                    };
                    if import.is_wildcard {
                        continue;
                    }
                    let Some((owner, member)) = import.path.rsplit_once('.') else {
                        continue;
                    };
                    if import.is_static {
                        context
                            .static_members
                            .insert(member.to_owned(), owner.to_owned());
                    } else {
                        context
                            .imports
                            .insert(member.to_owned(), import.path.clone());
                    }
                }
                _ => {}
            }
        }

        // Named types live at the top level and in type bodies; method bodies only hold
        // local and anonymous classes, which can't be named from outside.
        let mut stack = {
            let mut cursor = root.walk();
            root.named_children(&mut cursor).collect::<Vec<_>>()
        };
        while let Some(node) = stack.pop() {
            if TYPE_DECLARATIONS.contains(&node.kind()) {
                if let Some(name) = named_child_text(node, "name", source)
                    && let Some(parents) = declaring_types(node, source)
                {
                    context.declared_types.insert(qualify(&parents, &name));
                }
                if let Some(body) = node.child_by_field_name("body") {
                    let mut cursor = body.walk();
                    stack.extend(body.named_children(&mut cursor));
                }
            } else if node.kind() == "enum_body_declarations" {
                let mut cursor = node.walk();
                stack.extend(node.named_children(&mut cursor));
            } else if node.kind() == "method_declaration"
                && let Some(name) = named_child_text(node, "name", source)
                && let Some(parents) = declaring_types(node, source)
            {
                context.declared_methods.insert(qualify(&parents, &name));
            }
        }
        context
    }

    /// The type an unqualified call `name(...)` is spelled with: the innermost enclosing type
    /// that declares a method of that name, as Java resolves it (an inner class calling its
    /// outer class's helper). Without a declaration in the file, the immediately enclosing
    /// class, or no owner when that is an anonymous class.
    fn method_owner(&self, call: Node<'_>, name: &str, source: &[u8]) -> Option<String> {
        let mut immediate = None;
        let mut crossed_anonymous = false;
        let mut current = call.parent();
        while let Some(scope) = current {
            if scope.kind() == "class_body"
                && scope.parent().is_some_and(|owner| {
                    matches!(owner.kind(), "object_creation_expression" | "enum_constant")
                })
            {
                crossed_anonymous = true;
            }
            if TYPE_DECLARATIONS.contains(&scope.kind())
                && let Some(type_name) = named_child_text(scope, "name", source)
                && let Some(parents) = declaring_types(scope, source)
            {
                let owner = qualify(&parents, &type_name);
                if self.declared_methods.contains(&format!("{owner}::{name}")) {
                    return Some(owner);
                }
                if immediate.is_none() && !crossed_anonymous {
                    immediate = Some(owner);
                }
                crossed_anonymous = true;
            }
            current = scope.parent();
        }
        immediate
    }

    /// Whether a fully qualified name belongs to another project: the JDK, or any package
    /// outside the file's own two leading package segments.
    fn is_external(&self, qualified: &str) -> bool {
        if qualified.starts_with("java.") || qualified.starts_with("javax.") {
            return true;
        }
        match &self.project_prefix {
            Some(prefix) => !qualified
                .strip_prefix(prefix.as_str())
                .is_some_and(|rest| rest.starts_with('.')),
            None => false,
        }
    }

    /// Whether a written type comes from another project: through an import (`Map` or
    /// `Map.Entry` with `java.util.Map` imported) or as a fully qualified name in such a
    /// package.
    fn is_external_type(&self, written: &str) -> bool {
        let first = written.split('.').next().unwrap_or(written);
        match self.imports.get(first) {
            Some(qualified) => self.is_external(qualified),
            None => first.starts_with(char::is_lowercase) && self.is_external(written),
        }
    }

    /// Spells a written type the way the parser qualifies Java types:
    /// - a type from another project keeps its dotted, fully qualified name
    ///   (`java.util.Map.Entry`), which no workspace symbol has;
    /// - a simple name that refers to a nested type gets its declaring types
    ///   (`Builder` → `Order::Builder`);
    /// - otherwise the written segments are joined with `::`, without a package prefix.
    fn spell(&self, written: &str, at: Node<'_>, source: &[u8]) -> String {
        let segments = written.split('.').collect::<Vec<_>>();
        match self.imports.get(segments[0]) {
            Some(qualified) if self.is_external(qualified) => {
                return std::iter::once(qualified.as_str())
                    .chain(segments[1..].iter().copied())
                    .collect::<Vec<_>>()
                    .join(".");
            }
            // An import from this project names the type by its path: `com.acme.Order.Status`
            // is `Order::Status`.
            Some(qualified) => {
                return type_path_from_qualified(qualified)
                    .into_iter()
                    .chain(segments[1..].iter().map(|segment| (*segment).to_owned()))
                    .collect::<Vec<_>>()
                    .join("::");
            }
            None if segments.len() > 1 && segments[0].starts_with(char::is_lowercase) => {
                // A fully qualified name written inline.
                if self.is_external(written) {
                    return written.to_owned();
                }
                return type_path_from_qualified(written).join("::");
            }
            None => {}
        }
        if let [simple] = segments.as_slice()
            && let Some(nested) = self.nested_type(simple, at, source)
        {
            return nested;
        }
        segments.join("::")
    }

    /// The nested type a simple name refers to at `at`: a member type of an enclosing type,
    /// innermost first. Another class's nested types need an import or a qualified name.
    fn nested_type(&self, simple: &str, at: Node<'_>, source: &[u8]) -> Option<String> {
        let mut current = at.parent();
        while let Some(scope) = current {
            if TYPE_DECLARATIONS.contains(&scope.kind())
                && let Some(name) = named_child_text(scope, "name", source)
                && let Some(parents) = declaring_types(scope, source)
            {
                let candidate = format!("{}::{simple}", qualify(&parents, &name));
                if self.declared_types.contains(&candidate) {
                    return Some(candidate);
                }
            }
            current = scope.parent();
        }
        None
    }

    /// Written type names that never point at a workspace declaration: `java.lang` types
    /// that no import, declaration or member type in scope shadows, type parameters in scope
    /// and types from other projects.
    fn excludes(
        &self,
        written: &str,
        type_parameters: &HashSet<String>,
        at: Node<'_>,
        source: &[u8],
    ) -> bool {
        let simple = !written.contains('.');
        let java_lang = simple
            && JAVA_LANG_TYPES.contains(&written)
            && !self.imports.contains_key(written)
            && !self.declared_types.contains(written)
            && self.nested_type(written, at, source).is_none();
        java_lang || (simple && type_parameters.contains(written)) || self.is_external_type(written)
    }

    /// The class a statically imported member belongs to, spelled like [`Self::spell`].
    fn static_owner(&self, member: &str) -> Option<String> {
        let owner = self.static_members.get(member)?;
        if self.is_external(owner) {
            Some(owner.clone())
        } else {
            Some(type_path_from_qualified(owner).join("::"))
        }
    }
}

/// Organizations whose packages hold many unrelated projects. Under them, a project is
/// the third segment (`org.apache.hadoop`), not the second.
const SHARED_ORGANIZATIONS: &[&str] = &[
    "com.github",
    "com.google",
    "io.github",
    "org.apache",
    "org.eclipse",
    "org.springframework",
];

/// The package prefix that marks this project: the first two segments (`com.acme`), or
/// three under an organization that hosts many projects (`org.apache.hadoop`).
fn project_prefix(package: &str) -> String {
    let segments = package.split('.').map(str::trim).collect::<Vec<_>>();
    let two = segments
        .iter()
        .take(2)
        .copied()
        .collect::<Vec<_>>()
        .join(".");
    let length = if SHARED_ORGANIZATIONS.contains(&two.as_str()) {
        3
    } else {
        2
    };
    segments
        .iter()
        .take(length)
        .copied()
        .collect::<Vec<_>>()
        .join(".")
}

/// The type part of a fully qualified name, from its first capitalized segment:
/// `com.acme.Order.Status` → `Order`, `Status`.
fn type_path_from_qualified(qualified: &str) -> Vec<String> {
    let segments = qualified.split('.').collect::<Vec<_>>();
    let start = segments
        .iter()
        .position(|segment| segment.starts_with(char::is_uppercase))
        .unwrap_or(segments.len().saturating_sub(1));
    segments[start..]
        .iter()
        .map(|segment| (*segment).to_owned())
        .collect()
}

/// Type parameters in scope at `node`: its own and those of every enclosing declaration.
fn type_parameter_names(node: Node<'_>, source: &[u8]) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut current = Some(node);
    while let Some(scope) = current {
        if let Some(parameters) = scope.child_by_field_name("type_parameters") {
            let mut cursor = parameters.walk();
            for parameter in parameters.named_children(&mut cursor) {
                let mut inner = parameter.walk();
                if let Some(name) = parameter
                    .named_children(&mut inner)
                    .find(|child| child.kind() == "type_identifier")
                {
                    names.insert(node_text(name, source).trim().to_owned());
                }
            }
        }
        current = scope.parent();
    }
    names
}

/// One type-reference edge per workspace type named in a method's return type or a method's
/// or constructor's parameters.
fn type_ref_edges(
    declaration: Node<'_>,
    source: &[u8],
    file_path: &str,
    symbols: &[Symbol],
) -> Vec<SymbolEdge> {
    let Some(source_symbol) = symbol_for_node(symbols, declaration) else {
        return Vec::new();
    };
    let context = FileContext::of(declaration, source);
    let type_parameters = type_parameter_names(declaration, source);

    let mut targets = Vec::new();
    for field in ["type", "parameters"] {
        if let Some(node) = declaration.child_by_field_name(field) {
            collect_type_refs(node, source, &context, &type_parameters, &mut targets);
        }
    }
    targets.sort();
    targets.dedup();

    targets
        .into_iter()
        .map(|target| SymbolEdge {
            source_id: source_symbol.id.clone(),
            target_qualified_name: target,
            edge_kind: EdgeKind::TypeRef,
            file_path: file_path.to_owned(),
        })
        .collect()
}

fn collect_type_refs(
    node: Node<'_>,
    source: &[u8],
    context: &FileContext,
    type_parameters: &HashSet<String>,
    out: &mut Vec<String>,
) {
    match node.kind() {
        "type_identifier" | "scoped_type_identifier" => {
            if let Some(written) = type_name(node, source)
                && !context.excludes(&written, type_parameters, node, source)
            {
                out.push(context.spell(&written, node, source));
            }
        }
        // Annotations on parameters and parameter names are not type references.
        "marker_annotation" | "annotation" | "identifier" | "modifiers" => {}
        _ => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_type_refs(child, source, context, type_parameters, out);
            }
        }
    }
}

/// `extends` and `implements` clauses become edges from the declared type to each supertype.
fn implements_edges(
    declaration: Node<'_>,
    source: &[u8],
    file_path: &str,
    symbols: &[Symbol],
) -> Vec<SymbolEdge> {
    let Some(source_symbol) = symbol_for_node(symbols, declaration) else {
        return Vec::new();
    };
    let context = FileContext::of(declaration, source);
    let type_parameters = type_parameter_names(declaration, source);

    let mut clauses = Vec::new();
    for field in ["superclass", "interfaces"] {
        clauses.extend(declaration.child_by_field_name(field));
    }
    let mut cursor = declaration.walk();
    clauses.extend(
        declaration
            .named_children(&mut cursor)
            .filter(|child| child.kind() == "extends_interfaces"),
    );

    let mut targets = Vec::new();
    for clause in clauses {
        let mut stack = vec![clause];
        while let Some(node) = stack.pop() {
            match node.kind() {
                "type_identifier" | "scoped_type_identifier" | "generic_type" => {
                    if let Some(written) = type_name(node, source)
                        && !context.excludes(&written, &type_parameters, declaration, source)
                    {
                        targets.push(context.spell(&written, declaration, source));
                    }
                }
                _ => {
                    let mut inner = node.walk();
                    stack.extend(node.named_children(&mut inner));
                }
            }
        }
    }
    targets.sort();
    targets.dedup();

    targets
        .into_iter()
        .map(|target| SymbolEdge {
            source_id: source_symbol.id.clone(),
            target_qualified_name: target,
            edge_kind: EdgeKind::Implements,
            file_path: file_path.to_owned(),
        })
        .collect()
}

fn symbol_for_node<'a>(symbols: &'a [Symbol], node: Node<'_>) -> Option<&'a Symbol> {
    symbols.iter().find(|symbol| {
        symbol.range.start_byte == Some(node.start_byte())
            && symbol.range.end_byte == Some(node.end_byte())
    })
}

fn annotations(declaration: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = declaration.walk();
    let Some(modifiers) = declaration
        .named_children(&mut cursor)
        .find(|child| child.kind() == "modifiers")
    else {
        return Vec::new();
    };
    let mut inner = modifiers.walk();
    modifiers
        .named_children(&mut inner)
        .filter(|child| matches!(child.kind(), "marker_annotation" | "annotation"))
        .collect()
}

/// The annotation's simple name: `Test` for `@Test` and `@org.junit.Test`.
fn annotation_name(annotation: Node<'_>, source: &[u8]) -> Option<String> {
    let name = node_text(annotation.child_by_field_name("name")?, source);
    name.rsplit('.')
        .next()
        .map(|simple| simple.trim().to_owned())
        .filter(|simple| !simple.is_empty())
}

fn has_annotation(declaration: Node<'_>, source: &[u8], names: &[&str]) -> bool {
    annotations(declaration).into_iter().any(|annotation| {
        annotation_name(annotation, source).is_some_and(|name| names.contains(&name.as_str()))
    })
}

/// The string literal passed to an annotation such as `@DisplayName("...")`.
fn annotation_string_argument(declaration: Node<'_>, source: &[u8], name: &str) -> Option<String> {
    let annotation = annotations(declaration).into_iter().find(|annotation| {
        annotation.kind() == "annotation"
            && annotation_name(*annotation, source).as_deref() == Some(name)
    })?;
    let arguments = annotation.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let literal = arguments
        .named_children(&mut cursor)
        .find(|child| child.kind() == "string_literal")?;
    let text = string_literal_text(literal, source);
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// The text of a string literal or text block, with escape sequences decoded.
fn string_literal_text(literal: Node<'_>, source: &[u8]) -> String {
    let mut text = String::new();
    let mut cursor = literal.walk();
    for part in literal.named_children(&mut cursor) {
        match part.kind() {
            "string_fragment" | "multiline_string_fragment" => {
                text.push_str(&node_text(part, source));
            }
            "escape_sequence" => text.push_str(&decode_escape(&node_text(part, source))),
            _ => {}
        }
    }
    text
}

/// The prose of a `/** ... */` comment directly above a declaration, without its block tags.
fn javadoc(node: Node<'_>, source: &[u8]) -> Option<String> {
    let comment = node.prev_sibling()?;
    if comment.kind() != "block_comment"
        || comment.end_position().row + 1 < node.start_position().row
    {
        return None;
    }
    let text = node_text(comment, source);
    let body = text.trim().strip_prefix("/**")?.trim_end_matches("*/");
    let prose = body
        .lines()
        .map(|line| line.trim().trim_start_matches('*').trim())
        .take_while(|line| !line.starts_with('@'))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let normalized = collapse_whitespace(&prose);
    (!normalized.is_empty()).then_some(normalized)
}

/// Decodes one escape sequence: `\u00e9` → `é`, octal `\101` → `A`, `\"` → `"`. Whitespace
/// and control escapes become a space.
fn decode_escape(escape: &str) -> String {
    let Some(body) = escape.strip_prefix('\\') else {
        return escape.to_owned();
    };
    let decoded = if body.starts_with('u') {
        u32::from_str_radix(body.trim_start_matches('u'), 16)
            .ok()
            .and_then(char::from_u32)
    } else if !body.is_empty() && body.bytes().all(|byte| (b'0'..=b'7').contains(&byte)) {
        u32::from_str_radix(body, 8).ok().and_then(char::from_u32)
    } else {
        match body {
            "n" | "r" | "t" | "s" | "b" | "f" => Some(' '),
            other => other.chars().next(),
        }
    };
    match decoded {
        Some(ch) if ch.is_control() => " ".to_owned(),
        Some(ch) => ch.to_string(),
        None => String::new(),
    }
}

/// Joins whitespace runs into single spaces. Unlike the shared intent normalizer it keeps
/// quotes, which a display name may legitimately start or end with.
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `shouldRejectEmptyInput` becomes "should reject empty input", and a leading `test`
/// prefix is dropped: `testParse_emptyInput` becomes "parse empty input".
fn humanize_test_name(name: &str) -> String {
    let trimmed = name
        .strip_prefix("test")
        .filter(|rest| rest.starts_with(|ch: char| ch.is_uppercase() || ch == '_'))
        .unwrap_or(name);
    split_identifier_words(trimmed).join(" ")
}

fn split_identifier_words(value: &str) -> Vec<String> {
    let chars = value.chars().collect::<Vec<_>>();
    let mut words = Vec::new();
    let mut current = String::new();
    for (index, &ch) in chars.iter().enumerate() {
        if ch == '_' || ch == '-' || ch.is_whitespace() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            continue;
        }
        if !current.is_empty() {
            let previous = chars[index - 1];
            let next = chars.get(index + 1).copied();
            let starts_word = (ch.is_uppercase()
                && (previous.is_lowercase() || previous.is_ascii_digit()))
                || (ch.is_uppercase()
                    && previous.is_uppercase()
                    && next.is_some_and(char::is_lowercase))
                || (ch.is_ascii_digit() && previous.is_alphabetic());
            if starts_word {
                words.push(std::mem::take(&mut current));
            }
        }
        current.extend(ch.to_lowercase());
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::humanize_test_name;

    #[test]
    fn humanizes_junit_method_names() {
        assert_eq!(
            humanize_test_name("shouldRejectEmptyInput"),
            "should reject empty input"
        );
        assert_eq!(
            humanize_test_name("testParse_emptyInput"),
            "parse empty input"
        );
        assert_eq!(humanize_test_name("testing"), "testing");
        assert_eq!(
            humanize_test_name("returnsHTTP404ForMissingKey"),
            "returns http 404 for missing key"
        );
    }
}
