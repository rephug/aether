use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use aether_mcp::{AetherMcpServer, SharedState};
use aether_query::DynError;
use aether_query::config::{
    QueryConfig, apply_client_overrides, apply_serve_overrides, load_query_config,
};
use aether_query::server::build_router;
use clap::{Args, Parser, Subcommand};
use tokio::signal;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "aether-query")]
#[command(about = "Read-only MCP-over-HTTP query server for a live AETHER index")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve(ServeArgs),
    Status(ClientArgs),
    Info(ClientArgs),
}

#[derive(Debug, Args)]
struct ServeArgs {
    #[arg(
        long,
        help = "Workspace root containing .aether/ (not the .aether dir itself)"
    )]
    index_path: Option<PathBuf>,
    #[arg(long, help = "Bind address for HTTP server, e.g. 127.0.0.1:9731")]
    bind: Option<String>,
    #[arg(long, help = "Bearer token required on all routes when set")]
    auth_token: Option<String>,
    #[arg(long)]
    verbose: bool,
}

#[derive(Debug, Args)]
struct ClientArgs {
    #[arg(long, help = "Server bind address, e.g. 127.0.0.1:9731")]
    bind: Option<String>,
    #[arg(long, help = "Bearer token for Authorization header")]
    auth_token: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), DynError> {
    init_tracing();
    let cli = Cli::parse();

    match cli.command {
        Command::Serve(args) => run_serve(args).await,
        Command::Status(args) => run_client_command("/health", args).await,
        Command::Info(args) => run_client_command("/info", args).await,
    }
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();
}

async fn run_serve(args: ServeArgs) -> Result<(), DynError> {
    let mut config = load_query_config()?;
    apply_serve_overrides(&mut config, args.index_path, args.bind, args.auth_token);

    let state = Arc::new(SharedState::open_readonly_async(&config.query.index_path).await?);
    let mcp_server = AetherMcpServer::from_state(state.clone(), args.verbose);
    let app = build_router(state, mcp_server, config.clone());

    let listener = tokio::net::TcpListener::bind(&config.query.bind_address).await?;
    tracing::info!(
        bind = %config.query.bind_address,
        workspace = %config.query.index_path.display(),
        "aether-query listening"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn run_client_command(path: &str, args: ClientArgs) -> Result<(), DynError> {
    let mut config = load_query_config()?;
    apply_client_overrides(&mut config, args.bind, args.auth_token);
    print_endpoint_json(&config, path).await
}

async fn print_endpoint_json(config: &QueryConfig, path: &str) -> Result<(), DynError> {
    let url = format!("http://{}{}", config.query.bind_address, path);
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let mut request = client.get(url.clone());
    if !config.query.auth_token.is_empty() {
        request = request.bearer_auth(&config.query.auth_token);
    }

    let response = request.send().await?;
    let status = response.status();
    let body = response.text().await?;

    if !status.is_success() {
        return Err(Box::new(io::Error::other(format!(
            "request to {url} failed with {status}: {body}"
        ))));
    }

    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        println!("{body}");
    }
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(signal) => signal,
            Err(err) => {
                tracing::warn!(error = %err, "failed to install SIGTERM handler");
                let _ = signal::ctrl_c().await;
                return;
            }
        };

        tokio::select! {
            _ = signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        let _ = signal::ctrl_c().await;
    }
}
