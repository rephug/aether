use std::fs;

use aether_core::{EdgeKind, Language};
use aether_parse::{ExtractedFile, SymbolExtractor};

fn extract(source: &str) -> ExtractedFile {
    SymbolExtractor::new()
        .unwrap()
        .extract_with_edges_from_source(Language::Rust, "src/lib.rs", source)
        .unwrap()
}

#[test]
fn rust_local_calls_and_type_endpoints_carry_exact_ids() {
    let extracted = extract(
        "trait Mark {} struct Buf; impl Mark for Buf {} impl Buf { fn new()->Self { Self } fn next(){ Self::new(); } } fn helper() {} fn run(value:Buf){helper();Buf::new();}",
    );
    let run = extracted
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "run")
        .unwrap();
    for target in ["helper", "Buf::new"] {
        let symbol = extracted
            .symbols
            .iter()
            .find(|symbol| symbol.qualified_name == target)
            .unwrap();
        assert!(
            extracted
                .edges
                .iter()
                .any(|edge| edge.edge_kind == EdgeKind::Calls
                    && edge.source_id == run.id
                    && edge.target_symbol_id.as_deref() == Some(symbol.id.as_str()))
        );
    }
    for kind in [EdgeKind::TypeRef, EdgeKind::Implements] {
        assert!(
            extracted
                .edges
                .iter()
                .any(|edge| edge.edge_kind == kind && edge.target_symbol_id.is_some())
        );
    }
}

#[test]
fn rust_unrelated_module_or_value_receiver_never_gives_local_proof() {
    let extracted = extract(
        "mod unrelated {pub fn helper(){}} struct Buf; impl Buf {fn new(){}} fn run(value:Buf){helper();value.new();}",
    );
    for edge in extracted
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
    {
        assert!(edge.target_symbol_id.is_none(), "{edge:?}");
        assert!(edge.target_file_path.is_none(), "{edge:?}");
    }
}

#[test]
fn rust_lexical_shadows_and_generic_types_are_unresolved() {
    for source in [
        "fn helper(){} fn run(helper:fn()){helper();}",
        "fn helper(){} fn run(){let helper = || {};helper();}",
        "fn helper(){} fn run(){for helper in [] {helper();}}",
        "fn helper(){} fn run(){const helper:fn()=||{};helper();}",
        "fn helper(){} fn run(){static helper:fn()=||{};helper();}",
        "struct Buf;impl Buf{fn new(){}} fn run<Buf>(){Buf::new();}",
    ] {
        let extracted = extract(source);
        assert!(
            extracted
                .edges
                .iter()
                .filter(|edge| edge.edge_kind == EdgeKind::Calls)
                .all(|edge| edge.target_symbol_id.is_none())
        );
    }
    for source in [
        "struct Buf; fn run<Buf>(value:Buf){}",
        "struct Buf; fn run(value:unrelated::Buf){}",
        "struct T;struct C<U>(U);impl<T> C<T>{fn run(x:T){}}",
    ] {
        let extracted = extract(source);
        assert!(
            extracted
                .edges
                .iter()
                .all(|edge| edge.edge_kind != EdgeKind::TypeRef)
        );
    }
}

#[test]
fn rust_inline_module_paths_and_import_aliases_keep_local_proof() {
    let extracted = extract(
        "mod dep {pub struct Data;pub fn helper(){}} use self::dep::helper as imported;use self::dep::Data as Input;fn run(x:Input){imported();dep::helper();}",
    );
    let helper = extracted
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "dep::helper")
        .unwrap();
    assert!(
        extracted
            .edges
            .iter()
            .filter(|edge| edge.edge_kind == EdgeKind::Calls)
            .all(|edge| edge.target_symbol_id.as_deref() == Some(helper.id.as_str()))
    );
    assert!(
        extracted
            .edges
            .iter()
            .any(|edge| edge.edge_kind == EdgeKind::TypeRef
                && edge.target_qualified_name == "dep::Data"
                && edge.target_symbol_id.is_some())
    );
}

#[test]
fn rust_workspace_explicit_imports_prove_one_declared_file() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname='fixture'\nversion='0.1.0'\n",
    )
    .unwrap();
    let source =
        "mod dep;use crate::dep::helper as imported;fn run(){imported();crate::dep::helper();}";
    fs::write(temp.path().join("src/lib.rs"), source).unwrap();
    fs::write(temp.path().join("src/dep.rs"), "pub fn helper() {}").unwrap();
    let mut extractor = SymbolExtractor::new()
        .unwrap()
        .with_workspace_root(temp.path());
    let extracted = extractor
        .extract_with_edges_from_source(Language::Rust, "src/lib.rs", source)
        .unwrap();
    let calls = extracted
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .collect::<Vec<_>>();
    assert!(!calls.is_empty());
    assert!(
        calls
            .iter()
            .all(|edge| edge.target_qualified_name == "helper"
                && edge.target_file_path.as_deref() == Some("src/dep.rs")
                && edge.target_symbol_id.is_none())
    );
    fs::write(temp.path().join("src/dep.rs"), "fn helper() {}").unwrap();
    assert!(
        extractor
            .refresh_workspace_file("src/dep.rs")
            .contains(&"src/lib.rs".to_owned())
    );
    let edited = extractor
        .extract_with_edges_from_source(Language::Rust, "src/lib.rs", source)
        .unwrap();
    assert!(
        edited
            .edges
            .iter()
            .filter(|edge| edge.edge_kind == EdgeKind::Calls)
            .all(|edge| edge.target_file_path.is_none())
    );
}

