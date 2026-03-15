pub fn time_staleness(days_since: f64, half_life: f64, k: f64) -> f64 {
    1.0 / (1.0 + (-k * (days_since - half_life)).exp())
}

pub fn effective_age(days_since: f64, git_churn_30d: f64) -> f64 {
    days_since * (1.0 + (1.0 + git_churn_30d).log2())
}

pub fn noisy_or(s_time: f64, s_neighbor: f64) -> f64 {
    1.0 - (1.0 - s_time) * (1.0 - s_neighbor)
}

pub fn compute_staleness(
    source_changed: bool,
    model_deprecated: bool,
    s_time: f64,
    s_neighbor: f64,
) -> f64 {
    let s_source: f64 = if source_changed { 1.0 } else { 0.0 };
    let s_model: f64 = if model_deprecated { 1.0 } else { 0.0 };
    let soft = noisy_or(s_time, s_neighbor);
    s_source.max(s_model).max(soft)
}

#[cfg(test)]
mod tests {
    use super::{compute_staleness, effective_age, noisy_or, time_staleness};

    #[test]
    fn sigmoid_has_expected_shape() {
        let early = time_staleness(0.0, 15.0, 0.3);
        let midpoint = time_staleness(15.0, 15.0, 0.3);
        let late = time_staleness(60.0, 15.0, 0.3);

        assert!(early < 0.1, "{early}");
        assert!((midpoint - 0.5).abs() < 1e-9, "{midpoint}");
        assert!(late > 0.99, "{late}");
    }

    #[test]
    fn hard_gates_override_soft_signals() {
        assert_eq!(compute_staleness(true, false, 0.0, 0.0), 1.0);
        assert_eq!(compute_staleness(false, true, 0.0, 0.0), 1.0);
    }

    #[test]
    fn noisy_or_stays_bounded() {
        let combined = noisy_or(0.4, 0.6);
        assert!(combined >= 0.0);
        assert!(combined <= 1.0);
        assert!((combined - 0.76).abs() < 1e-9, "{combined}");
    }

    #[test]
    fn effective_age_increases_with_churn() {
        let base = effective_age(10.0, 0.0);
        let volatile = effective_age(10.0, 8.0);
        assert!(volatile > base);
    }
}
