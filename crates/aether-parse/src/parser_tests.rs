use super::*;

fn extract(language: Language, path: &str, source: &str) -> Vec<Symbol> {
    let mut extractor = SymbolExtractor::new().expect("extractor");
    extractor
        .extract_from_source(language, path, source)
        .expect("extract")
}

fn extract_with_edges(language: Language, path: &str, source: &str) -> ExtractedFile {
    let mut extractor = SymbolExtractor::new().expect("extractor");
    extractor
        .extract_with_edges_from_source(language, path, source)
        .expect("extract with edges")
}

fn edge_targets_for_source(
    extracted: &ExtractedFile,
    source_qualified_name: &str,
    edge_kind: EdgeKind,
) -> Vec<String> {
    let source_symbol = extracted
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == source_qualified_name)
        .unwrap_or_else(|| panic!("missing source symbol {source_qualified_name}"));
    let mut targets = extracted
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == edge_kind && edge.source_id == source_symbol.id)
        .map(|edge| edge.target_qualified_name.clone())
        .collect::<Vec<_>>();
    targets.sort();
    targets
}

#[test]
fn call_owner_literals_remain_distinct_and_formatting_independent() {
    let mut extractor = SymbolExtractor::new().expect("extractor");
    let first = "test('a b', () => { send({ dispose() { alpha(); } }); });";
    let second = "test('ab', () => { send({ dispose() { beta(); } }); });";
    let before = extractor
        .extract_with_edges_from_source(
            Language::JavaScript,
            "literal.mjs",
            &format!("{first}\n{second}"),
        )
        .expect("distinct literal owners");
    let after = extractor
        .extract_with_edges_from_source(
            Language::JavaScript,
            "literal.mjs",
            &format!("{second}\n{first}").replace("send(", "send ( "),
        )
        .expect("reordering unrelated named calls does not change owners");
    for name in ["alpha", "beta"] {
        let old = before
            .edges
            .iter()
            .find(|edge| edge.target_qualified_name == name)
            .expect("original edge");
        let new = after
            .edges
            .iter()
            .find(|edge| edge.target_qualified_name == name)
            .expect("reordered edge");
        assert_eq!(old.source_id, new.source_id);
    }
}

#[test]
fn expression_arrow_and_array_object_methods_keep_identity_and_call_edges() {
    let mut extractor = SymbolExtractor::new().expect("extractor");
    for source in [
        "const f = () => (send({ dispose() { alpha(); } }), send({ dispose() { beta(); } }));",
        "const f = () => [send({ dispose() { alpha(); } }), send({ dispose() { beta(); } })];",
        "const xs = [{ dispose() { alpha(); } }, { dispose() { beta(); } }];",
    ] {
        let before = extractor
            .extract_with_edges_from_source(Language::JavaScript, "anonymous.mjs", source)
            .expect("separate anonymous object declarations");
        assert_eq!(before.symbols.len(), 2);
        assert_ne!(before.symbols[0].id, before.symbols[1].id);
        let alpha = before
            .edges
            .iter()
            .find(|e| e.target_qualified_name == "alpha")
            .expect("alpha call");
        let beta = before
            .edges
            .iter()
            .find(|e| e.target_qualified_name == "beta")
            .expect("beta call");
        assert_ne!(alpha.source_id, beta.source_id);
        assert!(before.symbols.iter().any(|s| s.id == alpha.source_id));
        assert!(before.symbols.iter().any(|s| s.id == beta.source_id));
        let edited = format!(
            "// unrelated spacing\n{}",
            source
                .replace("alpha();", "alpha(); extra();")
                .replace("beta();", "beta(); extra();")
                .replace("send(", "send(/* argument comment */ ")
                .replace("[", "[/* leading element comment */ ")
                .replace(", { dispose", ", /* between elements */ { dispose")
        );
        let after = extractor
            .extract_with_edges_from_source(Language::JavaScript, "anonymous.mjs", &edited)
            .expect("body changes and line shifts");
        assert_eq!(
            before.symbols.iter().map(|s| &s.id).collect::<Vec<_>>(),
            after.symbols.iter().map(|s| &s.id).collect::<Vec<_>>()
        );
        for edge in [&alpha, &beta] {
            assert!(
                after
                    .edges
                    .iter()
                    .any(|candidate| candidate.source_id == edge.source_id
                        && candidate.target_qualified_name == edge.target_qualified_name)
            );
        }
    }
}