#[test]
fn rust_missing_ambiguous_glob_reexport_and_shadow_imports_stay_raw() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname='fixture'\nversion='0.1.0'\n",
    )
    .unwrap();
    for target in ["", "pub use other::helper;", "fn helper(){}"] {
        let source = "mod dep;use crate::dep::helper as imported;fn run(){imported();}";
        fs::write(temp.path().join("src/lib.rs"), source).unwrap();
        fs::write(temp.path().join("src/dep.rs"), target).unwrap();
        let mut extractor = SymbolExtractor::new()
            .unwrap()
            .with_workspace_root(temp.path());
        let extracted = extractor
            .extract_with_edges_from_source(Language::Rust, "src/lib.rs", source)
            .unwrap();
        assert!(
            extracted
                .edges
                .iter()
                .filter(|edge| edge.edge_kind == EdgeKind::Calls)
                .all(|edge| edge.target_file_path.is_none())
        );
    }
    fs::write(temp.path().join("src/dep.rs"), "pub fn helper(){}").unwrap();
    for source in [
        "mod dep;use crate::dep::*;fn run(){helper();}",
        "mod dep;use crate::dep::helper;fn run(helper:fn()){helper();}",
        "mod dep;use crate::dep::helper;fn run(){let helper=||{};helper();}",
    ] {
        fs::write(temp.path().join("src/lib.rs"), source).unwrap();
        let mut extractor = SymbolExtractor::new()
            .unwrap()
            .with_workspace_root(temp.path());
        let extracted = extractor
            .extract_with_edges_from_source(Language::Rust, "src/lib.rs", source)
            .unwrap();
        assert!(
            extracted
                .edges
                .iter()
                .filter(|edge| edge.edge_kind == EdgeKind::Calls)
                .all(|edge| edge.target_file_path.is_none())
        );
    }
    fs::create_dir_all(temp.path().join("src/dep")).unwrap();
    fs::write(temp.path().join("src/dep/mod.rs"), "pub fn helper(){}").unwrap();
    let source = "mod dep;use crate::dep::helper;fn run(){helper();}";
    let mut extractor = SymbolExtractor::new()
        .unwrap()
        .with_workspace_root(temp.path());
    let extracted = extractor
        .extract_with_edges_from_source(Language::Rust, "src/lib.rs", source)
        .unwrap();
    assert!(
        extracted
            .edges
            .iter()
            .filter(|edge| edge.edge_kind == EdgeKind::Calls)
            .all(|edge| edge.target_file_path.is_none())
    );
}

#[test]
fn rust_custom_cargo_target_roots_are_not_guessed() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    let source = "mod dep;use crate::dep::helper;fn run(){helper();}";
    fs::write(temp.path().join("src/lib.rs"), source).unwrap();
    fs::write(temp.path().join("src/dep.rs"), "pub fn helper(){}").unwrap();
    for custom in [
        "[lib]\npath='actual.rs'\n",
        "[ lib ]\npath='actual.rs'\n",
        "[\"lib\"]\npath='actual.rs'\n",
        "[[bin]]\nname='custom'\npath='actual.rs'\n",
    ] {
        fs::write(
            temp.path().join("Cargo.toml"),
            format!("[package]\nname='fixture'\nversion='0.1.0'\n{custom}"),
        )
        .unwrap();
        let mut extractor = SymbolExtractor::new()
            .unwrap()
            .with_workspace_root(temp.path());
        let extracted = extractor
            .extract_with_edges_from_source(Language::Rust, "src/lib.rs", source)
            .unwrap();
        assert!(
            extracted
                .edges
                .iter()
                .filter(|edge| edge.edge_kind == EdgeKind::Calls)
                .all(|edge| edge.target_file_path.is_none())
        );
    }
}

#[test]
fn rust_unlisted_or_custom_path_callers_do_not_get_module_proof() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname='fixture'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(temp.path().join("src/dep.rs"), "pub fn helper(){}").unwrap();
    let caller = "use super::dep::helper;fn run(){helper();}";
    fs::write(temp.path().join("src/unused.rs"), caller).unwrap();
    for root in ["mod dep;", "mod dep;#[path=\"other.rs\"] mod unused;"] {
        fs::write(temp.path().join("src/lib.rs"), root).unwrap();
        let mut extractor = SymbolExtractor::new()
            .unwrap()
            .with_workspace_root(temp.path());
        let extracted = extractor
            .extract_with_edges_from_source(Language::Rust, "src/unused.rs", caller)
            .unwrap();
        assert!(
            extracted
                .edges
                .iter()
                .filter(|edge| edge.edge_kind == EdgeKind::Calls)
                .all(|edge| edge.target_file_path.is_none())
        );
    }
    fs::write(temp.path().join("src/lib.rs"), "mod dep;mod unused;").unwrap();
    let mut extractor = SymbolExtractor::new()
        .unwrap()
        .with_workspace_root(temp.path());
    let extracted = extractor
        .extract_with_edges_from_source(Language::Rust, "src/unused.rs", caller)
        .unwrap();
    assert!(
        extracted
            .edges
            .iter()
            .any(|edge| edge.edge_kind == EdgeKind::Calls
                && edge.target_file_path.as_deref() == Some("src/dep.rs"))
    );
}

#[test]
fn rust_unsaved_caller_module_edits_override_disk_declarations() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname='fixture'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(temp.path().join("src/lib.rs"), "mod dep;").unwrap();
    fs::write(temp.path().join("src/dep.rs"), "pub fn helper(){}").unwrap();
    let mut extractor = SymbolExtractor::new()
        .unwrap()
        .with_workspace_root(temp.path());
    let extracted = extractor
        .extract_with_edges_from_source(
            Language::Rust,
            "src/lib.rs",
            "use crate::dep::helper;fn run(){helper();}",
        )
        .unwrap();
    assert!(
        extracted
            .edges
            .iter()
            .filter(|edge| edge.edge_kind == EdgeKind::Calls)
            .all(|edge| edge.target_file_path.is_none())
    );
}

