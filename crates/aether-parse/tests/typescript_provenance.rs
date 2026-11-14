use std::fs;
use std::path::Path;

use aether_core::EdgeKind;
use aether_parse::{ExtractedFile, SymbolExtractor};
use tempfile::TempDir;

fn write(root: &Path, path: &str, source: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    fs::write(path, source).expect("write source");
}

fn extract(extractor: &mut SymbolExtractor, root: &Path, path: &str) -> ExtractedFile {
    let source = fs::read_to_string(root.join(path)).expect("source");
    extractor
        .extract_with_edges_from_path(Path::new(path), &source)
        .expect("extract")
}

#[test]
fn local_functions_have_exact_ids_and_unknown_or_shadowed_calls_keep_raw_spelling() {
    let mut extractor = SymbolExtractor::new().expect("extractor");
    let file = extractor
        .extract_with_edges_from_path(
            Path::new("one.ts"),
            r#"
function helper() {}
function run() { helper(); unknown(); obj.helper(); }
function shadow(helper: () => void) { helper(); }
"#,
        )
        .expect("extract");
    let helper_id = &file
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "helper")
        .expect("helper")
        .id;
    let run_id = &file
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "run")
        .expect("run")
        .id;
    let shadow_id = &file
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "shadow")
        .expect("shadow")
        .id;
    let calls = file
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .collect::<Vec<_>>();
    let local = calls
        .iter()
        .find(|edge| &edge.source_id == run_id && edge.target_qualified_name == "helper")
        .expect("local call");
    assert_eq!(local.target_symbol_id.as_ref(), Some(helper_id));
    assert!(local.target_file_path.is_none());
    for edge in calls
        .iter()
        .filter(|edge| &edge.source_id == shadow_id || edge.target_qualified_name != "helper")
    {
        assert!(edge.target_symbol_id.is_none(), "{edge:?}");
        assert!(edge.target_file_path.is_none(), "{edge:?}");
    }
    assert!(
        calls
            .iter()
            .any(|edge| &edge.source_id == shadow_id && edge.target_qualified_name == "helper")
    );
}

#[test]
fn a_reassigned_local_function_is_not_a_proven_callee() {
    let mut extractor = SymbolExtractor::new().expect("extractor");
    for source in [
        "function helper() {} function run() { helper = external; helper(); }",
        "function run() { function helper() {} var helper = external; helper(); }",
        "function run() { function helper() {} ({helper} = external); helper(); }",
    ] {
        let file = extractor
            .extract_with_edges_from_path(Path::new("one.ts"), source)
            .expect("extract");
        let edge = file
            .edges
            .iter()
            .find(|edge| edge.edge_kind == EdgeKind::Calls)
            .expect("call");
        assert_eq!(edge.target_qualified_name, "helper", "{source}");
        assert!(edge.target_symbol_id.is_none(), "{source}");
        assert!(edge.target_file_path.is_none(), "{source}");
    }
}

#[test]
fn dynamic_with_objects_do_not_prove_an_outer_function_target() {
    let mut extractor = SymbolExtractor::new().expect("extractor");
    let file = extractor
        .extract_with_edges_from_path(
            Path::new("one.js"),
            "function helper() {} function run() { with (object) { helper(); } }",
        )
        .expect("extract");
    let edge = file
        .edges
        .iter()
        .find(|edge| edge.edge_kind == EdgeKind::Calls)
        .expect("raw call");
    assert_eq!(edge.target_qualified_name, "helper");
    assert!(edge.target_symbol_id.is_none());
    assert!(edge.target_file_path.is_none());
}

#[test]
fn explicit_named_imports_preserve_file_provenance_and_shadows_stay_unresolved() {
    let root = TempDir::new().expect("workspace");
    write(root.path(), "one/dep.ts", "export function helper() {}");
    write(root.path(), "two/dep.ts", "export function helper() {}");
    write(
        root.path(),
        "one/main.js",
        r#"
import {helper as imported} from './dep';
function run() { imported(); }
function shadow(imported) { imported(); }
function block() { let imported = callback; imported(); }
"#,
    );
    let mut extractor = SymbolExtractor::new()
        .expect("extractor")
        .with_workspace_root(root.path());
    let file = extract(&mut extractor, root.path(), "one/main.js");
    let run = file
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "run")
        .expect("run");
    let calls = file
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .collect::<Vec<_>>();
    let imported = calls
        .iter()
        .find(|edge| edge.source_id == run.id)
        .expect("imported call");
    assert_eq!(imported.target_qualified_name, "helper");
    assert_eq!(imported.target_file_path.as_deref(), Some("one/dep.ts"));
    assert!(imported.target_symbol_id.is_none());
    for edge in calls.iter().filter(|edge| edge.source_id != run.id) {
        assert_eq!(edge.target_qualified_name, "imported");
        assert!(edge.target_file_path.is_none());
        assert!(edge.target_symbol_id.is_none());
    }
}

