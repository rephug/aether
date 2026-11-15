//! Unknown item attributes can introduce block bindings, not just change imports.
//! Exercise parser/store publication and an unchanged-source startup refresh before
//! removing and restoring the attribute; no procedural-macro dependency is needed.

use std::collections::BTreeSet;
use std::fs;

use aether_core::{EdgeKind, SymbolEdge};
use aether_store::{
    SirHistoryStore, SirStateStore, SqliteStore, SymbolCatalogStore, SymbolRelationStore,
};

use super::index_once_tests::index_once;

const LIB: &str = "src/lib.rs";
const ATTR: &str = "#[binding_rewriter::replace]";
const EDGE_KINDS: &[EdgeKind] = &[
    EdgeKind::Calls,
    EdgeKind::DependsOn,
    EdgeKind::TypeRef,
    EdgeKind::Implements,
];

fn source(external: bool, item: &str, attributed: bool) -> String {
    let target = if external {
        "mod dep; use crate::dep::stable;"
    } else {
        "fn stable() -> u8 { 1 }"
    };
    let attribute = if attributed { ATTR } else { "" };
    format!(
        "{target}\nfn run() -> u8 {{ {attribute} /* attached item */ {item} stable() }}\nfn control() -> u8 {{ stable() }}\n"
    )
}

fn neighbors(store: &SqliteStore, id: &str) -> BTreeSet<String> {
    store
        .get_symbol_neighbors_by_type(id, "calls")
        .expect("cached calls")
        .into_iter()
        .map(|neighbor| neighbor.neighbor_id)
        .collect()
}

fn assert_calls(
    store: &SqliteStore,
    run: &str,
    control: &str,
    target: &str,
    proved: bool,
    external: bool,
) {
    let calls = store
        .list_symbol_edges_for_source_and_kinds(run, &[EdgeKind::Calls])
        .expect("retained raw call");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].target_qualified_name, "stable");
    if proved {
        if external {
            assert!(calls[0].target_symbol_id.is_none());
            assert_eq!(calls[0].target_file_path.as_deref(), Some("src/dep.rs"));
        } else {
            assert_eq!(calls[0].target_symbol_id.as_deref(), Some(target));
            assert!(calls[0].target_file_path.is_none());
        }
        assert_eq!(neighbors(store, run), BTreeSet::from([target.to_owned()]));
    } else {
        assert!(calls[0].target_symbol_id.is_none() && calls[0].target_file_path.is_none());
        assert!(neighbors(store, run).is_empty());
    }
    assert_eq!(
        neighbors(store, control),
        BTreeSet::from([target.to_owned()]),
        "an unrelated unattributed block must retain its call proof"
    );
    let expected_callers = if proved {
        BTreeSet::from([run.to_owned(), control.to_owned()])
    } else {
        BTreeSet::from([control.to_owned()])
    };
    assert_eq!(
        store
            .list_callers_for_symbol(target)
            .expect("selected target callers")
            .into_iter()
            .map(|caller| caller.id)
            .collect::<BTreeSet<_>>(),
        expected_callers,
    );
    let coverage = store
        .call_edge_coverage_for_sources(&[run.to_owned()])
        .expect("outgoing coverage");
    assert_eq!(
        (coverage.call_edges, coverage.resolved, coverage.unresolved),
        if proved { (1, 1, 0) } else { (1, 0, 1) }
    );
}

