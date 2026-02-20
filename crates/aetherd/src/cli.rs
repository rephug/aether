use std::ffi::OsStr;
use std::path::PathBuf;
use std::time::Duration;

use aether_analysis::{IntentScope, RiskLevel as CouplingRiskLevel};
use aether_config::{InferenceProviderKind, OLLAMA_DEFAULT_ENDPOINT, VerifyMode};
use clap::{Args, Parser, Subcommand, ValueEnum};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum AskIncludeArg {
    Symbols,
    Notes,
    Coupling,
    Tests,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct AskArgs {
    #[arg(help = "Unified query text across symbols, notes, coupling, and tests")]
    pub query: String,

    #[arg(
        long,
        default_value_t = 10,
        help = "Result limit for ask (clamped to 1..100)"
    )]
    pub limit: u32,

    #[arg(
        long,
        value_delimiter = ',',
        value_name = "TYPE",
        help = "Optional include filters: symbols,notes,coupling,tests"
    )]
    pub include: Vec<AskIncludeArg>,
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

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct MineCouplingArgs {
    #[arg(
        long,
        help = "Maximum number of commits to scan for this run (defaults to config coupling.commit_window)"
    )]
    pub commits: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct BlastRadiusArgs {
    #[arg(help = "Target file path")]
    pub file: String,

    #[arg(
        long,
        default_value = "medium",
        value_parser = parse_coupling_risk_level,
        help = "Minimum risk level to include: low, medium, high, critical"
    )]
    pub min_risk: CouplingRiskLevel,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct CouplingReportArgs {
    #[arg(
        long,
        default_value_t = 20,
        help = "Number of top coupling pairs to return (clamped to 1..200)"
    )]
    pub top: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct TestIntentsArgs {
    #[arg(help = "Test file path to inspect extracted test intents")]
    pub file: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommunitiesFormat {
    Table,
    Json,
}

impl CommunitiesFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Json => "json",
        }
    }
}

impl std::str::FromStr for CommunitiesFormat {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "table" => Ok(Self::Table),
            "json" => Ok(Self::Json),
            other => Err(format!(
                "invalid communities format '{other}', expected one of: table, json"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Args)]
pub struct DriftReportArgs {
    #[arg(
        long,
        default_value = "100 commits",
        help = "Analysis window (examples: '50 commits', '30d', 'since:a1b2c3d')"
    )]
    pub window: String,

    #[arg(
        long = "min-drift",
        default_value_t = 0.15,
        help = "Minimum semantic drift magnitude to include (0.0..1.0)"
    )]
    pub min_drift: f32,

    #[arg(
        long,
        help = "Include acknowledged drift findings in the report output"
    )]
    pub include_acknowledged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct DriftAckArgs {
    #[arg(help = "Drift result identifier to acknowledge")]
    pub result_id: String,

    #[arg(long, help = "Acknowledgement note to store in project memory")]
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct CommunitiesArgs {
    #[arg(
        long,
        default_value = "table",
        value_parser = parse_communities_format,
        help = "Output format: table or json"
    )]
    pub format: CommunitiesFormat,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct TraceCauseArgs {
    #[arg(
        help = "Target symbol name (required unless --symbol-id is provided)",
        required_unless_present = "symbol_id"
    )]
    pub symbol_name: Option<String>,

    #[arg(
        long,
        help = "Direct symbol identifier (bypasses name+file resolution)"
    )]
    pub symbol_id: Option<String>,

    #[arg(
        long,
        help = "File path containing the target symbol name",
        required_unless_present = "symbol_id"
    )]
    pub file: Option<String>,

    #[arg(
        long,
        default_value = "20 commits",
        help = "Lookback window (examples: '20 commits', '14d', 'since:a1b2c3d')"
    )]
    pub lookback: String,

    #[arg(
        long = "depth",
        default_value_t = 5,
        help = "Maximum upstream traversal depth (clamped to 1..10)"
    )]
    pub depth: u32,

    #[arg(
        long,
        default_value_t = 5,
        help = "Maximum causal candidates to return (clamped to 1..50)"
    )]
    pub limit: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthFilter {
    Critical,
    Cycles,
    Orphans,
    Bottlenecks,
    RiskHotspots,
}