#[test]
fn imported_target_edits_removal_and_ambiguous_candidates_invalidate_the_caller() {
    let root = TempDir::new().expect("workspace");
    write(root.path(), "dep.ts", "export function helper() {}");
    write(
        root.path(),
        "main.ts",
        "import {helper as imported} from './dep'; function run(){ imported(); }",
    );
    let mut extractor = SymbolExtractor::new()
        .expect("extractor")
        .with_workspace_root(root.path());
    assert_eq!(
        extract(&mut extractor, root.path(), "main.ts")
            .edges
            .iter()
            .find(|edge| edge.edge_kind == EdgeKind::Calls)
            .expect("call")
            .target_file_path
            .as_deref(),
        Some("dep.ts")
    );

    write(root.path(), "dep.ts", "function helper() {}");
    assert!(
        extractor
            .refresh_workspace_file("dep.ts")
            .contains(&"main.ts".to_owned())
    );
    assert!(
        extract(&mut extractor, root.path(), "main.ts")
            .edges
            .iter()
            .find(|edge| edge.edge_kind == EdgeKind::Calls)
            .expect("call")
            .target_file_path
            .is_none()
    );

    write(root.path(), "dep.ts", "export function helper() {}");
    extractor.refresh_workspace_file("dep.ts");
    extract(&mut extractor, root.path(), "main.ts");
    write(root.path(), "dep.js", "export function helper() {}");
    assert!(
        extractor
            .refresh_workspace_file("dep.js")
            .contains(&"main.ts".to_owned())
    );
    assert!(
        extract(&mut extractor, root.path(), "main.ts")
            .edges
            .iter()
            .find(|edge| edge.edge_kind == EdgeKind::Calls)
            .expect("call")
            .target_file_path
            .is_none()
    );

    fs::remove_file(root.path().join("dep.js")).expect("remove competitor");
    fs::remove_file(root.path().join("dep.ts")).expect("remove target");
    assert!(
        extractor
            .refresh_workspace_file("dep.ts")
            .contains(&"main.ts".to_owned())
    );
    let missing = extract(&mut extractor, root.path(), "main.ts");
    let edge = missing
        .edges
        .iter()
        .find(|edge| edge.edge_kind == EdgeKind::Calls)
        .expect("raw call");
    assert_eq!(edge.target_qualified_name, "imported");
    assert!(edge.target_file_path.is_none());
}

#[test]
fn unsupported_import_shapes_fail_closed() {
    let root = TempDir::new().expect("workspace");
    write(
        root.path(),
        "dep.ts",
        "function helper() {} export {helper};",
    );
    write(
        root.path(),
        "main.ts",
        r#"
import {helper as imported} from './dep';
import {missing} from './missing';
import {helper as packageHelper} from 'package';
function run(){ imported(); missing(); packageHelper(); }
"#,
    );
    let mut extractor = SymbolExtractor::new()
        .expect("extractor")
        .with_workspace_root(root.path());
    let file = extract(&mut extractor, root.path(), "main.ts");
    let calls = file
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 3);
    assert!(
        calls
            .iter()
            .all(|edge| edge.target_file_path.is_none() && edge.target_symbol_id.is_none())
    );
}

#[test]
fn commented_default_and_type_modifiers_do_not_prove_value_imports() {
    let root = TempDir::new().expect("workspace");
    for (declaration, import) in [
        (
            "export/*comment*/default function helper() {}",
            "import {helper} from './dep';",
        ),
        (
            "export/*comment*/default/*comment*/class helper {}",
            "import {helper} from './dep';",
        ),
        (
            "export function helper() {}",
            "import/*comment*/type {helper} from './dep';",
        ),
        (
            "export function helper() {}",
            "import {type/*comment*/helper} from './dep';",
        ),
        (
            "export function helper() {}",
            "import {type/*comment*/helper as alias} from './dep';",
        ),
    ] {
        write(root.path(), "dep.ts", declaration);
        write(
            root.path(),
            "main.ts",
            &format!("{import} function run(){{ helper(); alias(); }}"),
        );
        let mut extractor = SymbolExtractor::new()
            .expect("extractor")
            .with_workspace_root(root.path());
        let file = extract(&mut extractor, root.path(), "main.ts");
        let calls = file
            .edges
            .iter()
            .filter(|edge| edge.edge_kind == EdgeKind::Calls)
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 2, "{import} {declaration}");
        assert!(
            calls
                .iter()
                .all(|edge| edge.target_symbol_id.is_none() && edge.target_file_path.is_none()),
            "modifier must prevent value provenance: {import} {declaration}"
        );
    }
}

