//! SQL for resolving edges within a language family.
//!
//! An edge names its target by qualified name alone, and different languages can declare
//! the same qualified name: a Java class and a TypeScript class can both declare
//! `UserService::getUser`. An edge therefore resolves only to symbols of its source's
//! language family ([`aether_core::language_family`]).

use aether_core::ECMASCRIPT_LANGUAGES;

/// SQL condition that holds when the stored language names `left` and `right` (SQL
/// expressions) belong to the same language family.
pub fn same_language_family_sql(left: &str, right: &str) -> String {
    let ecmascript = ECMASCRIPT_LANGUAGES
        .iter()
        .map(|language| format!("'{language}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("({left} = {right} OR ({left} IN ({ecmascript}) AND {right} IN ({ecmascript})))")
}

/// A `WITH` table, `edge_file_language(file_path, language)`, of each indexed file's
/// language. It gives the language of an edge's source by the file the edge was read
/// from, which also covers edges whose source is the file itself (`file::<path>`), such
/// as imports and top-level calls. A file without indexed symbols has no row. It is
/// materialized so that it is computed once per statement, even inside a correlated
/// subquery.
pub const EDGE_FILE_LANGUAGE_CTE: &str = "edge_file_language(file_path, language) AS MATERIALIZED (\
     SELECT file_path, MIN(language) FROM symbols GROUP BY file_path)";

/// SQL condition that holds when the edge source language `source_language` (a column
/// of [`EDGE_FILE_LANGUAGE_CTE`], NULL for a file without symbols) is unknown or in the
/// family of `target_language`.
pub fn edge_source_family_matches_sql(source_language: &str, target_language: &str) -> String {
    format!(
        "({source_language} IS NULL OR {})",
        same_language_family_sql(source_language, target_language)
    )
}
