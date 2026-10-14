use std::collections::HashMap;

use aether_store::SirFingerprintHistoryRecord;

use crate::batch::hash::{compute_neighbor_hash_segment, decompose_prompt_hash};

/// Result of semantic velocity computation for a single batch.
#[derive(Debug, Clone)]
pub struct VelocityResult {
    pub codebase_shift: f64,
    pub semantic_velocity: f64,
    pub symbols_regenerated: usize,
    pub symbols_above_noise: usize,
}

/// Compute semantic velocity from a batch of fingerprint records.
///
/// **Per-batch codebase shift:**
/// ```text
/// S_t = Σ(PR_i × max(0, Δ_sem_i - τ)) / Σ(PR_i)
///     for all symbols i regenerated in batch t
///     where τ = noise_floor
/// ```
///
/// **Semantic velocity (EMA):**
/// ```text
/// V_t = α × S_t + (1 - α) × V_{t-1}
/// ```
///
/// Model-upgrade spikes are filtered out: rows where the generation config changed but
/// neither the source nor the neighbor intents did. A real neighbor change counts even
/// when the config changed too (see [`neighbors_really_changed`]).
pub fn compute_semantic_velocity(
    records: &[SirFingerprintHistoryRecord],
    pagerank_map: &HashMap<String, f64>,
    noise_floor: f64,
    ema_alpha: f64,
    prev_velocity: Option<f64>,
) -> VelocityResult {
    // Filter out model-upgrade spikes: only the config changed.
    let filtered: Vec<&SirFingerprintHistoryRecord> = records
        .iter()
        .filter(|r| !r.config_changed || r.source_changed || neighbors_really_changed(r))
        .collect();

    let symbols_regenerated = filtered.len();

    if symbols_regenerated == 0 {
        let velocity = match prev_velocity {
            Some(v) => ema_alpha * 0.0 + (1.0 - ema_alpha) * v,
            None => 0.0,
        };
        return VelocityResult {
            codebase_shift: 0.0,
            semantic_velocity: velocity,
            symbols_regenerated: 0,
            symbols_above_noise: 0,
        };
    }

    let mut weighted_sum = 0.0_f64;
    let mut pr_sum = 0.0_f64;
    let mut above_noise = 0_usize;

    for record in &filtered {
        let pr = pagerank_map.get(&record.symbol_id).copied().unwrap_or(0.0);
        let delta = record.delta_sem.unwrap_or(0.0);
        let contribution = (delta - noise_floor).max(0.0);

        if contribution > 0.0 {
            above_noise += 1;
        }

        weighted_sum += pr * contribution;
        pr_sum += pr;
    }

    let codebase_shift = if pr_sum > 0.0 {
        weighted_sum / pr_sum
    } else {
        0.0
    };

    let semantic_velocity = match prev_velocity {
        Some(v) => ema_alpha * codebase_shift + (1.0 - ema_alpha) * v,
        None => codebase_shift, // V_0 = S_0
    };

    VelocityResult {
        codebase_shift,
        semantic_velocity,
        symbols_regenerated,
        symbols_above_noise: above_noise,
    }
}