#[test]
fn comment_words_and_identifier_names_are_not_import_export_modifiers() {
    let root = TempDir::new().expect("workspace");
    write(
        root.path(),
        "dep.ts",
        "export /* default type */ function helper() {} export function type() {}",
    );
    write(
        root.path(),
        "main.ts",
        r#"
import/* type */ {helper /* type */ as imported, type as namedType} from './dep';
function run(){ imported(); namedType(); }
"#,
    );
    let mut extractor = SymbolExtractor::new()
        .expect("extractor")
        .with_workspace_root(root.path());
    let file = extract(&mut extractor, root.path(), "main.ts");
    let calls = file
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 2);
    for edge in calls {
        assert_eq!(edge.target_file_path.as_deref(), Some("dep.ts"), "{edge:?}");
        assert!(
            ["helper", "type"].contains(&edge.target_qualified_name.as_str()),
            "{edge:?}"
        );
    }
}

#[test]
fn explicit_directory_imports_consider_only_index_files() {
    let root = TempDir::new().expect("workspace");
    write(root.path(), "dep.ts", "export function helper() {}");
    write(root.path(), "dep/index.ts", "export function helper() {}");
    fs::create_dir_all(root.path().join("dep/nested")).expect("nested directory");
    for module in ["./dep/", "./dep/.", "./dep/nested/.."] {
        write(
            root.path(),
            "main.ts",
            &format!("import {{helper}} from '{module}'; function run(){{ helper(); }}"),
        );
        let mut extractor = SymbolExtractor::new()
            .expect("extractor")
            .with_workspace_root(root.path());
        let file = extract(&mut extractor, root.path(), "main.ts");
        let edge = file
            .edges
            .iter()
            .find(|edge| edge.edge_kind == EdgeKind::Calls)
            .expect("call");
        assert_eq!(
            edge.target_file_path.as_deref(),
            Some("dep/index.ts"),
            "{module}"
        );
        assert_eq!(edge.target_qualified_name, "helper");
    }
}

#[test]
fn explicit_directory_imports_never_fall_back_to_a_neighboring_stem_file() {
    let root = TempDir::new().expect("workspace");
    write(root.path(), "dep.ts", "export function helper() {}");
    fs::create_dir_all(root.path().join("dep/nested")).expect("nested directory");
    for module in ["./dep/", "./dep/.", "./dep/nested/.."] {
        write(
            root.path(),
            "main.ts",
            &format!("import {{helper}} from '{module}'; function run(){{ helper(); }}"),
        );
        let mut extractor = SymbolExtractor::new()
            .expect("extractor")
            .with_workspace_root(root.path());
        let file = extract(&mut extractor, root.path(), "main.ts");
        let edge = file
            .edges
            .iter()
            .find(|edge| edge.edge_kind == EdgeKind::Calls)
            .expect("raw call");
        assert_eq!(edge.target_qualified_name, "helper");
        assert!(edge.target_symbol_id.is_none(), "{module}");
        assert!(edge.target_file_path.is_none(), "{module}");
        write(root.path(), "dep/index.ts", "export function helper() {}");
        assert!(
            extractor
                .refresh_workspace_file("dep/index.ts")
                .contains(&"main.ts".to_owned()),
            "{module}"
        );
        fs::remove_file(root.path().join("dep/index.ts")).expect("remove index");
    }
}

#[test]
fn explicit_workspace_root_directory_import_has_a_relative_index_path() {
    let root = TempDir::new().expect("workspace");
    write(root.path(), "index.ts", "export function helper() {}");
    for (source_path, module) in [("main.ts", "./"), ("child/main.ts", "../")] {
        write(
            root.path(),
            source_path,
            &format!("import {{helper}} from '{module}'; function run(){{ helper(); }}"),
        );
        let mut extractor = SymbolExtractor::new()
            .expect("extractor")
            .with_workspace_root(root.path());
        let file = extract(&mut extractor, root.path(), source_path);
        let edge = file
            .edges
            .iter()
            .find(|edge| edge.edge_kind == EdgeKind::Calls)
            .expect("call");
        assert_eq!(
            edge.target_file_path.as_deref(),
            Some("index.ts"),
            "{source_path} {module}"
        );
    }
}

