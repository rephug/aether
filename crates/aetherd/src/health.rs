use std::io::Write;
use std::path::Path;

use aether_analysis::{HealthAnalyzer, HealthInclude, HealthReportRequest};
use anyhow::{Context, Result};

use crate::cli::{HealthArgs, HealthFilter};

pub fn run_health_command(workspace: &Path, args: HealthArgs) -> Result<()> {
    let analyzer =
        HealthAnalyzer::new(workspace).context("failed to initialize health analyzer")?;
    let include = args.filter.map(|filter| match filter {
        HealthFilter::Critical => vec![HealthInclude::CriticalSymbols],
        HealthFilter::Cycles => vec![HealthInclude::Cycles],
        HealthFilter::Orphans => vec![HealthInclude::Orphans],
        HealthFilter::Bottlenecks => vec![HealthInclude::Bottlenecks],
        HealthFilter::RiskHotspots => vec![HealthInclude::RiskHotspots],
    });

    let report = analyzer
        .report(HealthReportRequest {
            include,
            limit: Some(args.limit.clamp(1, 200)),
            min_risk: Some(args.min_risk.clamp(0.0, 1.0)),
        })
        .context("health report failed")?;
    let value = serde_json::to_value(report).context("failed to serialize health report")?;
    write_json_to_stdout(&value)
}

fn write_json_to_stdout(value: &serde_json::Value) -> Result<()> {
    let mut out = std::io::stdout();
    serde_json::to_writer_pretty(&mut out, value).context("failed to serialize JSON output")?;
    writeln!(&mut out).context("failed to write trailing newline")?;
    Ok(())
}
