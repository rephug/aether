use std::collections::HashMap;

use aether_store::SirFingerprintHistoryRecord;

/// Community stability result for a single Louvain community.
#[derive(Debug, Clone)]
pub struct CommunityStabilityResult {
    pub community_id: String,
    pub stability: f64,
    pub symbol_count: usize,
    pub breach_count: usize,
}

/// Compute per-community stability scores over a rolling window.
///
/// ```text
/// Stability_C = 1.0 - Σ(PR_i × 𝟙(Δ_sem_i > τ)) / Σ(PR_i)
///     for all symbols i in community C
/// ```
///
/// `community_map` maps symbol_id → community_id.
/// `records` should be pre-filtered to the rolling window.
pub fn compute_community_stability(
    records: &[SirFingerprintHistoryRecord],
    community_map: &HashMap<String, String>,
    pagerank_map: &HashMap<String, f64>,
    noise_floor: f64,
) -> Vec<CommunityStabilityResult> {
    // Build per-community: total PR weight and breach PR weight.
    // A "breach" is any record where delta_sem > noise_floor for a symbol in the community.
    // We aggregate the max delta_sem per symbol in the window, then check if it breaches.
    struct CommunityAccum {
        symbol_prs: HashMap<String, f64>,
        breached_prs: HashMap<String, f64>,
    }

    let mut communities: HashMap<String, CommunityAccum> = HashMap::new();

    // Build map of max delta_sem per symbol from the window records
    let mut max_delta_by_symbol: HashMap<&str, f64> = HashMap::new();
    for record in records {
        let delta = record.delta_sem.unwrap_or(0.0);
        let entry = max_delta_by_symbol.entry(&record.symbol_id).or_insert(0.0);
        if delta > *entry {
            *entry = delta;
        }
    }

    // Assign symbols to communities and compute breach status
    for (symbol_id, community_id) in community_map {
        let pr = pagerank_map.get(symbol_id).copied().unwrap_or(0.0);

        let accum = communities
            .entry(community_id.clone())
            .or_insert_with(|| CommunityAccum {
                symbol_prs: HashMap::new(),
                breached_prs: HashMap::new(),
            });

        accum.symbol_prs.insert(symbol_id.clone(), pr);

        if let Some(&max_delta) = max_delta_by_symbol.get(symbol_id.as_str())
            && max_delta > noise_floor
        {
            accum.breached_prs.insert(symbol_id.clone(), pr);
        }
    }

    let mut results: Vec<CommunityStabilityResult> = communities
        .into_iter()
        .map(|(community_id, accum)| {
            let total_pr: f64 = accum.symbol_prs.values().sum();
            let breach_pr: f64 = accum.breached_prs.values().sum();
            let symbol_count = accum.symbol_prs.len();
            let breach_count = accum.breached_prs.len();

            let stability = if total_pr > 0.0 {
                (1.0 - breach_pr / total_pr).max(0.0)
            } else {
                1.0 // No PR data → stable
            };

            CommunityStabilityResult {
                community_id,
                stability,
                symbol_count,
                breach_count,
            }
        })
        .collect();

    // Sort by stability ascending (most unstable first)
    results.sort_by(|a, b| {
        a.stability
            .partial_cmp(&b.stability)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(symbol_id: &str, delta_sem: f64) -> SirFingerprintHistoryRecord {
        SirFingerprintHistoryRecord {
            symbol_id: symbol_id.to_owned(),
            timestamp: 1000,
            prompt_hash: "hash".to_owned(),
            prompt_hash_previous: None,
            trigger: "batch".to_owned(),
            source_changed: true,
            neighbor_changed: false,
            config_changed: false,
            generation_model: None,
            generation_pass: None,
            delta_sem: Some(delta_sem),
        }
    }

    #[test]
    fn stability_perfect_when_no_drift() {
        let records = vec![
            make_record("sym_a", 0.05), // below noise floor
            make_record("sym_b", 0.10), // below noise floor
        ];
        let mut community_map = HashMap::new();
        community_map.insert("sym_a".to_owned(), "c1".to_owned());
        community_map.insert("sym_b".to_owned(), "c1".to_owned());

        let mut pr = HashMap::new();
        pr.insert("sym_a".to_owned(), 1.0);
        pr.insert("sym_b".to_owned(), 1.0);

        let results = compute_community_stability(&records, &community_map, &pr, 0.15);
        assert_eq!(results.len(), 1);
        assert!((results[0].stability - 1.0).abs() < 1e-10);
        assert_eq!(results[0].breach_count, 0);
    }

    #[test]
    fn stability_drops_when_hub_shifts() {
        let records = vec![
            make_record("hub", 0.5),   // hub breaches
            make_record("leaf", 0.05), // leaf fine
        ];
        let mut community_map = HashMap::new();
        community_map.insert("hub".to_owned(), "c1".to_owned());
        community_map.insert("leaf".to_owned(), "c1".to_owned());

        let mut pr = HashMap::new();
        pr.insert("hub".to_owned(), 10.0); // high PR
        pr.insert("leaf".to_owned(), 0.1); // low PR

        let results = compute_community_stability(&records, &community_map, &pr, 0.15);
        assert_eq!(results.len(), 1);
        // breach_pr = 10.0 (hub), total_pr = 10.1
        // stability = 1.0 - 10.0/10.1 ≈ 0.0099
        assert!(results[0].stability < 0.02, "stability should be very low");
        assert_eq!(results[0].breach_count, 1);
    }

    #[test]
    fn stability_empty_community_is_stable() {
        // Community has symbols but no fingerprint data in the window
        let records: Vec<SirFingerprintHistoryRecord> = vec![];
        let mut community_map = HashMap::new();
        community_map.insert("sym_a".to_owned(), "c1".to_owned());

        let mut pr = HashMap::new();
        pr.insert("sym_a".to_owned(), 1.0);

        let results = compute_community_stability(&records, &community_map, &pr, 0.15);
        assert_eq!(results.len(), 1);
        assert!((results[0].stability - 1.0).abs() < 1e-10);
    }
}
