use std::collections::HashMap;
use std::path::Path;

use aether_core::{EdgeKind, Language, SymbolKind, file_source_id};
use aether_parse::{ExtractedFile, SymbolExtractor, language_for_path};

const BASIC_PATH: &str = "src/main/java/com/acme/store/Inventory.java";
const TEST_PATH: &str = "src/test/java/com/acme/store/InventoryTest.java";

fn extract_with_edges(path: &str, source: &str) -> ExtractedFile {
    let mut extractor = SymbolExtractor::new().expect("extractor");
    extractor
        .extract_with_edges_from_source(Language::Java, path, source)
        .expect("java extraction")
}

fn targets_from(extracted: &ExtractedFile, qualified_name: &str, kind: EdgeKind) -> Vec<String> {
    let source = extracted
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == qualified_name)
        .unwrap_or_else(|| panic!("missing symbol {qualified_name}"));
    let mut targets = extracted
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == kind && edge.source_id == source.id)
        .map(|edge| edge.target_qualified_name.clone())
        .collect::<Vec<_>>();
    targets.sort();
    targets
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn java_files_map_to_the_java_language() {
    assert_eq!(
        language_for_path(Path::new("src/main/java/App.java")),
        Some(Language::Java)
    );

    let mut extractor = SymbolExtractor::new().expect("extractor");
    let extracted = extractor
        .extract_with_edges_from_path(
            Path::new(BASIC_PATH),
            include_str!("fixtures/java_basic.java"),
        )
        .expect("extract by path");
    assert!(
        extracted
            .symbols
            .iter()
            .all(|symbol| symbol.language == Language::Java)
    );
}

#[test]
fn extracts_java_symbols_with_expected_kinds_and_qualified_names() {
    let extracted = extract_with_edges(BASIC_PATH, include_str!("fixtures/java_basic.java"));

    let kinds = extracted
        .symbols
        .iter()
        .map(|symbol| (symbol.qualified_name.as_str(), symbol.kind))
        .collect::<HashMap<_, _>>();
    let expected = [
        ("Inventory", SymbolKind::Class),
        ("Inventory::Inventory", SymbolKind::Method),
        ("Inventory::find", SymbolKind::Method),
        ("Inventory::load", SymbolKind::Method),
        ("Inventory::trace", SymbolKind::Method),
        ("Inventory::all", SymbolKind::Method),
        ("Inventory::Visitor", SymbolKind::Class),
        ("Inventory::Visitor::visitAll", SymbolKind::Method),
        ("Inventory::close", SymbolKind::Method),
        ("Repository", SymbolKind::Interface),
        ("Repository::exists", SymbolKind::Method),
        ("Status", SymbolKind::Enum),
        ("Status::label", SymbolKind::Method),
        ("Item", SymbolKind::Class),
        ("Item::from", SymbolKind::Method),
        ("Audited", SymbolKind::Interface),
    ];
    for (qualified_name, kind) in expected {
        assert_eq!(
            kinds.get(qualified_name),
            Some(&kind),
            "unexpected kind for {qualified_name}"
        );
    }
    // Abstract and interface methods have no body of their own, and the anonymous class's
    // `run` can't be named from outside.
    assert_eq!(extracted.symbols.len(), expected.len(), "{kinds:?}");
}

#[test]
fn java_symbol_ids_are_stable_with_line_offset_changes() {
    let source = include_str!("fixtures/java_basic.java");
    let ids = |extracted: &ExtractedFile| {
        extracted
            .symbols
            .iter()
            .map(|symbol| (symbol.qualified_name.clone(), symbol.id.clone()))
            .collect::<HashMap<_, _>>()
    };

    let original = ids(&extract_with_edges(BASIC_PATH, source));
    let shifted = ids(&extract_with_edges(BASIC_PATH, &format!("\n\n{source}")));
    assert_eq!(original, shifted);
}

#[test]
fn extracts_java_import_edges_from_the_file() {
    let extracted = extract_with_edges(BASIC_PATH, include_str!("fixtures/java_basic.java"));
    let mut imports = extracted
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::DependsOn)
        .inspect(|edge| assert_eq!(edge.source_id, file_source_id(BASIC_PATH)))
        .map(|edge| edge.target_qualified_name.clone())
        .collect::<Vec<_>>();
    imports.sort();
    assert_eq!(
        imports,
        strings(&[
            "com.acme.db.Connection",
            "com.acme.util.Checks.requireKey",
            "java.util.List",
            "java.util.Optional",
            "org.slf4j.Logger",
        ])
    );
}

