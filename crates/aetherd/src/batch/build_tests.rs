//! Cache regressions through the real offline builder and provider formatter.
use super::build::*;
use super::{BatchRuntimeConfig, PassConfig};
use aether_parse::SymbolExtractor;
use aether_store::{SirMetaRecord, SirStateStore, SymbolCatalogStore, SymbolRecord};

#[test]
fn provider_cache_context_covers_transport_without_credentials() {
    use super::BatchProvider;
    let secret = "credential-must-not-enter-cache-identity";
    let providers: Vec<Box<dyn BatchProvider>> = vec![
        Box::new(super::gemini::GeminiBatchProvider::new(secret.into())),
        Box::new(super::openai::OpenAiBatchProvider::new(secret.into())),
        Box::new(super::anthropic::AnthropicBatchProvider::new(secret.into())),
    ];
    for provider in providers {
        let context = provider.cache_context();
        assert!(
            context["api_base"]
                .as_str()
                .is_some_and(|s| s.starts_with("https://"))
        );
        assert_eq!(context["provider"], provider.name());
        assert!(!context.to_string().contains(secret));
    }
}

#[test]
fn effective_cache_identity_changes_for_each_supplied_enrichment_input() {
    use super::BatchProvider;
    use aether_infer::sir_prompt::*;
    let source = "fn run() {}";
    let context = aether_infer::SirContext {
        language: "rust".into(),
        file_path: "src/lib.rs".into(),
        qualified_name: "demo::run".into(),
        priority_score: None,
        kind: "function".into(),
        is_public: false,
        line_count: 1,
    };
    let baseline: aether_sir::SirAnnotation = serde_json::from_value(serde_json::json!({"intent":"Does work", "inputs":[], "outputs":[], "side_effects":[], "dependencies":[], "error_modes":[], "confidence":0.8})).expect("baseline");
    let enrichment = SirEnrichmentContext {
        file_intent: Some("File role".into()),
        neighbor_intents: vec![("peer [file=other.rs; id=one]".into(), "Peer role".into())],
        baseline_sir: Some(baseline),
        priority_reason: "Selected".into(),
        caller_contract_clauses: vec![("caller".into(), "must".into(), "Preserve input".into())],
    };
    let provider = super::gemini::GeminiBatchProvider::new("unused".into());
    let system = sir_enriched_system_prompt(PromptTier::Compact);
    let hash = |e: &SirEnrichmentContext,
                c: &aether_infer::SirContext,
                original_bytes,
                system: &str,
                model: &str,
                thinking: &str| {
        let user = sir_enriched_user_prompt_with_source_size(source, c, e, false, original_bytes);
        let template = provider
            .format_request("cache-key", system, "dynamic-placeholder", model, thinking)
            .expect("offline format");
        let settings =
            serde_json::json!({"model": model,"request":template,"schema":SIR_RESPONSE_SCHEMA})
                .to_string();
        super::hash::compute_effective_prompt_hash("full-source-identity", &user, &settings)
    };
    let base = hash(&enrichment, &context, source.len(), &system, "model", "low");
    let mut mutations = Vec::new();
    let mut e = enrichment.clone();
    e.file_intent = Some("Changed file role".into());
    mutations.push(e);
    let mut e = enrichment.clone();
    e.neighbor_intents[0].0 = "peer [file=new.rs; id=two]".into();
    mutations.push(e);
    let mut e = enrichment.clone();
    e.neighbor_intents[0].1 = "Changed peer behavior".into();
    mutations.push(e);
    let mut e = enrichment.clone();
    e.baseline_sir.as_mut().expect("baseline").intent = "Corrected baseline".into();
    mutations.push(e);
    let mut e = enrichment.clone();
    e.baseline_sir.as_mut().expect("baseline").confidence = 0.3;
    mutations.push(e);
    let mut e = enrichment.clone();
    e.caller_contract_clauses[0].2 = "Do not mutate input".into();
    mutations.push(e);
    let mut e = enrichment.clone();
    e.priority_reason = "Another reason".into();
    mutations.push(e);
    for (i, e) in mutations.iter().enumerate() {
        assert_ne!(
            base,
            hash(e, &context, source.len(), &system, "model", "low"),
            "enrichment mutation {i}"
        );
    }
    let mut renamed = context.clone();
    renamed.file_path = "different.rs".into();
    assert_ne!(
        base,
        hash(&enrichment, &renamed, source.len(), &system, "model", "low")
    );
    assert_ne!(
        base,
        hash(
            &enrichment,
            &context,
            source.len() + 1,
            &system,
            "model",
            "low"
        )
    );
    assert_ne!(
        base,
        hash(
            &enrichment,
            &context,
            source.len(),
            &(system + "Changed policy"),
            "model",
            "low"
        )
    );
    let system = sir_enriched_system_prompt(PromptTier::Compact);
    assert_ne!(
        base,
        hash(
            &enrichment,
            &context,
            source.len(),
            &system,
            "other-model",
            "low"
        )
    );
    assert_ne!(
        base,
        hash(
            &enrichment,
            &context,
            source.len(),
            &system,
            "model",
            "high"
        )
    );
}

