use std::fs;

use aether_core::{EdgeKind, Language};
use aether_parse::{ExtractedFile, SymbolExtractor};

fn extract(source: &str) -> ExtractedFile {
    SymbolExtractor::new()
        .unwrap()
        .extract_with_edges_from_source(Language::Rust, "src/lib.rs", source)
        .unwrap()
}

fn assert_raw_call(file: &ExtractedFile, raw: &str) {
    let owner = file
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "run")
        .unwrap();
    let calls = file
        .edges
        .iter()
        .filter(|edge| {
            edge.edge_kind == EdgeKind::Calls
                && edge.source_id == owner.id
                && edge.target_qualified_name == raw
        })
        .collect::<Vec<_>>();
    assert!(!calls.is_empty(), "missing raw {raw}: {:?}", file.edges);
    assert!(
        calls
            .iter()
            .all(|edge| edge.target_symbol_id.is_none() && edge.target_file_path.is_none()),
        "{calls:?}"
    );
}

fn assert_local_call(file: &ExtractedFile, target: &str) {
    let owner = file
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "run")
        .unwrap();
    let target = file
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == target)
        .unwrap();
    assert!(
        file.edges
            .iter()
            .any(|edge| edge.edge_kind == EdgeKind::Calls
                && edge.source_id == owner.id
                && edge.target_symbol_id.as_deref() == Some(target.id.as_str())),
        "missing {}: {:?}",
        target.qualified_name,
        file.edges
    );
}

#[test]
fn block_item_expansion_withholds_outer_and_same_block_binding_proofs() {
    // Real procedural attributes can replace each item with `let stable=||2;`.
    // That shadows even an explicit fn stable in the SAME block, not just an
    // outer definition. Unknown cfg_attr conditions are not evaluated here.
    for attribute in [
        "#[binding_rewriter::replace]",
        "#[cfg_attr(all(), binding_rewriter::replace)]",
        "#[cfg_attr(any(), binding_rewriter::replace)]",
    ] {
        for item in [
            "fn placeholder(){}",
            "struct Placeholder;",
            "const PLACEHOLDER:u8=0;",
        ] {
            for same_block in [false, true] {
                let (outer, inner) = if same_block {
                    ("", "fn stable()->u8{1}")
                } else {
                    ("fn stable()->u8{1}", "")
                };
                let source = format!(
                    "{outer}struct Data;trait Mark{{}}fn run(){{{inner}{attribute}/* attached */{item}stable();fn nested(_:Data){{}}struct Local;impl Mark for Local{{}}}}"
                );
                let file = extract(&source);
                assert_raw_call(&file, "stable");
                assert!(
                    file.edges.iter().all(|edge| !matches!(
                        edge.edge_kind,
                        EdgeKind::TypeRef | EdgeKind::Implements
                    )),
                    "{source}: {:?}",
                    file.edges
                );
            }
        }
    }
}

#[test]
fn block_custom_derives_including_shadowed_builtin_names_keep_calls_raw() {
    for (import, derive) in [
        ("use binding_rewriter::Shadow as Debug;", "Debug"),
        ("use binding_rewriter::Shadow as Clone;", "Clone"),
        ("", "binding_rewriter::LetBang"),
    ] {
        for same_block in [false, true] {
            let (outer, inner) = if same_block {
                ("", "fn stable()->u8{1}")
            } else {
                ("fn stable()->u8{1}", "")
            };
            let source = format!(
                "{import}{outer}fn run(){{{inner}#[derive({derive})]struct Marker;stable();}}"
            );
            assert_raw_call(&extract(&source), "stable");
        }
    }
}

#[test]
fn module_expanders_preserve_known_unattributed_free_and_static_proofs() {
    // In a valid Rust module, expansion cannot replace these existing direct
    // names with same-name items without a compiler duplicate-definition error.
    // This does not apply to blocks or to receiver trait dispatch.
    for item in [
        "#[binding_rewriter::replace]fn placeholder(){}",
        "#[binding_rewriter::replace]struct Placeholder;",
        "#[binding_rewriter::replace]const PLACEHOLDER:u8=0;",
        "#[derive(Debug)]struct Marker;",
        "#[derive(Clone)]struct Marker;",
        "#[derive(binding_rewriter::Custom)]struct Marker;",
    ] {
        let source = format!(
            "{item}fn stable(){{}}struct Buf;impl Buf{{fn new()->Self{{Self}}}}fn run(){{stable();Buf::new();}}"
        );
        let file = extract(&source);
        assert_local_call(&file, "stable");
        assert_local_call(&file, "Buf::new");
    }
}

