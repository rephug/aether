//! Resolution counts describe recorded call references, never whole-program recall.
use aether_store::SqliteStore;
use serde::{Deserialize, Serialize};

pub(crate) const OBSERVED_DISCONNECTION_NOTE: &str = "Disconnected from the largest observed dependency component; unresolved relationships may connect it. This does not establish unused or dead code";

const COVERAGE_NOTE: &str = "Observed graph only. Stored call counts are not recall; missing links cannot prove complete caller coverage or unused code.";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedGraphStatus {
    Incomplete,
    #[default]
    Unknown,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ObservedGraphCoverage {
    pub status: ObservedGraphStatus,
    pub whole_program_completeness: String,
    pub call_edges: Option<u64>,
    pub resolved: Option<u64>,
    pub ambiguous: Option<u64>,
    pub unresolved: Option<u64>,
    pub file_level_calls: Option<u64>,
    pub note: String,
}

impl Default for ObservedGraphCoverage {
    fn default() -> Self {
        Self {
            status: ObservedGraphStatus::Unknown,
            whole_program_completeness: "unknown".to_owned(),
            call_edges: None,
            resolved: None,
            ambiguous: None,
            unresolved: None,
            file_level_calls: None,
            note: COVERAGE_NOTE.to_owned(),
        }
    }
}

impl ObservedGraphCoverage {
    /// Graph data may remain usable when the separate count query is unavailable.
    pub fn from_store(store: &SqliteStore) -> Self {
        match store.call_edge_coverage_by_language() {
            Ok(rows) => {
                let (mut calls, mut resolved, mut ambiguous, mut unresolved, mut file_calls) =
                    (0, 0, 0, 0, 0);
                for row in rows {
                    calls += row.symbol_calls.call_edges;
                    resolved += row.symbol_calls.resolved;
                    ambiguous += row.symbol_calls.ambiguous;
                    unresolved += row.symbol_calls.unresolved;
                    file_calls += row.file_level_calls;
                }
                Self {
                    status: if ambiguous + unresolved + file_calls > 0 {
                        ObservedGraphStatus::Incomplete
                    } else {
                        ObservedGraphStatus::Unknown
                    },
                    call_edges: Some(calls),
                    resolved: Some(resolved),
                    ambiguous: Some(ambiguous),
                    unresolved: Some(unresolved),
                    file_level_calls: Some(file_calls),
                    ..Self::default()
                }
            }
            Err(err) => Self {
                status: ObservedGraphStatus::Unavailable,
                note: format!("Stored call-reference coverage unavailable: {err}. {COVERAGE_NOTE}"),
                ..Self::default()
            },
        }
    }

    pub fn unavailable(reason: impl std::fmt::Display) -> Self {
        Self {
            status: ObservedGraphStatus::Unavailable,
            note: format!("Observed dependency graph unavailable: {reason}. {COVERAGE_NOTE}"),
            ..Self::default()
        }
    }
}

/// Entry-point check for native analyses handed an already-open graph.
pub(crate) fn require_native_graph_projection(
    store: &SqliteStore,
) -> Result<(), aether_store::StoreError> {
    if store.has_graph_projection_provenance()? {
        Ok(())
    } else {
        Err(aether_store::StoreError::Compatibility(
            "Persistent dependency graph proof is unavailable; run writable indexing/startup and structural reindex before reading graph relationships".to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::{EdgeKind, SymbolEdge};
    use aether_store::{SymbolCatalogStore, SymbolRecord, SymbolRelationStore};

    #[test]
    fn zero_and_fully_resolved_references_never_claim_completeness() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("store");
        let empty = ObservedGraphCoverage::from_store(&store);
        assert_eq!(empty.status, ObservedGraphStatus::Unknown);
        assert_eq!(empty.call_edges, Some(0));
        assert_eq!(empty.whole_program_completeness, "unknown");
        for id in ["caller", "helper"] {
            store
                .upsert_symbol(SymbolRecord {
                    id: id.to_owned(),
                    file_path: "src/lib.rs".to_owned(),
                    language: "rust".to_owned(),
                    kind: "function".to_owned(),
                    qualified_name: id.to_owned(),
                    signature_fingerprint: id.to_owned(),
                    last_seen_at: 1,
                })
                .expect("symbol");
        }
        let edge = |name: &str, target: Option<&str>| SymbolEdge {
            source_id: "caller".to_owned(),
            target_qualified_name: name.to_owned(),
            target_symbol_id: target.map(str::to_owned),
            target_file_path: None,
            edge_kind: EdgeKind::Calls,
            file_path: "src/lib.rs".to_owned(),
        };
        store
            .upsert_edges(&[edge("helper", Some("helper"))])
            .expect("proved call");
        let proved = ObservedGraphCoverage::from_store(&store);
        assert_eq!(proved.status, ObservedGraphStatus::Unknown);
        assert_eq!(proved.resolved, Some(1));
        assert_eq!(proved.unresolved, Some(0));
        assert_eq!(proved.whole_program_completeness, "unknown");
        let mut file_call = edge("helper", Some("helper"));
        file_call.source_id = aether_core::file_source_id("src/lib.rs");
        store.upsert_edges(&[file_call]).expect("file-level call");
        let file_level = ObservedGraphCoverage::from_store(&store);
        assert_eq!(file_level.status, ObservedGraphStatus::Incomplete);
        assert_eq!(file_level.call_edges, Some(1));
        assert_eq!(file_level.file_level_calls, Some(1));
        store
            .upsert_edges(&[edge("unknown", None)])
            .expect("unresolved call");
        let partial = ObservedGraphCoverage::from_store(&store);
        assert_eq!(partial.status, ObservedGraphStatus::Incomplete);
        assert_eq!(partial.resolved, Some(1));
        assert_eq!(partial.unresolved, Some(1));
    }

    #[test]
    fn unavailable_is_distinct_from_usable_unknown_graph() {
        let coverage = ObservedGraphCoverage::unavailable("reindex_required");
        assert_eq!(coverage.status, ObservedGraphStatus::Unavailable);
        assert_eq!(coverage.call_edges, None);
        assert!(coverage.note.contains("reindex_required"));
    }
}
