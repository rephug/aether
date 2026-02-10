use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(author, version, about = "AETHER MCP server")]
struct Cli {
    #[arg(long)]
    workspace: PathBuf,

    #[arg(long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    aether_mcp::run_stdio_server(cli.workspace, cli.verbose).await
}
