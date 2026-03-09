use rusqlite::{Connection, OptionalExtension};

use crate::Result;
use crate::models::ScoreReport;

pub fn create_table_if_needed(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS health_score_history (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            run_at      INTEGER NOT NULL,
            git_commit  TEXT,
            score       INTEGER NOT NULL,
            score_json  TEXT NOT NULL,
            UNIQUE(git_commit)
        );",
    )?;
    Ok(())
}

pub fn write_score(conn: &Connection, report: &ScoreReport) -> Result<()> {
    create_table_if_needed(conn)?;
    let score_json = serde_json::to_string(report)?;
    conn.execute(
        "INSERT INTO health_score_history (run_at, git_commit, score, score_json)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(git_commit) DO UPDATE SET
            run_at = excluded.run_at,
            score = excluded.score,
            score_json = excluded.score_json",
        (
            report.run_at as i64,
            report.git_commit.as_deref(),
            report.workspace_score as i64,
            score_json,
        ),
    )?;
    Ok(())
}

pub fn read_previous_score(conn: &Connection) -> Result<Option<(u32, String)>> {
    let mut statement = match conn.prepare(
        "SELECT score, score_json
         FROM health_score_history
         ORDER BY run_at DESC, id DESC
         LIMIT 1",
    ) {
        Ok(statement) => statement,
        Err(err) if is_missing_table(&err) => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let mut rows = statement.query([])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };

    let score = row.get::<_, i64>(0)? as u32;
    let score_json = row.get(1)?;
    Ok(Some((score, score_json)))
}

