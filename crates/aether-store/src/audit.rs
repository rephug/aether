use std::time::{SystemTime, UNIX_EPOCH};

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditFinding {
    pub id: i64,
    pub symbol_id: String,
    pub audit_type: String,
    pub severity: String,
    pub category: String,
    pub certainty: String,
    pub trigger_condition: String,
    pub impact: String,
    pub description: String,
    pub related_symbols: Vec<String>,
    pub model: String,
    pub provider: String,
    pub reasoning: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub resolved_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAuditFinding {
    pub symbol_id: String,
    pub audit_type: String,
    pub severity: String,
    pub category: String,
    pub certainty: String,
    pub trigger_condition: String,
    pub impact: String,
    pub description: String,
    pub related_symbols: Vec<String>,
    pub model: String,
    pub provider: String,
    pub reasoning: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditFindingFilters {
    pub symbol_id: Option<String>,
    pub file_path_prefix: Option<String>,
    pub min_severity: Option<String>,
    pub category: Option<String>,
    pub status: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditSeverityCounts {
    pub total: u32,
    pub critical: u32,
    pub high: u32,
    pub medium: u32,
    pub low: u32,
    pub informational: u32,
}

const SEVERITY_ORDER_SQL: &str = r#"
CASE LOWER(TRIM(a.severity))
    WHEN 'critical' THEN 0
    WHEN 'high' THEN 1
    WHEN 'medium' THEN 2
    WHEN 'low' THEN 3
    WHEN 'informational' THEN 4
    ELSE 999
END
"#;

fn current_unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn normalize_optional_filter(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_owned())
}

fn severity_rank(value: &str) -> Option<i64> {
    match value.trim().to_ascii_lowercase().as_str() {
        "critical" => Some(0),
        "high" => Some(1),
        "medium" => Some(2),
        "low" => Some(3),
        "informational" => Some(4),
        _ => None,
    }
}

fn parse_string_array_json(raw: &str) -> Result<Vec<String>, StoreError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    Ok(json_from_str::<Vec<String>>(trimmed)?)
}

fn normalized_limit(limit: Option<u32>) -> Option<i64> {
    limit.map(|value| value.max(1) as i64)
}

fn normalized_file_path_prefix(prefix: Option<&str>) -> Option<String> {
    normalize_optional_filter(prefix.as_ref().copied()).map(|value| normalize_path(&value))
}

fn build_audit_where_clause(
    filters: &AuditFindingFilters,
) -> Result<(String, Vec<SqlValue>, bool), StoreError> {
    let mut conditions = Vec::<String>::new();
    let mut params = Vec::<SqlValue>::new();
    let mut join_symbols = false;

    if let Some(symbol_id) = normalize_optional_filter(filters.symbol_id.as_deref()) {
        conditions.push("a.symbol_id = ?".to_owned());
        params.push(SqlValue::Text(symbol_id));
    }

    if let Some(prefix) = normalized_file_path_prefix(filters.file_path_prefix.as_deref()) {
        join_symbols = true;
        conditions.push("s.file_path LIKE ?".to_owned());
        params.push(SqlValue::Text(format!("{prefix}%")));
    }

    if let Some(min_severity) = normalize_optional_filter(filters.min_severity.as_deref()) {
        let rank = severity_rank(min_severity.as_str()).ok_or_else(|| {
            StoreError::Compatibility(format!("invalid audit severity '{}'", min_severity.trim()))
        })?;
        conditions.push(format!("{SEVERITY_ORDER_SQL} <= ?"));
        params.push(SqlValue::Integer(rank));
    }

    if let Some(category) = normalize_optional_filter(filters.category.as_deref()) {
        conditions.push("LOWER(TRIM(a.category)) = ?".to_owned());
        params.push(SqlValue::Text(category.to_ascii_lowercase()));
    }

    if let Some(status) = normalize_optional_filter(filters.status.as_deref()) {
        conditions.push("LOWER(TRIM(a.status)) = ?".to_owned());
        params.push(SqlValue::Text(status.to_ascii_lowercase()));
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    Ok((where_clause, params, join_symbols))
}

fn audit_finding_from_row(row: &rusqlite::Row<'_>) -> Result<AuditFinding, StoreError> {
    let related_symbols_json = row.get::<_, String>(9)?;
    Ok(AuditFinding {
        id: row.get(0)?,
        symbol_id: row.get(1)?,
        audit_type: row.get(2)?,
        severity: row.get(3)?,
        category: row.get(4)?,
        certainty: row.get(5)?,
        trigger_condition: row.get(6)?,
        impact: row.get(7)?,
        description: row.get(8)?,
        related_symbols: parse_string_array_json(&related_symbols_json)?,
        model: row.get(10)?,
        provider: row.get(11)?,
        reasoning: row.get(12)?,
        status: row.get(13)?,
        created_at: row.get(14)?,
        resolved_at: row.get(15)?,
    })
}

impl SqliteStore {
    pub(crate) fn store_insert_audit_finding(
        &self,
        record: NewAuditFinding,
    ) -> Result<i64, StoreError> {
        let created_at = current_unix_timestamp();
        let related_symbols_json = serde_json::to_string(&record.related_symbols)?;
        let status = if record.status.trim().is_empty() {
            "open".to_owned()
        } else {
            record.status.trim().to_ascii_lowercase()
        };

        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO sir_audit (
                symbol_id, audit_type, severity, category, certainty, trigger_condition,
                impact, description, related_symbols, model, provider, reasoning, status,
                created_at, resolved_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, NULL)
            "#,
            params![
                record.symbol_id.trim(),
                record.audit_type.trim(),
                record.severity.trim(),
                record.category.trim(),
                record.certainty.trim(),
                record.trigger_condition.trim(),
                record.impact.trim(),
                record.description.trim(),
                related_symbols_json,
                record.model.trim(),
                record.provider.trim(),
                record.reasoning,
                status,
                created_at,
            ],
        )?;

        Ok(conn.last_insert_rowid())
    }

    pub(crate) fn store_query_audit_findings(
        &self,
        filters: &AuditFindingFilters,
    ) -> Result<Vec<AuditFinding>, StoreError> {
        let (where_clause, mut params_vec, join_symbols) = build_audit_where_clause(filters)?;
        let join_clause = if join_symbols {
            " JOIN symbols s ON s.id = a.symbol_id"
        } else {
            ""
        };
        let mut sql = format!(
            r#"
            SELECT
                a.id,
                a.symbol_id,
                a.audit_type,
                a.severity,
                a.category,
                a.certainty,
                a.trigger_condition,
                a.impact,
                a.description,
                a.related_symbols,
                a.model,
                a.provider,
                a.reasoning,
                a.status,
                a.created_at,
                a.resolved_at
            FROM sir_audit a
            {join_clause}
            {where_clause}
            ORDER BY {SEVERITY_ORDER_SQL} ASC, a.created_at DESC, a.id DESC
            "#
        );

        if let Some(limit) = normalized_limit(filters.limit) {
            sql.push_str(" LIMIT ?");
            params_vec.push(SqlValue::Integer(limit));
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(params_vec), |row| {
            audit_finding_from_row(row)
                .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))
        })?;

        let mut findings = Vec::new();
        for row in rows {
            findings.push(row?);
        }
        Ok(findings)
    }

    pub(crate) fn store_resolve_audit_finding(
        &self,
        finding_id: i64,
        status: &str,
    ) -> Result<bool, StoreError> {
        let resolved_at = current_unix_timestamp();
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            r#"
            UPDATE sir_audit
            SET status = ?2,
                resolved_at = ?3
            WHERE id = ?1
            "#,
            params![finding_id, status.trim(), resolved_at],
        )?;
        Ok(changed > 0)
    }

    pub(crate) fn store_count_audit_findings_by_severity(
        &self,
        filters: &AuditFindingFilters,
    ) -> Result<AuditSeverityCounts, StoreError> {
        let (where_clause, params_vec, join_symbols) = build_audit_where_clause(filters)?;
        let join_clause = if join_symbols {
            " JOIN symbols s ON s.id = a.symbol_id"
        } else {
            ""
        };
        let sql = format!(
            r#"
            SELECT LOWER(TRIM(a.severity)) AS severity, COUNT(*)
            FROM sir_audit a
            {join_clause}
            {where_clause}
            GROUP BY LOWER(TRIM(a.severity))
            "#
        );

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(params_vec), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;

        let mut counts = AuditSeverityCounts::default();
        for row in rows {
            let (severity, count) = row?;
            let count = count.max(0) as u32;
            counts.total = counts.total.saturating_add(count);
            match severity.as_str() {
                "critical" => counts.critical = count,
                "high" => counts.high = count,
                "medium" => counts.medium = count,
                "low" => counts.low = count,
                "informational" => counts.informational = count,
                _ => {}
            }
        }

        Ok(counts)
    }
}
