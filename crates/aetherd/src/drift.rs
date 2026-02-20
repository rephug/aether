use std::io::Write;
use std::path::Path;

use aether_analysis::{
    AcknowledgeDriftRequest, CommunitiesRequest, DriftAnalyzer, DriftReportRequest,
};
use anyhow::{Context, Result};

use crate::cli::{CommunitiesArgs, CommunitiesFormat, DriftAckArgs, DriftReportArgs};

pub fn run_drift_report_command(workspace: &Path, args: DriftReportArgs) -> Result<()> {
    let analyzer = DriftAnalyzer::new(workspace).context("failed to initialize drift analyzer")?;
    let report = analyzer
        .report(DriftReportRequest {
            window: Some(args.window),
            include: None,
            min_drift_magnitude: Some(args.min_drift.clamp(0.0, 1.0)),
            include_acknowledged: Some(args.include_acknowledged),
        })
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
    let analyzer = DriftAnalyzer::new(workspace).context("failed to initialize drift analyzer")?;
    let result = analyzer
        .communities(CommunitiesRequest {
            format: Some(args.format.as_str().to_owned()),
        })
        .context("communities query failed")?;

    match args.format {
        CommunitiesFormat::Json => {
            let value =
                serde_json::to_value(result).context("failed to serialize communities output")?;
            write_json_to_stdout(&value)
        }
        CommunitiesFormat::Table => {
            let mut out = std::io::stdout();
            writeln!(&mut out, "community_id\tsymbol_id\tsymbol_name\tfile_path")
                .context("write header")?;
            for entry in result.communities {
                writeln!(
                    &mut out,
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
    serde_json::to_writer_pretty(&mut out, value).context("failed to serialize JSON output")?;
    writeln!(&mut out).context("failed to write trailing newline")?;
    Ok(())
}
