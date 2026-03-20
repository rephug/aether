use std::io::Write;
use std::path::Path;

use aether_analysis::{
    AcknowledgeDriftRequest, CommunitiesRequest, CommunitiesResult, DriftAnalyzer,
    DriftReportRequest,
};
use aether_config::{AetherConfig, GraphBackend, load_workspace_config};
use aether_store::open_surreal_graph_store_readonly;
use anyhow::{Context, Result};

use crate::cli::{CommunitiesArgs, CommunitiesFormat, DriftAckArgs, DriftReportArgs};

pub fn run_drift_report_command(workspace: &Path, args: DriftReportArgs) -> Result<()> {
    let analyzer = DriftAnalyzer::new(workspace).context("failed to initialize drift analyzer")?;
    let request = DriftReportRequest {
        window: Some(args.window),
        include: None,
        min_drift_magnitude: Some(args.min_drift.clamp(0.0, 1.0)),
        include_acknowledged: Some(args.include_acknowledged),
    };
    let config = load_workspace_config(workspace).context("failed to load workspace config")?;
    if matches!(
        config.storage.graph_backend,
        GraphBackend::Surreal | GraphBackend::Cozo
    ) && let Some(daemon) = crate::daemon_detect::detect_running_daemon(&config, workspace)
    {
        crate::daemon_detect::exit_daemon_detected(&daemon, "drift-report");
    }
    let report = match config.storage.graph_backend {
        GraphBackend::Surreal => {
            let graph = open_surreal_graph_store_readonly(workspace)
                .context("failed to open configured surreal graph store")?;
            analyzer.report_with_graph(&graph, request)
        }
        _ => analyzer.report(request),
    }
    .context("drift report failed")?;
    let value = serde_json::to_value(report).context("failed to serialize drift report")?;
    write_json_to_stdout(&value)
}

pub fn run_drift_ack_command(workspace: &Path, args: DriftAckArgs) -> Result<()> {
    let analyzer = DriftAnalyzer::new(workspace).context("failed to initialize drift analyzer")?;
    let result = analyzer
        .acknowledge_drift(AcknowledgeDriftRequest {
            result_ids: vec![args.result_id],
            note: args.note,
        })
        .context("drift acknowledgement failed")?;
    let value =
        serde_json::to_value(result).context("failed to serialize drift acknowledgement output")?;
    write_json_to_stdout(&value)
}

pub fn run_communities_command(workspace: &Path, args: CommunitiesArgs) -> Result<()> {
    let config = load_workspace_config(workspace).context("failed to load workspace config")?;
    let output = load_communities_result(workspace, &config, &args)?;
    if let Some(port) = output.daemon_port {
        eprintln!("{}", daemon_notice_message(port));
    }

    let mut out = std::io::stdout();
    write_communities_output(&mut out, &output.result, args.format)
}

#[derive(Debug)]
struct CommunitiesCommandOutput {
    daemon_port: Option<u16>,
    result: CommunitiesResult,
}

fn load_communities_result(
    workspace: &Path,
    config: &AetherConfig,
    args: &CommunitiesArgs,
) -> Result<CommunitiesCommandOutput> {
    if matches!(
        config.storage.graph_backend,
        GraphBackend::Surreal | GraphBackend::Cozo
    ) && let Some(daemon) = crate::daemon_detect::detect_running_daemon(config, workspace)
    {
        if daemon.has_http_api() {
            let result = crate::daemon_client::fetch_communities(daemon.port, workspace)
                .context("failed to fetch communities from running daemon")?;
            return Ok(CommunitiesCommandOutput {
                daemon_port: Some(daemon.port),
                result,
            });
        }

        crate::daemon_detect::exit_daemon_detected(&daemon, "communities");
    }

    let analyzer = DriftAnalyzer::new(workspace).context("failed to initialize drift analyzer")?;
    let graph = open_surreal_graph_store_readonly(workspace)
        .context("failed to open configured surreal graph store")?;
    let result = analyzer
        .communities_with_graph(
            &graph,
            CommunitiesRequest {
                format: Some(args.format.as_str().to_owned()),
            },
        )
        .context("communities query failed")?;

    Ok(CommunitiesCommandOutput {
        daemon_port: None,
        result,
    })
}

fn daemon_notice_message(port: u16) -> String {
    format!("Info: Results fetched from running daemon (port {port})")
}

fn write_communities_output<W: Write>(
    out: &mut W,
    result: &CommunitiesResult,
    format: CommunitiesFormat,
) -> Result<()> {
    match format {
        CommunitiesFormat::Json => {
            let value =
                serde_json::to_value(result).context("failed to serialize communities output")?;
            write_json(out, &value)
        }
        CommunitiesFormat::Table => {
            writeln!(out, "community_id\tsymbol_id\tsymbol_name\tfile_path")
                .context("write header")?;
            for entry in &result.communities {
                writeln!(
                    out,
                    "{}\t{}\t{}\t{}",
                    entry.community_id, entry.symbol_id, entry.symbol_name, entry.file_path
                )
                .context("write row")?;
            }
            Ok(())
        }
    }
}

