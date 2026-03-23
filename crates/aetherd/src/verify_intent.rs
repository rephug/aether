use std::io::Write;
use std::path::Path;

use aether_analysis::{VerifyIntentReport, verify_intent_snapshot};
use aether_config::AetherConfig;
use aether_store::SqliteStore;
use anyhow::{Context, Result};

use crate::cli::VerifyIntentArgs;

#[derive(Debug)]
pub struct VerifyIntentExecution {
    pub report: VerifyIntentReport,
    pub rendered: String,
    pub exit_code: i32,
}

pub fn run_verify_intent_command(
    workspace: &Path,
    config: &AetherConfig,
    args: VerifyIntentArgs,
) -> Result<()> {
    let execution = execute_verify_intent_command(workspace, config, args)?;
    write_rendered_output(execution.rendered.as_str(), "verify-intent")?;
    if execution.exit_code != 0 {
        std::process::exit(execution.exit_code);
    }
    Ok(())
}

pub fn execute_verify_intent_command(
    workspace: &Path,
    _config: &AetherConfig,
    args: VerifyIntentArgs,
) -> Result<VerifyIntentExecution> {
    let store = SqliteStore::open(workspace).context("failed to open local store")?;
    let report = verify_intent_snapshot(workspace, &store, args.snapshot.as_str(), args.threshold)
        .context("failed to verify refactor intent snapshot")?;
    let rendered = render_verify_intent_human(&report);
    let exit_code = if report.passed { 0 } else { 1 };

    Ok(VerifyIntentExecution {
        report,
        rendered,
        exit_code,
    })
}

fn render_verify_intent_human(report: &VerifyIntentReport) -> String {
    let mut lines = vec![
        format!(
            "Verification: {}",
            if report.passed { "PASS" } else { "FAIL" }
        ),
        format!("Snapshot: {}", report.snapshot_id),
        format!("Scope: {}", report.scope),
        format!("Compared: {}", report.compared_entries),
        format!("Below threshold: {}", report.failed_entries),
        format!("Disappeared: {}", report.disappeared_symbols.len()),
        format!("New: {}", report.new_symbols.len()),
    ];

    if !report.entries.is_empty() {
        lines.push(String::new());
        lines.push("Entries:".to_owned());
        for entry in &report.entries {
            let issue = entry.issue.as_deref().unwrap_or("ok");
            lines.push(format!(
                "- {} [{}] similarity {:.2} via {} {}",
                entry.qualified_name, entry.file_path, entry.similarity, entry.method, issue
            ));
        }
    }

    if !report.disappeared_symbols.is_empty() {
        lines.push(String::new());
        lines.push("Disappeared symbols:".to_owned());
        for symbol in &report.disappeared_symbols {
            lines.push(format!(
                "- {} [{}]",
                symbol.qualified_name, symbol.file_path
            ));
        }
    }

    if !report.new_symbols.is_empty() {
        lines.push(String::new());
        lines.push("New symbols:".to_owned());
        for symbol in &report.new_symbols {
            lines.push(format!(
                "- {} [{}]",
                symbol.qualified_name, symbol.file_path
            ));
        }
    }

    if !report.notes.is_empty() {
        lines.push(String::new());
        lines.push("Notes:".to_owned());
        for note in &report.notes {
            lines.push(format!("- {note}"));
        }
    }

    lines.join("\n")
}

