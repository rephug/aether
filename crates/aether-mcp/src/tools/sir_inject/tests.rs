use std::fs;
use std::path::Path;

use aether_sir::SirAnnotation;
use aether_store::{SirHistoryStore, SirStateStore, SymbolCatalogStore, SymbolRecord};
use tempfile::tempdir;

use super::{
    AetherSirInjectRequest, AetherSirInjectResponse, DEFAULT_INJECT_CONFIDENCE,
    resolve_symbol_selector, round_confidence_for_display,
};
use crate::AetherMcpServer;

fn write_test_config(workspace: &Path) {
    fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[inference]
provider = "qwen3_local"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
    )
    .expect("write config");
}

fn seed_symbol(workspace: &Path, symbol_id: &str, qualified_name: &str) {
    let store = aether_store::SqliteStore::open(workspace).expect("open store");
    store
        .upsert_symbol(SymbolRecord {
            id: symbol_id.to_owned(),
            file_path: "src/lib.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature_fingerprint: format!("sig-{symbol_id}"),
            last_seen_at: 1_700_000_000,
        })
        .expect("upsert symbol");
}

fn seed_existing_sir(workspace: &Path, symbol_id: &str, confidence: f32) {
    let store = aether_store::SqliteStore::open(workspace).expect("open store");
    let sir_json = format!(
        r#"{{
            "intent":"existing intent",
            "inputs":[],
            "outputs":[],
            "side_effects":["writes cache"],
            "dependencies":[],
            "error_modes":["timeout"],
            "confidence":{confidence}
        }}"#
    );
    let history = store
        .record_sir_version_if_changed(
            symbol_id,
            "seed-hash",
            "seed",
            "seed",
            sir_json.as_str(),
            1_700_000_100,
            None,
        )
        .expect("record seed history");
    store
        .write_sir_blob(symbol_id, sir_json.as_str())
        .expect("write seed sir");
    store
        .upsert_sir_meta(aether_store::SirMetaRecord {
            id: symbol_id.to_owned(),
            sir_hash: "seed-hash".to_owned(),
            sir_version: history.version,
            provider: "seed".to_owned(),
            model: "seed".to_owned(),
            generation_pass: "scan".to_owned(),
            reasoning_trace: None,
            prompt_hash: None,
            staleness_score: None,
            updated_at: history.updated_at,
            sir_status: "fresh".to_owned(),
            last_error: None,
            last_attempt_at: history.updated_at,
        })
        .expect("upsert seed meta");
}

#[test]
fn resolve_symbol_selector_prefers_exact_matches() {
    let temp = tempdir().expect("tempdir");
    write_test_config(temp.path());
    seed_symbol(temp.path(), "sym-alpha", "crate::alpha");
    let store = aether_store::SqliteStore::open(temp.path()).expect("open store");

    let resolved = resolve_symbol_selector(&store, "crate::alpha").expect("resolve symbol");
    assert_eq!(resolved.id, "sym-alpha");
}

