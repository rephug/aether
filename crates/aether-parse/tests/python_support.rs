use aether_core::{EdgeKind, Language, SymbolKind};
use aether_parse::SymbolExtractor;

fn extract_with_edges(path: &str, source: &str) -> aether_parse::ExtractedFile {
    let mut extractor = SymbolExtractor::new().expect("extractor");
    extractor
        .extract_with_edges_from_source(Language::Python, path, source)
        .expect("python extraction")
}

#[test]
fn extracts_python_symbols_with_expected_kinds_and_qualified_names() {
    let source = include_str!("fixtures/python_basic.py");
    let extracted = extract_with_edges("tests/fixtures/python_basic.py", source);

    let names = extracted
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.clone())
        .collect::<Vec<_>>();

    assert!(names.contains(&"tests.fixtures.python_basic::MODULE_FLAG".to_owned()));
    assert!(names.contains(&"tests.fixtures.python_basic::UserId".to_owned()));
    assert!(names.contains(&"tests.fixtures.python_basic::top_level".to_owned()));
    assert!(names.contains(&"tests.fixtures.python_basic::top_level::nested".to_owned()));
    assert!(names.contains(&"tests.fixtures.python_basic::decorated_function".to_owned()));
    assert!(names.contains(&"tests.fixtures.python_basic::Worker".to_owned()));
    assert!(names.contains(&"tests.fixtures.python_basic::Worker::__init__".to_owned()));
    assert!(names.contains(&"tests.fixtures.python_basic::Worker::factory".to_owned()));
    assert!(names.contains(&"tests.fixtures.python_basic::Worker::run".to_owned()));

    let by_name = extracted
        .symbols
        .iter()
        .map(|symbol| (symbol.name.as_str(), symbol.kind))
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(by_name.get("MODULE_FLAG"), Some(&SymbolKind::Variable));
    assert_eq!(by_name.get("UserId"), Some(&SymbolKind::TypeAlias));
    assert_eq!(by_name.get("top_level"), Some(&SymbolKind::Function));
    assert_eq!(by_name.get("nested"), Some(&SymbolKind::Function));
    assert_eq!(by_name.get("Worker"), Some(&SymbolKind::Class));
    assert_eq!(by_name.get("__init__"), Some(&SymbolKind::Method));
}

#[test]
fn python_symbol_ids_are_stable_with_line_offset_changes() {
    let source = include_str!("fixtures/python_basic.py");
    let shifted = format!("\n\n{source}");

    let original = extract_with_edges("tests/fixtures/python_basic.py", source);
    let shifted = extract_with_edges("tests/fixtures/python_basic.py", &shifted);

    let original_ids = original
        .symbols
        .iter()
        .map(|symbol| (symbol.qualified_name.clone(), symbol.id.clone()))
        .collect::<std::collections::HashMap<_, _>>();
    let shifted_ids = shifted
        .symbols
        .iter()
        .map(|symbol| (symbol.qualified_name.clone(), symbol.id.clone()))
        .collect::<std::collections::HashMap<_, _>>();

    assert_eq!(original_ids, shifted_ids);
}

#[test]
fn extracts_python_calls_and_import_edges() {
    let source = include_str!("fixtures/python_imports.py");
    let extracted = extract_with_edges("tests/fixtures/python_imports.py", source);

    let call_targets = extracted
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .map(|edge| edge.target_qualified_name.clone())
        .collect::<Vec<_>>();
    assert!(call_targets.contains(&"helper".to_owned()));
    assert!(call_targets.contains(&"thing.format".to_owned()));
    assert!(call_targets.contains(&"str".to_owned()));

    let depends_targets = extracted
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::DependsOn)
        .map(|edge| edge.target_qualified_name.clone())
        .collect::<Vec<_>>();
    assert!(depends_targets.contains(&"os".to_owned()));
    assert!(depends_targets.contains(&"pkg.alpha".to_owned()));
    assert!(depends_targets.contains(&"pkg.beta".to_owned()));
    assert!(depends_targets.contains(&"core.util.helper".to_owned()));
    assert!(depends_targets.contains(&"tests.fixtures.local.thing".to_owned()));
    assert!(depends_targets.contains(&"tests.shared.model".to_owned()));
    assert!(depends_targets.contains(&"pkg.star".to_owned()));
}

#[test]
fn init_module_uses_package_name_for_qualified_symbols() {
    let source = include_str!("fixtures/python_package/__init__.py");
    let extracted = extract_with_edges("tests/fixtures/python_package/__init__.py", source);

    let names = extracted
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.clone())
        .collect::<Vec<_>>();
    assert!(names.contains(&"tests.fixtures.python_package::PACKAGE_VALUE".to_owned()));
    assert!(names.contains(&"tests.fixtures.python_package::bootstrap".to_owned()));
}
