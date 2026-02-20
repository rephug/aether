const RRF_K: f32 = 60.0;
const RECENCY_WINDOW_MS: f32 = 30.0 * 24.0 * 60.0 * 60.0 * 1000.0;
const ACCESS_NORMALIZATION_BASE: f32 = 100.0;

pub(crate) fn rrf_score(rank: usize) -> f32 {
    1.0 / (RRF_K + rank as f32 + 1.0)
}

pub(crate) fn apply_recency_access_boost(
    base: f32,
    access_count: i64,
    last_accessed_at: Option<i64>,
    now_ms: i64,
) -> f32 {
    let base = base.max(0.0);
    let recency_factor = last_accessed_at
        .map(|value| {
            let age_ms = (now_ms.saturating_sub(value)).max(0) as f32;
            (1.0 - (age_ms / RECENCY_WINDOW_MS)).clamp(0.0, 1.0)
        })
        .unwrap_or(0.0);
    let access_factor = (access_count.max(0) as f32 + 1.0).ln() / ACCESS_NORMALIZATION_BASE.ln();

    base * (1.0 + (0.1 * recency_factor) + (0.05 * access_factor))
}
