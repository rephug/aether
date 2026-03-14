use std::collections::{HashMap, HashSet};

use aether_store::{SymbolCatalogStore, SymbolRelationStore, SymbolSearchResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    AetherMcpServer, MEMORY_SCHEMA_VERSION, child_method_symbols,
    normalize_workspace_relative_path, symbol_leaf_name,
};
use crate::AetherMcpError;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherUsageMatrixRequest {
    /// Symbol name (e.g., "Store", "SqliteStore")
    pub symbol: String,
    /// Optional file path to disambiguate
    pub file: Option<String>,
    /// Optional kind filter (e.g., "trait", "struct")
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherUsageMatrixResponse {
    pub schema_version: String,
    pub target: String,
    pub target_file: String,
    pub method_count: u32,
    pub consumer_count: u32,
    pub matrix: Vec<ConsumerRow>,
    pub method_consumers: Vec<MethodConsumers>,
    pub uncalled_methods: Vec<String>,
    pub suggested_clusters: Vec<MethodCluster>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConsumerRow {
    pub consumer_file: String,
    pub methods_used: Vec<String>,
    pub method_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MethodConsumers {
    pub method: String,
    pub consumer_files: Vec<String>,
    pub consumer_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MethodCluster {
    pub cluster_name: String,
    pub methods: Vec<String>,
    pub shared_consumers: Vec<String>,
    pub reason: String,
}

impl AetherMcpServer {
    pub fn aether_usage_matrix_logic(
        &self,
        request: AetherUsageMatrixRequest,
    ) -> Result<AetherUsageMatrixResponse, AetherMcpError> {
        let symbol_query = request.symbol.trim();
        if symbol_query.is_empty() {
            return Err(AetherMcpError::Message(
                "symbol must not be empty".to_owned(),
            ));
        }

        let file_filter = self.normalize_usage_matrix_file(request.file.as_deref())?;
        let kind_filter = request
            .kind
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase());
        let store = self.state.store.as_ref();

        let mut candidates = store
            .search_symbols(symbol_query, 100)?
            .into_iter()
            .filter(|candidate| {
                candidate.qualified_name == symbol_query
                    || symbol_leaf_name(candidate.qualified_name.as_str()) == symbol_query
            })
            .filter(|candidate| {
                file_filter
                    .as_deref()
                    .map(|file| candidate.file_path == file)
                    .unwrap_or(true)
            })
            .filter(|candidate| {
                kind_filter
                    .as_deref()
                    .map(|kind| candidate.kind == *kind)
                    .unwrap_or(true)
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            left.qualified_name
                .cmp(&right.qualified_name)
                .then_with(|| left.file_path.cmp(&right.file_path))
                .then_with(|| left.kind.cmp(&right.kind))
                .then_with(|| left.symbol_id.cmp(&right.symbol_id))
        });

        let target = match candidates.as_slice() {
            [] => {
                return Err(AetherMcpError::Message(format!(
                    "symbol '{}' not found{}{}",
                    symbol_query,
                    file_filter
                        .as_deref()
                        .map(|file| format!(" in {file}"))
                        .unwrap_or_default(),
                    kind_filter
                        .as_deref()
                        .map(|kind| format!(" with kind '{kind}'"))
                        .unwrap_or_default()
                )));
            }
            [candidate] => candidate,
            many => {
                let candidates = many
                    .iter()
                    .map(format_candidate)
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(AetherMcpError::Message(format!(
                    "symbol '{}' is ambiguous: {}",
                    symbol_query, candidates
                )));
            }
        };

        let Some(target_record) = store.get_symbol_record(target.symbol_id.as_str())? else {
            return Err(AetherMcpError::Message(format!(
                "symbol '{}' could not be resolved after selection",
                target.symbol_id
            )));
        };

        let child_methods = child_method_symbols(store, &target_record)?;
        let method_count = child_methods.len() as u32;
        let mut method_edges = HashMap::<String, Vec<(String, String)>>::new();
        let mut caller_symbol_ids = HashSet::<String>::new();

        for method in &child_methods {
            let method_name = symbol_leaf_name(method.qualified_name.as_str()).to_owned();
            let edges = store
                .get_callers(method.qualified_name.as_str())?
                .into_iter()
                .map(|edge| {
                    caller_symbol_ids.insert(edge.source_id.clone());
                    (edge.source_id, edge.file_path)
                })
                .collect::<Vec<_>>();
            method_edges.insert(method_name, edges);
        }

        let caller_symbol_ids = caller_symbol_ids.into_iter().collect::<Vec<_>>();
        let caller_records = store.get_symbol_search_results_batch(&caller_symbol_ids)?;
        let mut method_to_consumers = HashMap::<String, HashSet<String>>::new();
        for method in &child_methods {
            let method_name = symbol_leaf_name(method.qualified_name.as_str()).to_owned();
            let consumers = method_to_consumers.entry(method_name.clone()).or_default();
            for (source_id, fallback_file) in
                method_edges.get(method_name.as_str()).into_iter().flatten()
            {
                let consumer_file = caller_records
                    .get(source_id)
                    .map(|record| record.file_path.clone())
                    .unwrap_or_else(|| fallback_file.clone());
                if !consumer_file.trim().is_empty() {
                    consumers.insert(consumer_file);
                }
            }
        }

        let mut consumer_to_methods = HashMap::<String, HashSet<String>>::new();
        for (method_name, consumer_files) in &method_to_consumers {
            for consumer_file in consumer_files {
                consumer_to_methods
                    .entry(consumer_file.clone())
                    .or_default()
                    .insert(method_name.clone());
            }
        }

        let mut matrix = consumer_to_methods
            .into_iter()
            .map(|(consumer_file, methods_used)| {
                let mut methods_used = methods_used.into_iter().collect::<Vec<_>>();
                methods_used.sort();
                ConsumerRow {
                    consumer_file,
                    method_count: methods_used.len() as u32,
                    methods_used,
                }
            })
            .collect::<Vec<_>>();
        matrix.sort_by(|left, right| {
            right
                .method_count
                .cmp(&left.method_count)
                .then_with(|| left.consumer_file.cmp(&right.consumer_file))
        });

        let mut method_consumers = method_to_consumers
            .iter()
            .map(|(method_name, consumer_files)| {
                let mut consumer_files = consumer_files.iter().cloned().collect::<Vec<_>>();
                consumer_files.sort();
                MethodConsumers {
                    method: method_name.clone(),
                    consumer_count: consumer_files.len() as u32,
                    consumer_files,
                }
            })
            .collect::<Vec<_>>();
        method_consumers.sort_by(|left, right| {
            right
                .consumer_count
                .cmp(&left.consumer_count)
                .then_with(|| left.method.cmp(&right.method))
        });

        let mut uncalled_methods = child_methods
            .iter()
            .map(|method| symbol_leaf_name(method.qualified_name.as_str()).to_owned())
            .filter(|method_name| {
                method_to_consumers
                    .get(method_name)
                    .map(|consumers| consumers.is_empty())
                    .unwrap_or(true)
            })
            .collect::<Vec<_>>();
        uncalled_methods.sort();

        let mut clusters_by_consumers = HashMap::<Vec<String>, Vec<String>>::new();
        for (method_name, consumer_files) in &method_to_consumers {
            if consumer_files.is_empty() {
                continue;
            }
            let mut consumer_key = consumer_files.iter().cloned().collect::<Vec<_>>();
            consumer_key.sort();
            clusters_by_consumers
                .entry(consumer_key)
                .or_default()
                .push(method_name.clone());
        }

        let mut suggested_clusters = clusters_by_consumers
            .into_iter()
            .filter_map(|(shared_consumers, mut methods)| {
                if methods.len() <= 1 {
                    return None;
                }
                methods.sort();
                Some(MethodCluster {
                    cluster_name: derive_cluster_name(methods.as_slice()),
                    reason: format!("Always co-consumed by: {}", shared_consumers.join(", ")),
                    methods,
                    shared_consumers,
                })
            })
            .collect::<Vec<_>>();
        suggested_clusters.sort_by(|left, right| {
            right
                .methods
                .len()
                .cmp(&left.methods.len())
                .then_with(|| left.cluster_name.cmp(&right.cluster_name))
        });

        Ok(AetherUsageMatrixResponse {
            schema_version: MEMORY_SCHEMA_VERSION.to_owned(),
            target: symbol_leaf_name(target_record.qualified_name.as_str()).to_owned(),
            target_file: target.file_path.clone(),
            method_count,
            consumer_count: matrix.len() as u32,
            matrix,
            method_consumers,
            uncalled_methods,
            suggested_clusters,
        })
    }

    fn normalize_usage_matrix_file(
        &self,
        raw_file: Option<&str>,
    ) -> Result<Option<String>, AetherMcpError> {
        let Some(raw_file) = raw_file.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        normalize_workspace_relative_path(self.workspace(), raw_file, "file").map(Some)
    }
}

fn format_candidate(candidate: &SymbolSearchResult) -> String {
    format!(
        "{} [{} @ {}]",
        candidate.qualified_name, candidate.kind, candidate.file_path
    )
}

fn derive_cluster_name(methods: &[String]) -> String {
    let Some(first_method) = methods.first() else {
        return "cluster".to_owned();
    };

    let first_tokens = first_method.split('_').collect::<Vec<_>>();
    let mut prefix_len = first_tokens.len();
    for method in methods.iter().skip(1) {
        let tokens = method.split('_').collect::<Vec<_>>();
        let shared_len = first_tokens
            .iter()
            .zip(tokens.iter())
            .take_while(|(left, right)| left == right)
            .count();
        prefix_len = prefix_len.min(shared_len);
    }

    if prefix_len > 0 {
        first_tokens[..prefix_len].join("_")
    } else {
        first_method.clone()
    }
}
