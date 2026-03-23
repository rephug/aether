use std::io::Write;
use std::path::Path;

use aether_sir::SirAnnotation;
use aether_store::{SirStateStore, SqliteStore};
use anyhow::{Context, Result};

use crate::batch::hash::{compute_source_hash_segment, decompose_prompt_hash};
use crate::cli::SirDiffArgs;
use crate::sir_agent_support::{format_relative_age, load_fresh_symbol_source, resolve_symbol};

pub fn run_sir_diff_command(workspace: &Path, args: SirDiffArgs) -> Result<()> {
    let rendered = execute_sir_diff_command(workspace, args)?;
    let mut out = std::io::stdout();
    out.write_all(rendered.as_bytes())
        .context("failed to write sir-diff output")?;
    if !rendered.ends_with('\n') {
        writeln!(&mut out).context("failed to write trailing newline")?;
    }
    Ok(())
}

fn execute_sir_diff_command(workspace: &Path, args: SirDiffArgs) -> Result<String> {
    let store = SqliteStore::open(workspace).context("failed to open local store")?;
    let record = resolve_symbol(&store, args.selector.as_str())?;
    let meta = store
        .get_sir_meta(record.id.as_str())
        .with_context(|| format!("failed to read SIR metadata for {}", record.id))?;
    let blob = store
        .read_sir_blob(record.id.as_str())
        .with_context(|| format!("failed to read SIR blob for {}", record.id))?;

    let Some(meta) = meta else {
        return Ok(format!(
            "Symbol: {}\nNo SIR recorded for this symbol.\nRecommendation: run `aetherd sir-inject` or wait for watcher re-index.\n",
            record.qualified_name
        ));
    };
    if let Some(blob) = blob.as_deref() {
        let _ = serde_json::from_str::<SirAnnotation>(blob)
            .with_context(|| format!("failed to parse SIR JSON for {}", record.id))?;
    } else {
        return Ok(format!(
            "Symbol: {}\nSIR metadata exists but SIR content is missing (partial state).\n\
             Recommendation: run `aetherd sir-inject` or wait for watcher re-index.\n",
            record.qualified_name
        ));
    }

    let fresh = load_fresh_symbol_source(workspace, &record)?;
    let signature_changed = fresh.symbol.signature_fingerprint != record.signature_fingerprint;

    let (body_changed, body_detail) = match meta
        .prompt_hash
        .as_deref()
        .and_then(|hash| decompose_prompt_hash(hash).0)
    {
        Some(stored_source_hash) => {
            let current_source_hash = compute_source_hash_segment(fresh.symbol_source.as_str());
            let changed = stored_source_hash != current_source_hash;
            (
                Some(changed),
                format!(
                    "stored {} vs current {}",
                    stored_source_hash, current_source_hash
                ),
            )
        }
        None => (None, "unknown (no prompt hash recorded)".to_owned()),
    };

    let mut out = String::new();
    out.push_str(&format!("Symbol: {}\n", record.qualified_name));
    out.push_str(&format!(
        "SIR generated: {} ({} pass, {})\n",
        format_relative_age(meta.updated_at),
        meta.generation_pass,
        meta.model
    ));
    out.push_str(&format!(
        "Staleness score: {}\n\n",
        meta.staleness_score
            .map(|value| format!("{value:.2}"))
            .unwrap_or_else(|| "n/a".to_owned())
    ));

    let mut findings = Vec::new();
    if signature_changed {
        findings.push(
            "[SIGNATURE] Signature fingerprint changed (function signature modified)".to_owned(),
        );
    }
    match body_changed {
        Some(true) => findings.push(format!("[BODY] Source hash changed ({body_detail})")),
        Some(false) => {}
        None => findings.push(format!("[BODY] {body_detail}")),
    }

    if findings.is_empty() {
        out.push_str("SIR appears current. No structural drift detected.\n");
    } else {
        out.push_str("Changes detected:\n");
        for finding in findings {
            out.push_str("  ");
            out.push_str(&finding);
            out.push('\n');
        }
        out.push_str(
            "\nRecommendation: SIR is stale. Run `aetherd sir-inject` or wait for watcher re-index.\n",
        );
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use aether_core::Language;
    use aether_store::{SirStateStore, SqliteStore, SymbolCatalogStore, SymbolRecord};
    use tempfile::tempdir;

    use super::execute_sir_diff_command;
    use crate::batch::hash::compute_prompt_hash;
    use crate::cli::SirDiffArgs;

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

    fn seed_workspace(
        source: &str,
        prompt_hash: Option<String>,
    ) -> (tempfile::TempDir, SymbolRecord) {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        fs::create_dir_all(temp.path().join("src")).expect("create src");
        fs::write(temp.path().join("src/lib.rs"), source).expect("write source");

        let mut extractor = aether_parse::SymbolExtractor::new().expect("extractor");
        let symbols = extractor
            .extract_from_source(Language::Rust, "src/lib.rs", source)
            .expect("parse");
        let symbol = symbols.first().expect("symbol");
        let record = SymbolRecord {
            id: symbol.id.clone(),
            file_path: symbol.file_path.clone(),
            language: symbol.language.as_str().to_owned(),
            kind: symbol.kind.as_str().to_owned(),
            qualified_name: symbol.qualified_name.clone(),
            signature_fingerprint: symbol.signature_fingerprint.clone(),
            last_seen_at: 1_700_000_000,
        };
        let store = SqliteStore::open(temp.path()).expect("open store");
        store.upsert_symbol(record.clone()).expect("upsert symbol");
        store
            .write_sir_blob(
                record.id.as_str(),
                r#"{"confidence":0.4,"dependencies":[],"error_modes":[],"inputs":[],"intent":"alpha intent","outputs":[],"side_effects":[]}"#,
            )
            .expect("write sir");
        store
            .upsert_sir_meta(aether_store::SirMetaRecord {
                id: record.id.clone(),
                sir_hash: "hash".to_owned(),
                sir_version: 1,
                provider: "mock".to_owned(),
                model: "mock".to_owned(),
                generation_pass: "scan".to_owned(),
                reasoning_trace: None,
                prompt_hash,
                staleness_score: Some(0.2),
                updated_at: 1_700_000_001,
                sir_status: "fresh".to_owned(),
                last_error: None,
                last_attempt_at: 1_700_000_001,
            })
            .expect("upsert meta");
        (temp, record)
    }

    #[test]
    fn diff_reports_current_when_signature_and_body_match() {
        let source = "pub fn alpha() -> i32 { 1 }\n";
        let prompt_hash = Some(compute_prompt_hash(
            source.trim_end(),
            &[],
            "manual:inject:0",
        ));
        let (temp, record) = seed_workspace(source, prompt_hash);

        let rendered = execute_sir_diff_command(
            temp.path(),
            SirDiffArgs {
                selector: record.id.clone(),
            },
        )
        .expect("diff");

        assert!(rendered.contains("No structural drift detected"));
    }

    #[test]
    fn diff_reports_body_change_from_prompt_hash_source_segment() {
        let source = "pub fn alpha() -> i32 { 1 }\n";
        let prompt_hash = Some(compute_prompt_hash(
            source.trim_end(),
            &[],
            "manual:inject:0",
        ));
        let (temp, record) = seed_workspace(source, prompt_hash);
        fs::write(
            temp.path().join("src/lib.rs"),
            "pub fn alpha() -> i32 { 2 }\n",
        )
        .expect("rewrite source");

        let rendered = execute_sir_diff_command(
            temp.path(),
            SirDiffArgs {
                selector: record.id.clone(),
            },
        )
        .expect("diff");

        assert!(rendered.contains("[BODY]"));
        assert!(rendered.contains("Source hash changed"));
    }

    #[test]
    fn diff_reports_missing_prompt_hash_as_unknown() {
        let (temp, record) = seed_workspace("pub fn alpha() -> i32 { 1 }\n", None);

        let rendered = execute_sir_diff_command(
            temp.path(),
            SirDiffArgs {
                selector: record.id.clone(),
            },
        )
        .expect("diff");

        assert!(rendered.contains("unknown (no prompt hash recorded)"));
    }

    #[test]
    fn diff_reports_partial_state_when_meta_exists_but_blob_is_missing() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        fs::create_dir_all(temp.path().join("src")).expect("create src");
        let source = "pub fn alpha() -> i32 { 1 }\n";
        fs::write(temp.path().join("src/lib.rs"), source).expect("write source");

        let mut extractor = aether_parse::SymbolExtractor::new().expect("extractor");
        let symbols = extractor
            .extract_from_source(Language::Rust, "src/lib.rs", source)
            .expect("parse");
        let symbol = symbols.first().expect("symbol");
        let record = SymbolRecord {
            id: symbol.id.clone(),
            file_path: symbol.file_path.clone(),
            language: symbol.language.as_str().to_owned(),
            kind: symbol.kind.as_str().to_owned(),
            qualified_name: symbol.qualified_name.clone(),
            signature_fingerprint: symbol.signature_fingerprint.clone(),
            last_seen_at: 1_700_000_000,
        };
        let store = SqliteStore::open(temp.path()).expect("open store");
        store.upsert_symbol(record.clone()).expect("upsert symbol");
        // Write meta but NO sir blob — simulates partial/corrupted state
        store
            .upsert_sir_meta(aether_store::SirMetaRecord {
                id: record.id.clone(),
                sir_hash: "hash".to_owned(),
                sir_version: 1,
                provider: "mock".to_owned(),
                model: "mock".to_owned(),
                generation_pass: "scan".to_owned(),
                reasoning_trace: None,
                prompt_hash: None,
                staleness_score: None,
                updated_at: 1_700_000_001,
                sir_status: "fresh".to_owned(),
                last_error: None,
                last_attempt_at: 1_700_000_001,
            })
            .expect("upsert meta");

        let rendered = execute_sir_diff_command(
            temp.path(),
            SirDiffArgs {
                selector: record.id.clone(),
            },
        )
        .expect("diff");

        assert!(
            rendered.contains("SIR content is missing"),
            "should report partial state, got: {rendered}"
        );
        assert!(rendered.contains("sir-inject"));
    }
}