pub fn read_latest_report(conn: &Connection) -> Result<Option<ScoreReport>> {
    let score_json = match conn
        .query_row(
            "SELECT score_json
             FROM health_score_history
             ORDER BY run_at DESC, id DESC
             LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
    {
        Ok(score_json) => score_json,
        Err(err) if is_missing_table(&err) => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    score_json.map(deserialize_report).transpose()
}

pub fn read_recent_reports(conn: &Connection, limit: usize) -> Result<Vec<ScoreReport>> {
    let limit = limit.clamp(1, 1000) as i64;
    let mut statement = match conn.prepare(
        "SELECT score_json
         FROM health_score_history
         ORDER BY run_at DESC, id DESC
         LIMIT ?1",
    ) {
        Ok(statement) => statement,
        Err(err) if is_missing_table(&err) => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let rows = statement.query_map([limit], |row| row.get::<_, String>(0))?;
    let mut reports = Vec::new();
    for row in rows {
        reports.push(deserialize_report(row?)?);
    }
    Ok(reports)
}

pub fn read_report_by_commit_prefix(
    conn: &Connection,
    commit: &str,
) -> Result<Option<ScoreReport>> {
    let commit = commit.trim().to_ascii_lowercase();
    if commit.is_empty() {
        return Ok(None);
    }

    let score_json = match conn
        .query_row(
            "SELECT score_json
             FROM health_score_history
             WHERE git_commit IS NOT NULL
               AND (
                    LOWER(git_commit) LIKE ?1 || '%'
                    OR ?1 LIKE LOWER(git_commit) || '%'
               )
             ORDER BY
                CASE WHEN LOWER(git_commit) = ?1 THEN 0 ELSE 1 END,
                LENGTH(git_commit) DESC,
                run_at DESC,
                id DESC
             LIMIT 1",
            [commit.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
    {
        Ok(score_json) => score_json,
        Err(err) if is_missing_table(&err) => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    score_json.map(deserialize_report).transpose()
}

fn deserialize_report(score_json: String) -> Result<ScoreReport> {
    Ok(serde_json::from_str(&score_json)?)
}

fn is_missing_table(err: &rusqlite::Error) -> bool {
    err.to_string().contains("no such table")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rusqlite::Connection;

    use crate::models::{
        CrateMetricsSnapshot, CrateScore, Severity, SignalAvailability, WorkspaceViolation,
    };
    use crate::{Archetype, ScoreReport, Violation, ViolationLevel};

    use super::{
        create_table_if_needed, read_recent_reports, read_report_by_commit_prefix, write_score,
    };

    fn sample_report(git_commit: Option<&str>, workspace_score: u32) -> ScoreReport {
        ScoreReport {
            schema_version: 2,
            run_at: 1_700_000_000,
            git_commit: git_commit.map(str::to_owned),
            workspace_score,
            severity: Severity::from_score(workspace_score),
            previous_score: None,
            delta: None,
            crate_count: 1,
            total_loc: 100,
            crates: vec![CrateScore {
                name: "example".to_owned(),
                score: workspace_score,
                severity: Severity::from_score(workspace_score),
                archetypes: vec![Archetype::GodFile],
                total_loc: 100,
                file_count: 1,
                total_lines: 120,
                metrics: CrateMetricsSnapshot {
                    max_file_loc: 100,
                    max_file_path: Some("crates/example/src/lib.rs".to_owned()),
                    trait_method_max: 0,
                    internal_dep_count: 0,
                    todo_density: 0.0,
                    dead_feature_flags: 0,
                    stale_backend_refs: 0,
                },
                violations: vec![Violation {
                    metric: "max_file_loc".to_owned(),
                    value: 100.0,
                    threshold: 50.0,
                    severity: ViolationLevel::Warn,
                    reason: "big file".to_owned(),
                }],
                git_signals: None,
                semantic_signals: None,
                signal_availability: SignalAvailability::default(),
                score_breakdown: None,
            }],
            worst_crate: Some("example".to_owned()),
            top_violations: vec![WorkspaceViolation {
                crate_name: "example".to_owned(),
                violation: Violation {
                    metric: "max_file_loc".to_owned(),
                    value: 100.0,
                    threshold: 50.0,
                    severity: ViolationLevel::Warn,
                    reason: "big file".to_owned(),
                },
            }],
            workspace_root: PathBuf::from("/tmp/workspace"),
        }
    }

    #[test]
    fn read_report_by_commit_prefix_matches_short_and_full_hashes() {
        let conn = Connection::open_in_memory().expect("open sqlite");
        create_table_if_needed(&conn).expect("create table");
        write_score(&conn, &sample_report(Some("abc123"), 42)).expect("write score");

        let report = read_report_by_commit_prefix(&conn, "abc123def456")
            .expect("query report")
            .expect("matching report");
        assert_eq!(report.workspace_score, 42);
    }

    #[test]
    fn read_recent_reports_supports_legacy_rows_without_max_file_path() {
        let conn = Connection::open_in_memory().expect("open sqlite");
        create_table_if_needed(&conn).expect("create table");
        let legacy_json = r#"{
            "schema_version":2,
            "run_at":1700000000,
            "git_commit":"deadbee",
            "workspace_score":58,
            "severity":"moderate",
            "previous_score":null,
            "delta":null,
            "crate_count":1,
            "total_loc":200,
            "crates":[{
                "name":"example",
                "score":58,
                "severity":"moderate",
                "archetypes":["God File"],
                "total_loc":200,
                "file_count":1,
                "total_lines":240,
                "metrics":{
                    "max_file_loc":200,
                    "trait_method_max":10,
                    "internal_dep_count":1,
                    "todo_density":0.0,
                    "dead_feature_flags":0,
                    "stale_backend_refs":0
                },
                "violations":[],
                "signal_availability":{"git_available":false,"semantic_available":false,"notes":[]}
            }],
            "worst_crate":"example",
            "top_violations":[]
        }"#;
        conn.execute(
            "INSERT INTO health_score_history (run_at, git_commit, score, score_json)
             VALUES (?1, ?2, ?3, ?4)",
            (1_700_000_000_i64, "deadbee", 58_i64, legacy_json),
        )
        .expect("seed legacy row");

        let reports = read_recent_reports(&conn, 5).expect("read reports");
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].crates[0].metrics.max_file_path, None);
    }
}
