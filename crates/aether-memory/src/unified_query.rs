use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::note::{ProjectMemoryService, project_note_from_store};
use crate::ranking::{apply_recency_access_boost, rrf_score};
use crate::search::SemanticQuery;
use crate::{MemoryError, current_unix_timestamp_millis};
use aether_store::{CouplingEdgeRecord, CozoGraphStore, SqliteStore, Store, open_vector_store};

const SYMBOL_COUPLING_SEED_LIMIT: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AskInclude {
    Symbols,
    Notes,
    Coupling,
    Tests,
}

impl AskInclude {
    pub fn all() -> Vec<Self> {
        vec![Self::Symbols, Self::Notes, Self::Coupling, Self::Tests]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AskResultKind {
    Symbol,
    Note,
    TestGuard,
    CoupledFile,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AskQueryRequest {
    pub query: String,
    pub limit: u32,
    pub include: Vec<AskInclude>,
    pub now_ms: Option<i64>,
    pub semantic: Option<SemanticQuery>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AskResultItem {
    pub kind: AskResultKind,
    pub id: Option<String>,
    pub title: Option<String>,
    pub snippet: String,
    pub relevance_score: f32,
    pub file: Option<String>,
    pub language: Option<String>,
    pub tags: Vec<String>,
    pub source_type: Option<String>,
    pub test_file: Option<String>,
    pub fused_score: Option<f32>,
    pub coupling_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AskQueryResult {
    pub query: String,
    pub results: Vec<AskResultItem>,
}

#[derive(Debug, Clone)]
struct RankedCandidate {
    key: String,
    access_count: i64,
    last_accessed_at: Option<i64>,
    item: AskResultItem,
}

#[derive(Debug, Clone)]
struct CouplingAggregate {
    coupled_file: String,
    anchor_file: String,
    git_coupling: f32,
    fused_score: f32,
    coupling_type: String,
}

impl ProjectMemoryService {
    pub async fn ask(&self, request: AskQueryRequest) -> Result<AskQueryResult, MemoryError> {
        let query = request.query.trim();
        if query.is_empty() {
            return Ok(AskQueryResult {
                query: String::new(),
                results: Vec::new(),
            });
        }

        let limit = request.limit.clamp(1, 100) as usize;
        let candidate_limit = (limit as u32).max(30);
        let now_ms = request
            .now_ms
            .unwrap_or_else(current_unix_timestamp_millis)
            .max(0);
        let include = normalize_include(request.include);

        let fetch_symbols =
            include.contains(&AskInclude::Symbols) || include.contains(&AskInclude::Coupling);
        let fetch_notes = include.contains(&AskInclude::Notes);
        let fetch_tests = include.contains(&AskInclude::Tests);

        let workspace = self.workspace().to_path_buf();
        let query_owned = query.to_owned();

        let (symbol_lexical, note_lexical, test_lexical) = std::thread::scope(|scope| {
            let symbol_handle = fetch_symbols.then(|| {
                let workspace = workspace.clone();
                let query = query_owned.clone();
                scope.spawn(
                    move || -> Result<Vec<aether_store::SymbolSearchResult>, MemoryError> {
                        let store = SqliteStore::open(&workspace)?;
                        store
                            .search_symbols(query.as_str(), candidate_limit)
                            .map_err(Into::into)
                    },
                )
            });

            let note_handle = fetch_notes.then(|| {
                let workspace = workspace.clone();
                let query = query_owned.clone();
                scope.spawn(
                    move || -> Result<Vec<aether_store::ProjectNoteRecord>, MemoryError> {
                        let store = SqliteStore::open(&workspace)?;
                        store
                            .search_project_notes_lexical(
                                query.as_str(),
                                candidate_limit,
                                false,
                                &[],
                            )
                            .map_err(Into::into)
                    },
                )
            });

            let test_handle = fetch_tests.then(|| {
                let workspace = workspace.clone();
                let query = query_owned.clone();
                scope.spawn(
                    move || -> Result<Vec<aether_store::TestIntentRecord>, MemoryError> {
                        let store = SqliteStore::open(&workspace)?;
                        store
                            .search_test_intents_lexical(query.as_str(), candidate_limit)
                            .map_err(Into::into)
                    },
                )
            });

            let symbol_lexical = match symbol_handle {
                Some(handle) => handle.join().map_err(|_| {
                    MemoryError::InvalidInput("symbol search thread panicked".to_owned())
                })??,
                None => Vec::new(),
            };

            let note_lexical = match note_handle {
                Some(handle) => handle.join().map_err(|_| {
                    MemoryError::InvalidInput("note search thread panicked".to_owned())
                })??,
                None => Vec::new(),
            };

            let test_lexical = match test_handle {
                Some(handle) => handle.join().map_err(|_| {
                    MemoryError::InvalidInput("test intent search thread panicked".to_owned())
                })??,
                None => Vec::new(),
            };

            Ok::<_, MemoryError>((symbol_lexical, note_lexical, test_lexical))
        })?;

        let mut symbol_semantic = Vec::<aether_store::SymbolSearchResult>::new();
        let mut note_semantic = Vec::<aether_store::ProjectNoteRecord>::new();
        if let Some(semantic) = request.semantic {
            let vector_store = open_vector_store(self.workspace()).await?;
            let store = self.open_store()?;

            if fetch_symbols {
                let semantic_rows = vector_store
                    .search_nearest(
                        semantic.embedding.as_slice(),
                        semantic.provider.as_str(),
                        semantic.model.as_str(),
                        candidate_limit,
                    )
                    .await?;
                for row in semantic_rows {
                    let Some(symbol) = store.get_symbol_search_result(row.symbol_id.as_str())?
                    else {
                        continue;
                    };
                    symbol_semantic.push(symbol);
                }
            }

            if fetch_notes {
                let semantic_rows = vector_store
                    .search_project_notes_nearest(
                        semantic.embedding.as_slice(),
                        semantic.provider.as_str(),
                        semantic.model.as_str(),
                        candidate_limit,
                    )
                    .await?;
                for row in semantic_rows {
                    let Some(note) = store.get_project_note(row.note_id.as_str())? else {
                        continue;
                    };
                    if note.is_archived {
                        continue;
                    }
                    note_semantic.push(note);
                }
            }
        }

        let symbol_candidates = if fetch_symbols {
            rank_symbol_candidates(symbol_lexical, symbol_semantic, now_ms)
        } else {
            Vec::new()
        };
        let note_candidates = if fetch_notes {
            rank_note_candidates(note_lexical, note_semantic, now_ms)
        } else {
            Vec::new()
        };
        let test_candidates = if fetch_tests {
            rank_test_candidates(test_lexical)
        } else {
            Vec::new()
        };
        let coupling_candidates = if include.contains(&AskInclude::Coupling) {
            rank_coupling_candidates(self.workspace(), symbol_candidates.as_slice())?
        } else {
            Vec::new()
        };
        let symbol_candidates_for_results = if include.contains(&AskInclude::Symbols) {
            symbol_candidates
        } else {
            Vec::new()
        };

        let mut result = merge_candidates(
            now_ms,
            limit,
            symbol_candidates_for_results,
            note_candidates,
            test_candidates,
            coupling_candidates,
        );
        result.query = query.to_owned();

        let store = self.open_store()?;
        enrich_symbol_snippets(&store, result.results.as_mut_slice())?;
        increment_access_from_results(&store, result.results.as_mut_slice(), now_ms)?;

        Ok(result)
    }
}

fn normalize_include(include: Vec<AskInclude>) -> BTreeSet<AskInclude> {
    if include.is_empty() {
        return AskInclude::all().into_iter().collect();
    }
    include.into_iter().collect()
}

fn rank_symbol_candidates(
    lexical: Vec<aether_store::SymbolSearchResult>,
    semantic: Vec<aether_store::SymbolSearchResult>,
    now_ms: i64,
) -> Vec<RankedCandidate> {
    let mut by_id = HashMap::<String, aether_store::SymbolSearchResult>::new();
    let mut score_by_id = HashMap::<String, f32>::new();

    for (rank, row) in lexical.iter().enumerate() {
        by_id
            .entry(row.symbol_id.clone())
            .or_insert_with(|| row.clone());
        *score_by_id.entry(row.symbol_id.clone()).or_insert(0.0) += rrf_score(rank);
    }
    for (rank, row) in semantic.iter().enumerate() {
        by_id
            .entry(row.symbol_id.clone())
            .or_insert_with(|| row.clone());
        *score_by_id.entry(row.symbol_id.clone()).or_insert(0.0) += rrf_score(rank);
    }

    let mut ranked = score_by_id
        .into_iter()
        .filter_map(|(symbol_id, score)| {
            by_id.remove(symbol_id.as_str()).map(|symbol| {
                let boosted = apply_recency_access_boost(
                    score,
                    symbol.access_count,
                    symbol.last_accessed_at,
                    now_ms,
                );
                (
                    boosted,
                    RankedCandidate {
                        key: format!("symbol:{}", symbol.symbol_id),
                        access_count: symbol.access_count,
                        last_accessed_at: symbol.last_accessed_at,
                        item: AskResultItem {
                            kind: AskResultKind::Symbol,
                            id: Some(symbol.symbol_id),
                            title: Some(symbol.qualified_name.clone()),
                            snippet: symbol.qualified_name,
                            relevance_score: boosted,
                            file: Some(symbol.file_path),
                            language: Some(symbol.language),
                            tags: Vec::new(),
                            source_type: None,
                            test_file: None,
                            fused_score: None,
                            coupling_type: None,
                        },
                    },
                )
            })
        })
        .collect::<Vec<_>>();

    ranked.sort_by(|left, right| {
        right
            .0
            .partial_cmp(&left.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.1.key.cmp(&right.1.key))
    });
    ranked.into_iter().map(|(_, entry)| entry).collect()
}

fn rank_note_candidates(
    lexical: Vec<aether_store::ProjectNoteRecord>,
    semantic: Vec<aether_store::ProjectNoteRecord>,
    now_ms: i64,
) -> Vec<RankedCandidate> {
    let mut by_id = HashMap::<String, aether_store::ProjectNoteRecord>::new();
    let mut score_by_id = HashMap::<String, f32>::new();

    for (rank, row) in lexical.iter().enumerate() {
        by_id
            .entry(row.note_id.clone())
            .or_insert_with(|| row.clone());
        *score_by_id.entry(row.note_id.clone()).or_insert(0.0) += rrf_score(rank);
    }
    for (rank, row) in semantic.iter().enumerate() {
        by_id
            .entry(row.note_id.clone())
            .or_insert_with(|| row.clone());
        *score_by_id.entry(row.note_id.clone()).or_insert(0.0) += rrf_score(rank);
    }

    let mut ranked = score_by_id
        .into_iter()
        .filter_map(|(note_id, score)| {
            by_id.remove(note_id.as_str()).map(|record| {
                let note = project_note_from_store(record);
                let boosted = apply_recency_access_boost(
                    score,
                    note.access_count,
                    note.last_accessed_at,
                    now_ms,
                );
                (
                    boosted,
                    RankedCandidate {
                        key: format!("note:{}", note.note_id),
                        access_count: note.access_count,
                        last_accessed_at: note.last_accessed_at,
                        item: AskResultItem {
                            kind: AskResultKind::Note,
                            id: Some(note.note_id),
                            title: None,
                            snippet: compact_snippet(note.content.as_str()),
                            relevance_score: boosted,
                            file: None,
                            language: None,
                            tags: note.tags,
                            source_type: Some(note.source_type),
                            test_file: None,
                            fused_score: None,
                            coupling_type: None,
                        },
                    },
                )
            })
        })
        .collect::<Vec<_>>();

    ranked.sort_by(|left, right| {
        right
            .0
            .partial_cmp(&left.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.1.key.cmp(&right.1.key))
    });
    ranked.into_iter().map(|(_, entry)| entry).collect()
}

fn rank_test_candidates(test_rows: Vec<aether_store::TestIntentRecord>) -> Vec<RankedCandidate> {
    test_rows
        .into_iter()
        .enumerate()
        .map(|(rank, row)| {
            let score = rrf_score(rank);
            RankedCandidate {
                key: format!("test:{}", row.intent_id),
                access_count: 0,
                last_accessed_at: None,
                item: AskResultItem {
                    kind: AskResultKind::TestGuard,
                    id: Some(row.intent_id),
                    title: Some(row.test_name),
                    snippet: compact_snippet(row.intent_text.as_str()),
                    relevance_score: score,
                    file: None,
                    language: None,
                    tags: Vec::new(),
                    source_type: None,
                    test_file: Some(row.file_path),
                    fused_score: None,
                    coupling_type: None,
                },
            }
        })
        .collect()
}

fn rank_coupling_candidates(
    workspace: &Path,
    symbol_candidates: &[RankedCandidate],
) -> Result<Vec<RankedCandidate>, MemoryError> {
    let cozo = match CozoGraphStore::open(workspace) {
        Ok(cozo) => cozo,
        Err(_) => return Ok(Vec::new()),
    };

    let mut by_file = HashMap::<String, CouplingAggregate>::new();
    for symbol in symbol_candidates.iter().take(SYMBOL_COUPLING_SEED_LIMIT) {
        let Some(anchor_file) = symbol.item.file.as_deref() else {
            continue;
        };
        let edges = cozo.list_co_change_edges_for_file(anchor_file, 0.0)?;
        for edge in edges {
            let coupled_file = coupled_file_for_edge(anchor_file, &edge);
            if coupled_file.is_empty() {
                continue;
            }

            let aggregate = CouplingAggregate {
                coupled_file: coupled_file.clone(),
                anchor_file: anchor_file.to_owned(),
                git_coupling: edge.git_coupling.clamp(0.0, 1.0),
                fused_score: edge.fused_score.clamp(0.0, 1.0),
                coupling_type: edge.coupling_type,
            };

            by_file
                .entry(coupled_file)
                .and_modify(|current| {
                    if aggregate.fused_score > current.fused_score {
                        *current = aggregate.clone();
                    }
                })
                .or_insert(aggregate);
        }
    }

    let mut ranked = by_file.into_values().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .fused_score
            .partial_cmp(&left.fused_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.coupled_file.cmp(&right.coupled_file))
    });

    Ok(ranked
        .into_iter()
        .map(|row| RankedCandidate {
            key: format!("coupling:{}", row.coupled_file),
            access_count: 0,
            last_accessed_at: None,
            item: AskResultItem {
                kind: AskResultKind::CoupledFile,
                id: None,
                title: Some(row.coupled_file),
                snippet: format!(
                    "Co-changes with {} in {:.0}% of commits ({} coupling, type: {})",
                    row.anchor_file,
                    row.git_coupling * 100.0,
                    risk_label(row.fused_score),
                    row.coupling_type
                ),
                relevance_score: row.fused_score,
                file: None,
                language: None,
                tags: Vec::new(),
                source_type: None,
                test_file: None,
                fused_score: Some(row.fused_score),
                coupling_type: Some(row.coupling_type),
            },
        })
        .collect())
}

fn coupled_file_for_edge(anchor_file: &str, edge: &CouplingEdgeRecord) -> String {
    if edge.file_a == anchor_file {
        return edge.file_b.clone();
    }
    if edge.file_b == anchor_file {
        return edge.file_a.clone();
    }
    String::new()
}

fn risk_label(score: f32) -> &'static str {
    if score >= 0.7 {
        return "Critical";
    }
    if score >= 0.4 {
        return "High";
    }
    if score >= 0.2 {
        return "Medium";
    }
    "Low"
}

fn merge_candidates(
    now_ms: i64,
    limit: usize,
    symbols: Vec<RankedCandidate>,
    notes: Vec<RankedCandidate>,
    tests: Vec<RankedCandidate>,
    coupling: Vec<RankedCandidate>,
) -> AskQueryResult {
    let mut by_key = HashMap::<String, RankedCandidate>::new();
    let mut score_by_key = HashMap::<String, f32>::new();

    add_rrf_scores(&mut by_key, &mut score_by_key, symbols.as_slice());
    add_rrf_scores(&mut by_key, &mut score_by_key, notes.as_slice());
    add_rrf_scores(&mut by_key, &mut score_by_key, tests.as_slice());
    add_rrf_scores(&mut by_key, &mut score_by_key, coupling.as_slice());

    let mut merged = score_by_key
        .into_iter()
        .filter_map(|(key, score)| {
            by_key.remove(key.as_str()).map(|candidate| {
                let boosted = apply_recency_access_boost(
                    score,
                    candidate.access_count,
                    candidate.last_accessed_at,
                    now_ms,
                );
                (key, boosted, candidate.item)
            })
        })
        .collect::<Vec<_>>();
    merged.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });
    merged.truncate(limit);

