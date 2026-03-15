use std::ffi::OsStr;
use std::path::PathBuf;
use std::time::Duration;

use aether_analysis::RiskLevel as CouplingRiskLevel;
use aether_config::{InferenceProviderKind, OLLAMA_DEFAULT_ENDPOINT, VerifyMode};
use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::init_agent::AgentPlatform;
use crate::search::{SearchMode, SearchOutputFormat};

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

#[derive(Debug, Clone, PartialEq, Args)]
pub struct RegenerateArgs {
    #[arg(long, default_value_t = 0.5)]
    pub below_confidence: f32,

    #[arg(long)]
    pub from_provider: Option<String>,

    #[arg(long)]
    pub file: Option<String>,

    #[arg(long)]
    pub deep: bool,

    #[arg(long)]
    pub max: Option<usize>,

    #[arg(long)]
    pub dry_run: bool,
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

#[derive(Debug, Clone, PartialEq, Args)]
pub struct HealthArgs {
    #[arg(help = "Optional section filter: critical, cycles, orphans, bottlenecks, risk-hotspots")]
    pub filter: Option<String>,

    #[arg(long, default_value = "10", help = "Maximum rows per selected section")]
    pub limit: u32,

    #[arg(
        long,
        default_value = "0.0",
        help = "Minimum risk score threshold for critical/risk-hotspot sections"
    )]
    pub min_risk: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HealthScoreOutputFormat {
    #[default]
    Table,
    Json,
}

impl HealthScoreOutputFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Json => "json",
        }
    }
}

impl std::str::FromStr for HealthScoreOutputFormat {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "table" => Ok(Self::Table),
            "json" => Ok(Self::Json),
            other => Err(format!(
                "invalid health-score output format '{other}', expected one of: table, json"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RefactorPrepOutputFormat {
    #[default]
    Human,
    Json,
}

impl RefactorPrepOutputFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Json => "json",
        }
    }
}

impl std::str::FromStr for RefactorPrepOutputFormat {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "human" => Ok(Self::Human),
            "json" => Ok(Self::Json),
            other => Err(format!(
                "invalid refactor-prep output format '{other}', expected one of: human, json"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct HealthScoreArgs {
    #[arg(
        long,
        default_value = "table",
        value_parser = parse_health_score_output_format,
        help = "Output format: table or json"
    )]
    pub output: HealthScoreOutputFormat,

    #[arg(
        long,
        help = "Exit with code 1 when the workspace score is less than this value"
    )]
    pub fail_below: Option<u32>,

    #[arg(long, help = "Skip reading and writing score history")]
    pub no_history: bool,

    #[arg(
        long = "crate",
        value_name = "NAME",
        help = "Limit scoring to the named crate (repeatable)"
    )]
    pub crate_filter: Vec<String>,

    #[arg(
        long,
        help = "Enable git and semantic health signals when data is available"
    )]
    pub semantic: bool,

    #[arg(
        long,
        help = "Show heuristic split suggestions for qualifying hotspot crates"
    )]
    pub suggest_splits: bool,

    #[arg(
        long,
        value_name = "HASH|last",
        help = "Compare the current workspace score against the latest saved report or a specific commit hash"
    )]
    pub compare: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct FsckArgs {
    #[arg(long, help = "Attempt to repair detected inconsistencies")]
    pub repair: bool,

    #[arg(long, help = "Print additional reconciliation diagnostics")]
    pub verbose: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct RefactorPrepArgs {
    #[arg(
        long,
        conflicts_with = "crate_name",
        required_unless_present = "crate_name",
        help = "Workspace-relative file path to prepare for refactoring"
    )]
    pub file: Option<String>,

    #[arg(
        long = "crate",
        conflicts_with = "file",
        required_unless_present = "file",
        help = "Crate name to prepare for refactoring"
    )]
    pub crate_name: Option<String>,

    #[arg(
        long,
        default_value_t = 20,
        help = "Maximum number of refactor candidates"
    )]
    pub top_n: usize,

    #[arg(long, help = "Force the deep pass to use the local Qwen provider")]
    pub local: bool,

    #[arg(
        long,
        default_value = "human",
        value_parser = parse_refactor_prep_output_format,
        help = "Output format: human or json"
    )]
    pub output: RefactorPrepOutputFormat,
}