#[test]
fn inert_item_builtins_preserve_unrelated_block_and_receiver_proofs() {
    for item in [
        "fn placeholder(){}",
        "struct Placeholder;",
        "const PLACEHOLDER:u8=0;",
        "#[inline]fn placeholder(){}",
        "#[cold]fn placeholder(){}",
        "#[must_use]struct Placeholder;",
        "#[deprecated]const PLACEHOLDER:u8=0;",
        "#[repr(C)]struct Placeholder;",
        "#[non_exhaustive]enum Placeholder{One}",
        "#[track_caller]fn placeholder(){}",
        "#[used]static PLACEHOLDER:u8=0;",
        "#[target_feature(enable=\"sse2\")]unsafe fn placeholder(){}",
        "#[instruction_set(arm::a32)]fn placeholder(){}",
        "#[optimize(size)]fn placeholder(){}",
        "#[unsafe(no_mangle)]fn placeholder(){}",
        r#"#[unsafe(export_name="placeholder_export")]fn placeholder(){}"#,
        r##"#[unsafe(link_section=r#".placeholder"#)]static PLACEHOLDER:u8=0;"##,
        r#"#[unsafe(naked)]unsafe extern "C" fn placeholder(){core::arch::naked_asm!("ret");}"#,
        "#[allow(dead_code)]/* attached */#[inline]fn placeholder(){}",
    ] {
        let source = format!(
            "fn stable(){{}}struct Buf;impl Buf{{fn work(&self){{}}}}fn run(buf:Buf){{{item}stable();buf.work();}}"
        );
        let file = extract(&source);
        assert_local_call(&file, "stable");
        assert_local_call(&file, "Buf::work");
    }
}

#[test]
fn unknown_scoped_raw_and_unsafe_item_attributes_stay_opaque() {
    for attribute in [
        "#[tool::inline]",
        "#[r#inline]",
        "#[cfg_attr(all(), inline)]",
        "#[macro_use]",
        "#[path=\"elsewhere.rs\"]",
        "#[unsafe(tool::no_mangle)]",
        "#[unsafe(r#no_mangle)]",
        "#[unsafe(no_mangle, export_name=\"x\")]",
        "#[unsafe(export_name=concat!(\"x\"))]",
        "#[allow(dead_code)]/* comment */#[binding_rewriter::replace]",
    ] {
        let source = format!("fn stable(){{}}fn run(){{{attribute}fn placeholder(){{}}stable();}}");
        assert_raw_call(&extract(&source), "stable");
    }
}

#[test]
fn item_generated_trait_candidates_withhold_dot_but_keep_module_ufcs_proof() {
    // A generated trait work(self) can beat known inherent work(&self). Explicit
    // Buf::work(&buf) still selects the known inherent method.
    for item in [
        "#[binding_rewriter::trait_impl]fn placeholder(){}",
        "#[binding_rewriter::trait_impl]struct Placeholder;",
        "#[binding_rewriter::trait_impl]const PLACEHOLDER:u8=0;",
        "#[derive(Debug)]struct Marker;",
        "#[derive(binding_rewriter::Trait)]struct Marker;",
    ] {
        let source = format!(
            "{item}struct Buf;impl Buf{{fn work(&self){{}}}}fn run(buf:Buf){{buf.work();Buf::work(&buf);}}"
        );
        let file = extract(&source);
        assert_raw_call(&file, "work");
        assert_local_call(&file, "Buf::work");
        let source = format!(
            "struct Buf;impl Buf{{fn work(&self){{}}}}fn run(buf:Buf){{{item}buf.work();}}"
        );
        assert_raw_call(&extract(&source), "work");
    }
}

#[test]
fn external_target_expansion_withholds_dot_but_preserves_static_file_proof() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname='fixture'\nversion='0.1.0'\n",
    )
    .unwrap();
    let source = "mod dep;fn run(buf:crate::dep::Buf){buf.work();crate::dep::Buf::work(&buf);}";
    fs::write(temp.path().join("src/lib.rs"), source).unwrap();
    for (producer, opaque) in [
        ("", false),
        ("#[inline]fn placeholder(){}", false),
        ("#[derive(Debug)]struct Marker;", true),
        ("#[binding_rewriter::trait_impl]fn placeholder(){}", true),
    ] {
        fs::write(
            temp.path().join("src/dep.rs"),
            format!("{producer}pub struct Buf;impl Buf{{pub fn work(&self){{}}}}"),
        )
        .unwrap();
        let mut extractor = SymbolExtractor::new()
            .unwrap()
            .with_workspace_root(temp.path());
        let file = extractor
            .extract_with_edges_from_source(Language::Rust, "src/lib.rs", source)
            .unwrap();
        if opaque {
            assert_raw_call(&file, "work");
        } else {
            assert!(
                !file
                    .edges
                    .iter()
                    .any(|edge| edge.edge_kind == EdgeKind::Calls
                        && edge.target_qualified_name == "work"),
                "{file:?}"
            );
        }
        assert!(
            file.edges
                .iter()
                .any(|edge| edge.edge_kind == EdgeKind::Calls
                    && edge.target_qualified_name == "Buf::work"
                    && edge.target_file_path.as_deref() == Some("src/dep.rs")),
            "UFCS file proof lost: {producer}: {:?}",
            file.edges
        );
    }
}