fn write_rendered_output(rendered: &str, label: &str) -> Result<()> {
    let mut stdout = std::io::stdout();
    stdout
        .write_all(rendered.as_bytes())
        .with_context(|| format!("failed to write {label} output"))?;
    if !rendered.ends_with('\n') {
        writeln!(&mut stdout).with_context(|| format!("failed to terminate {label} output"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use aether_analysis::{RefactorScope, collect_intent_snapshot};
    use aether_config::load_workspace_config;
    use aether_core::Language;
    use aether_store::{
        SirMetaRecord, SirStateStore, SnapshotStore, SqliteStore, SymbolCatalogStore, SymbolRecord,
    };
    use tempfile::tempdir;

    use super::execute_verify_intent_command;
    use crate::cli::VerifyIntentArgs;

    fn write_test_config(workspace: &Path) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[inference]
provider = "qwen3_local"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
        )
        .expect("write config");
    }

    fn write_demo_source(workspace: &Path) -> String {
        let relative = "crates/demo/src/lib.rs";
        let absolute = workspace.join(relative);
        fs::create_dir_all(absolute.parent().expect("parent")).expect("mkdirs");
        fs::write(
            &absolute,
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n\npub fn sub(a: i32, b: i32) -> i32 { a - b }\n",
        )
        .expect("write source");
        fs::write(
            workspace.join("crates/demo/Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .expect("write cargo");
        relative.to_owned()
    }

    fn parse_symbols(workspace: &Path, relative: &str) -> Vec<aether_core::Symbol> {
        let absolute = workspace.join(relative);
        let source = fs::read_to_string(&absolute).expect("read source");
        let mut extractor = aether_parse::SymbolExtractor::new().expect("extractor");
        extractor
            .extract_from_source(Language::Rust, relative, &source)
            .expect("parse symbols")
    }

    fn symbol_record(symbol: &aether_core::Symbol) -> SymbolRecord {
        SymbolRecord {
            id: symbol.id.clone(),
            file_path: symbol.file_path.clone(),
            language: symbol.language.as_str().to_owned(),
            kind: symbol.kind.as_str().to_owned(),
            qualified_name: symbol.qualified_name.clone(),
            signature_fingerprint: symbol.signature_fingerprint.clone(),
            last_seen_at: 1_700_000_000,
        }
    }

    fn seed_deep_sir(store: &SqliteStore, symbol: &aether_core::Symbol) {
        let sir_json = format!(
            "{{\"confidence\":0.95,\"dependencies\":[],\"error_modes\":[],\"inputs\":[],\"intent\":\"{} stable\",\"outputs\":[],\"side_effects\":[]}}",
            symbol.qualified_name
        );
        store
            .write_sir_blob(symbol.id.as_str(), &sir_json)
            .expect("write sir");
        store
            .upsert_sir_meta(SirMetaRecord {
                id: symbol.id.clone(),
                sir_hash: format!("hash-{}", symbol.id),
                sir_version: 1,
                provider: "mock".to_owned(),
                model: "mock".to_owned(),
                generation_pass: "deep".to_owned(),
                reasoning_trace: None,
                prompt_hash: None,
                staleness_score: None,
                updated_at: 1_700_000_001,
                sir_status: "fresh".to_owned(),
                last_error: None,
                last_attempt_at: 1_700_000_001,
            })
            .expect("upsert meta");
    }

    #[test]
    fn verify_intent_against_unchanged_snapshot_returns_pass() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        let relative = write_demo_source(temp.path());
        let store = SqliteStore::open(temp.path()).expect("open store");
        let symbols = parse_symbols(temp.path(), &relative);
        for symbol in &symbols {
            store
                .upsert_symbol(symbol_record(symbol))
                .expect("upsert symbol");
            seed_deep_sir(&store, symbol);
        }
        let snapshot = collect_intent_snapshot(
            temp.path(),
            &store,
            &RefactorScope::File {
                path: relative.clone(),
            },
            &symbols,
            &std::collections::HashSet::new(),
        )
        .expect("collect snapshot");
        let snapshot_id = snapshot.snapshot_id.clone();
        store.create_snapshot(&snapshot).expect("persist snapshot");
        let config = load_workspace_config(temp.path()).expect("config");

        let execution = execute_verify_intent_command(
            temp.path(),
            &config,
            VerifyIntentArgs {
                snapshot: snapshot_id,
                threshold: 0.85,
            },
        )
        .expect("execute verify-intent");

        assert!(execution.report.passed);
        assert_eq!(execution.exit_code, 0);
        assert!(execution.rendered.contains("Verification: PASS"));
    }
}
