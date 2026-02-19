use std::ffi::OsStr;
use std::path::PathBuf;
use std::time::Duration;

use aether_config::{InferenceProviderKind, OLLAMA_DEFAULT_ENDPOINT, VerifyMode};
use clap::{Args, Parser, Subcommand};

use crate::init_agent::AgentPlatform;
use crate::search::{SearchMode, SearchOutputFormat};
use crate::sir_pipeline::DEFAULT_SIR_CONCURRENCY;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogFormat {
    #[default]
    Human,
    Json,
}

impl LogFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Json => "json",
        }
    }
}

impl std::str::FromStr for LogFormat {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "human" => Ok(Self::Human),
            "json" => Ok(Self::Json),
            other => Err(format!(
                "invalid log format '{other}', expected one of: human, json"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct InitAgentArgs {
    #[arg(
        long,
        value_enum,
        default_value_t = AgentPlatform::All,
        help = "Platform template set to generate"
    )]
    pub platform: AgentPlatform,

    #[arg(long, help = "Overwrite existing files")]
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct SetupLocalArgs {
    #[arg(
        long,
        default_value = OLLAMA_DEFAULT_ENDPOINT,
        help = "Ollama endpoint base URL"
    )]
    pub endpoint: String,

    #[arg(long, help = "Model to use for local SIR generation")]
    pub model: Option<String>,

    #[arg(long, help = "Skip model pull even when model is missing")]
    pub skip_pull: bool,

    #[arg(
        long,
        help = "Skip writing provider/model settings to .aether/config.toml"
    )]
    pub skip_config: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct RememberArgs {
    #[arg(help = "Note content to store")]
    pub content: String,

    #[arg(
        long,
        value_delimiter = ',',
        value_name = "TAG",
        help = "Optional comma-separated tags"
    )]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct RecallArgs {
    #[arg(help = "Query text for note recall")]
    pub query: String,

    #[arg(
        long,
        default_value = "hybrid",
        value_parser = parse_search_mode,
        help = "Recall mode: lexical, semantic, or hybrid"
    )]
    pub mode: SearchMode,

    #[arg(
        long,
        default_value_t = 5,
        help = "Result limit for recall (clamped to 1..100)"
    )]
    pub limit: u32,

    #[arg(
        long,
        value_delimiter = ',',
        value_name = "TAG",
        help = "Optional comma-separated tag filter"
    )]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct NotesArgs {
    #[arg(
        long,
        default_value_t = 10,
        help = "Number of recent notes to list (clamped to 1..100)"
    )]
    pub limit: u32,

    #[arg(
        long,
        value_parser = parse_since_duration,
        help = "Include only notes updated within <n><unit>, where unit is d|h|m|s (example: 7d)"
    )]
    pub since: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Commands {
    /// Generate agent configuration files for AI coding agents
    InitAgent(InitAgentArgs),
    /// Set up local Ollama inference for offline SIR generation
    SetupLocal(SetupLocalArgs),
    /// Store a project memory note
    Remember(RememberArgs),
    /// Search project memory notes
    Recall(RecallArgs),
    /// List recent project memory notes
    Notes(NotesArgs),
}

