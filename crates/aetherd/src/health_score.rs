use std::io::Write;
use std::path::Path;

use aether_config::AetherConfig;
use aether_health::history::{create_table_if_needed, read_previous_score, write_score};
use aether_health::{
    ScoreReport, compute_workspace_score, compute_workspace_score_filtered, format_json,
    format_table,
};
use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::cli::{HealthScoreArgs, HealthScoreOutputFormat};

#[derive(Debug)]
pub struct HealthScoreExecution {
    pub report: ScoreReport,
    pub rendered: String,
    pub exit_code: i32,
}

pub fn run_health_score_command(
    workspace: &Path,
    config: &AetherConfig,
    args: HealthScoreArgs,
) -> Result<()> {
    let execution = execute_health_score_command(workspace, config, args)?;
    let mut stdout = std::io::stdout();
    stdout
        .write_all(execution.rendered.as_bytes())
        .context("failed to write health-score output")?;
    if !execution.rendered.ends_with('\n') {
        writeln!(&mut stdout).context("failed to terminate health-score output")?;
    }

    if execution.exit_code != 0 {
        std::process::exit(execution.exit_code);
    }

    Ok(())
}

pub fn execute_health_score_command(
    workspace: &Path,
    config: &AetherConfig,
    args: HealthScoreArgs,
) -> Result<HealthScoreExecution> {
    let mut report = if args.crate_filter.is_empty() {
        compute_workspace_score(workspace, &config.health_score)
    } else {
        compute_workspace_score_filtered(workspace, &config.health_score, &args.crate_filter)
    }
    .context("failed to compute structural health score")?;

    let history_allowed = !args.no_history && args.crate_filter.is_empty();
    if history_allowed {
        maybe_attach_history(workspace, &mut report)?;
    }

    let rendered = match args.output {
        HealthScoreOutputFormat::Table => format_table(&report),
        HealthScoreOutputFormat::Json => format_json(&report),
    };
    let exit_code = if args
        .fail_above
        .is_some_and(|threshold| report.workspace_score > threshold)
    {
        1
    } else {
        0
    };

    Ok(HealthScoreExecution {
        report,
        rendered,
        exit_code,
    })
}

fn maybe_attach_history(workspace: &Path, report: &mut ScoreReport) -> Result<()> {
    let sqlite_path = workspace.join(".aether").join("meta.sqlite");
    if !sqlite_path.exists() {
        return Ok(());
    }

    let conn = Connection::open(&sqlite_path)
        .with_context(|| format!("failed to open {}", sqlite_path.display()))?;
    create_table_if_needed(&conn).context("failed to prepare health score history table")?;
    if let Some((previous_score, _previous_json)) =
        read_previous_score(&conn).context("failed to read previous health score")?
    {
        report.previous_score = Some(previous_score);
        report.delta = Some(report.workspace_score as i32 - previous_score as i32);
    }
    write_score(&conn, report).context("failed to write health score history")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use aether_config::AetherConfig;
    use rusqlite::Connection;
    use tempfile::tempdir;

    use super::execute_health_score_command;
    use crate::cli::{HealthScoreArgs, HealthScoreOutputFormat};

    fn write_file(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().expect("test file path parent")).expect("create parent");
        fs::write(path, content).expect("write file");
    }

    fn create_workspace() -> tempfile::TempDir {
        let temp = tempdir().expect("tempdir");
        write_file(
            &temp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/example\"]\nresolver = \"2\"\n",
        );
        write_file(
            &temp.path().join("crates/example/Cargo.toml"),
            "[package]\nname = \"example\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        temp
    }

    fn default_args() -> HealthScoreArgs {
        HealthScoreArgs {
            output: HealthScoreOutputFormat::Json,
            fail_above: None,
            no_history: false,
            crate_filter: Vec::new(),
        }
    }

    #[test]
    fn fail_above_exit_code() {
        let workspace = create_workspace();
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub fn alpha() { let _ = \"cozo\"; let _ = \"cozo\"; }\n",
        );

        let mut args = default_args();
        args.fail_above = Some(0);
        let execution =
            execute_health_score_command(workspace.path(), &AetherConfig::default(), args)
                .expect("health-score execution");

        assert_eq!(execution.exit_code, 1);
    }

    #[test]
    fn no_aether_dir_no_error() {
        let workspace = create_workspace();
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub fn alpha() {}\n",
        );

        let execution = execute_health_score_command(
            workspace.path(),
            &AetherConfig::default(),
            default_args(),
        )
        .expect("health-score execution");

        assert_eq!(execution.exit_code, 0);
        assert!(!workspace.path().join(".aether").exists());
    }

    #[test]
    fn history_written_and_delta() {
        let workspace = create_workspace();
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub fn alpha() { let _ = \"cozo\"; let _ = \"cozo\"; }\n",
        );
        fs::create_dir_all(workspace.path().join(".aether")).expect("create .aether");
        Connection::open(workspace.path().join(".aether/meta.sqlite")).expect("create sqlite");

        let first = execute_health_score_command(
            workspace.path(),
            &AetherConfig::default(),
            default_args(),
        )
        .expect("first run");
        assert!(first.report.previous_score.is_none());

        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub fn alpha() { let _ = \"cozo\"; let _ = \"cozo\"; let _ = \"cozo\"; }\n",
        );
        let second = execute_health_score_command(
            workspace.path(),
            &AetherConfig::default(),
            default_args(),
        )
        .expect("second run");

        assert!(second.report.previous_score.is_some());
        assert!(second.report.delta.is_some());
    }

    #[test]
    fn filtered_run_skips_history() {
        let workspace = create_workspace();
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub fn alpha() { let _ = \"cozo\"; let _ = \"cozo\"; }\n",
        );
        fs::create_dir_all(workspace.path().join(".aether")).expect("create .aether");
        let conn = Connection::open(workspace.path().join(".aether/meta.sqlite")).expect("sqlite");
        conn.execute(
            "CREATE TABLE IF NOT EXISTS health_score_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_at INTEGER NOT NULL,
                git_commit TEXT,
                score INTEGER NOT NULL,
                score_json TEXT NOT NULL,
                UNIQUE(git_commit)
            )",
            [],
        )
        .expect("create table");

        let mut args = default_args();
        args.crate_filter = vec!["example".to_owned()];
        execute_health_score_command(workspace.path(), &AetherConfig::default(), args)
            .expect("filtered run");

        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM health_score_history", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("row count");
        assert_eq!(row_count, 0);
    }
}
