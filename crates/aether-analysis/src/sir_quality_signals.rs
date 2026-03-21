use std::collections::HashSet;

const BEHAVIORAL_MARKERS: &[&str] = &[
    "by",
    "then",
    "when",
    "if",
    "after",
    "before",
    "while",
    "returns",
    "produces",
    "emits",
    "triggers",
    "delegates",
    "falls back",
    "retries",
];
const ERROR_MARKERS: &[&str] = &[
    "error", "fail", "panic", "fallback", "default", "missing", "invalid", "timeout", "retry",
    "graceful", "degraded", "skip", "abort",
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SirQualitySignals {
    pub specificity: f64,
    pub behavioral_depth: f64,
    pub error_coverage: f64,
    pub length_score: f64,
    pub composite_quality: f64,
}

#[must_use]
pub fn compute_sir_quality_signals(intent: &str) -> SirQualitySignals {
    let intent = intent.trim();
    if intent.is_empty() {
        return SirQualitySignals {
            specificity: 0.0,
            behavioral_depth: 0.0,
            error_coverage: 0.0,
            length_score: 0.0,
            composite_quality: 0.0,
        };
    }

    let lowered = intent.to_ascii_lowercase();
    let specificity = score_from_count(extract_code_identifiers(intent).len(), 5.0);
    let behavioral_depth =
        score_from_count(count_present_markers(&lowered, BEHAVIORAL_MARKERS), 4.0);
    let error_coverage = score_from_count(count_present_markers(&lowered, ERROR_MARKERS), 2.0);
    let length_score = ((intent.chars().count() as f64) / 300.0).min(1.0);
    let composite_quality = (0.30 * specificity
        + 0.30 * behavioral_depth
        + 0.20 * error_coverage
        + 0.20 * length_score)
        .clamp(0.0, 1.0);

    SirQualitySignals {
        specificity,
        behavioral_depth,
        error_coverage,
        length_score,
        composite_quality,
    }
}

#[must_use]
pub fn compute_confidence_percentiles(confidences: &[f32]) -> Vec<f64> {
    if confidences.is_empty() {
        return Vec::new();
    }

    if confidences.len() == 1 {
        return vec![0.5];
    }

    let first = confidences[0];
    if confidences
        .iter()
        .all(|value| value.total_cmp(&first).is_eq())
    {
        return vec![0.5; confidences.len()];
    }

    let mut indexed = confidences.iter().copied().enumerate().collect::<Vec<_>>();
    indexed.sort_by(|left, right| {
        left.1
            .total_cmp(&right.1)
            .then_with(|| left.0.cmp(&right.0))
    });

    let denominator = (confidences.len() - 1) as f64;
    let mut percentiles = vec![0.0; confidences.len()];
    let mut start = 0usize;
    while start < indexed.len() {
        let value = indexed[start].1;
        let mut end = start + 1;
        while end < indexed.len() && indexed[end].1.total_cmp(&value).is_eq() {
            end += 1;
        }

        let mid_rank = ((start + 1) as f64 + end as f64) / 2.0;
        let percentile = ((mid_rank - 1.0) / denominator).clamp(0.0, 1.0);
        for (original_index, _) in &indexed[start..end] {
            percentiles[*original_index] = percentile;
        }
        start = end;
    }

    percentiles
}

#[must_use]
pub fn blend_normalized_quality(composite_quality: f64, confidence_percentile: f64) -> f64 {
    (0.6 * composite_quality + 0.4 * confidence_percentile).clamp(0.0, 1.0)
}

fn score_from_count(count: usize, divisor: f64) -> f64 {
    ((count as f64) / divisor).min(1.0)
}

fn extract_code_identifiers(intent: &str) -> HashSet<String> {
    let mut identifiers = HashSet::new();
    identifiers.extend(extract_backtick_terms(intent));

    for token in intent.split_whitespace() {
        let normalized = trim_token(token);
        if normalized.is_empty() {
            continue;
        }
        if is_snake_case_identifier(normalized) || is_camel_case_identifier(normalized) {
            identifiers.insert(normalized.to_owned());
        }
    }

    identifiers
}

fn extract_backtick_terms(intent: &str) -> HashSet<String> {
    let mut terms = HashSet::new();
    let mut current = String::new();
    let mut in_backticks = false;

    for ch in intent.chars() {
        if ch == '`' {
            if in_backticks {
                let term = current.trim();
                if !term.is_empty() {
                    terms.insert(term.to_owned());
                }
                current.clear();
                in_backticks = false;
            } else {
                current.clear();
                in_backticks = true;
            }
            continue;
        }

        if in_backticks {
            current.push(ch);
        }
    }

    terms
}

fn trim_token(token: &str) -> &str {
    token.trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
}

fn is_snake_case_identifier(token: &str) -> bool {
    token.contains('_')
        && token
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
        && token.chars().any(|ch| ch.is_ascii_alphabetic())
}

fn is_camel_case_identifier(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    first.is_ascii_uppercase()
        && token.chars().all(|ch| ch.is_ascii_alphanumeric())
        && token.chars().any(|ch| ch.is_ascii_lowercase())
}

fn count_present_markers(intent_lower: &str, markers: &[&str]) -> usize {
    markers
        .iter()
        .filter(|marker| contains_marker(intent_lower, marker))
        .count()
}

fn contains_marker(intent_lower: &str, marker: &str) -> bool {
    intent_lower.match_indices(marker).any(|(start, _)| {
        let end = start + marker.len();
        has_marker_boundary(intent_lower, start, end)
    })
}

fn has_marker_boundary(text: &str, start: usize, end: usize) -> bool {
    let left_boundary = text[..start]
        .chars()
        .next_back()
        .is_none_or(|ch| !ch.is_ascii_alphanumeric());
    let right_boundary = text[end..]
        .chars()
        .next()
        .is_none_or(|ch| !ch.is_ascii_alphanumeric());
    left_boundary && right_boundary
}

#[cfg(test)]
mod tests {
    use super::{
        blend_normalized_quality, compute_confidence_percentiles, compute_sir_quality_signals,
    };

    #[test]
    fn specificity_counts_distinct_code_like_identifiers() {
        let signals = compute_sir_quality_signals(
            "Uses `build_index` to populate cache_key entries and returns StoreError",
        );

        assert!((signals.specificity - 0.8).abs() < 1e-9);
    }

    #[test]
    fn behavioral_depth_counts_distinct_behavioral_markers() {
        let signals = compute_sir_quality_signals(
            "When input arrives, delegates parsing, then returns a value and falls back after timeout.",
        );

        assert!((signals.behavioral_depth - 1.0).abs() < 1e-9);
    }

    #[test]
    fn error_coverage_counts_distinct_error_markers() {
        let signals = compute_sir_quality_signals(
            "Returns an error with fallback behavior when configuration is missing or invalid.",
        );

        assert!((signals.error_coverage - 1.0).abs() < 1e-9);
    }

    #[test]
    fn length_score_caps_at_one() {
        let signals = compute_sir_quality_signals(&"a".repeat(450));

        assert!((signals.length_score - 1.0).abs() < 1e-9);
    }

    #[test]
    fn composite_quality_uses_configured_weights() {
        let intent = "Uses `cache_key` when parsing request_config and returns default output after timeout.";
        let signals = compute_sir_quality_signals(intent);
        let expected = 0.30 * signals.specificity
            + 0.30 * signals.behavioral_depth
            + 0.20 * signals.error_coverage
            + 0.20 * signals.length_score;

        assert!((signals.composite_quality - expected).abs() < 1e-9);
    }

    #[test]
    fn confidence_percentiles_scale_from_low_to_high() {
        let percentiles = compute_confidence_percentiles(&[0.1, 0.5, 0.9]);

        assert_eq!(percentiles, vec![0.0, 0.5, 1.0]);
    }

    #[test]
    fn confidence_percentiles_average_tied_ranks() {
        let percentiles = compute_confidence_percentiles(&[0.1, 0.5, 0.5, 0.9]);

        assert_eq!(percentiles, vec![0.0, 0.5, 0.5, 1.0]);
    }

    #[test]
    fn confidence_percentiles_use_neutral_value_for_singletons() {
        let percentiles = compute_confidence_percentiles(&[0.42]);

        assert_eq!(percentiles, vec![0.5]);
    }

    #[test]
    fn confidence_percentiles_use_neutral_value_for_flat_groups() {
        let percentiles = compute_confidence_percentiles(&[0.42, 0.42, 0.42]);

        assert_eq!(percentiles, vec![0.5, 0.5, 0.5]);
    }

    #[test]
    fn normalized_quality_blends_composite_and_percentile() {
        let blended = blend_normalized_quality(0.8, 0.25);

        assert!((blended - 0.58).abs() < 1e-9);
    }
}
