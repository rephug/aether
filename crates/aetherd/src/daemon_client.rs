use std::path::Path;
use std::time::Duration;

use aether_analysis::{CommunitiesResult, CommunityEntry};
use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;

const COMMUNITIES_SCHEMA_VERSION: &str = "1.0";

#[derive(Debug, Deserialize)]
struct DaemonStatusResponse {
    pid: u32,
    port: u16,
    workspace: String,
    uptime_seconds: u64,
}

#[derive(Debug, Deserialize)]
struct ApiEnvelope<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct ArchitectureData {
    symbols: Vec<ArchitectureSymbol>,
}

#[derive(Debug, Deserialize)]
struct ArchitectureSymbol {
    symbol_id: String,
    qualified_name: String,
    file_path: String,
    community_id: i64,
}

pub fn fetch_communities(port: u16, workspace: &Path) -> Result<CommunitiesResult> {
    let status: DaemonStatusResponse = get_json(port, "/api/v1/daemon-status")
        .context("failed to fetch daemon status from running daemon")?;
    if status.pid == 0 {
        bail!("daemon status returned invalid pid 0");
    }
    if status.port != port {
        bail!(
            "daemon status reported port {} but the CLI is configured for {}",
            status.port,
            port
        );
    }
    ensure_same_workspace(workspace, status.workspace.as_str())?;
    let _ = status.uptime_seconds;

    let response: ApiEnvelope<ArchitectureData> =
        get_json(port, "/api/v1/architecture?granularity=symbol")
            .context("failed to fetch communities from daemon architecture endpoint")?;

    let mut communities = response
        .data
        .symbols
        .into_iter()
        .map(|symbol| CommunityEntry {
            symbol_id: symbol.symbol_id,
            symbol_name: symbol.qualified_name,
            file_path: symbol.file_path,
            community_id: symbol.community_id,
        })
        .collect::<Vec<_>>();

    communities.sort_by(|left, right| {
        left.community_id
            .cmp(&right.community_id)
            .then_with(|| left.file_path.cmp(&right.file_path))
            .then_with(|| left.symbol_name.cmp(&right.symbol_name))
    });

    Ok(CommunitiesResult {
        schema_version: COMMUNITIES_SCHEMA_VERSION.to_owned(),
        result_count: communities.len() as u32,
        communities,
    })
}

fn get_json<T: DeserializeOwned>(port: u16, path: &str) -> Result<T> {
    let agent_config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(2)))
        .build();
    let agent: ureq::Agent = agent_config.into();
    let url = format!("http://127.0.0.1:{port}{path}");

    let mut response = agent
        .get(&url)
        .call()
        .map_err(|err| anyhow!("GET {url} failed: {err}"))?;
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|err| anyhow!("failed to read {url} response body: {err}"))?;

    serde_json::from_str(&body)
        .map_err(|err| anyhow!("failed to parse {url} response as JSON: {err}"))
}

fn ensure_same_workspace(workspace: &Path, remote_workspace: &str) -> Result<()> {
    let local = workspace
        .canonicalize()
        .with_context(|| format!("failed to canonicalize workspace {}", workspace.display()))?;
    let remote = Path::new(remote_workspace)
        .canonicalize()
        .with_context(|| format!("failed to canonicalize daemon workspace {remote_workspace}"))?;

    if local != remote {
        bail!(
            "daemon is serving a different workspace: local={} remote={}",
            local.display(),
            remote.display()
        );
    }

    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::net::TcpListener;
    use std::thread;

    use anyhow::Result;
    use axum::extract::Query;
    use axum::routing::get;
    use axum::{Json, Router};
    use serde_json::json;
    use tempfile::TempDir;
    use tokio::sync::oneshot;

    use super::fetch_communities;

    pub(crate) struct TestServer {
        port: u16,
        shutdown: Option<oneshot::Sender<()>>,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl TestServer {
        pub(crate) fn port(&self) -> u16 {
            self.port
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            if let Some(shutdown) = self.shutdown.take() {
                let _ = shutdown.send(());
            }
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    #[test]
    fn fetch_communities_maps_and_sorts_architecture_symbols() -> Result<()> {
        let workspace = TempDir::new()?;
        let server = spawn_test_server_for_integration(
            workspace.path().display().to_string(),
            json!([
                {
                    "symbol_id": "sym-c",
                    "qualified_name": "crate::Gamma",
                    "file_path": "src/z.rs",
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
                },
                {
                    "symbol_id": "sym-b",
                    "qualified_name": "crate::Beta",
                    "file_path": "src/b.rs",
                    "directory": "src",
                    "community_id": 1,
                    "misplaced": true
                }
            ]),
        )?;

        let result = fetch_communities(server.port, workspace.path())?;
        let ids = result
            .communities
            .iter()
            .map(|entry| entry.symbol_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(result.schema_version, "1.0");
        assert_eq!(result.result_count, 3);
        assert_eq!(ids, vec!["sym-a", "sym-b", "sym-c"]);

        Ok(())
    }

    #[test]
    fn fetch_communities_rejects_mismatched_workspace() -> Result<()> {
        let workspace = TempDir::new()?;
        let other_workspace = TempDir::new()?;
        let server = spawn_test_server_for_integration(
            other_workspace.path().display().to_string(),
            json!([]),
        )?;

        let err = fetch_communities(server.port, workspace.path())
            .expect_err("mismatched daemon workspace should fail");

        assert!(
            err.to_string()
                .contains("daemon is serving a different workspace")
        );

        Ok(())
    }

    pub(crate) fn spawn_test_server_for_integration(
        workspace: String,
        symbols: serde_json::Value,
    ) -> Result<TestServer> {
        let std_listener = TcpListener::bind("127.0.0.1:0")?;
        let port = std_listener.local_addr()?.port();
        std_listener.set_nonblocking(true)?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let handle = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test server runtime");

            runtime.block_on(async move {
                let daemon_workspace = workspace.clone();
                let architecture_body = json!({
                    "data": {
                        "granularity": "symbol",
                        "not_computed": false,
                        "community_count": 0,
                        "misplaced_count": 0,
                        "communities": [],
                        "symbols": symbols
                    },
                    "meta": {
                        "generated_at": "2026-03-20T00:00:00Z",
                        "stale": false,
                        "index_age_seconds": 0
                    }
                });
                let listener = tokio::net::TcpListener::from_std(std_listener)
                    .expect("test listener should convert to tokio listener");

                let app = Router::new()
                    .route(
                        "/api/v1/daemon-status",
                        get({
                            let daemon_workspace = daemon_workspace.clone();
                            move || {
                                let daemon_workspace = daemon_workspace.clone();
                                async move {
                                    Json(json!({
                                        "daemon": true,
                                        "pid": 4242,
                                        "port": port,
                                        "workspace": daemon_workspace,
                                        "uptime_seconds": 12
                                    }))
                                }
                            }
                        }),
                    )
                    .route(
                        "/api/v1/architecture",
                        get({
                            let architecture_body = architecture_body.clone();
                            move |Query(_query): Query<
                                std::collections::HashMap<String, String>,
                            >| {
                                let architecture_body = architecture_body.clone();
                                async move { Json(architecture_body) }
                            }
                        }),
                    );

                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = shutdown_rx.await;
                    })
                    .await
                    .expect("test server should run");
            });
        });

        Ok(TestServer {
            port,
            shutdown: Some(shutdown_tx),
            handle: Some(handle),
        })
    }
}