#[test]
fn spells_java_call_targets_like_java_qualified_names() {
    let extracted = extract_with_edges(BASIC_PATH, include_str!("fixtures/java_basic.java"));

    // `super(...)` is a constructor chain, not a method call.
    assert!(targets_from(&extracted, "Inventory::Inventory", EdgeKind::Calls).is_empty());
    // A static import, a field's declared type, an unqualified call to the enclosing class,
    // and a capitalized type name that the JDK provides, so it keeps its full name.
    assert_eq!(
        targets_from(&extracted, "Inventory::find", EdgeKind::Calls),
        strings(&[
            "Cache::lookup",
            "Checks::requireKey",
            "Inventory::load",
            "java.util.Optional::of",
            "java.util.Optional::ofNullable",
        ])
    );
    // A type imported from another project keeps its full name, so it can't resolve to a
    // workspace class that happens to share the simple name.
    assert_eq!(
        targets_from(&extracted, "Inventory::trace", EdgeKind::Calls),
        strings(&["Item::describe", "org.slf4j.Logger::info"])
    );
    // `var` takes its type from `new`, `this.field` and `super.` resolve, and constructors
    // call the created type.
    assert_eq!(
        targets_from(&extracted, "Inventory::load", EdgeKind::Calls),
        strings(&[
            "BaseStore::audit",
            "Connection::close",
            "Connection::fetch",
            "Item::from",
            "Row",
            "Row::validate",
        ])
    );
    // Calls on another call's result keep the bare method name.
    assert_eq!(
        targets_from(&extracted, "Inventory::all", EdgeKind::Calls),
        strings(&["Inventory::items", "filter", "stream", "toList"])
    );
    // Unqualified calls in a nested class belong to the class that declares the method;
    // enhanced-for variables resolve.
    assert_eq!(
        targets_from(&extracted, "Inventory::Visitor::visitAll", EdgeKind::Calls),
        strings(&["Inventory::Visitor::visit", "Item::describe"])
    );
    // Calls inside an anonymous class belong to the enclosing method, and the outer field
    // is still visible there.
    assert_eq!(
        targets_from(&extracted, "Inventory::close", EdgeKind::Calls),
        strings(&["Connection::close", "Runnable", "Runnable::run"])
    );
    assert_eq!(
        targets_from(&extracted, "Repository::exists", EdgeKind::Calls),
        strings(&["Repository::find", "isPresent"])
    );
    assert_eq!(
        targets_from(&extracted, "Item::from", EdgeKind::Calls),
        strings(&["Item", "Price::zero", "Row::key"])
    );
}

#[test]
fn extracts_java_type_refs_from_signatures() {
    let extracted = extract_with_edges(BASIC_PATH, include_str!("fixtures/java_basic.java"));

    assert_eq!(
        targets_from(&extracted, "Inventory::Inventory", EdgeKind::TypeRef),
        strings(&["Connection"])
    );
    // `Optional` and `List` come from the JDK and `String` from java.lang.
    assert_eq!(
        targets_from(&extracted, "Inventory::find", EdgeKind::TypeRef),
        strings(&["Item"])
    );
    assert_eq!(
        targets_from(&extracted, "Inventory::all", EdgeKind::TypeRef),
        strings(&["Filter", "Item"])
    );
    // `Logger` comes from another project.
    assert_eq!(
        targets_from(&extracted, "Inventory::trace", EdgeKind::TypeRef),
        strings(&["Item"])
    );
    assert_eq!(
        targets_from(&extracted, "Item::from", EdgeKind::TypeRef),
        strings(&["Item", "Row"])
    );
    assert!(targets_from(&extracted, "Repository::exists", EdgeKind::TypeRef).is_empty());
}

#[test]
fn extracts_java_supertypes_as_implements_edges() {
    let extracted = extract_with_edges(BASIC_PATH, include_str!("fixtures/java_basic.java"));

    // `AutoCloseable` is a java.lang type.
    assert_eq!(
        targets_from(&extracted, "Inventory", EdgeKind::Implements),
        strings(&["BaseStore", "Repository"])
    );
    assert_eq!(
        targets_from(&extracted, "Status", EdgeKind::Implements),
        strings(&["Labeled"])
    );
    assert!(targets_from(&extracted, "Item", EdgeKind::Implements).is_empty());
}

