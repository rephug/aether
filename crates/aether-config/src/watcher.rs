use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatcherConfig {
    #[serde(default)]
    pub realtime_model: String,
    #[serde(default)]
    pub realtime_provider: String,
    #[serde(default = "default_true")]
    pub trigger_on_branch_switch: bool,
    #[serde(default = "default_true")]
    pub trigger_on_git_pull: bool,
    #[serde(default = "default_true")]
    pub trigger_on_merge: bool,
    #[serde(default = "default_true")]
    pub git_trigger_changed_files_only: bool,
    #[serde(default = "default_git_debounce")]
    pub git_debounce_secs: f64,
    #[serde(default)]
    pub trigger_on_build_success: bool,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            realtime_model: String::new(),
            realtime_provider: String::new(),
            trigger_on_branch_switch: default_true(),
            trigger_on_git_pull: default_true(),
            trigger_on_merge: default_true(),
            git_trigger_changed_files_only: default_true(),
            git_debounce_secs: default_git_debounce(),
            trigger_on_build_success: false,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_git_debounce() -> f64 {
    3.0
}
