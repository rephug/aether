//! How many stored call edges the graph resolves, so a tool can tell "calls nothing"
//! from "calls things the graph can't name".
//!
//! The counts use the rule the stored graph itself uses (`symbol_neighbors`, graph sync,
//! `get_call_chain`): an edge's target text matches a workspace symbol by exact qualified
//! name, in the caller's language family.

use super::*;
use crate::language_family::{EDGE_FILE_LANGUAGE_CTE, same_language_family_sql};

/// Most source ids bound in one statement.
const SOURCE_ID_CHUNK: usize = 500;

/// How a set of call edges resolves.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CallEdgeCoverage {
    pub call_edges: u64,
    /// The target names exactly one symbol.
    pub resolved: u64,
    /// The target names several symbols; the graph links every one of them.
    pub ambiguous: u64,
    /// No workspace symbol has the target's name: an external or standard-library call,
    /// or a call recorded by a name no symbol carries (the Rust parser records method
    /// calls by their bare method name). The graph drops these edges.
    pub unresolved: u64,
}

impl CallEdgeCoverage {
    fn add(&mut self, resolved: u64, ambiguous: u64, unresolved: u64) {
        self.call_edges += resolved + ambiguous + unresolved;
        self.resolved += resolved;
        self.ambiguous += ambiguous;
        self.unresolved += unresolved;
    }
}

/// The call edges recorded in one language's files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageCallEdgeCoverage {
    /// Empty for edges of files without indexed symbols.
    pub language: String,
    /// Edges whose caller is an indexed symbol: the ones dependency and call-chain tools
    /// walk.
    pub symbol_calls: CallEdgeCoverage,
    /// Edges from top-level code (a file, not a symbol, is the caller). No tool attributes
    /// them to a caller.
    pub file_level_calls: u64,
}

/// One row per (language, caller is a symbol) of the call edges `filter` selects.
fn coverage_sql(filter: &str) -> String {
    let source_language = "COALESCE(src.language, fl.language)";
    let family = same_language_family_sql(source_language, "nc.language");
    format!(
        r#"
        WITH name_counts AS MATERIALIZED (
            SELECT qualified_name, language, COUNT(*) AS n
            FROM symbols
            GROUP BY qualified_name, language
        ),
        {EDGE_FILE_LANGUAGE_CTE},
        edges AS (
            SELECT
                {source_language} AS language,
                src.id IS NOT NULL AS from_symbol,
                COALESCE(SUM(CASE WHEN {family} THEN nc.n ELSE 0 END), 0) AS candidates
            FROM symbol_edges e
            LEFT JOIN symbols src ON src.id = e.source_id
            LEFT JOIN edge_file_language fl ON fl.file_path = e.file_path
            LEFT JOIN name_counts nc ON nc.qualified_name = e.target_qualified_name
            WHERE e.edge_kind = 'calls' {filter}
            GROUP BY e.rowid
        )
        SELECT
            COALESCE(language, ''),
            from_symbol,
            SUM(CASE WHEN candidates = 1 THEN 1 ELSE 0 END),
            SUM(CASE WHEN candidates > 1 THEN 1 ELSE 0 END),
            SUM(CASE WHEN candidates = 0 THEN 1 ELSE 0 END)
        FROM edges
        GROUP BY 1, 2
        ORDER BY 1, 2
        "#
    )
}

type CoverageRow = (String, bool, u64, u64, u64);

fn coverage_rows(
    conn: &Connection,
    sql: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<Vec<CoverageRow>, StoreError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params, |row| {
        let count = |index: usize| -> rusqlite::Result<u64> {
            Ok(row.get::<_, Option<i64>>(index)?.unwrap_or(0).max(0) as u64)
        };
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, bool>(1)?,
            count(2)?,
            count(3)?,
            count(4)?,
        ))
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

