use std::collections::HashMap;

use aether_core::{
    SEARCH_FALLBACK_EMBEDDINGS_DISABLED, SEARCH_FALLBACK_SEMANTIC_INDEX_NOT_READY, SearchMode,
};
use serde::{Deserialize, Serialize};

use crate::note::{ProjectMemoryService, ProjectNote, normalize_tags, project_note_from_store};
use crate::ranking::{apply_recency_access_boost, rrf_score};
use crate::{MemoryError, current_unix_timestamp_millis};
use aether_store::{Store, open_vector_store};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticQuery {
    pub provider: String,
    pub model: String,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecallRequest {
    pub query: String,
    pub mode: SearchMode,
    pub limit: u32,
    pub include_archived: bool,
    pub tags_filter: Vec<String>,
    pub now_ms: Option<i64>,
    pub semantic: Option<SemanticQuery>,
    pub semantic_fallback_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecallScoredNote {
    pub note: ProjectNote,
    pub relevance_score: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecallResult {
    pub mode_requested: SearchMode,
    pub mode_used: SearchMode,
    pub fallback_reason: Option<String>,
    pub notes: Vec<RecallScoredNote>,
}

impl ProjectMemoryService {
    pub async fn recall(&self, request: RecallRequest) -> Result<RecallResult, MemoryError> {
        let limit = request.limit.clamp(1, 100);
        let now_ms = request
            .now_ms
            .unwrap_or_else(current_unix_timestamp_millis)
            .max(0);
        let tags_filter = normalize_tags(request.tags_filter);

        let store = self.open_store()?;
        let candidate_limit = limit.max(20);
        let lexical = store
            .search_project_notes_lexical(
                request.query.as_str(),
                candidate_limit,
                request.include_archived,
                tags_filter.as_slice(),
            )?
            .into_iter()
            .map(project_note_from_store)
            .collect::<Vec<_>>();

        let mut semantic = Vec::<(ProjectNote, f32)>::new();
        let mut semantic_fallback = request.semantic_fallback_reason;
        if !matches!(request.mode, SearchMode::Lexical) {
            if let Some(semantic_query) = request.semantic {
                let vector_store = open_vector_store(self.workspace()).await?;
                let candidates = vector_store
                    .search_project_notes_nearest(
                        semantic_query.embedding.as_slice(),
                        semantic_query.provider.as_str(),
                        semantic_query.model.as_str(),
                        candidate_limit,
                    )
                    .await?;

                for candidate in candidates {
                    let Some(record) = store.get_project_note(candidate.note_id.as_str())? else {
                        continue;
                    };
                    let note = project_note_from_store(record);
                    if !request.include_archived && note.is_archived {
                        continue;
                    }
                    if !note_matches_all_tags(&note, tags_filter.as_slice()) {
                        continue;
                    }

                    semantic.push((note, candidate.semantic_score));
                }

                if semantic.is_empty() {
                    semantic_fallback = Some(SEARCH_FALLBACK_SEMANTIC_INDEX_NOT_READY.to_owned());
                }
            } else if semantic_fallback.is_none() {
                semantic_fallback = Some(SEARCH_FALLBACK_EMBEDDINGS_DISABLED.to_owned());
            }
        }

        let mut result = build_recall_from_candidates(
            request.mode,
            limit,
            now_ms,
            lexical,
            semantic,
            semantic_fallback,
        );

        let note_ids = result
            .notes
            .iter()
            .map(|entry| entry.note.note_id.clone())
            .collect::<Vec<_>>();
        store.increment_project_note_access(note_ids.as_slice(), now_ms)?;

        for entry in &mut result.notes {
            entry.note.access_count = entry.note.access_count.saturating_add(1);
            entry.note.last_accessed_at = Some(now_ms);
        }

        Ok(result)
    }
}

pub(crate) fn build_recall_from_candidates(
    mode_requested: SearchMode,
    limit: u32,
    now_ms: i64,
    lexical: Vec<ProjectNote>,
    semantic: Vec<(ProjectNote, f32)>,
    semantic_fallback_reason: Option<String>,
) -> RecallResult {
    let limit = limit.clamp(1, 100) as usize;

    let (mode_used, fallback_reason, mut notes) = match mode_requested {
        SearchMode::Lexical => (
            SearchMode::Lexical,
            None,
            lexical_ranked(lexical.as_slice(), now_ms, limit),
        ),
        SearchMode::Semantic => {
            if semantic.is_empty() {
                (
                    SearchMode::Lexical,
                    semantic_fallback_reason,
                    lexical_ranked(lexical.as_slice(), now_ms, limit),
                )
            } else {
                (
                    SearchMode::Semantic,
                    None,
                    semantic_ranked(semantic.as_slice(), now_ms, limit),
                )
            }
        }
        SearchMode::Hybrid => {
            if semantic.is_empty() {
                (
                    SearchMode::Lexical,
                    semantic_fallback_reason,
                    lexical_ranked(lexical.as_slice(), now_ms, limit),
                )
            } else {
                (
                    SearchMode::Hybrid,
                    None,
                    hybrid_ranked(lexical.as_slice(), semantic.as_slice(), now_ms, limit),
                )
            }
        }
    };

    notes.truncate(limit);

    RecallResult {
        mode_requested,
        mode_used,
        fallback_reason,
        notes,
    }
}

fn lexical_ranked(lexical: &[ProjectNote], now_ms: i64, limit: usize) -> Vec<RecallScoredNote> {
    let mut scored = lexical
        .iter()
        .enumerate()
        .map(|(rank, note)| {
            let base = rrf_score(rank);
            RecallScoredNote {
                note: note.clone(),
                relevance_score: apply_recency_access_boost(
                    base,
                    note.access_count,
                    note.last_accessed_at,
                    now_ms,
                ),
            }
        })
        .collect::<Vec<_>>();
    sort_scored_notes(scored.as_mut_slice());
    scored.truncate(limit);
    scored
}

fn semantic_ranked(
    semantic: &[(ProjectNote, f32)],
    now_ms: i64,
    limit: usize,
) -> Vec<RecallScoredNote> {
    let mut scored = semantic
        .iter()
        .map(|(note, semantic_score)| RecallScoredNote {
            note: note.clone(),
            relevance_score: apply_recency_access_boost(
                *semantic_score,
                note.access_count,
                note.last_accessed_at,
                now_ms,
            ),
        })
        .collect::<Vec<_>>();

    sort_scored_notes(scored.as_mut_slice());
    scored.truncate(limit);
    scored
}

fn hybrid_ranked(
    lexical: &[ProjectNote],
    semantic: &[(ProjectNote, f32)],
    now_ms: i64,
    limit: usize,
) -> Vec<RecallScoredNote> {
    let mut by_id = HashMap::<String, &ProjectNote>::new();
    let mut score_by_id = HashMap::<String, f32>::new();

    for (rank, note) in lexical.iter().enumerate() {
        let id = note.note_id.clone();
        by_id.entry(id.clone()).or_insert(note);
        *score_by_id.entry(id).or_insert(0.0) += rrf_score(rank);
    }

    for (rank, (note, _semantic_score)) in semantic.iter().enumerate() {
        let id = note.note_id.clone();
        by_id.entry(id.clone()).or_insert(note);
        *score_by_id.entry(id).or_insert(0.0) += rrf_score(rank);
    }

    let mut scored = score_by_id
        .into_iter()
        .filter_map(|(id, base)| {
            by_id.get(id.as_str()).map(|note| RecallScoredNote {
                relevance_score: apply_recency_access_boost(
                    base,
                    note.access_count,
                    note.last_accessed_at,
                    now_ms,
                ),
                note: (*note).clone(),
            })
        })
        .collect::<Vec<_>>();

    sort_scored_notes(scored.as_mut_slice());
    scored.truncate(limit);
    scored
}

fn sort_scored_notes(entries: &mut [RecallScoredNote]) {
    entries.sort_by(|left, right| {
        right
            .relevance_score
            .partial_cmp(&left.relevance_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.note.note_id.cmp(&right.note.note_id))
    });
}

fn note_matches_all_tags(note: &ProjectNote, filter: &[String]) -> bool {
    if filter.is_empty() {
        return true;
    }

    filter
        .iter()
        .all(|tag| note.tags.iter().any(|note_tag| note_tag == tag))
}

#[cfg(test)]
mod tests {
    use aether_core::{SEARCH_FALLBACK_EMBEDDINGS_DISABLED, SearchMode};

    use super::build_recall_from_candidates;
    use crate::ProjectNote;

    fn note(
        note_id: &str,
        updated_at: i64,
        access_count: i64,
        last_accessed_at: Option<i64>,
        tags: &[&str],
    ) -> ProjectNote {
        ProjectNote {
            note_id: note_id.to_owned(),
            content: format!("content-{note_id}"),
            content_hash: format!("hash-{note_id}"),
            source_type: "manual".to_owned(),
            source_agent: None,
            tags: tags.iter().map(|item| (*item).to_owned()).collect(),
            entity_refs: Vec::new(),
            file_refs: Vec::new(),
            symbol_refs: Vec::new(),
            created_at: updated_at,
            updated_at,
            access_count,
            last_accessed_at,
            is_archived: false,
        }
    }

    #[test]
    fn hybrid_ranking_prefers_rrf_overlap() {
        let now = 1_700_000_100_000;
        let a = note("a", now, 0, None, &["architecture"]);
        let b = note("b", now, 0, None, &["architecture"]);
        let c = note("c", now, 0, None, &["architecture"]);

        let result = build_recall_from_candidates(
            SearchMode::Hybrid,
            10,
            now,
            vec![a.clone(), b.clone()],
            vec![(b.clone(), 0.95), (c.clone(), 0.94)],
            None,
        );

        let ids = result
            .notes
            .iter()
            .map(|item| item.note.note_id.clone())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["b".to_owned(), "a".to_owned(), "c".to_owned()]);
    }

    #[test]
    fn semantic_ranking_applies_recency_and_access_boosts() {
        let now = 1_700_000_200_000;
        let stale = note("stale", now - (45 * 24 * 60 * 60 * 1000), 0, None, &[]);
        let active = note(
            "active",
            now - (2 * 24 * 60 * 60 * 1000),
            10,
            Some(now - (2 * 24 * 60 * 60 * 1000)),
            &[],
        );

        let result = build_recall_from_candidates(
            SearchMode::Semantic,
            10,
            now,
            Vec::new(),
            vec![(stale, 0.80), (active, 0.80)],
            None,
        );

        assert_eq!(result.notes[0].note.note_id, "active");
        assert_eq!(result.mode_used, SearchMode::Semantic);
        assert_eq!(result.fallback_reason, None);
    }

    #[test]
    fn ranking_boost_changes_order_from_raw_score() {
        let now = 1_700_000_300_000;
        let stronger_raw = note("raw-stronger", now, 0, None, &[]);
        let boosted = note("boosted", now, 100, Some(now), &[]);

        let result = build_recall_from_candidates(
            SearchMode::Semantic,
            10,
            now,
            Vec::new(),
            vec![(stronger_raw, 0.80), (boosted, 0.79)],
            None,
        );

        let ids = result
            .notes
            .iter()
            .map(|entry| entry.note.note_id.clone())
            .collect::<Vec<_>>();
        assert_eq!(ids[0], "boosted");
        assert_eq!(ids[1], "raw-stronger");
    }

    #[test]
    fn semantic_mode_falls_back_to_lexical_when_semantic_unavailable() {
        let now = 1_700_000_300_000;
        let lexical = note("lexical", now, 0, None, &[]);

        let result = build_recall_from_candidates(
            SearchMode::Semantic,
            5,
            now,
            vec![lexical],
            Vec::new(),
            Some(SEARCH_FALLBACK_EMBEDDINGS_DISABLED.to_owned()),
        );

        assert_eq!(result.mode_used, SearchMode::Lexical);
        assert_eq!(
            result.fallback_reason.as_deref(),
            Some(SEARCH_FALLBACK_EMBEDDINGS_DISABLED)
        );
        assert_eq!(result.notes.len(), 1);
    }
}
