use super::*;

#[test]
fn javascript_nested_function_calls_resolve_lexically_not_to_global_namesakes() {
    let source = r#"
function helper() {}
function outer() {
    helper();
    function helper() { helper(); }
    function sibling() { helper(); }
}
function other() { helper(); }
"#;
    let extracted = extract_with_edges(Language::JavaScript, "scope.mjs", source);
    assert_eq!(
        edge_targets_for_source(&extracted, "outer", EdgeKind::Calls),
        vec!["outer::helper"]
    );
    assert_eq!(
        edge_targets_for_source(&extracted, "outer::helper", EdgeKind::Calls),
        vec!["outer::helper"]
    );
    assert_eq!(
        edge_targets_for_source(&extracted, "outer::sibling", EdgeKind::Calls),
        vec!["outer::helper"]
    );
    assert_eq!(
        edge_targets_for_source(&extracted, "other", EdgeKind::Calls),
        vec!["helper"]
    );
    for edge in &extracted.edges {
        assert!(
            extracted
                .symbols
                .iter()
                .any(|s| s.qualified_name == edge.target_qualified_name)
        );
    }
}

#[test]
fn javascript_dynamic_bindings_do_not_link_to_outer_function_namesakes() {
    for body in [
        "function caller(helper) { helper(); }",
        "function caller({helper}) { helper(); }",
        "function caller(...helper) { helper(); }",
        "function caller(helper = fallback) { helper(); }",
        "function caller() { helper(); let helper = dynamic; }",
        "function caller() { const helper = dynamic; helper(); }",
        "function caller() { helper(); if (flag) { var helper = dynamic; } }",
        "function caller() { try {} catch(helper) { helper(); } }",
        "function caller() { { class helper {} helper(); } }",
        "function caller() { for (const helper of helpers) { helper(); } }",
        "function caller() { for (let helper in helpers) { helper(); } }",
        "function caller() { helper(); for (var helper of helpers) {} }",
    ] {
        let source = format!("function helper() {{}} {body}");
        let extracted = extract_with_edges(Language::JavaScript, "shadow.mjs", &source);
        assert!(
            !extracted
                .edges
                .iter()
                .any(|edge| edge.target_qualified_name == "helper"),
            "must not resolve dynamic shadow binding to global helper: {body}"
        );
    }
}

#[test]
fn javascript_switch_case_function_target_matches_qualified_declaration() {
    let extracted = extract_with_edges(
        Language::JavaScript,
        "switch.mjs",
        "function helper() {} function caller() { switch(flag) { case 0: function helper() {} helper(); break; } }",
    );
    assert_eq!(
        edge_targets_for_source(&extracted, "caller", EdgeKind::Calls),
        vec!["caller::$switch((flag))::$case(0)::helper"]
    );
}

#[test]
fn javascript_identifier_constructors_resolve_lexically() {
    let extracted = extract_with_edges(
        Language::JavaScript,
        "construct.mjs",
        "function Local() {} function outer() { function Local() {} new Local(); } function other() { new Local(); } function shadow(Local) { new Local(); }",
    );
    assert_eq!(
        edge_targets_for_source(&extracted, "outer", EdgeKind::Calls),
        vec!["outer::Local"]
    );
    assert_eq!(
        edge_targets_for_source(&extracted, "other", EdgeKind::Calls),
        vec!["Local"]
    );
    assert!(edge_targets_for_source(&extracted, "shadow", EdgeKind::Calls).is_empty());
}

#[test]
fn javascript_class_constructors_keep_static_targets_without_crossing_shadow_bindings() {
    let extracted = extract_with_edges(
        Language::JavaScript,
        "classes.mjs",
        "class Widget {} function caller() { new Widget(); } function outer() { class Widget {} new Widget(); } function shadow(Widget) { new Widget(); }",
    );
    assert_eq!(
        edge_targets_for_source(&extracted, "caller", EdgeKind::Calls),
        vec!["Widget"]
    );
    assert_eq!(
        edge_targets_for_source(&extracted, "outer", EdgeKind::Calls),
        vec!["outer::Widget"]
    );
    assert!(edge_targets_for_source(&extracted, "shadow", EdgeKind::Calls).is_empty());
}

