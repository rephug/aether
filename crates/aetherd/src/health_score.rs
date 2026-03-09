use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use aether_analysis::HealthAnalyzer;
use aether_config::AetherConfig;
use aether_core::{GitContext, SIR_STATUS_STALE, normalize_path};
use aether_health::history::{create_table_if_needed, read_previous_score, write_score};
use aether_health::{
    ScoreReport, SemanticFileInput, SemanticInput, compute_workspace_score,
    compute_workspace_score_filtered, compute_workspace_score_with_signals, format_json,
    format_table,
};
use aether_store::{SqliteStore, Store};
use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::cli::{HealthScoreArgs, HealthScoreOutputFormat};

#[derive(Debug)]
pub struct HealthScoreExecution {
    pub report: ScoreReport,
    pub rendered: String,
    pub exit_code: i32,
}

pub fn run_health_score_command(
    workspace: &Path,
    config: &AetherConfig,
    args: HealthScoreArgs,
) -> Result<()> {
    let execution = execute_health_score_command(workspace, config, args)?;
    let mut stdout = std::io::stdout();
    stdout
        .write_all(execution.rendered.as_bytes())
        .context("failed to write health-score output")?;
    if !execution.rendered.ends_with('\n') {
        writeln!(&mut stdout).context("failed to terminate health-score output")?;
    }

    if execution.exit_code != 0 {
        std::process::exit(execution.exit_code);
    }

    Ok(())
}

pub fn execute_health_score_command(
    workspace: &Path,
    config: &AetherConfig,
    args: HealthScoreArgs,
) -> Result<HealthScoreExecution> {
    let mut report = if args.semantic {
        let git = GitContext::open(workspace);
        let semantic = load_semantic_input(workspace)?;
        compute_workspace_score_with_signals(
            workspace,
            &config.health_score,
            &args.crate_filter,
            git.as_ref(),
            semantic.as_ref(),
        )
    } else if args.crate_filter.is_empty() {
        compute_workspace_score(workspace, &config.health_score)
    } else {
        compute_workspace_score_filtered(workspace, &config.health_score, &args.crate_filter)
    }
    .context("failed to compute health score")?;

    let history_allowed = !args.no_history && args.crate_filter.is_empty();
    if history_allowed {
        maybe_attach_history(workspace, &mut report)?;
    }

    let rendered = match args.output {
        HealthScoreOutputFormat::Table => format_table(&report),
        HealthScoreOutputFormat::Json => format_json(&report),
    };
    let exit_code = if args
        .fail_above
        .is_some_and(|threshold| report.workspace_score > threshold)
    {
        1
    } else {
        0
    };

    Ok(HealthScoreExecution {
        report,
        rendered,
        exit_code,
    })
}

fn maybe_attach_history(workspace: &Path, report: &mut ScoreReport) -> Result<()> {
    let sqlite_path = workspace.join(".aether").join("meta.sqlite");
    if !sqlite_path.exists() {
        return Ok(());
    }

    let conn = Connection::open(&sqlite_path)
        .with_context(|| format!("failed to open {}", sqlite_path.display()))?;
    create_table_if_needed(&conn).context("failed to prepare health score history table")?;
    if let Some((previous_score, _previous_json)) =
        read_previous_score(&conn).context("failed to read previous health score")?
    {
        report.previous_score = Some(previous_score);
        report.delta = Some(report.workspace_score as i32 - previous_score as i32);
    }
    write_score(&conn, report).context("failed to write health score history")?;
    Ok(())
}