#[test]
fn actual_batch_cache_skips_only_matching_prompt_and_detects_omitted_tail_edits() {
    let temp = tempfile::tempdir().expect("fixture");
    let workspace = temp.path();
    std::fs::create_dir_all(workspace.join("src")).expect("src");
    let parse = |source: &str| {
        std::fs::write(workspace.join("src/lib.rs"), source).expect("source");
        SymbolExtractor::new()
            .expect("parser")
            .extract_from_path(std::path::Path::new("src/lib.rs"), source)
            .expect("parse")
            .remove(0)
    };
    let symbol = parse("fn run() -> i32 { 1 }\n");
    let store = aether_store::SqliteStore::open(workspace).expect("synthetic store");
    store
        .upsert_symbol(SymbolRecord {
            id: symbol.id.clone(),
            file_path: symbol.file_path.clone(),
            language: "rust".into(),
            kind: "function".into(),
            qualified_name: symbol.qualified_name.clone(),
            signature_fingerprint: symbol.signature_fingerprint.clone(),
            last_seen_at: 1,
        })
        .expect("catalog");
    let pass = PassConfig {
        pass: crate::cli::BatchPass::Scan,
        model: "offline-model".into(),
        thinking: "low".into(),
        neighbor_depth: 0,
        max_chars: 8,
        prompt_tier: "compact".into(),
    };
    let runtime = BatchRuntimeConfig {
        batch_dir: workspace.join("batch"),
        jsonl_chunk_size: 10,
        poll_interval_secs: 1,
        max_symbols: 0,
        max_concurrent_jobs: 1,
        scan: pass.clone(),
        triage: pass.clone(),
        deep: pass.clone(),
    };
    // Formatting a request with a dummy key never submits it or requires credentials.
    let provider = super::gemini::GeminiBatchProvider::new("unused-offline-key".into());
    let build = |symbol: aether_core::Symbol, pass: &PassConfig| {
        let ids = [symbol.id.clone()];
        let symbols = std::collections::HashMap::from([(symbol.id.clone(), symbol)]);
        build_pass_jsonl_for_ids(
            workspace,
            &store,
            &runtime,
            pass,
            &symbols,
            Some(&ids),
            false,
            &provider,
        )
        .expect("offline build")
    };
    let first = build(symbol.clone(), &pass);
    assert_eq!(first.written, 1);
    let key = first.origins.keys().next().expect("request");
    let hash = key.split('|').nth(1).expect("hash").to_owned();
    assert!(hash.starts_with("v3:"));
    assert_eq!(key.split('|').count(), 3);
    let line = std::fs::read_to_string(&first.files[0]).expect("request JSONL");
    assert!(line.contains("TRUNCATED"));
    store
        .upsert_sir_meta(SirMetaRecord {
            id: symbol.id.clone(),
            sir_hash: "fixture".into(),
            sir_version: 1,
            provider: "gemini".into(),
            model: pass.model.clone(),
            generation_pass: "scan".into(),
            reasoning_trace: None,
            prompt_hash: Some(hash.clone()),
            staleness_score: None,
            updated_at: 1,
            sir_status: "fresh".into(),
            last_error: None,
            last_attempt_at: 1,
        })
        .expect("cached fixture");
    let same = build(symbol.clone(), &pass);
    assert_eq!((same.written, same.skipped), (0, 1));
    let mut changed_settings = pass.clone();
    changed_settings.neighbor_depth = 1;
    assert_eq!(build(symbol.clone(), &changed_settings).written, 1);
    let edited = parse("fn run() -> i32 { 2 }\n");
    assert_eq!(edited.id, symbol.id);
    let tail = build(edited, &pass);
    assert_eq!(
        tail.written, 1,
        "same excerpt and size, different full-source identity"
    );
    assert!(!tail.origins.keys().next().expect("new key").contains(&hash));
}
