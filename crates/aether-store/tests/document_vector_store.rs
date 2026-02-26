use aether_document::DocumentVectorBackend;
use aether_store::document_vector_store::LanceDocumentVectorStore;
use tempfile::tempdir;

#[tokio::test]
async fn lance_document_vector_store_upsert_search_and_delete_work() {
    let temp = tempdir().expect("tempdir");
    let vectors_dir = temp.path().join(".aether").join("vectors");
    let store = LanceDocumentVectorStore::new(vectors_dir, "mock", "mock-3d");

    let inserted = store
        .upsert_embeddings(
            "docs",
            &[
                ("rec-1".to_owned(), vec![1.0, 0.0, 0.0]),
                ("rec-2".to_owned(), vec![0.0, 1.0, 0.0]),
            ],
        )
        .await
        .expect("upsert embeddings");
    assert_eq!(inserted, 2);

    let results = store
        .search("docs", &[1.0, 0.0, 0.0], 5)
        .await
        .expect("search embeddings");
    assert!(!results.is_empty());
    assert_eq!(results[0].record_id, "rec-1");
    if results.len() > 1 {
        assert!(results[0].score >= results[1].score);
    }

    let deleted = store.delete_by_domain("docs").await.expect("delete domain");
    assert_eq!(deleted, 1);

    let post_delete = store
        .search("docs", &[1.0, 0.0, 0.0], 5)
        .await
        .expect("search after delete");
    assert!(post_delete.is_empty());
}

#[tokio::test]
async fn lance_document_vector_store_rejects_inconsistent_dimensions() {
    let temp = tempdir().expect("tempdir");
    let vectors_dir = temp.path().join(".aether").join("vectors");
    let store = LanceDocumentVectorStore::new(vectors_dir, "mock", "mock-test");

    let err = store
        .upsert_embeddings(
            "docs",
            &[
                ("rec-1".to_owned(), vec![1.0, 0.0]),
                ("rec-2".to_owned(), vec![1.0, 0.0, 0.0]),
            ],
        )
        .await
        .expect_err("dimension mismatch should fail");

    let message = err.to_string().to_ascii_lowercase();
    assert!(message.contains("dimension"));
}
