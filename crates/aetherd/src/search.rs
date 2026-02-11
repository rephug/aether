use std::io::Write;
use std::path::Path;

use aether_store::{SqliteStore, Store, SymbolSearchResult};
use anyhow::{Context, Result};

pub fn run_search_once(
    workspace: &Path,
    query: &str,
    limit: u32,
    out: &mut dyn Write,
) -> Result<()> {
    let store = SqliteStore::open(workspace).context("failed to initialize local store")?;
    let matches = store
        .search_symbols(query, limit)
        .context("failed to search symbols")?;
    write_search_results(&matches, out).context("failed to write search results")?;
    Ok(())
}

pub fn write_search_results(
    matches: &[SymbolSearchResult],
    out: &mut dyn Write,
) -> std::io::Result<()> {
    writeln!(out, "symbol_id\tqualified_name\tfile_path\tlanguage\tkind")?;

    for entry in matches {
        writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}",
            normalize_search_field(&entry.symbol_id),
            normalize_search_field(&entry.qualified_name),
            normalize_search_field(&entry.file_path),
            normalize_search_field(&entry.language),
            normalize_search_field(&entry.kind)
        )?;
    }

    Ok(())
}

fn normalize_search_field(value: &str) -> String {
    value.replace(['\t', '\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use aether_infer::{InferenceProvider, MockProvider};
    use tempfile::tempdir;

    use super::*;
    use crate::observer::ObserverState;
    use crate::sir_pipeline::SirPipeline;
    use aether_store::SymbolRecord;

    #[test]
    fn write_search_results_outputs_stable_header_and_columns() {
        let mut out = Vec::new();
        let matches = vec![SymbolSearchResult {
            symbol_id: "sym-1".to_owned(),
            qualified_name: "demo::run".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
        }];

        write_search_results(&matches, &mut out).expect("write output");
        let rendered = String::from_utf8(out).expect("utf8 output");
        let lines: Vec<&str> = rendered.lines().collect();

        assert_eq!(
            lines[0],
            "symbol_id\tqualified_name\tfile_path\tlanguage\tkind"
        );
        assert_eq!(lines.len(), 2);

        let columns: Vec<&str> = lines[1].split('\t').collect();
        assert_eq!(columns.len(), 5);
        assert_eq!(columns[1], "demo::run");
    }

    #[test]
    fn run_search_once_reads_symbols_from_store() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let store = SqliteStore::open(workspace).expect("open store");

        store
            .upsert_symbol(SymbolRecord {
                id: "sym-1".to_owned(),
                file_path: "src/lib.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                qualified_name: "demo::alpha".to_owned(),
                signature_fingerprint: "sig-a".to_owned(),
                last_seen_at: 1_700_000_000,
            })
            .expect("upsert symbol");

        let mut out = Vec::new();
        run_search_once(workspace, "alpha", 20, &mut out).expect("run search");

        let rendered = String::from_utf8(out).expect("utf8 output");
        assert!(rendered.contains("symbol_id\tqualified_name\tfile_path\tlanguage\tkind"));
        assert!(rendered.contains("demo::alpha"));
    }

    #[test]
    fn search_reflects_symbol_rename_and_removal() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();

        fs::create_dir_all(workspace.join("src")).expect("create src dir");
        let rust_file = workspace.join("src/lib.rs");
        fs::write(
            &rust_file,
            "fn alpha() -> i32 { 1 }\nfn beta() -> i32 { 2 }\n",
        )
        .expect("write source");

        let mut observer = ObserverState::new(workspace.to_path_buf()).expect("observer");
        observer.seed_from_disk().expect("seed observer");

        let store = SqliteStore::open(workspace).expect("open store");
        let provider: Arc<dyn InferenceProvider> = Arc::new(MockProvider);
        let pipeline =
            SirPipeline::new_with_provider(workspace.to_path_buf(), 2, provider, "mock", "mock")
                .expect("pipeline");

        let mut startup_stdout = Vec::new();
        for event in observer.initial_symbol_events() {
            pipeline
                .process_event(&store, &event, false, &mut startup_stdout)
                .expect("process startup event");
        }

        let alpha_hits = store.search_symbols("alpha", 20).expect("search alpha");
        assert_eq!(alpha_hits.len(), 1);

        fs::write(
            &rust_file,
            "fn gamma() -> i32 { 1 }\nfn beta() -> i32 { 2 }\n",
        )
        .expect("write renamed source");
        let rename_event = observer
            .process_path(&rust_file)
            .expect("process rename path")
            .expect("expected rename event");
        let mut update_stdout = Vec::new();
        pipeline
            .process_event(&store, &rename_event, false, &mut update_stdout)
            .expect("process rename event");

        let alpha_after_rename = store
            .search_symbols("alpha", 20)
            .expect("search alpha after rename");
        let gamma_after_rename = store
            .search_symbols("gamma", 20)
            .expect("search gamma after rename");
        assert!(alpha_after_rename.is_empty());
        assert_eq!(gamma_after_rename.len(), 1);

        fs::write(&rust_file, "fn gamma() -> i32 { 1 }\n").expect("write removal source");
        let removal_event = observer
            .process_path(&rust_file)
            .expect("process removal path")
            .expect("expected removal event");
        let mut removal_stdout = Vec::new();
        pipeline
            .process_event(&store, &removal_event, false, &mut removal_stdout)
            .expect("process removal event");

        let beta_after_remove = store
            .search_symbols("beta", 20)
            .expect("search beta after remove");
        assert!(beta_after_remove.is_empty());
    }
}
