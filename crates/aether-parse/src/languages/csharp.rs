use std::collections::HashSet;

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
    let ts_language = tree_sitter_c_sharp::LANGUAGE.into();
    let symbol_query = Query::new(&ts_language, include_str!("../queries/csharp_symbols.scm"))
        .expect("invalid csharp symbol query");
    let edge_query = Query::new(&ts_language, include_str!("../queries/csharp_edges.scm"))
        .expect("invalid csharp edge query");
    let test_intent_query = Query::new(
        &ts_language,
        include_str!("../queries/csharp_test_intents.scm"),
    )
    .expect("invalid csharp test intent query");

    LanguageConfig {
        id: "csharp",
        extensions: &["cs"],
        ts_language,
        symbol_query,
        edge_query,
        test_intent_query: Some(test_intent_query),
        // Project files (`*.csproj`) have no fixed name, so only solution-wide files qualify.
        module_markers: &["Directory.Build.props", "global.json"],
        hooks: Some(Box::new(CSharpHooks)),
    }
}

struct CSharpHooks;

/// Declarations that introduce a named type.
const TYPE_DECLARATIONS: &[&str] = &[
    "class_declaration",
    "struct_declaration",
    "interface_declaration",
    "enum_declaration",
    "record_declaration",
    "delegate_declaration",
];

/// Nodes whose bodies hold local code. Declarations nested inside them are not addressable
/// from outside, so they are not indexed.
const LOCAL_SCOPES: &[&str] = &[
    "block",
    "arrow_expression_clause",
    "lambda_expression",
    "anonymous_method_expression",
    "local_function_statement",
];

/// Base class library types that workspace code references constantly. References to them
/// never resolve to workspace symbols, so they are not recorded as type references.
const BCL_TYPES: &[&str] = &[
    "Action",
    "ArgumentException",
    "ArgumentNullException",
    "ArgumentOutOfRangeException",
    "Array",
    "Attribute",
    "CancellationToken",
    "Console",
    "DateTime",
    "DateTimeOffset",
    "Dictionary",
    "Enum",
    "EventArgs",
    "EventHandler",
    "Exception",
    "Func",
    "Guid",
    "HashSet",
    "IAsyncDisposable",
    "IAsyncEnumerable",
    "ICollection",
    "IComparable",
    "IDictionary",
    "IDisposable",
    "IEnumerable",
    "IEnumerator",
    "IEquatable",
    "IList",
    "IReadOnlyCollection",
    "IReadOnlyDictionary",
    "IReadOnlyList",
    "ISet",
    "InvalidOperationException",
    "KeyValuePair",
    "Lazy",
    "List",
    "Math",
    "Memory",
    "NotImplementedException",
    "NotSupportedException",
    "Nullable",
    "Object",
    "Queue",
    "ReadOnlyMemory",
    "ReadOnlySpan",
    "Span",
    "Stack",
    "Stream",
    "String",
    "StringBuilder",
    "Task",
    "TimeSpan",
    "Tuple",
    "Type",
    "Uri",
    "ValueTask",
    "ValueTuple",
];

/// xUnit (`Fact`, `Theory`), NUnit (`Test`, `TestCase`, `TestCaseSource`) and MSTest
/// (`TestMethod`, `DataTestMethod`) attributes that mark a test method.
const TEST_ATTRIBUTES: &[&str] = &[
    "Fact",
    "Theory",
    "Test",
    "TestCase",
    "TestCaseSource",
    "TestMethod",
    "DataTestMethod",
];