#[test]
fn repeated_anonymous_event_methods_keep_distinct_body_stable_ids() {
    let mut extractor = SymbolExtractor::new().expect("extractor");
    let source = "test('events', () => { submit({ preventDefault() {} }); submit({ preventDefault() {} }); });";
    let before = extractor
        .extract_from_source(Language::JavaScript, "events.mjs", source)
        .expect("repeated event objects are distinct declarations");
    assert_eq!(before.len(), 2);
    assert_ne!(before[0].id, before[1].id);
    let edited = source
        .replace("preventDefault() {}", "preventDefault() { report(); }")
        .replace("; submit", "; unrelated(); submit");
    let after = extractor
        .extract_from_source(Language::JavaScript, "events.mjs", &edited)
        .expect("body edit and unrelated statement preserve identities");
    assert_eq!(
        before.iter().map(|s| &s.id).collect::<Vec<_>>(),
        after.iter().map(|s| &s.id).collect::<Vec<_>>()
    );
}

#[test]
fn typescript_branch_and_callback_object_owners_are_distinct() {
    let mut extractor = SymbolExtractor::new().expect("extractor");
    let source = r#"
function factory(empty: boolean) {
  if (empty) return { dispose() {} };
  return { dispose() {} };
}
test('first', () => { const obj = { dispose() {} }; });
test('second', () => { const obj = { dispose() {} }; });
function install() {
  set('ResizeObserver', class { disconnect() {} });
  set('IntersectionObserver', class { disconnect() {} });
}
"#;
    let before = extractor
        .extract_from_source(Language::TypeScript, "owners.ts", source)
        .expect("distinct branches and callback owners");
    assert_eq!(
        before
            .iter()
            .filter(|s| s.qualified_name.ends_with("dispose"))
            .count(),
        4
    );
    assert_eq!(
        before
            .iter()
            .filter(|s| s.qualified_name.ends_with("disconnect"))
            .count(),
        2
    );
    let edited = source
        .replace("dispose() {}", "dispose() { release(); }")
        .replace("disconnect() {}", "disconnect() { release(); }");
    let after = extractor
        .extract_from_source(Language::TypeScript, "owners.ts", &edited)
        .expect("body changes retain owners");
    assert_eq!(
        before.iter().map(|s| &s.id).collect::<Vec<_>>(),
        after.iter().map(|s| &s.id).collect::<Vec<_>>()
    );
}

#[test]
fn module_extensions_route_to_the_same_ecmascript_parser() {
    let mut extractor = SymbolExtractor::new().expect("extractor");
    for (ext, language) in [
        ("mjs", Language::JavaScript),
        ("cjs", Language::JavaScript),
        ("mts", Language::TypeScript),
        ("cts", Language::TypeScript),
    ] {
        let path = format!("voice.{ext}");
        assert_eq!(language_for_path(Path::new(&path)), Some(language));
        let symbols = extractor
            .extract_from_path(Path::new(&path), "export class Voice { stop() {} }")
            .expect("module parses");
        assert!(symbols.iter().any(|s| s.qualified_name == "Voice::stop"));
        assert!(symbols.iter().all(|s| s.language == language));
    }
}

#[test]
fn typescript_factory_object_methods_have_distinct_stable_owners() {
    let mut extractor = SymbolExtractor::new().expect("extractor");
    let source = r#"
function makeEgg() { return { dispose() {}, setOpen(amount: number) {} }; }
function makePile() {
  const models: any[] = [];
  models.push({ dispose() {}, setOpen(amount: number) {} });
  return { dispose() {} };
}
const left = { dispose() {} };
const right = { dispose() {} };
"#;
    let symbols = extractor
        .extract_from_source(Language::TypeScript, "actors.ts", source)
        .expect("legal factory objects must index");
    let names: Vec<_> = symbols.iter().map(|s| s.qualified_name.as_str()).collect();
    for expected in [
        "makeEgg::$return::dispose",
        "makePile::$return::dispose",
        "makePile::$arg(models.push,0)::dispose",
        "left::dispose",
        "right::dispose",
    ] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }
    let edited = source.replace("dispose() {}", "dispose() { console.log(1); }");
    let after = extractor
        .extract_from_source(Language::TypeScript, "actors.ts", &edited)
        .expect("body edit");
    for old in &symbols {
        let new = after
            .iter()
            .find(|s| s.qualified_name == old.qualified_name)
            .expect("same declaration");
        assert_eq!(
            old.id, new.id,
            "body edit must preserve {}",
            old.qualified_name
        );
    }
}

