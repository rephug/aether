//! Which methods' dependencies a type's SIR picks up: only methods that belong to
//! the selected type, never a same-name type's in another file or package.

use super::*;

/// Writes an empty package manifest so `dir` (workspace-relative) is a package root.
fn write_cargo_manifest(workspace: &Path, dir: &str) {
    let manifest = workspace.join(dir).join("Cargo.toml");
    fs::create_dir_all(manifest.parent().expect("manifest has a parent")).expect("create dir");
    fs::write(manifest, "[package]\nname = \"fixture\"\n").expect("write manifest");
}

fn calls(source_id: &str, target: &str, file_path: &str) -> SymbolEdge {
    SymbolEdge {
        source_id: source_id.to_owned(),
        target_qualified_name: target.to_owned(),
        edge_kind: EdgeKind::Calls,
        file_path: file_path.to_owned(),
    }
}

fn type_symbol(id: &str, qualified_name: &str, kind: SymbolKind, file_path: &str) -> Symbol {
    let name = qualified_name.rsplit("::").next().unwrap_or(qualified_name);
    demo_type_symbol(
        id,
        name,
        qualified_name,
        file_path,
        kind,
        "struct Placeholder;\n",
    )
}

/// Stores `records` and `edges`, then injects method dependencies into a fresh SIR
/// for `parent`.
fn inject_for(
    workspace: &Path,
    records: &[SymbolRecord],
    edges: &[SymbolEdge],
    parent: &Symbol,
) -> Option<HashMap<String, Vec<String>>> {
    let store = SqliteStore::open(workspace).expect("open store");
    for record in records {
        store.upsert_symbol(record.clone()).expect("upsert symbol");
    }
    store.upsert_edges(edges).expect("upsert edges");
    let pipeline = build_write_pipeline(workspace, Arc::new(PanicInferenceProvider));
    let mut sir = demo_sir();
    pipeline
        .inject_method_dependencies(&store, parent, &mut sir)
        .expect("inject method dependencies");
    sir.method_dependencies
}

fn deps(entries: &[(&str, &[&str])]) -> Option<HashMap<String, Vec<String>>> {
    Some(
        entries
            .iter()
            .map(|(method, targets)| {
                (
                    (*method).to_owned(),
                    targets.iter().map(|target| (*target).to_owned()).collect(),
                )
            })
            .collect(),
    )
}

#[test]
fn same_name_types_in_different_packages_keep_their_own_methods() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);
    write_cargo_manifest(workspace, "crates/a");
    write_cargo_manifest(workspace, "crates/b");

    let records = [
        demo_symbol_record_with_kind("a-store", "Store", "struct", "crates/a/src/store.rs"),
        demo_symbol_record_with_kind("a-load", "Store::load", "method", "crates/a/src/store.rs"),
        demo_symbol_record_with_kind("b-store", "Store", "struct", "crates/b/src/store.rs"),
        demo_symbol_record_with_kind("b-load", "Store::load", "method", "crates/b/src/store.rs"),
        demo_symbol_record_with_kind("b-save", "Store::save", "method", "crates/b/src/save.rs"),
    ];
    let edges = [
        calls("a-load", "Expected", "crates/a/src/store.rs"),
        calls("b-load", "UnrelatedSecret", "crates/b/src/store.rs"),
        calls("b-save", "OtherSecret", "crates/b/src/save.rs"),
    ];

    let a_store = type_symbol(
        "a-store",
        "Store",
        SymbolKind::Struct,
        "crates/a/src/store.rs",
    );
    assert_eq!(
        inject_for(workspace, &records, &edges, &a_store),
        deps(&[("load", &["Expected"])])
    );

    let b_store = type_symbol(
        "b-store",
        "Store",
        SymbolKind::Struct,
        "crates/b/src/store.rs",
    );
    assert_eq!(
        inject_for(workspace, &records, &edges, &b_store),
        deps(&[("load", &["UnrelatedSecret"]), ("save", &["OtherSecret"])])
    );
}