fn load_semantic_input(workspace: &Path) -> Result<Option<SemanticInput>> {
    let sqlite_path = workspace.join(".aether").join("meta.sqlite");
    if !sqlite_path.exists() {
        return Ok(None);
    }

    let analyzer =
        HealthAnalyzer::new(workspace).context("failed to initialize health analyzer")?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build runtime for semantic health scoring")?;
    let centrality = runtime
        .block_on(analyzer.centrality_by_file())
        .context("failed to collect centrality by file")?;
    if centrality.files.is_empty() && !centrality.notes.is_empty() {
        return Ok(None);
    }

    let store = match SqliteStore::open_readonly(workspace) {
        Ok(store) => store,
        Err(_) => return Ok(None),
    };

    let drift_by_symbol = latest_semantic_drift_by_symbol(&store)?;
    let community_by_symbol = store
        .list_latest_community_snapshot()
        .unwrap_or_default()
        .into_iter()
        .map(|entry| (entry.symbol_id, entry.community_id))
        .collect::<HashMap<_, _>>();

    let mut files = HashMap::new();
    for entry in centrality.files {
        let path = normalize_path(entry.file.as_str());
        let symbols = store
            .list_symbols_for_file(path.as_str())
            .with_context(|| format!("failed to list symbols for {}", path))?;
        if symbols.is_empty() {
            continue;
        }

        let drifted_symbol_count = symbols
            .iter()
            .filter(|symbol| {
                drift_by_symbol
                    .get(symbol.id.as_str())
                    .is_some_and(|magnitude| *magnitude > 0.3)
            })
            .count();
        let stale_or_missing_sir_count = symbols
            .iter()
            .filter(|symbol| {
                store
                    .get_sir_meta(symbol.id.as_str())
                    .ok()
                    .flatten()
                    .is_none_or(|meta| {
                        meta.sir_status
                            .trim()
                            .eq_ignore_ascii_case(SIR_STATUS_STALE)
                    })
            })
            .count();
        let community_count = symbols
            .iter()
            .filter_map(|symbol| community_by_symbol.get(symbol.id.as_str()).copied())
            .collect::<std::collections::HashSet<_>>()
            .len();
        let has_test_coverage = symbols.iter().any(|symbol| {
            store
                .list_test_intents_for_symbol(symbol.id.as_str())
                .map(|records| !records.is_empty())
                .unwrap_or(false)
        });

        files.insert(
            path,
            SemanticFileInput {
                max_pagerank: entry.max_pagerank,
                symbol_count: symbols.len(),
                drifted_symbol_count,
                stale_or_missing_sir_count,
                community_count,
                has_test_coverage,
            },
        );
    }

    Ok(Some(SemanticInput {
        workspace_max_pagerank: centrality.workspace_max_pagerank,
        files,
    }))
}