#[test]
fn sir_inject_creates_new_sir_when_missing() {
    let temp = tempdir().expect("tempdir");
    write_test_config(temp.path());
    seed_symbol(temp.path(), "sym-new", "crate::new_symbol");
    let server = AetherMcpServer::new(temp.path(), false).expect("server");

    let response = server
        .aether_sir_inject_logic(AetherSirInjectRequest {
            symbol: "sym-new".to_owned(),
            intent: "Persist a new SIR annotation".to_owned(),
            behavior: Some("Reads inputs and stores audit metadata".to_owned()),
            edge_cases: Some("Blank inputs are rejected before persistence".to_owned()),
            side_effects: Some(vec!["writes audit history".to_owned()]),
            dependencies: Some(vec!["sqlx::PgPool".to_owned()]),
            error_modes: Some(vec!["io".to_owned()]),
            confidence: Some(0.6),
            inputs: Some(vec!["payload: CreateRequest".to_owned()]),
            outputs: Some(vec!["Result<(), io::Error>".to_owned()]),
            complexity: Some("medium".to_owned()),
            generation_pass: None,
            model: None,
            provider: None,
            force: None,
            source_hash: None,
        })
        .expect("inject sir");

    assert_eq!(response.status, "injected");
    assert_eq!(response.sir_version, 1);
    assert_eq!(response.previous_confidence, None);
    assert_eq!(response.new_confidence, 0.6);
    assert!(!response.sir_hash.is_empty());
    assert_eq!(response.file_rollup_status, "refreshed");

    let store = aether_store::SqliteStore::open(temp.path()).expect("open store");
    let rollup_id = aether_sir::synthetic_file_sir_id("rust", "src/lib.rs");
    let rollup = store
        .read_sir_blob(&rollup_id)
        .expect("read file rollup")
        .expect("file rollup rebuilt from the injected leaf");
    assert!(
        rollup.contains("Persist a new SIR annotation"),
        "file rollup must reflect the injected leaf: {rollup}"
    );
    let history = store.list_sir_history("sym-new").expect("list sir history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].version, 1);
    let meta = store
        .get_sir_meta("sym-new")
        .expect("get sir meta")
        .expect("sir meta exists");
    assert_eq!(meta.sir_hash, response.sir_hash);
    assert_eq!(meta.sir_version, response.sir_version);
    assert_eq!(meta.model, "manual");
}

#[test]
fn sir_inject_blocks_when_existing_confidence_is_high_without_force() {
    let temp = tempdir().expect("tempdir");
    write_test_config(temp.path());
    seed_symbol(temp.path(), "sym-block", "crate::blocked");
    seed_existing_sir(temp.path(), "sym-block", 0.95);
    let server = AetherMcpServer::new(temp.path(), false).expect("server");

    let response = server
        .aether_sir_inject_logic(AetherSirInjectRequest {
            symbol: "sym-block".to_owned(),
            intent: "Attempted overwrite".to_owned(),
            behavior: None,
            edge_cases: None,
            side_effects: None,
            dependencies: None,
            error_modes: None,
            confidence: Some(0.95),
            inputs: None,
            outputs: None,
            complexity: None,
            generation_pass: None,
            model: None,
            provider: None,
            force: Some(false),
            source_hash: None,
        })
        .expect("inject sir");

    assert_eq!(response.status, "blocked");
    assert_eq!(response.previous_confidence, Some(0.95));

    let store = aether_store::SqliteStore::open(temp.path()).expect("open store");
    let history = store
        .list_sir_history("sym-block")
        .expect("list sir history");
    assert_eq!(history.len(), 1);
}

#[test]
fn sir_inject_concurrent_injections_of_one_symbol_serialize_through_the_guard() {
    // Two injectors (separate servers over one workspace, as two MCP processes
    // would be) race to replace a low-confidence SIR without force: the one that
    // takes the lock second must see the first's high-confidence write and be
    // blocked by the guard instead of overwriting it.
    let temp = tempdir().expect("tempdir");
    write_test_config(temp.path());
    seed_symbol(temp.path(), "sym-race", "crate::raced");
    seed_existing_sir(temp.path(), "sym-race", 0.1);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = ["first", "second"]
        .into_iter()
        .map(|label| {
            let workspace = temp.path().to_path_buf();
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                let server = AetherMcpServer::new(&workspace, false).expect("server");
                barrier.wait();
                server
                    .aether_sir_inject_logic(AetherSirInjectRequest {
                        symbol: "sym-race".to_owned(),
                        intent: format!("{label} injector"),
                        behavior: None,
                        edge_cases: None,
                        side_effects: None,
                        dependencies: None,
                        error_modes: None,
                        confidence: Some(0.9),
                        inputs: None,
                        outputs: None,
                        complexity: None,
                        generation_pass: None,
                        model: None,
                        provider: None,
                        force: Some(false),
                        source_hash: None,
                    })
                    .expect("inject")
            })
        })
        .collect();
    let responses: Vec<AetherSirInjectResponse> = workers
        .into_iter()
        .map(|worker| worker.join().expect("join"))
        .collect();

    let injected: Vec<_> = responses
        .iter()
        .filter(|response| response.status == "injected")
        .collect();
    let blocked: Vec<_> = responses
        .iter()
        .filter(|response| response.status == "blocked")
        .collect();
    assert_eq!(injected.len(), 1, "{responses:?}");
    assert_eq!(blocked.len(), 1, "{responses:?}");
    assert_eq!(injected[0].previous_confidence, Some(0.1));
    assert_eq!(blocked[0].previous_confidence, Some(0.9));

    let store = aether_store::SqliteStore::open(temp.path()).expect("open store");
    let blob = store
        .read_sir_blob("sym-race")
        .expect("read blob")
        .expect("blob exists");
    let sir: SirAnnotation = serde_json::from_str(&blob).expect("parse sir");
    assert_eq!(sir.confidence, 0.9);
    assert!(
        sir.intent.ends_with(" injector"),
        "winner's intent kept: {}",
        sir.intent
    );
    assert_eq!(
        store.list_sir_history("sym-race").expect("history").len(),
        2,
        "seed plus exactly one injection"
    );
}

