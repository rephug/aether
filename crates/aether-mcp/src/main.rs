use std::path::PathBuf;

use aether_config::McpToolProfile;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(author, version, about = "AETHER MCP server")]
struct Cli {
    #[arg(long)]
    workspace: PathBuf,

    #[arg(long)]
    verbose: bool,

    /// Tools to list and serve: `full` (every tool) or `compact` (symbol lookup and
    /// search, dependencies, call chain, SIR context and test intents, with short
    /// definitions). Overrides `[mcp] tools` in .aether/config.toml; default full.
    #[arg(long, value_name = "full|compact")]
    tools: Option<McpToolProfile>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    aether_mcp::run_stdio_server(cli.workspace, cli.verbose, cli.tools).await
}
