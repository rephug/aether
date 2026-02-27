use std::io::Write;
use std::path::Path;

use aether_analysis::{HealthAnalyzer, HealthInclude, HealthRequest};
use anyhow::{Context, Result, anyhow};

use crate::cli::HealthArgs;

pub fn run_health_command(workspace: &Path, args: HealthArgs) -> Result<()> {
    let include = parse_health_filter(args.filter.as_deref())?;
    let analyzer =
        HealthAnalyzer::new(workspace).context("failed to initialize health analyzer")?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime for health command")?;
    let report = runtime
        .block_on(analyzer.analyze(&HealthRequest {
            include,
            limit: args.limit,
            min_risk: args.min_risk.clamp(0.0, 1.0),
        }))
        .context("health analysis failed")?;

    let value = serde_json::to_value(report).context("failed to serialize health report")?;
    write_json_to_stdout(&value)
}

fn parse_health_filter(raw: Option<&str>) -> Result<Vec<HealthInclude>> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(Vec::new());
    };

    match raw.to_ascii_lowercase().as_str() {
        "critical" => Ok(vec![HealthInclude::CriticalSymbols]),
        "cycles" => Ok(vec![HealthInclude::Cycles]),
        "orphans" => Ok(vec![HealthInclude::Orphans]),
        "bottlenecks" => Ok(vec![HealthInclude::Bottlenecks]),
        "risk-hotspots" | "risk_hotspots" => Ok(vec![HealthInclude::RiskHotspots]),
        other => Err(anyhow!(
            "invalid health filter '{other}', expected one of: critical, cycles, orphans, bottlenecks, risk-hotspots"
        )),
    }
}

fn write_json_to_stdout(value: &serde_json::Value) -> Result<()> {
    let mut out = std::io::stdout();
    serde_json::to_writer_pretty(&mut out, value).context("failed to serialize JSON output")?;
    writeln!(&mut out).context("failed to write trailing newline")?;
    Ok(())
}