#[test]
fn sir_inject_force_overrides_existing_high_confidence_sir() {
    let temp = tempdir().expect("tempdir");
    write_test_config(temp.path());
    seed_symbol(temp.path(), "sym-force", "crate::forced");
    seed_existing_sir(temp.path(), "sym-force", 0.9);
    let server = AetherMcpServer::new(temp.path(), false).expect("server");

    let response = server
        .aether_sir_inject_logic(AetherSirInjectRequest {
            symbol: "crate::forced".to_owned(),
            intent: "Forced overwrite".to_owned(),
            behavior: Some("Overwrites existing SIR metadata".to_owned()),
            edge_cases: Some("Existing history row is preserved".to_owned()),
            side_effects: Some(vec!["updates sir row".to_owned()]),
            dependencies: Some(vec!["sqlite".to_owned()]),
            error_modes: Some(vec!["network".to_owned()]),
            confidence: Some(0.4),
            inputs: Some(vec!["symbol selector".to_owned()]),
            outputs: Some(vec!["AetherSirInjectResponse".to_owned()]),
            complexity: Some("High".to_owned()),
            generation_pass: Some("deep".to_owned()),
            model: Some("claude-opus-4-6".to_owned()),
            provider: Some("manual".to_owned()),
            force: Some(true),
            source_hash: None,
        })
        .expect("inject sir");

    assert_eq!(response.status, "injected");
    assert_eq!(response.previous_confidence, Some(0.9));
    assert_eq!(response.new_confidence, 0.4);
    assert!(response.sir_version >= 2);

    let store = aether_store::SqliteStore::open(temp.path()).expect("open store");
    let blob = store
        .read_sir_blob("sym-force")
        .expect("read sir blob")
        .expect("sir blob exists");
    assert!(blob.contains("Forced overwrite"));
    assert!(blob.contains("\"behavior\":\"Overwrites existing SIR metadata\""));
    assert!(blob.contains("\"complexity\":\"High\""));
    let meta = store
        .get_sir_meta("sym-force")
        .expect("get sir meta")
        .expect("sir meta exists");
    assert_eq!(meta.sir_hash, response.sir_hash);
    assert_eq!(meta.sir_version, response.sir_version);
    assert_eq!(meta.model, "claude-opus-4-6");
}