    let max_score = merged
        .iter()
        .map(|(_, score, _)| *score)
        .fold(0.0f32, f32::max)
        .max(1.0);
    let results = merged
        .into_iter()
        .map(|(_, score, mut item)| {
            item.relevance_score = (score / max_score).clamp(0.0, 1.0);
            item
        })
        .collect::<Vec<_>>();

    AskQueryResult {
        query: String::new(),
        results,
    }
}

fn add_rrf_scores(
    by_key: &mut HashMap<String, RankedCandidate>,
    score_by_key: &mut HashMap<String, f32>,
    ranked: &[RankedCandidate],
) {
    for (rank, candidate) in ranked.iter().enumerate() {
        by_key
            .entry(candidate.key.clone())
            .or_insert_with(|| candidate.clone());
        *score_by_key.entry(candidate.key.clone()).or_insert(0.0) += rrf_score(rank);
    }
}

fn compact_snippet(value: &str) -> String {
    const LIMIT: usize = 180;
    let trimmed = value.trim();
    if trimmed.len() <= LIMIT {
        return trimmed.to_owned();
    }

    let mut end = LIMIT;
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &trimmed[..end])
}

fn enrich_symbol_snippets(
    store: &SqliteStore,
    results: &mut [AskResultItem],
) -> Result<(), MemoryError> {
    for item in results {
        if item.kind != AskResultKind::Symbol {
            continue;
        }
        let Some(symbol_id) = item.id.as_deref() else {
            continue;
        };
        let Some(blob) = store.read_sir_blob(symbol_id)? else {
            continue;
        };
        let parsed = match serde_json::from_str::<serde_json::Value>(blob.as_str()) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let Some(intent) = parsed
            .get("intent")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        item.snippet = compact_snippet(intent);
    }
    Ok(())
}

