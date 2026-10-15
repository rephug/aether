//! Workspace-wide (global) Louvain communities as drift and health scoring use them:
//! test symbols left out and the planner's resolution, so that files are judged by
//! how production code clusters (decision #89.1).

use std::collections::{HashMap, HashSet};

pub use aether_core::is_test_symbol;
use aether_store::{SqliteStore, SurrealGraphStore, SymbolRecord};

use crate::coupling::AnalysisError;

/// Ids of every indexed symbol that [`is_test_symbol`] classifies as test code.
pub fn test_symbol_ids(store: &SqliteStore) -> Result<HashSet<String>, AnalysisError> {
    Ok(store
        .list_symbol_names_and_paths()?
        .into_iter()
        .filter(|(_, qualified_name, file_path)| is_test_symbol(qualified_name, file_path))
        .map(|(id, _, _)| id)
        .collect())
}

/// Global Louvain community assignments with test symbols excluded, at `resolution`
/// (normally `planner.community_resolution`).
pub async fn production_community_assignments(
    store: &SqliteStore,
    graph: &SurrealGraphStore,
    resolution: f64,
) -> Result<Vec<(String, i64)>, AnalysisError> {
    let exclude = test_symbol_ids(store)?;
    Ok(graph
        .list_louvain_communities_filtered(resolution, &exclude)
        .await?)
}

/// How many production communities a file's symbols are split across: those holding at
/// least three of its symbols and at least a fifth of its production symbols. Test
/// symbols belong to no community, so they count toward neither.
pub fn file_community_count(
    symbols: &[SymbolRecord],
    community_by_symbol: &HashMap<String, i64>,
) -> usize {
    let production_count = symbols
        .iter()
        .filter(|symbol| !is_test_symbol(&symbol.qualified_name, &symbol.file_path))
        .count();
    let threshold = 3_usize.max((production_count as f64 * 0.2).ceil() as usize);
    let mut sizes = HashMap::<i64, usize>::new();
    for symbol in symbols {
        if let Some(community) = community_by_symbol.get(symbol.id.as_str()) {
            *sizes.entry(*community).or_default() += 1;
        }
    }
    sizes.values().filter(|&&size| size >= threshold).count()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use aether_store::SymbolRecord;

    use super::file_community_count;

    fn symbol(id: &str, qualified_name: &str) -> SymbolRecord {
        SymbolRecord {
            id: id.to_owned(),
            file_path: "crates/a/src/parser.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature_fingerprint: String::new(),
            last_seen_at: 0,
        }
    }

    #[test]
    fn test_symbols_dont_raise_the_per_file_community_threshold() {
        // Ten production functions in two communities of five, beside thirty test functions
        // of an inline `mod tests`, which global communities leave out. Counting the tests,
        // a community would need a fifth of 40 symbols (8) to count; it needs a fifth of
        // the 10 production ones, or 3.
        let mut symbols = Vec::new();
        let mut community_by_symbol = HashMap::new();
        for index in 0..10 {
            let id = format!("p{index}");
            community_by_symbol.insert(id.clone(), i64::from(index % 2));
            symbols.push(symbol(&id, &format!("parse_{index}")));
        }
        for index in 0..30 {
            symbols.push(symbol(
                &format!("t{index}"),
                &format!("tests::case_{index}"),
            ));
        }
        assert_eq!(file_community_count(&symbols, &community_by_symbol), 2);

        // Communities of one or two production symbols are below the floor of three.
        community_by_symbol.insert("p0".to_owned(), 7);
        community_by_symbol.insert("p2".to_owned(), 7);
        community_by_symbol.insert("p4".to_owned(), 8);
        assert_eq!(file_community_count(&symbols, &community_by_symbol), 1);
    }
}