#[derive(Debug, Clone, PartialEq, Args)]
pub struct VerifyIntentArgs {
    #[arg(long, help = "Snapshot identifier created by refactor-prep")]
    pub snapshot: String,

    #[arg(
        long,
        default_value_t = 0.85,
        help = "Minimum similarity required to pass verification"
    )]
    pub threshold: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BatchPass {
    Scan,
    Triage,
    Deep,
}

impl BatchPass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scan => "scan",
            Self::Triage => "triage",
            Self::Deep => "deep",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct BatchArgs {
    #[command(subcommand)]
    pub command: BatchCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum BatchCommand {
    /// Run structural extraction only and populate the symbols/graph store
    Extract,
    /// Build Gemini Batch API JSONL for a single pass
    Build(BatchBuildArgs),
    /// Ingest Gemini Batch API result JSONL for a single pass
    Ingest(BatchIngestArgs),
    /// Run extract -> build -> submit/poll/download -> ingest across one or more passes
    Run(BatchRunArgs),
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct BatchBuildArgs {
    #[arg(long, value_enum, help = "Batch pass to build")]
    pub pass: BatchPass,

    #[arg(long, help = "Model override for this pass")]
    pub model: Option<String>,

    #[arg(long, help = "Thinking override for this pass")]
    pub thinking: Option<String>,

    #[arg(long, help = "Neighbor depth override for this pass")]
    pub neighbor_depth: Option<u32>,

    #[arg(long, help = "Max source chars override for this pass")]
    pub max_chars: Option<usize>,

    #[arg(long, help = "Batch JSONL output directory override")]
    pub batch_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct BatchIngestArgs {
    #[arg(long, value_enum, help = "Batch pass to ingest")]
    pub pass: BatchPass,

    #[arg(long, help = "Model override for this pass")]
    pub model: Option<String>,

    #[arg(help = "Path to the Gemini result JSONL file")]
    pub results_jsonl: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct BatchRunArgs {
    #[arg(
        long,
        default_value = "scan",
        help = "Comma-separated passes to run in order"
    )]
    pub passes: String,

    #[arg(long, help = "Scan-pass model override")]
    pub scan_model: Option<String>,

    #[arg(long, help = "Triage-pass model override")]
    pub triage_model: Option<String>,

    #[arg(long, help = "Deep-pass model override")]
    pub deep_model: Option<String>,

    #[arg(long, help = "Scan-pass thinking override")]
    pub scan_thinking: Option<String>,

    #[arg(long, help = "Triage-pass thinking override")]
    pub triage_thinking: Option<String>,

    #[arg(long, help = "Deep-pass thinking override")]
    pub deep_thinking: Option<String>,

    #[arg(long, help = "Triage-pass neighbor depth override")]
    pub triage_neighbor_depth: Option<u32>,

    #[arg(long, help = "Deep-pass neighbor depth override")]
    pub deep_neighbor_depth: Option<u32>,

    #[arg(long, help = "Scan-pass max source chars override")]
    pub scan_max_chars: Option<usize>,

    #[arg(long, help = "Triage-pass max source chars override")]
    pub triage_max_chars: Option<usize>,

    #[arg(long, help = "Deep-pass max source chars override")]
    pub deep_max_chars: Option<usize>,

    #[arg(long, help = "Batch JSONL output directory override")]
    pub batch_dir: Option<String>,

    #[arg(long, help = "JSONL chunk size override")]
    pub jsonl_chunk_size: Option<usize>,

    #[arg(long, help = "Batch poll interval override in seconds")]
    pub poll_interval_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Subcommand)]
pub enum Commands {
    /// Batch indexing operations
    Batch(BatchArgs),
    /// Generate agent configuration files for AI coding agents
    InitAgent(InitAgentArgs),
    /// Regenerate low-quality SIR records with optional deep enrichment
    Regenerate(RegenerateArgs),
    /// Set up local Ollama inference for offline SIR generation
    SetupLocal(SetupLocalArgs),
    /// Show local index health and SIR coverage
    Status,
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
    /// Show graph health metrics with risk scoring
    Health(HealthArgs),
    /// Compute a structural health score for each workspace crate
    HealthScore(HealthScoreArgs),
    /// Deep-scan the riskiest symbols in a file or crate and persist an intent snapshot
    RefactorPrep(RefactorPrepArgs),
    /// Compare current SIR against a saved refactor-prep snapshot
    VerifyIntent(VerifyIntentArgs),
    /// Verify and optionally repair cross-store consistency
    Fsck(FsckArgs),
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

    #[cfg(feature = "dashboard")]
    #[arg(long, help = "Disable web dashboard even if compiled with feature")]
    pub no_dashboard: bool,

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
        requires = "index_once",
        conflicts_with_all = ["force", "download_models", "calibrate"],
        help = "When used with --index-once, re-embed all symbols with existing SIR using the current embedding provider without regenerating SIR"
    )]
    pub embeddings_only: bool,

    #[arg(
        long,
        requires = "index_once",
        help = "When used with --index-once, run structural indexing plus the full scan/quality pipeline before exit"
    )]
    pub full: bool,

    #[arg(
        long,
        requires = "index_once",
        help = "When used with --index-once --full, force the deep pass after scan/triage generation"
    )]
    pub deep: bool,

    #[arg(
        long,
        requires_all = ["index_once", "full"],
        help = "With --index-once --full, report stale symbol reconciliations and prunes without mutating the store"
    )]
    pub dry_run: bool,

    #[arg(
        long,
        help = "Force SIR regeneration during indexing, even when existing SIR data is fresh"
    )]
    pub force: bool,

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

    #[arg(long)]
    pub sir_concurrency: Option<usize>,

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

