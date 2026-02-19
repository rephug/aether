use std::io::Write;
use std::path::Path;

use aether_analysis::TestIntentAnalyzer;
use aether_core::normalize_path;
use aether_store::{SqliteStore, Store};
use anyhow::{Context, Result};
use serde_json::json;

use crate::cli::TestIntentsArgs;

pub fn run_test_intents_command(workspace: &Path, args: TestIntentsArgs) -> Result<()> {
    let file_path = normalize_path(args.file.trim());
    if file_path.is_empty() {
        return write_json_to_stdout(&json!({
            "file": "",
            "result_count": 0,
            "intents": [],
        }));
    }

    let analyzer = TestIntentAnalyzer::new(workspace).context("failed to initialize analyzer")?;
    let _ = analyzer
        .refresh_for_test_file(file_path.as_str())
        .context("failed to refresh tested_by links")?;

    let store = SqliteStore::open(workspace).context("failed to open store")?;
    let intents = store
        .list_test_intents_for_file(file_path.as_str())
        .context("failed to list test intents")?;
    let response = json!({
        "file": file_path,
        "result_count": intents.len(),
        "intents": intents,
    });
    write_json_to_stdout(&response)
}

fn write_json_to_stdout(value: &serde_json::Value) -> Result<()> {
    let mut out = std::io::stdout();
    serde_json::to_writer_pretty(&mut out, value).context("failed to serialize JSON output")?;
    writeln!(&mut out).context("failed to write trailing newline")?;
    Ok(())
}