#[derive(Debug, Clone, PartialEq, Args)]
pub struct HealthArgs {
    #[arg(
        value_parser = parse_health_filter,
        help = "Optional health view filter: critical, cycles, orphans, bottlenecks, risk-hotspots"
    )]
    pub filter: Option<HealthFilter>,

    #[arg(
        long,
        default_value_t = 10,
        help = "Result limit for list sections (clamped to 1..200)"
    )]
    pub limit: u32,

    #[arg(
        long = "min-risk",
        default_value_t = 0.5,
        help = "Minimum composite risk score threshold (0.0..1.0)"
    )]
    pub min_risk: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct SnapshotIntentArgs {
    #[arg(
        long,
        value_parser = parse_intent_scope,
        help = "Snapshot scope: file, symbol, directory"
    )]
    pub scope: IntentScope,

    #[arg(long, help = "Target identifier for scope (path or symbol id)")]
    pub target: String,

    #[arg(long, help = "Human-readable snapshot label")]
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct VerifyIntentArgs {
    #[arg(help = "Intent snapshot identifier")]
    pub snapshot_id: String,

    #[arg(
        long,
        conflicts_with = "no_regenerate_sir",
        help = "Force SIR regeneration before verification"
    )]
    pub regenerate_sir: bool,

    #[arg(
        long,
        conflicts_with = "regenerate_sir",
        help = "Skip SIR regeneration before verification"
    )]
    pub no_regenerate_sir: bool,
}