#[test]
fn commonjs_assigned_object_methods_have_distinct_stable_owners() {
    let source = "exports.left = { run() { leftWork(); } }; exports.right = { run() { rightWork(); } }; function unrelated() {}";
    let first = extract_with_edges(Language::JavaScript, "exports.cjs", source);
    assert_eq!(first.symbols.len(), 3);
    assert_eq!(
        edge_targets_for_source(&first, "$assign(exports.left)::run", EdgeKind::Calls),
        vec!["leftWork"]
    );
    assert_eq!(
        edge_targets_for_source(&first, "$assign(exports.right)::run", EdgeKind::Calls),
        vec!["rightWork"]
    );
    let second = extract_with_edges(
        Language::JavaScript,
        "exports.cjs",
        &source
            .replace("exports.left", "exports /*note*/ . left")
            .replace("leftWork();", "leftWork(); moreWork();"),
    );
    for symbol in &first.symbols {
        assert!(second.symbols.iter().any(|next| next.id == symbol.id));
    }
}

#[test]
fn typescript_class_fields_own_nested_object_methods() {
    let source = "class C { left = { run() { leftWork(); } }; right = { run() { rightWork(); } }; } function unrelated() {}";
    let first = extract_with_edges(Language::TypeScript, "fields.ts", source);
    assert_eq!(first.symbols.len(), 4);
    assert_eq!(
        edge_targets_for_source(&first, "C::left::run", EdgeKind::Calls),
        vec!["leftWork"]
    );
    assert_eq!(
        edge_targets_for_source(&first, "C::right::run", EdgeKind::Calls),
        vec!["rightWork"]
    );
    let second = extract_with_edges(
        Language::TypeScript,
        "fields.ts",
        &source
            .replace("left =", "/*note*/ left =")
            .replace("leftWork();", "leftWork(); moreWork();"),
    );
    assert!(
        first
            .symbols
            .iter()
            .all(|symbol| second.symbols.iter().any(|next| next.id == symbol.id))
    );
}

#[test]
fn typescript_static_private_and_computed_fields_remain_distinct() {
    let source = r#"class C { static x={run(){}}; x={run(){}}; #x={run(){}}; ["a b"]={run(){}}; ["ab"]={run(){}}; }"#;
    let first = extract_with_edges(Language::TypeScript, "fields.ts", source);
    assert_eq!(first.symbols.len(), 6);
    let names = first
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect::<Vec<_>>();
    for expected in [
        "C::$static(x)::run",
        "C::x::run",
        "C::#x::run",
        "C::[\"a b\"]::run",
        "C::[\"ab\"]::run",
    ] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }
    let second = extract_with_edges(
        Language::TypeScript,
        "fields.ts",
        &source
            .replace("[\"a b\"]", "[ /* key */ \"a b\" ]")
            .replace("run(){}", "run(){ work(); }"),
    );
    // Class body hash changes, but declaration and contained method identities do not.
    assert!(
        first
            .symbols
            .iter()
            .all(|symbol| second.symbols.iter().any(|next| next.id == symbol.id))
    );
}

#[test]
fn repeated_same_assignment_owner_still_fails_closed_without_overwriting() {
    let mut extractor = SymbolExtractor::new().expect("extractor");
    let result = extractor.extract_from_source(
        Language::JavaScript,
        "repeat.cjs",
        "exports.same={run(){first();}}; exports.same={run(){second();}};",
    );
    assert!(
        result
            .err()
            .is_some_and(|error| error.downcast_ref::<DuplicateSymbolIdentity>().is_some())
    );
}

#[test]
fn typescript_static_and_accessor_method_owners_are_distinct() {
    let extracted = extract_with_edges(
        Language::TypeScript,
        "methods.ts",
        "class C { static make() { return {run(){staticWork();}}; } make() { return {run(){instanceWork();}}; } get value() { return {run(){getWork();}}; } set value(v) { const x={run(){setWork();}}; } }",
    );
    for (owner, target) in [
        ("C::$static(make)::$return::run", "staticWork"),
        ("C::make::$return::run", "instanceWork"),
        ("C::$get(value)::$return::run", "getWork"),
        ("C::$set(value)::x::run", "setWork"),
    ] {
        assert_eq!(
            edge_targets_for_source(&extracted, owner, EdgeKind::Calls),
            vec![target]
        );
    }
}

