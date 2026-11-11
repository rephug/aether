use std::collections::BTreeSet;

use aether_core::{EdgeKind, Language, Symbol, stable_symbol_id};
use aether_parse::{DuplicateSymbolIdentity, ExtractedFile, SymbolExtractor};

const PATH: &str = "pkg/alternatives.py";
const SOURCE: &str = include_str!("fixtures/python_identity_alternatives.py");
const FAMILIES: [&str; 6] = [
    "async_variant::decorator::is_async",
    "CodeGenerator::_make_finalize::finalize",
    "_get_zipimporter_files",
    "Parser::parse_assign_target",
    "Parser::parse_tuple::parse",
    "urlize::trim_url",
];

fn extract(source: &str) -> ExtractedFile {
    SymbolExtractor::new()
        .expect("extractor")
        .extract_with_edges_from_source(Language::Python, PATH, source)
        .expect("Python extraction")
}

fn family<'a>(file: &'a ExtractedFile, suffix: &str) -> Vec<&'a Symbol> {
    let name = format!("pkg.alternatives::{suffix}");
    file.symbols
        .iter()
        .filter(|symbol| symbol.qualified_name == name)
        .collect()
}

#[test]
fn jinja_alternative_shapes_keep_both_declarations_and_their_call_owners() {
    let file = extract(SOURCE);
    let ids = file
        .symbols
        .iter()
        .map(|symbol| &symbol.id)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        ids.len(),
        file.symbols.len(),
        "no declaration may overwrite another"
    );
    for suffix in FAMILIES {
        let variants = family(&file, suffix);
        assert_eq!(variants.len(), 2, "{suffix}");
        assert_ne!(variants[0].id, variants[1].id, "{suffix}");
        assert_ne!(
            variants[0].signature_fingerprint,
            variants[1].signature_fingerprint
        );
        for symbol in variants {
            assert_eq!(
                symbol.id,
                stable_symbol_id(
                    symbol.language,
                    &symbol.file_path,
                    symbol.kind,
                    &symbol.qualified_name,
                    &symbol.signature_fingerprint
                ),
                "hover identity must agree"
            );
            let (start, end) = (
                symbol.range.start_byte.expect("start"),
                symbol.range.end_byte.expect("end"),
            );
            let body = &SOURCE[start..end];
            let expected = if body.contains("left()") {
                "left"
            } else {
                "right"
            };
            let targets = file
                .edges
                .iter()
                .filter(|edge| edge.source_id == symbol.id && edge.edge_kind == EdgeKind::Calls)
                .map(|edge| edge.target_qualified_name.as_str())
                .collect::<Vec<_>>();
            if !body.ends_with("...") {
                assert_eq!(
                    targets,
                    [format!("pkg.alternatives::{expected}")],
                    "{suffix}: {body}"
                );
            }
        }
    }
}

#[test]
fn collision_ids_survive_body_formatting_offsets_and_unrelated_declarations() {
    let before = extract(SOURCE);
    let edited = format!(
        "\n# shifted source\n{}\n\ndef unrelated():\n    return None\n",
        SOURCE.replace("return left()", "return left() + 10")
    );
    let after = extract(&edited);
    for suffix in FAMILIES {
        let before_ids = family(&before, suffix)
            .into_iter()
            .map(|symbol| &symbol.id)
            .collect::<BTreeSet<_>>();
        let after_ids = family(&after, suffix)
            .into_iter()
            .map(|symbol| &symbol.id)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            before_ids, after_ids,
            "body/line changes must not change {suffix}"
        );
    }
    for name in ["left", "right", "urlize"] {
        assert_eq!(family(&before, name)[0].id, family(&after, name)[0].id);
    }
}

#[test]
fn adding_a_branch_does_not_renumber_existing_collision_variants() {
    let source = "if mode == 'a b':\n    def select(x):\n        return 1\nelse:\n    def select(x):\n        return 2\n";
    let original = extract(source);
    let extended = extract(&format!(
        "{source}\nif mode == 'ab':\n    def select(x):\n        return 3\n"
    ));
    let original_ids = family(&original, "select")
        .into_iter()
        .map(|symbol| &symbol.id)
        .collect::<BTreeSet<_>>();
    let extended_ids = family(&extended, "select")
        .into_iter()
        .map(|symbol| &symbol.id)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        extended_ids.len(),
        3,
        "literal whitespace distinguishes contexts"
    );
    assert!(original_ids.is_subset(&extended_ids));
}

#[test]
fn singleton_ids_keep_the_historical_signature_scheme() {
    let source = "def select(x):\n    return x\n";
    let file = extract(source);
    let symbol = &file.symbols[0];
    let signature = aether_core::signature_fingerprint("def select(x):\n    ");
    assert_eq!(symbol.signature_fingerprint, signature);
    assert_eq!(
        symbol.id,
        stable_symbol_id(
            Language::Python,
            PATH,
            symbol.kind,
            &symbol.qualified_name,
            &signature
        )
    );
}

#[test]
fn unresolved_same_context_declarations_fail_before_map_projection() {
    let source = "def select(x):\n    return 1\n\ndef select(x):\n    return 2\n";
    let error = SymbolExtractor::new()
        .expect("extractor")
        .extract_with_edges_from_source(Language::Python, PATH, source)
        .expect_err("ambiguous twins");
    assert!(
        error.downcast_ref::<DuplicateSymbolIdentity>().is_some(),
        "{error}"
    );
}

#[test]
fn test_intents_use_the_final_branch_specific_symbol_ids() {
    let file = extract(
        "if enabled:\n    def test_result():\n        assert True\nelse:\n    def test_result():\n        assert False\n",
    );
    let ids = file
        .symbols
        .iter()
        .map(|symbol| symbol.id.clone())
        .collect::<BTreeSet<_>>();
    let linked_ids = file
        .test_intents
        .iter()
        .map(|intent| intent.symbol_id.clone().expect("linked symbol"))
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), 2);
    assert_eq!(linked_ids, ids);
}