#[derive(Debug, Clone, Parser)]
#[command(author, version, about = "AETHER Observer daemon")]
pub struct Cli {
    #[arg(
        long,
        global = true,
        default_value = ".",
        help = "Workspace root to index/search"
    )]
    pub workspace: PathBuf,

    #[command(subcommand)]
    pub command: Option<Commands>,

    #[arg(
        long,
        default_value_t = 300,
        help = "Debounce window for watcher events"
    )]
    pub debounce_ms: u64,

    #[arg(long, help = "Print symbol-change events as JSON lines")]
    pub print_events: bool,

    #[arg(long, help = "Print SIR lifecycle lines as symbols are processed")]
    pub print_sir: bool,

    #[arg(
        long,
        default_value = "human",
        value_parser = parse_log_format,
        help = "Log format: human or json"
    )]
    pub log_format: LogFormat,

    #[arg(long, help = "Run as stdio LSP server")]
    pub lsp: bool,

    #[arg(
        long,
        requires = "lsp",
        help = "Run background indexing while LSP is active"
    )]
    pub index: bool,

    #[arg(
        long,
        conflicts_with_all = ["lsp", "index", "index_once", "verify"],
        help = "Run one-shot symbol search and exit"
    )]
    pub search: Option<String>,

    #[arg(
        long,
        requires = "workspace",
        conflicts_with_all = [
            "search",
            "lsp",
            "index",
            "index_once",
            "verify",
            "download_models"
        ],
        help = "Calibrate per-language semantic thresholds from indexed embeddings and exit"
    )]
    pub calibrate: bool,

    #[arg(
        long,
        default_value_t = 20,
        requires = "search",
        help = "Result limit for --search (clamped to 1..100)"
    )]
    pub search_limit: u32,

    #[arg(
        long,
        default_value = "lexical",
        value_parser = parse_search_mode,
        requires = "search",
        help = "Search mode: lexical, semantic, or hybrid. Semantic/hybrid fall back to lexical with a reason when unavailable"
    )]
    pub search_mode: SearchMode,

    #[arg(
        long,
        default_value = "table",
        value_parser = parse_search_output_format,
        requires = "search",
        help = "Search output format: table or json"
    )]
    pub output: SearchOutputFormat,

    #[arg(
        long,
        conflicts_with_all = ["search", "lsp", "index", "verify"],
        help = "Run one full index pass and exit"
    )]
    pub index_once: bool,

    #[arg(
        long,
        conflicts_with_all = ["search", "lsp", "index", "index_once"],
        help = "Run verification commands and exit"
    )]
    pub verify: bool,

    #[arg(
        long,
        conflicts_with_all = ["search", "lsp", "index", "index_once", "verify"],
        help = "Download local model files required for Candle embeddings and any configured Candle reranker, then exit"
    )]
    pub download_models: bool,

    #[arg(
        long,
        requires = "download_models",
        help = "Override model cache directory for --download-models"
    )]
    pub model_dir: Option<PathBuf>,

    #[arg(
        long,
        requires = "verify",
        help = "Run only the provided allowlisted command"
    )]
    pub verify_command: Vec<String>,

    #[arg(
        long,
        requires = "verify",
        value_parser = parse_verify_mode,
        help = "Verification mode override: host, container, or microvm"
    )]
    pub verify_mode: Option<VerifyMode>,

    #[arg(
        long,
        requires = "verify",
        help = "Fall back to host mode when the selected verification runtime is unavailable"
    )]
    pub verify_fallback_host_on_unavailable: bool,

    #[arg(
        long,
        requires = "verify",
        help = "When verify mode is microvm, fall back to container mode if microvm runtime is unavailable"
    )]
    pub verify_fallback_container_on_unavailable: bool,

    #[arg(long, default_value_t = DEFAULT_SIR_CONCURRENCY)]
    pub sir_concurrency: usize,

    #[arg(long, value_parser = parse_inference_provider)]
    pub inference_provider: Option<InferenceProviderKind>,

    #[arg(long)]
    pub inference_model: Option<String>,

    #[arg(long)]
    pub inference_endpoint: Option<String>,

    #[arg(long)]
    pub inference_api_key_env: Option<String>,
}

pub fn parse_cli() -> Cli {
    let mut args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|arg| arg == OsStr::new("--")) {
        args.remove(1);
    }

    Cli::parse_from(args)
}

fn parse_inference_provider(value: &str) -> Result<InferenceProviderKind, String> {
    value.parse()
}

fn parse_search_mode(value: &str) -> Result<SearchMode, String> {
    value.parse()
}

fn parse_search_output_format(value: &str) -> Result<SearchOutputFormat, String> {
    value.parse()
}

fn parse_verify_mode(value: &str) -> Result<VerifyMode, String> {
    value.parse()
}

fn parse_log_format(value: &str) -> Result<LogFormat, String> {
    value.parse()
}