#[test]
fn quoted_property_owners_preserve_semantic_whitespace() {
    let source = r#"const handlers = { " a ": {run(){spaceWork();}}, a: {run(){plainWork();}}, ["a b"]: {run(){computedWork();}} };"#;
    let first = extract_with_edges(Language::JavaScript, "keys.mjs", source);
    assert_eq!(first.symbols.len(), 3);
    for (owner, target) in [
        ("handlers::\" a \"::run", "spaceWork"),
        ("handlers::a::run", "plainWork"),
        ("handlers::[\"a b\"]::run", "computedWork"),
    ] {
        assert_eq!(
            edge_targets_for_source(&first, owner, EdgeKind::Calls),
            vec![target]
        );
    }
    let second = extract_with_edges(
        Language::JavaScript,
        "keys.mjs",
        &source
            .replace("[\"a b\"]", "[ /*key*/ \"a b\" ]")
            .replace("spaceWork();", "spaceWork(); moreWork();"),
    );
    assert!(
        first
            .symbols
            .iter()
            .all(|symbol| second.symbols.iter().any(|next| next.id == symbol.id))
    );
}

#[test]
fn dynamic_computed_owner_expressions_are_syntactically_distinct_not_runtime_resolved() {
    let extracted = extract_with_edges(
        Language::JavaScript,
        "dynamic.mjs",
        "const table = { [keyA]: {run(){aWork();}}, [keyB]: {run(){bWork();}} };",
    );
    assert_eq!(
        edge_targets_for_source(&extracted, "table::[keyA]::run", EdgeKind::Calls),
        vec!["aWork"]
    );
    assert_eq!(
        edge_targets_for_source(&extracted, "table::[keyB]::run", EdgeKind::Calls),
        vec!["bWork"]
    );
}

#[test]
fn unsupported_ambiguous_owner_forms_are_explicitly_rejected() {
    for (language, path, source) in [
        (
            Language::JavaScript,
            "dynamic.mjs",
            "exports[key]={run(){first();}}; exports[key]={run(){second();}};",
        ),
        (
            Language::JavaScript,
            "blocks.mjs",
            "function outer(){ {const x={run(){first();}};} {const x={run(){second();}};} }",
        ),
        (
            Language::Tsx,
            "attrs.tsx",
            "const widget=<C left={{run(){one();}}} right={{run(){two();}}}/>;",
        ),
        (
            Language::TypeScript,
            "namespaces.ts",
            "namespace A { function work() {} } namespace B { function work() {} }",
        ),
    ] {
        let mut extractor = SymbolExtractor::new().expect("extractor");
        let result = extractor.extract_from_source(language, path, source);
        assert!(
            result
                .err()
                .is_some_and(|error| error.downcast_ref::<DuplicateSymbolIdentity>().is_some()),
            "must reject ambiguous identities rather than overwrite: {path}"
        );
    }
}

#[test]
fn generator_local_bindings_do_not_shadow_surrounding_callers() {
    for generator in [
        "function* iter() { var helper; }",
        "const iter = function* () { var helper; };",
        "async function* iter() { var helper; }",
    ] {
        let source =
            format!("function helper() {{}} function caller() {{ {generator} helper(); }}");
        let extracted = extract_with_edges(Language::JavaScript, "generator.mjs", &source);
        assert_eq!(
            edge_targets_for_source(&extracted, "caller", EdgeKind::Calls),
            vec!["helper"]
        );
    }
}

