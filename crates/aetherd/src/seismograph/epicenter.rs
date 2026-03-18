use std::collections::HashMap;

use aether_store::SirFingerprintHistoryRecord;
use serde::{Deserialize, Serialize};

/// A single step in a cascade chain from epicenter to affected symbol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CascadeStep {
    pub symbol_id: String,
    pub delta_sem: f64,
    pub timestamp: i64,
    pub source_changed: bool,
    pub hop: usize,
}

/// Trace the epicenter of a symbol's semantic shift via time-respecting reverse BFS.
///
/// Algorithm:
/// 1. Start at `symbol_id` at its most recent fingerprint timestamp.
/// 2. If `source_changed == true` → this IS the epicenter. Stop.
/// 3. If `neighbor_changed == true` → look at DEPENDS_ON/CALLS neighbors.
/// 4. Among neighbors, find the one with highest Δ_sem that registered a
///    change at a timestamp ≤ current timestamp.
/// 5. Recurse with strict temporal monotonicity. Max depth from config.
///
/// `edge_lookup` returns the symbol IDs that `symbol_id` depends on (neighbors).
/// `history_by_symbol` maps symbol_id → records, sorted by timestamp DESC.
///
/// **Coupling simplification:** Uses 1.0 as coupling weight, ranking neighbors
/// by Δ_sem only. Coupling data lives in SurrealDB; accessing it would require
/// async graph store access. This can be upgraded later.
pub fn trace_epicenter<F>(
    symbol_id: &str,
    history_by_symbol: &HashMap<String, Vec<SirFingerprintHistoryRecord>>,
    edge_lookup: &F,
    noise_floor: f64,
    max_depth: usize,
) -> Vec<CascadeStep>
where
    F: Fn(&str) -> Vec<String>,
{
    let mut chain = Vec::new();
    let mut current_id = symbol_id.to_owned();
    let mut max_timestamp = i64::MAX; // First step: any timestamp allowed

    for hop in 0..max_depth {
        // Find the most recent record for this symbol at or before max_timestamp
        let record = match find_latest_record(history_by_symbol, &current_id, max_timestamp) {
            Some(r) => r,
            None => break, // No history for this symbol
        };

        let delta = record.delta_sem.unwrap_or(0.0);

        chain.push(CascadeStep {
            symbol_id: current_id.clone(),
            delta_sem: delta,
            timestamp: record.timestamp,
            source_changed: record.source_changed,
            hop,
        });

        // If this symbol had a source change, it's the epicenter — stop
        if record.source_changed {
            break;
        }

        // If neighbor_changed, trace upstream
        if !record.neighbor_changed {
            break; // Neither source nor neighbor changed — can't trace further
        }

        // Get dependencies (symbols this one depends on)
        let neighbors = edge_lookup(&current_id);
        if neighbors.is_empty() {
            break;
        }

        // Find the neighbor with highest Δ_sem at timestamp ≤ record.timestamp
        // Strict temporal monotonicity: next timestamp must be ≤ current
        let next_timestamp = record.timestamp;
        let mut best_neighbor: Option<(String, f64, i64)> = None;

        for neighbor_id in &neighbors {
            if let Some(nr) = find_latest_record(history_by_symbol, neighbor_id, next_timestamp) {
                let n_delta = nr.delta_sem.unwrap_or(0.0);
                if n_delta > noise_floor {
                    let is_better = match &best_neighbor {
                        None => true,
                        Some((_, best_delta, _)) => n_delta > *best_delta,
                    };
                    if is_better {
                        best_neighbor = Some((neighbor_id.clone(), n_delta, nr.timestamp));
                    }
                }
            }
        }

        match best_neighbor {
            Some((next_id, _, ts)) => {
                current_id = next_id;
                max_timestamp = ts;
            }
            None => break, // No upstream neighbor with significant Δ_sem
        }
    }

    // Reverse so chain goes from epicenter → target
    chain.reverse();
    chain
}

/// Find the most recent fingerprint record for a symbol at or before `max_timestamp`.
fn find_latest_record<'a>(
    history_by_symbol: &'a HashMap<String, Vec<SirFingerprintHistoryRecord>>,
    symbol_id: &str,
    max_timestamp: i64,
) -> Option<&'a SirFingerprintHistoryRecord> {
    let records = history_by_symbol.get(symbol_id)?;
    // Records are sorted by timestamp ASC, so iterate in reverse to find latest ≤ max_timestamp
    records.iter().rev().find(|r| r.timestamp <= max_timestamp)
}

