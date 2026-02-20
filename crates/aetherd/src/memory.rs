use std::io::Write;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_core::{
    SEARCH_FALLBACK_EMBEDDING_EMPTY_QUERY_VECTOR, SEARCH_FALLBACK_EMBEDDINGS_DISABLED, SearchMode,
};
use aether_infer::{EmbeddingProviderOverrides, load_embedding_provider_from_config};
use aether_memory::{
    AskInclude, AskQueryRequest, ListNotesRequest, NoteEmbeddingRequest, NoteSourceType,
    ProjectMemoryService, RecallRequest, RememberRequest, SemanticQuery,
    truncate_content_for_embedding,
};
use anyhow::{Context, Result};
use serde_json::json;

use crate::cli::{AskArgs, AskIncludeArg, NotesArgs, RecallArgs, RememberArgs};

pub fn run_remember_command(workspace: &Path, args: RememberArgs) -> Result<()> {
    let service = ProjectMemoryService::new(workspace);
    let request = RememberRequest {
        content: args.content,
        source_type: NoteSourceType::Manual,
        source_agent: None,
        tags: args.tags,
        entity_refs: Vec::new(),
        file_refs: Vec::new(),
        symbol_refs: Vec::new(),
        now_ms: None,
    };

    let remember = service
        .remember(request)
        .context("failed to store project note")?;

    if matches!(remember.action, aether_memory::RememberAction::Created) {
        let runtime = build_runtime().context("failed to build runtime for note embeddings")?;
        match load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default())
        {
            Ok(Some(loaded)) => {
                let embedding_text = truncate_content_for_embedding(remember.note.content.as_str());
                match runtime.block_on(loaded.provider.embed_text(embedding_text.as_str())) {
                    Ok(embedding) if !embedding.is_empty() => {
                        if let Err(err) =
                            runtime.block_on(service.upsert_note_embedding(NoteEmbeddingRequest {
                                note_id: remember.note.note_id.clone(),
                                provider: loaded.provider_name,
                                model: loaded.model_name,
                                embedding,
                                content: remember.note.content.clone(),
                                created_at: remember.note.created_at,
                                updated_at: Some(remember.note.updated_at),
                            }))
                        {
                            eprintln!("warning: failed to persist note embedding: {err}");
                        }
                    }
                    Ok(_) => {
                        eprintln!(
                            "warning: embedding provider returned an empty vector; note stored without semantic index"
                        );
                    }
                    Err(err) => {
                        eprintln!(
                            "warning: embedding generation failed; note stored without semantic index: {err}"
                        );
                    }
                }
            }
            Ok(None) => {}
            Err(err) => {
                eprintln!(
                    "warning: failed to load embedding provider; note stored without semantic index: {err}"
                );
            }
        }
    }

    let response = json!({
        "note_id": remember.note.note_id,
        "action": remember.action.as_str(),
        "content_hash": remember.note.content_hash,
        "tags": remember.note.tags,
        "created_at": remember.note.created_at,
        "updated_at": remember.note.updated_at,
    });
    write_json_to_stdout(&response)
}

