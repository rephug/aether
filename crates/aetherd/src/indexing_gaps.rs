//! Last structural snapshot's identity quarantine, separate from index freshness.
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const REPORT: &str = "indexing-blocked.json";
const MAX_REPORT_BYTES: u64 = 1024 * 1024;
const MAX_REPORTED_PATHS: usize = 128;
const MAX_SUMMARY_PATHS: usize = 4;
static RECEIPT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockedSource {
    pub path: String,
    pub reason: String,
}

#[derive(Serialize, Deserialize)]
struct Report {
    version: u32,
    snapshot_started_at: i64,
    blocked_sources: Vec<BlockedSource>,
    omitted_paths: usize,
}

pub(crate) fn record(workspace: &Path, started_at: i64, blocked: &[BlockedSource]) -> Result<()> {
    let mut paths = blocked.to_vec();
    paths.sort_by(|left, right| left.path.cmp(&right.path));
    paths.truncate(MAX_REPORTED_PATHS);
    for path in &mut paths {
        path.reason = path.reason.chars().take(512).collect();
    }
    let report = Report {
        version: 1,
        snapshot_started_at: started_at,
        omitted_paths: blocked.len().saturating_sub(paths.len()),
        blocked_sources: paths,
    };
    let directory = workspace.join(".aether");
    // The writable store has already initialized this directory. Write+rename
    // gives readers a complete receipt; it is not a global watcher ownership lock.
    let temporary = directory.join(format!(
        "{REPORT}.{}.{}.tmp",
        std::process::id(),
        RECEIPT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let bytes = serde_json::to_vec(&report)?;
    if bytes.len() as u64 > MAX_REPORT_BYTES {
        bail!("indexing quarantine receipt exceeds size limit");
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .context("create indexing quarantine receipt")?;
    file.write_all(&bytes)
        .context("write indexing quarantine receipt")?;
    drop(file);
    fs::rename(&temporary, directory.join(REPORT)).context("publish indexing quarantine receipt")
}

pub fn read_summary(workspace: &Path) -> Result<Option<String>> {
    let file = match fs::File::open(workspace.join(".aether").join(REPORT)) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("open indexing quarantine receipt"),
    };
    let mut bytes = Vec::new();
    file.take(MAX_REPORT_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("read indexing quarantine receipt")?;
    if bytes.len() as u64 > MAX_REPORT_BYTES {
        bail!("indexing quarantine receipt exceeds size limit");
    }
    let report: Report =
        serde_json::from_slice(&bytes).context("decode indexing quarantine receipt")?;
    if report.version != 1 {
        bail!(
            "unsupported indexing quarantine receipt version {}",
            report.version
        );
    }
    if report.blocked_sources.len() > MAX_REPORTED_PATHS {
        bail!("indexing quarantine receipt exceeds path limit");
    }
    let total = report
        .blocked_sources
        .len()
        .checked_add(report.omitted_paths)
        .context("indexing quarantine receipt count overflow")?;
    if total == 0 {
        return Ok(None);
    }
    let paths = report
        .blocked_sources
        .iter()
        .take(MAX_SUMMARY_PATHS)
        .map(|blocked| {
            format!(
                "{} ({})",
                abbreviated(&blocked.path, 120),
                abbreviated(&blocked.reason, 96)
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let hidden = total.saturating_sub(report.blocked_sources.len().min(MAX_SUMMARY_PATHS));
    let more = if hidden > 0 {
        format!(
            " {hidden} additional blocked paths are not expanded in this summary; consult the bounded .aether/{REPORT} receipt and indexing logs."
        )
    } else {
        String::new()
    };
    Ok(Some(format!(
        "Latest indexer observation (started {}): {} quarantined identity-collision files, {} paths omitted from this receipt. Stored rows for blocked files are retained and may be stale. Full-sync coverage remains blocked until all files can be read. This receipt is not a live freshness or completed-generation guarantee. {}{}",
        report.snapshot_started_at, total, report.omitted_paths, paths, more
    )))
}

fn abbreviated(text: &str, limit: usize) -> String {
    let mut characters = text.chars();
    let mut output = characters.by_ref().take(limit).collect::<String>();
    if characters.next().is_some() {
        output.push('…');
    }
    output
}