#[test]
fn colliding_rust_trait_implementations_fail_instead_of_overwriting() {
    let mut extractor = SymbolExtractor::new().expect("extractor");
    let source = r#"
struct Worker;
trait Read { fn run(&self); }
trait Write { fn run(&self); }
impl Read for Worker { fn run(&self) {} }
impl Write for Worker { fn run(&self) {} }
"#;
    let error = extractor
        .extract_with_edges_from_source(Language::Rust, "src/lib.rs", source)
        .expect_err("trait identity collisions must not overwrite either method");
    assert!(error.to_string().contains("Worker::run"));
}

#[test]
fn inherent_and_trait_methods_with_identical_signatures_are_explicitly_unsupported() {
    let mut extractor = SymbolExtractor::new().expect("extractor");
    let source = "struct Worker; trait Run { fn run(&self); } impl Worker { fn run(&self) {} } impl Run for Worker { fn run(&self) {} }";
    let error = extractor
        .extract_from_source(Language::Rust, "src/lib.rs", source)
        .expect_err("legal inherent/trait collision must not overwrite either definition");
    assert!(error.to_string().contains("Worker::run"));
}

#[test]
fn qualified_generic_trait_implementations_are_explicitly_unsupported_when_colliding() {
    let mut extractor = SymbolExtractor::new().expect("extractor");
    let source = r#"
mod io { pub trait Read<T> { fn run(&self); } pub trait Write<T> { fn run(&self); } }
struct Worker<T>(T);
impl<T> io::Read<T> for Worker<T> { fn run(&self) {} }
impl<T> io::Write<T> for Worker<T> { fn run(&self) {} }
"#;
    let error = extractor
        .extract_from_source(Language::Rust, "src/lib.rs", source)
        .expect_err("qualified generic traits need distinct implementation identities");
    assert!(error.to_string().contains("duplicate symbol identity"));
    assert!(error.to_string().contains("run"));
}

#[test]
fn cfg_alternatives_with_colliding_identities_are_explicitly_unsupported() {
    let mut extractor = SymbolExtractor::new().expect("extractor");
    let source = "#[cfg(unix)] fn platform() {}\n#[cfg(windows)] fn platform() {}";
    let error = extractor
        .extract_with_edges_from_source(Language::Rust, "src/lib.rs", source)
        .expect_err("cfg identity ambiguity must be surfaced");
    assert!(error.to_string().contains("duplicate symbol identity"));
}

#[test]
fn duplicate_symbol_identities_are_rejected_before_indexing() {
    let mut extractor = SymbolExtractor::new().expect("extractor");
    let error = extractor
        .extract_with_edges_from_source(Language::Rust, "src/lib.rs", "fn same() {}\nfn same() {}")
        .expect_err("ambiguous identities must not overwrite each other");
    let message = error.to_string();
    assert!(message.contains("duplicate symbol identity in src/lib.rs"));
    assert!(message.contains("same"));
    assert!(message.contains("refusing to overwrite"));
}

#[test]
fn extracts_rust_symbols_and_qualified_methods() {
    let source = r#"
struct Widget;

type WidgetId = u64;

impl Widget {
    fn run(&self) -> bool {
        true
    }
}

fn helper() -> i32 {
    1
}
"#;

    let symbols = extract(Language::Rust, "src/lib.rs", source);
    let names: Vec<String> = symbols.iter().map(|s| s.qualified_name.clone()).collect();

    assert!(names.contains(&"Widget".to_owned()));
    assert!(names.contains(&"WidgetId".to_owned()));
    assert!(names.contains(&"Widget::run".to_owned()));
    assert!(names.contains(&"helper".to_owned()));
}

#[test]
fn extracts_typescript_symbols_and_methods() {
    let source = r#"
function topLevel(value: number): number {
  return value + 1;
}

class Greeter {
  greet(name: string): string {
    return `hello ${name}`;
  }
}

interface User {
  id: string;
}

type UserId = string;
"#;

    let symbols = extract(Language::TypeScript, "src/index.ts", source);
    let names: Vec<String> = symbols.iter().map(|s| s.qualified_name.clone()).collect();

    assert!(names.contains(&"topLevel".to_owned()));
    assert!(names.contains(&"Greeter".to_owned()));
    assert!(names.contains(&"Greeter::greet".to_owned()));
    assert!(names.contains(&"User".to_owned()));
    assert!(names.contains(&"UserId".to_owned()));
}

