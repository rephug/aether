pub fn compute_priority(s_total: f64, pagerank: f64, pr_max: f64, alpha: f64) -> f64 {
    let pr_normalized = if pr_max > 0.0 && (1.0 + pr_max).log10() > f64::EPSILON {
        (1.0 + pagerank).log10() / (1.0 + pr_max).log10()
    } else {
        0.0
    };
    s_total + alpha * pr_normalized
}

#[cfg(test)]
mod tests {
    use super::compute_priority;

    #[test]
    fn fully_stale_leaf_outranks_barely_stale_hub() {
        let stale_leaf = compute_priority(1.0, 0.0, 100.0, 0.2);
        let stale_hub = compute_priority(0.1, 100.0, 100.0, 0.2);
        assert!(stale_leaf > stale_hub);
    }

    #[test]
    fn pagerank_breaks_ties_between_equal_staleness() {
        let lower = compute_priority(0.5, 1.0, 100.0, 0.2);
        let higher = compute_priority(0.5, 10.0, 100.0, 0.2);
        assert!(higher > lower);
    }
}