fn parse_health_score_output_format(value: &str) -> Result<HealthScoreOutputFormat, String> {
    value.parse()
}

fn parse_refactor_prep_output_format(value: &str) -> Result<RefactorPrepOutputFormat, String> {
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

    #[cfg(feature = "dashboard")]
    #[test]
    fn legacy_dashboard_disable_flag_parses_without_subcommand() {
        let cli = Cli::try_parse_from(["aetherd", "--workspace", ".", "--lsp", "--no-dashboard"])
            .expect("no-dashboard flag should parse");

        assert!(cli.command.is_none());
        assert!(cli.lsp);
        assert!(cli.no_dashboard);
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
    fn cli_embeddings_only_flag_parses() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "--index-once",
            "--embeddings-only",
        ])
        .expect("embeddings-only should parse");

        assert!(cli.command.is_none());
        assert!(cli.index_once);
        assert!(cli.embeddings_only);
    }

    #[test]
    fn cli_embeddings_only_rejects_without_index_once() {
        let result = Cli::try_parse_from(["aetherd", "--workspace", ".", "--embeddings-only"]);

        assert!(result.is_err());
    }

    #[test]
    fn cli_embeddings_only_rejects_with_force() {
        let result = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "--index-once",
            "--embeddings-only",
            "--force",
        ]);

        assert!(result.is_err());
    }

    #[test]
    fn cli_full_dry_run_flag_parses() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "--index-once",
            "--full",
            "--dry-run",
        ])
        .expect("full dry-run should parse");

        assert!(cli.index_once);
        assert!(cli.full);
        assert!(cli.dry_run);
    }

    #[test]
    fn cli_dry_run_requires_full_index_once() {
        let result = Cli::try_parse_from(["aetherd", "--workspace", ".", "--dry-run"]);
        assert!(result.is_err());
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
            "qwen3.5:9b",
            "--skip-pull",
            "--skip-config",
        ])
        .expect("setup-local with args should parse");

        match cli.command {
            Some(Commands::SetupLocal(args)) => {
                assert_eq!(args.endpoint, "http://127.0.0.1:11435");
                assert_eq!(args.model.as_deref(), Some("qwen3.5:9b"));
                assert!(args.skip_pull);
                assert!(args.skip_config);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn status_subcommand_parses() {
        let cli =
            Cli::try_parse_from(["aetherd", "--workspace", ".", "status"]).expect("status parse");
        assert!(matches!(cli.command, Some(Commands::Status)));
    }

    #[test]
    fn regenerate_subcommand_parses_all_flags() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "regenerate",
            "--below-confidence",
            "0.42",
            "--from-provider",
            "qwen3.5:4b",
            "--file",
            "src/lib.rs",
            "--deep",
            "--max",
            "12",
            "--dry-run",
        ])
        .expect("regenerate should parse");

        match cli.command {
            Some(Commands::Regenerate(args)) => {
                assert!((args.below_confidence - 0.42).abs() < f32::EPSILON);
                assert_eq!(args.from_provider.as_deref(), Some("qwen3.5:4b"));
                assert_eq!(args.file.as_deref(), Some("src/lib.rs"));
                assert!(args.deep);
                assert_eq!(args.max, Some(12));
                assert!(args.dry_run);
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
    fn health_subcommand_parses_filter_limit_and_min_risk() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "health",
            "risk-hotspots",
            "--limit",
            "25",
            "--min-risk",
            "0.4",
        ])
        .expect("health subcommand should parse");

        match cli.command {
            Some(Commands::Health(args)) => {
                assert_eq!(args.filter.as_deref(), Some("risk-hotspots"));
                assert_eq!(args.limit, 25);
                assert!((args.min_risk - 0.4).abs() < f64::EPSILON);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn health_score_subcommand_parses_output_threshold_and_filters() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "health-score",
            "--workspace",
            ".",
            "--output",
            "json",
            "--fail-below",
            "70",
            "--semantic",
            "--suggest-splits",
            "--compare",
            "last",
            "--crate",
            "aether-core",
            "--crate",
            "aether-store",
        ])
        .expect("health-score should parse");

        match cli.command {
            Some(Commands::HealthScore(args)) => {
                assert_eq!(args.output.as_str(), "json");
                assert_eq!(args.fail_below, Some(70));
                assert!(args.semantic);
                assert!(args.suggest_splits);
                assert_eq!(args.compare.as_deref(), Some("last"));
                assert_eq!(
                    args.crate_filter,
                    vec!["aether-core".to_owned(), "aether-store".to_owned()]
                );
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn refactor_prep_subcommand_parses_file_local_and_output() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "refactor-prep",
            "--file",
            "crates/aetherd/src/main.rs",
            "--top-n",
            "12",
            "--local",
            "--output",
            "json",
        ])
        .expect("refactor-prep should parse");

        match cli.command {
            Some(Commands::RefactorPrep(args)) => {
                assert_eq!(args.file.as_deref(), Some("crates/aetherd/src/main.rs"));
                assert_eq!(args.crate_name, None);
                assert_eq!(args.top_n, 12);
                assert!(args.local);
                assert_eq!(args.output.as_str(), "json");
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn verify_intent_subcommand_parses_snapshot_and_threshold() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "verify-intent",
            "--snapshot",
            "refactor-prep-1234",
            "--threshold",
            "0.9",
        ])
        .expect("verify-intent should parse");

        match cli.command {
            Some(Commands::VerifyIntent(args)) => {
                assert_eq!(args.snapshot, "refactor-prep-1234");
                assert!((args.threshold - 0.9).abs() < f64::EPSILON);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn fsck_subcommand_parses_repair_and_verbose() {
        let cli = Cli::try_parse_from([
            "aetherd",
            "--workspace",
            ".",
            "fsck",
            "--repair",
            "--verbose",
        ])
        .expect("fsck should parse");

        match cli.command {
            Some(Commands::Fsck(args)) => {
                assert!(args.repair);
                assert!(args.verbose);
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
