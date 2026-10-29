use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn symbol(id: &str) -> SymbolRecord {
    SymbolRecord {
        id: id.to_owned(),
        file_path: "src/lib.rs".to_owned(),
        language: "rust".to_owned(),
        kind: "function".to_owned(),
        qualified_name: id.to_owned(),
        signature_fingerprint: format!("sig-{id}"),
        last_seen_at: 1,
    }
}

#[tokio::test]
async fn mutation_surfaces_statement_error() -> TestResult {
    let workspace = tempfile::tempdir()?;
    let graph = SurrealGraphStore::open(workspace.path()).await?;
    let original = symbol("invalid");
    graph.upsert_symbol_node(&original).await?;
    graph
        .db()
        .query("DEFINE FIELD OVERWRITE last_seen_at ON symbol TYPE int ASSERT $value > 0;")
        .await?
        .check()?;

    let mut invalid = symbol("invalid");
    invalid.last_seen_at = 0;
    let result = graph.upsert_symbol_node(&invalid).await;
    assert!(
        matches!(&result, Err(StoreError::Graph(message)) if message.contains("upsert symbol")),
        "a successfully received query response must still reject failed statements: {result:?}"
    );
    assert_eq!(graph.list_all_symbol_ids().await?, vec![original.id]);
    Ok(())
}

#[tokio::test]
async fn invalid_symbol_insert_cannot_report_success_without_a_node() -> TestResult {
    let workspace = tempfile::tempdir()?;
    let graph = SurrealGraphStore::open(workspace.path()).await?;
    graph
        .db()
        .query("DEFINE FIELD OVERWRITE last_seen_at ON symbol TYPE int ASSERT $value > 0;")
        .await?
        .check()?;
    let mut invalid = symbol("new-invalid");
    invalid.last_seen_at = 0;
    let result = graph.upsert_symbol_node(&invalid).await;
    let ids = graph.list_all_symbol_ids().await?;
    assert!(
        result.is_err(),
        "invalid insert returned {result:?}; stored IDs: {ids:?}"
    );
    assert!(ids.is_empty());
    Ok(())
}