#[test]
fn cross_file_methods_of_a_type_unique_in_its_package_are_attributed() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);
    write_cargo_manifest(workspace, "crates/store");

    // One `impl` block per file, as with a large store type split across modules.
    let records = [
        demo_symbol_record_with_kind("store", "Store", "struct", "crates/store/src/lib.rs"),
        demo_symbol_record_with_kind("load", "Store::load", "method", "crates/store/src/load.rs"),
        demo_symbol_record_with_kind(
            "save",
            "Store::save",
            "method",
            "crates/store/src/nested/save.rs",
        ),
    ];
    let edges = [
        calls("load", "read_row", "crates/store/src/load.rs"),
        calls("save", "write_row", "crates/store/src/nested/save.rs"),
    ];

    let store = type_symbol(
        "store",
        "Store",
        SymbolKind::Struct,
        "crates/store/src/lib.rs",
    );
    assert_eq!(
        inject_for(workspace, &records, &edges, &store),
        deps(&[("load", &["read_row"]), ("save", &["write_row"])])
    );
}

#[test]
fn same_name_types_in_one_package_only_keep_same_file_methods() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);
    write_cargo_manifest(workspace, "crates/a");

    let records = [
        demo_symbol_record_with_kind("x-store", "Store", "struct", "crates/a/src/x.rs"),
        demo_symbol_record_with_kind("x-load", "Store::load", "method", "crates/a/src/x.rs"),
        demo_symbol_record_with_kind("y-store", "Store", "enum", "crates/a/src/y.rs"),
        demo_symbol_record_with_kind("y-load", "Store::load", "method", "crates/a/src/y.rs"),
        // Which `Store` this belongs to can't be told from the index.
        demo_symbol_record_with_kind("z-save", "Store::save", "method", "crates/a/src/z.rs"),
    ];
    let edges = [
        calls("x-load", "FromX", "crates/a/src/x.rs"),
        calls("y-load", "FromY", "crates/a/src/y.rs"),
        calls("z-save", "FromZ", "crates/a/src/z.rs"),
    ];

    let x_store = type_symbol("x-store", "Store", SymbolKind::Struct, "crates/a/src/x.rs");
    assert_eq!(
        inject_for(workspace, &records, &edges, &x_store),
        deps(&[("load", &["FromX"])])
    );
    let y_store = type_symbol("y-store", "Store", SymbolKind::Enum, "crates/a/src/y.rs");
    assert_eq!(
        inject_for(workspace, &records, &edges, &y_store),
        deps(&[("load", &["FromY"])])
    );
}

#[test]
fn nested_qualified_names_are_not_methods_of_the_type() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);

    let records = [
        demo_symbol_record_with_kind("store", "Store", "struct", "src/store.rs"),
        demo_symbol_record_with_kind("load", "Store::load", "method", "src/store.rs"),
        demo_symbol_record_with_kind("nested", "Store::load::nested", "function", "src/store.rs"),
    ];
    let edges = [
        calls("load", "Expected", "src/store.rs"),
        calls("nested", "NestedSecret", "src/store.rs"),
    ];

    let store = type_symbol("store", "Store", SymbolKind::Struct, "src/store.rs");
    assert_eq!(
        inject_for(workspace, &records, &edges, &store),
        deps(&[("load", &["Expected"])])
    );
}

#[test]
fn methods_in_another_language_are_not_attributed() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);

    let mut foreign =
        demo_symbol_record_with_kind("ts-load", "Store::load", "method", "src/store.ts");
    foreign.language = "typescript".to_owned();
    let records = [
        demo_symbol_record_with_kind("store", "Store", "struct", "src/store.rs"),
        demo_symbol_record_with_kind("load", "Store::load", "method", "src/store.rs"),
        foreign,
    ];
    let edges = [
        calls("load", "Expected", "src/store.rs"),
        calls("ts-load", "ScriptSecret", "src/store.ts"),
    ];

    let store = type_symbol("store", "Store", SymbolKind::Struct, "src/store.rs");
    assert_eq!(
        inject_for(workspace, &records, &edges, &store),
        deps(&[("load", &["Expected"])])
    );
}

