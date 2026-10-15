use serde::{Deserialize, Serialize};

/// Lower bound for `implicit_min_versions` after normalization.
pub const MIN_IMPLICIT_HISTORY_VERSIONS: u32 = 3;
/// Upper bound for `implicit_min_versions` and for the history window that
/// implicit inference reads.
pub const MAX_IMPLICIT_HISTORY_VERSIONS: u32 = 50;

const MIN_IMPLICIT_PERSISTENCE_THRESHOLD: f64 = 0.5;
const MAX_IMPLICIT_PERSISTENCE_THRESHOLD: f64 = 1.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContractsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_embedding_pass_threshold")]
    pub embedding_pass_threshold: f64,
    #[serde(default = "default_embedding_fail_threshold")]
    pub embedding_fail_threshold: f64,
    #[serde(default)]
    pub judge_model: String,
    #[serde(default)]
    pub judge_provider: String,
    #[serde(default = "default_streak_threshold")]
    pub streak_threshold: u32,
    /// Opt-in switch for `aetherd contract infer`.
    #[serde(default)]
    pub implicit_inference_enabled: bool,
    /// Minimum number of usable SIR history versions before inference runs.
    #[serde(default = "default_implicit_min_versions")]
    pub implicit_min_versions: u32,
    /// Fraction of considered versions that must state a claim before it is
    /// treated as an invariant.
    #[serde(default = "default_implicit_persistence_threshold")]
    pub implicit_persistence_threshold: f64,
}

impl Default for ContractsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            embedding_pass_threshold: default_embedding_pass_threshold(),
            embedding_fail_threshold: default_embedding_fail_threshold(),
            judge_model: String::new(),
            judge_provider: String::new(),
            streak_threshold: default_streak_threshold(),
            implicit_inference_enabled: false,
            implicit_min_versions: default_implicit_min_versions(),
            implicit_persistence_threshold: default_implicit_persistence_threshold(),
        }
    }
}

impl ContractsConfig {
    /// `implicit_min_versions` with 0 mapped to the default and other values
    /// clamped to `[MIN_IMPLICIT_HISTORY_VERSIONS, MAX_IMPLICIT_HISTORY_VERSIONS]`.
    pub fn effective_implicit_min_versions(&self) -> u32 {
        if self.implicit_min_versions == 0 {
            return default_implicit_min_versions();
        }
        self.implicit_min_versions
            .clamp(MIN_IMPLICIT_HISTORY_VERSIONS, MAX_IMPLICIT_HISTORY_VERSIONS)
    }

    /// `implicit_persistence_threshold` clamped to `[0.5, 1.0]`, with non-finite
    /// values mapped to the default. Below 0.5 two contradictory claims could
    /// both qualify as invariants.
    pub fn effective_implicit_persistence_threshold(&self) -> f64 {
        if !self.implicit_persistence_threshold.is_finite() {
            return default_implicit_persistence_threshold();
        }
        self.implicit_persistence_threshold.clamp(
            MIN_IMPLICIT_PERSISTENCE_THRESHOLD,
            MAX_IMPLICIT_PERSISTENCE_THRESHOLD,
        )
    }

    /// Number of most recent SIR history versions implicit inference reads.
    /// Always at least `effective_implicit_min_versions()`.
    pub fn implicit_history_window(&self) -> u32 {
        (2 * self.effective_implicit_min_versions()).min(MAX_IMPLICIT_HISTORY_VERSIONS)
    }
}

fn default_embedding_pass_threshold() -> f64 {
    0.88
}

fn default_embedding_fail_threshold() -> f64 {
    0.50
}

fn default_streak_threshold() -> u32 {
    2
}

pub(crate) fn default_implicit_min_versions() -> u32 {
    10
}

pub(crate) fn default_implicit_persistence_threshold() -> f64 {
    0.80
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_keeps_implicit_inference_off() {
        let config = ContractsConfig::default();
        assert!(!config.implicit_inference_enabled);
        assert_eq!(config.implicit_min_versions, 10);
        assert_eq!(config.implicit_persistence_threshold, 0.80);
    }

    #[test]
    fn effective_min_versions_maps_zero_and_clamps() {
        let mut config = ContractsConfig {
            implicit_min_versions: 0,
            ..ContractsConfig::default()
        };
        assert_eq!(config.effective_implicit_min_versions(), 10);
        config.implicit_min_versions = 1;
        assert_eq!(config.effective_implicit_min_versions(), 3);
        config.implicit_min_versions = 500;
        assert_eq!(config.effective_implicit_min_versions(), 50);
        config.implicit_min_versions = 12;
        assert_eq!(config.effective_implicit_min_versions(), 12);
    }

    #[test]
    fn effective_persistence_threshold_clamps_and_rejects_non_finite() {
        let mut config = ContractsConfig {
            implicit_persistence_threshold: 1.5,
            ..ContractsConfig::default()
        };
        assert_eq!(config.effective_implicit_persistence_threshold(), 1.0);
        config.implicit_persistence_threshold = 0.2;
        assert_eq!(config.effective_implicit_persistence_threshold(), 0.5);
        config.implicit_persistence_threshold = f64::NAN;
        assert_eq!(config.effective_implicit_persistence_threshold(), 0.80);
        config.implicit_persistence_threshold = f64::INFINITY;
        assert_eq!(config.effective_implicit_persistence_threshold(), 0.80);
        config.implicit_persistence_threshold = 0.9;
        assert_eq!(config.effective_implicit_persistence_threshold(), 0.9);
    }

    #[test]
    fn history_window_is_twice_min_capped_at_max() {
        let mut config = ContractsConfig::default();
        assert_eq!(config.implicit_history_window(), 20);
        config.implicit_min_versions = 40;
        assert_eq!(config.implicit_history_window(), 50);
        config.implicit_min_versions = 1;
        assert_eq!(config.implicit_history_window(), 6);
    }
}
