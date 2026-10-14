use std::collections::HashMap;
use std::path::Path;

use aether_core::{EdgeKind, Language, SymbolKind, file_source_id};
use aether_parse::{ExtractedFile, SymbolExtractor, language_for_path};

const BASIC_PATH: &str = "internal/store/go_basic.go";
const TEST_PATH: &str = "internal/store/go_basic_test.go";

fn extract_with_edges(path: &str, source: &str) -> ExtractedFile {
    let mut extractor = SymbolExtractor::new().expect("extractor");
    extractor
        .extract_with_edges_from_source(Language::Go, path, source)
        .expect("go extraction")
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

fn ids(extracted: &ExtractedFile) -> HashMap<String, String> {
    extracted
        .symbols
        .iter()
        .map(|symbol| (symbol.qualified_name.clone(), symbol.id.clone()))
        .collect()
}

#[test]
fn go_files_map_to_the_go_language() {
    assert_eq!(
        language_for_path(Path::new("cmd/main.go")),
        Some(Language::Go)
    );

    let mut extractor = SymbolExtractor::new().expect("extractor");
    let extracted = extractor
        .extract_with_edges_from_path(Path::new(BASIC_PATH), include_str!("fixtures/go_basic.go"))
        .expect("extract by path");
    assert!(
        extracted
            .symbols
            .iter()
            .all(|symbol| symbol.language == Language::Go)
    );
}

#[test]
fn extracts_go_symbols_with_expected_kinds_and_qualified_names() {
    let extracted = extract_with_edges(BASIC_PATH, include_str!("fixtures/go_basic.go"));

    let kinds = extracted
        .symbols
        .iter()
        .map(|symbol| (symbol.qualified_name.as_str(), symbol.kind))
        .collect::<HashMap<_, _>>();
    let expected = [
        ("store::Item", SymbolKind::Struct),
        ("store::Getter", SymbolKind::Interface),
        ("store::Celsius", SymbolKind::TypeAlias),
        ("store::ItemMap", SymbolKind::TypeAlias),
        ("store::Kelvin", SymbolKind::TypeAlias),
        ("store::Store", SymbolKind::Struct),
        ("store::NewStore", SymbolKind::Function),
        ("store::Store::Get", SymbolKind::Method),
        ("store::Store::touch", SymbolKind::Method),
        ("store::normalize", SymbolKind::Function),
        ("store::max", SymbolKind::Function),
        ("store::Load", SymbolKind::Function),
        ("store::Map", SymbolKind::Function),
        ("store::Transform", SymbolKind::Function),
        ("store::Render", SymbolKind::Function),
    ];
    for (qualified_name, kind) in expected {
        assert_eq!(
            kinds.get(qualified_name),
            Some(&kind),
            "unexpected kind for {qualified_name}"
        );
    }
    // Interface method specs have no body of their own, blank functions can't be referenced,
    // and types declared inside a function body are local.
    assert_eq!(extracted.symbols.len(), expected.len(), "{kinds:?}");

    let method = extracted
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "store::Store::Get")
        .expect("store::Store::Get");
    assert_eq!(method.name, "Get");
}

#[test]
fn type_declaration_ranges_include_the_keyword_only_when_ungrouped() {
    let source = include_str!("fixtures/go_basic.go");
    let extracted = extract_with_edges(BASIC_PATH, source);
    let text_of = |qualified_name: &str| {
        let symbol = extracted
            .symbols
            .iter()
            .find(|symbol| symbol.qualified_name == qualified_name)
            .unwrap_or_else(|| panic!("missing {qualified_name}"));
        let start = symbol.range.start_byte.expect("start byte");
        let end = symbol.range.end_byte.expect("end byte");
        source[start..end].to_owned()
    };

    assert!(text_of("store::Item").starts_with("type Item struct {"));
    // Grouped specs start at their own name, even alone in their group.
    assert_eq!(text_of("store::Celsius"), "Celsius float64");
    assert_eq!(text_of("store::Kelvin"), "Kelvin float64");
}