pub fn run_recall_command(workspace: &Path, args: RecallArgs) -> Result<()> {
    let service = ProjectMemoryService::new(workspace);
    let mut semantic_query = None;
    let mut semantic_fallback_reason = None;

    if !matches!(args.mode, SearchMode::Lexical) {
        match load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default())
        {
            Ok(Some(loaded)) => {
                let runtime = build_runtime().context("failed to build runtime for recall")?;
                match runtime.block_on(loaded.provider.embed_text(args.query.as_str())) {
                    Ok(embedding) if !embedding.is_empty() => {
                        semantic_query = Some(SemanticQuery {
                            provider: loaded.provider_name,
                            model: loaded.model_name,
                            embedding,
                        });
                    }
                    Ok(_) => {
                        semantic_fallback_reason =
                            Some(SEARCH_FALLBACK_EMBEDDING_EMPTY_QUERY_VECTOR.to_owned());
                    }
                    Err(err) => {
                        semantic_fallback_reason = Some(format!("embedding provider error: {err}"));
                    }
                }
            }
            Ok(None) => {
                semantic_fallback_reason = Some(SEARCH_FALLBACK_EMBEDDINGS_DISABLED.to_owned());
            }
            Err(err) => {
                semantic_fallback_reason =
                    Some(format!("failed to load embedding provider: {err}"));
            }
        }
    }

    let runtime = build_runtime().context("failed to build runtime for recall")?;
    let result = runtime
        .block_on(service.recall(RecallRequest {
            query: args.query.clone(),
            mode: args.mode,
            limit: args.limit,
            include_archived: false,
            tags_filter: args.tags,
            now_ms: None,
            semantic: semantic_query,
            semantic_fallback_reason,
        }))
        .context("failed to recall project notes")?;

    let notes = result
        .notes
        .into_iter()
        .map(|entry| {
            json!({
                "note_id": entry.note.note_id,
                "content": entry.note.content,
                "tags": entry.note.tags,
                "file_refs": entry.note.file_refs,
                "symbol_refs": entry.note.symbol_refs,
                "source_type": entry.note.source_type,
                "created_at": entry.note.created_at,
                "access_count": entry.note.access_count,
                "relevance_score": entry.relevance_score,
            })
        })
        .collect::<Vec<_>>();

    let response = json!({
        "query": args.query,
        "mode_requested": result.mode_requested.as_str(),
        "mode_used": result.mode_used.as_str(),
        "fallback_reason": result.fallback_reason,
        "result_count": notes.len() as u32,
        "notes": notes,
    });

    write_json_to_stdout(&response)
}

pub fn run_ask_command(workspace: &Path, args: AskArgs) -> Result<()> {
    let service = ProjectMemoryService::new(workspace);
    let mut semantic_query = None;

    match load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default()) {
        Ok(Some(loaded)) => {
            let runtime = build_runtime().context("failed to build runtime for ask query")?;
            match runtime.block_on(loaded.provider.embed_text(args.query.as_str())) {
                Ok(embedding) if !embedding.is_empty() => {
                    semantic_query = Some(SemanticQuery {
                        provider: loaded.provider_name,
                        model: loaded.model_name,
                        embedding,
                    });
                }
                Ok(_) => {
                    eprintln!(
                        "warning: embedding provider returned empty vector; running lexical-only ask"
                    );
                }
                Err(err) => {
                    eprintln!(
                        "warning: embedding generation failed for ask query; running lexical-only: {err}"
                    );
                }
            }
        }
        Ok(None) => {}
        Err(err) => {
            eprintln!(
                "warning: failed to load embedding provider for ask query; running lexical-only: {err}"
            );
        }
    }

    let include = args
        .include
        .into_iter()
        .map(|value| match value {
            AskIncludeArg::Symbols => AskInclude::Symbols,
            AskIncludeArg::Notes => AskInclude::Notes,
            AskIncludeArg::Coupling => AskInclude::Coupling,
            AskIncludeArg::Tests => AskInclude::Tests,
        })
        .collect::<Vec<_>>();

    let runtime = build_runtime().context("failed to build runtime for ask query")?;
    let result = runtime
        .block_on(service.ask(AskQueryRequest {
            query: args.query.clone(),
            limit: args.limit,
            include,
            now_ms: None,
            semantic: semantic_query,
        }))
        .context("failed to run unified ask query")?;

    let response = json!({
        "query": result.query,
        "result_count": result.results.len() as u32,
        "results": result.results,
    });
    write_json_to_stdout(&response)
}

pub fn run_notes_command(workspace: &Path, args: NotesArgs) -> Result<()> {
    let service = ProjectMemoryService::new(workspace);
    let since_epoch_ms = args.since.map(|duration| {
        current_unix_timestamp_millis().saturating_sub(duration.as_millis() as i64)
    });

    let notes = service
        .list_notes(ListNotesRequest {
            limit: args.limit,
            since_epoch_ms,
            include_archived: false,
        })
        .context("failed to list project notes")?;

    let response = serde_json::to_value(notes).context("failed to serialize notes response")?;
    write_json_to_stdout(&response)
}

fn build_runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")
}

fn current_unix_timestamp_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_millis() as i64
}

fn write_json_to_stdout(value: &serde_json::Value) -> Result<()> {
    let mut out = std::io::stdout();
    serde_json::to_writer_pretty(&mut out, value).context("failed to serialize JSON output")?;
    writeln!(&mut out).context("failed to write trailing newline")?;
    Ok(())
}