#[test]
fn sir_inject_defaults_confidence_to_agent_default_when_omitted_for_existing_sir() {
    let temp = tempdir().expect("tempdir");
    write_test_config(temp.path());
    seed_symbol(temp.path(), "sym-default", "crate::defaulted");
    seed_existing_sir(temp.path(), "sym-default", 0.4);
    let server = AetherMcpServer::new(temp.path(), false).expect("server");

    let response = server
        .aether_sir_inject_logic(AetherSirInjectRequest {
            symbol: "sym-default".to_owned(),
            intent: "Default confidence overwrite".to_owned(),
            behavior: None,
            edge_cases: None,
            side_effects: None,
            dependencies: None,
            error_modes: None,
            confidence: None,
            inputs: None,
            outputs: None,
            complexity: None,
            generation_pass: None,
            model: None,
            provider: None,
            force: Some(false),
            source_hash: None,
        })
        .expect("inject sir");

    assert_eq!(response.status, "injected");
    assert_eq!(response.new_confidence, DEFAULT_INJECT_CONFIDENCE);

    let store = aether_store::SqliteStore::open(temp.path()).expect("open store");
    let blob = store
        .read_sir_blob("sym-default")
        .expect("read sir blob")
        .expect("sir blob exists");
    let sir: SirAnnotation = serde_json::from_str(&blob).expect("parse sir");
    assert_eq!(sir.confidence, DEFAULT_INJECT_CONFIDENCE);
}

#[test]
fn inject_response_rounds_confidence_for_display() {
    assert_eq!(round_confidence_for_display(0.97), 0.97);
    assert_eq!(round_confidence_for_display(0.123_456), 0.1235);
    let response = AetherSirInjectResponse {
        symbol_id: "sym".to_owned(),
        qualified_name: "crate::sym".to_owned(),
        sir_hash: "hash".to_owned(),
        sir_version: 1,
        previous_confidence: Some(0.83),
        new_confidence: 0.97,
        status: "injected".to_owned(),
        note: None,
        embedding_status: "skipped: embeddings disabled".to_owned(),
        file_rollup_status: "refreshed".to_owned(),
    };
    let rendered = serde_json::to_string(&response).expect("serialize");
    assert!(rendered.contains("\"new_confidence\":0.97"), "{rendered}");
    assert!(
        rendered.contains("\"previous_confidence\":0.83"),
        "{rendered}"
    );
    assert!(!rendered.contains("0.9700000"), "{rendered}");
    // serde_json::Value goes through f64 too, which is where the artifact used to appear.
    let value = serde_json::to_value(&response).expect("to_value");
    assert_eq!(value["new_confidence"].to_string(), "0.97");
}