#[test]
fn extracts_rust_test_intents_from_names_and_doc_comments() {
    let source = r#"
#[test]
fn test_handles_negative_balance() {}

/// returns none for missing symbol
#[test]
fn lookup_missing_symbol_returns_none() {}

fn helper() {}
"#;

    let extracted = extract_with_edges(Language::Rust, "src/lib.rs", source);
    let intents = extracted
        .test_intents
        .iter()
        .map(|intent| intent.intent_text.clone())
        .collect::<Vec<_>>();
    assert!(intents.contains(&"handles negative balance".to_owned()));
    assert!(intents.contains(&"returns none for missing symbol".to_owned()));
    assert!(
        extracted
            .test_intents
            .iter()
            .all(|intent| intent.symbol_id.is_some())
    );
}

#[test]
fn extracts_typescript_test_intents_from_it_test_and_describe() {
    let source = r#"
describe("PaymentService", () => {
  it("handles negative balance", () => {});
  test("returns none for missing symbol", () => {});
});
"#;

    let extracted = extract_with_edges(Language::TypeScript, "src/payment.test.ts", source);
    let intents = extracted
        .test_intents
        .iter()
        .map(|intent| intent.intent_text.clone())
        .collect::<Vec<_>>();
    assert!(intents.contains(&"PaymentService".to_owned()));
    assert!(intents.contains(&"handles negative balance".to_owned()));
    assert!(intents.contains(&"returns none for missing symbol".to_owned()));
    assert!(
        extracted
            .test_intents
            .iter()
            .any(|intent| intent.group_label.as_deref() == Some("PaymentService"))
    );
}

#[test]
fn extracts_python_test_intents_from_names_and_docstrings() {
    let source = r#"
def test_handles_negative_balance():
    """handles negative balance"""
    assert True

def test_returns_none_for_missing_symbol():
    assert True

def helper():
    return 1
"#;

    let extracted = extract_with_edges(Language::Python, "tests/test_payments.py", source);
    let intents = extracted
        .test_intents
        .iter()
        .map(|intent| intent.intent_text.clone())
        .collect::<Vec<_>>();
    assert!(intents.contains(&"handles negative balance".to_owned()));
    assert!(intents.contains(&"returns none for missing symbol".to_owned()));
}

#[test]
fn extracts_rust_calls_edges() {
    let source = r#"
fn beta() {}

fn alpha() {
    beta();
}
"#;

    let extracted = extract_with_edges(Language::Rust, "src/lib.rs", source);
    let alpha = extracted
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "alpha")
        .expect("alpha symbol should exist");

    let calls = extracted
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].source_id, alpha.id);
    assert_eq!(calls[0].target_qualified_name, "beta");
    assert_eq!(calls[0].file_path, "src/lib.rs");
}

#[test]
fn extracts_rust_calls_edges_for_free_method_and_path_calls() {
    let source = r#"
struct Worker;

impl Worker {
    fn process(&self) {}
}

fn run_free() {}

fn example(obj: Worker) {
    run_free();
    obj.process();
    String::from("x");
}
"#;

    let extracted = extract_with_edges(Language::Rust, "src/lib.rs", source);
    let example = extracted
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "example")
        .expect("example symbol should exist");

    let targets = extracted
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_id == example.id)
        .map(|edge| edge.target_qualified_name.clone())
        .collect::<Vec<_>>();

    assert!(targets.iter().any(|target| target == "run_free"));
    assert!(targets.iter().any(|target| target == "Worker::process"));
    let process = extracted
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "Worker::process")
        .expect("process method");
    let method_call = extracted
        .edges
        .iter()
        .find(|edge| {
            edge.source_id == example.id && edge.target_qualified_name == "Worker::process"
        })
        .expect("nominal receiver proof");
    assert_eq!(
        method_call.target_symbol_id.as_deref(),
        Some(process.id.as_str())
    );
    let external_call = extracted
        .edges
        .iter()
        .find(|edge| edge.source_id == example.id && edge.target_qualified_name == "String::from")
        .expect("unresolved external call is retained");
    assert!(external_call.target_symbol_id.is_none() && external_call.target_file_path.is_none());
    assert!(targets.iter().any(|target| target == "String::from"));
}

