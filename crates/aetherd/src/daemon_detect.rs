use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

use aether_config::AetherConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonSource {
    HttpApi,
    LockFile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonInfo {
    pub port: u16,
    pub pid: u32,
    pub workspace: String,
    pub uptime_seconds: u64,
    pub source: DaemonSource,
}

impl DaemonInfo {
    pub fn has_http_api(&self) -> bool {
        matches!(self.source, DaemonSource::HttpApi)
    }
}

/// Check if a daemon is running for the given workspace by probing the configured
/// dashboard port. Returns `Some(DaemonInfo)` if either a same-workspace daemon
/// responds or another process holds the graph lock, `None` otherwise.
pub fn detect_running_daemon(config: &AetherConfig, workspace: &Path) -> Option<DaemonInfo> {
    let port = config.dashboard.port;
    let addr = format!("127.0.0.1:{port}");

    // Primary: HTTP probe (works when dashboard feature is enabled)
    if let Some(info) = probe_daemon_http(&addr, port, workspace) {
        return Some(info);
    }

    // Fallback: check if SurrealKV LOCK file is held by another process
    probe_lock_file(workspace, port)
}

/// Probe the daemon's HTTP status endpoint.
fn probe_daemon_http(addr: &str, port: u16, workspace: &Path) -> Option<DaemonInfo> {
    // TCP connect with 500ms timeout — fast fail if nothing is listening
    TcpStream::connect_timeout(&addr.parse().ok()?, Duration::from_millis(500)).ok()?;

    // HTTP GET /api/v1/daemon-status with 2s global timeout
    let agent_config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(2)))
        .build();
    let agent: ureq::Agent = agent_config.into();

    let url = format!("http://{addr}/api/v1/daemon-status");
    let mut response = agent.get(&url).call().ok()?;
    let body_str = response.body_mut().read_to_string().ok()?;
    let body: serde_json::Value = serde_json::from_str(&body_str).ok()?;

    let info = DaemonInfo {
        port,
        pid: body.get("pid")?.as_u64()? as u32,
        workspace: body.get("workspace")?.as_str()?.to_owned(),
        uptime_seconds: body.get("uptime_seconds")?.as_u64()?,
        source: DaemonSource::HttpApi,
    };

    // Only match if the daemon is serving the same workspace
    let local = workspace.canonicalize().ok()?;
    let remote = Path::new(&info.workspace).canonicalize().ok()?;
    if local != remote {
        return None;
    }

    Some(info)
}

/// Check if the SurrealKV LOCK file is held by another process.
fn probe_lock_file(workspace: &Path, port: u16) -> Option<DaemonInfo> {
    use fs2::FileExt;

    let lock_path = workspace.join(".aether").join("graph").join("LOCK");
    let file = std::fs::File::open(&lock_path).ok()?;

    // If we CAN get an exclusive lock, nobody else holds it — release and return None
    if file.try_lock_exclusive().is_ok() {
        file.unlock().ok();
        return None;
    }

    // Lock held by another process — pid/uptime unknown via this path
    Some(DaemonInfo {
        port,
        pid: 0,
        workspace: workspace.display().to_string(),
        uptime_seconds: 0,
        source: DaemonSource::LockFile,
    })
}

/// Print a user-friendly daemon-detected message and exit with code 1.
pub fn exit_daemon_detected(daemon: &DaemonInfo, command_name: &str) -> ! {
    eprintln!();
    if daemon.has_http_api() {
        eprintln!(
            "  The AETHER daemon is running (PID {}, port {})",
            daemon.pid, daemon.port
        );
    } else {
        eprintln!(
            "  Another process holds the graph database lock (port {})",
            daemon.port
        );
    }
    eprintln!("  The graph database can only be accessed by one process at a time.");
    eprintln!();
    eprintln!("  The `{command_name}` command requires graph access. Options:");
    eprintln!();
    if daemon.has_http_api() {
        eprintln!(
            "    \u{2022} Use the dashboard:  http://127.0.0.1:{}/dashboard/",
            daemon.port
        );
        eprintln!("    \u{2022} Stop the daemon:    pkill -f aetherd");
    } else {
        eprintln!("    \u{2022} If no daemon is running, remove the stale lock:");
        eprintln!("      rm -f .aether/graph/LOCK");
        eprintln!("    \u{2022} If another process is using the graph, stop that process first");
    }
    eprintln!("    \u{2022} Use MCP tools for programmatic access");
    eprintln!();
    std::process::exit(1);
}

/// Print a warning that graph data is unavailable due to daemon lock, then continue.
pub fn warn_daemon_detected(daemon: &DaemonInfo, command_name: &str) {
    if daemon.has_http_api() {
        eprintln!(
            "  Warning: AETHER daemon is running (PID {}, port {}). \
             Graph data unavailable for `{command_name}` \u{2014} results may be incomplete. \
             Use the dashboard at http://127.0.0.1:{}/dashboard/ for full data.",
            daemon.pid, daemon.port, daemon.port
        );
    } else {
        eprintln!(
            "  Warning: another process holds the graph database lock. \
             Graph data unavailable for `{command_name}` \u{2014} results may be incomplete. \
             If no daemon is running, remove the stale lock with `rm -f .aether/graph/LOCK`.",
        );
    }
}
