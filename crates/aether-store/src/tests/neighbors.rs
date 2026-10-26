use super::*;

fn test_symbol(id: &str, file_path: &str, qualified_name: &str) -> SymbolRecord {
    SymbolRecord {
        id: id.to_owned(),
        file_path: file_path.to_owned(),
        language: "rust".to_owned(),
        kind: "function".to_owned(),
        qualified_name: qualified_name.to_owned(),
        signature_fingerprint: format!("sig-{id}"),
        last_seen_at: 1_700_000_000,
    }
}

#[test]
fn populate_symbol_neighbors_creates_forward_and_reverse_rows() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    let alpha = test_symbol("sym-alpha", "src/a.rs", "alpha");
    let beta = test_symbol("sym-beta", "src/b.rs", "beta");
    store.upsert_symbol(alpha.clone()).expect("upsert alpha");
    store.upsert_symbol(beta.clone()).expect("upsert beta");
    store
        .upsert_edges(&[calls_edge(
            alpha.id.as_str(),
            beta.qualified_name.as_str(),
            alpha.file_path.as_str(),
        )])
        .expect("upsert edge");

    store
        .populate_symbol_neighbors(alpha.file_path.as_str())
        .expect("populate symbol neighbors");

    assert_eq!(
        store
            .get_symbol_neighbors(alpha.id.as_str())
            .expect("load alpha neighbors"),
        vec![SymbolNeighborRecord {
            symbol_id: alpha.id.clone(),
            neighbor_id: beta.id.clone(),
            edge_type: "calls".to_owned(),
            neighbor_name: beta.qualified_name.clone(),
            neighbor_file: beta.file_path.clone(),
        }]
    );
    assert_eq!(
        store
            .get_symbol_neighbors(beta.id.as_str())
            .expect("load beta neighbors"),
        vec![SymbolNeighborRecord {
            symbol_id: beta.id.clone(),
            neighbor_id: alpha.id.clone(),
            edge_type: "called_by".to_owned(),
            neighbor_name: alpha.qualified_name.clone(),
            neighbor_file: alpha.file_path.clone(),
        }]
    );
}

#[test]
fn populate_symbol_neighbors_replaces_stale_rows_for_reindexed_file() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    let alpha = test_symbol("sym-alpha", "src/a.rs", "alpha");
    let beta = test_symbol("sym-beta", "src/b.rs", "beta");
    let gamma = test_symbol("sym-gamma", "src/c.rs", "gamma");
    for symbol in [&alpha, &beta, &gamma] {
        store.upsert_symbol(symbol.clone()).expect("upsert symbol");
    }

    store
        .upsert_edges(&[calls_edge(
            alpha.id.as_str(),
            beta.qualified_name.as_str(),
            alpha.file_path.as_str(),
        )])
        .expect("upsert first edge");
    store
        .populate_symbol_neighbors(alpha.file_path.as_str())
        .expect("populate first neighbor set");

    store
        .delete_edges_for_file(alpha.file_path.as_str())
        .expect("delete old file edges");
    store
        .upsert_edges(&[calls_edge(
            alpha.id.as_str(),
            gamma.qualified_name.as_str(),
            alpha.file_path.as_str(),
        )])
        .expect("upsert replacement edge");
    store
        .populate_symbol_neighbors(alpha.file_path.as_str())
        .expect("populate replacement neighbor set");

    assert_eq!(
        store
            .get_symbol_neighbors(alpha.id.as_str())
            .expect("load alpha neighbors"),
        vec![SymbolNeighborRecord {
            symbol_id: alpha.id.clone(),
            neighbor_id: gamma.id.clone(),
            edge_type: "calls".to_owned(),
            neighbor_name: gamma.qualified_name.clone(),
            neighbor_file: gamma.file_path.clone(),
        }]
    );
    assert!(
        store
            .get_symbol_neighbors(beta.id.as_str())
            .expect("load beta neighbors")
            .is_empty()
    );
    assert_eq!(
        store
            .get_symbol_neighbors(gamma.id.as_str())
            .expect("load gamma neighbors"),
        vec![SymbolNeighborRecord {
            symbol_id: gamma.id.clone(),
            neighbor_id: alpha.id.clone(),
            edge_type: "called_by".to_owned(),
            neighbor_name: alpha.qualified_name.clone(),
            neighbor_file: alpha.file_path.clone(),
        }]
    );
}