#[test]
fn trait_implementor_fallback_requires_an_implements_edge() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);

    // `Alpha` sorts first and shares the trait's file, but doesn't implement it.
    let records = [
        demo_symbol_record_with_kind("trait", "Store", "trait", "src/lib.rs"),
        demo_symbol_record_with_kind("alpha", "Alpha", "struct", "src/lib.rs"),
        demo_symbol_record_with_kind("alpha-run", "Alpha::run", "method", "src/lib.rs"),
        demo_symbol_record_with_kind("beta", "Beta", "struct", "src/lib.rs"),
        demo_symbol_record_with_kind("beta-run", "Beta::run", "method", "src/lib.rs"),
    ];
    let trait_symbol = type_symbol("trait", "Store", SymbolKind::Trait, "src/lib.rs");

    let without_implementor = [
        calls("alpha-run", "AlphaDep", "src/lib.rs"),
        calls("beta-run", "BetaDep", "src/lib.rs"),
    ];
    assert_eq!(
        inject_for(workspace, &records, &without_implementor, &trait_symbol),
        None,
        "no same-file type implements the trait"
    );

    let with_implementor = [
        calls("alpha-run", "AlphaDep", "src/lib.rs"),
        calls("beta-run", "BetaDep", "src/lib.rs"),
        SymbolEdge {
            source_id: "beta".to_owned(),
            target_qualified_name: "Store".to_owned(),
            edge_kind: EdgeKind::Implements,
            file_path: "src/lib.rs".to_owned(),
        },
    ];
    assert_eq!(
        inject_for(workspace, &records, &with_implementor, &trait_symbol),
        deps(&[("run", &["BetaDep"])])
    );
}

#[test]
fn cross_file_methods_need_the_parent_in_the_catalog_and_inside_the_workspace() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);

    // The parent itself was never indexed, so the package can't vouch for it.
    let records = [
        demo_symbol_record_with_kind("load", "Store::load", "method", "src/store.rs"),
        demo_symbol_record_with_kind("save", "Store::save", "method", "src/save.rs"),
    ];
    let edges = [
        calls("load", "Expected", "src/store.rs"),
        calls("save", "CrossFile", "src/save.rs"),
    ];
    let store = type_symbol("store", "Store", SymbolKind::Struct, "src/store.rs");
    assert_eq!(
        inject_for(workspace, &records, &edges, &store),
        deps(&[("load", &["Expected"])])
    );

    // A path outside the workspace has no package, so it never matches one.
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);
    let records = [
        demo_symbol_record_with_kind("outside", "Outside", "struct", "../elsewhere/outside.rs"),
        demo_symbol_record_with_kind(
            "outside-load",
            "Outside::load",
            "method",
            "../elsewhere/load.rs",
        ),
    ];
    let edges = [calls("outside-load", "Escaped", "../elsewhere/load.rs")];
    let outside = type_symbol(
        "outside",
        "Outside",
        SymbolKind::Struct,
        "../elsewhere/outside.rs",
    );
    assert_eq!(inject_for(workspace, &records, &edges, &outside), None);
}

#[test]
fn trait_implementor_fallback_ignores_implements_edges_to_other_traits() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);

    let records = [
        demo_symbol_record_with_kind("trait", "Store", "trait", "src/lib.rs"),
        demo_symbol_record_with_kind("beta", "Beta", "struct", "src/lib.rs"),
        demo_symbol_record_with_kind("beta-run", "Beta::run", "method", "src/lib.rs"),
    ];
    let edges = [
        calls("beta-run", "BetaDep", "src/lib.rs"),
        SymbolEdge {
            source_id: "beta".to_owned(),
            target_qualified_name: "Loader".to_owned(),
            edge_kind: EdgeKind::Implements,
            file_path: "src/lib.rs".to_owned(),
        },
    ];
    let trait_symbol = type_symbol("trait", "Store", SymbolKind::Trait, "src/lib.rs");
    assert_eq!(inject_for(workspace, &records, &edges, &trait_symbol), None);
}