#[test]
fn sir_inject_rollup_failed_marker_only_admits_a_rerun_of_the_same_injection() {
    let temp = tempdir().expect("tempdir");
    write_test_config(temp.path());
    seed_symbol(temp.path(), "sym-rollup", "crate::rollup_retry");
    seed_existing_sir(temp.path(), "sym-rollup", 0.9);
    {
        let store = aether_store::SqliteStore::open(temp.path()).expect("open store");
        let meta = store
            .get_sir_meta("sym-rollup")
            .expect("get sir meta")
            .expect("seeded meta");
        store
            .upsert_sir_meta(aether_store::SirMetaRecord {
                sir_status: super::SIR_STATUS_ROLLUP_FAILED.to_owned(),
                ..meta
            })
            .expect("mark rollup failed");
    }
    let server = AetherMcpServer::new(temp.path(), false).expect("server");
    let request = |intent: &str, confidence: f32| AetherSirInjectRequest {
        symbol: "sym-rollup".to_owned(),
        intent: intent.to_owned(),
        behavior: None,
        edge_cases: None,
        side_effects: None,
        dependencies: None,
        error_modes: None,
        confidence: Some(confidence),
        inputs: None,
        outputs: None,
        complexity: None,
        generation_pass: None,
        model: None,
        provider: None,
        force: Some(false),
        source_hash: None,
    };

    // A different request that was queued against the old placeholder does not get
    // to overwrite the reviewed SIR just because its rollup once failed.
    // A different request queued against the old placeholder must not overwrite the
    // reviewed SIR; it repairs the outstanding rollup instead, so the symbol stops
    // being a scan target without anyone having to reproduce the stored annotation.
    let other = server
        .aether_sir_inject_logic(request("A different request's intent", 0.8))
        .expect("inject sir");
    assert_eq!(other.status, "rollup_repaired");
    assert_eq!(other.file_rollup_status, "refreshed");
    // The stored leaf's embedding refresh runs after the repair (embeddings are off
    // in this workspace, so it reports that rather than "pending").
    assert_eq!(other.embedding_status, "skipped: embeddings disabled");
    assert!(
        other
            .note
            .as_deref()
            .is_some_and(|note| note.contains("was kept") && note.contains("rollup_failed")),
        "note: {:?}",
        other.note
    );
    let store = aether_store::SqliteStore::open(temp.path()).expect("open store");
    let stored: SirAnnotation = serde_json::from_str(
        &store
            .read_sir_blob("sym-rollup")
            .expect("read blob")
            .expect("blob"),
    )
    .expect("parse blob");
    assert_eq!(stored.intent, "existing intent");
    let meta = store
        .get_sir_meta("sym-rollup")
        .expect("get sir meta")
        .expect("meta");
    assert_eq!(meta.sir_status, "fresh");
    assert_eq!(meta.sir_hash, "seed-hash", "the leaf was not rewritten");

    // The rerun of the failed injection itself reconstructs the stored SIR and is
    // admitted while the marker is outstanding.
    store
        .upsert_sir_meta(aether_store::SirMetaRecord {
            sir_status: super::SIR_STATUS_ROLLUP_FAILED.to_owned(),
            ..meta
        })
        .expect("mark rollup failed again");
    let retry = server
        .aether_sir_inject_logic(request("existing intent", 0.9))
        .expect("inject sir");
    assert_eq!(retry.status, "injected");
    assert_eq!(retry.file_rollup_status, "refreshed");
    let meta = store
        .get_sir_meta("sym-rollup")
        .expect("get sir meta")
        .expect("meta");
    assert_eq!(meta.sir_status, "fresh");

    // A leaf whose process exited between its transaction and the rollup rebuild is
    // left `rollup_pending`: a fresh session's different annotation repairs it too.
    store
        .upsert_sir_meta(aether_store::SirMetaRecord {
            sir_status: super::SIR_STATUS_ROLLUP_PENDING.to_owned(),
            ..meta
        })
        .expect("mark rollup pending");
    let other = server
        .aether_sir_inject_logic(request("A different request's intent", 0.8))
        .expect("inject sir");
    assert_eq!(other.status, "rollup_repaired");
    assert_eq!(
        store
            .get_sir_meta("sym-rollup")
            .expect("get sir meta")
            .expect("meta")
            .sir_status,
        "fresh"
    );

    // With nothing outstanding, the guard blocks as before.
    let blocked = server
        .aether_sir_inject_logic(request("A different request's intent", 0.8))
        .expect("inject sir");
    assert_eq!(blocked.status, "blocked");
    assert!(
        blocked
            .note
            .as_deref()
            .is_some_and(|note| note.contains("force=true")),
        "note: {:?}",
        blocked.note
    );
}