#[test]
fn conditional_object_argument_labels_exclude_nested_declaration_bodies() {
    let source = "send(enabled && {run(){work();}}); send(enabled && {run(){otherWork();}});";
    let first = extract_with_edges(Language::JavaScript, "conditional.mjs", source);
    assert_eq!(first.symbols.len(), 2);
    let second = extract_with_edges(
        Language::JavaScript,
        "conditional.mjs",
        &source.replace("work();", "work(); changedBody();"),
    );
    assert!(
        first
            .symbols
            .iter()
            .all(|symbol| second.symbols.iter().any(|next| next.id == symbol.id))
    );
    assert_eq!(
        second
            .edges
            .iter()
            .filter(|edge| ["work", "changedBody", "otherWork"]
                .contains(&edge.target_qualified_name.as_str()))
            .count(),
        3
    );
    for edge in second.edges.iter().filter(|edge| {
        ["work", "changedBody", "otherWork"].contains(&edge.target_qualified_name.as_str())
    }) {
        assert!(
            second
                .symbols
                .iter()
                .any(|symbol| symbol.id == edge.source_id)
        );
    }
}

#[test]
fn unindexed_generator_names_shadow_outer_function_namesakes() {
    for inner in [
        "function* helper() {} helper();",
        "const iter = function* helper() { helper(); };",
    ] {
        let source = format!("function helper() {{}} function outer() {{ {inner} }}");
        let extracted = extract_with_edges(Language::JavaScript, "generator.mjs", &source);
        assert!(
            !extracted
                .edges
                .iter()
                .any(|edge| edge.target_qualified_name == "helper")
        );
    }
}

#[test]
fn parameter_initializers_cannot_see_body_var_bindings() {
    let extracted = extract_with_edges(
        Language::JavaScript,
        "params.mjs",
        "function helper() {} function caller(arg=helper()) { var helper; helper(); }",
    );
    assert_eq!(
        edge_targets_for_source(&extracted, "caller", EdgeKind::Calls),
        vec!["helper"]
    );
    let extracted = extract_with_edges(
        Language::JavaScript,
        "params.mjs",
        "function helper() {} function caller(helper,arg=helper()) { var helper; }",
    );
    assert!(edge_targets_for_source(&extracted, "caller", EdgeKind::Calls).is_empty());
    let extracted = extract_with_edges(
        Language::JavaScript,
        "params.mjs",
        "function helper() {} function caller(arg=helper()) { function helper() {} helper(); }",
    );
    let targets = edge_targets_for_source(&extracted, "caller", EdgeKind::Calls);
    assert!(targets.contains(&"helper".to_owned()));
    assert!(targets.contains(&"caller::helper".to_owned()));
}

#[test]
fn static_initialization_var_bindings_do_not_shadow_outer_calls() {
    let extracted = extract_with_edges(
        Language::JavaScript,
        "static.mjs",
        "function helper() {} function caller() { class C { static { var helper; } } helper(); }",
    );
    assert_eq!(
        edge_targets_for_source(&extracted, "caller", EdgeKind::Calls),
        vec!["helper"]
    );
}

#[test]
fn nested_closures_still_see_enclosing_body_var_shadow() {
    let extracted = extract_with_edges(
        Language::JavaScript,
        "closure.mjs",
        "function helper() {} function caller() { var helper; function inner(){ helper(); } }",
    );
    assert!(edge_targets_for_source(&extracted, "caller::inner", EdgeKind::Calls).is_empty());
}

#[test]
fn default_parameter_bindings_own_their_object_methods() {
    let source = "function setup(left={run(){leftWork();}}, right={run(){rightWork();}}) {} function unrelated() {}";
    let first = extract_with_edges(Language::JavaScript, "defaults.mjs", source);
    assert_eq!(first.symbols.len(), 4);
    for (owner, target) in [
        ("setup::$param(left)::run", "leftWork"),
        ("setup::$param(right)::run", "rightWork"),
    ] {
        assert_eq!(
            edge_targets_for_source(&first, owner, EdgeKind::Calls),
            vec![target]
        );
    }
    let second = extract_with_edges(
        Language::JavaScript,
        "defaults.mjs",
        &source.replace("leftWork();", "leftWork(); changed();"),
    );
    for symbol in first
        .symbols
        .iter()
        .filter(|symbol| symbol.kind == SymbolKind::Method)
    {
        assert!(second.symbols.iter().any(|next| next.id == symbol.id));
    }
}