/// Whether the neighbor intents behind a fingerprint row changed. Scan, watcher and
/// inject prompts carry no neighbor intents while triage and deep prompts do, so moving
/// between the two flips the neighbor segment without any neighbor changing; only a
/// change between two prompts that both carried neighbors counts.
fn neighbors_really_changed(record: &SirFingerprintHistoryRecord) -> bool {
    if !record.neighbor_changed {
        return false;
    }
    let empty = compute_neighbor_hash_segment(&[]);
    let segment = |hash: &str| decompose_prompt_hash(hash).1.map(str::to_owned);
    let previous = record.prompt_hash_previous.as_deref().and_then(segment);
    let current = segment(&record.prompt_hash);
    previous.is_some_and(|previous| previous != empty)
        && current.is_some_and(|current| current != empty)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(
        symbol_id: &str,
        delta_sem: f64,
        source_changed: bool,
        config_changed: bool,
    ) -> SirFingerprintHistoryRecord {
        make_record_with_neighbors(symbol_id, delta_sem, source_changed, false, config_changed)
    }

    fn make_record_with_neighbors(
        symbol_id: &str,
        delta_sem: f64,
        source_changed: bool,
        neighbor_changed: bool,
        config_changed: bool,
    ) -> SirFingerprintHistoryRecord {
        SirFingerprintHistoryRecord {
            symbol_id: symbol_id.to_owned(),
            timestamp: 1000,
            prompt_hash: "hash".to_owned(),
            prompt_hash_previous: None,
            trigger: "batch".to_owned(),
            source_changed,
            neighbor_changed,
            config_changed,
            generation_model: None,
            generation_pass: None,
            delta_sem: Some(delta_sem),
            sir_write_generation: None,
        }
    }

    #[test]
    fn velocity_zero_when_no_symbols() {
        let result = compute_semantic_velocity(&[], &HashMap::new(), 0.15, 0.2, None);
        assert_eq!(result.codebase_shift, 0.0);
        assert_eq!(result.semantic_velocity, 0.0);
        assert_eq!(result.symbols_regenerated, 0);
        assert_eq!(result.symbols_above_noise, 0);
    }

    #[test]
    fn velocity_filters_model_upgrades() {
        let records = vec![
            make_record("sym_a", 0.5, true, false), // real change
            make_record("sym_b", 0.8, false, true), // model upgrade — filtered
            make_record("sym_c", 0.3, true, true),  // config + source changed — kept
        ];
        let mut pr = HashMap::new();
        pr.insert("sym_a".to_owned(), 1.0);
        pr.insert("sym_b".to_owned(), 1.0);
        pr.insert("sym_c".to_owned(), 1.0);

        let result = compute_semantic_velocity(&records, &pr, 0.15, 0.2, None);
        // sym_b is filtered, so 2 symbols regenerated
        assert_eq!(result.symbols_regenerated, 2);
    }

    /// A fingerprint row between two prompts, with its change flags derived from them.
    fn transition(
        symbol_id: &str,
        before: (&[&str], &str),
        after: (&[&str], &str),
    ) -> SirFingerprintHistoryRecord {
        use crate::batch::hash::{compute_prompt_hash, diff_prompt_hashes};
        let previous = compute_prompt_hash("fn same() {}", before.0, before.1);
        let current = compute_prompt_hash("fn same() {}", after.0, after.1);
        let (source_changed, neighbor_changed, config_changed) =
            diff_prompt_hashes(&previous, &current);
        SirFingerprintHistoryRecord {
            prompt_hash: current,
            prompt_hash_previous: Some(previous),
            ..make_record_with_neighbors(
                symbol_id,
                0.6,
                source_changed,
                neighbor_changed,
                config_changed,
            )
        }
    }

    #[test]
    fn velocity_keeps_neighbor_changes_that_coincide_with_a_config_change() {
        let records = vec![
            // Neighbor intents changed between two deep prompts: kept.
            transition("sym_a", (&["old"], "deep:v1"), (&["new"], "deep:v2")),
            // Scan (no neighbors) to deep: the segment flips, no neighbor changed.
            transition("sym_b", (&[], "scan:v1"), (&["n"], "deep:v1")),
            // Config only: filtered.
            transition("sym_c", (&["n"], "deep:v1"), (&["n"], "deep:v2")),
        ];
        let pr = records
            .iter()
            .map(|record| (record.symbol_id.clone(), 1.0))
            .collect::<HashMap<_, _>>();

        let result = compute_semantic_velocity(&records, &pr, 0.15, 0.2, None);
        assert_eq!(result.symbols_regenerated, 1);
        assert_eq!(result.symbols_above_noise, 1);
    }

    #[test]
    fn velocity_noise_floor_filters_small_deltas() {
        let records = vec![
            make_record("sym_a", 0.10, true, false), // below noise floor of 0.15
            make_record("sym_b", 0.50, true, false), // above noise floor
        ];
        let mut pr = HashMap::new();
        pr.insert("sym_a".to_owned(), 1.0);
        pr.insert("sym_b".to_owned(), 1.0);

        let result = compute_semantic_velocity(&records, &pr, 0.15, 0.2, None);
        assert_eq!(result.symbols_above_noise, 1);
        // Only sym_b contributes: (0.50 - 0.15) * 1.0 / (1.0 + 1.0) = 0.175
        assert!((result.codebase_shift - 0.175).abs() < 1e-10);
    }

    #[test]
    fn velocity_ema_chains_correctly() {
        let records = vec![make_record("sym_a", 0.5, true, false)];
        let mut pr = HashMap::new();
        pr.insert("sym_a".to_owned(), 1.0);

        let prev_velocity = 0.8;
        let result = compute_semantic_velocity(&records, &pr, 0.15, 0.2, Some(prev_velocity));

        // S_t = (0.5 - 0.15) * 1.0 / 1.0 = 0.35
        // V_t = 0.2 * 0.35 + 0.8 * 0.8 = 0.07 + 0.64 = 0.71
        let expected = 0.2 * 0.35 + 0.8 * 0.8;
        assert!(
            (result.semantic_velocity - expected).abs() < 1e-10,
            "got {} expected {}",
            result.semantic_velocity,
            expected
        );
    }

    #[test]
    fn velocity_pagerank_weights_correctly() {
        let records = vec![
            make_record("hub", 0.3, true, false),
            make_record("leaf", 0.3, true, false),
        ];
        let mut pr = HashMap::new();
        pr.insert("hub".to_owned(), 10.0); // high PR
        pr.insert("leaf".to_owned(), 0.1); // low PR

        let result = compute_semantic_velocity(&records, &pr, 0.15, 0.2, None);
        // Both have same delta_sem, but PR-weighted: hub dominates
        // contribution = (0.3 - 0.15) = 0.15 for both
        // S_t = (10.0 * 0.15 + 0.1 * 0.15) / (10.0 + 0.1) = 1.515 / 10.1 ≈ 0.15
        // Almost equal to the contribution because hub dominates weight
        assert!(result.codebase_shift > 0.0);
        assert!((result.codebase_shift - 0.15).abs() < 0.001);
    }
}