#[test]
fn type_ref_extracted_from_function_parameter() {
    let source = r#"
struct MyStruct {}

fn foo(bar: MyStruct) {}
"#;

    let targets = edge_targets_for_source(
        &extract_with_edges(Language::Rust, "src/lib.rs", source),
        "foo",
        EdgeKind::TypeRef,
    );
    assert_eq!(targets, vec!["MyStruct".to_owned()]);
}

#[test]
fn type_ref_extracted_from_return_type() {
    let source = r#"
struct MyStruct {}

fn foo() -> MyStruct { todo!() }
"#;

    let targets = edge_targets_for_source(
        &extract_with_edges(Language::Rust, "src/lib.rs", source),
        "foo",
        EdgeKind::TypeRef,
    );
    assert_eq!(targets, vec!["MyStruct".to_owned()]);
}

#[test]
fn type_ref_extracted_from_generic() {
    let source = r#"
struct MyStruct {}

fn foo() -> Vec<MyStruct> { todo!() }
"#;

    let targets = edge_targets_for_source(
        &extract_with_edges(Language::Rust, "src/lib.rs", source),
        "foo",
        EdgeKind::TypeRef,
    );
    assert_eq!(targets, vec!["MyStruct".to_owned()]);
}

#[test]
fn type_ref_extracted_from_reference() {
    let source = r#"
struct MyStruct {}

fn foo(bar: &MyStruct) {}
"#;

    let targets = edge_targets_for_source(
        &extract_with_edges(Language::Rust, "src/lib.rs", source),
        "foo",
        EdgeKind::TypeRef,
    );
    assert_eq!(targets, vec!["MyStruct".to_owned()]);
}

#[test]
fn type_ref_extracted_to_enum() {
    let source = r#"
enum MyEnum { A, B }

fn foo(e: MyEnum) {}
"#;

    let targets = edge_targets_for_source(
        &extract_with_edges(Language::Rust, "src/lib.rs", source),
        "foo",
        EdgeKind::TypeRef,
    );
    assert_eq!(targets, vec!["MyEnum".to_owned()]);
}

#[test]
fn type_ref_extracted_to_type_alias() {
    let source = r#"
type MyAlias = u32;

fn foo(x: MyAlias) {}
"#;

    let targets = edge_targets_for_source(
        &extract_with_edges(Language::Rust, "src/lib.rs", source),
        "foo",
        EdgeKind::TypeRef,
    );
    assert_eq!(targets, vec!["MyAlias".to_owned()]);
}

#[test]
fn type_ref_extracted_to_trait_object() {
    let source = r#"
trait MyTrait {}

fn foo(t: &dyn MyTrait) {}
"#;

    let targets = edge_targets_for_source(
        &extract_with_edges(Language::Rust, "src/lib.rs", source),
        "foo",
        EdgeKind::TypeRef,
    );
    assert_eq!(targets, vec!["MyTrait".to_owned()]);
}

#[test]
fn type_ref_resolves_scoped_type() {
    let source = r#"
mod models {
    pub struct MyStruct {}
}

use self::models::MyStruct as ImportedStruct;

fn foo(value: ImportedStruct) {}
"#;

    let targets = edge_targets_for_source(
        &extract_with_edges(Language::Rust, "src/lib.rs", source),
        "foo",
        EdgeKind::TypeRef,
    );
    assert_eq!(targets, vec!["models::MyStruct".to_owned()]);
}

#[test]
fn type_ref_extracts_inner_from_nested_generic() {
    let source = r#"
struct MyStruct {}

fn foo() -> Option<Result<MyStruct, String>> { todo!() }
"#;

    let targets = edge_targets_for_source(
        &extract_with_edges(Language::Rust, "src/lib.rs", source),
        "foo",
        EdgeKind::TypeRef,
    );
    assert_eq!(targets, vec!["MyStruct".to_owned()]);
}

#[test]
fn type_ref_dedupes_repeated_same_type() {
    let source = r#"
struct MyType {}

fn foo(a: MyType, b: MyType) -> MyType { todo!() }
"#;

    let targets = edge_targets_for_source(
        &extract_with_edges(Language::Rust, "src/lib.rs", source),
        "foo",
        EdgeKind::TypeRef,
    );
    assert_eq!(targets, vec!["MyType".to_owned()]);
}

