use aether_infer::{
    RerankCandidate, RerankerProviderOverrides, load_reranker_provider_from_config,
};
use tempfile::tempdir;

#[tokio::test]
#[ignore]
async fn candle_reranker_real_model_produces_finite_scores() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();

    std::fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
    std::fs::write(
        workspace.join(".aether/config.toml"),
        r#"[search]
reranker = "candle"

[search.candle]
model_dir = ".aether/models"
"#,
    )
    .expect("write config");

    let loaded =
        load_reranker_provider_from_config(workspace, RerankerProviderOverrides::default())
            .expect("load reranker provider")
            .expect("provider should be enabled");

    let reranked = loaded
        .provider
        .rerank(
            "authentication handler",
            &[
                RerankCandidate {
                    id: "a".to_owned(),
                    text: "fn auth_handler validates and refreshes tokens".to_owned(),
                },
                RerankCandidate {
                    id: "b".to_owned(),
                    text: "fn parse_config reads yaml into structs".to_owned(),
                },
            ],
            2,
        )
        .await
        .expect("candle rerank");

    assert_eq!(reranked.len(), 2);
    assert!(reranked.iter().all(|entry| entry.score.is_finite()));
    assert!(
        reranked
            .iter()
            .all(|entry| (0.0..=1.0).contains(&entry.score))
    );
}

#[tokio::test]
#[ignore]
async fn cohere_reranker_api_call_returns_ranked_results() {
    if std::env::var("COHERE_API_KEY")
        .ok()
        .map(|value| value.trim().is_empty())
        .unwrap_or(true)
    {
        return;
    }

    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();

    std::fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
    std::fs::write(
        workspace.join(".aether/config.toml"),
        r#"[search]
reranker = "cohere"
"#,
    )
    .expect("write config");

    let loaded =
        load_reranker_provider_from_config(workspace, RerankerProviderOverrides::default())
            .expect("load reranker provider")
            .expect("provider should be enabled");

    let reranked = loaded
        .provider
        .rerank(
            "token refresh",
            &[
                RerankCandidate {
                    id: "auth".to_owned(),
                    text: "refresh token and session validation".to_owned(),
                },
                RerankCandidate {
                    id: "cache".to_owned(),
                    text: "memoize cache hits for rendered templates".to_owned(),
                },
            ],
            1,
        )
        .await
        .expect("cohere rerank");

    assert_eq!(reranked.len(), 1);
    assert!(reranked[0].score.is_finite());
}
