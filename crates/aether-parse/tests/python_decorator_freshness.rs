use aether_core::{EdgeKind, Language, Symbol, content_hash, file_source_id};
use aether_parse::SymbolExtractor;

fn named(source: &str, name: &str) -> Symbol {
    SymbolExtractor::new()
        .expect("extractor")
        .extract_from_source(Language::Python, "fixture.py", source)
        .expect("parse Python")
        .into_iter()
        .find(|symbol| symbol.name == name)
        .expect("named symbol")
}

#[test]
fn python_decorator_argument_edits_change_content_without_retiring_identity() {
    for source in [
        "@limit(2)\ndef adjust(value: int) -> int:\n    return value + 1\n",
        "@limit(2)\nasync def adjust(value: int) -> int:\n    return value + 1\n",
        "@limit(2)\nclass Adjust:\n    pass\n",
        "class Container:\n    @limit(2)\n    def adjust(self, value):\n        return value + 1\n",
    ] {
        let name = if source.contains("class Adjust") {
            "Adjust"
        } else {
            "adjust"
        };
        let original = named(source, name);
        let edited = named(&source.replace("limit(2)", "limit(3)"), name);
        assert_eq!(original.id, edited.id, "decorator edits preserve history");
        assert_ne!(
            original.content_hash, edited.content_hash,
            "decorators affect behavior: {source}"
        );
        let start = original.range.start_byte.expect("start byte");
        let end = original.range.end_byte.expect("end byte");
        assert!(source[start..end].starts_with("@limit(2)"));
        assert_eq!(original.content_hash, content_hash(&source[start..end]));
    }
}

#[test]
fn python_adding_and_removing_decorators_keep_identity_and_change_content() {
    let plain = "def adjust(value: int) -> int:\n    return value + 1\n";
    let decorated = format!("@outer\n@limit(2)\n{plain}");
    let undecorated = named(plain, "adjust");
    let wrapped = named(&decorated, "adjust");
    assert_eq!(undecorated.id, wrapped.id);
    assert_eq!(
        undecorated.signature_fingerprint,
        wrapped.signature_fingerprint
    );
    assert_ne!(undecorated.content_hash, wrapped.content_hash);
    assert_eq!(wrapped.range.start.line, 1);
    assert_eq!(wrapped.range.start_byte, Some(0));
    assert_eq!(wrapped.content_hash, content_hash(decorated.trim_end()));
}

#[test]
fn decorated_function_calls_keep_the_function_as_their_source() {
    for limit in [2, 3] {
        let source = format!(
            "def increment(value):\n    return value + 1\n\n@limit({limit})\ndef adjust(value):\n    return increment(value)\n"
        );
        let extracted = SymbolExtractor::new()
            .expect("extractor")
            .extract_with_edges_from_source(Language::Python, "fixture.py", &source)
            .expect("extract Python calls");
        let function = extracted
            .symbols
            .iter()
            .find(|symbol| symbol.name == "adjust")
            .expect("function");
        let call = extracted
            .edges
            .iter()
            .find(|edge| {
                edge.edge_kind == EdgeKind::Calls
                    && edge.target_qualified_name == "fixture::increment"
            })
            .expect("increment call");
        assert_eq!(
            call.source_id, function.id,
            "decorators must not move function-body calls to the module"
        );
    }
}

#[test]
fn multiline_decorators_cover_the_complete_wrapper_and_preserve_identity() {
    let prefix = "# not part of the declaration\n";
    let declaration = "@outer\n@limit(\n    2,\n    mode=\"strict\",\n)\ndef adjust(value):\n    return value + 1";
    let source = format!("{prefix}{declaration}\n\ndef sibling():\n    pass\n");
    let original = named(&source, "adjust");
    assert_eq!(original.range.start.line, 2);
    assert_eq!(original.range.start_byte, Some(prefix.len()));
    assert_eq!(
        original.range.end_byte,
        Some(prefix.len() + declaration.len())
    );
    assert_eq!(original.content_hash, content_hash(declaration));

    let edited = named(&source.replace("    2,", "    3,"), "adjust");
    assert_eq!(original.id, edited.id);
    assert_eq!(original.signature_fingerprint, edited.signature_fingerprint);
    assert_ne!(original.content_hash, edited.content_hash);
    assert_eq!(
        edited.content_hash,
        content_hash(&declaration.replace("    2,", "    3,"))
    );
}

#[test]
fn non_ascii_decorator_arguments_use_exact_utf8_source_boundaries() {
    let prefix = "# préface 🦀\n";
    let declaration = "@label(\"café 🌍\")\ndef adjust(value):\n    return value + 1";
    let source = format!("{prefix}{declaration}\n\ndef sibling():\n    pass\n");
    let original = named(&source, "adjust");
    let edited_source = source.replace("café", "日本語");
    let edited = named(&edited_source, "adjust");
    assert_eq!(original.id, edited.id);
    assert_ne!(original.content_hash, edited.content_hash);
    for (text, symbol, expected) in [
        (source.as_str(), &original, declaration.to_owned()),
        (
            edited_source.as_str(),
            &edited,
            declaration.replace("café", "日本語"),
        ),
    ] {
        let start = symbol.range.start_byte.expect("start byte");
        let end = symbol.range.end_byte.expect("end byte");
        assert_eq!(start, prefix.len());
        assert_eq!(end, prefix.len() + expected.len());
        assert_eq!(text.get(start..end).expect("UTF-8 boundaries"), expected);
        assert_eq!(symbol.content_hash, content_hash(&expected));
    }
}

#[test]
fn nested_decorated_functions_own_body_calls_but_not_their_decorator_calls() {
    let source = "def decorate(label):\n    return lambda function: function\n\ndef helper(value):\n    return value + 1\n\n@decorate(\"outer\")\ndef outer(value):\n    @decorate(\"inner\")\n    def inner(item):\n        return helper(item)\n    return inner(value)\n";
    let original_outer = named(source, "outer");
    let original_inner = named(source, "inner");
    for source in [source.to_owned(), source.replace("\"inner\"", "\"nested\"")] {
        let extracted = SymbolExtractor::new()
            .expect("extractor")
            .extract_with_edges_from_source(Language::Python, "fixture.py", &source)
            .expect("extract nested decorated functions");
        let outer = extracted
            .symbols
            .iter()
            .find(|symbol| symbol.name == "outer")
            .expect("outer");
        let inner = extracted
            .symbols
            .iter()
            .find(|symbol| symbol.name == "inner")
            .expect("inner");
        assert_eq!(outer.id, original_outer.id);
        assert_eq!(inner.id, original_inner.id);
        assert_eq!(inner.qualified_name, "fixture::outer::inner");
        let mut calls = extracted
            .edges
            .iter()
            .filter(|edge| edge.edge_kind == EdgeKind::Calls)
            .map(|edge| (edge.source_id.clone(), edge.target_qualified_name.clone()))
            .collect::<Vec<_>>();
        calls.sort();
        let mut expected = vec![
            (file_source_id("fixture.py"), "fixture::decorate".to_owned()),
            (outer.id.clone(), "fixture::decorate".to_owned()),
            (outer.id.clone(), "fixture::outer::inner".to_owned()),
            (inner.id.clone(), "fixture::helper".to_owned()),
        ];
        expected.sort();
        assert_eq!(calls, expected, "decorators execute in the enclosing scope");
    }
}