#[test]
fn type_ref_skips_stdlib_types() {
    let source = r#"
fn foo(s: String, n: u32) {}
"#;

    let extracted = extract_with_edges(Language::Rust, "src/lib.rs", source);
    assert!(
        extracted
            .edges
            .iter()
            .all(|edge| edge.edge_kind != EdgeKind::TypeRef)
    );
}

#[test]
fn type_ref_skips_unresolved_types() {
    let source = r#"
fn foo(x: SomeExternalType) {}
"#;

    let extracted = extract_with_edges(Language::Rust, "src/lib.rs", source);
    assert!(
        extracted
            .edges
            .iter()
            .all(|edge| edge.edge_kind != EdgeKind::TypeRef)
    );
}

#[test]
fn multiple_type_refs_from_one_function() {
    let source = r#"
struct A {}
struct B {}
struct C {}

fn foo(a: A, b: B) -> C { todo!() }
"#;

    let targets = edge_targets_for_source(
        &extract_with_edges(Language::Rust, "src/lib.rs", source),
        "foo",
        EdgeKind::TypeRef,
    );
    assert_eq!(
        targets,
        vec!["A".to_owned(), "B".to_owned(), "C".to_owned()]
    );
}

#[test]
fn type_ref_and_calls_coexist() {
    let source = r#"
struct Input {}

fn helper() {}

fn foo(value: Input) {
    helper();
}
"#;

    let extracted = extract_with_edges(Language::Rust, "src/lib.rs", source);
    let type_targets = edge_targets_for_source(&extracted, "foo", EdgeKind::TypeRef);
    let call_targets = edge_targets_for_source(&extracted, "foo", EdgeKind::Calls);

    assert_eq!(type_targets, vec!["Input".to_owned()]);
    assert_eq!(call_targets, vec!["helper".to_owned()]);
}

/// Macro arguments parse as token trees, so a call written inside one makes no edge.
/// The coverage counts in `aether_status` and `aether_dependencies` say so; if this
/// starts recording them, update that wording.
#[test]
fn rust_calls_inside_macro_arguments_make_no_call_edge() {
    let source = r#"
fn helper() -> i32 { 1 }
fn other() -> i32 { 2 }

fn foo() {
    helper();
    println!("{}", other());
    assert_eq!(other(), 2);
}
"#;

    let call_targets = edge_targets_for_source(
        &extract_with_edges(Language::Rust, "src/lib.rs", source),
        "foo",
        EdgeKind::Calls,
    );
    assert_eq!(call_targets, vec!["helper".to_owned()]);
}

#[test]
fn implements_extracted_from_impl_trait() {
    let source = r#"
trait Store {}
struct SqliteStore {}

impl Store for SqliteStore {}
"#;

    let targets = edge_targets_for_source(
        &extract_with_edges(Language::Rust, "src/lib.rs", source),
        "SqliteStore",
        EdgeKind::Implements,
    );
    assert_eq!(targets, vec!["Store".to_owned()]);
}

#[test]
fn implements_no_edge_for_bare_impl() {
    let source = r#"
struct Foo {}

impl Foo {
    fn bar(&self) {}
}
"#;

    let extracted = extract_with_edges(Language::Rust, "src/lib.rs", source);
    assert!(
        extracted
            .edges
            .iter()
            .all(|edge| edge.edge_kind != EdgeKind::Implements)
    );
}

#[test]
fn rust_trait_function_is_classified_as_method() {
    let source = r#"
trait Processor {
    fn process(&self) {}
}
"#;

    let symbols = extract(Language::Rust, "src/lib.rs", source);
    let method = symbols
        .iter()
        .find(|symbol| symbol.name == "process")
        .expect("trait method symbol should exist");
    assert_eq!(method.kind, SymbolKind::Method);
}

#[test]
fn rust_source_range_includes_byte_offsets() {
    let source = "fn alpha() {\n    beta();\n}\n";
    let symbols = extract(Language::Rust, "src/lib.rs", source);
    let alpha = symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "alpha")
        .expect("alpha symbol should exist");

    assert_eq!(alpha.range.start_byte, Some(0));
    assert_eq!(alpha.range.end_byte, Some(source.len() - 1));
}

