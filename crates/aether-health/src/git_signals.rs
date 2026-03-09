use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_config::HealthScoreConfig;
use aether_core::git::GitContext;

use crate::metrics::count_loc;
use crate::models::{GitSignals, HealthError};
use crate::scanner::WorkspaceCrate;

const GIT_FILE_LIMIT: usize = 20;
const CHURN_30D_WEIGHT: f64 = 0.35;
const CHURN_90D_WEIGHT: f64 = 0.25;
const AUTHOR_COUNT_WEIGHT: f64 = 0.25;
const BLAME_AGE_WEIGHT: f64 = 0.15;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileGitStats {
    pub commits_30d: usize,
    pub commits_90d: usize,
    pub author_count: usize,
    pub blame_age_std_dev: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct FileGitSignals {
    pub path: String,
    pub raw: FileGitStats,
    pub normalized: GitSignals,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct GitSignalAnalysis {
    pub files: Vec<FileGitSignals>,
    pub signals: GitSignals,
}

pub fn compute_file_git_stats(git: &GitContext, file_path: &Path) -> FileGitStats {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let cutoff_30d = now - 30 * 24 * 60 * 60;
    let cutoff_90d = now - 90 * 24 * 60 * 60;

    let commits = git.file_log(file_path, 500);
    let commits_30d = commits
        .iter()
        .filter(|commit| commit.timestamp >= cutoff_30d)
        .count();
    let commits_90d = commits
        .iter()
        .filter(|commit| commit.timestamp >= cutoff_90d)
        .count();

    let blame = git.blame_lines(file_path);
    let authors = blame
        .iter()
        .map(|line| line.author.as_str())
        .collect::<HashSet<_>>();
    let commit_timestamps = commits
        .iter()
        .map(|commit| (commit.hash.clone(), commit.timestamp))
        .collect::<HashMap<_, _>>();
    let blame_timestamps = blame
        .iter()
        .filter_map(|line| commit_timestamps.get(line.commit_hash.as_str()).copied())
        .collect::<Vec<_>>();

    FileGitStats {
        commits_30d,
        commits_90d,
        author_count: authors.len(),
        blame_age_std_dev: standard_deviation(&blame_timestamps),
    }
}

pub fn normalize_git_signals(stats: &FileGitStats, config: &HealthScoreConfig) -> GitSignals {
    let churn_30d = normalize_count(stats.commits_30d, config.churn_30d_high);
    let churn_90d = normalize_count(stats.commits_90d, config.churn_90d_high);
    let author_count = normalize_author_count(stats.author_count, config.author_count_high);
    let blame_age_spread = if config.blame_age_spread_high_secs == 0 {
        0.0
    } else {
        (stats.blame_age_std_dev / config.blame_age_spread_high_secs as f64).clamp(0.0, 1.0)
    };

    GitSignals {
        churn_30d,
        churn_90d,
        author_count,
        blame_age_spread,
        git_pressure: combined_git_pressure(churn_30d, churn_90d, author_count, blame_age_spread),
    }
}

pub fn aggregate_crate_git_signals(file_stats: &[GitSignals]) -> GitSignals {
    if file_stats.is_empty() {
        return GitSignals::default();
    }

    let churn_30d = file_stats
        .iter()
        .map(|entry| entry.churn_30d)
        .fold(0.0, f64::max);
    let churn_90d = file_stats
        .iter()
        .map(|entry| entry.churn_90d)
        .fold(0.0, f64::max);
    let author_count = file_stats
        .iter()
        .map(|entry| entry.author_count)
        .fold(0.0, f64::max);
    let blame_age_spread = file_stats
        .iter()
        .map(|entry| entry.blame_age_spread)
        .sum::<f64>()
        / file_stats.len() as f64;

    GitSignals {
        churn_30d,
        churn_90d,
        author_count,
        blame_age_spread,
        git_pressure: combined_git_pressure(churn_30d, churn_90d, author_count, blame_age_spread),
    }
}

pub(crate) fn analyze_crate_git_signals(
    crate_info: &WorkspaceCrate,
    workspace_root: &Path,
    git: &GitContext,
    config: &HealthScoreConfig,
) -> Result<GitSignalAnalysis, HealthError> {
    let files = largest_source_files(crate_info)?
        .into_iter()
        .map(|path| {
            let raw = compute_file_git_stats(git, &path);
            let normalized = normalize_git_signals(&raw, config);
            let display_path = path
                .strip_prefix(workspace_root)
                .unwrap_or(path.as_path())
                .display()
                .to_string();

            FileGitSignals {
                path: display_path,
                raw,
                normalized,
            }
        })
        .collect::<Vec<_>>();

    let signals = aggregate_crate_git_signals(
        &files
            .iter()
            .map(|entry| entry.normalized.clone())
            .collect::<Vec<_>>(),
    );

    Ok(GitSignalAnalysis { files, signals })
}

fn largest_source_files(crate_info: &WorkspaceCrate) -> Result<Vec<PathBuf>, HealthError> {
    let mut sized_files = Vec::with_capacity(crate_info.source_files.len());
    for path in &crate_info.source_files {
        let content = fs::read_to_string(path).map_err(|err| {
            HealthError::Message(format!("failed to read {}: {err}", path.display()))
        })?;
        let (loc, _) = count_loc(&content);
        sized_files.push((path.clone(), loc));
    }

    sized_files.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    Ok(sized_files
        .into_iter()
        .take(GIT_FILE_LIMIT)
        .map(|(path, _)| path)
        .collect())
}

fn normalize_count(value: usize, high: usize) -> f64 {
    if high == 0 {
        return 0.0;
    }

    (value as f64 / high as f64).clamp(0.0, 1.0)
}

fn normalize_author_count(value: usize, high: usize) -> f64 {
    if value <= 1 || high <= 1 {
        return 0.0;
    }

    ((value - 1) as f64 / (high - 1) as f64).clamp(0.0, 1.0)
}

fn combined_git_pressure(
    churn_30d: f64,
    churn_90d: f64,
    author_count: f64,
    blame_age_spread: f64,
) -> f64 {
    (churn_30d * CHURN_30D_WEIGHT
        + churn_90d * CHURN_90D_WEIGHT
        + author_count * AUTHOR_COUNT_WEIGHT
        + blame_age_spread * BLAME_AGE_WEIGHT)
        .clamp(0.0, 1.0)
}

fn standard_deviation(values: &[i64]) -> f64 {
    if values.len() <= 1 {
        return 0.0;
    }

    let mean = values.iter().sum::<i64>() as f64 / values.len() as f64;
    let variance = values
        .iter()
        .map(|value| {
            let delta = *value as f64 - mean;
            delta * delta
        })
        .sum::<f64>()
        / values.len() as f64;

    variance.sqrt()
}

#[cfg(test)]
mod tests {
    use aether_config::HealthScoreConfig;

    use super::{FileGitStats, aggregate_crate_git_signals, normalize_git_signals};

    #[test]
    fn normalize_git_signals_respects_thresholds() {
        let config = HealthScoreConfig::default();
        let stats = FileGitStats {
            commits_30d: 7,
            commits_90d: 15,
            author_count: 3,
            blame_age_std_dev: 7_776_000.0,
        };

        let normalized = normalize_git_signals(&stats, &config);
        assert!((normalized.churn_30d - (7.0 / 15.0)).abs() < 1e-6);
        assert!((normalized.author_count - 0.4).abs() < 1e-6);
        assert!((normalized.blame_age_spread - 0.5).abs() < 1e-6);
    }

    #[test]
    fn aggregate_git_signals_uses_max_and_mean() {
        let aggregated = aggregate_crate_git_signals(&[
            normalize_git_signals(
                &FileGitStats {
                    commits_30d: 15,
                    commits_90d: 10,
                    author_count: 2,
                    blame_age_std_dev: 100.0,
                },
                &HealthScoreConfig::default(),
            ),
            normalize_git_signals(
                &FileGitStats {
                    commits_30d: 1,
                    commits_90d: 30,
                    author_count: 6,
                    blame_age_std_dev: 15_552_000.0,
                },
                &HealthScoreConfig::default(),
            ),
        ]);

        assert_eq!(aggregated.churn_30d, 1.0);
        assert_eq!(aggregated.churn_90d, 1.0);
        assert_eq!(aggregated.author_count, 1.0);
        assert!(aggregated.blame_age_spread > 0.49);
    }
}
