use aether_infer::{EmbeddingProviderOverrides, load_embedding_provider_from_config};
use tempfile::tempdir;

#[tokio::test]
#[ignore]
async fn candle_provider_real_model_produces_non_zero_embeddings() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();

    std::fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
    std::fs::write(
        workspace.join(".aether/config.toml"),
        r#"[embeddings]
enabled = true
provider = "candle"

[embeddings.candle]
model_dir = ".aether/models"
"#,
    )
    .expect("write config");

    let loaded =
        load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default())
            .expect("load embedding provider")
            .expect("provider should be enabled");

    let embedding = loaded
        .provider
        .embed_text("offline semantic search in Rust")
        .await
        .expect("compute embedding");

    assert_eq!(embedding.len(), 1024);
    assert!(embedding.iter().all(|value| value.is_finite()));
    assert!(embedding.iter().any(|value| value.abs() > 1e-6));
}