#[test]
fn rust_call_node_diagnostics_prints_ast_and_call_kinds() {
    let source = r#"
fn process() {}

fn example(obj: String) {
    process();
    obj.process();
    String::from("x");
}
"#;

    let mut parser = Parser::new();
    parser
        .set_language(&rust_language())
        .expect("load rust language");
    let tree = parser.parse(source, None).expect("parse rust source");
    let root = tree.root_node();

    println!("RUST_DIAG_SEXP: {}", root.to_sexp());

    let mut kinds = Vec::new();
    let mut stack = vec![root];
    let source_bytes = source.as_bytes();
    while let Some(node) = stack.pop() {
        if node.kind().contains("call")
            && node.named_child_count() > 0
            && let Ok(text) = node.utf8_text(source_bytes)
        {
            let trimmed = text.trim().to_owned();
            if trimmed.contains("process()")
                || trimmed.contains("obj.process()")
                || trimmed.contains("String::from(\"x\")")
            {
                println!("RUST_DIAG_CALL kind={} text={}", node.kind(), trimmed);
                kinds.push((trimmed, node.kind().to_owned()));
            }
        }

        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }

    kinds.sort_by(|left, right| left.0.cmp(&right.0));
    assert!(kinds.iter().any(|(text, _)| text == "process()"));
    assert!(kinds.iter().any(|(text, _)| text == "obj.process()"));
    assert!(kinds.iter().any(|(text, _)| text == "String::from(\"x\")"));
}

#[test]
fn extracts_typescript_dependency_edges_from_imports() {
    let source = r#"
import { run } from "./dep";

function alpha() {
    return run();
}
"#;

    let extracted = extract_with_edges(Language::TypeScript, "src/app.ts", source);
    let depends = extracted
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::DependsOn)
        .collect::<Vec<_>>();

    assert_eq!(depends.len(), 1);
    assert_eq!(depends[0].source_id, file_source_id("src/app.ts"));
    assert_eq!(depends[0].target_qualified_name, "./dep");
    assert_eq!(depends[0].file_path, "src/app.ts");
}

#[test]
fn preserves_symbol_id_when_line_offsets_shift() {
    let source_a = "fn add(x: i32, y: i32) -> i32 { x + y }\n";
    let source_b = "\n\nfn add(x: i32, y: i32) -> i32 { x + y }\n";

    let symbols_a = extract(Language::Rust, "src/math.rs", source_a);
    let symbols_b = extract(Language::Rust, "src/math.rs", source_b);

    assert_eq!(symbols_a.len(), 1);
    assert_eq!(symbols_b.len(), 1);
    assert_eq!(symbols_a[0].id, symbols_b[0].id);
}

#[test]
fn changes_symbol_id_when_renamed() {
    let source_a = "fn add(x: i32, y: i32) -> i32 { x + y }\n";
    let source_b = "fn sum(x: i32, y: i32) -> i32 { x + y }\n";

    let symbols_a = extract(Language::Rust, "src/math.rs", source_a);
    let symbols_b = extract(Language::Rust, "src/math.rs", source_b);

    assert_eq!(symbols_a.len(), 1);
    assert_eq!(symbols_b.len(), 1);
    assert_ne!(symbols_a[0].id, symbols_b[0].id);
}

#[test]
fn rust_use_path_at_cursor_extracts_crate_path_and_cursor_segment() {
    let source = "use crate::config::loader;\n";
    let loader_col = source.find("loader").expect("loader segment");
    let path = rust_use_path_at_cursor(source, 0, loader_col).expect("rust use path");
    assert_eq!(path.prefix, RustUsePrefix::Crate);
    assert_eq!(
        path.segments,
        vec!["config".to_owned(), "loader".to_owned()]
    );
    assert_eq!(path.cursor_segment_index, Some(1));
}

#[test]
fn rust_use_path_at_cursor_returns_none_for_non_use_cursor() {
    let source = "fn alpha() {}\n";
    assert!(rust_use_path_at_cursor(source, 0, 3).is_none());
}

#[test]
fn rust_use_path_at_cursor_ignores_external_crate_imports() {
    let source = "use serde::Deserialize;\n";
    assert!(rust_use_path_at_cursor(source, 0, 8).is_none());
}

#[path = "parser_ecmascript_tests.rs"]
mod ecmascript;