impl SqliteStore {
    /// Every recorded call edge, by the language of the file it was read from.
    pub fn call_edge_coverage_by_language(
        &self,
    ) -> Result<Vec<LanguageCallEdgeCoverage>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut by_language = Vec::<LanguageCallEdgeCoverage>::new();
        for (language, from_symbol, resolved, ambiguous, unresolved) in
            coverage_rows(&conn, &coverage_sql(""), &[])?
        {
            let index = match by_language
                .iter()
                .position(|entry| entry.language == language)
            {
                Some(index) => index,
                None => {
                    by_language.push(LanguageCallEdgeCoverage {
                        language,
                        symbol_calls: CallEdgeCoverage::default(),
                        file_level_calls: 0,
                    });
                    by_language.len() - 1
                }
            };
            let entry = &mut by_language[index];
            if from_symbol {
                entry.symbol_calls.add(resolved, ambiguous, unresolved);
            } else {
                entry.file_level_calls += resolved + ambiguous + unresolved;
            }
        }
        Ok(by_language)
    }

    /// The call edges whose caller is one of `source_ids`.
    pub fn call_edge_coverage_for_sources(
        &self,
        source_ids: &[String],
    ) -> Result<CallEdgeCoverage, StoreError> {
        let mut source_ids = source_ids
            .iter()
            .map(|id| id.trim())
            .filter(|id| !id.is_empty())
            .collect::<Vec<_>>();
        source_ids.sort_unstable();
        source_ids.dedup();

        let conn = self.conn.lock().unwrap();
        let mut coverage = CallEdgeCoverage::default();
        for chunk in source_ids.chunks(SOURCE_ID_CHUNK) {
            let placeholders = (1..=chunk.len())
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = coverage_sql(&format!("AND e.source_id IN ({placeholders})"));
            let params = chunk
                .iter()
                .map(|id| id as &dyn rusqlite::ToSql)
                .collect::<Vec<_>>();
            for (_, _, resolved, ambiguous, unresolved) in coverage_rows(&conn, &sql, &params)? {
                coverage.add(resolved, ambiguous, unresolved);
            }
        }
        Ok(coverage)
    }
}

#[cfg(test)]
mod tests {
    use aether_core::{EdgeKind, SymbolEdge};
    use tempfile::tempdir;

    use super::CallEdgeCoverage;
    use crate::{SqliteStore, SymbolCatalogStore, SymbolRecord, SymbolRelationStore};

    fn symbol(id: &str, file_path: &str, language: &str, qualified_name: &str) -> SymbolRecord {
        SymbolRecord {
            id: id.to_owned(),
            file_path: file_path.to_owned(),
            language: language.to_owned(),
            kind: "function".to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature_fingerprint: format!("sig-{id}"),
            last_seen_at: 1,
        }
    }

    fn call(source_id: &str, target: &str, file_path: &str) -> SymbolEdge {
        SymbolEdge {
            source_id: source_id.to_owned(),
            target_qualified_name: target.to_owned(),
            edge_kind: EdgeKind::Calls,
            file_path: file_path.to_owned(),
        }
    }

    #[test]
    fn coverage_counts_resolved_ambiguous_unresolved_and_file_level_calls() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");
        for record in [
            symbol("caller", "src/lib.rs", "rust", "caller"),
            symbol("helper", "src/lib.rs", "rust", "helper"),
            symbol("dup-a", "src/a.rs", "rust", "dup"),
            symbol("dup-b", "src/b.rs", "rust", "dup"),
            // Same name in another language family: not a candidate for a Rust caller.
            symbol("py-helper", "pkg/mod.py", "python", "helper2"),
            symbol("py-caller", "pkg/mod.py", "python", "py_caller"),
        ] {
            store.upsert_symbol(record).expect("upsert symbol");
        }
        store
            .upsert_edges(&[
                call("caller", "helper", "src/lib.rs"),
                call("caller", "dup", "src/lib.rs"),
                call("caller", "Vec::new", "src/lib.rs"),
                call("caller", "helper2", "src/lib.rs"),
                call("py-caller", "helper2", "pkg/mod.py"),
                // Top-level Python code: the file is the caller.
                call("file::pkg/mod.py", "helper2", "pkg/mod.py"),
            ])
            .expect("upsert edges");

        assert_eq!(
            store
                .call_edge_coverage_for_sources(&["caller".to_owned()])
                .expect("coverage"),
            CallEdgeCoverage {
                call_edges: 4,
                resolved: 1,
                ambiguous: 1,
                unresolved: 2,
            }
        );
        assert_eq!(
            store
                .call_edge_coverage_for_sources(&["helper".to_owned()])
                .expect("coverage"),
            CallEdgeCoverage::default()
        );

        let by_language = store
            .call_edge_coverage_by_language()
            .expect("coverage by language");
        let python = by_language
            .iter()
            .find(|entry| entry.language == "python")
            .expect("python");
        assert_eq!(python.symbol_calls.resolved, 1);
        assert_eq!(python.file_level_calls, 1);
        let rust = by_language
            .iter()
            .find(|entry| entry.language == "rust")
            .expect("rust");
        assert_eq!(rust.symbol_calls.call_edges, 4);
        assert_eq!(rust.file_level_calls, 0);
    }
}