impl LanguageHooks for CSharpHooks {
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
            "symbol.csharp_class" => SymbolKind::Class,
            "symbol.csharp_record" => {
                if has_token(node, "struct") {
                    SymbolKind::Struct
                } else {
                    SymbolKind::Class
                }
            }
            "symbol.csharp_struct" => SymbolKind::Struct,
            "symbol.csharp_interface" => SymbolKind::Interface,
            "symbol.csharp_enum" => SymbolKind::Enum,
            "symbol.csharp_delegate" => SymbolKind::TypeAlias,
            "symbol.csharp_method" => {
                // Abstract, interface, extern and partial declarations without a body are part
                // of their type's text, as Rust trait signatures are.
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
            "edge.depends_on" => using_target(node, source)
                .map(|target| SymbolEdge {
                    source_id: file_source_id(file_path),
                    target_qualified_name: target,
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
        let attributes = attributes(node, source);
        let test_attribute = attributes
            .iter()
            .find(|(name, _)| TEST_ATTRIBUTES.contains(&name.as_str()))?;
        let method_name = named_child_text(node, "name", source)?;
        // Test classes in one file may reuse method names. A test is named by its path below
        // the file's outermost class, or by its full path when the file declares several
        // top-level classes. Helper enums, interfaces and structs don't count.
        let several_top_level = FileTypes::of(node, source).top_level_classes.len() > 1;
        let test_name = match declaring_types(node, source) {
            Some(types) if several_top_level => format!("{}::{method_name}", types.join("::")),
            Some(types) if types.len() > 1 => format!("{}::{method_name}", types[1..].join("::")),
            _ => method_name.clone(),
        };

        let intent_text = display_name(test_attribute.0.as_str(), test_attribute.1, source)
            .or_else(|| {
                attributes
                    .iter()
                    .find(|(name, _)| name == "Description")
                    .and_then(|(_, attribute)| positional_string_argument(*attribute, source))
            })
            .or_else(|| xml_doc_summary(node, source))
            .unwrap_or_else(|| humanize_test_name(&method_name));
        let intent_text = collapse_whitespace(&intent_text);
        if intent_text.is_empty() {
            return None;
        }

        let group_label = enclosing_type(node)
            .and_then(|class| named_child_text(class, "name", source))
            .map(|label| collapse_whitespace(&label));

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

/// A declaration's text up to its body, without the declaration's own attributes and
/// comments. Adding `[Fact]`, `[Obsolete]` or `[Fact(Skip = "...")]` therefore keeps the
/// symbol id, as adding an attribute to a Rust item does. The enclosing namespaces and the
/// type parameters of enclosing types come first, so `Mapper` in two namespace blocks of one
/// file, and members of `Result` and `Result<T>`, get different ids. A file-scoped namespace
/// counts the same as a block, so converting between the two keeps ids.
fn signature_text(node: Node<'_>, source: &[u8]) -> String {
    let end = node
        .child_by_field_name("body")
        .map_or(node.end_byte(), |body| body.start_byte());
    let mut text = String::new();
    let mut current = node.parent();
    while let Some(scope) = current {
        let prefix = match scope.kind() {
            kind if TYPE_DECLARATIONS.contains(&kind) => {
                let mut cursor = scope.walk();
                scope
                    .named_children(&mut cursor)
                    .find(|child| child.kind() == "type_parameter_list")
                    .map(|parameters| node_text(parameters, source))
            }
            // `namespace A { namespace B {` and `namespace A.B;` both give `A.B.`.
            "namespace_declaration" => scope
                .child_by_field_name("name")
                .map(|name| format!("{}.", node_text(name, source))),
            "compilation_unit" => {
                let mut cursor = scope.walk();
                scope
                    .named_children(&mut cursor)
                    .find(|child| child.kind() == "file_scoped_namespace_declaration")
                    .and_then(|namespace| namespace.child_by_field_name("name"))
                    .map(|name| format!("{}.", node_text(name, source)))
            }
            _ => None,
        };
        if let Some(prefix) = prefix {
            text.insert_str(0, &prefix);
        }
        current = scope.parent();
    }
    let mut position = node.start_byte();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.start_byte() >= end {
            break;
        }
        if matches!(child.kind(), "attribute_list" | "comment") {
            if let Some(part) = source.get(position..child.start_byte()) {
                text.push_str(&String::from_utf8_lossy(part));
            }
            position = child.end_byte();
        }
    }
    if let Some(part) = source.get(position..end.max(position)) {
        text.push_str(&String::from_utf8_lossy(part));
    }
    text
}

/// Names of the types that declare `node`, outermost first. Namespaces are left out, as Rust
/// file modules are. `None` when `node` sits in a method body or lambda.
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

fn has_token(node: Node<'_>, kind: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| !child.is_named() && child.kind() == kind)
}

fn enclosing_type(node: Node<'_>) -> Option<Node<'_>> {
    let mut current = node.parent();
    while let Some(parent) = current {
        if TYPE_DECLARATIONS.contains(&parent.kind()) {
            return Some(parent);
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
    let target = match call.kind() {
        "invocation_expression" => invocation_target(call, source)?,
        "object_creation_expression" => {
            let created = type_written(call.child_by_field_name("type")?, source)?;
            // `new T()` creates whatever the type argument is.
            if type_parameter_names(call, source).contains(&created) {
                return None;
            }
            FileTypes::of(call, source).spell(&created, call, source)
        }
        _ => return None,
    };
    // Overloads share a qualified name, so an unqualified or `this.` call to the caller's own
    // name (an overload delegating to another, or recursion) would resolve to every overload
    // and read as a dependency cycle between them. A same-named method reached through a
    // field or `base.` may belong to a same-named type elsewhere and keeps its edge.
    if calls_own_type(call)
        && symbols
            .iter()
            .any(|symbol| symbol.id == source_id && symbol.qualified_name == target)
    {
        return None;
    }
    Some(SymbolEdge {
        source_id,
        target_qualified_name: target,
        edge_kind: EdgeKind::Calls,
        file_path: file_path.to_owned(),
    })
}

/// Whether an invocation calls a method of the enclosing type: `Name(...)` or `this.Name(...)`.
fn calls_own_type(call: Node<'_>) -> bool {
    let Some(function) = call
        .child_by_field_name("function")
        .filter(|_| call.kind() == "invocation_expression")
    else {
        return false;
    };
    match function.kind() {
        "identifier" | "generic_name" => true,
        "member_access_expression" => function
            .child_by_field_name("expression")
            .is_some_and(|receiver| receiver.kind() == "this"),
        _ => false,
    }
}

/// Spells a call the way C# methods are qualified (`Type::Method`) whenever the receiver's
/// type can be read from the source: the enclosing type for unqualified and `this.` calls,
/// the first base type for `base.` calls, a declared variable, parameter, field or property,
/// or a capitalized type name. Anything else keeps the bare method name.
fn invocation_target(call: Node<'_>, source: &[u8]) -> Option<String> {
    let function = call.child_by_field_name("function")?;
    let (name, owner) = match function.kind() {
        "identifier" | "generic_name" => {
            let name = simple_name(function, source)?;
            // Invoking a delegate held in a variable, field or property has no static target.
            // (A type can't declare a method and a field with the same name.)
            if declared_type(call, &name, source).is_some() {
                return None;
            }
            let types = FileTypes::of(call, source);
            let owner = types.method_owner(call, &name, source);
            // `nameof(x)` is an operator unless the file declares a method of that name.
            if name == "nameof"
                && owner
                    .as_ref()
                    .is_none_or(|owner| !types.methods.contains(&format!("{owner}::nameof")))
            {
                return None;
            }
            (name, owner)
        }
        "member_access_expression" => {
            let name = simple_name(function.child_by_field_name("name")?, source)?;
            let receiver = function.child_by_field_name("expression")?;
            (name, receiver_type(receiver, call, source))
        }
        // `repo?.Save()`: the member binding carries the name and the enclosing conditional
        // access carries the receiver, whichever of the two wraps the invocation.
        "conditional_access_expression" => {
            let mut cursor = function.walk();
            let binding = function
                .named_children(&mut cursor)
                .find(|child| child.kind() == "member_binding_expression")?;
            let name = simple_name(binding.child_by_field_name("name")?, source)?;
            let receiver = function.child_by_field_name("condition")?;
            (name, receiver_type(receiver, call, source))
        }
        "member_binding_expression" => {
            let name = simple_name(function.child_by_field_name("name")?, source)?;
            let owner = call
                .parent()
                .filter(|parent| parent.kind() == "conditional_access_expression")
                .and_then(|access| access.child_by_field_name("condition"))
                .and_then(|receiver| receiver_type(receiver, call, source));
            (name, owner)
        }
        _ => return None,
    };
    Some(match owner {
        Some(owner) => format!("{owner}::{name}"),
        None => name,
    })
}

fn receiver_type(receiver: Node<'_>, call: Node<'_>, source: &[u8]) -> Option<String> {
    let written = match receiver.kind() {
        "this" => return enclosing_type_name(call, source),
        "base" => enclosing_type(call)
            .and_then(|class| base_types(class).into_iter().next())
            .and_then(|base| type_written(base, source)),
        "identifier" => {
            let receiver_name = node_text(receiver, source).trim().to_owned();
            match declared_type(call, &receiver_name, source) {
                Some(declared) => declared,
                None if receiver_name.starts_with(char::is_uppercase) => Some(receiver_name),
                None => None,
            }
        }
        "generic_name" | "qualified_name" | "alias_qualified_name" => {
            type_written(receiver, source)
        }
        // `string.Join(...)`: a keyword type keeps its keyword.
        "predefined_type" => {
            let text = node_text(receiver, source).trim().to_owned();
            return (!text.is_empty()).then_some(text);
        }
        "member_access_expression" => this_member_type(receiver, call, source),
        _ => None,
    }?;
    // A receiver typed by a type parameter has no declaration to point at.
    if type_parameter_names(call, source).contains(&written) {
        return None;
    }
    Some(FileTypes::of(call, source).spell(&written, call, source))
}

/// `this.repo` resolves through the enclosing type's fields and properties.
fn this_member_type(access: Node<'_>, call: Node<'_>, source: &[u8]) -> Option<String> {
    let object = access.child_by_field_name("expression")?;
    if object.kind() != "this" {
        return None;
    }
    let member = simple_name(access.child_by_field_name("name")?, source)?;
    member_type(enclosing_type(call)?, &member, source).flatten()
}

/// Finds the declared type of `name` as seen from `node`: parameters, local variables,
/// `foreach`, `catch` and `using` variables in enclosing scopes, then fields, properties and
/// primary-constructor parameters of the enclosing types. The outer `Option` says whether
/// `name` was found; the inner one is `None` when it was declared without a usable type
/// (`var` without `new`, a predefined type, or an untyped lambda parameter), so a shadowed
/// name never falls through to an outer declaration.
fn declared_type(node: Node<'_>, name: &str, source: &[u8]) -> Option<Option<String>> {
    let mut child = node;
    let mut current = node.parent();
    while let Some(scope) = current {
        let found = match scope.kind() {
            "method_declaration"
            | "constructor_declaration"
            | "local_function_statement"
            | "lambda_expression"
            | "anonymous_method_expression" => parameter_type(scope, name, source),
            "block" => local_variable_type(scope, child, name, source),
            // `case Deposit deposit: deposit.Review(); break;`, then any locals of the section.
            "switch_section" => {
                let mut cursor = scope.walk();
                let patterns = scope
                    .named_children(&mut cursor)
                    .filter(|label| *label != child && label.kind().contains("pattern"))
                    .collect::<Vec<_>>();
                patterns
                    .into_iter()
                    .find_map(|pattern| pattern_binding_type(pattern, name, source))
                    .or_else(|| local_variable_type(scope, child, name, source))
            }
            // `Deposit deposit => deposit.Amount`
            "switch_expression_arm" => {
                let mut cursor = scope.walk();
                let patterns = scope
                    .named_children(&mut cursor)
                    .filter(|pattern| *pattern != child && pattern.kind().contains("pattern"))
                    .collect::<Vec<_>>();
                patterns
                    .into_iter()
                    .find_map(|pattern| pattern_binding_type(pattern, name, source))
            }
            "foreach_statement" => {
                (named_child_text(scope, "left", source).as_deref() == Some(name)).then(|| {
                    scope
                        .child_by_field_name("type")
                        .and_then(|declared| type_written(declared, source))
                })
            }
            "for_statement" => scope
                .child_by_field_name("initializer")
                .filter(|initializer| {
                    initializer.kind() == "variable_declaration" && *initializer != child
                })
                .and_then(|initializer| variable_type(initializer, name, source)),
            // `if (e is Deposit deposit) { deposit.Review(); }`
            "if_statement" | "while_statement" => scope
                .child_by_field_name("condition")
                .filter(|condition| *condition != child)
                .and_then(|condition| pattern_binding_type(condition, name, source)),
            // `e is Deposit deposit && deposit.IsLarge()`
            "binary_expression" => scope
                .child_by_field_name("left")
                .filter(|left| *left != child)
                .and_then(|left| pattern_binding_type(left, name, source)),
            "catch_clause" => catch_variable_type(scope, name, source),
            "using_statement" => {
                let mut cursor = scope.walk();
                scope
                    .named_children(&mut cursor)
                    .filter(|child| child.kind() == "variable_declaration")
                    .find_map(|declaration| variable_type(declaration, name, source))
            }
            _ => None,
        };
        if found.is_some() {
            return found;
        }
        // A member found without a usable type (`string Name`) still shadows outer names
        // and keeps a capitalized member from being read as a type.
        if TYPE_DECLARATIONS.contains(&scope.kind())
            && let Some(declared) = member_type(scope, name, source)
        {
            return Some(declared);
        }
        child = scope;
        current = scope.parent();
    }
    None
}

fn parameter_list(scope: Node<'_>) -> Option<Node<'_>> {
    scope.child_by_field_name("parameters").or_else(|| {
        let mut cursor = scope.walk();
        scope
            .named_children(&mut cursor)
            .find(|child| child.kind() == "parameter_list")
    })
}

fn parameter_type(scope: Node<'_>, name: &str, source: &[u8]) -> Option<Option<String>> {
    let parameters = parameter_list(scope)?;
    if parameters.kind() == "implicit_parameter" {
        // `x => ...`: a single untyped lambda parameter.
        return (node_text(parameters, source).trim() == name).then_some(None);
    }
    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        if parameter.kind() == "parameter"
            && named_child_text(parameter, "name", source).as_deref() == Some(name)
        {
            return Some(
                parameter
                    .child_by_field_name("type")
                    .and_then(|declared| type_written(declared, source)),
            );
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
        if statement.kind() != "local_declaration_statement" {
            continue;
        }
        let mut inner = statement.walk();
        for declaration in statement
            .named_children(&mut inner)
            .filter(|child| child.kind() == "variable_declaration")
        {
            if let Some(declared) = variable_type(declaration, name, source) {
                found = Some(declared);
            }
        }
    }
    found
}

/// The type a `variable_declaration` gives `name`, reading `var x = new T()` as `T`.
fn variable_type(declaration: Node<'_>, name: &str, source: &[u8]) -> Option<Option<String>> {
    let declared = declaration.child_by_field_name("type");
    let mut cursor = declaration.walk();
    let declarator = declaration
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "variable_declarator")
        .find(|declarator| {
            named_child_text(*declarator, "name", source).as_deref() == Some(name)
        })?;

    let is_var = declared.is_some_and(|declared| node_text(declared, source).trim() == "var");
    if !is_var {
        return Some(declared.and_then(|declared| type_written(declared, source)));
    }
    let mut inner = declarator.walk();
    let created = declarator
        .named_children(&mut inner)
        .find(|child| child.kind() == "object_creation_expression")
        .and_then(|creation| creation.child_by_field_name("type"))
        .and_then(|created| type_written(created, source));
    Some(created)
}

/// The type a pattern inside `node` binds `name` to: `e is Deposit deposit` or
/// `Deposit { Amount: > 0 } deposit`.
fn pattern_binding_type(node: Node<'_>, name: &str, source: &[u8]) -> Option<Option<String>> {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        match current.kind() {
            "declaration_pattern" | "recursive_pattern" => {
                if named_child_text(current, "name", source).as_deref() == Some(name) {
                    return Some(
                        current
                            .child_by_field_name("type")
                            .and_then(|declared| type_written(declared, source)),
                    );
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

fn catch_variable_type(clause: Node<'_>, name: &str, source: &[u8]) -> Option<Option<String>> {
    let mut cursor = clause.walk();
    let declaration = clause
        .named_children(&mut cursor)
        .find(|child| child.kind() == "catch_declaration")?;
    if named_child_text(declaration, "name", source).as_deref() != Some(name) {
        return None;
    }
    Some(
        declaration
            .child_by_field_name("type")
            .and_then(|declared| type_written(declared, source)),
    )
}

/// The declared type of field or property `name` in a type, or of a primary-constructor
/// parameter.
fn member_type(type_declaration: Node<'_>, name: &str, source: &[u8]) -> Option<Option<String>> {
    if let Some(parameter) = parameter_type(type_declaration, name, source) {
        return Some(parameter);
    }
    let body = type_declaration.child_by_field_name("body")?;
    let mut members = vec![body];
    while let Some(member) = members.pop() {
        match member.kind() {
            // Members of `#if`/`#elif`/`#else` blocks belong to the type.
            "declaration_list" | "preproc_if" | "preproc_elif" | "preproc_else" => {
                let mut cursor = member.walk();
                let children = member.named_children(&mut cursor).collect::<Vec<_>>();
                members.extend(children.into_iter().rev());
            }
            "field_declaration" | "event_field_declaration" => {
                let mut inner = member.walk();
                let found = member
                    .named_children(&mut inner)
                    .filter(|child| child.kind() == "variable_declaration")
                    .find_map(|declaration| variable_type(declaration, name, source));
                if found.is_some() {
                    return found;
                }
            }
            "property_declaration" | "event_declaration"
                if named_child_text(member, "name", source).as_deref() == Some(name) =>
            {
                return Some(
                    member
                        .child_by_field_name("type")
                        .and_then(|declared| type_written(declared, source)),
                );
            }
            _ => {}
        }
    }
    None
}

/// The simple name of a class, struct, interface or delegate type: `Repo` for `Repo`,
/// `Repo?`, `IList<Repo>` (gives `IList`) or `Acme.Data.Repo`. Predefined types, arrays,
/// tuples and `var` have none.
fn type_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" => {
            let name = node_text(node, source).trim().to_owned();
            (!name.is_empty() && name != "var").then_some(name)
        }
        "generic_name" => simple_name(node, source),
        "qualified_name" | "alias_qualified_name" => node
            .child_by_field_name("name")
            .and_then(|name| type_name(name, source)),
        "nullable_type" | "pointer_type" | "ref_type" | "scoped_type" => node
            .child_by_field_name("type")
            .and_then(|inner| type_name(inner, source)),
        _ => None,
    }
}

/// The written name of a class, struct, interface or delegate type without type arguments:
/// `Repo`, `Order.Builder` or `Acme.Data.Repo`. Predefined types, arrays, tuples and `var`
/// have none.
fn type_written(node: Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" | "generic_name" => type_name(node, source),
        "qualified_name" => {
            let qualifier = type_written(node.child_by_field_name("qualifier")?, source)?;
            let name = type_name(node.child_by_field_name("name")?, source)?;
            Some(format!("{qualifier}.{name}"))
        }
        "alias_qualified_name" => node
            .child_by_field_name("name")
            .and_then(|name| type_name(name, source)),
        "nullable_type" | "pointer_type" | "ref_type" | "scoped_type" => node
            .child_by_field_name("type")
            .and_then(|inner| type_written(inner, source)),
        _ => None,
    }
}

/// The types a file declares, by qualified name (`Order`, `Order::Builder`), for spelling
/// references to nested types the way their symbols are named.
struct FileTypes {
    declared: HashSet<String>,
    /// The declared types with type parameters (`Result<T>`), and those without (`Result`).
    generic: HashSet<String>,
    plain: HashSet<String>,
    /// Qualified names of the methods those types declare: `Order::Total`.
    methods: HashSet<String>,
    /// Segments of the namespaces the file declares or imports, and `using` aliases: a dotted
    /// name starting with one of them is namespace-qualified.
    namespaces: HashSet<String>,
    /// Roots of the namespaces the file declares.
    own_roots: HashSet<String>,
    /// Names of the top-level classes and records, the types that can hold tests.
    top_level_classes: HashSet<String>,
}

/// Namespace roots of the .NET libraries. A dotted name starting at one names a library type
/// (`System.Threading.Tasks.Task`) unless the file's own namespace starts there too.
const LIBRARY_ROOTS: &[&str] = &["System", "Microsoft"];

/// Containers whose children are declarations of the enclosing scope: namespaces, type bodies
/// and `#if`/`#elif`/`#else` blocks.
const DECLARATION_CONTAINERS: &[&str] = &[
    "compilation_unit",
    "namespace_declaration",
    "file_scoped_namespace_declaration",
    "declaration_list",
    "preproc_if",
    "preproc_elif",
    "preproc_else",
];

impl FileTypes {
    fn of(node: Node<'_>, source: &[u8]) -> Self {
        let mut root = node;
        while let Some(parent) = root.parent() {
            root = parent;
        }
        let mut types = Self {
            declared: HashSet::new(),
            generic: HashSet::new(),
            plain: HashSet::new(),
            methods: HashSet::new(),
            namespaces: HashSet::new(),
            own_roots: HashSet::new(),
            top_level_classes: HashSet::new(),
        };
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            match node.kind() {
                kind if TYPE_DECLARATIONS.contains(&kind) => {
                    if let Some(name) = named_child_text(node, "name", source)
                        && let Some(parents) = declaring_types(node, source)
                    {
                        if parents.is_empty()
                            && (kind == "class_declaration"
                                || (kind == "record_declaration" && !has_token(node, "struct")))
                        {
                            types.top_level_classes.insert(name.clone());
                        }
                        let qualified = qualify(&parents, &name);
                        let mut cursor = node.walk();
                        if node
                            .named_children(&mut cursor)
                            .any(|child| child.kind() == "type_parameter_list")
                        {
                            types.generic.insert(qualified.clone());
                        } else {
                            types.plain.insert(qualified.clone());
                        }
                        types.declared.insert(qualified);
                    }
                    if let Some(body) = node.child_by_field_name("body") {
                        stack.push(body);
                    }
                }
                // Member bodies never hold named types.
                kind if DECLARATION_CONTAINERS.contains(&kind) => {
                    if let Some(name) = node.child_by_field_name("name") {
                        let segments = namespace_segments(&node_text(name, source));
                        types.own_roots.extend(segments.first().cloned());
                        types.namespaces.extend(segments);
                    }
                    let mut cursor = node.walk();
                    stack.extend(node.named_children(&mut cursor));
                }
                "using_directive" => {
                    if let Some(alias) = node.child_by_field_name("name") {
                        types
                            .namespaces
                            .insert(node_text(alias, source).trim().to_owned());
                    }
                    if let Some(target) = using_target(node, source) {
                        types.namespaces.extend(namespace_segments(&target));
                    }
                }
                "method_declaration" => {
                    if let Some(name) = named_child_text(node, "name", source)
                        && let Some(parents) = declaring_types(node, source)
                    {
                        types.methods.insert(qualify(&parents, &name));
                    }
                }
                _ => {}
            }
        }
        types
    }

    /// The type an unqualified call `Name(...)` is spelled with: the innermost enclosing type
    /// that declares a method of that name, as C# resolves it (a nested class calling its
    /// outer class's static helper), else the immediately enclosing type.
    fn method_owner(&self, call: Node<'_>, name: &str, source: &[u8]) -> Option<String> {
        let mut immediate = None;
        let mut current = call.parent();
        while let Some(scope) = current {
            if TYPE_DECLARATIONS.contains(&scope.kind())
                && let Some(type_name) = named_child_text(scope, "name", source)
                && let Some(parents) = declaring_types(scope, source)
            {
                let owner = qualify(&parents, &type_name);
                if self.methods.contains(&format!("{owner}::{name}")) {
                    return Some(owner);
                }
                immediate.get_or_insert(owner);
            }
            current = scope.parent();
        }
        immediate
    }

    /// Spells a written type the way the parser qualifies C# types:
    /// - a .NET library type written in full keeps its dotted name
    ///   (`System.Threading.Tasks.Task`), which no workspace symbol has;
    /// - the longest trailing part of a dotted name that the file declares (`Order.Builder` →
    ///   `Order::Builder`), and a simple name that refers to a nested type with its declaring
    ///   types (`Builder` → `Order::Builder`);
    /// - a dotted name starting at a namespace the file declares or imports keeps its last
    ///   segment (`Acme.Data.Repo` → `Repo`); one starting anywhere else starts at a type
    ///   declared in another file and names its nested type (`Payment.Status` →
    ///   `Payment::Status`).
    fn spell(&self, written: &str, at: Node<'_>, source: &[u8]) -> String {
        let segments = written.split('.').collect::<Vec<_>>();
        if let [first, .., last] = segments.as_slice() {
            if LIBRARY_ROOTS.contains(first) && !self.own_roots.contains(*first) {
                return written.to_owned();
            }
            for start in 0..segments.len() {
                let candidate = segments[start..].join("::");
                if self.declared.contains(&candidate) {
                    return candidate;
                }
            }
            return if self.namespaces.contains(*first) || LIBRARY_ROOTS.contains(first) {
                (*last).to_owned()
            } else {
                segments.join("::")
            };
        }
        self.nested_type(written, at, source)
            .unwrap_or_else(|| written.to_owned())
    }

    /// The nested type a simple name refers to at `at`: a member type of an enclosing type,
    /// innermost first. Another type's nested types aren't in scope by simple name.
    fn nested_type(&self, simple: &str, at: Node<'_>, source: &[u8]) -> Option<String> {
        let mut current = at.parent();
        while let Some(scope) = current {
            if TYPE_DECLARATIONS.contains(&scope.kind())
                && let Some(name) = named_child_text(scope, "name", source)
                && let Some(parents) = declaring_types(scope, source)
            {
                let candidate = format!("{}::{simple}", qualify(&parents, &name));
                if self.declared.contains(&candidate) {
                    return Some(candidate);
                }
            }
            current = scope.parent();
        }
        None
    }
}

/// The identifier of a simple or generic name: `Save` for `Save` and `Save<T>`.
fn simple_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    let identifier = match node.kind() {
        "identifier" => node,
        "generic_name" => {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .find(|child| child.kind() == "identifier")?
        }
        _ => return None,
    };
    let name = node_text(identifier, source).trim().to_owned();
    (!name.is_empty()).then_some(name)
}

/// The segments of a namespace name: `Acme`, `Billing` for `Acme.Billing` or
/// `global::Acme.Billing`.
fn namespace_segments(name: &str) -> Vec<String> {
    let name = name.split_whitespace().collect::<String>();
    let name = name.strip_prefix("global::").unwrap_or(&name);
    name.split('.')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect()
}

fn using_target(node: Node<'_>, source: &[u8]) -> Option<String> {
    let alias = node.child_by_field_name("name");
    let mut cursor = node.walk();
    let target = node
        .named_children(&mut cursor)
        .filter(|child| Some(*child) != alias)
        .last()?;
    let text = node_text(target, source)
        .split_whitespace()
        .collect::<String>();
    (!text.is_empty()).then_some(text)
}

/// The entries of a type's base list, without the primary-constructor arguments.
fn base_types(declaration: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = declaration.walk();
    let Some(base_list) = declaration
        .named_children(&mut cursor)
        .find(|child| child.kind() == "base_list")
    else {
        return Vec::new();
    };
    let mut inner = base_list.walk();
    base_list
        .named_children(&mut inner)
        .filter_map(|entry| match entry.kind() {
            "argument_list" => None,
            "primary_constructor_base_type" => entry.child_by_field_name("type"),
            _ => Some(entry),
        })
        .collect()
}

/// Type parameters in scope at `node`: its own and those of every enclosing declaration.
fn type_parameter_names(node: Node<'_>, source: &[u8]) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut current = Some(node);
    while let Some(scope) = current {
        let mut cursor = scope.walk();
        for list in scope
            .named_children(&mut cursor)
            .filter(|child| child.kind() == "type_parameter_list")
        {
            let mut inner = list.walk();
            for parameter in list.named_children(&mut inner) {
                if let Some(name) = named_child_text(parameter, "name", source) {
                    names.insert(name);
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
    let type_parameters = type_parameter_names(declaration, source);
    let types = FileTypes::of(declaration, source);

    let mut targets = Vec::new();
    for field in ["returns", "parameters"] {
        if let Some(node) = declaration.child_by_field_name(field) {
            collect_type_refs(node, source, &type_parameters, &types, &mut targets);
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

/// Whether a written type, spelled as `spelled`, may be declared in the workspace: not a type
/// parameter in scope, not a .NET library type written in full, and not one of the listed base
/// class library types unless the file declares a type of that name and arity (`Task<int>` is
/// still the library's when the file declares a plain `Task`).
fn is_workspace_type(
    node: Node<'_>,
    written: &str,
    spelled: &str,
    type_parameters: &HashSet<String>,
    types: &FileTypes,
) -> bool {
    if !written.contains('.') && type_parameters.contains(written) {
        return false;
    }
    if spelled.contains('.') {
        return false;
    }
    let declared = if is_generic_use(node) {
        &types.generic
    } else {
        &types.plain
    };
    declared.contains(spelled) || !BCL_TYPES.contains(&spelled)
}

/// Whether a written type carries type arguments: `Task<int>`, `Acme.Result<T>`, `Task<int>?`.
fn is_generic_use(node: Node<'_>) -> bool {
    match node.kind() {
        "generic_name" => true,
        "qualified_name" | "alias_qualified_name" => {
            node.child_by_field_name("name").is_some_and(is_generic_use)
        }
        "nullable_type" | "pointer_type" | "ref_type" | "scoped_type" => {
            node.child_by_field_name("type").is_some_and(is_generic_use)
        }
        _ => false,
    }
}

fn collect_type_refs(
    node: Node<'_>,
    source: &[u8],
    type_parameters: &HashSet<String>,
    types: &FileTypes,
    out: &mut Vec<String>,
) {
    match node.kind() {
        "identifier" | "qualified_name" | "alias_qualified_name" | "generic_name" => {
            if let Some(written) = type_written(node, source) {
                let spelled = types.spell(&written, node, source);
                if is_workspace_type(node, &written, &spelled, type_parameters, types) {
                    out.push(spelled);
                }
            }
            // Type arguments name types of their own: `IList<Invoice>` references `Invoice`.
            let mut stack = vec![node];
            while let Some(current) = stack.pop() {
                let mut cursor = current.walk();
                for child in current.named_children(&mut cursor) {
                    if child.kind() == "type_argument_list" {
                        collect_type_refs(child, source, type_parameters, types, out);
                    } else if matches!(child.kind(), "qualified_name" | "generic_name") {
                        stack.push(child);
                    }
                }
            }
        }
        "parameter" | "tuple_element" => {
            if let Some(declared) = node.child_by_field_name("type") {
                collect_type_refs(declared, source, type_parameters, types, out);
            }
        }
        // A `params T[] items` array sits directly in the list: only its type is a type.
        "parameter_list" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() == "parameter" {
                    collect_type_refs(child, source, type_parameters, types, out);
                }
            }
            let mut cursor = node.walk();
            for declared in node.children_by_field_name("type", &mut cursor) {
                collect_type_refs(declared, source, type_parameters, types, out);
            }
        }
        // Attributes and default values are not part of a parameter's type.
        "attribute_list" | "predefined_type" => {}
        _ => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_type_refs(child, source, type_parameters, types, out);
            }
        }
    }
}

/// Base-list entries become edges from the declared type to each base class or interface.
/// C# syntax doesn't tell the two apart, so both count as `implements`.
fn implements_edges(
    declaration: Node<'_>,
    source: &[u8],
    file_path: &str,
    symbols: &[Symbol],
) -> Vec<SymbolEdge> {
    let Some(source_symbol) = symbol_for_node(symbols, declaration) else {
        return Vec::new();
    };
    let type_parameters = type_parameter_names(declaration, source);
    let types = FileTypes::of(declaration, source);

    let mut targets = base_types(declaration)
        .into_iter()
        .filter_map(|base| {
            let written = type_written(base, source)?;
            let spelled = types.spell(&written, declaration, source);
            is_workspace_type(base, &written, &spelled, &type_parameters, &types).then_some(spelled)
        })
        .collect::<Vec<_>>();
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

/// A declaration's attributes with their normalized names: `Fact` for `[Fact]`,
/// `[FactAttribute]` and `[Xunit.Fact]`.
fn attributes<'tree>(declaration: Node<'tree>, source: &[u8]) -> Vec<(String, Node<'tree>)> {
    let mut found = Vec::new();
    let mut cursor = declaration.walk();
    for list in declaration
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "attribute_list")
    {
        let mut inner = list.walk();
        for attribute in list
            .named_children(&mut inner)
            .filter(|child| child.kind() == "attribute")
        {
            let Some(name) = attribute
                .child_by_field_name("name")
                .and_then(|name| type_name(name, source))
            else {
                continue;
            };
            let name = name
                .strip_suffix("Attribute")
                .filter(|stripped| !stripped.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or(name);
            found.push((name, attribute));
        }
    }
    found
}

fn attribute_arguments(attribute: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = attribute.walk();
    let Some(list) = attribute
        .named_children(&mut cursor)
        .find(|child| child.kind() == "attribute_argument_list")
    else {
        return Vec::new();
    };
    let mut inner = list.walk();
    list.named_children(&mut inner)
        .filter(|child| child.kind() == "attribute_argument")
        .collect()
}

/// A string literal argument, with its name when it is passed as `Name = "..."`.
fn string_argument(argument: Node<'_>, source: &[u8]) -> Option<(Option<String>, String)> {
    let mut cursor = argument.walk();
    let children = argument.named_children(&mut cursor).collect::<Vec<_>>();
    let (name, value) = match children.as_slice() {
        // `DisplayName = "..."` parses as an assignment.
        [assignment] if assignment.kind() == "assignment_expression" => (
            assignment
                .child_by_field_name("left")
                .map(|name| node_text(name, source).trim().to_owned()),
            assignment.child_by_field_name("right")?,
        ),
        [value] => (None, *value),
        [name, value] if name.kind() == "identifier" => {
            (Some(node_text(*name, source).trim().to_owned()), *value)
        }
        _ => return None,
    };
    if !matches!(
        value.kind(),
        "string_literal" | "verbatim_string_literal" | "raw_string_literal"
    ) {
        return None;
    }
    let text = string_literal_text(value, source);
    let text = text.trim().to_owned();
    (!text.is_empty()).then_some((name, text))
}

/// The text of a string literal with its escapes decoded: `"a \"b\""`, `@"a ""b"""` and raw
/// `"""a "b" """` all read `a "b"`.
fn string_literal_text(literal: Node<'_>, source: &[u8]) -> String {
    match literal.kind() {
        "verbatim_string_literal" => {
            let text = node_text(literal, source);
            let inner = text
                .trim()
                .strip_prefix("@\"")
                .and_then(|rest| rest.strip_suffix('"'))
                .unwrap_or_default();
            inner.replace("\"\"", "\"")
        }
        "raw_string_literal" => {
            let mut cursor = literal.walk();
            literal
                .named_children(&mut cursor)
                .filter(|part| part.kind() == "raw_string_content")
                .map(|part| node_text(part, source))
                .collect()
        }
        _ => {
            let mut text = String::new();
            let mut cursor = literal.walk();
            for part in literal.named_children(&mut cursor) {
                match part.kind() {
                    "string_literal_content" => text.push_str(&node_text(part, source)),
                    "escape_sequence" => text.push_str(&decode_escape(&node_text(part, source))),
                    _ => {}
                }
            }
            text
        }
    }
}

fn positional_string_argument(attribute: Node<'_>, source: &[u8]) -> Option<String> {
    attribute_arguments(attribute)
        .into_iter()
        .filter_map(|argument| string_argument(argument, source))
        .find_map(|(name, text)| name.is_none().then_some(text))
}

/// The display text a test framework shows for a test: xUnit's `DisplayName = "..."`,
/// NUnit's `Description = "..."` or MSTest's `[TestMethod("...")]`.
fn display_name(attribute_name: &str, attribute: Node<'_>, source: &[u8]) -> Option<String> {
    let arguments = attribute_arguments(attribute)
        .into_iter()
        .filter_map(|argument| string_argument(argument, source))
        .collect::<Vec<_>>();
    arguments
        .iter()
        .find(|(name, _)| matches!(name.as_deref(), Some("DisplayName" | "Description")))
        .map(|(_, text)| text.clone())
        .or_else(|| {
            matches!(attribute_name, "TestMethod" | "DataTestMethod")
                .then(|| {
                    arguments
                        .iter()
                        .find(|(name, _)| name.is_none())
                        .map(|(_, text)| text.clone())
                })
                .flatten()
        })
}

/// The `<summary>` of the `///` XML doc comment directly above a declaration, or the whole
/// comment when it is plain text. Other tags (`<param>`, `<remarks>`) don't describe the test.
fn xml_doc_summary(node: Node<'_>, source: &[u8]) -> Option<String> {
    let mut lines = Vec::new();
    let mut next_row = node.start_position().row;
    let mut current = node.prev_sibling();
    while let Some(comment) = current {
        if comment.kind() != "comment" || comment.end_position().row + 1 != next_row {
            break;
        }
        // A comment that trails code on its own line belongs to that code.
        let trails_code = comment.prev_sibling().is_some_and(|previous| {
            previous.kind() != "comment"
                && previous.end_position().row == comment.start_position().row
        });
        let text = node_text(comment, source);
        let Some(line) = text.trim().strip_prefix("///").filter(|_| !trails_code) else {
            break;
        };
        lines.push(line.trim().to_owned());
        next_row = comment.start_position().row;
        current = comment.prev_sibling();
    }
    lines.reverse();
    let doc = lines.join(" ");
    let summary = match doc.split_once("<summary>") {
        Some((_, rest)) => rest.split("</summary>").next().unwrap_or(rest),
        None if doc.contains("</") || doc.contains("/>") => return None,
        None => doc.as_str(),
    };
    let normalized = collapse_whitespace(&strip_xml_tags(summary));
    (!normalized.is_empty()).then_some(normalized)
}

fn strip_xml_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

/// Decodes one escape sequence: `\u00e9` and `\U000000e9` → `é`, `\x41` → `A`, `\"` → `"`.
/// Whitespace and control escapes become a space.
fn decode_escape(escape: &str) -> String {
    let Some(body) = escape.strip_prefix('\\') else {
        return escape.to_owned();
    };
    let decoded = match body.chars().next() {
        Some('u' | 'U' | 'x') => u32::from_str_radix(&body[1..], 16)
            .ok()
            .and_then(char::from_u32),
        Some('0' | 'a' | 'b' | 'f' | 'n' | 'r' | 't' | 'v') => Some(' '),
        _ => body.chars().next(),
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

/// `Add_ReturnsSum_WhenBothPositive` becomes "add returns sum when both positive", and a
/// leading `Test` prefix is dropped.
fn humanize_test_name(name: &str) -> String {
    let trimmed = name
        .strip_prefix("Test")
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
    use super::{humanize_test_name, strip_xml_tags};

    #[test]
    fn humanizes_xunit_style_method_names() {
        assert_eq!(
            humanize_test_name("Add_ReturnsSum_WhenBothPositive"),
            "add returns sum when both positive"
        );
        assert_eq!(humanize_test_name("TestParseEmpty"), "parse empty");
        assert_eq!(humanize_test_name("Testing"), "testing");
    }

    #[test]
    fn strips_xml_doc_tags() {
        assert_eq!(
            strip_xml_tags("Uses <see cref=\"Repo\"/> to load."),
            "Uses  to load."
        );
    }
}