fn increment_access_from_results(
    store: &SqliteStore,
    results: &mut [AskResultItem],
    now_ms: i64,
) -> Result<(), MemoryError> {
    let mut symbol_ids = Vec::<String>::new();
    let mut note_ids = Vec::<String>::new();

    for item in &*results {
        match item.kind {
            AskResultKind::Symbol => {
                if let Some(id) = item.id.as_deref() {
                    symbol_ids.push(id.to_owned());
                }
            }
            AskResultKind::Note => {
                if let Some(id) = item.id.as_deref() {
                    note_ids.push(id.to_owned());
                }
            }
            AskResultKind::TestGuard | AskResultKind::CoupledFile => {}
        }
    }

    store.increment_symbol_access(symbol_ids.as_slice(), now_ms)?;
    store.increment_project_note_access(note_ids.as_slice(), now_ms)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use aether_store::{CouplingEdgeRecord, CozoGraphStore};

    use super::{AskResultItem, AskResultKind, RankedCandidate, merge_candidates};

    fn test_workspace(name: &str) -> PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let seq = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "aether-memory-unified-{name}-{}-{seq}",
            std::process::id()
        ));
        if path.exists() {
            let _ = fs::remove_dir_all(&path);
        }
        fs::create_dir_all(&path).expect("create workspace dir");
        path
    }

    #[test]
    fn merge_candidates_returns_mixed_result_types() {
        let symbols = vec![RankedCandidate {
            key: "symbol:a".to_owned(),
            access_count: 0,
            last_accessed_at: None,
            item: AskResultItem {
                kind: AskResultKind::Symbol,
                id: Some("a".to_owned()),
                title: Some("a".to_owned()),
                snippet: "a".to_owned(),
                relevance_score: 0.0,
                file: Some("src/a.rs".to_owned()),
                language: Some("rust".to_owned()),
                tags: Vec::new(),
                source_type: None,
                test_file: None,
                fused_score: None,
                coupling_type: None,
            },
        }];
        let notes = vec![RankedCandidate {
            key: "note:n1".to_owned(),
            access_count: 0,
            last_accessed_at: None,
            item: AskResultItem {
                kind: AskResultKind::Note,
                id: Some("n1".to_owned()),
                title: None,
                snippet: "n1".to_owned(),
                relevance_score: 0.0,
                file: None,
                language: None,
                tags: Vec::new(),
                source_type: Some("session".to_owned()),
                test_file: None,
                fused_score: None,
                coupling_type: None,
            },
        }];
        let tests = vec![RankedCandidate {
            key: "test:t1".to_owned(),
            access_count: 0,
            last_accessed_at: None,
            item: AskResultItem {
                kind: AskResultKind::TestGuard,
                id: Some("t1".to_owned()),
                title: Some("t1".to_owned()),
                snippet: "t1".to_owned(),
                relevance_score: 0.0,
                file: None,
                language: None,
                tags: Vec::new(),
                source_type: None,
                test_file: Some("tests/a.rs".to_owned()),
                fused_score: None,
                coupling_type: None,
            },
        }];
        let coupling = vec![RankedCandidate {
            key: "coupling:c1".to_owned(),
            access_count: 0,
            last_accessed_at: None,
            item: AskResultItem {
                kind: AskResultKind::CoupledFile,
                id: None,
                title: Some("src/c.rs".to_owned()),
                snippet: "c1".to_owned(),
                relevance_score: 0.0,
                file: None,
                language: None,
                tags: Vec::new(),
                source_type: None,
                test_file: None,
                fused_score: Some(0.9),
                coupling_type: Some("multi".to_owned()),
            },
        }];

        let result = merge_candidates(1_700_000_000_000, 10, symbols, notes, tests, coupling);
        assert_eq!(result.results.len(), 4);
        let kinds = result
            .results
            .iter()
            .map(|item| item.kind)
            .collect::<Vec<_>>();
        assert!(kinds.contains(&AskResultKind::Symbol));
        assert!(kinds.contains(&AskResultKind::Note));
        assert!(kinds.contains(&AskResultKind::TestGuard));
        assert!(kinds.contains(&AskResultKind::CoupledFile));
    }

    #[test]
    fn cross_type_rrf_merging_normalizes_scores() {
        let symbols = vec![RankedCandidate {
            key: "symbol:a".to_owned(),
            access_count: 0,
            last_accessed_at: None,
            item: AskResultItem {
                kind: AskResultKind::Symbol,
                id: Some("a".to_owned()),
                title: Some("a".to_owned()),
                snippet: "a".to_owned(),
                relevance_score: 0.0,
                file: Some("src/a.rs".to_owned()),
                language: Some("rust".to_owned()),
                tags: Vec::new(),
                source_type: None,
                test_file: None,
                fused_score: None,
                coupling_type: None,
            },
        }];

        let result = merge_candidates(
            1_700_000_000_000,
            10,
            symbols,
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(result.results.len(), 1);
        for entry in result.results {
            assert!((0.0..=1.0).contains(&entry.relevance_score));
        }
    }

    #[test]
    fn cross_type_ranking_prefers_more_accessed_item_when_base_is_equal() {
        let symbols = vec![RankedCandidate {
            key: "symbol:a".to_owned(),
            access_count: 0,
            last_accessed_at: None,
            item: AskResultItem {
                kind: AskResultKind::Symbol,
                id: Some("a".to_owned()),
                title: Some("a".to_owned()),
                snippet: "a".to_owned(),
                relevance_score: 0.0,
                file: Some("src/a.rs".to_owned()),
                language: Some("rust".to_owned()),
                tags: Vec::new(),
                source_type: None,
                test_file: None,
                fused_score: None,
                coupling_type: None,
            },
        }];
        let notes = vec![RankedCandidate {
            key: "note:n1".to_owned(),
            access_count: 200,
            last_accessed_at: Some(1_700_000_000_000),
            item: AskResultItem {
                kind: AskResultKind::Note,
                id: Some("n1".to_owned()),
                title: None,
                snippet: "n1".to_owned(),
                relevance_score: 0.0,
                file: None,
                language: None,
                tags: Vec::new(),
                source_type: Some("session".to_owned()),
                test_file: None,
                fused_score: None,
                coupling_type: None,
            },
        }];

        let result = merge_candidates(
            1_700_000_000_000,
            10,
            symbols,
            notes,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(result.results.len(), 2);
        assert_eq!(result.results[0].kind, AskResultKind::Note);
    }

    #[test]
    fn coupling_candidates_are_derived_from_top_symbol_files() {
        let workspace = test_workspace("coupling");
        let cozo = CozoGraphStore::open(&workspace).expect("open cozo");
        cozo.upsert_co_change_edges(&[CouplingEdgeRecord {
            file_a: "src/a.rs".to_owned(),
            file_b: "src/b.rs".to_owned(),
            co_change_count: 6,
            total_commits_a: 10,
            total_commits_b: 8,
            git_coupling: 0.75,
            static_signal: 0.6,
            semantic_signal: 0.4,
            fused_score: 0.7,
            coupling_type: "multi".to_owned(),
            last_co_change_commit: "abc123".to_owned(),
            last_co_change_at: 1_700_000_000,
            mined_at: 1_700_000_100,
        }])
        .expect("upsert co-change edge");
        drop(cozo);

        let symbols = vec![RankedCandidate {
            key: "symbol:a".to_owned(),
            access_count: 0,
            last_accessed_at: None,
            item: AskResultItem {
                kind: AskResultKind::Symbol,
                id: Some("sym-a".to_owned()),
                title: Some("a".to_owned()),
                snippet: "a".to_owned(),
                relevance_score: 0.0,
                file: Some("src/a.rs".to_owned()),
                language: Some("rust".to_owned()),
                tags: Vec::new(),
                source_type: None,
                test_file: None,
                fused_score: None,
                coupling_type: None,
            },
        }];

        let coupling =
            super::rank_coupling_candidates(&workspace, symbols.as_slice()).expect("rank coupling");
        assert_eq!(coupling.len(), 1);
        assert_eq!(coupling[0].item.kind, AskResultKind::CoupledFile);
        assert_eq!(coupling[0].item.title.as_deref(), Some("src/b.rs"));
    }
}
