use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use aether_config::{AetherConfig, EmbeddingVectorBackend, GraphBackend, save_workspace_config};
use aether_core::{EdgeKind, SearchMode, SymbolEdge};
use aether_mcp::{
    AetherAskInclude, AetherAskRequest, AetherBlastRadiusRequest, AetherDependenciesRequest,
    AetherDriftReportRequest, AetherEnhancePromptRequest, AetherExplainRequest,
    AetherGetSirRequest, AetherHealthRequest, AetherMcpError, AetherMcpServer, AetherRecallRequest,
    AetherRememberRequest, AetherSirContextRequest, AetherSymbolTimelineRequest,
    AetherTraceCauseRequest, SharedState,
};
use aether_memory::{NoteSourceType, ProjectMemoryService, RememberRequest};
use aether_store::{
    ProjectNoteStore, SqliteStore, SymbolCatalogStore, SymbolEmbeddingRecord, SymbolRecord,
    SymbolRelationStore,
};
use anyhow::Result;

// Compare bytes as well as mtimes: this is stronger than comparing file hashes.
// Keep the fixture writer open so WAL coordination files already exist. This
// checks application writes; SQLite READ_ONLY does not promise filesystem
// immutability of its own WAL/shared-memory coordination in every setup.
type StoreSnapshot = BTreeMap<PathBuf, (Vec<u8>, SystemTime)>;

fn snapshot(root: &Path) -> Result<StoreSnapshot> {
    fn collect(root: &Path, path: &Path, result: &mut StoreSnapshot) -> Result<()> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                collect(root, &path, result)?;
            } else {
                result.insert(
                    path.strip_prefix(root)?.to_path_buf(),
                    (fs::read(&path)?, entry.metadata()?.modified()?),
                );
            }
        }
        Ok(())
    }
    let mut result = BTreeMap::new();
    collect(root, root, &mut result)?;
    Ok(result)
}