#[derive(Debug, Clone, PartialEq, Subcommand)]
pub enum Commands {
    /// Generate agent configuration files for AI coding agents
    InitAgent(InitAgentArgs),
    /// Set up local Ollama inference for offline SIR generation
    SetupLocal(SetupLocalArgs),
    /// Store a project memory note
    Remember(RememberArgs),
    /// Search project memory notes
    Recall(RecallArgs),
    /// Unified search across symbols, notes, coupling, and test intents
    Ask(AskArgs),
    /// List recent project memory notes
    Notes(NotesArgs),
    /// Mine temporal/static/semantic file coupling signals
    MineCoupling(MineCouplingArgs),
    /// Show coupled files and risk when changing one file
    BlastRadius(BlastRadiusArgs),
    /// Show highest-scoring coupling pairs
    CouplingReport(CouplingReportArgs),
    /// List extracted behavioral test intents for a test file
    TestIntents(TestIntentsArgs),
    /// Run semantic/boundary/structural drift analysis
    DriftReport(DriftReportArgs),
    /// Acknowledge a drift result and store a project note
    DriftAck(DriftAckArgs),
    /// Show current community assignments from dependency graph
    Communities(CommunitiesArgs),
    /// Trace likely upstream semantic causes for a target symbol
    TraceCause(TraceCauseArgs),
    /// Graph health dashboard and risk hotspots
    Health(HealthArgs),
    /// Snapshot current symbol intent state for a refactor scope
    SnapshotIntent(SnapshotIntentArgs),
    /// Verify post-refactor intent against a saved snapshot
    VerifyIntent(VerifyIntentArgs),
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

fn parse_coupling_risk_level(value: &str) -> Result<CouplingRiskLevel, String> {
    match value.trim() {
        "low" => Ok(CouplingRiskLevel::Low),
        "medium" => Ok(CouplingRiskLevel::Medium),
        "high" => Ok(CouplingRiskLevel::High),
        "critical" => Ok(CouplingRiskLevel::Critical),
        other => Err(format!(
            "invalid risk level '{other}', expected one of: low, medium, high, critical"
        )),
    }
}

fn parse_communities_format(value: &str) -> Result<CommunitiesFormat, String> {
    value.parse()
}

fn parse_health_filter(value: &str) -> Result<HealthFilter, String> {
    match value.trim() {
        "critical" => Ok(HealthFilter::Critical),
        "cycles" => Ok(HealthFilter::Cycles),
        "orphans" => Ok(HealthFilter::Orphans),
        "bottlenecks" => Ok(HealthFilter::Bottlenecks),
        "risk-hotspots" | "risk_hotspots" => Ok(HealthFilter::RiskHotspots),
        other => Err(format!(
            "invalid health filter '{other}', expected one of: critical, cycles, orphans, bottlenecks, risk-hotspots"
        )),
    }
}

fn parse_intent_scope(value: &str) -> Result<IntentScope, String> {
    IntentScope::parse(value).ok_or_else(|| {
        format!("invalid intent scope '{value}', expected one of: file, symbol, directory")
    })
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
    fn ask_subcommand_parses_limit_and_include_filters() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "ask",
            "payment retry",
            "--limit",
            "12",
            "--include",
            "symbols,tests",
        ])
        .expect("ask should parse");

        match cli.command {
            Some(Commands::Ask(args)) => {
                assert_eq!(args.query, "payment retry");
                assert_eq!(args.limit, 12);
                assert_eq!(args.include.len(), 2);
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
    fn mine_coupling_subcommand_parses_optional_commit_window() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "mine-coupling",
            "--commits",
            "120",
        ])
        .expect("mine-coupling should parse");

        match cli.command {
            Some(Commands::MineCoupling(args)) => {
                assert_eq!(args.commits, Some(120));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn blast_radius_subcommand_parses_file_and_min_risk() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "blast-radius",
            "crates/aether-store/src/lib.rs",
            "--min-risk",
            "high",
        ])
        .expect("blast-radius should parse");

        match cli.command {
            Some(Commands::BlastRadius(args)) => {
                assert_eq!(args.file, "crates/aether-store/src/lib.rs");
                assert_eq!(args.min_risk, aether_analysis::RiskLevel::High);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn coupling_report_subcommand_parses_top() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "coupling-report",
            "--top",
            "15",
        ])
        .expect("coupling-report should parse");

        match cli.command {
            Some(Commands::CouplingReport(args)) => {
                assert_eq!(args.top, 15);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn test_intents_subcommand_parses_file_argument() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "test-intents",
            "tests/payment_test.rs",
        ])
        .expect("test-intents should parse");

        match cli.command {
            Some(Commands::TestIntents(args)) => {
                assert_eq!(args.file, "tests/payment_test.rs");
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn drift_report_subcommand_parses_window_and_threshold() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "drift-report",
            "--window",
            "50 commits",
            "--min-drift",
            "0.2",
            "--include-acknowledged",
        ])
        .expect("drift-report should parse");

        match cli.command {
            Some(Commands::DriftReport(args)) => {
                assert_eq!(args.window, "50 commits");
                assert!((args.min_drift - 0.2).abs() < f32::EPSILON);
                assert!(args.include_acknowledged);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn drift_ack_subcommand_parses_result_and_note() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "drift-ack",
            "drift-123",
            "--note",
            "Intentional change",
        ])
        .expect("drift-ack should parse");

        match cli.command {
            Some(Commands::DriftAck(args)) => {
                assert_eq!(args.result_id, "drift-123");
                assert_eq!(args.note, "Intentional change");
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn communities_subcommand_parses_format() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "communities",
            "--format",
            "table",
        ])
        .expect("communities should parse");

        match cli.command {
            Some(Commands::Communities(args)) => {
                assert_eq!(args.format.as_str(), "table");
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn trace_cause_subcommand_parses_symbol_name_and_file() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "trace-cause",
            "process_payment",
            "--file",
            "src/payments/processor.rs",
            "--lookback",
            "20 commits",
            "--depth",
            "4",
            "--limit",
            "6",
        ])
        .expect("trace-cause should parse");

        match cli.command {
            Some(Commands::TraceCause(args)) => {
                assert_eq!(args.symbol_name.as_deref(), Some("process_payment"));
                assert_eq!(args.file.as_deref(), Some("src/payments/processor.rs"));
                assert_eq!(args.symbol_id, None);
                assert_eq!(args.lookback, "20 commits");
                assert_eq!(args.depth, 4);
                assert_eq!(args.limit, 6);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn trace_cause_subcommand_parses_symbol_id_mode() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "trace-cause",
            "--symbol-id",
            "sym-123",
            "--depth",
            "3",
        ])
        .expect("trace-cause symbol-id mode should parse");

        match cli.command {
            Some(Commands::TraceCause(args)) => {
                assert_eq!(args.symbol_id.as_deref(), Some("sym-123"));
                assert_eq!(args.symbol_name, None);
                assert_eq!(args.file, None);
                assert_eq!(args.depth, 3);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn health_subcommand_parses_optional_filter_and_flags() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "health",
            "risk-hotspots",
            "--limit",
            "15",
            "--min-risk",
            "0.7",
        ])
        .expect("health should parse");

        match cli.command {
            Some(Commands::Health(args)) => {
                assert_eq!(args.filter, Some(super::HealthFilter::RiskHotspots));
                assert_eq!(args.limit, 15);
                assert!((args.min_risk - 0.7).abs() < f32::EPSILON);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn snapshot_intent_subcommand_parses_scope_target_label() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "snapshot-intent",
            "--scope",
            "file",
            "--target",
            "src/payments/processor.rs",
            "--label",
            "pre-refactor",
        ])
        .expect("snapshot-intent should parse");

        match cli.command {
            Some(Commands::SnapshotIntent(args)) => {
                assert_eq!(args.scope, super::IntentScope::File);
                assert_eq!(args.target, "src/payments/processor.rs");
                assert_eq!(args.label, "pre-refactor");
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn verify_intent_subcommand_parses_regeneration_flags() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "verify-intent",
            "snap_abc123",
            "--regenerate-sir",
        ])
        .expect("verify-intent should parse");

        match cli.command {
            Some(Commands::VerifyIntent(args)) => {
                assert_eq!(args.snapshot_id, "snap_abc123");
                assert!(args.regenerate_sir);
                assert!(!args.no_regenerate_sir);
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
