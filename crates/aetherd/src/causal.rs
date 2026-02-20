use std::io::Write;
use std::path::Path;

use aether_analysis::{CausalAnalyzer, TraceCauseRequest};
use aether_core::normalize_path;
use aether_store::{SqliteStore, Store};
use anyhow::{Context, Result, anyhow};

use crate::cli::TraceCauseArgs;

pub fn run_trace_cause_command(workspace: &Path, args: TraceCauseArgs) -> Result<()> {
    let store = SqliteStore::open(workspace).context("failed to open store")?;
    let target_symbol_id =
        resolve_target_symbol_id(&store, &args).context("failed to resolve target symbol")?;

    let analyzer =
        CausalAnalyzer::new(workspace).context("failed to initialize causal analyzer")?;
    let result = analyzer
        .trace_cause(TraceCauseRequest {
            target_symbol_id,
            lookback: Some(args.lookback),
            max_depth: Some(args.depth),
            limit: Some(args.limit),
        })
        .context("trace cause analysis failed")?;

    let value = serde_json::to_value(result).context("failed to serialize trace-cause output")?;
    write_json_to_stdout(&value)
}

fn resolve_target_symbol_id(store: &SqliteStore, args: &TraceCauseArgs) -> Result<String> {
    if let Some(symbol_id) = args.symbol_id.as_deref().map(str::trim)
        && !symbol_id.is_empty()
    {
        return store
            .get_symbol_record(symbol_id)?
            .map(|row| row.id)
            .ok_or_else(|| anyhow!("symbol not found, try `aether search \"{}\"`", symbol_id));
    }

    let symbol_name = args
        .symbol_name
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    let file = args.file.as_deref().map(normalize_path).unwrap_or_default();
    if symbol_name.is_empty() || file.is_empty() {
        return Err(anyhow!(
            "provide either --symbol-id, or <symbol_name> with --file"
        ));
    }

    let mut matches = store
        .list_symbols_for_file(file.as_str())?
        .into_iter()
        .filter(|row| {
            row.qualified_name == symbol_name
                || symbol_leaf_name(row.qualified_name.as_str()) == symbol_name
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| left.id.cmp(&right.id));
    matches
        .into_iter()
        .map(|row| row.id)
        .next()
        .ok_or_else(|| anyhow!("symbol not found, try `aether search \"{}\"`", symbol_name))
}

fn symbol_leaf_name(qualified_name: &str) -> &str {
    qualified_name
        .rsplit("::")
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(qualified_name)
}

fn write_json_to_stdout(value: &serde_json::Value) -> Result<()> {
    let mut out = std::io::stdout();
    serde_json::to_writer_pretty(&mut out, value).context("failed to serialize JSON output")?;
    writeln!(&mut out).context("failed to write trailing newline")?;
    Ok(())
}