#[test]
fn rust_attributed_local_targets_keep_raw_unresolved_calls() {
    for source in [
        "#[cfg(any())] fn helper(){} fn run(){helper();}",
        "#[rewrite] fn helper(){} fn run(){helper();}",
        "#[cfg(any())] mod dep{pub fn helper(){}} fn run(){dep::helper();}",
        "struct Buf;#[cfg(any())] impl Buf{fn new(){}} fn run(){Buf::new();}",
    ] {
        let extracted = extract(source);
        let calls = extracted
            .edges
            .iter()
            .filter(|edge| edge.edge_kind == EdgeKind::Calls)
            .collect::<Vec<_>>();
        assert!(!calls.is_empty());
        assert!(calls.iter().all(|edge| edge.target_symbol_id.is_none()));
    }
}

#[test]
fn rust_foreign_functions_and_extern_crate_aliases_shadow_local_items() {
    // The pinned grammar supports edition-2021 extern blocks. Its recovery for
    // edition-2024 unsafe extern can place calls outside their source function;
    // use supported syntax so these cases exercise raw-reference preservation.
    for source in [
        "fn helper(){} fn run(){extern \"C\"{fn helper();} unsafe{helper();}}",
        "fn helper(){} fn run(){extern \"C\"{static helper:fn();} unsafe{helper();}}",
        "mod util{pub mod thread{pub fn yield_now(){}}} fn run(){extern crate std as util;util::thread::yield_now();}",
        "mod std{pub mod thread{pub fn yield_now(){}}} fn run(){extern crate std;std::thread::yield_now();}",
    ] {
        let extracted = extract(source);
        let calls = extracted
            .edges
            .iter()
            .filter(|edge| edge.edge_kind == EdgeKind::Calls)
            .collect::<Vec<_>>();
        assert!(!calls.is_empty());
        assert!(
            calls
                .iter()
                .all(|edge| edge.target_symbol_id.is_none() && edge.target_file_path.is_none())
        );
    }
}

#[test]
fn rust_foreign_bindings_also_block_outer_explicit_import_proofs() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname='fixture'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(temp.path().join("src/dep.rs"), "pub fn helper(){}").unwrap();
    let source =
        "mod dep;use crate::dep::helper;fn run(){extern \"C\"{fn helper();} unsafe{helper();}}";
    fs::write(temp.path().join("src/lib.rs"), source).unwrap();
    let mut extractor = SymbolExtractor::new()
        .unwrap()
        .with_workspace_root(temp.path());
    let extracted = extractor
        .extract_with_edges_from_source(Language::Rust, "src/lib.rs", source)
        .unwrap();
    let calls = extracted
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .collect::<Vec<_>>();
    assert!(!calls.is_empty());
    assert!(
        calls
            .iter()
            .all(|edge| edge.target_symbol_id.is_none() && edge.target_file_path.is_none())
    );
}

#[test]
fn rust_parameter_type_annotations_do_not_shadow_associated_calls() {
    for source in [
        "struct Buf;impl Buf{fn new(){}} fn run(value:Buf){Buf::new();}",
        "struct Buf;impl Buf{fn new(){}} fn run(value:Buf){let _=|other:Buf|{Buf::new();};}",
    ] {
        let extracted = extract(source);
        let constructor = extracted
            .symbols
            .iter()
            .find(|symbol| symbol.qualified_name == "Buf::new")
            .unwrap();
        let run = extracted
            .symbols
            .iter()
            .find(|symbol| symbol.qualified_name == "run")
            .unwrap();
        assert!(
            extracted
                .edges
                .iter()
                .any(|edge| edge.edge_kind == EdgeKind::Calls
                    && edge.source_id == run.id
                    && edge.target_symbol_id.as_deref() == Some(constructor.id.as_str()))
        );
    }
}

#[test]
fn rust_comments_neither_hide_attached_attributes_nor_create_attributes() {
    for comment in ["// explanation\n", "/* explanation */"] {
        for declaration in [
            format!("#[opaque]\n{comment}fn helper(){{}} fn run(){{helper();}}"),
            format!(
                "#[cfg(any())]\n{comment}mod dep{{pub fn helper(){{}}}} fn run(){{dep::helper();}}"
            ),
            format!(
                "struct Buf;#[opaque]\n{comment}impl Buf{{fn new(){{}}}} fn run(){{Buf::new();}}"
            ),
        ] {
            let extracted = extract(&declaration);
            let calls = extracted
                .edges
                .iter()
                .filter(|edge| edge.edge_kind == EdgeKind::Calls)
                .collect::<Vec<_>>();
            assert!(!calls.is_empty());
            assert!(calls.iter().all(|edge| edge.target_symbol_id.is_none()));
        }
    }
    for comment in ["// #[cfg(any())]\n", "/* #[opaque] */"] {
        let extracted = extract(&format!("{comment}fn helper(){{}} fn run(){{helper();}}"));
        let helper = extracted
            .symbols
            .iter()
            .find(|symbol| symbol.qualified_name == "helper")
            .unwrap();
        assert!(
            extracted
                .edges
                .iter()
                .any(|edge| edge.edge_kind == EdgeKind::Calls
                    && edge.target_symbol_id.as_deref() == Some(helper.id.as_str()))
        );
    }

    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname='fixture'\nversion='0.1.0'\n",
    )
    .unwrap();
    for comment in ["// explanation\n", "/* explanation */"] {
        for (source, target) in [
            (
                format!(
                    "#[cfg(any())]\n{comment}mod dep;use crate::dep::helper;fn run(){{helper();}}"
                ),
                "pub fn helper(){}".to_owned(),
            ),
            (
                "mod dep;use crate::dep::helper;fn run(){helper();}".to_owned(),
                format!("#[opaque]\n{comment}pub fn helper(){{}}"),
            ),
        ] {
            fs::write(temp.path().join("src/lib.rs"), &source).unwrap();
            fs::write(temp.path().join("src/dep.rs"), target).unwrap();
            let mut extractor = SymbolExtractor::new()
                .unwrap()
                .with_workspace_root(temp.path());
            let extracted = extractor
                .extract_with_edges_from_source(Language::Rust, "src/lib.rs", &source)
                .unwrap();
            let calls = extracted
                .edges
                .iter()
                .filter(|edge| edge.edge_kind == EdgeKind::Calls)
                .collect::<Vec<_>>();
            assert!(!calls.is_empty());
            assert!(
                calls
                    .iter()
                    .all(|edge| edge.target_symbol_id.is_none() && edge.target_file_path.is_none())
            );
        }
    }
}