fn fixture_symbol(id: &str) -> SymbolRecord {
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

fn seed(workspace: &Path) -> Result<SqliteStore> {
    let mut config = AetherConfig::default();
    config.storage.graph_backend = GraphBackend::Surreal;
    config.embeddings.enabled = false;
    config.embeddings.vector_backend = EmbeddingVectorBackend::Sqlite;
    save_workspace_config(workspace, &config)?;
    fs::create_dir_all(workspace.join("src"))?;
    fs::write(
        workspace.join("src/lib.rs"),
        "pub fn fixture() { helper(); }\nfn helper() {}\n",
    )?;
    let store = SqliteStore::open(workspace)?;
    store.upsert_symbol(fixture_symbol("fixture"))?;
    store.upsert_symbol(fixture_symbol("helper"))?;
    store.upsert_edges(&[SymbolEdge {
        source_id: "fixture".to_owned(),
        target_qualified_name: "helper".to_owned(),
        edge_kind: EdgeKind::Calls,
        file_path: "src/lib.rs".to_owned(),
    }])?;
    Ok(store)
}

#[tokio::test]
async fn readonly_missing_index_does_not_create_files() -> Result<()> {
    let workspace = tempfile::tempdir()?;
    assert!(SharedState::open_readonly(workspace.path()).is_err());
    assert!(
        SharedState::open_readonly_async(workspace.path())
            .await
            .is_err()
    );
    assert_eq!(fs::read_dir(workspace.path())?.count(), 0);
    Ok(())
}

#[tokio::test]
async fn readonly_reads_and_rejected_writes_preserve_store_contents_and_mtimes() -> Result<()> {
    let workspace = tempfile::tempdir()?;
    let writer = seed(workspace.path())?;
    let note = ProjectMemoryService::new(workspace.path()).remember(RememberRequest {
        content: "fixture decision: keep SQLite for local portability".to_owned(),
        source_type: NoteSourceType::Manual,
        source_agent: None,
        tags: Vec::new(),
        entity_refs: Vec::new(),
        file_refs: vec!["src/lib.rs".to_owned()],
        symbol_refs: vec!["fixture".to_owned()],
        now_ms: Some(1),
    })?;
    let state = Arc::new(SharedState::open_readonly_async(workspace.path()).await?);
    let server = AetherMcpServer::from_state(Arc::clone(&state), false);
    let before = snapshot(&workspace.path().join(".aether"))?;

    state.warm_surreal_graph();
    assert!(matches!(
        state.surreal_graph().await,
        Err(AetherMcpError::ReadOnly(_))
    ));
    assert!(state.surreal_graph.lock().await.is_none());
    assert!(matches!(
        state.embedding_pipeline(),
        Err(AetherMcpError::ReadOnly(_))
    ));

    assert_eq!(server.aether_status_logic()?.symbol_count, 2);
    let sir = server.aether_get_sir_logic(AetherGetSirRequest {
        level: None,
        symbol_id: Some("fixture".to_owned()),
        file_path: None,
        module_path: None,
        language: None,
    })?;
    assert!(sir.structural.is_some());
    assert_ne!(sir.sir_status.as_deref(), Some("generating"));
    assert!(state.store.list_sir_request_symbol_ids(10)?.is_empty());
    server.aether_explain_logic(AetherExplainRequest {
        file_path: "src/lib.rs".to_owned(),
        line: 1,
        column: 8,
    })?;
    assert!(
        server
            .aether_dependencies_logic(AetherDependenciesRequest {
                symbol_id: "fixture".to_owned()
            })
            .await?
            .found
    );
    server.aether_symbol_timeline_logic(AetherSymbolTimelineRequest {
        symbol_id: "fixture".to_owned(),
        limit: Some(5),
        include_retired_evidence: None,
    })?;
    // Match the router: the synchronous assembler runs outside the async executor.
    let context_server = server.clone();
    tokio::task::spawn_blocking(move || {
        context_server.aether_sir_context_logic(AetherSirContextRequest {
            symbol: "fixture".to_owned(),
            max_tokens: Some(8_000),
            format: None,
            include_layers: None,
            depth: None,
            include_layer_content: None,
        })
    })
    .await??;
    let recalled = server
        .aether_recall_logic(AetherRecallRequest {
            query: "fixture".to_owned(),
            mode: Some(SearchMode::Lexical),
            limit: Some(5),
            include_archived: None,
            tags_filter: None,
        })
        .await?;
    assert_eq!(recalled.notes.len(), 1);
    assert_eq!(recalled.notes[0].access_count, 0);
    let asked = server
        .aether_ask_logic(AetherAskRequest {
            query: "fixture".to_owned(),
            limit: Some(5),
            include: Some(vec![
                AetherAskInclude::Symbols,
                AetherAskInclude::Notes,
                AetherAskInclude::Coupling,
            ]),
        })
        .await?;
    assert!(!asked.results.is_empty());
    assert!(!asked.warnings.is_empty());
    let blast = server.aether_blast_radius_logic(AetherBlastRadiusRequest {
        file: "src/lib.rs".to_owned(),
        min_risk: None,
    })?;
    assert!(!blast.warnings.is_empty());
    let health = server
        .aether_health_logic(AetherHealthRequest {
            include: None,
            limit: None,
            min_risk: None,
        })
        .await?;
    assert!(
        health
            .notes
            .iter()
            .any(|note| note.contains("Read-only mode"))
    );

    assert!(matches!(
        server.aether_drift_report_logic(AetherDriftReportRequest {
            window: None,
            include: None,
            min_drift_magnitude: None,
            include_acknowledged: None,
        }),
        Err(AetherMcpError::ReadOnly(_))
    ));
    assert!(matches!(
        server.aether_trace_cause_logic(AetherTraceCauseRequest {
            symbol: None,
            symbol_id: Some("fixture".to_owned()),
            file: None,
            lookback: None,
            max_depth: None,
            limit: None,
        }),
        Err(AetherMcpError::ReadOnly(_))
    ));
    assert!(matches!(
        server
            .aether_remember_logic(AetherRememberRequest {
                content: "must not be written".to_owned(),
                tags: None,
                entity_refs: None,
                file_refs: None,
                symbol_refs: None,
            })
            .await,
        Err(AetherMcpError::ReadOnly(_))
    ));
    assert!(matches!(
        server.aether_enhance_prompt_logic(AetherEnhancePromptRequest {
            prompt: "Change fixture without affecting helper".to_owned(),
            budget: 8_000,
            rewrite: false,
            format: "text".to_owned(),
        }),
        Err(AetherMcpError::ReadOnly(_))
    ));
    let stored_note = writer.get_project_note(&note.note.note_id)?;
    assert_eq!(stored_note.map(|note| note.access_count), Some(0));
    assert_eq!(snapshot(&workspace.path().join(".aether"))?, before);
    assert!(!workspace.path().join(".aether/graph").exists());
    Ok(())
}

#[tokio::test]
async fn readonly_vectors_reject_writes_and_lancedb_open_is_skipped() -> Result<()> {
    let workspace = tempfile::tempdir()?;
    let _writer = seed(workspace.path())?;
    let mut config = aether_config::load_workspace_config(workspace.path())?;
    config.embeddings.enabled = true;
    save_workspace_config(workspace.path(), &config)?;
    let state = SharedState::open_readonly_async(workspace.path()).await?;
    let vector = state
        .vector_store
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("missing SQLite vectors"))?;
    let before = snapshot(&workspace.path().join(".aether"))?;
    assert!(
        vector
            .upsert_embedding(SymbolEmbeddingRecord {
                symbol_id: "fixture".to_owned(),
                sir_hash: "fixture".to_owned(),
                provider: "fixture".to_owned(),
                model: "fixture-2d".to_owned(),
                embedding: vec![1.0, 0.0],
                updated_at: 1,
            })
            .await
            .is_err()
    );
    assert!(vector.delete_embedding("fixture").await.is_err());
    assert_eq!(snapshot(&workspace.path().join(".aether"))?, before);
    drop(state);

    config.embeddings.vector_backend = EmbeddingVectorBackend::Lancedb;
    save_workspace_config(workspace.path(), &config)?;
    let before = snapshot(&workspace.path().join(".aether"))?;
    let state = SharedState::open_readonly_async(workspace.path()).await?;
    assert!(state.vector_store.is_none());
    assert!(!state.semantic_search_available);
    assert_eq!(snapshot(&workspace.path().join(".aether"))?, before);
    assert!(!workspace.path().join(".aether/vectors").exists());
    Ok(())
}
