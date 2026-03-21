use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_analysis::{
    SirQualitySignals, blend_normalized_quality, compute_confidence_percentiles,
    compute_sir_quality_signals,
};
use aether_store::SqliteStore;
use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use serde::Deserialize;

use crate::cli::ComputeQualityArgs;

#[derive(Debug, Clone, Deserialize)]
struct SirQualityPayload {
    intent: String,
    confidence: f32,
}

#[derive(Debug, Clone)]
struct ParsedSirRow {
    sir_id: String,
    model: String,
    generation_pass: String,
    intent: String,
    confidence: f32,
    has_quality_row: bool,
}

#[derive(Debug, Clone)]
struct PersistedQualityRow {
    sir_id: String,
    specificity: f64,
    behavioral_depth: f64,
    error_coverage: f64,
    length_score: f64,
    composite_quality: f64,
    confidence_percentile: f64,
    normalized_quality: f64,
    computed_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
struct GroupAverage {
    model: String,
    generation_pass: String,
    total: usize,
    average_normalized_quality: f64,
}

#[derive(Debug, Clone, PartialEq)]
struct HistogramBucket {
    label: String,
    count: usize,
}

#[derive(Debug, Clone, PartialEq)]
struct ComputeQualityReport {
    scanned_rows: usize,
    written_rows: usize,
    skipped_rows: usize,
    touched_groups: usize,
    group_averages: Vec<GroupAverage>,
    histogram: Vec<HistogramBucket>,
}

pub fn run_compute_quality_command(workspace: &Path, args: ComputeQualityArgs) -> Result<()> {
    let store = SqliteStore::open(workspace).context("failed to open local store")?;
    let sqlite_path = store.aether_dir().join("meta.sqlite");
    drop(store);

    let mut conn = Connection::open(&sqlite_path)
        .with_context(|| format!("failed to open {}", sqlite_path.display()))?;
    conn.busy_timeout(Duration::from_secs(5))
        .context("failed to set SQLite busy timeout")?;

    let report = compute_quality_for_connection(&mut conn, args.recompute)
        .context("failed to compute SIR quality scores")?;
    let mut out = std::io::stdout();
    write_report(&mut out, &report)
}

fn compute_quality_for_connection(
    conn: &mut Connection,
    recompute: bool,
) -> Result<ComputeQualityReport> {
    let raw_rows = load_sir_rows(conn).context("failed to load SIR rows")?;
    let scanned_rows = raw_rows.len();

    let mut skipped_rows = 0usize;
    let mut grouped_rows = BTreeMap::<(String, String), Vec<ParsedSirRow>>::new();
    for row in raw_rows {
        let Some(parsed) = parse_sir_row(row, &mut skipped_rows) else {
            continue;
        };
        grouped_rows
            .entry((parsed.model.clone(), parsed.generation_pass.clone()))
            .or_default()
            .push(parsed);
    }

    let touched_groups = select_touched_groups(&grouped_rows, recompute);
    let written_rows = if touched_groups.is_empty() {
        0
    } else {
        let computed_at = unix_timestamp_secs()?;
        let mut rows_to_write = Vec::new();
        for group in &touched_groups {
            let Some(rows) = grouped_rows.get(group) else {
                continue;
            };
            rows_to_write.extend(compute_group_quality_rows(rows, computed_at));
        }
        upsert_quality_rows(conn, &rows_to_write).context("failed to persist sir_quality rows")?;
        rows_to_write.len()
    };

    Ok(ComputeQualityReport {
        scanned_rows,
        written_rows,
        skipped_rows,
        touched_groups: touched_groups.len(),
        group_averages: load_group_averages(conn).context("failed to load quality averages")?,
        histogram: load_histogram(conn).context("failed to load quality histogram")?,
    })
}

fn load_sir_rows(conn: &Connection) -> rusqlite::Result<Vec<RawSirRow>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT
            s.id,
            s.model,
            s.generation_pass,
            s.sir_json,
            CASE WHEN q.sir_id IS NULL THEN 0 ELSE 1 END
        FROM sir s
        LEFT JOIN sir_quality q ON q.sir_id = s.id
        ORDER BY
            COALESCE(NULLIF(TRIM(s.generation_pass), ''), 'scan') ASC,
            COALESCE(TRIM(s.model), '') ASC,
            s.id ASC
        "#,
    )?;

    let rows = stmt
        .query_map([], |row| {
            Ok(RawSirRow {
                sir_id: row.get(0)?,
                model: row.get(1)?,
                generation_pass: row.get(2)?,
                sir_json: row.get(3)?,
                has_quality_row: row.get::<_, i64>(4)? != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn parse_sir_row(row: RawSirRow, skipped_rows: &mut usize) -> Option<ParsedSirRow> {
    let sir_json = match row.sir_json {
        Some(sir_json) => sir_json,
        None => {
            *skipped_rows += 1;
            return None;
        }
    };
    let sir_json = sir_json.trim().to_owned();
    if sir_json.is_empty() {
        *skipped_rows += 1;
        return None;
    }

    let payload: SirQualityPayload = match serde_json::from_str(sir_json.as_str()) {
        Ok(payload) => payload,
        Err(_) => {
            *skipped_rows += 1;
            return None;
        }
    };
    let intent = payload.intent.trim();
    if intent.is_empty()
        || !payload.confidence.is_finite()
        || !(0.0..=1.0).contains(&payload.confidence)
    {
        *skipped_rows += 1;
        return None;
    }

    Some(ParsedSirRow {
        sir_id: row.sir_id,
        model: normalize_model(row.model),
        generation_pass: normalize_generation_pass(row.generation_pass),
        intent: intent.to_owned(),
        confidence: payload.confidence,
        has_quality_row: row.has_quality_row,
    })
}

fn select_touched_groups(
    grouped_rows: &BTreeMap<(String, String), Vec<ParsedSirRow>>,
    recompute: bool,
) -> BTreeSet<(String, String)> {
    if recompute {
        return grouped_rows.keys().cloned().collect();
    }

    grouped_rows
        .iter()
        .filter_map(|(group, rows)| {
            rows.iter()
                .any(|row| !row.has_quality_row)
                .then_some(group.clone())
        })
        .collect()
}

fn compute_group_quality_rows(rows: &[ParsedSirRow], computed_at: i64) -> Vec<PersistedQualityRow> {
    let percentiles =
        compute_confidence_percentiles(&rows.iter().map(|row| row.confidence).collect::<Vec<_>>());

    rows.iter()
        .zip(percentiles)
        .map(|(row, confidence_percentile)| {
            let signals = compute_sir_quality_signals(row.intent.as_str());
            build_quality_row(row, signals, confidence_percentile, computed_at)
        })
        .collect()
}

fn build_quality_row(
    row: &ParsedSirRow,
    signals: SirQualitySignals,
    confidence_percentile: f64,
    computed_at: i64,
) -> PersistedQualityRow {
    PersistedQualityRow {
        sir_id: row.sir_id.clone(),
        specificity: signals.specificity,
        behavioral_depth: signals.behavioral_depth,
        error_coverage: signals.error_coverage,
        length_score: signals.length_score,
        composite_quality: signals.composite_quality,
        confidence_percentile,
        normalized_quality: blend_normalized_quality(
            signals.composite_quality,
            confidence_percentile,
        ),
        computed_at,
    }
}

fn upsert_quality_rows(
    conn: &mut Connection,
    rows: &[PersistedQualityRow],
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            r#"
            INSERT INTO sir_quality (
                sir_id,
                specificity,
                behavioral_depth,
                error_coverage,
                length_score,
                composite_quality,
                confidence_percentile,
                normalized_quality,
                computed_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(sir_id) DO UPDATE SET
                specificity = excluded.specificity,
                behavioral_depth = excluded.behavioral_depth,
                error_coverage = excluded.error_coverage,
                length_score = excluded.length_score,
                composite_quality = excluded.composite_quality,
                confidence_percentile = excluded.confidence_percentile,
                normalized_quality = excluded.normalized_quality,
                computed_at = excluded.computed_at
            "#,
        )?;
        for row in rows {
            stmt.execute(params![
                row.sir_id,
                row.specificity,
                row.behavioral_depth,
                row.error_coverage,
                row.length_score,
                row.composite_quality,
                row.confidence_percentile,
                row.normalized_quality,
                row.computed_at,
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn load_group_averages(conn: &Connection) -> rusqlite::Result<Vec<GroupAverage>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT
            COALESCE(TRIM(s.model), '') AS model,
            COALESCE(NULLIF(TRIM(s.generation_pass), ''), 'scan') AS generation_pass,
            COUNT(*) AS total,
            AVG(q.normalized_quality) AS avg_normalized_quality
        FROM sir s
        JOIN sir_quality q ON q.sir_id = s.id
        GROUP BY model, generation_pass
        ORDER BY generation_pass ASC, model ASC
        "#,
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(GroupAverage {
                model: row.get(0)?,
                generation_pass: row.get(1)?,
                total: row.get::<_, i64>(2)?.max(0) as usize,
                average_normalized_quality: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn load_histogram(conn: &Connection) -> rusqlite::Result<Vec<HistogramBucket>> {
    let mut counts = [0usize; 10];
    let mut stmt =
        conn.prepare("SELECT normalized_quality FROM sir_quality ORDER BY normalized_quality ASC")?;
    let values = stmt
        .query_map([], |row| row.get::<_, f64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    for value in values {
        let clamped = value.clamp(0.0, 1.0);
        let bucket = if clamped >= 1.0 {
            9
        } else {
            (clamped * 10.0).floor() as usize
        };
        counts[bucket] += 1;
    }

    Ok(counts
        .iter()
        .enumerate()
        .map(|(index, count)| HistogramBucket {
            label: histogram_label(index),
            count: *count,
        })
        .collect())
}

fn write_report(out: &mut dyn Write, report: &ComputeQualityReport) -> Result<()> {
    writeln!(out, "Scanned SIR rows: {}", report.scanned_rows)
        .context("failed to write scanned row count")?;
    writeln!(out, "Rows written/upserted: {}", report.written_rows)
        .context("failed to write written row count")?;
    writeln!(out, "Rows skipped: {}", report.skipped_rows)
        .context("failed to write skipped row count")?;
    writeln!(out, "Touched groups: {}", report.touched_groups)
        .context("failed to write touched group count")?;
    writeln!(out).context("failed to write summary spacer")?;

    writeln!(out, "Average normalized_quality by model/pass:")
        .context("failed to write averages header")?;
    if report.group_averages.is_empty() {
        writeln!(out, "  (no sir_quality rows)").context("failed to write empty averages state")?;
    } else {
        for entry in &report.group_averages {
            let model = display_model(entry.model.as_str());
            writeln!(
                out,
                "  {} | {} -> {:.3} (n={})",
                entry.generation_pass, model, entry.average_normalized_quality, entry.total
            )
            .context("failed to write averages row")?;
        }
    }
    writeln!(out).context("failed to write histogram spacer")?;

    writeln!(out, "Normalized quality histogram:").context("failed to write histogram header")?;
    for bucket in &report.histogram {
        let bar = "#".repeat(bucket.count.min(40));
        writeln!(out, "  {:>10} | {:>4} {}", bucket.label, bucket.count, bar)
            .context("failed to write histogram row")?;
    }

    Ok(())
}

fn histogram_label(index: usize) -> String {
    let start = index as f64 / 10.0;
    let end = (index + 1) as f64 / 10.0;
    if index == 9 {
        format!("{start:.1}-{end:.1}]")
    } else {
        format!("{start:.1}-{end:.1})")
    }
}

fn display_model(model: &str) -> &str {
    if model.is_empty() { "<empty>" } else { model }
}

fn normalize_model(model: Option<String>) -> String {
    model.unwrap_or_default().trim().to_owned()
}

fn normalize_generation_pass(generation_pass: Option<String>) -> String {
    let generation_pass = generation_pass.unwrap_or_default().trim().to_owned();
    if generation_pass.is_empty() {
        "scan".to_owned()
    } else {
        generation_pass
    }
}

fn unix_timestamp_secs() -> Result<i64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs() as i64)
}

#[derive(Debug, Clone)]
struct RawSirRow {
    sir_id: String,
    model: Option<String>,
    generation_pass: Option<String>,
    sir_json: Option<String>,
    has_quality_row: bool,
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use aether_config::{
        AetherConfig, EmbeddingVectorBackend, GraphBackend, save_workspace_config,
    };
    use aether_store::SqliteStore;
    use anyhow::Result;
    use rusqlite::{Connection, params};
    use serde_json::json;
    use tempfile::tempdir;

    use super::{compute_quality_for_connection, display_model};

    #[test]
    fn compute_quality_recomputes_entire_touched_group_by_default() -> Result<()> {
        let temp = tempdir()?;
        let workspace = temp.path();
        write_test_config(workspace)?;
        let store = SqliteStore::open(workspace)?;
        let sqlite_path = store.aether_dir().join("meta.sqlite");
        drop(store);

        let mut conn = Connection::open(&sqlite_path)?;
        seed_sir_row(
            &conn,
            "sym-a",
            "model-a",
            "scan",
            &json!({
                "intent": "Uses `cache_key` when input is missing and returns after retry.",
                "confidence": 0.2
            })
            .to_string(),
        )?;
        seed_sir_row(
            &conn,
            "sym-b",
            "model-a",
            "scan",
            &json!({
                "intent": "Delegates parsing to RequestConfig and then returns a result.",
                "confidence": 0.8
            })
            .to_string(),
        )?;
        seed_sir_row(
            &conn,
            "sym-c",
            "model-b",
            "triage",
            &json!({
                "intent": "Emits a summary.",
                "confidence": 0.6
            })
            .to_string(),
        )?;
        seed_sir_row(&conn, "sym-invalid", "model-a", "scan", "{\"intent\":123}")?;

        seed_quality_row(&conn, "sym-a", 11, 0.01)?;
        seed_quality_row(&conn, "sym-c", 22, 0.77)?;

        let report = compute_quality_for_connection(&mut conn, false)?;

        assert_eq!(report.scanned_rows, 4);
        assert_eq!(report.written_rows, 2);
        assert_eq!(report.skipped_rows, 1);
        assert_eq!(report.touched_groups, 1);

        let sym_a: (i64, f64) = conn.query_row(
            "SELECT computed_at, normalized_quality FROM sir_quality WHERE sir_id = 'sym-a'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let sym_b: (f64, f64) = conn.query_row(
            "SELECT confidence_percentile, normalized_quality FROM sir_quality WHERE sir_id = 'sym-b'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let sym_c_computed_at: i64 = conn.query_row(
            "SELECT computed_at FROM sir_quality WHERE sir_id = 'sym-c'",
            [],
            |row| row.get(0),
        )?;

        assert!(sym_a.0 > 11);
        assert!(sym_a.1 > 0.01);
        assert!((sym_b.0 - 1.0).abs() < 1e-9);
        assert_eq!(sym_c_computed_at, 22);
        assert_eq!(display_model(""), "<empty>");

        Ok(())
    }

    #[test]
    fn compute_quality_recompute_updates_all_groups() -> Result<()> {
        let temp = tempdir()?;
        let workspace = temp.path();
        write_test_config(workspace)?;
        let store = SqliteStore::open(workspace)?;
        let sqlite_path = store.aether_dir().join("meta.sqlite");
        drop(store);

        let mut conn = Connection::open(&sqlite_path)?;
        seed_sir_row(
            &conn,
            "sym-a",
            "model-a",
            "scan",
            &json!({
                "intent": "Returns value when parsing CacheEntry after timeout.",
                "confidence": 0.4
            })
            .to_string(),
        )?;
        seed_sir_row(
            &conn,
            "sym-b",
            "model-b",
            "triage",
            &json!({
                "intent": "Produces output.",
                "confidence": 0.4
            })
            .to_string(),
        )?;
        seed_quality_row(&conn, "sym-a", 1, 0.11)?;
        seed_quality_row(&conn, "sym-b", 2, 0.22)?;

        let report = compute_quality_for_connection(&mut conn, true)?;

        assert_eq!(report.written_rows, 2);
        assert_eq!(report.touched_groups, 2);
        let sym_a_percentile: f64 = conn.query_row(
            "SELECT confidence_percentile FROM sir_quality WHERE sir_id = 'sym-a'",
            [],
            |row| row.get(0),
        )?;
        let sym_b_computed_at: i64 = conn.query_row(
            "SELECT computed_at FROM sir_quality WHERE sir_id = 'sym-b'",
            [],
            |row| row.get(0),
        )?;

        assert!((sym_a_percentile - 0.5).abs() < 1e-9);
        assert!(sym_b_computed_at > 2);

        Ok(())
    }

    fn write_test_config(workspace: &Path) -> Result<()> {
        fs::create_dir_all(workspace.join(".aether"))?;

        let mut config = AetherConfig::default();
        config.storage.graph_backend = GraphBackend::Sqlite;
        config.embeddings.enabled = false;
        config.embeddings.vector_backend = EmbeddingVectorBackend::Sqlite;
        save_workspace_config(workspace, &config)?;
        Ok(())
    }

    fn seed_sir_row(
        conn: &Connection,
        sir_id: &str,
        model: &str,
        generation_pass: &str,
        sir_json: &str,
    ) -> Result<()> {
        conn.execute(
            r#"
            INSERT INTO sir (
                id, sir_hash, sir_version, provider, model, updated_at, sir_json, generation_pass
            )
            VALUES (?1, ?2, 1, 'mock', ?3, ?4, ?5, ?6)
            "#,
            params![
                sir_id,
                format!("hash-{sir_id}"),
                model,
                1_700_000_000i64,
                sir_json,
                generation_pass
            ],
        )?;
        Ok(())
    }

    fn seed_quality_row(
        conn: &Connection,
        sir_id: &str,
        computed_at: i64,
        normalized_quality: f64,
    ) -> Result<()> {
        conn.execute(
            r#"
            INSERT INTO sir_quality (
                sir_id,
                specificity,
                behavioral_depth,
                error_coverage,
                length_score,
                composite_quality,
                confidence_percentile,
                normalized_quality,
                computed_at
            )
            VALUES (?1, 0.1, 0.2, 0.3, 0.4, 0.25, 0.5, ?2, ?3)
            "#,
            params![sir_id, normalized_quality, computed_at],
        )?;
        Ok(())
    }
}