#[tokio::test]
async fn tested_by_replacement_rolls_back_on_constraint_error() -> TestResult {
    let workspace = tempfile::tempdir()?;
    let graph = SurrealGraphStore::open(workspace.path()).await?;
    let original = TestedByRecord {
        target_file: "src/original.rs".to_owned(),
        test_file: "tests/regression.rs".to_owned(),
        intent_count: 1,
        confidence: 0.75,
        inference_method: "fixture".to_owned(),
    };
    graph
        .replace_tested_by_for_test_file(&original.test_file, std::slice::from_ref(&original))
        .await?;

    let replacement = TestedByRecord {
        target_file: "src/replacement.rs".to_owned(),
        ..original.clone()
    };
    // The unique target/test index rejects the second insertion after DELETE and
    // the first insertion have already executed inside the transaction.
    let result = graph
        .replace_tested_by_for_test_file(
            &original.test_file,
            &[replacement.clone(), replacement.clone()],
        )
        .await;
    assert!(
        matches!(&result, Err(StoreError::Graph(message)) if message.contains("replace tested_by")),
        "replacement must return the database constraint error: {result:?}"
    );
    assert_eq!(
        graph
            .list_tested_by_for_target_file(&original.target_file)
            .await?,
        vec![original.clone()]
    );
    assert!(
        graph
            .list_tested_by_for_target_file(&replacement.target_file)
            .await?
            .is_empty()
    );

    graph
        .replace_tested_by_for_test_file(&original.test_file, &[])
        .await?;
    assert!(
        graph
            .list_tested_by_for_target_file(&original.target_file)
            .await?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn edge_replacement_rolls_back_on_statement_error() -> TestResult {
    let workspace = tempfile::tempdir()?;
    let graph = SurrealGraphStore::open(workspace.path()).await?;
    for id in ["caller", "callee"] {
        graph.upsert_symbol_node(&symbol(id)).await?;
    }
    let edge = ResolvedEdge {
        source_id: "caller".to_owned(),
        target_id: "callee".to_owned(),
        edge_kind: EdgeKind::Calls,
        file_path: "src/lib.rs".to_owned(),
    };
    graph.upsert_edge(&edge).await?;
    graph
        .db()
        .query("DEFINE FIELD OVERWRITE weight ON depends_on TYPE float ASSERT $value < 1.0;")
        .await?
        .check()?;

    let result = graph.upsert_edge(&edge).await;
    assert!(
        matches!(&result, Err(StoreError::Graph(message)) if message.contains("upsert edge")),
        "replacement must return the failed RELATE statement: {result:?}"
    );
    assert_eq!(graph.list_dependency_edges().await?.len(), 1);
    assert_eq!(
        graph.get_dependencies("caller").await?,
        vec![symbol("callee")]
    );
    Ok(())
}

#[tokio::test]
async fn call_chain_excludes_root_and_returns_shortest_unique_depths() -> TestResult {
    let workspace = tempfile::tempdir()?;
    let graph = SurrealGraphStore::open(workspace.path()).await?;
    for id in ["root", "left", "right", "join"] {
        graph.upsert_symbol_node(&symbol(id)).await?;
    }
    for (source, target) in [
        ("root", "root"),
        ("root", "left"),
        ("root", "right"),
        ("left", "root"),
        ("left", "join"),
        ("right", "join"),
    ] {
        graph
            .upsert_edge(&ResolvedEdge {
                source_id: source.to_owned(),
                target_id: target.to_owned(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            })
            .await?;
    }

    assert!(graph.get_call_chain("root", 0).await?.is_empty());
    assert_eq!(
        graph.get_call_chain("root", 1).await?,
        vec![vec![symbol("left"), symbol("right")]]
    );
    assert_eq!(
        graph.get_call_chain("root", 20).await?,
        vec![vec![symbol("left"), symbol("right")], vec![symbol("join")]]
    );
    Ok(())
}

#[tokio::test]
async fn co_change_upsert_rolls_back_the_entire_failed_batch() -> TestResult {
    let workspace = tempfile::tempdir()?;
    let graph = SurrealGraphStore::open(workspace.path()).await?;
    let original = CouplingEdgeRecord {
        file_a: "src/a.rs".to_owned(),
        file_b: "src/b.rs".to_owned(),
        co_change_count: 1,
        total_commits_a: 2,
        total_commits_b: 2,
        git_coupling: 0.5,
        static_signal: 0.5,
        semantic_signal: 0.5,
        fused_score: 0.5,
        coupling_type: "mixed".to_owned(),
        last_co_change_commit: "fixture".to_owned(),
        last_co_change_at: 1,
        mined_at: 1,
    };
    graph
        .upsert_co_change_edges(std::slice::from_ref(&original))
        .await?;
    graph
        .db()
        .query("DEFINE FIELD OVERWRITE fused_score ON co_change TYPE float ASSERT $value <= 1.0;")
        .await?
        .check()?;

    let replacement = CouplingEdgeRecord {
        fused_score: 0.75,
        ..original.clone()
    };
    let invalid = CouplingEdgeRecord {
        file_a: "src/invalid.rs".to_owned(),
        fused_score: 2.0,
        ..original.clone()
    };
    let result = graph.upsert_co_change_edges(&[replacement, invalid]).await;
    assert!(
        matches!(&result, Err(StoreError::Graph(message)) if message.contains("upsert co_change")),
        "batch must return the statement constraint error: {result:?}"
    );
    assert_eq!(
        graph
            .get_co_change_edge(&original.file_a, &original.file_b)
            .await?,
        Some(original)
    );
    assert!(
        graph
            .get_co_change_edge("src/invalid.rs", "src/b.rs")
            .await?
            .is_none()
    );
    Ok(())
}
