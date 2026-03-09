use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub enum HealthError {
    Io(std::io::Error),
    Toml(toml::de::Error),
    Json(serde_json::Error),
    Sql(rusqlite::Error),
    Message(String),
}

impl fmt::Display for HealthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::Toml(err) => write!(f, "toml parse error: {err}"),
            Self::Json(err) => write!(f, "json error: {err}"),
            Self::Sql(err) => write!(f, "sqlite error: {err}"),
            Self::Message(message) => f.write_str(message),
        }
    }
}

impl Error for HealthError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Toml(err) => Some(err),
            Self::Json(err) => Some(err),
            Self::Sql(err) => Some(err),
            Self::Message(_) => None,
        }
    }
}

impl From<std::io::Error> for HealthError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<toml::de::Error> for HealthError {
    fn from(value: toml::de::Error) -> Self {
        Self::Toml(value)
    }
}

impl From<serde_json::Error> for HealthError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<rusqlite::Error> for HealthError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sql(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Healthy,
    Watch,
    Moderate,
    High,
    Critical,
}

impl Severity {
    pub fn from_score(score: u32) -> Self {
        match score {
            0..=24 => Self::Healthy,
            25..=49 => Self::Watch,
            50..=69 => Self::Moderate,
            70..=84 => Self::High,
            _ => Self::Critical,
        }
    }

    pub fn as_label(self) -> &'static str {
        match self {
            Self::Healthy => "Healthy",
            Self::Watch => "Watch",
            Self::Moderate => "Moderate",
            Self::High => "High",
            Self::Critical => "Critical",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Archetype {
    #[serde(rename = "God File")]
    GodFile,
    #[serde(rename = "Brittle Hub")]
    BrittleHub,
    #[serde(rename = "Churn Magnet")]
    ChurnMagnet,
    #[serde(rename = "Legacy Residue")]
    LegacyResidue,
    #[serde(rename = "Boundary Leaker")]
    BoundaryLeaker,
    #[serde(rename = "Zombie File")]
    ZombieFile,
    #[serde(rename = "False Stable")]
    FalseStable,
}

impl Archetype {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GodFile => "God File",
            Self::BrittleHub => "Brittle Hub",
            Self::ChurnMagnet => "Churn Magnet",
            Self::LegacyResidue => "Legacy Residue",
            Self::BoundaryLeaker => "Boundary Leaker",
            Self::ZombieFile => "Zombie File",
            Self::FalseStable => "False Stable",
        }
    }
}

impl fmt::Display for Archetype {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViolationLevel {
    Warn,
    Fail,
}

impl ViolationLevel {
    pub fn as_tag(self) -> &'static str {
        match self {
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Violation {
    pub metric: String,
    pub value: f64,
    pub threshold: f64,
    #[serde(rename = "severity")]
    pub severity: ViolationLevel,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceViolation {
    pub crate_name: String,
    #[serde(flatten)]
    pub violation: Violation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrateMetricsSnapshot {
    pub max_file_loc: usize,
    pub trait_method_max: usize,
    pub internal_dep_count: usize,
    pub todo_density: f32,
    pub dead_feature_flags: usize,
    pub stale_backend_refs: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GitSignals {
    pub churn_30d: f64,
    pub churn_90d: f64,
    pub author_count: f64,
    pub blame_age_spread: f64,
    pub git_pressure: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SemanticSignals {
    pub max_centrality: f64,
    pub drift_density: f64,
    pub stale_sir_ratio: f64,
    pub test_gap: f64,
    pub boundary_leakage: f64,
    pub semantic_pressure: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalAvailability {
    pub git_available: bool,
    pub semantic_available: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScoreBreakdown {
    pub structural: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrateScore {
    pub name: String,
    pub score: u32,
    pub severity: Severity,
    pub archetypes: Vec<Archetype>,
    pub total_loc: usize,
    pub file_count: usize,
    pub total_lines: usize,
    pub metrics: CrateMetricsSnapshot,
    pub violations: Vec<Violation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_signals: Option<GitSignals>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_signals: Option<SemanticSignals>,
    #[serde(default, skip_serializing_if = "signal_availability_is_default")]
    pub signal_availability: SignalAvailability,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_breakdown: Option<ScoreBreakdown>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoreReport {
    pub schema_version: u32,
    pub run_at: u64,
    pub git_commit: Option<String>,
    pub workspace_score: u32,
    pub severity: Severity,
    pub previous_score: Option<u32>,
    pub delta: Option<i32>,
    pub crate_count: usize,
    pub total_loc: usize,
    pub crates: Vec<CrateScore>,
    pub worst_crate: Option<String>,
    pub top_violations: Vec<WorkspaceViolation>,
    #[serde(skip, default)]
    pub workspace_root: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CrateMetrics {
    pub total_loc: usize,
    pub total_lines: usize,
    pub file_count: usize,
    pub max_file_loc: usize,
    pub max_file_path: Option<String>,
    pub trait_method_max: usize,
    pub trait_name: Option<String>,
    pub internal_dep_count: usize,
    pub todo_density: f32,
    pub dead_feature_flags: usize,
    pub stale_backend_refs: usize,
}

impl CrateMetrics {
    pub fn snapshot(&self) -> CrateMetricsSnapshot {
        CrateMetricsSnapshot {
            max_file_loc: self.max_file_loc,
            trait_method_max: self.trait_method_max,
            internal_dep_count: self.internal_dep_count,
            todo_density: self.todo_density,
            dead_feature_flags: self.dead_feature_flags,
            stale_backend_refs: self.stale_backend_refs,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MetricPenalties {
    pub max_file_loc: f64,
    pub trait_method_max: f64,
    pub internal_dep_count: f64,
    pub todo_density: f64,
    pub dead_feature_flags: f64,
    pub stale_backend_refs: f64,
}

impl MetricPenalties {
    pub fn total(self) -> f64 {
        self.max_file_loc
            + self.trait_method_max
            + self.internal_dep_count
            + self.todo_density
            + self.dead_feature_flags
            + self.stale_backend_refs
    }
}

fn signal_availability_is_default(value: &SignalAvailability) -> bool {
    !value.git_available && !value.semantic_available && value.notes.is_empty()
}
