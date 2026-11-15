//! A normal startup must replace stale parser proof even when no source changed.
//! SQL-format upgrades and old-writer fencing have separate store regressions.

use std::collections::BTreeSet;
use std::fs;

use aether_core::{EdgeKind, SymbolEdge};
use aether_store::{
    SirHistoryStore, SirStateStore, SqliteStore, SymbolCatalogStore, SymbolRelationStore,
};

use super::index_once_tests::index_once;

fn refreshes_unchanged_attributed_import(external: bool, condition: &str) {
    let temp = tempfile::tempdir().expect("workspace");
    let workspace = temp.path();
    fs::create_dir_all(workspace.join(".aether")).expect("configuration directory");
    fs::create_dir_all(workspace.join("src")).expect("source directory");
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[inference]
provider = "mock"
[storage]
mirror_sir_files = true
graph_backend = "sqlite"
[embeddings]
enabled = false
vector_backend = "sqlite"
"#,
    )
    .expect("mock configuration");
    fs::write(
        workspace.join("Cargo.toml"),
        "[package]\nname = \"attributed_import_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    ).expect("conventional crate root");
    let (module, import, target_path, target_name) = if external {
        fs::write(workspace.join("src/dep.rs"), "pub fn helper() {}\n").expect("external target");
        ("mod dep;", "crate::dep::helper", "src/dep.rs", "helper")
    } else {
        (
            "mod dep { pub fn helper() {} }",
            "self::dep::helper",
            "src/lib.rs",
            "dep::helper",
        )
    };
    let source = format!(
        "fn helper() {{}}\n{module}\nfn stable() {{}}\nfn run() {{ #[cfg({condition})] use {import}; helper(); stable(); }}\n"
    );
    fs::write(workspace.join("src/lib.rs"), &source).expect("unchanging source");
    index_once(workspace, true);

    let store = SqliteStore::open(workspace).expect("indexed catalog");
    let declarations = store
        .list_symbols_for_file("src/lib.rs")
        .expect("declarations");
    let named = |name: &str| {
        declarations
            .iter()
            .find(|symbol| symbol.qualified_name == name)
            .unwrap_or_else(|| panic!("missing declaration {name}"))
    };
    let run = named("run");
    let stable = named("stable");
    let target = store
        .list_symbols_for_file(target_path)
        .expect("target declarations")
        .into_iter()
        .find(|symbol| symbol.qualified_name == target_name)
        .expect("wrong legacy target");
    store
        .insert_intent_contract(
            &run.id,
            "must",
            "Keep this declaration's contract during structural refresh",
            None,
            "regression",
        )
        .expect("contract");
    assert!(
        store.read_sir_blob(&run.id).expect("run SIR").is_some(),
        "initial full mock indexing must create a summary"
    );

    let ids = store
        .list_all_symbol_ids()
        .expect("identities")
        .into_iter()
        .collect::<BTreeSet<_>>();
    let evidence = |catalog: &SqliteStore| {
        ids.iter()
            .map(|id| {
                let mirror_path = catalog.sir_dir().join(format!("{id}.json"));
                let mirror = mirror_path
                    .exists()
                    .then(|| fs::read_to_string(mirror_path).expect("read existing mirror"));
                (
                    id.clone(),
                    catalog.read_sir_blob(id).expect("canonical summary"),
                    catalog.get_sir_meta(id).expect("summary metadata"),
                    catalog.list_sir_history(id).expect("summary history"),
                    catalog
                        .get_sir_source_hash(id)
                        .expect("summary source hash"),
                    catalog
                        .list_intent_contracts_for_symbol(id, true)
                        .expect("all contracts"),
                    mirror,
                )
            })
            .collect::<Vec<_>>()
    };
    let before = evidence(&store);

    // Reproduce the old parser's false content through the public edge writer.
    // This is deliberately not a SQL migration fixture: the store suite tests
    // the real old shape. Here even a current-format stale proof must be replaced
    // by a normal index, despite matching source hashes and stable symbol IDs.
    let mut old_edges = store
        .list_symbol_edges_for_source_and_kinds(
            &run.id,
            &[
                EdgeKind::Calls,
                EdgeKind::DependsOn,
                EdgeKind::TypeRef,
                EdgeKind::Implements,
            ],
        )
        .expect("current raw structural references");
    old_edges
        .retain(|edge| edge.edge_kind != EdgeKind::Calls || edge.target_qualified_name != "helper");
    old_edges.push(SymbolEdge {
        source_id: run.id.clone(),
        target_qualified_name: target.qualified_name.clone(),
        edge_kind: EdgeKind::Calls,
        file_path: "src/lib.rs".to_owned(),
        target_symbol_id: (!external).then(|| target.id.clone()),
        target_file_path: external.then(|| target_path.to_owned()),
    });
    store
        .delete_edges_for_file("src/lib.rs")
        .expect("replace legacy source edges");
    store.upsert_edges(&old_edges).expect("seed stale proof");
    store
        .populate_symbol_neighbors("src/lib.rs")
        .expect("seed stale projection");
    assert_eq!(
        store
            .get_symbol_neighbors_by_type(&run.id, "calls")
            .expect("stale neighbors")
            .into_iter()
            .map(|neighbor| neighbor.neighbor_id)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([target.id.clone(), stable.id.clone()]),
        "fixture must retain the valid call alongside its false projected call before refresh"
    );
    assert_eq!(evidence(&store), before);
    drop(store);

    for _ in 0..2 {
        index_once(workspace, false);
        assert_eq!(
            fs::read_to_string(workspace.join("src/lib.rs")).expect("source after startup"),
            source
        );
        let refreshed = SqliteStore::open(workspace).expect("refreshed catalog");
        assert_eq!(
            refreshed
                .list_all_symbol_ids()
                .expect("stable identities")
                .into_iter()
                .collect::<BTreeSet<_>>(),
            ids
        );
        assert_eq!(
            evidence(&refreshed),
            before,
            "structural-only refresh must retain exact SIR and contract ownership"
        );
        let raw = refreshed
            .list_symbol_edges_for_source_and_kinds(&run.id, &[EdgeKind::Calls])
            .expect("raw calls");
        assert_eq!(raw.len(), 2);
        let uncertain = raw
            .iter()
            .find(|edge| edge.target_qualified_name == "helper")
            .expect("raw attributed reference retained");
        assert!(uncertain.target_symbol_id.is_none() && uncertain.target_file_path.is_none());
        let valid = raw
            .iter()
            .find(|edge| edge.target_qualified_name == "stable")
            .expect("unrelated supported call retained");
        assert_eq!(valid.target_symbol_id.as_deref(), Some(stable.id.as_str()));
        assert_eq!(
            refreshed
                .get_symbol_neighbors_by_type(&run.id, "calls")
                .expect("refreshed neighbors")
                .into_iter()
                .map(|neighbor| neighbor.neighbor_id)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([stable.id.clone()])
        );
        assert!(
            !refreshed
                .list_callers_for_symbol(&target.id)
                .expect("target callers")
                .iter()
                .any(|caller| caller.id == run.id)
        );
        let coverage = refreshed
            .call_edge_coverage_for_sources(std::slice::from_ref(&run.id))
            .expect("explicit incomplete coverage");
        assert_eq!(
            (coverage.call_edges, coverage.resolved, coverage.unresolved),
            (2, 1, 1)
        );
    }
}

#[test]
fn normal_index_refreshes_stale_attributed_import_id_proof_on_unchanged_source() {
    for condition in ["any()", "all()"] {
        refreshes_unchanged_attributed_import(false, condition);
    }
}

#[test]
fn normal_index_refreshes_stale_attributed_import_file_proof_on_unchanged_source() {
    for condition in ["any()", "all()"] {
        refreshes_unchanged_attributed_import(true, condition);
    }
}
