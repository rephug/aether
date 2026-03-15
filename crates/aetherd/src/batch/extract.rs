use std::collections::HashMap;
use std::path::Path;

use aether_core::Symbol;
use aether_store::SqliteStore;
use anyhow::{Context, Result};

use crate::indexer::run_structural_index_once;

pub(crate) struct ExtractSummary {
    pub store: SqliteStore,
    pub symbols_by_id: HashMap<String, Symbol>,
    pub symbol_count: usize,
}

pub(crate) fn run_extract(workspace: &Path) -> Result<ExtractSummary> {
    let (store, symbols_by_id, symbol_count) =
        run_structural_index_once(workspace).context("batch extract failed")?;
    Ok(ExtractSummary {
        store,
        symbols_by_id,
        symbol_count,
    })
}