#[test]
fn extracts_junit_test_intents() {
    let extracted = extract_with_edges(TEST_PATH, include_str!("fixtures/java_tests.java"));

    let intents = extracted
        .test_intents
        .iter()
        .map(|intent| {
            (
                intent.test_name.as_str(),
                intent.intent_text.as_str(),
                intent.group_label.as_deref(),
            )
        })
        .collect::<Vec<_>>();
    // Nested classes reuse method names, so a nested test is named by its path.
    assert_eq!(
        intents,
        vec![
            (
                "WhenEmpty::returnsNothing",
                "returns nothing",
                Some("WhenEmpty")
            ),
            (
                "WhenFull::returnsNothing",
                "returns nothing",
                Some("WhenFull")
            ),
            ("cafeMenu", "caf\u{e9} menu", Some("Inventory lookups")),
            (
                "findsStoredItem",
                "finds stored item",
                Some("Inventory lookups")
            ),
            (
                "legacyStyleWorks",
                "legacy style works",
                Some("Inventory lookups")
            ),
            ("parsesNull", "parses \"null\"", Some("Inventory lookups")),
            (
                "rejectsBlank",
                "rejects blank keys",
                Some("Inventory lookups")
            ),
            (
                "testFind_missingKey",
                "Missing keys come back empty.",
                Some("Inventory lookups")
            ),
        ]
    );

    for intent in &extracted.test_intents {
        assert_eq!(intent.language, Language::Java);
        let symbol = extracted
            .symbols
            .iter()
            .find(|symbol| Some(&symbol.id) == intent.symbol_id.as_ref())
            .expect("intent points at its test method");
        assert!(intent.test_name.ends_with(&symbol.name), "{intent:?}");
    }

    // A test method's calls resolve through the test class's fields.
    assert_eq!(
        targets_from(
            &extracted,
            "InventoryTest::findsStoredItem",
            EdgeKind::Calls
        ),
        strings(&["Inventory::find"])
    );
}

#[test]
fn java_extraction_tolerates_syntax_errors() {
    let broken = [
        "class Broken extends {",
        "class A { void m( { } }",
        "class A { @Test void }",
        "import static ;\nclass A {}",
        "class A { void m() { new (); x.; } }",
        "interface I extends { default void m() { super.(); } }",
    ];
    let mut extractor = SymbolExtractor::new().expect("extractor");
    for source in broken {
        extractor
            .extract_with_edges_from_source(Language::Java, TEST_PATH, source)
            .unwrap_or_else(|error| panic!("extraction failed for {source:?}: {error}"));
    }
}

const RECEIVERS_PATH: &str = "src/main/java/com/acme/store/Ledger.java";