fn latest_semantic_drift_by_symbol(store: &SqliteStore) -> Result<HashMap<String, f64>> {
    let mut drift_by_symbol = HashMap::new();
    for record in store
        .list_drift_results(true)
        .context("failed to list semantic drift results")?
    {
        if record.drift_type != "semantic" {
            continue;
        }
        let Some(magnitude) = record.drift_magnitude else {
            continue;
        };

        drift_by_symbol
            .entry(record.symbol_id)
            .or_insert((magnitude as f64).clamp(0.0, 1.0));
    }
    Ok(drift_by_symbol)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use aether_config::AetherConfig;
    use aether_core::EdgeKind;
    use aether_store::{
        CommunitySnapshotRecord, DriftResultRecord, GraphStore, ResolvedEdge, SqliteStore, Store,
        SurrealGraphStore, SymbolRecord, TestIntentRecord,
    };
    use rusqlite::Connection;
    use tempfile::tempdir;

    use super::execute_health_score_command;
    use crate::cli::{HealthScoreArgs, HealthScoreOutputFormat};

    fn write_file(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().expect("test file path parent")).expect("create parent");
        fs::write(path, content).expect("write file");
    }

    fn create_workspace() -> tempfile::TempDir {
        let temp = tempdir().expect("tempdir");
        write_file(
            &temp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/example\"]\nresolver = \"2\"\n",
        );
        write_file(
            &temp.path().join("crates/example/Cargo.toml"),
            "[package]\nname = \"example\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        temp
    }

    fn now_millis() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }

    fn write_semantic_config(workspace: &Path) {
        write_file(
            &workspace.join(".aether/config.toml"),
            r#"[storage]
mirror_sir_files = true
graph_backend = "surreal"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
        );
    }

    fn run_git(workspace: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(workspace)
            .output()
            .expect("git command");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn init_git_repo(workspace: &Path) {
        run_git(workspace, &["init"]);
        run_git(workspace, &["config", "user.name", "Aether Test"]);
        run_git(
            workspace,
            &["config", "user.email", "aether-test@example.com"],
        );
    }

    fn commit_all(workspace: &Path, message: &str) {
        run_git(workspace, &["add", "."]);
        run_git(workspace, &["commit", "-m", message]);
    }

    fn symbol(id: &str, qualified_name: &str, file_path: &str) -> SymbolRecord {
        SymbolRecord {
            id: id.to_owned(),
            file_path: file_path.to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature_fingerprint: format!("sig-{id}"),
            last_seen_at: now_millis(),
        }
    }

    fn copy_dir_all(from: &Path, to: &Path) {
        fs::create_dir_all(to).expect("create destination dir");
        for entry in fs::read_dir(from).expect("read source dir") {
            let entry = entry.expect("dir entry");
            let source_path = entry.path();
            let target_path = to.join(entry.file_name());
            if source_path.is_dir() {
                copy_dir_all(&source_path, &target_path);
            } else {
                fs::copy(&source_path, &target_path).expect("copy file");
            }
        }
    }

    fn seed_surreal_graph_snapshot(
        workspace: &Path,
        symbols: &[SymbolRecord],
        edges: &[(&str, &str)],
    ) {
        let seed_workspace = tempdir().expect("seed workspace");
        write_file(&seed_workspace.path().join("Cargo.toml"), "[workspace]\n");
        fs::create_dir_all(seed_workspace.path().join(".aether")).expect("create seed .aether");

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let graph = SurrealGraphStore::open(seed_workspace.path())
                .await
                .expect("open surreal graph");
            for symbol in symbols {
                graph
                    .upsert_symbol_node(symbol)
                    .await
                    .expect("upsert symbol node");
            }
            for (source, target) in edges {
                graph
                    .upsert_edge(&ResolvedEdge {
                        source_id: (*source).to_owned(),
                        target_id: (*target).to_owned(),
                        edge_kind: EdgeKind::Calls,
                        file_path: "crates/example/src/lib.rs".to_owned(),
                    })
                    .await
                    .expect("upsert edge");
            }
        });

        let source_graph = seed_workspace.path().join(".aether/graph");
        let target_graph = workspace.join(".aether/graph");
        if target_graph.exists() {
            fs::remove_dir_all(&target_graph).expect("remove existing graph dir");
        }
        copy_dir_all(&source_graph, &target_graph);
    }

    fn default_args() -> HealthScoreArgs {
        HealthScoreArgs {
            output: HealthScoreOutputFormat::Json,
            fail_above: None,
            no_history: false,
            crate_filter: Vec::new(),
            semantic: false,
        }
    }

    #[test]
    fn fail_above_exit_code() {
        let workspace = create_workspace();
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub fn alpha() { let _ = \"cozo\"; let _ = \"cozo\"; }\n",
        );

        let mut args = default_args();
        args.fail_above = Some(0);
        let execution =
            execute_health_score_command(workspace.path(), &AetherConfig::default(), args)
                .expect("health-score execution");

        assert_eq!(execution.exit_code, 1);
    }

    #[test]
    fn no_aether_dir_no_error() {
        let workspace = create_workspace();
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub fn alpha() {}\n",
        );

        let execution = execute_health_score_command(
            workspace.path(),
            &AetherConfig::default(),
            default_args(),
        )
        .expect("health-score execution");

        assert_eq!(execution.exit_code, 0);
        assert!(!workspace.path().join(".aether").exists());
    }

    #[test]
    fn history_written_and_delta() {
        let workspace = create_workspace();
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub fn alpha() { let _ = \"cozo\"; let _ = \"cozo\"; }\n",
        );
        fs::create_dir_all(workspace.path().join(".aether")).expect("create .aether");
        Connection::open(workspace.path().join(".aether/meta.sqlite")).expect("create sqlite");

        let first = execute_health_score_command(
            workspace.path(),
            &AetherConfig::default(),
            default_args(),
        )
        .expect("first run");
        assert!(first.report.previous_score.is_none());

        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub fn alpha() { let _ = \"cozo\"; let _ = \"cozo\"; let _ = \"cozo\"; }\n",
        );
        let second = execute_health_score_command(
            workspace.path(),
            &AetherConfig::default(),
            default_args(),
        )
        .expect("second run");

        assert!(second.report.previous_score.is_some());
        assert!(second.report.delta.is_some());
    }

    #[test]
    fn filtered_run_skips_history() {
        let workspace = create_workspace();
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub fn alpha() { let _ = \"cozo\"; let _ = \"cozo\"; }\n",
        );
        fs::create_dir_all(workspace.path().join(".aether")).expect("create .aether");
        let conn = Connection::open(workspace.path().join(".aether/meta.sqlite")).expect("sqlite");
        conn.execute(
            "CREATE TABLE IF NOT EXISTS health_score_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_at INTEGER NOT NULL,
                git_commit TEXT,
                score INTEGER NOT NULL,
                score_json TEXT NOT NULL,
                UNIQUE(git_commit)
            )",
            [],
        )
        .expect("create table");

        let mut args = default_args();
        args.crate_filter = vec!["example".to_owned()];
        execute_health_score_command(workspace.path(), &AetherConfig::default(), args)
            .expect("filtered run");

        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM health_score_history", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("row count");
        assert_eq!(row_count, 0);
    }

    #[test]
    fn semantic_flag_without_index_gracefully_falls_back() {
        let workspace = create_workspace();
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub fn alpha() {}\n",
        );

        let mut args = default_args();
        args.semantic = true;
        let execution =
            execute_health_score_command(workspace.path(), &AetherConfig::default(), args)
                .expect("semantic fallback execution");

        assert_eq!(execution.report.schema_version, 2);
        assert!(
            execution
                .report
                .crates
                .iter()
                .all(|crate_score| !crate_score.signal_availability.semantic_available)
        );
    }

    #[test]
    fn semantic_mode_populates_git_and_semantic_signals() {
        let workspace = create_workspace();
        write_semantic_config(workspace.path());
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub fn alpha() -> i32 { 1 }\npub fn beta() -> i32 { alpha() }\n",
        );
        init_git_repo(workspace.path());
        commit_all(workspace.path(), "initial");
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub fn alpha() -> i32 { 2 }\npub fn beta() -> i32 { alpha() + 1 }\n",
        );
        commit_all(workspace.path(), "update");

        let store = SqliteStore::open(workspace.path()).expect("open sqlite");
        let symbols = vec![
            symbol("sym-a", "crate::alpha", "crates/example/src/lib.rs"),
            symbol("sym-b", "crate::beta", "crates/example/src/lib.rs"),
        ];
        for symbol in &symbols {
            store.upsert_symbol(symbol.clone()).expect("upsert symbol");
        }
        store
            .upsert_drift_results(&[DriftResultRecord {
                result_id: "drift-a".to_owned(),
                symbol_id: "sym-a".to_owned(),
                file_path: "crates/example/src/lib.rs".to_owned(),
                symbol_name: "crate::alpha".to_owned(),
                drift_type: "semantic".to_owned(),
                drift_magnitude: Some(0.8),
                current_sir_hash: None,
                baseline_sir_hash: None,
                commit_range_start: Some("a".to_owned()),
                commit_range_end: Some("b".to_owned()),
                drift_summary: Some("alpha changed".to_owned()),
                detail_json: "{}".to_owned(),
                detected_at: now_millis(),
                is_acknowledged: false,
            }])
            .expect("seed drift");
        store
            .replace_community_snapshot(
                "snapshot-1",
                now_millis(),
                &[
                    CommunitySnapshotRecord {
                        snapshot_id: "snapshot-1".to_owned(),
                        symbol_id: "sym-a".to_owned(),
                        community_id: 1,
                        captured_at: now_millis(),
                    },
                    CommunitySnapshotRecord {
                        snapshot_id: "snapshot-1".to_owned(),
                        symbol_id: "sym-b".to_owned(),
                        community_id: 2,
                        captured_at: now_millis(),
                    },
                ],
            )
            .expect("seed communities");
        store
            .replace_test_intents_for_file(
                "tests/example_test.rs",
                &[TestIntentRecord {
                    intent_id: "intent-alpha".to_owned(),
                    file_path: "tests/example_test.rs".to_owned(),
                    test_name: "test_alpha".to_owned(),
                    intent_text: "covers alpha".to_owned(),
                    group_label: None,
                    language: "rust".to_owned(),
                    symbol_id: Some("sym-a".to_owned()),
                    created_at: now_millis(),
                    updated_at: now_millis(),
                }],
            )
            .expect("seed test intents");

        seed_surreal_graph_snapshot(workspace.path(), &symbols, &[("sym-a", "sym-b")]);

        let mut args = default_args();
        args.semantic = true;
        let execution =
            execute_health_score_command(workspace.path(), &AetherConfig::default(), args)
                .expect("semantic execution");

        assert_eq!(execution.report.schema_version, 2);
        let crate_score = &execution.report.crates[0];
        assert!(crate_score.signal_availability.git_available);
        assert!(crate_score.signal_availability.semantic_available);
        assert!(crate_score.git_signals.is_some());
        assert!(crate_score.semantic_signals.is_some());
        assert!(crate_score.score_breakdown.is_some());
        assert!(execution.rendered.contains("\"git_signals\""));
    }
}