/// Build a per-symbol history map from a flat list of records.
/// Records within each symbol are sorted by timestamp ASC.
pub fn build_history_map(
    records: &[SirFingerprintHistoryRecord],
) -> HashMap<String, Vec<SirFingerprintHistoryRecord>> {
    let mut map: HashMap<String, Vec<SirFingerprintHistoryRecord>> = HashMap::new();
    for record in records {
        map.entry(record.symbol_id.clone())
            .or_default()
            .push(record.clone());
    }
    // Ensure each symbol's records are sorted by timestamp ASC
    for records in map.values_mut() {
        records.sort_by_key(|r| r.timestamp);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(
        symbol_id: &str,
        timestamp: i64,
        delta_sem: f64,
        source_changed: bool,
        neighbor_changed: bool,
    ) -> SirFingerprintHistoryRecord {
        SirFingerprintHistoryRecord {
            symbol_id: symbol_id.to_owned(),
            timestamp,
            prompt_hash: "hash".to_owned(),
            prompt_hash_previous: None,
            trigger: "batch".to_owned(),
            source_changed,
            neighbor_changed,
            config_changed: false,
            generation_model: None,
            generation_pass: None,
            delta_sem: Some(delta_sem),
        }
    }

    fn make_edges() -> HashMap<String, Vec<String>> {
        let mut edges = HashMap::new();
        // sym_c depends on sym_b depends on sym_a
        edges.insert("sym_c".to_owned(), vec!["sym_b".to_owned()]);
        edges.insert("sym_b".to_owned(), vec!["sym_a".to_owned()]);
        edges.insert("sym_a".to_owned(), vec![]);
        edges
    }

    #[test]
    fn epicenter_stops_at_source_change() {
        let records = vec![
            make_record("sym_a", 100, 0.6, true, false), // epicenter
            make_record("sym_b", 101, 0.4, false, true), // propagated
            make_record("sym_c", 102, 0.2, false, true), // propagated
        ];
        let history = build_history_map(&records);
        let edge_map = make_edges();
        let lookup = |id: &str| edge_map.get(id).cloned().unwrap_or_default();

        let chain = trace_epicenter("sym_c", &history, &lookup, 0.15, 10);

        // Chain should be: sym_a (epicenter) → sym_b → sym_c
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].symbol_id, "sym_a");
        assert!(chain[0].source_changed);
        assert_eq!(chain[2].symbol_id, "sym_c");
    }

    #[test]
    fn epicenter_follows_temporal_monotonicity() {
        // sym_a has a change at timestamp 200 (future of sym_b's change at 100)
        let records = vec![
            make_record("sym_a", 200, 0.6, true, false), // future — should be skipped
            make_record("sym_b", 100, 0.4, false, true),
        ];
        let history = build_history_map(&records);
        let mut edge_map = HashMap::new();
        edge_map.insert("sym_b".to_owned(), vec!["sym_a".to_owned()]);

        let lookup = |id: &str| edge_map.get(id).cloned().unwrap_or_default();

        let chain = trace_epicenter("sym_b", &history, &lookup, 0.15, 10);

        // sym_a's only record is at t=200 which is > sym_b's t=100, so it won't be found
        assert_eq!(
            chain.len(),
            1,
            "should only contain sym_b since sym_a is in the future"
        );
        assert_eq!(chain[0].symbol_id, "sym_b");
    }

    #[test]
    fn epicenter_respects_max_depth() {
        let records = vec![
            make_record("sym_a", 100, 0.6, false, true),
            make_record("sym_b", 101, 0.4, false, true),
            make_record("sym_c", 102, 0.3, false, true),
            make_record("sym_d", 103, 0.2, false, true),
        ];
        let history = build_history_map(&records);
        let mut edge_map = HashMap::new();
        edge_map.insert("sym_d".to_owned(), vec!["sym_c".to_owned()]);
        edge_map.insert("sym_c".to_owned(), vec!["sym_b".to_owned()]);
        edge_map.insert("sym_b".to_owned(), vec!["sym_a".to_owned()]);

        let lookup = |id: &str| edge_map.get(id).cloned().unwrap_or_default();

        // max_depth = 2, so we can only take 2 hops from sym_d
        let chain = trace_epicenter("sym_d", &history, &lookup, 0.15, 2);

        assert!(
            chain.len() <= 2,
            "chain should be at most 2 steps, got {}",
            chain.len()
        );
    }
}