#[test]
fn java_receivers_resolve_through_every_kind_of_declaration() {
    let extracted =
        extract_with_edges(RECEIVERS_PATH, include_str!("fixtures/java_receivers.java"));

    // A for-loop variable shadows the field of the same name; a pattern variable is typed
    // by its pattern; resources, catch variables and constructor arguments resolve; an
    // untyped lambda parameter stays bare.
    assert_eq!(
        targets_from(&extracted, "Ledger::post", EdgeKind::Calls),
        strings(&[
            "Account::cursor",
            "Account::lines",
            "Cursor::advance",
            "Cursor::hasNext",
            "Cursor::mark",
            "Deposit::isLarge",
            "Deposit::review",
            "Entry",
            "Ledger::label",
            "Ledger::nextNumber",
            "Ledger::open",
            "LedgerException::report",
            "Session::flush",
            "forEach",
            "settle",
        ])
    );
    // A nested type of a JDK type keeps its full name.
    assert_eq!(
        targets_from(&extracted, "Ledger::index", EdgeKind::Calls),
        strings(&["java.util.Map.Entry::getValue"])
    );
    assert_eq!(
        targets_from(&extracted, "Ledger::index", EdgeKind::TypeRef),
        strings(&["Entry"])
    );
    // Nested types are spelled with their declaring types, written simply or qualified.
    assert_eq!(
        targets_from(&extracted, "Ledger::builder", EdgeKind::Calls),
        strings(&["Ledger::Builder"])
    );
    assert_eq!(
        targets_from(&extracted, "Ledger::builder", EdgeKind::TypeRef),
        strings(&["Ledger::Builder"])
    );
    // An anonymous class's own field wins over the enclosing class's field, and its calls to
    // the enclosing class's methods are spelled with that class.
    assert_eq!(
        targets_from(&extracted, "Ledger::configure", EdgeKind::Calls),
        strings(&[
            "Cache",
            "Cache::clear",
            "Ledger::Builder::limit",
            "Ledger::builder",
            "Ledger::reset",
            "Runnable",
        ])
    );
    // A nested class calling its outer class's method.
    assert_eq!(
        targets_from(&extracted, "Ledger::Builder::limit", EdgeKind::Calls),
        strings(&["Ledger::validate"])
    );
    // A switch pattern binds its variable.
    assert_eq!(
        targets_from(&extracted, "Ledger::route", EdgeKind::Calls),
        strings(&["Withdrawal::cancel"])
    );
    // A nested type imported from this project keeps its declaring type, and an import
    // shadows the java.lang type of the same name.
    assert_eq!(
        targets_from(&extracted, "Ledger::done", EdgeKind::Calls),
        strings(&["Order::Status::isFinal"])
    );
    assert_eq!(
        targets_from(&extracted, "Ledger::done", EdgeKind::TypeRef),
        strings(&["Order::Status"])
    );
    assert_eq!(
        targets_from(&extracted, "Ledger::save", EdgeKind::TypeRef),
        strings(&["Record"])
    );
    // Another class's nested type is not in scope by simple name.
    assert_eq!(
        targets_from(&extracted, "Factory::start", EdgeKind::Calls),
        strings(&["Builder"])
    );
    assert_eq!(
        targets_from(&extracted, "Factory::start", EdgeKind::TypeRef),
        strings(&["Builder"])
    );
    // A generic method's type parameter is not a type to reference.
    assert!(targets_from(&extracted, "Factory::keep", EdgeKind::TypeRef).is_empty());
    // A compact constructor is a method; record components and enum fields are fields.
    assert_eq!(
        targets_from(&extracted, "Entry::Entry", EdgeKind::Calls),
        strings(&["Entry::check"])
    );
    assert_eq!(
        targets_from(&extracted, "Entry::describe", EdgeKind::Calls),
        strings(&["String::trim"])
    );
    assert_eq!(
        targets_from(&extracted, "Currency::symbol", EdgeKind::Calls),
        strings(&["Currency::name", "Formatter::format"])
    );
    // An interface's `extends` is a supertype too.
    assert_eq!(
        targets_from(&extracted, "Journal", EdgeKind::Implements),
        strings(&["Source"])
    );
    assert_eq!(
        targets_from(&extracted, "Journal::append", EdgeKind::Calls),
        strings(&["Entry::describe"])
    );
}

#[test]
fn java_annotations_and_comments_do_not_change_ids() {
    let source = include_str!("fixtures/java_receivers.java");
    let id_of = |source: &str, qualified_name: &str| {
        extract_with_edges(RECEIVERS_PATH, source)
            .symbols
            .into_iter()
            .find(|symbol| symbol.qualified_name == qualified_name)
            .map(|symbol| symbol.id)
            .unwrap_or_else(|| panic!("missing {qualified_name}"))
    };
    let edited = source
        .replace(
            "    @Deprecated\n    void reset() {}",
            "    // kept for callers\n    void reset() {}",
        )
        .replace(
            "    void save(Record record) {}",
            "    @Override\n    void save(Record record) {}",
        );
    assert_ne!(edited, source);
    for qualified_name in ["Ledger::reset", "Ledger::save"] {
        assert_eq!(
            id_of(source, qualified_name),
            id_of(&edited, qualified_name),
            "{qualified_name}"
        );
    }
}

#[test]
fn java_overloads_share_a_qualified_name_but_not_an_id() {
    let extracted =
        extract_with_edges(RECEIVERS_PATH, include_str!("fixtures/java_receivers.java"));
    let overloads = extracted
        .symbols
        .iter()
        .filter(|symbol| symbol.qualified_name == "Overloads::find")
        .collect::<Vec<_>>();
    assert_eq!(overloads.len(), 2);
    assert_ne!(overloads[0].id, overloads[1].id);
}