fn attribute_lifecycle(external: bool, item: &str) {
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
    .expect("mock-only configuration");
    fs::write(
        workspace.join("Cargo.toml"),
        "[package]\nname='attribute_expansion_fixture'\nversion='0.1.0'\nedition='2021'\n",
    )
    .expect("conventional crate root");
    if external {
        fs::write(
            workspace.join("src/dep.rs"),
            "pub fn stable() -> u8 { 1 }\n",
        )
        .expect("external written target");
    }
    let attributed = source(external, item, true);
    fs::write(workspace.join(LIB), &attributed).expect("attributed caller");
    index_once(workspace, true);

    let store = SqliteStore::open(workspace).expect("initial catalog");
    let declarations = store
        .list_symbols_for_file(LIB)
        .expect("source declarations");
    let named = |name: &str| {
        declarations
            .iter()
            .find(|symbol| symbol.qualified_name == name)
            .unwrap_or_else(|| panic!("missing {name}"))
    };
    let run = named("run");
    let control = named("control");
    let target_path = if external { "src/dep.rs" } else { LIB };
    let target = store
        .list_symbols_for_file(target_path)
        .expect("target declarations")
        .into_iter()
        .find(|symbol| symbol.qualified_name == "stable")
        .expect("written stable target");
    assert_calls(&store, &run.id, &control.id, &target.id, false, external);
    for id in [&run.id, &control.id] {
        store
            .insert_intent_contract(
                id,
                "must",
                "Preserve this written declaration's ownership",
                None,
                "attribute-regression",
            )
            .expect("written contract");
    }
    assert!(
        store
            .read_sir_blob(&run.id)
            .expect("initial run summary")
            .is_some(),
        "initial full mock index must create a summary"
    );

    let ids = store
        .list_all_symbol_ids()
        .expect("written identities")
        .into_iter()
        .collect::<BTreeSet<_>>();
    let payloads = |catalog: &SqliteStore| {
        ids.iter()
            .map(|id| {
                let mirror_path = catalog.sir_dir().join(format!("{id}.json"));
                let mirror = mirror_path
                    .exists()
                    .then(|| fs::read_to_string(mirror_path).expect("existing summary mirror"));
                (
                    id.clone(),
                    catalog.read_sir_blob(id).expect("canonical payload"),
                    catalog
                        .get_sir_identity(id)
                        .expect("summary write identity"),
                    catalog.list_sir_history(id).expect("summary history"),
                    catalog
                        .list_intent_contracts_for_symbol(id, true)
                        .expect("all contracts"),
                    mirror,
                )
            })
            .collect::<Vec<_>>()
    };
    let metadata = |catalog: &SqliteStore| {
        ids.iter()
            .map(|id| {
                (
                    id.clone(),
                    catalog.get_sir_meta(id).expect("summary metadata"),
                    catalog
                        .get_sir_source_hash(id)
                        .expect("summary source hash"),
                )
            })
            .collect::<Vec<_>>()
    };
    let before_payloads = payloads(&store);
    let before_metadata = metadata(&store);

    // Retain every source declaration's other references, including the valid
    // control call, while replacing run's raw stable reference with old proof.
    // The format-upgrade suite separately tests downgrade of real old catalogs.
    let mut stale_edges = declarations
        .iter()
        .flat_map(|declaration| {
            store
                .list_symbol_edges_for_source_and_kinds(&declaration.id, EDGE_KINDS)
                .expect("all structural references")
        })
        .collect::<Vec<_>>();
    stale_edges.retain(|edge| {
        !(edge.source_id == run.id
            && edge.edge_kind == EdgeKind::Calls
            && edge.target_qualified_name == "stable")
    });
    stale_edges.push(SymbolEdge {
        source_id: run.id.clone(),
        target_qualified_name: target.qualified_name.clone(),
        edge_kind: EdgeKind::Calls,
        file_path: LIB.to_owned(),
        target_symbol_id: (!external).then(|| target.id.clone()),
        target_file_path: external.then(|| target_path.to_owned()),
    });
    store
        .delete_edges_for_file(LIB)
        .expect("replace stale references");
    store
        .upsert_edges(&stale_edges)
        .expect("seed old parser proof content");
    store
        .populate_symbol_neighbors(LIB)
        .expect("seed stale projection");
    assert_calls(&store, &run.id, &control.id, &target.id, true, external);
    assert_eq!(payloads(&store), before_payloads);
    assert_eq!(metadata(&store), before_metadata);
    drop(store);

    for _ in 0..2 {
        index_once(workspace, false);
        assert_eq!(
            fs::read_to_string(workspace.join(LIB)).expect("unchanged caller"),
            attributed
        );
        let refreshed = SqliteStore::open(workspace).expect("refreshed catalog");
        assert_eq!(
            refreshed
                .list_all_symbol_ids()
                .expect("retained identities")
                .into_iter()
                .collect::<BTreeSet<_>>(),
            ids
        );
        assert_calls(
            &refreshed,
            &run.id,
            &control.id,
            &target.id,
            false,
            external,
        );
        assert_eq!(
            payloads(&refreshed),
            before_payloads,
            "unchanged-source refresh must preserve owned evidence"
        );
        assert_eq!(
            metadata(&refreshed),
            before_metadata,
            "unchanged-source refresh must not rewrite freshness metadata"
        );
    }

    // An intentional source edit can update stale/status metadata, but structural
    // indexing must not rewrite the explanation, its history or its ownership.
    // The write generation in SirIdentity also detects a same-content rewrite.
    for (has_attribute, proved) in [(false, true), (true, false)] {
        let edited = source(external, item, has_attribute);
        fs::write(workspace.join(LIB), &edited).expect("toggle only item attribute");
        index_once(workspace, false);
        let refreshed = SqliteStore::open(workspace).expect("edited catalog");
        assert_eq!(
            fs::read_to_string(workspace.join(LIB)).expect("caller after edit"),
            edited
        );
        assert_eq!(
            refreshed
                .list_all_symbol_ids()
                .expect("same written identities")
                .into_iter()
                .collect::<BTreeSet<_>>(),
            ids
        );
        assert_calls(
            &refreshed,
            &run.id,
            &control.id,
            &target.id,
            proved,
            external,
        );
        assert_eq!(
            payloads(&refreshed),
            before_payloads,
            "attribute edits must not regenerate or reassign existing evidence"
        );
    }
}

#[test]
fn block_item_attributes_refresh_and_restore_local_id_proof_without_losing_owned_evidence() {
    for item in [
        "fn placeholder() {}",
        "struct Placeholder;",
        "const PLACEHOLDER: u8 = 0;",
    ] {
        attribute_lifecycle(false, item);
    }
}

#[test]
fn block_item_attributes_refresh_and_restore_external_file_proof_without_losing_owned_evidence() {
    for item in [
        "fn placeholder() {}",
        "struct Placeholder;",
        "const PLACEHOLDER: u8 = 0;",
    ] {
        attribute_lifecycle(true, item);
    }
}