fn parse_since_duration(value: &str) -> Result<Duration, String> {
    let trimmed = value.trim().to_ascii_lowercase();
    if trimmed.len() < 2 {
        return Err("invalid duration, expected format <n><unit> like 7d or 12h".to_owned());
    }

    let unit = trimmed
        .chars()
        .last()
        .ok_or_else(|| "invalid duration".to_owned())?;
    let amount_str = &trimmed[..trimmed.len() - 1];
    let amount = amount_str
        .parse::<u64>()
        .map_err(|_| format!("invalid duration value '{amount_str}'"))?;

    let seconds = match unit {
        'd' => amount.saturating_mul(24 * 60 * 60),
        'h' => amount.saturating_mul(60 * 60),
        'm' => amount.saturating_mul(60),
        's' => amount,
        _ => {
            return Err(format!(
                "invalid duration unit '{unit}', expected one of: d, h, m, s"
            ));
        }
    };

    Ok(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use aether_config::OLLAMA_DEFAULT_ENDPOINT;
    use clap::Parser;

    use super::{Cli, Commands, parse_since_duration};
    use crate::init_agent::AgentPlatform;

    #[test]
    fn legacy_search_flags_parse_without_subcommand() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "--search",
            "alpha behavior",
            "--search-mode",
            "hybrid",
            "--output",
            "json",
        ])
        .expect("search flags should parse");

        assert!(cli.command.is_none());
        assert_eq!(cli.search.as_deref(), Some("alpha behavior"));
        assert_eq!(cli.search_mode.as_str(), "hybrid");
        assert_eq!(cli.output.as_str(), "json");
    }

    #[test]
    fn legacy_lsp_flags_parse_without_subcommand() {
        let cli = Cli::try_parse_from(["aetherd", "--workspace", ".", "--lsp", "--index"])
            .expect("lsp/index flags should parse");

        assert!(cli.command.is_none());
        assert!(cli.lsp);
        assert!(cli.index);
    }

    #[test]
    fn legacy_download_models_and_calibrate_flags_still_parse() {
        let download = Cli::try_parse_from(["aetherd", "--workspace", ".", "--download-models"])
            .expect("download-models should parse");
        assert!(download.command.is_none());
        assert!(download.download_models);

        let calibrate = Cli::try_parse_from(["aetherd", "--workspace", ".", "--calibrate"])
            .expect("calibrate should parse");
        assert!(calibrate.command.is_none());
        assert!(calibrate.calibrate);
    }

    #[test]
    fn init_agent_subcommand_parses_with_defaults() {
        let cli = Cli::try_parse_from(["aetherd", "--workspace", ".", "init-agent"])
            .expect("init-agent should parse");

        match cli.command {
            Some(Commands::InitAgent(args)) => {
                assert_eq!(args.platform, AgentPlatform::All);
                assert!(!args.force);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn init_agent_subcommand_parses_platform_and_force() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "init-agent",
            "--platform",
            "claude",
            "--force",
            "--workspace",
            ".",
        ])
        .expect("init-agent with args should parse");

        match cli.command {
            Some(Commands::InitAgent(args)) => {
                assert_eq!(args.platform, AgentPlatform::Claude);
                assert!(args.force);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn setup_local_subcommand_parses_with_defaults() {
        let cli = Cli::try_parse_from(["aetherd", "--workspace", ".", "setup-local"])
            .expect("setup-local should parse");

        match cli.command {
            Some(Commands::SetupLocal(args)) => {
                assert_eq!(args.endpoint, OLLAMA_DEFAULT_ENDPOINT);
                assert_eq!(args.model, None);
                assert!(!args.skip_pull);
                assert!(!args.skip_config);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn setup_local_subcommand_parses_all_flags() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "setup-local",
            "--endpoint",
            "http://127.0.0.1:11435",
            "--model",
            "qwen2.5-coder:7b-instruct-q4_K_M",
            "--skip-pull",
            "--skip-config",
        ])
        .expect("setup-local with args should parse");

        match cli.command {
            Some(Commands::SetupLocal(args)) => {
                assert_eq!(args.endpoint, "http://127.0.0.1:11435");
                assert_eq!(
                    args.model.as_deref(),
                    Some("qwen2.5-coder:7b-instruct-q4_K_M")
                );
                assert!(args.skip_pull);
                assert!(args.skip_config);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn remember_subcommand_parses_content_and_tags() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "remember",
            "Document rationale",
            "--tags",
            "architecture,decision",
        ])
        .expect("remember should parse");

        match cli.command {
            Some(Commands::Remember(args)) => {
                assert_eq!(args.content, "Document rationale");
                assert_eq!(
                    args.tags,
                    vec!["architecture".to_owned(), "decision".to_owned()]
                );
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn recall_subcommand_parses_mode_limit_and_tags() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "recall",
            "graph decision",
            "--mode",
            "semantic",
            "--limit",
            "7",
            "--tags",
            "architecture",
        ])
        .expect("recall should parse");

        match cli.command {
            Some(Commands::Recall(args)) => {
                assert_eq!(args.query, "graph decision");
                assert_eq!(args.mode.as_str(), "semantic");
                assert_eq!(args.limit, 7);
                assert_eq!(args.tags, vec!["architecture".to_owned()]);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn notes_subcommand_parses_limit_and_since() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "notes",
            "--limit",
            "12",
            "--since",
            "7d",
        ])
        .expect("notes should parse");

        match cli.command {
            Some(Commands::Notes(args)) => {
                assert_eq!(args.limit, 12);
                assert_eq!(args.since, Some(Duration::from_secs(7 * 24 * 60 * 60)));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_since_duration_rejects_invalid_unit() {
        let err = parse_since_duration("7w").expect_err("expected error");
        assert!(err.contains("invalid duration unit"));
    }
}