#[test]
fn refreshing_callee_preserves_incoming_edges_and_replaces_only_its_outgoing_edges() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");
    let alpha = test_symbol("sym-alpha", "src/a.rs", "alpha");
    let beta = test_symbol("sym-beta", "src/b.rs", "beta");
    let gamma = test_symbol("sym-gamma", "src/c.rs", "gamma");
    let delta = test_symbol("sym-delta", "src/d.rs", "delta");
    for symbol in [&alpha, &beta, &gamma, &delta] {
        store.upsert_symbol(symbol.clone()).expect("upsert symbol");
    }
    store
        .upsert_edges(&[
            calls_edge(&alpha.id, &beta.qualified_name, &alpha.file_path),
            calls_edge(&beta.id, &gamma.qualified_name, &beta.file_path),
            calls_edge(&delta.id, &gamma.qualified_name, &delta.file_path),
        ])
        .expect("upsert edges");
    for symbol in [&alpha, &beta, &delta] {
        store
            .populate_symbol_neighbors(&symbol.file_path)
            .expect("populate neighbors");
    }

    store
        .delete_edges_for_file(&beta.file_path)
        .expect("delete callee's old edges");
    store
        .upsert_edges(&[calls_edge(&beta.id, &delta.qualified_name, &beta.file_path)])
        .expect("replace callee's outgoing edge");
    for _ in 0..2 {
        store
            .populate_symbol_neighbors(&beta.file_path)
            .expect("refresh callee neighbors");
    }

    let alpha_calls = store
        .get_symbol_neighbors_by_type(&alpha.id, "calls")
        .expect("load caller's dependencies");
    assert_eq!(alpha_calls.len(), 1);
    assert_eq!(alpha_calls[0].neighbor_id, beta.id);
    let beta_callers = store
        .get_symbol_neighbors_by_type(&beta.id, "called_by")
        .expect("load callee's callers");
    assert_eq!(beta_callers.len(), 1);
    assert_eq!(beta_callers[0].neighbor_id, alpha.id);
    let beta_calls = store
        .get_symbol_neighbors_by_type(&beta.id, "calls")
        .expect("load replaced dependencies");
    assert_eq!(beta_calls.len(), 1);
    assert_eq!(beta_calls[0].neighbor_id, delta.id);
    let gamma_callers = store
        .get_symbol_neighbors_by_type(&gamma.id, "called_by")
        .expect("load unrelated callee's callers");
    assert_eq!(gamma_callers.len(), 1);
    assert_eq!(gamma_callers[0].neighbor_id, delta.id);
    let delta_callers = store
        .get_symbol_neighbors_by_type(&delta.id, "called_by")
        .expect("load replacement callee's callers");
    assert_eq!(delta_callers.len(), 1);
    assert_eq!(delta_callers[0].neighbor_id, beta.id);
}

#[test]
fn refreshing_new_callee_resolves_previously_unresolved_callers() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");
    let alpha = test_symbol("sym-alpha", "src/a.rs", "alpha");
    let beta = test_symbol("sym-beta", "src/b.rs", "beta");
    store.upsert_symbol(alpha.clone()).expect("upsert caller");
    store
        .upsert_edges(&[calls_edge(
            &alpha.id,
            &beta.qualified_name,
            &alpha.file_path,
        )])
        .expect("upsert unresolved edge");
    store
        .populate_symbol_neighbors(&alpha.file_path)
        .expect("populate unresolved caller");
    assert!(
        store
            .get_symbol_neighbors(&alpha.id)
            .expect("load unresolved neighbors")
            .is_empty()
    );

    store.upsert_symbol(beta.clone()).expect("discover callee");
    store
        .populate_symbol_neighbors(&beta.file_path)
        .expect("populate newly discovered callee");
    let dependencies = store
        .get_symbol_neighbors_by_type(&alpha.id, "calls")
        .expect("load resolved dependencies");
    assert_eq!(dependencies.len(), 1);
    assert_eq!(dependencies[0].neighbor_id, beta.id);
    let callers = store
        .get_symbol_neighbors_by_type(&beta.id, "called_by")
        .expect("load resolved callers");
    assert_eq!(callers.len(), 1);
    assert_eq!(callers[0].neighbor_id, alpha.id);
}

#[test]
fn get_symbol_neighbors_by_type_filters_rows() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    let alpha = test_symbol("sym-alpha", "src/a.rs", "alpha");
    let beta = test_symbol("sym-beta", "src/b.rs", "beta");
    let gamma = test_symbol("sym-gamma", "src/c.rs", "gamma");
    for symbol in [&alpha, &beta, &gamma] {
        store.upsert_symbol(symbol.clone()).expect("upsert symbol");
    }

    store
        .upsert_edges(&[
            calls_edge(
                alpha.id.as_str(),
                beta.qualified_name.as_str(),
                alpha.file_path.as_str(),
            ),
            depends_edge(
                alpha.id.as_str(),
                gamma.qualified_name.as_str(),
                alpha.file_path.as_str(),
            ),
        ])
        .expect("upsert mixed edges");
    store
        .populate_symbol_neighbors(alpha.file_path.as_str())
        .expect("populate neighbors");

    assert_eq!(
        store
            .get_symbol_neighbors_by_type(alpha.id.as_str(), "calls")
            .expect("load call neighbors"),
        vec![SymbolNeighborRecord {
            symbol_id: alpha.id.clone(),
            neighbor_id: beta.id.clone(),
            edge_type: "calls".to_owned(),
            neighbor_name: beta.qualified_name.clone(),
            neighbor_file: beta.file_path.clone(),
        }]
    );
    assert_eq!(
        store
            .get_symbol_neighbors_by_type(alpha.id.as_str(), "depends_on")
            .expect("load dependency neighbors"),
        vec![SymbolNeighborRecord {
            symbol_id: alpha.id.clone(),
            neighbor_id: gamma.id.clone(),
            edge_type: "depends_on".to_owned(),
            neighbor_name: gamma.qualified_name.clone(),
            neighbor_file: gamma.file_path.clone(),
        }]
    );
}