fn call_id_for_owner(file: &ExtractedFile, owner: &str, target: &str) -> bool {
    let owner = file
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == owner)
        .unwrap();
    let target = file
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == target)
        .unwrap();
    file.edges.iter().any(|edge| {
        edge.edge_kind == EdgeKind::Calls
            && edge.source_id == owner.id
            && edge.target_symbol_id.as_deref() == Some(target.id.as_str())
    })
}

#[test]
fn rust_explicit_nominal_receivers_and_tuple_constructors_keep_local_ids() {
    let file = extract(
        "struct Square;impl Square{fn area(&self){} fn forward(&self){self.area();}} fn measure(s:Square){s.area();} fn make(){let s=Square;s.area();} fn borrowed(s:&Square){s.area();} struct Wrap(u8);fn build(){Wrap(1);}",
    );
    for owner in ["measure", "make", "Square::forward"] {
        assert!(
            call_id_for_owner(&file, owner, "Square::area"),
            "{owner}: {:?}",
            file.edges
        );
    }
    let borrowed = file
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "borrowed")
        .unwrap();
    assert!(
        file.edges
            .iter()
            .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_id == borrowed.id)
            .all(|edge| edge.target_symbol_id.is_none())
    );
    assert!(call_id_for_owner(&file, "build", "Wrap"));
}

#[test]
fn rust_unknown_mutable_shadowed_or_generic_receivers_stay_unresolved() {
    let prefix = "struct Square;impl Square{fn area(&self){}}";
    for body in [
        "fn run(mut s:Square){s.area();}",
        "fn run(s:Square){let s=unknown();s.area();}",
        "fn run(s:Square){s=unknown();s.area();}",
        "fn run(s:Square){let _=|s|{s.area();};}",
        "fn run(s:Square){for s in []{s.area();}}",
        "fn run<T>(s:T){s.area();}",
        "fn run(){let s=make();s.area();}",
        "fn run(){let (s,)=(Square,);s.area();}",
    ] {
        let file = extract(&format!("{prefix}{body}"));
        let methods = file
            .edges
            .iter()
            .filter(|edge| {
                edge.edge_kind == EdgeKind::Calls && edge.target_qualified_name.ends_with("area")
            })
            .collect::<Vec<_>>();
        assert!(!methods.is_empty(), "{body}");
        assert!(
            methods
                .iter()
                .all(|edge| edge.target_symbol_id.is_none() && edge.target_file_path.is_none()),
            "{body}: {methods:?}"
        );
    }
    for source in [
        "struct Square;impl Square{fn area(){}} fn run(s:Square){s.area();}",
        "struct Square;impl Square{fn area(&mut self){}} fn run(s:Square){s.area();}",
        "struct Square;impl Square{fn area(&self){} fn area(&self,x:u8){}} fn run(s:Square){s.area();}",
        "struct Square;trait Measure{fn area(self);}impl Measure for Square{fn area(self){}}impl Square{fn area(&self){}}fn run(s:Square){s.area();}",
        "struct Square<T>(T);impl<T> Square<T>{fn area(&self){}}fn run(s:Square<u8>){s.area();}",
        "struct Square;impl Square{#[opaque] /* doc */ fn area(&self){}}fn run(s:Square){s.area();}",
    ] {
        let file = extract(source);
        let methods = file
            .edges
            .iter()
            .filter(|edge| {
                edge.edge_kind == EdgeKind::Calls && edge.target_qualified_name.ends_with("area")
            })
            .collect::<Vec<_>>();
        assert!(!methods.is_empty());
        assert!(
            methods.iter().all(|edge| edge.target_symbol_id.is_none()),
            "{source}: {methods:?}"
        );
    }
}

