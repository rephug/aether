use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeismographConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_noise_floor")]
    pub noise_floor: f64,
    #[serde(default = "default_ema_alpha")]
    pub ema_alpha: f64,
    #[serde(default = "default_community_window_days")]
    pub community_window_days: u32,
    #[serde(default = "default_cascade_max_depth")]
    pub cascade_max_depth: usize,
    #[serde(default)]
    pub aftershock_enabled: bool,
    #[serde(default = "default_aftershock_retrain_interval")]
    pub aftershock_retrain_interval: String,
}

impl Default for SeismographConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            noise_floor: default_noise_floor(),
            ema_alpha: default_ema_alpha(),
            community_window_days: default_community_window_days(),
            cascade_max_depth: default_cascade_max_depth(),
            aftershock_enabled: false,
            aftershock_retrain_interval: default_aftershock_retrain_interval(),
        }
    }
}

fn default_noise_floor() -> f64 {
    0.15
}

fn default_ema_alpha() -> f64 {
    0.2
}

fn default_community_window_days() -> u32 {
    30
}

fn default_cascade_max_depth() -> usize {
    10
}

fn default_aftershock_retrain_interval() -> String {
    "weekly".to_owned()
}
