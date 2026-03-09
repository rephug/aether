use std::collections::HashMap;

use aether_config::HealthScoreConfig;

use crate::models::SemanticSignals;

const MAX_CENTRALITY_WEIGHT: f64 = 0.25;
const DRIFT_DENSITY_WEIGHT: f64 = 0.20;
const STALE_SIR_WEIGHT: f64 = 0.15;
const TEST_GAP_WEIGHT: f64 = 0.20;
const BOUNDARY_LEAKAGE_WEIGHT: f64 = 0.20;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SemanticFileInput {
    pub max_pagerank: f64,
    pub symbol_count: usize,
    pub drifted_symbol_count: usize,
    pub stale_or_missing_sir_count: usize,
    pub community_count: usize,
    pub has_test_coverage: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SemanticInput {
    pub workspace_max_pagerank: f64,
    pub files: HashMap<String, SemanticFileInput>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SemanticFileAnalysis {
    pub path: String,
    pub max_centrality: f64,
    pub symbol_count: usize,
    pub drift_ratio: f64,
    pub community_count: usize,
    pub has_test_coverage: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SemanticSignalAnalysis {
    pub files: Vec<SemanticFileAnalysis>,
    pub drifted_symbols: usize,
    pub total_symbols: usize,
    pub signals: SemanticSignals,
}

pub fn compute_semantic_signals(
    input: &SemanticInput,
    config: &HealthScoreConfig,
) -> SemanticSignals {
    analyze_semantic_signals(input, config).signals
}

pub(crate) fn analyze_semantic_signals(
    input: &SemanticInput,
    config: &HealthScoreConfig,
) -> SemanticSignalAnalysis {
    let mut files = input
        .files
        .iter()
        .map(|(path, entry)| {
            let drift_ratio = if entry.symbol_count == 0 {
                0.0
            } else {
                entry.drifted_symbol_count as f64 / entry.symbol_count as f64
            };
            let max_centrality = if input.workspace_max_pagerank <= f64::EPSILON {
                0.0
            } else {
                (entry.max_pagerank / input.workspace_max_pagerank).clamp(0.0, 1.0)
            };

            SemanticFileAnalysis {
                path: path.clone(),
                max_centrality,
                symbol_count: entry.symbol_count,
                drift_ratio,
                community_count: entry.community_count,
                has_test_coverage: entry.has_test_coverage,
            }
        })
        .collect::<Vec<_>>();

    files.sort_by(|left, right| {
        right
            .max_centrality
            .partial_cmp(&left.max_centrality)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.path.cmp(&right.path))
    });

    let total_symbols = input
        .files
        .values()
        .map(|entry| entry.symbol_count)
        .sum::<usize>();
    let drifted_symbols = input
        .files
        .values()
        .map(|entry| entry.drifted_symbol_count)
        .sum::<usize>();
    let stale_or_missing_sir = input
        .files
        .values()
        .map(|entry| entry.stale_or_missing_sir_count)
        .sum::<usize>();
    let indexed_file_count = files.iter().filter(|entry| entry.symbol_count > 0).count();
    let max_centrality = files
        .iter()
        .map(|entry| entry.max_centrality)
        .fold(0.0, f64::max);
    let drift_density_ratio = ratio(drifted_symbols, total_symbols);
    let stale_sir_ratio = ratio(stale_or_missing_sir, total_symbols);

    let top_file_count = ((indexed_file_count as f64) * 0.2).ceil().max(1.0) as usize;
    let uncovered_top_files = files
        .iter()
        .filter(|entry| entry.symbol_count > 0)
        .take(top_file_count)
        .filter(|entry| !entry.has_test_coverage)
        .count();
    let test_gap_ratio = if indexed_file_count == 0 {
        0.0
    } else {
        uncovered_top_files as f64 / top_file_count.min(indexed_file_count) as f64
    };
    let multi_community_files = files
        .iter()
        .filter(|entry| entry.symbol_count > 0 && entry.community_count > 1)
        .count();
    let boundary_leakage_ratio = if indexed_file_count == 0 {
        0.0
    } else {
        multi_community_files as f64 / indexed_file_count as f64
    };

    let signals = SemanticSignals {
        max_centrality,
        drift_density: normalize_ratio(drift_density_ratio, config.drift_density_high),
        stale_sir_ratio: normalize_ratio(stale_sir_ratio, config.stale_sir_high),
        test_gap: normalize_ratio(test_gap_ratio, config.test_gap_high),
        boundary_leakage: normalize_ratio(boundary_leakage_ratio, config.boundary_leakage_high),
        semantic_pressure: 0.0,
    };
    let semantic_pressure = (signals.max_centrality * MAX_CENTRALITY_WEIGHT
        + signals.drift_density * DRIFT_DENSITY_WEIGHT
        + signals.stale_sir_ratio * STALE_SIR_WEIGHT
        + signals.test_gap * TEST_GAP_WEIGHT
        + signals.boundary_leakage * BOUNDARY_LEAKAGE_WEIGHT)
        .clamp(0.0, 1.0);

    SemanticSignalAnalysis {
        files,
        drifted_symbols,
        total_symbols,
        signals: SemanticSignals {
            semantic_pressure,
            ..signals
        },
    }
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn normalize_ratio(value: f64, high: f32) -> f64 {
    if high <= 0.0 {
        return 0.0;
    }

    (value / high as f64).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use aether_config::HealthScoreConfig;

    use super::{SemanticFileInput, SemanticInput, compute_semantic_signals};

    #[test]
    fn compute_semantic_signals_uses_ratios() {
        let input = SemanticInput {
            workspace_max_pagerank: 10.0,
            files: HashMap::from([
                (
                    "src/a.rs".to_owned(),
                    SemanticFileInput {
                        max_pagerank: 10.0,
                        symbol_count: 4,
                        drifted_symbol_count: 2,
                        stale_or_missing_sir_count: 1,
                        community_count: 3,
                        has_test_coverage: false,
                    },
                ),
                (
                    "src/b.rs".to_owned(),
                    SemanticFileInput {
                        max_pagerank: 2.0,
                        symbol_count: 2,
                        drifted_symbol_count: 0,
                        stale_or_missing_sir_count: 0,
                        community_count: 1,
                        has_test_coverage: true,
                    },
                ),
            ]),
        };

        let signals = compute_semantic_signals(&input, &HealthScoreConfig::default());
        assert_eq!(signals.max_centrality, 1.0);
        assert_eq!(signals.drift_density, 1.0);
        assert!((signals.stale_sir_ratio - (1.0 / 6.0 / 0.4)).abs() < 1e-6);
        assert_eq!(signals.test_gap, 1.0);
        assert_eq!(signals.boundary_leakage, 1.0);
    }
}