#[test]
fn rust_imported_nominal_receivers_retain_files_aliases_and_refresh_reads() {
    let temp = tempfile::tempdir().unwrap();
    let source = "mod dep;use crate::dep::Buf as Input;fn run(buf:Input){buf.len();}fn make(){let buf=Input;buf.len();}fn unknown<T>(buf:T){buf.len();}";
    for project in ["alpha", "beta"] {
        fs::create_dir_all(temp.path().join(project).join("src")).unwrap();
        fs::write(
            temp.path().join(project).join("Cargo.toml"),
            format!("[package]\nname='{project}'\nversion='0.1.0'\n"),
        )
        .unwrap();
        fs::write(temp.path().join(project).join("src/lib.rs"), source).unwrap();
        fs::write(
            temp.path().join(project).join("src/dep.rs"),
            "pub struct Buf;impl Buf{pub fn len(&self)->usize{0}}",
        )
        .unwrap();
    }
    let mut extractor = SymbolExtractor::new()
        .unwrap()
        .with_workspace_root(temp.path());
    for project in ["alpha", "beta"] {
        let file = extractor
            .extract_with_edges_from_source(
                Language::Rust,
                &format!("{project}/src/lib.rs"),
                source,
            )
            .unwrap();
        for owner in ["run", "make"] {
            let owner = file
                .symbols
                .iter()
                .find(|symbol| symbol.qualified_name == owner)
                .unwrap();
            let calls = file
                .edges
                .iter()
                .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_id == owner.id)
                .collect::<Vec<_>>();
            assert_eq!(calls.len(), 1);
            assert_eq!(
                calls[0].target_file_path.as_deref(),
                Some(format!("{project}/src/dep.rs").as_str())
            );
            assert_eq!(calls[0].target_qualified_name, "Buf::len");
        }
        let unknown = file
            .symbols
            .iter()
            .find(|symbol| symbol.qualified_name == "unknown")
            .unwrap();
        assert!(
            file.edges
                .iter()
                .filter(|edge| edge.source_id == unknown.id && edge.edge_kind == EdgeKind::Calls)
                .all(|edge| edge.target_symbol_id.is_none() && edge.target_file_path.is_none())
        );
    }
    let stale = extractor.refresh_workspace_file("alpha/src/dep.rs");
    assert!(stale.contains(&"alpha/src/lib.rs".to_owned()));
    assert!(!stale.contains(&"beta/src/lib.rs".to_owned()));
    for target in [
        "pub struct Buf;impl Buf{pub fn len(&mut self)->usize{0}}",
        "pub struct Buf;trait SizedBuf{fn len(&self)->usize;}impl SizedBuf for Buf{fn len(&self)->usize{0}}",
        "pub struct Buf;impl Buf{#[opaque] /* explanation */ pub fn len(&self)->usize{0}}",
    ] {
        fs::write(temp.path().join("alpha/src/dep.rs"), target).unwrap();
        let file = extractor
            .extract_with_edges_from_source(Language::Rust, "alpha/src/lib.rs", source)
            .unwrap();
        assert!(
            file.edges
                .iter()
                .filter(|edge| edge.edge_kind == EdgeKind::Calls)
                .all(|edge| edge.target_symbol_id.is_none() && edge.target_file_path.is_none())
        );
    }
    fs::remove_file(temp.path().join("alpha/src/dep.rs")).unwrap();
    let file = extractor
        .extract_with_edges_from_source(Language::Rust, "alpha/src/lib.rs", source)
        .unwrap();
    assert!(
        file.edges
            .iter()
            .filter(|edge| edge.edge_kind == EdgeKind::Calls)
            .all(|edge| edge.target_symbol_id.is_none() && edge.target_file_path.is_none())
    );
}

#[test]
fn rust_receiver_trait_dispatch_candidates_keep_raw_calls() {
    for source in [
        "struct Buf;impl Buf{fn into(&self){}}fn run(buf:Buf){let _:Buf=buf.into();}",
        "mod traits{pub trait Consume{fn alpha(self);}}use crate::traits::Consume;struct Buf;impl Buf{fn alpha(&self){}}fn run(buf:Buf){buf.alpha();}",
        "mod traits{pub trait Consume{fn alpha(self);}}use crate::traits::*;struct Buf;impl Buf{fn alpha(&self){}}fn run(buf:Buf){buf.alpha();}",
        "trait Consume{fn alpha(self);}struct Buf;impl Buf{fn alpha(&self){}}fn run(buf:Buf){buf.alpha();}",
        "struct Buf;impl Buf{fn alpha(&self){}}fn run(buf:&Buf){buf.alpha();}",
    ] {
        let file = extract(source);
        let run = file
            .symbols
            .iter()
            .find(|symbol| symbol.qualified_name == "run")
            .unwrap();
        let calls = file
            .edges
            .iter()
            .filter(|edge| edge.source_id == run.id && edge.edge_kind == EdgeKind::Calls)
            .collect::<Vec<_>>();
        assert!(!calls.is_empty());
        assert!(
            calls
                .iter()
                .all(|edge| edge.target_symbol_id.is_none() && edge.target_file_path.is_none()),
            "{source}: {calls:?}"
        );
    }
}

#[test]
fn rust_module_boundaries_require_explicit_self_or_super_paths() {
    let file = extract(
        "fn helper(){}struct Buf;impl Buf{fn new(){}}mod child{fn run(){helper();Buf::new();}fn explicit(){super::helper();}fn own(){}fn local(){self::own();}}",
    );
    let run = file
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "child::run")
        .unwrap();
    let calls = file
        .edges
        .iter()
        .filter(|edge| edge.source_id == run.id && edge.edge_kind == EdgeKind::Calls)
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 2);
    assert!(calls.iter().all(|edge| edge.target_symbol_id.is_none()));
    assert!(call_id_for_owner(&file, "child::explicit", "helper"));
    assert!(call_id_for_owner(&file, "child::local", "child::own"));
}

#[test]
fn rust_error_recovery_never_assigns_local_target_proofs() {
    let source = "fn helper(){}fn run(){helper();let = ;}";
    let file = extract(source);
    let calls = file
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .collect::<Vec<_>>();
    assert!(!calls.is_empty(), "recoverable raw call remains recorded");
    assert!(
        calls
            .iter()
            .all(|edge| edge.target_symbol_id.is_none() && edge.target_file_path.is_none())
    );
    let source = "fn helper(){}fn run(){unsafe extern \"C\"{fn helper();}unsafe{helper();}}";
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .unwrap();
    let tree = parser.parse(source, None).unwrap();
    assert!(
        tree.root_node().has_error(),
        "fixture exercises pinned grammar recovery"
    );
    let file = extract(source);
    assert!(
        file.edges
            .iter()
            .all(|edge| edge.target_symbol_id.is_none() && edge.target_file_path.is_none())
    );
}

#[test]
fn rust_explicit_imported_static_inherent_functions_keep_file_proof() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname='fixture'\nversion='0.1.0'\n",
    )
    .unwrap();
    let source = "mod dep;use crate::dep::Store;fn run(){Store::alpha();}";
    fs::write(temp.path().join("src/lib.rs"), source).unwrap();
    fs::write(
        temp.path().join("src/dep.rs"),
        "pub struct Store;impl Store{pub fn alpha(){}}",
    )
    .unwrap();
    let mut extractor = SymbolExtractor::new()
        .unwrap()
        .with_workspace_root(temp.path());
    let file = extractor
        .extract_with_edges_from_source(Language::Rust, "src/lib.rs", source)
        .unwrap();
    assert!(
        file.edges
            .iter()
            .any(|edge| edge.edge_kind == EdgeKind::Calls
                && edge.target_file_path.as_deref() == Some("src/dep.rs")
                && edge.target_qualified_name == "Store::alpha")
    );
}

