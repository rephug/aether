use std::io::Write;
use std::path::Path;

use aether_analysis::{BlastRadiusRequest, CouplingAnalyzer, MineCouplingRequest};
use anyhow::{Context, Result};
use serde_json::json;

use crate::cli::{BlastRadiusArgs, CouplingReportArgs, MineCouplingArgs};

pub fn run_mine_coupling_command(workspace: &Path, args: MineCouplingArgs) -> Result<()> {
    let analyzer = CouplingAnalyzer::new(workspace).context("failed to initialize analyzer")?;
    let result = analyzer
        .mine(MineCouplingRequest {
            commits: args.commits,
        })
        .context("coupling mining failed")?;

    let response = serde_json::to_value(result).context("failed to serialize mining output")?;
    write_json_to_stdout(&response)
}

pub fn run_blast_radius_command(workspace: &Path, args: BlastRadiusArgs) -> Result<()> {
    let analyzer = CouplingAnalyzer::new(workspace).context("failed to initialize analyzer")?;
    let result = analyzer
        .blast_radius(BlastRadiusRequest {
            file_path: args.file,
            min_risk: args.min_risk,
            auto_mine: true,
        })
        .context("blast radius query failed")?;

    let response =
        serde_json::to_value(result).context("failed to serialize blast radius output")?;
    write_json_to_stdout(&response)
}

pub fn run_coupling_report_command(workspace: &Path, args: CouplingReportArgs) -> Result<()> {
    let analyzer = CouplingAnalyzer::new(workspace).context("failed to initialize analyzer")?;
    let edges = analyzer
        .coupling_report(args.top.clamp(1, 200))
        .context("coupling report query failed")?;

    let response = json!({
        "result_count": edges.len(),
        "edges": edges,
    });
    write_json_to_stdout(&response)
}

fn write_json_to_stdout(value: &serde_json::Value) -> Result<()> {
    let mut out = std::io::stdout();
    serde_json::to_writer_pretty(&mut out, value).context("failed to serialize JSON output")?;
    writeln!(&mut out).context("failed to write trailing newline")?;
    Ok(())
}