#[test]
fn destructured_parameter_defaults_have_distinct_binding_owners() {
    let extracted = extract_with_edges(
        Language::JavaScript,
        "defaults.mjs",
        "function setup({left={run(){leftWork();}}, right={run(){rightWork();}}}={}) {}",
    );
    assert_eq!(extracted.symbols.len(), 3);
    assert_eq!(
        edge_targets_for_source(&extracted, "setup::$default(left)::run", EdgeKind::Calls),
        vec!["leftWork"]
    );
    assert_eq!(
        edge_targets_for_source(&extracted, "setup::$default(right)::run", EdgeKind::Calls),
        vec!["rightWork"]
    );
}

#[test]
fn switch_factory_cases_and_default_own_returned_methods() {
    let source = "function factory(kind) { switch(kind) { case 'a': return {run(){first();}}; case 'b': return {run(){second();}}; default: return {run(){last();}}; } } function unrelated() {}";
    let first = extract_with_edges(Language::JavaScript, "switch.mjs", source);
    assert_eq!(first.symbols.len(), 5);
    let edges = first
        .edges
        .iter()
        .filter(|edge| ["first", "second", "last"].contains(&edge.target_qualified_name.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(edges.len(), 3);
    let owners = edges
        .iter()
        .map(|edge| edge.source_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(owners.len(), 3);
    assert!(edges.iter().all(|edge| {
        first
            .symbols
            .iter()
            .any(|symbol| symbol.id == edge.source_id)
    }));
    let second = extract_with_edges(
        Language::JavaScript,
        "switch.mjs",
        &source.replace("first();", "first(); edited();"),
    );
    assert!(
        first
            .symbols
            .iter()
            .all(|symbol| second.symbols.iter().any(|next| next.id == symbol.id))
    );
}

#[test]
fn audited_branch_assignment_loop_and_class_owners_are_distinct() {
    for source in [
        "function factory(){try{return {run(){one();}};}catch(e){return {run(){two();}};}finally{return {run(){three();}};}}",
        "exports.left ??= {run(){one();}}; exports.right ??= {run(){two();}};",
        "function factory(){for(const x of left){const value={run(){one();}};} for(const x of right){const value={run(){two();}};} while(ready){const value={run(){three();}};} do{const value={run(){four();}};}while(other);}",
        "(class Left {run(){one();}}, class Right {run(){two();}});",
    ] {
        let first = extract_with_edges(Language::JavaScript, "owners.mjs", source);
        let methods = first
            .symbols
            .iter()
            .filter(|symbol| symbol.kind == SymbolKind::Method)
            .collect::<Vec<_>>();
        assert_eq!(methods.len(), source.matches("run()").count(), "{source}");
        let ids = methods
            .iter()
            .map(|symbol| symbol.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), methods.len());
        let body_edges = first
            .edges
            .iter()
            .filter(|edge| {
                ["one", "two", "three", "four"].contains(&edge.target_qualified_name.as_str())
            })
            .collect::<Vec<_>>();
        assert_eq!(body_edges.len(), methods.len(), "{source}");
        for edge in body_edges {
            assert!(methods.iter().any(|symbol| symbol.id == edge.source_id));
        }
        let second = extract_with_edges(
            Language::JavaScript,
            "owners.mjs",
            &source.replace("one();", "one(); edited();"),
        );
        assert!(
            methods
                .iter()
                .all(|symbol| second.symbols.iter().any(|next| next.id == symbol.id)),
            "{source}"
        );
    }
}

#[test]
fn destructured_variable_owner_omits_nested_default_method_bodies() {
    let source = "const {left={run(){one();}}, right={run(){two();}}}=input;";
    let first = extract_with_edges(Language::JavaScript, "bindings.mjs", source);
    assert_eq!(first.symbols.len(), 2);
    let second = extract_with_edges(
        Language::JavaScript,
        "bindings.mjs",
        &source.replace("one();", "one(); edited();"),
    );
    assert!(
        first
            .symbols
            .iter()
            .all(|symbol| second.symbols.iter().any(|next| next.id == symbol.id))
    );
}

#[test]
fn named_class_expression_self_binding_does_not_resolve_outer_constructor() {
    let extracted = extract_with_edges(
        Language::JavaScript,
        "class.mjs",
        "function Local(){} const X=class Local {run(){new Local();}};",
    );
    assert!(
        !extracted
            .edges
            .iter()
            .any(|edge| edge.target_qualified_name == "Local")
    );
}