#[test]
fn rust_statement_macros_cannot_supply_hidden_lexical_or_trait_bindings() {
    for source in [
        "macro_rules! make{()=>{trait Consume{fn alpha(self);}impl Consume for Buf{fn alpha(self){}}}}struct Buf;impl Buf{fn alpha(&self){}}make!();fn run(buf:Buf){buf.alpha();}",
        "macro_rules! shadow{()=>{fn helper(){}}}fn helper(){}fn run(){shadow!();helper();}",
        "macro_rules! shadow{()=>{struct Buf;impl Buf{fn new(){}}}}struct Buf;impl Buf{fn new(){}}fn run(){shadow!();Buf::new();}",
        "macro_rules! members{()=>{}}struct Buf;impl Buf{members!();fn alpha(&self){}}fn run(buf:Buf){buf.alpha();}",
        "macro_rules! methods{()=>{fn alpha(self){}}}trait Consume{methods!();}struct Buf;impl Consume for Buf{}impl Buf{fn alpha(&self){}}fn run(buf:Buf){buf.alpha();}",
    ] {
        let file = extract(source);
        let run = file
            .symbols
            .iter()
            .find(|symbol| symbol.qualified_name == "run")
            .unwrap();
        let calls = file
            .edges
            .iter()
            .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_id == run.id)
            .collect::<Vec<_>>();
        assert!(!calls.is_empty());
        assert!(
            calls
                .iter()
                .all(|edge| edge.target_symbol_id.is_none() && edge.target_file_path.is_none()),
            "{source}: {calls:?}"
        );
    }
    // A macro inside a different method's body cannot inject a sibling binding
    // into the caller's scope. This exact lexical case remains useful.
    let file = extract(
        "struct Buf;impl Buf{fn len(&self){println!(\"body\");}}fn run(buf:Buf){buf.len();}",
    );
    assert!(call_id_for_owner(&file, "run", "Buf::len"));
}

#[test]
fn rust_attributed_callers_keep_raw_local_and_imported_references() {
    for source in [
        "struct Buf;fn helper(){}#[rewrite]fn run(x:Buf){helper();}",
        "fn helper(){}#[rewrite] /* explanation */ mod child{fn run(){super::helper();}}",
    ] {
        let file = extract(source);
        let calls = file
            .edges
            .iter()
            .filter(|edge| edge.edge_kind == EdgeKind::Calls)
            .collect::<Vec<_>>();
        assert!(!calls.is_empty());
        assert!(
            calls
                .iter()
                .all(|edge| edge.target_symbol_id.is_none() && edge.target_file_path.is_none())
        );
        assert!(
            file.edges
                .iter()
                .all(|edge| edge.edge_kind != EdgeKind::TypeRef)
        );
    }
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname='fixture'\nversion='0.1.0'\n",
    )
    .unwrap();
    let source = "mod dep;use crate::dep::helper as imported;#[rewrite] // explanation\nfn run(){imported();}";
    fs::write(temp.path().join("src/lib.rs"), source).unwrap();
    fs::write(temp.path().join("src/dep.rs"), "pub fn helper(){}").unwrap();
    let mut extractor = SymbolExtractor::new()
        .unwrap()
        .with_workspace_root(temp.path());
    let file = extractor
        .extract_with_edges_from_source(Language::Rust, "src/lib.rs", source)
        .unwrap();
    let calls = file
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .collect::<Vec<_>>();
    assert!(!calls.is_empty());
    assert!(
        calls
            .iter()
            .all(|edge| edge.target_symbol_id.is_none() && edge.target_file_path.is_none())
    );
}

#[test]
fn rust_bare_names_skip_associated_item_scopes() {
    let file = extract(
        "fn helper(){}struct Buf;impl Buf{fn helper(){}fn run(){helper();Self::helper();}}",
    );
    assert!(call_id_for_owner(&file, "Buf::run", "helper"));
    assert!(call_id_for_owner(&file, "Buf::run", "Buf::helper"));
}

#[test]
fn rust_unanchored_use_paths_withhold_edition_and_root_dependent_proofs() {
    for source in [
        "mod helpers{pub fn helper()->u8{0}}mod inner{mod helpers{pub fn helper()->u16{0}}use helpers::helper as alias;fn run(){alias();}}",
        "mod helpers{pub fn helper()->u8{0}}fn run(){mod helpers{pub fn helper()->u16{0}}use helpers::helper as alias;alias();}",
        "mod helpers{pub fn helper(){}}use helpers::helper as alias;fn run(){alias();}",
    ] {
        let file = extract(source);
        let calls = file
            .edges
            .iter()
            .filter(|edge| edge.edge_kind == EdgeKind::Calls)
            .collect::<Vec<_>>();
        assert!(!calls.is_empty());
        assert!(
            calls
                .iter()
                .all(|edge| edge.target_symbol_id.is_none() && edge.target_file_path.is_none())
        );
    }
    let source = "mod helpers{pub fn helper(){}}mod inner{mod helpers{pub fn helper(){}}use self::helpers::helper as alias;fn run(){alias();}}";
    let file = extract(source);
    assert!(call_id_for_owner(
        &file,
        "inner::run",
        "inner::helpers::helper"
    ));
}

fn assert_raw_call(file: &ExtractedFile, owner: &str, raw: &str) {
    let owner = file
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == owner)
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
    assert!(
        !calls.is_empty(),
        "raw call {raw} missing: {:?}",
        file.edges
    );
    assert!(
        calls
            .iter()
            .all(|edge| edge.target_symbol_id.is_none() && edge.target_file_path.is_none()),
        "{calls:?}"
    );
}