#[test]
fn go_symbol_ids_survive_line_shifts_and_body_changes() {
    let source = include_str!("fixtures/go_basic.go");
    let original = ids(&extract_with_edges(BASIC_PATH, source));
    let shifted = ids(&extract_with_edges(BASIC_PATH, &format!("\n\n{source}")));
    assert_eq!(original, shifted);

    // A new struct field, a new interface method and a new spec in a group keep the ids of
    // the types around them.
    let edited = source
        .replace("\tKey   string\n", "\tKey   string\n\tOwner string\n")
        .replace(
            "\tGet(ctx context.Context, key string) (*Item, error)\n",
            "\tGet(ctx context.Context, key string) (*Item, error)\n\tLen() int\n",
        )
        .replace(
            "\tKelvin float64\n",
            "\tKelvin float64\n\tRankine float64\n",
        );
    assert_eq!(edited.matches("Owner").count(), 1);
    assert_eq!(edited.matches("Len()").count(), 1);
    assert_eq!(edited.matches("Rankine").count(), 1);
    let changed = ids(&extract_with_edges(BASIC_PATH, &edited));
    for qualified_name in ["store::Item", "store::Getter", "store::Kelvin"] {
        assert_eq!(
            changed.get(qualified_name),
            original.get(qualified_name),
            "id of {qualified_name} changed"
        );
    }
}

#[test]
fn extracts_go_import_edges_from_the_file() {
    let extracted = extract_with_edges(BASIC_PATH, include_str!("fixtures/go_basic.go"));
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
            "context",
            "example.com/app/api/v1",
            "example.com/app/internal/db",
            "example.com/app/internal/dbextra",
            "fmt",
            "github.com/google/uuid",
            "strings",
        ])
    );
}

#[test]
fn spells_go_call_targets_like_go_qualified_names() {
    let extracted = extract_with_edges(BASIC_PATH, include_str!("fixtures/go_basic.go"));

    // `make` is a builtin, and composite literals are not calls.
    assert!(targets_from(&extracted, "store::NewStore", EdgeKind::Calls).is_empty());
    // A method on the receiver; a standard-library call keeps Go's own spelling, which no
    // workspace symbol can match.
    assert_eq!(
        targets_from(&extracted, "store::Store::Get", EdgeKind::Calls),
        strings(&["fmt.Errorf", "store::Store::touch"])
    );
    // An import alias is spelled with the package's own name; a field's method stays bare.
    assert_eq!(
        targets_from(&extracted, "store::Store::touch", EdgeKind::Calls),
        strings(&["Exec", "store::normalize", "strings.TrimSpace"])
    );
    // A third-party `New` is qualified by its package, so it can't match a workspace `New`.
    // `max` is declared in the file, so it shadows the builtin; `len`, `string` and `rune`
    // don't, and a method on a call's result stays bare.
    assert_eq!(
        targets_from(&extracted, "store::normalize", EdgeKind::Calls),
        strings(&["String", "db::Flush", "store::max", "uuid::New"])
    );
    // Parameters' declared types qualify method calls, including through an import alias,
    // and conversions call the type.
    assert_eq!(
        targets_from(&extracted, "store::Load", EdgeKind::Calls),
        strings(&[
            "context.Background",
            "db::Conn::Ping",
            "dbextra::Pool::Drain",
            "store::Celsius",
            "store::Getter::Get",
        ])
    );
    // Calls through a parameter or local have no static target, and a local that shadows an
    // import is not that package; an explicitly instantiated generic is still a call.
    assert_eq!(
        targets_from(&extracted, "store::Transform", EdgeKind::Calls),
        strings(&["Reset", "store::Map", "store::pick"])
    );
    // A versioned API package (`.../api/v1`) declares `package v1`.
    assert_eq!(
        targets_from(&extracted, "store::Render", EdgeKind::Calls),
        strings(&["v1::Register", "v1::Widget::Render"])
    );
}

