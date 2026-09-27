//! The enrichment context assembled for a symbol's SIR prompt, and the priority reason
//! reported with it.

use super::*;

#[allow(clippy::too_many_arguments)]
pub fn build_enrichment_context(
    store: &SqliteStore,
    symbol: &Symbol,
    baseline_sir: Option<SirAnnotation>,
    priority_scores: &HashMap<String, f64>,
    max_neighbors: usize,
    priority_threshold: f64,
    confidence_threshold: f64,
    priority_score: f64,
    contracts_enabled: bool,
) -> Result<SirEnrichmentContext> {
    let file_rollup_id = synthetic_file_sir_id(symbol.language.as_str(), symbol.file_path.as_str());
    let file_intent = store
        .read_sir_blob(file_rollup_id.as_str())?
        .and_then(|blob| match serde_json::from_str::<FileSir>(&blob) {
            Ok(sir) => Some(sir),
            Err(err) => {
                tracing::warn!(
                    symbol_id = %file_rollup_id,
                    error = %err,
                    "failed to parse file rollup SIR while building enrichment context"
                );
                None
            }
        })
        .map(|sir| sir.intent.trim().to_owned())
        .unwrap_or_default();

    let mut neighbors = Vec::<(f64, String, String)>::new();
    for peer in store.list_symbols_for_file(symbol.file_path.as_str())? {
        if peer.id == symbol.id {
            continue;
        }
        let Some(blob) = store.read_sir_blob(peer.id.as_str())? else {
            continue;
        };
        let peer_sir = match serde_json::from_str::<SirAnnotation>(&blob) {
            Ok(sir) => sir,
            Err(err) => {
                tracing::warn!(
                    symbol_id = %peer.id,
                    error = %err,
                    "failed to parse peer SIR while building enrichment context"
                );
                continue;
            }
        };
        neighbors.push((
            priority_scores
                .get(peer.id.as_str())
                .copied()
                .unwrap_or(0.0),
            peer.qualified_name,
            peer_sir.intent,
        ));
    }
    neighbors.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| left.1.cmp(&right.1))
    });
    if max_neighbors > 0 && neighbors.len() > max_neighbors {
        neighbors.truncate(max_neighbors);
    }

    let neighbor_intents = neighbors
        .into_iter()
        .map(|(_, name, intent)| (name, intent))
        .collect::<Vec<_>>();
    let baseline_confidence = baseline_sir
        .as_ref()
        .map(|sir| sir.confidence as f64)
        .unwrap_or(0.0);
    let priority_reason = format_priority_reason(
        store,
        symbol.id.as_str(),
        priority_score,
        baseline_confidence,
        priority_threshold,
        confidence_threshold,
    );

    let caller_contract_clauses = if contracts_enabled {
        lookup_caller_contracts(store, symbol.qualified_name.as_str())
    } else {
        Vec::new()
    };

    Ok(SirEnrichmentContext {
        file_intent: Some(file_intent),
        neighbor_intents,
        baseline_sir,
        priority_reason,
        caller_contract_clauses,
    })
}

/// Look up contract clauses from callers of the given symbol.
///
/// For each symbol that calls `qualified_name` and has active contracts,
/// collect (caller_qualified_name, clause_type, clause_text) triples.
pub(super) fn lookup_caller_contracts(
    store: &SqliteStore,
    qualified_name: &str,
) -> Vec<(String, String, String)> {
    let callers = match store.get_callers(qualified_name) {
        Ok(edges) => edges,
        Err(_) => return Vec::new(),
    };

    let mut result = Vec::new();
    for edge in callers {
        let contracts = match store.list_active_contracts_for_symbol(edge.source_id.as_str()) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if contracts.is_empty() {
            continue;
        }
        // Resolve caller qualified name from symbol record
        let caller_name = store
            .get_symbol_record(edge.source_id.as_str())
            .ok()
            .flatten()
            .map(|s| s.qualified_name)
            .unwrap_or_else(|| edge.source_id.clone());
        for contract in contracts {
            result.push((
                caller_name.clone(),
                contract.clause_type,
                contract.clause_text,
            ));
        }
    }
    result
}

pub(super) fn format_priority_reason(
    store: &SqliteStore,
    symbol_id: &str,
    priority_score: f64,
    confidence: f64,
    priority_threshold: f64,
    confidence_threshold: f64,
) -> String {
    let mut reasons = Vec::<String>::new();
    if priority_score >= priority_threshold {
        reasons.push(format!(
            "priority {:.2} at or above threshold {:.2}",
            priority_score, priority_threshold
        ));
    }
    if confidence <= confidence_threshold {
        reasons.push(format!(
            "baseline confidence {:.2} at or below threshold {:.2}",
            confidence, confidence_threshold
        ));
    }

    if let Ok(Some(metadata)) = store.get_symbol_metadata(symbol_id) {
        if metadata.is_public {
            reasons.push("public API symbol".to_owned());
        }
        let kind = metadata.kind.to_ascii_lowercase();
        if kind == "function" || kind == "method" {
            reasons.push("function/method".to_owned());
        }
    }

    if reasons.is_empty() {
        "selected for deeper analysis".to_owned()
    } else {
        reasons.join(" + ")
    }
}