fn write_json_to_stdout(value: &serde_json::Value) -> Result<()> {
    let mut out = std::io::stdout();
    write_json(&mut out, value)
}

fn write_json<W: Write>(out: &mut W, value: &serde_json::Value) -> Result<()> {
    serde_json::to_writer_pretty(&mut *out, value).context("failed to serialize JSON output")?;
    writeln!(out).context("failed to write trailing newline")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use tempfile::TempDir;

    use super::{daemon_notice_message, load_communities_result, write_communities_output};
    use crate::cli::{CommunitiesArgs, CommunitiesFormat};
    use crate::daemon_client;

    fn sample_result() -> aether_analysis::CommunitiesResult {
        aether_analysis::CommunitiesResult {
            schema_version: "1.0".to_owned(),
            result_count: 2,
            communities: vec![
                aether_analysis::CommunityEntry {
                    community_id: 1,
                    symbol_id: "sym-a".to_owned(),
                    symbol_name: "crate::Alpha".to_owned(),
                    file_path: "src/a.rs".to_owned(),
                },
                aether_analysis::CommunityEntry {
                    community_id: 2,
                    symbol_id: "sym-b".to_owned(),
                    symbol_name: "crate::Beta".to_owned(),
                    file_path: "src/b.rs".to_owned(),
                },
            ],
        }
    }

    #[test]
    fn write_communities_output_renders_table() -> Result<()> {
        let mut out = Vec::new();
        write_communities_output(&mut out, &sample_result(), CommunitiesFormat::Table)?;

        assert_eq!(
            String::from_utf8(out)?,
            "community_id\tsymbol_id\tsymbol_name\tfile_path\n\
1\tsym-a\tcrate::Alpha\tsrc/a.rs\n\
2\tsym-b\tcrate::Beta\tsrc/b.rs\n"
        );

        Ok(())
    }

    #[test]
    fn write_communities_output_renders_json() -> Result<()> {
        let mut out = Vec::new();
        write_communities_output(&mut out, &sample_result(), CommunitiesFormat::Json)?;

        let rendered = String::from_utf8(out)?;
        let reparsed: serde_json::Value = serde_json::from_str(&rendered)?;
        assert_eq!(reparsed["schema_version"], "1.0");
        assert_eq!(reparsed["result_count"], 2);

        Ok(())
    }

    #[test]
    fn load_communities_result_uses_daemon_when_available() -> Result<()> {
        let workspace = TempDir::new()?;
        let server = daemon_client::tests::spawn_test_server_for_integration(
            workspace.path().display().to_string(),
            serde_json::json!([
                {
                    "symbol_id": "sym-b",
                    "qualified_name": "crate::Beta",
                    "file_path": "src/b.rs",
                    "directory": "src",
                    "community_id": 2,
                    "misplaced": false
                },
                {
                    "symbol_id": "sym-a",
                    "qualified_name": "crate::Alpha",
                    "file_path": "src/a.rs",
                    "directory": "src",
                    "community_id": 1,
                    "misplaced": false
                }
            ]),
        )?;

        let mut config = aether_config::AetherConfig::default();
        config.dashboard.port = server.port();
        let args = CommunitiesArgs {
            format: CommunitiesFormat::Table,
        };

        let output = load_communities_result(workspace.path(), &config, &args)?;
        let mut rendered = Vec::new();
        write_communities_output(&mut rendered, &output.result, args.format)?;

        assert_eq!(output.daemon_port, Some(server.port()));
        assert_eq!(
            String::from_utf8(rendered)?,
            "community_id\tsymbol_id\tsymbol_name\tfile_path\n\
1\tsym-a\tcrate::Alpha\tsrc/a.rs\n\
2\tsym-b\tcrate::Beta\tsrc/b.rs\n"
        );
        assert_eq!(
            daemon_notice_message(server.port()),
            format!(
                "Info: Results fetched from running daemon (port {})",
                server.port()
            )
        );

        Ok(())
    }

    #[test]
    fn load_communities_result_uses_local_path_when_no_daemon_exists() -> Result<()> {
        let workspace = TempDir::new()?;
        let config = aether_config::AetherConfig::default();
        let args = CommunitiesArgs {
            format: CommunitiesFormat::Table,
        };

        let err = load_communities_result(workspace.path(), &config, &args)
            .expect_err("missing daemon should fall through to local graph path");
        let rendered = format!("{err:#}");

        assert!(rendered.contains("failed"));
        assert!(!rendered.contains("failed to fetch communities from running daemon"));

        Ok(())
    }
}
