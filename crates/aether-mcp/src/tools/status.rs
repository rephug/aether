use aether_core::normalize_path;
use aether_store::LanguageCallEdgeCoverage;
use aetherd::freshness::{FreshnessVerdict, IndexFreshness, assess_index_freshness};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{AetherMcpServer, MCP_SCHEMA_VERSION, current_unix_timestamp};
use crate::AetherMcpError;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherStatusResponse {
    pub schema_version: u32,
    pub generated_at: i64,
    pub workspace: String,
    pub store_present: bool,
    pub sqlite_path: String,
    pub sir_dir: String,
    pub symbol_count: i64,
    pub sir_count: i64,
    pub sir_coverage: SirCoverageStats,
    /// How far the index has drifted from the workspace. Read `freshness.summary` before
    /// trusting any other tool's output.
    pub freshness: AetherIndexFreshness,
    /// How many recorded call edges the dependency graph resolves, by language.
    pub call_graph: AetherCallGraphCoverage,
}

/// How call edges are matched to symbols, and the counts per language.
pub const CALL_GRAPH_RULE: &str = "A call's target text matches workspace symbols by exact \
qualified name in the caller's language family, as the stored graph links them. resolved: \
one match. ambiguous: several, and the graph links every one. unresolved: no exact match, so \
the graph drops the edge; many are external or standard-library calls, but they may be \
internal, since Rust method calls are recorded by bare method name and a call through a path \
or alias doesn't spell the qualified name. file_level_calls come from top-level code and are \
never attributed to a caller. Rust calls written inside macro arguments (format!, assert!, \
vec!, ...) are not recorded, so no count includes them.";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherCallGraphCoverage {
    /// How call edges are matched to symbols, and what each count means.
    pub rule: String,
    pub languages: Vec<AetherLanguageCallCoverage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherLanguageCallCoverage {
    /// `unknown` for edges of files without indexed symbols.
    pub language: String,
    /// Calls whose caller is an indexed symbol: what aether_dependencies and
    /// aether_call_chain walk.
    pub call_edges: u64,
    pub resolved: u64,
    pub ambiguous: u64,
    pub unresolved: u64,
    /// Calls from top-level code, attributed to no caller.
    pub file_level_calls: u64,
}

impl From<LanguageCallEdgeCoverage> for AetherLanguageCallCoverage {
    fn from(entry: LanguageCallEdgeCoverage) -> Self {
        Self {
            language: if entry.language.is_empty() {
                "unknown".to_owned()
            } else {
                entry.language
            },
            call_edges: entry.symbol_calls.call_edges,
            resolved: entry.symbol_calls.resolved,
            ambiguous: entry.symbol_calls.ambiguous,
            unresolved: entry.symbol_calls.unresolved,
            file_level_calls: entry.file_level_calls,
        }
    }
}

/// See [`AetherMcpServer::aether_store_counts_logic`].
#[derive(Debug, Clone)]
pub struct AetherStoreCounts {
    pub store_present: bool,
    pub symbol_count: i64,
    pub sir_count: i64,
    pub sir_coverage: SirCoverageStats,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SirCoverageStats {
    pub total_symbols: u64,
    pub symbols_with_sir: u64,
    pub percentage: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherIndexFreshness {
    /// `fresh` (no file or HEAD change since the index, so symbols and edges are current),
    /// `stale` (files or HEAD changed since the index) or `empty`. `fresh` says nothing
    /// about the `sirs_without_source_hash` SIRs, whose staleness can't be checked; the
    /// summary then reads "Structural index fresh" and names how many there are.
    pub verdict: String,
    /// One line on how fresh the index is.
    pub summary: String,
    /// What makes the index stale or uncertain, one sentence each.
    pub notes: Vec<String>,
    /// Unix seconds when the index last caught up with `indexed_head`: a full sync that
    /// read every path, or a git-triggered reindex that covered every change since the
    /// previous indexed commit. A skipped or failed reindex doesn't move it. Null when
    /// nothing recorded one.
    pub last_sync_at: Option<i64>,
    /// Unix seconds when a full sync (index-once or daemon start) last read every path.
    pub last_full_sync_at: Option<i64>,
    /// Unix seconds of the latest symbol write.
    pub last_symbol_update_at: Option<i64>,
    /// Unix seconds of the latest SIR write.
    pub last_sir_update_at: Option<i64>,
    /// The commit the index reflects: every change committed up to it is indexed. Null
    /// when not recorded.
    pub indexed_head: Option<String>,
    pub workspace_head: Option<String>,
    /// Null when either HEAD is unknown.
    pub head_matches: Option<bool>,
    pub indexed_files: u64,
    /// Indexed files modified after the indexer last read them, by modification time: a
    /// file touched or checked out again without a content change counts too.
    pub modified_files: u64,
    /// Indexed files that no longer exist.
    pub deleted_files: u64,
    /// Source files without indexed symbols that appeared or changed since the last full
    /// sync started and that no reindex has read since.
    pub new_files: u64,
    /// Indexed files, and paths of the workspace walk, whose metadata couldn't be read
    /// (permissions, a filesystem error). They may have changed, so while any remain the
    /// verdict is `stale`.
    pub unreadable_files: u64,
    /// Up to ten changed paths, prefixed `M ` (modified), `D ` (deleted), `A ` (new) or
    /// `? ` (couldn't be checked).
    pub changed_file_samples: Vec<String>,
    /// SIRs of modified or deleted files: they may describe code that is gone.
    pub sirs_in_changed_files: u64,
    /// SIRs marked stale because their symbol changed after they were written.
    pub stale_sirs: u64,
    /// SIRs without a recorded source hash: an edit made while no daemon watched can't
    /// mark them stale.
    pub sirs_without_source_hash: u64,
    /// File and module rollup SIRs marked stale.
    pub stale_rollups: u64,
    /// Whether an `aetherd` daemon is watching the workspace (it holds
    /// `.aether/watcher.lock`).
    pub daemon_running: bool,
}

impl From<IndexFreshness> for AetherIndexFreshness {
    fn from(freshness: IndexFreshness) -> Self {
        Self {
            verdict: freshness.verdict.as_str().to_owned(),
            summary: freshness.summary,
            notes: freshness.notes,
            last_sync_at: freshness.last_sync_at,
            last_full_sync_at: freshness.last_full_sync_at,
            last_symbol_update_at: freshness.last_symbol_update_at,
            last_sir_update_at: freshness.last_sir_update_at,
            indexed_head: freshness.indexed_head,
            workspace_head: freshness.workspace_head,
            head_matches: freshness.head_matches,
            indexed_files: freshness.indexed_files,
            modified_files: freshness.modified_files,
            deleted_files: freshness.deleted_files,
            new_files: freshness.new_files,
            unreadable_files: freshness.unreadable_files,
            changed_file_samples: freshness.changed_file_samples,
            sirs_in_changed_files: freshness.sirs_in_changed_files,
            stale_sirs: freshness.sir.stale_leaves,
            sirs_without_source_hash: freshness.sir.leaves_without_source_hash,
            stale_rollups: freshness.sir.stale_rollups,
            daemon_running: freshness.watcher_running,
        }
    }
}

impl AetherIndexFreshness {
    fn missing_store() -> Self {
        Self {
            verdict: FreshnessVerdict::Empty.as_str().to_owned(),
            summary: "No index: run `aetherd --workspace . --index-once` to build one.".to_owned(),
            notes: Vec::new(),
            last_sync_at: None,
            last_full_sync_at: None,
            last_symbol_update_at: None,
            last_sir_update_at: None,
            indexed_head: None,
            workspace_head: None,
            head_matches: None,
            indexed_files: 0,
            modified_files: 0,
            deleted_files: 0,
            new_files: 0,
            unreadable_files: 0,
            changed_file_samples: Vec::new(),
            sirs_in_changed_files: 0,
            stale_sirs: 0,
            sirs_without_source_hash: 0,
            stale_rollups: 0,
            daemon_running: false,
        }
    }
}

impl AetherMcpServer {
    /// Whether a store is present, and its symbol and SIR counts: the cheap part of
    /// `aether_status`, without the workspace walk and coverage queries.
    pub fn aether_store_counts_logic(&self) -> Result<AetherStoreCounts, AetherMcpError> {
        let store_present = self.sqlite_path().exists() && self.sir_dir().is_dir();
        let (symbol_count, sir_count, sir_coverage) = if store_present {
            let (total_symbols, symbols_with_sir) = self.state.store.count_symbols_with_sir()?;
            let percentage = if total_symbols > 0 {
                (symbols_with_sir as f64 / total_symbols as f64).clamp(0.0, 1.0)
            } else {
                0.0
            };
            (
                total_symbols as i64,
                symbols_with_sir as i64,
                SirCoverageStats {
                    total_symbols: total_symbols as u64,
                    symbols_with_sir: symbols_with_sir as u64,
                    percentage,
                },
            )
        } else {
            (
                0,
                0,
                SirCoverageStats {
                    total_symbols: 0,
                    symbols_with_sir: 0,
                    percentage: 0.0,
                },
            )
        };
        Ok(AetherStoreCounts {
            store_present,
            symbol_count,
            sir_count,
            sir_coverage,
        })
    }

    pub fn aether_status_logic(&self) -> Result<AetherStatusResponse, AetherMcpError> {
        let sqlite_path = self.sqlite_path();
        let sir_dir = self.sir_dir();
        let AetherStoreCounts {
            store_present,
            symbol_count,
            sir_count,
            sir_coverage,
        } = self.aether_store_counts_logic()?;

        let languages = if store_present {
            self.state
                .store
                .call_edge_coverage_by_language()?
                .into_iter()
                .map(AetherLanguageCallCoverage::from)
                .collect()
        } else {
            Vec::new()
        };
        let call_graph = AetherCallGraphCoverage {
            rule: CALL_GRAPH_RULE.to_owned(),
            languages,
        };

        let freshness = if store_present {
            assess_index_freshness(self.workspace(), self.state.store.as_ref())
                .map_err(|err| AetherMcpError::Message(format!("index freshness: {err:#}")))?
                .into()
        } else {
            AetherIndexFreshness::missing_store()
        };

        Ok(AetherStatusResponse {
            schema_version: MCP_SCHEMA_VERSION,
            generated_at: current_unix_timestamp(),
            workspace: normalize_path(&self.state.workspace.to_string_lossy()),
            store_present,
            sqlite_path: normalize_path(&sqlite_path.to_string_lossy()),
            sir_dir: normalize_path(&sir_dir.to_string_lossy()),
            symbol_count,
            sir_count,
            sir_coverage,
            freshness,
            call_graph,
        })
    }
}