#[test]
fn module_content_quotes_are_not_stripped_as_string_delimiters() {
    let root = TempDir::new().expect("workspace");
    write(root.path(), "dep.ts", "export function helper() {}");
    for (specifier, module) in [
        (r#""./dep'""#, "./dep'"),
        (r#""'./dep'""#, "'./dep'"),
        (r#"'./dep"'"#, r#"./dep""#),
    ] {
        write(
            root.path(),
            "main.ts",
            &format!("import {{helper}} from {specifier}; function run(){{ helper(); }}"),
        );
        let mut extractor = SymbolExtractor::new()
            .expect("extractor")
            .with_workspace_root(root.path());
        let file = extract(&mut extractor, root.path(), "main.ts");
        let dependency = file
            .edges
            .iter()
            .find(|edge| edge.edge_kind == EdgeKind::DependsOn)
            .expect("raw import");
        assert_eq!(dependency.target_qualified_name, module, "{specifier}");
        let edge = file
            .edges
            .iter()
            .find(|edge| edge.edge_kind == EdgeKind::Calls)
            .expect("raw call");
        assert_eq!(edge.target_qualified_name, "helper");
        assert!(edge.target_symbol_id.is_none(), "{specifier}");
        assert!(edge.target_file_path.is_none(), "{specifier}");
    }
}

#[test]
fn a_quote_in_an_actual_module_filename_retains_its_provenance() {
    let root = TempDir::new().expect("workspace");
    write(root.path(), "dep.ts", "export function helper() {}");
    write(root.path(), "dep'.ts", "export function helper() {}");
    write(
        root.path(),
        "main.ts",
        r#"import {helper} from "./dep'"; function run(){ helper(); }"#,
    );
    let mut extractor = SymbolExtractor::new()
        .expect("extractor")
        .with_workspace_root(root.path());
    let file = extract(&mut extractor, root.path(), "main.ts");
    let edge = file
        .edges
        .iter()
        .find(|edge| edge.edge_kind == EdgeKind::Calls)
        .expect("call");
    assert_eq!(edge.target_file_path.as_deref(), Some("dep'.ts"));
    assert_eq!(edge.target_qualified_name, "helper");
}

#[test]
fn erroneous_source_preserves_raw_references_without_local_or_imported_proof() {
    let root = TempDir::new().expect("workspace");
    write(root.path(), "dep.ts", "export function helper() {}");
    let source = r#"
import {helper as imported} from './dep';
function helper() {}
function run() { function local() {} helper(); local(); imported(); }
const broken = ;
"#;
    for path in ["main.ts", "main.js"] {
        let mut parser = tree_sitter::Parser::new();
        let grammar = if path.ends_with(".ts") {
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
        } else {
            tree_sitter_typescript::LANGUAGE_TSX.into()
        };
        parser.set_language(&grammar).expect("grammar");
        let tree = parser.parse(source, None).expect("syntax tree");
        assert!(
            tree.root_node().has_error(),
            "fixture must exercise grammar recovery: {path}"
        );
        write(root.path(), path, source);
        let mut extractor = SymbolExtractor::new()
            .expect("extractor")
            .with_workspace_root(root.path());
        let file = extract(&mut extractor, root.path(), path);
        assert!(
            file.symbols
                .iter()
                .any(|symbol| symbol.qualified_name == "helper")
        );
        assert!(
            file.symbols
                .iter()
                .any(|symbol| symbol.qualified_name == "run::local")
        );
        let calls = file
            .edges
            .iter()
            .filter(|edge| edge.edge_kind == EdgeKind::Calls)
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 3, "{path}");
        for target in ["helper", "local", "imported"] {
            assert!(
                calls
                    .iter()
                    .any(|edge| edge.target_qualified_name == target),
                "{path} {target}"
            );
        }
        for edge in calls {
            assert!(edge.target_symbol_id.is_none(), "{path} {edge:?}");
            assert!(edge.target_file_path.is_none(), "{path} {edge:?}");
        }
    }
}

#[test]
fn direct_eval_withholds_local_proof_including_erased_callee_wrappers() {
    let callees = [
        "eval",
        "(eval)",
        "((eval))",
        "(/* comment */ eval)",
        "eval!",
        "(eval as any)",
        "(eval satisfies any)",
        "(<any>eval)",
        "eval<any>",
        "(eval<any>)",
        "((eval<any>))",
        r"\u0065val",
    ];
    for callee in callees {
        let source = format!(
            "function helper() {{ return 1; }} function run() {{ helper(); {callee}(\"helper = () => 2\"); }}"
        );
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
            .expect("grammar");
        assert!(
            !parser
                .parse(&source, None)
                .expect("tree")
                .root_node()
                .has_error(),
            "{callee}"
        );
        let mut extractor = SymbolExtractor::new().expect("extractor");
        let file = extractor
            .extract_with_edges_from_path(Path::new("eval.ts"), &source)
            .expect("extract");
        let helper = file
            .edges
            .iter()
            .find(|edge| {
                edge.edge_kind == EdgeKind::Calls && edge.target_qualified_name == "helper"
            })
            .expect("raw helper call");
        assert!(helper.target_symbol_id.is_none(), "{callee}: {helper:?}");
        assert!(helper.target_file_path.is_none(), "{callee}: {helper:?}");
    }
}

#[test]
fn direct_eval_in_a_target_module_withholds_export_proof_and_tracks_the_caller() {
    let root = TempDir::new().expect("workspace");
    write(
        root.path(),
        "main.ts",
        "import {helper as imported} from './dep'; function run(){ imported(); }",
    );
    for callee in [
        "eval",
        "((eval))",
        "eval!",
        "(eval as any)",
        "(<any>eval)",
        r"\u0065val",
    ] {
        write(
            root.path(),
            "dep.ts",
            &format!(
                "export function helper(){{return 1;}} function mutate(){{ {callee}(\"helper = () => 2\"); }} mutate();"
            ),
        );
        let mut extractor = SymbolExtractor::new()
            .expect("extractor")
            .with_workspace_root(root.path());
        let file = extract(&mut extractor, root.path(), "main.ts");
        let call = file
            .edges
            .iter()
            .find(|edge| edge.edge_kind == EdgeKind::Calls)
            .expect("raw imported call");
        assert_eq!(call.target_qualified_name, "imported", "{callee}");
        assert!(call.target_symbol_id.is_none(), "{callee}");
        assert!(call.target_file_path.is_none(), "{callee}");
        assert!(
            extractor
                .refresh_workspace_file("dep.ts")
                .contains(&"main.ts".to_owned()),
            "{callee}"
        );
    }
}

#[test]
fn a_source_with_direct_eval_retains_its_import_dependency_candidates() {
    let root = TempDir::new().expect("workspace");
    write(root.path(), "dep.ts", "export function helper(){}");
    write(
        root.path(),
        "main.ts",
        r#"import {helper as imported} from './dep'; function run(){imported(); eval("anything");}"#,
    );
    let mut extractor = SymbolExtractor::new()
        .expect("extractor")
        .with_workspace_root(root.path());
    let file = extract(&mut extractor, root.path(), "main.ts");
    assert!(
        file.edges
            .iter()
            .filter(|edge| edge.edge_kind == EdgeKind::Calls)
            .all(|edge| edge.target_symbol_id.is_none() && edge.target_file_path.is_none())
    );
    assert!(
        extractor
            .refresh_workspace_file("dep.ts")
            .contains(&"main.ts".to_owned())
    );
}

#[test]
fn member_eval_calls_and_eval_text_do_not_hide_unrelated_local_or_imported_proof() {
    let root = TempDir::new().expect("workspace");
    write(
        root.path(),
        "dep.ts",
        r#"
export function helper(){return 1;}
const text = "eval('helper = replacement')";
/* eval("helper = replacement") */
function unrelated(){ object.eval("helper = replacement"); }
"#,
    );
    write(
        root.path(),
        "main.ts",
        r#"
import {helper as imported} from './dep';
function local(){}
function run(){local(); imported(); object.eval("local = replacement");}
const text = "eval('local = replacement')";
// eval("local = replacement")
"#,
    );
    let mut extractor = SymbolExtractor::new()
        .expect("extractor")
        .with_workspace_root(root.path());
    let file = extract(&mut extractor, root.path(), "main.ts");
    let local = file
        .edges
        .iter()
        .find(|edge| edge.edge_kind == EdgeKind::Calls && edge.target_qualified_name == "local")
        .expect("local call");
    assert!(local.target_symbol_id.is_some());
    let imported = file
        .edges
        .iter()
        .find(|edge| edge.edge_kind == EdgeKind::Calls && edge.target_qualified_name == "helper")
        .expect("imported call");
    assert_eq!(imported.target_file_path.as_deref(), Some("dep.ts"));
}
