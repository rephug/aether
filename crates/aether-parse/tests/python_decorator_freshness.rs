use aether_core::{EdgeKind, Language, Symbol, content_hash};
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