#[test]
fn extracts_go_type_refs_from_parameters_and_results() {
    let extracted = extract_with_edges(BASIC_PATH, include_str!("fixtures/go_basic.go"));

    assert_eq!(
        targets_from(&extracted, "store::NewStore", EdgeKind::TypeRef),
        strings(&["db::Conn", "store::Store"])
    );
    // `context.Context` is standard library, `K` is a type parameter and `error` is builtin;
    // the receiver is the method's parent, not a reference.
    assert_eq!(
        targets_from(&extracted, "store::Store::Get", EdgeKind::TypeRef),
        strings(&["store::Item"])
    );
    assert_eq!(
        targets_from(&extracted, "store::Load", EdgeKind::TypeRef),
        strings(&[
            "db::Conn",
            "dbextra::Pool",
            "store::Celsius",
            "store::Getter"
        ])
    );
    // A generic function's own type parameters are not types to reference.
    assert!(targets_from(&extracted, "store::Map", EdgeKind::TypeRef).is_empty());
    assert_eq!(
        targets_from(&extracted, "store::Render", EdgeKind::TypeRef),
        strings(&["v1::Widget"])
    );
}

#[test]
fn extracts_go_test_intents_from_test_functions() {
    let extracted = extract_with_edges(TEST_PATH, include_str!("fixtures/go_basic_test.go"));

    let intents = extracted
        .test_intents
        .iter()
        .map(|intent| (intent.test_name.as_str(), intent.intent_text.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        intents,
        vec![
            (
                "TestGetReturnsStoredItem",
                "checks that a stored item comes back unchanged."
            ),
            // A comment trailing the line above is not documentation.
            ("TestGolden", "golden"),
            ("TestHTTPServer_Handles404", "http server handles 404"),
        ]
    );

    for intent in &extracted.test_intents {
        assert_eq!(intent.language, Language::Go);
        assert_eq!(intent.group_label, None);
        let symbol = extracted
            .symbols
            .iter()
            .find(|symbol| Some(&symbol.id) == intent.symbol_id.as_ref())
            .expect("intent points at its test function");
        assert_eq!(symbol.name, intent.test_name);
    }
}

#[test]
fn go_test_intents_require_a_test_file() {
    let extracted = extract_with_edges(
        "internal/store/go_basic_helpers.go",
        include_str!("fixtures/go_basic_test.go"),
    );
    assert!(extracted.test_intents.is_empty());
}

#[test]
fn go_test_calls_are_attributed_to_the_test_function() {
    let extracted = extract_with_edges(TEST_PATH, include_str!("fixtures/go_basic_test.go"));
    // Calls inside the subtest closure belong to the enclosing test; `s` is a local variable,
    // so its method stays bare.
    assert_eq!(
        targets_from(
            &extracted,
            "store::TestGetReturnsStoredItem",
            EdgeKind::Calls
        ),
        strings(&[
            "Get",
            "store::NewStore",
            "testing.T::Fatal",
            "testing.T::Run"
        ])
    );
}

#[test]
fn go_testdata_files_are_skipped() {
    for path in [
        "internal/lint/testdata/src/a/a.go",
        "internal/lint/testdata/a_test.go",
    ] {
        let extracted = extract_with_edges(path, include_str!("fixtures/go_basic_test.go"));
        assert!(extracted.symbols.is_empty(), "{path}");
        assert!(extracted.edges.is_empty(), "{path}");
        assert!(extracted.test_intents.is_empty(), "{path}");
    }
}

#[test]
fn go_files_without_a_package_clause_are_skipped() {
    let extracted = extract_with_edges(
        "docs/snippet.go",
        "type NoPackage struct{}\nfunc Close() { g() }\n",
    );
    assert!(extracted.symbols.is_empty());
    assert!(extracted.edges.is_empty());
}

#[test]
fn go_extraction_tolerates_syntax_errors() {
    let broken = [
        "package p\n\ntype Broken struct\n",
        "package p\n\ntype Half interface {\n",
        "package p\n\nfunc (s *) Missing() {}\n",
        "package p\n\nfunc TestCut(t *testing.",
        "package p\n\nimport (\n\t\"fmt\"\n",
        "package p\n\nfunc f() { x.( }\n",
        "type NoPackage struct{}\nfunc f() { g() }\n",
    ];
    let mut extractor = SymbolExtractor::new().expect("extractor");
    for source in broken {
        extractor
            .extract_with_edges_from_source(Language::Go, TEST_PATH, source)
            .unwrap_or_else(|error| panic!("extraction failed for {source:?}: {error}"));
    }
}
