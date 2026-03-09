use rusqlite::Connection;

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
    create_table_if_needed(conn)?;
    let mut statement = conn.prepare(
        "SELECT score, score_json
         FROM health_score_history
         ORDER BY run_at DESC, id DESC
         LIMIT 1",
    )?;
    let mut rows = statement.query([])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };

    let score = row.get::<_, i64>(0)? as u32;
    let score_json = row.get(1)?;
    Ok(Some((score, score_json)))
}