#[test]
fn rust_attributed_named_imports_own_their_names_without_local_proof() {
    // Both programs compile: the disabled import calls the outer helper, while
    // the enabled import calls dep::helper. Without cfg context neither is proof.
    for attribute in [
        "#[cfg(any())]",
        "#[cfg(all())]",
        "#[allow(unused_imports)]",
        "#[cfg(any())] // attached comment\n",
        "#[cfg(all())] /* attached comment */",
    ] {
        for (import, raw, outer) in [
            ("use self::dep::helper;", "helper", "fn helper()->u8{1}"),
            (
                "use self::dep::{helper as imported};",
                "imported",
                "fn imported()->u8{1}",
            ),
            (
                "use self::dep::{ /* binding explanation */ helper as imported};",
                "imported",
                "fn imported()->u8{1}",
            ),
        ] {
            let source = format!(
                "{outer} fn stable(){{}} mod dep{{pub fn helper()->u8{{2}}}} fn run(){{{attribute} {import} {raw}();stable();}}"
            );
            let file = extract(&source);
            assert_raw_call(&file, "run", raw);
            assert!(
                call_id_for_owner(&file, "run", "stable"),
                "unrelated binding lost: {source}"
            );
            assert!(
                file.edges
                    .iter()
                    .any(|edge| edge.edge_kind == EdgeKind::DependsOn),
                "import dependency text lost: {source}"
            );
        }
    }
    let file = extract(
        "fn helper()->u8{1} mod dep{pub fn helper()->u8{2}} fn run(){use self::dep::helper;helper();}",
    );
    assert!(call_id_for_owner(&file, "run", "dep::helper"));
}

#[test]
fn rust_attributed_group_imports_withhold_type_and_implementation_proof() {
    for attribute in ["#[cfg(any())]", "#[cfg(all())] /* attached */"] {
        let source = format!(
            "struct Data; trait Mark{{}} fn imported(){{}} mod dep{{pub struct Data;pub trait Mark{{}}pub fn helper(){{}}}} fn run(){{{attribute} use self::dep::{{Data,Mark,helper as imported}}; fn nested(_:Data){{imported();}} struct Local; impl Mark for Local{{}}}}"
        );
        let file = extract(&source);
        assert_raw_call(&file, "nested", "imported");
        assert!(
            file.edges
                .iter()
                .all(|edge| !matches!(edge.edge_kind, EdgeKind::TypeRef | EdgeKind::Implements)),
            "{source}: {:?}",
            file.edges
        );
    }
    let file = extract(
        "struct Data;trait Mark{}mod dep{pub struct Data;pub trait Mark{}}fn run(){use self::dep::{Data,Mark};fn nested(_:Data){}struct Local;impl Mark for Local{}}",
    );
    for (kind, target) in [
        (EdgeKind::TypeRef, "dep::Data"),
        (EdgeKind::Implements, "dep::Mark"),
    ] {
        assert!(
            file.edges.iter().any(|edge| edge.edge_kind == kind
                && edge.target_qualified_name == target
                && edge.target_symbol_id.is_some()),
            "positive control {target}: {:?}",
            file.edges
        );
    }
}

#[test]
fn rust_attributed_grouped_self_imports_do_not_fall_through_to_outer_modules() {
    let declarations =
        "mod dep{pub fn helper()->u8{1}}mod other{pub mod dep{pub fn helper()->u8{2}}}";
    for attribute in ["#[cfg(any())]", "#[cfg(all())] // attached\n"] {
        for import in [
            "use self::other::dep::{self};",
            "use self::other::dep::{self as imported};",
            "use self::other::{dep::{self}};",
        ] {
            let (outer, raw) = if import.contains(" as imported") {
                ("mod imported{pub fn helper()->u8{1}}", "imported::helper")
            } else {
                ("", "dep::helper")
            };
            let source = format!("{declarations}{outer}fn run(){{{attribute}{import}{raw}();}}");
            assert_raw_call(&extract(&source), "run", raw);
        }
    }
    for (import, raw) in [
        ("use self::other::dep::{self};", "dep::helper"),
        (
            "use self::other::dep::{self as imported};",
            "imported::helper",
        ),
    ] {
        let source = format!("{declarations}fn run(){{{import}{raw}();}}");
        assert!(
            call_id_for_owner(&extract(&source), "run", "other::dep::helper"),
            "{source}"
        );
    }
}

#[test]
fn rust_attributed_workspace_imports_withhold_file_proof_and_keep_other_names() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src/remote")).unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname='fixture'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(temp.path().join("src/remote.rs"), "pub mod dep;").unwrap();
    fs::write(
        temp.path().join("src/remote/dep.rs"),
        "pub fn helper()->u8{2}",
    )
    .unwrap();
    for (import, raw, outer) in [
        (
            "use crate::remote::dep::helper;",
            "helper",
            "fn helper()->u8{1}",
        ),
        (
            "use crate::remote::dep::{helper as imported};",
            "imported",
            "fn imported()->u8{1}",
        ),
        (
            "use crate::remote::dep::{self};",
            "dep::helper",
            "mod dep{pub fn helper()->u8{1}}",
        ),
        (
            "use crate::remote::dep::{self as imported};",
            "imported::helper",
            "mod imported{pub fn helper()->u8{1}}",
        ),
    ] {
        for attribute in ["#[cfg(any())]", "#[cfg(all())] /* attached */"] {
            let source = format!(
                "mod remote;{outer}fn stable(){{}}fn run(){{{attribute}{import}{raw}();stable();}}"
            );
            fs::write(temp.path().join("src/lib.rs"), &source).unwrap();
            let mut extractor = SymbolExtractor::new()
                .unwrap()
                .with_workspace_root(temp.path());
            let file = extractor
                .extract_with_edges_from_source(Language::Rust, "src/lib.rs", &source)
                .unwrap();
            assert_raw_call(&file, "run", raw);
            assert!(call_id_for_owner(&file, "run", "stable"), "{source}");
        }
        let source = format!("mod remote;{outer}fn run(){{{import}{raw}();}}");
        fs::write(temp.path().join("src/lib.rs"), &source).unwrap();
        let mut extractor = SymbolExtractor::new()
            .unwrap()
            .with_workspace_root(temp.path());
        let file = extractor
            .extract_with_edges_from_source(Language::Rust, "src/lib.rs", &source)
            .unwrap();
        assert!(
            file.edges
                .iter()
                .any(|edge| edge.edge_kind == EdgeKind::Calls
                    && edge.target_qualified_name == "helper"
                    && edge.target_file_path.as_deref() == Some("src/remote/dep.rs")),
            "unconditional import lost: {source}: {:?}",
            file.edges
        );
    }
}