#[test]
fn sir_inject_refuses_a_source_that_changed_since_it_was_read() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_test_config(workspace);
    let source = "pub fn target() -> u32 {\n    1\n}\n";
    fs::create_dir_all(workspace.join("src")).expect("create src");
    fs::write(workspace.join("src/lib.rs"), source).expect("write source");
    let symbols = aether_parse::SymbolExtractor::new()
        .expect("parser")
        .extract_from_path(Path::new("src/lib.rs"), source)
        .expect("extract");
    let symbol = symbols
        .iter()
        .find(|symbol| symbol.name == "target")
        .expect("target symbol");
    {
        let store = aether_store::SqliteStore::open(workspace).expect("open store");
        store
            .upsert_symbol(SymbolRecord {
                id: symbol.id.clone(),
                file_path: symbol.file_path.clone(),
                language: symbol.language.as_str().to_owned(),
                kind: symbol.kind.as_str().to_owned(),
                qualified_name: symbol.qualified_name.clone(),
                signature_fingerprint: symbol.signature_fingerprint.clone(),
                last_seen_at: 1_700_000_000,
            })
            .expect("upsert symbol");
    }
    let server = AetherMcpServer::new(workspace, false).expect("server");

    // The lookup reports the hash of the text as the file holds it now, and, by id
    // with `include_source`, the very text that hash was computed from.
    let lookup = server
        .aether_symbol_lookup_logic(crate::AetherSymbolLookupRequest {
            query: String::new(),
            limit: None,
            symbol_ids: Some(vec![symbol.id.clone(), "no-such-symbol".to_owned()]),
            include_source: Some(true),
        })
        .expect("lookup");
    assert_eq!(lookup.matches.len(), 1, "unknown ids are left out");
    let found = &lookup.matches[0];
    assert_eq!(found.symbol_id, symbol.id);
    assert_eq!(
        found.source_hash.as_deref(),
        Some(symbol.content_hash.as_str())
    );
    let text = found.source_text.as_deref().expect("source text");
    assert!(text.contains("fn target()"), "text: {text}");
    assert_eq!(aether_core::content_hash(text), symbol.content_hash);

    let request = |source_hash: Option<String>| AetherSirInjectRequest {
        symbol: symbol.id.clone(),
        intent: "Returns the answer".to_owned(),
        behavior: None,
        edge_cases: None,
        side_effects: None,
        dependencies: None,
        error_modes: None,
        confidence: Some(0.75),
        inputs: None,
        outputs: None,
        complexity: None,
        generation_pass: Some("scan".to_owned()),
        model: None,
        provider: None,
        force: Some(true),
        source_hash,
    };
    let injected = server
        .aether_sir_inject_logic(request(Some(symbol.content_hash.clone())))
        .expect("inject against the text that was read");
    assert_eq!(injected.status, "injected");

    // The body changes under the same symbol id: a SIR for the old text is refused.
    fs::write(
        workspace.join("src/lib.rs"),
        "pub fn target() -> u32 {\n    2\n}\n",
    )
    .expect("edit source");
    let err = server
        .aether_sir_inject_logic(request(Some(symbol.content_hash.clone())))
        .expect_err("a changed source must refuse the injection");
    assert!(
        err.to_string().contains("changed since it was read"),
        "unexpected error: {err}"
    );
    let store = aether_store::SqliteStore::open(workspace).expect("open store");
    let meta = store
        .get_sir_meta(&symbol.id)
        .expect("get sir meta")
        .expect("meta");
    assert_eq!(
        meta.sir_hash, injected.sir_hash,
        "the refused call wrote nothing"
    );

    // The lookup now reports the new text's hash, and an injection bound to it lands.
    let lookup = server
        .aether_symbol_lookup_logic(crate::AetherSymbolLookupRequest {
            query: symbol.qualified_name.clone(),
            limit: Some(5),
            symbol_ids: None,
            include_source: None,
        })
        .expect("lookup");
    let current = lookup
        .matches
        .iter()
        .find(|entry| entry.symbol_id == symbol.id)
        .and_then(|entry| entry.source_hash.clone())
        .expect("current source hash");
    assert_ne!(current, symbol.content_hash);
    let rebound = server
        .aether_sir_inject_logic(request(Some(current.clone())))
        .expect("inject against the new text");
    assert_eq!(rebound.status, "injected");

    // The leaf records the source it was bound to. A leaf left `rollup_pending` for
    // text that has since changed is replaced by a request bound to the new text
    // (not merely repaired), while one for the same text is repaired.
    let store = aether_store::SqliteStore::open(workspace).expect("open store");
    assert_eq!(
        store.get_sir_source_hash(&symbol.id).expect("source hash"),
        Some(current.clone())
    );
    let unforced = |intent: &str, source_hash: String| AetherSirInjectRequest {
        symbol: symbol.id.clone(),
        intent: intent.to_owned(),
        behavior: None,
        edge_cases: None,
        side_effects: None,
        dependencies: None,
        error_modes: None,
        confidence: Some(0.75),
        inputs: None,
        outputs: None,
        complexity: None,
        generation_pass: Some("scan".to_owned()),
        model: None,
        provider: None,
        force: Some(false),
        source_hash: Some(source_hash),
    };
    let mark_pending = || {
        let meta = store
            .get_sir_meta(&symbol.id)
            .expect("get sir meta")
            .expect("meta");
        store
            .upsert_sir_meta(aether_store::SirMetaRecord {
                sir_status: super::SIR_STATUS_ROLLUP_PENDING.to_owned(),
                ..meta
            })
            .expect("mark pending");
    };
    mark_pending();
    let same_text = server
        .aether_sir_inject_logic(unforced(
            "Another annotation of the same text",
            current.clone(),
        ))
        .expect("inject");
    assert_eq!(same_text.status, "rollup_repaired");
    mark_pending();
    fs::write(
        workspace.join("src/lib.rs"),
        "pub fn target() -> u32 {\n    3\n}\n",
    )
    .expect("edit source again");
    let newest = server
        .aether_symbol_lookup_logic(crate::AetherSymbolLookupRequest {
            query: String::new(),
            limit: None,
            symbol_ids: Some(vec![symbol.id.clone()]),
            include_source: None,
        })
        .expect("lookup")
        .matches
        .into_iter()
        .next()
        .and_then(|entry| entry.source_hash)
        .expect("newest source hash");
    assert_ne!(newest, current);
    let replaced = server
        .aether_sir_inject_logic(unforced("Describes the newest text", newest.clone()))
        .expect("inject");
    assert_eq!(
        replaced.status, "injected",
        "a pending leaf of older text is replaced, not repaired"
    );
    assert_eq!(
        store.get_sir_source_hash(&symbol.id).expect("source hash"),
        Some(newest.clone())
    );
    let stored: SirAnnotation = serde_json::from_str(
        &store
            .read_sir_blob(&symbol.id)
            .expect("read blob")
            .expect("blob"),
    )
    .expect("parse blob");
    assert_eq!(stored.intent, "Describes the newest text");

    // A leaf that never recorded its source (an injection without `source_hash`)
    // and was left pending offers no evidence about the text it describes: a
    // request without a hash of its own repairs it, one bound to the current text
    // replaces it rather than certifying it `fresh`.
    let unbound = server
        .aether_sir_inject_logic(AetherSirInjectRequest {
            force: Some(true),
            source_hash: None,
            ..unforced("Injected without a source hash", String::new())
        })
        .expect("forced inject without a source hash");
    assert_eq!(unbound.status, "injected");
    assert_eq!(
        store.get_sir_source_hash(&symbol.id).expect("source hash"),
        None
    );
    mark_pending();
    let unbound_repair = server
        .aether_sir_inject_logic(AetherSirInjectRequest {
            source_hash: None,
            ..unforced("Another unbound annotation", String::new())
        })
        .expect("inject");
    assert_eq!(unbound_repair.status, "rollup_repaired");
    mark_pending();
    let bound = server
        .aether_sir_inject_logic(unforced("Bound to the newest text", newest.clone()))
        .expect("inject");
    assert_eq!(
        bound.status, "injected",
        "a pending leaf of unknown source is replaced by a request bound to the current text"
    );
    assert_eq!(
        store.get_sir_source_hash(&symbol.id).expect("source hash"),
        Some(newest)
    );

    // A symbol the file no longer declares is refused too.
    fs::write(workspace.join("src/lib.rs"), "pub fn other() {}\n").expect("remove symbol");
    let err = server
        .aether_sir_inject_logic(request(Some(symbol.content_hash.clone())))
        .expect_err("a vanished symbol must refuse the injection");
    assert!(
        err.to_string().contains("no longer declares"),
        "unexpected error: {err}"
    );
}