#[test]
fn mark_removed_cleans_reverse_symbol_neighbors() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    let alpha = test_symbol("sym-alpha", "src/a.rs", "alpha");
    let beta = test_symbol("sym-beta", "src/b.rs", "beta");
    store.upsert_symbol(alpha.clone()).expect("upsert alpha");
    store.upsert_symbol(beta.clone()).expect("upsert beta");
    store
        .upsert_edges(&[calls_edge(
            alpha.id.as_str(),
            beta.qualified_name.as_str(),
            alpha.file_path.as_str(),
        )])
        .expect("upsert edge");
    store
        .populate_symbol_neighbors(alpha.file_path.as_str())
        .expect("populate neighbors");

    store.mark_removed(alpha.id.as_str()).expect("remove alpha");

    assert!(
        store
            .get_symbol_neighbors(beta.id.as_str())
            .expect("load beta neighbors")
            .is_empty()
    );
}

#[test]
fn reconcile_and_prune_cleans_stale_symbol_neighbors() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    let old_symbol = test_symbol("sym-old", "src/a.rs", "alpha::old");
    let mut new_symbol = test_symbol("sym-new", "src/a.rs", "alpha::old");
    new_symbol.signature_fingerprint = "new-signature".to_owned();
    let target = test_symbol("sym-target", "src/b.rs", "beta");
    for symbol in [&old_symbol, &new_symbol, &target] {
        store.upsert_symbol(symbol.clone()).expect("upsert symbol");
    }

    store
        .upsert_edges(&[calls_edge(
            old_symbol.id.as_str(),
            target.qualified_name.as_str(),
            old_symbol.file_path.as_str(),
        )])
        .expect("upsert edge");
    store
        .populate_symbol_neighbors(old_symbol.file_path.as_str())
        .expect("populate neighbors");

    let (migrated, pruned) = store
        .reconcile_and_prune(&[(old_symbol.id.clone(), new_symbol.id.clone())], &[])
        .expect("reconcile old to new");
    assert_eq!((migrated, pruned), (1, 0));
    assert!(
        store
            .get_symbol_neighbors(target.id.as_str())
            .expect("load target neighbors")
            .is_empty()
    );
}

#[test]
fn reverse_edge_query_filters_target_and_kind_without_dropping_structural_evidence() {
    let workspace = tempdir().expect("workspace");
    let store = SqliteStore::open(workspace.path()).expect("store");
    let kinds = [
        EdgeKind::Calls,
        EdgeKind::DependsOn,
        EdgeKind::TypeRef,
        EdgeKind::Implements,
    ];
    let mut expected = kinds
        .into_iter()
        .map(|edge_kind| SymbolEdge {
            source_id: "caller".to_owned(),
            target_qualified_name: "target".to_owned(),
            edge_kind,
            file_path: "src/caller.rs".to_owned(),
        })
        .collect::<Vec<_>>();
    store.upsert_edges(&expected).expect("all structural kinds");
    store
        .upsert_edges(&[SymbolEdge {
            target_qualified_name: "unrelated".to_owned(),
            ..expected[0].clone()
        }])
        .expect("decoy");
    expected.sort_by(|left, right| left.edge_kind.as_str().cmp(right.edge_kind.as_str()));
    assert_eq!(
        store
            .list_symbol_edges_for_target_and_kinds("target", &kinds)
            .expect("reverse evidence"),
        expected
    );
    let calls = store
        .list_symbol_edges_for_target_and_kinds(" target ", &[EdgeKind::Calls])
        .expect("calls only");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].edge_kind, EdgeKind::Calls);
    assert!(
        store
            .list_symbol_edges_for_target_and_kinds("", &kinds)
            .expect("empty name")
            .is_empty()
    );
    assert!(
        store
            .list_symbol_edges_for_target_and_kinds("target", &[])
            .expect("empty kinds")
            .is_empty()
    );
}