#[test]
fn rust_use_comments_do_not_look_like_globs_but_real_wildcards_stay_uncertain() {
    let file = extract(
        "mod support{pub struct Other;}use self::support::{ /* not a wildcard */ Other};struct Buf;impl Buf{fn work(&self){}}fn run(buf:Buf){buf.work();}",
    );
    assert!(call_id_for_owner(&file, "run", "Buf::work"));
    let file = extract(
        "mod support{pub struct Other;}use self::support::*;struct Buf;impl Buf{fn work(&self){}}fn run(buf:Buf){buf.work();}",
    );
    assert_raw_call(&file, "run", "work");
}

#[test]
fn rust_expanding_import_attributes_withhold_entire_scope_proof() {
    // The attribute can replace the helper import with a new stable function.
    // This is deliberately broader than uncertainty about the imported name.
    for attribute in [
        "#[binding_rewriter::replace]",
        "#[replace]",
        "#[cfg_attr(all(), binding_rewriter::replace)]",
        "#[cfg_attr(any(), binding_rewriter::replace)]",
        "#[binding_rewriter::replace] #[allow(unused_imports)]",
        "#[allow(unused_imports)] /* between attributes */ #[cfg_attr(all(), binding_rewriter::replace)] // attached\n",
    ] {
        for import in [
            "use self::dep::helper;",
            "use self::dep::{helper as imported};",
        ] {
            let source = format!(
                "struct Data;trait Mark{{}}fn stable()->u8{{1}}mod dep{{pub fn helper(){{}}}}fn run(){{{attribute}{import}stable();fn nested(_:Data){{}}struct Local;impl Mark for Local{{}}}}"
            );
            let file = extract(&source);
            assert_raw_call(&file, "run", "stable");
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

#[test]
fn rust_known_gating_and_inert_use_attributes_keep_unrelated_proofs() {
    for attribute in [
        "#[cfg(any())]",
        "#[cfg(all())]",
        "#[allow(unused_imports)]",
        "#[warn(unused_imports)]",
        "#[deny(unused_imports)]",
        "#[forbid(unused_imports)]",
        "#[expect(unused_imports)]",
        "#[doc = \"import documentation\"]",
    ] {
        let source = format!(
            "struct Data;trait Mark{{}}fn helper(){{}}fn stable()->u8{{1}}mod dep{{pub fn helper(){{}}}}fn run(){{{attribute}use self::dep::helper;helper();stable();fn nested(_:Data){{}}struct Local;impl Mark for Local{{}}}}"
        );
        let file = extract(&source);
        assert_raw_call(&file, "run", "helper");
        assert!(call_id_for_owner(&file, "run", "stable"), "{source}");
        for (kind, target) in [(EdgeKind::TypeRef, "Data"), (EdgeKind::Implements, "Mark")] {
            assert!(
                file.edges.iter().any(|edge| edge.edge_kind == kind
                    && edge.target_qualified_name == target
                    && edge.target_symbol_id.is_some()),
                "{source}: missing unrelated {target}: {:?}",
                file.edges
            );
        }
    }
}

#[test]
fn rust_expanding_import_attributes_block_unrelated_workspace_file_proof() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname='fixture'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(temp.path().join("src/dep.rs"), "pub fn helper(){}").unwrap();
    fs::write(temp.path().join("src/remote.rs"), "pub fn stable()->u8{1}").unwrap();
    for attribute in [
        "#[binding_rewriter::replace]",
        "#[cfg_attr(all(), binding_rewriter::replace)]",
        "#[cfg_attr(any(), binding_rewriter::replace)]",
        "#[binding_rewriter::replace] /* comment */ #[allow(unused_imports)]",
    ] {
        let source = format!(
            "mod dep;mod remote;use crate::remote::stable;fn run(){{{attribute}use crate::dep::{{helper as imported}};stable();}}"
        );
        fs::write(temp.path().join("src/lib.rs"), &source).unwrap();
        let mut extractor = SymbolExtractor::new()
            .unwrap()
            .with_workspace_root(temp.path());
        let file = extractor
            .extract_with_edges_from_source(Language::Rust, "src/lib.rs", &source)
            .unwrap();
        assert_raw_call(&file, "run", "stable");
    }
    for attribute in [
        "",
        "#[cfg(any())]",
        "#[cfg(all())]",
        "#[allow(unused_imports)]",
    ] {
        let source = format!(
            "mod dep;mod remote;use crate::remote::stable;fn run(){{{attribute}use crate::dep::helper;stable();}}"
        );
        fs::write(temp.path().join("src/lib.rs"), &source).unwrap();
        let mut extractor = SymbolExtractor::new()
            .unwrap()
            .with_workspace_root(temp.path());
        let file = extractor
            .extract_with_edges_from_source(Language::Rust, "src/lib.rs", &source)
            .unwrap();
        assert!(
            file.edges
                .iter()
                .any(|edge| edge.edge_kind == EdgeKind::Calls
                    && edge.target_qualified_name == "stable"
                    && edge.target_symbol_id.is_none()
                    && edge.target_file_path.as_deref() == Some("src/remote.rs")),
            "unrelated file proof lost: {source}: {:?}",
            file.edges
        );
    }
}
