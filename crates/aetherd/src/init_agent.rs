use std::fs;
use std::path::{Path, PathBuf};

use aether_config::{AetherConfig, config_path, load_workspace_config};
use aether_core::AETHER_AGENT_SCHEMA_VERSION;
use aether_parse::LanguageRegistry;
use anyhow::{Context, Result};
use clap::ValueEnum;

use crate::templates::{
    AuditChangesCommandTemplate, AuditCommandTemplate, AuditReportCommandTemplate, ClaudeTemplate,
    CodexInstructionsTemplate, CursorRulesTemplate, McpJsonTemplate, OmpAgentsTemplate,
    RefactorCommandTemplate, RefactorDeepCommandTemplate, ScanAllScriptTemplate,
    ScanCommandTemplate, SkillTemplate, TemplateContext,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum AgentPlatform {
    Claude,
    Codex,
    Cursor,
    /// Oh My Pi: `AGENTS.md` plus a project-root `.mcp.json` for the AETHER MCP server.
    Omp,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitAgentOptions {
    pub platform: AgentPlatform,
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitAgentOutcome {
    pub written_files: Vec<PathBuf>,
    pub skipped_existing_files: Vec<PathBuf>,
    pub used_default_config: bool,
}

impl InitAgentOutcome {
    pub fn exit_code(&self) -> i32 {
        if self.skipped_existing_files.is_empty() {
            0
        } else {
            2
        }
    }
}

pub fn run_init_agent(workspace: &Path, options: InitAgentOptions) -> Result<InitAgentOutcome> {
    let config_file = config_path(workspace);
    let used_default_config = !config_file.exists();
    let config = load_workspace_config(workspace).with_context(|| {
        format!(
            "failed to load workspace config at {}",
            config_file.display()
        )
    })?;

    let context = build_template_context(workspace, &config);
    let files = files_for_platform(options.platform, &context);

    let mut written_files = Vec::new();
    let mut skipped_existing_files = Vec::new();

    for file in files {
        let absolute_path = workspace.join(&file.relative_path);
        if absolute_path.exists() && file.relative_path == Path::new(MCP_CONFIG_FILE) {
            // Shared with other MCP servers: never replaced wholesale, even with --force.
            match merge_mcp_server(&absolute_path, &file.content, options.force)? {
                McpMerge::Updated => {
                    written_files.push(file.relative_path);
                    continue;
                }
                // Without --force an existing registration is an untouched file; with
                // --force an identical entry means the forced refresh is already satisfied.
                McpMerge::Unchanged if options.force => {
                    written_files.push(file.relative_path);
                    continue;
                }
                McpMerge::Unchanged => {
                    skipped_existing_files.push(file.relative_path);
                    continue;
                }
                McpMerge::NotJson if !options.force => {
                    skipped_existing_files.push(file.relative_path);
                    continue;
                }
                McpMerge::NotJson => {}
            }
        } else if absolute_path.exists() && !options.force {
            skipped_existing_files.push(file.relative_path);
            continue;
        }

        if let Some(parent) = absolute_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create parent directory {}", parent.display())
            })?;
        }

        fs::write(&absolute_path, file.content)
            .with_context(|| format!("failed to write {}", absolute_path.display()))?;
        if file.executable {
            mark_executable(&absolute_path)?;
        }
        written_files.push(file.relative_path);
    }

    Ok(InitAgentOutcome {
        written_files,
        skipped_existing_files,
        used_default_config,
    })
}

struct GeneratedFile {
    relative_path: PathBuf,
    content: String,
    /// Mark the written file executable (shell scripts).
    executable: bool,
}

impl GeneratedFile {
    fn new(relative_path: impl Into<PathBuf>, content: String) -> Self {
        Self {
            relative_path: relative_path.into(),
            content,
            executable: false,
        }
    }

    fn script(relative_path: impl Into<PathBuf>, content: String) -> Self {
        Self {
            relative_path: relative_path.into(),
            content,
            executable: true,
        }
    }
}

#[cfg(unix)]
fn mark_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .with_context(|| format!("failed to stat {}", path.display()))?
        .permissions();
    permissions.set_mode(permissions.mode() | 0o755);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("failed to chmod {}", path.display()))
}

#[cfg(not(unix))]
fn mark_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn files_for_platform(platform: AgentPlatform, context: &TemplateContext) -> Vec<GeneratedFile> {
    let mut files = Vec::new();

    if matches!(platform, AgentPlatform::Claude | AgentPlatform::All) {
        files.push(GeneratedFile::new(
            "CLAUDE.md",
            ClaudeTemplate::render(context),
        ));
        files.push(GeneratedFile::new(
            ".agents/skills/aether-context/SKILL.md",
            SkillTemplate::render(context),
        ));
        files.push(GeneratedFile::new(
            ".claude/commands/audit.md",
            AuditCommandTemplate::render(context),
        ));
        files.push(GeneratedFile::new(
            ".claude/commands/refactor.md",
            RefactorCommandTemplate::render(context),
        ));
        files.push(GeneratedFile::new(
            ".claude/commands/refactor-deep.md",
            RefactorDeepCommandTemplate::render(context),
        ));
        files.push(GeneratedFile::new(
            ".claude/commands/audit-report.md",
            AuditReportCommandTemplate::render(context),
        ));
        files.push(GeneratedFile::new(
            ".claude/commands/audit-changes.md",
            AuditChangesCommandTemplate::render(context),
        ));
        files.push(GeneratedFile::new(
            ".claude/commands/scan.md",
            ScanCommandTemplate::render(context),
        ));
        files.push(GeneratedFile::script(
            "scripts/scan_all.sh",
            ScanAllScriptTemplate::render(context),
        ));
    }

    if matches!(platform, AgentPlatform::Codex | AgentPlatform::All) {
        files.push(GeneratedFile::new(
            ".codex-instructions",
            CodexInstructionsTemplate::render(context),
        ));
    }

    if matches!(platform, AgentPlatform::Cursor | AgentPlatform::All) {
        files.push(GeneratedFile::new(
            ".cursor/rules",
            CursorRulesTemplate::render(context),
        ));
    }

    if matches!(platform, AgentPlatform::Omp | AgentPlatform::All) {
        files.push(GeneratedFile::new(
            "AGENTS.md",
            OmpAgentsTemplate::render(context),
        ));
    }

    // Claude Code and Oh My Pi both read a project-root `.mcp.json`; the non-interactive
    // `claude -p "/scan ..."` sessions started by scripts/scan_all.sh depend on it.
    if matches!(
        platform,
        AgentPlatform::Claude | AgentPlatform::Omp | AgentPlatform::All
    ) {
        files.push(GeneratedFile::new(
            ".mcp.json",
            McpJsonTemplate::render(context),
        ));
    }

    files
}

fn build_template_context(workspace: &Path, config: &AetherConfig) -> TemplateContext {
    TemplateContext {
        languages: detected_languages(),
        verify_commands: config.verify.commands.clone(),
        embeddings_enabled: config.embeddings.enabled,
        inference_provider: config.inference.provider.as_str().to_owned(),
        agent_schema_version: AETHER_AGENT_SCHEMA_VERSION,
        mcp_binary_hint: resolve_mcp_binary_hint(workspace),
    }
}

const SOURCE_TREE_MCP_BINARY: &str = "./target/debug/aether-mcp";
const MCP_CONFIG_FILE: &str = ".mcp.json";
const MCP_SERVER_KEY: &str = "aether";

#[derive(Debug, PartialEq, Eq)]
enum McpMerge {
    /// The `aether` entry was added (or, with `force`, refreshed) and the file rewritten.
    Updated,
    /// The file already registers `aether` as rendered; nothing written.
    Unchanged,
    /// Not a JSON object we can extend; left untouched.
    NotJson,
}

/// `.mcp.json` is shared with every other MCP server the project registers, so an
/// existing file is never treated as an all-or-nothing template: only the `aether`
/// entry is added when missing, and `force` refreshes that one entry while leaving
/// every other registration in place.
fn merge_mcp_server(existing_path: &Path, rendered: &str, force: bool) -> Result<McpMerge> {
    let current = fs::read_to_string(existing_path)
        .with_context(|| format!("failed to read {}", existing_path.display()))?;
    let Ok(mut current_json) = serde_json::from_str::<serde_json::Value>(&current) else {
        return Ok(McpMerge::NotJson);
    };
    let Some(root) = current_json.as_object_mut() else {
        return Ok(McpMerge::NotJson);
    };
    let servers = root
        .entry("mcpServers")
        .or_insert_with(|| serde_json::Value::Object(Default::default()));
    let Some(servers) = servers.as_object_mut() else {
        return Ok(McpMerge::NotJson);
    };
    let rendered_json: serde_json::Value =
        serde_json::from_str(rendered).context("rendered .mcp.json template is valid JSON")?;
    let Some(aether) = rendered_json
        .get("mcpServers")
        .and_then(|value| value.get(MCP_SERVER_KEY))
        .cloned()
    else {
        return Ok(McpMerge::NotJson);
    };
    match servers.get(MCP_SERVER_KEY) {
        Some(existing) if !force || *existing == aether => return Ok(McpMerge::Unchanged),
        _ => {}
    }
    servers.insert(MCP_SERVER_KEY.to_owned(), aether);
    let mut merged =
        serde_json::to_string_pretty(&current_json).context("merged .mcp.json serializes")?;
    merged.push('\n');
    fs::write(existing_path, merged)
        .with_context(|| format!("failed to write {}", existing_path.display()))?;
    Ok(McpMerge::Updated)
}
const MCP_BINARY_NAME: &str = "aether-mcp";

/// The command `.mcp.json` and the agent docs use to start the AETHER MCP server.
///
/// The source-tree debug binary is used only when it exists in this workspace (the
/// AETHER checkout dogfooding its own assets). Otherwise prefer the `aether-mcp` that
/// was installed next to the running `aetherd`, and fall back to the bare name so the
/// PATH resolves it. A downstream project never gets a path into a build tree it
/// does not have.
fn resolve_mcp_binary_hint(workspace: &Path) -> String {
    if workspace.join(SOURCE_TREE_MCP_BINARY).is_file() {
        return SOURCE_TREE_MCP_BINARY.to_owned();
    }
    let installed_sibling = std::env::current_exe()
        .ok()
        .and_then(|current| current.parent().map(Path::to_path_buf))
        .map(|dir| dir.join(binary_file_name(MCP_BINARY_NAME)))
        .filter(|candidate| candidate.is_file());
    match installed_sibling {
        Some(sibling) => sibling.to_string_lossy().into_owned(),
        None => MCP_BINARY_NAME.to_owned(),
    }
}

fn binary_file_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

fn detected_languages() -> Vec<String> {
    let mut has_rust = false;
    let mut has_typescript = false;
    let mut has_python = false;

    for config in LanguageRegistry::default().configs() {
        match config.id {
            "rust" => has_rust = true,
            "typescript" | "tsx_js" => has_typescript = true,
            "python" => has_python = true,
            _ => {}
        }
    }

    let mut languages = Vec::new();
    if has_rust {
        languages.push("Rust".to_owned());
    }
    if has_typescript {
        languages.push("TypeScript/JavaScript".to_owned());
    }
    if has_python {
        languages.push("Python".to_owned());
    }

    languages
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use aether_core::AETHER_AGENT_SCHEMA_VERSION;
    use tempfile::tempdir;

    use super::{AgentPlatform, InitAgentOptions, run_init_agent};

    #[test]
    fn init_agent_creates_expected_files_for_all_platforms() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();

        write_config_with_embeddings(workspace, true);

        let outcome = run_init_agent(
            workspace,
            InitAgentOptions {
                platform: AgentPlatform::All,
                force: false,
            },
        )
        .expect("init-agent should succeed");

        assert_eq!(outcome.exit_code(), 0);
        assert!(outcome.skipped_existing_files.is_empty());
        assert!(workspace.join("CLAUDE.md").exists());
        assert!(workspace.join(".codex-instructions").exists());
        assert!(workspace.join(".cursor/rules").exists());
        assert!(workspace.join("AGENTS.md").exists());
        assert!(workspace.join(".mcp.json").exists());
        assert!(workspace.join(".claude/commands/audit.md").exists());
        assert!(workspace.join(".claude/commands/refactor.md").exists());
        assert!(workspace.join(".claude/commands/refactor-deep.md").exists());
        assert!(workspace.join(".claude/commands/audit-report.md").exists());
        assert!(workspace.join(".claude/commands/audit-changes.md").exists());
        assert!(workspace.join(".claude/commands/scan.md").exists());
        assert!(workspace.join("scripts/scan_all.sh").exists());
        assert!(
            workspace
                .join(".agents/skills/aether-context/SKILL.md")
                .exists()
        );
    }

    #[test]
    fn init_agent_claude_platform_ships_scan_assets_and_marks_script_executable() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_config_with_embeddings(workspace, false);

        let outcome = run_init_agent(
            workspace,
            InitAgentOptions {
                platform: AgentPlatform::Claude,
                force: false,
            },
        )
        .expect("init-agent claude should succeed");
        assert!(
            outcome
                .written_files
                .contains(&std::path::PathBuf::from(".claude/commands/scan.md"))
        );
        assert!(
            outcome
                .written_files
                .contains(&std::path::PathBuf::from("scripts/scan_all.sh"))
        );
        let command = fs::read_to_string(workspace.join(".claude/commands/scan.md"))
            .expect("read scan command");
        assert!(command.contains("aether_sir_inject"));
        let mcp = fs::read_to_string(workspace.join(".mcp.json")).expect("read .mcp.json");
        let parsed: serde_json::Value = serde_json::from_str(&mcp).expect("valid json");
        let command = parsed["mcpServers"]["aether"]["command"]
            .as_str()
            .expect("claude platform must register the MCP server");
        assert!(
            !command.starts_with("./target/"),
            "a workspace without a build tree must not be pointed at one: {command}"
        );
        assert!(
            command.ends_with("aether-mcp") || command.ends_with("aether-mcp.exe"),
            "unexpected MCP command: {command}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(workspace.join("scripts/scan_all.sh"))
                .expect("stat script")
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "scan_all.sh should be executable");
        }

        // Codex/Cursor platforms never write the Claude command set.
        let temp = tempdir().expect("tempdir");
        write_config_with_embeddings(temp.path(), false);
        run_init_agent(
            temp.path(),
            InitAgentOptions {
                platform: AgentPlatform::Codex,
                force: false,
            },
        )
        .expect("init-agent codex should succeed");
        assert!(!temp.path().join(".claude/commands/scan.md").exists());
        assert!(!temp.path().join("scripts/scan_all.sh").exists());
    }

    #[test]
    fn init_agent_merges_aether_into_an_existing_mcp_json() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::write(
            workspace.join(".mcp.json"),
            "{\n  \"mcpServers\": {\n    \"other\": { \"command\": \"other-mcp\" }\n  }\n}\n",
        )
        .expect("seed .mcp.json");
        let outcome = run_init_agent(
            workspace,
            InitAgentOptions {
                platform: AgentPlatform::Claude,
                force: false,
            },
        )
        .expect("init-agent should succeed");
        assert!(
            outcome
                .written_files
                .contains(&std::path::PathBuf::from(".mcp.json")),
            "an existing .mcp.json without aether must be updated, not skipped"
        );
        let merged: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(workspace.join(".mcp.json")).unwrap())
                .expect("valid json");
        assert!(merged["mcpServers"]["other"]["command"].is_string());
        assert!(merged["mcpServers"]["aether"]["command"].is_string());

        // A second run leaves the file alone and reports it as existing.
        let again = run_init_agent(
            workspace,
            InitAgentOptions {
                platform: AgentPlatform::Claude,
                force: false,
            },
        )
        .expect("second init-agent should succeed");
        assert!(
            again
                .skipped_existing_files
                .contains(&std::path::PathBuf::from(".mcp.json"))
        );

        // --force refreshes the aether entry only; other registrations survive.
        fs::write(
            workspace.join(".mcp.json"),
            "{\n  \"mcpServers\": {\n    \"other\": { \"command\": \"other-mcp\" },\n    \"aether\": { \"command\": \"stale\" }\n  }\n}\n",
        )
        .expect("seed stale .mcp.json");
        let forced = run_init_agent(
            workspace,
            InitAgentOptions {
                platform: AgentPlatform::Claude,
                force: true,
            },
        )
        .expect("forced init-agent should succeed");
        assert!(
            forced
                .written_files
                .contains(&std::path::PathBuf::from(".mcp.json"))
        );
        let refreshed: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(workspace.join(".mcp.json")).unwrap())
                .expect("valid json");
        assert_eq!(refreshed["mcpServers"]["other"]["command"], "other-mcp");
        assert_ne!(refreshed["mcpServers"]["aether"]["command"], "stale");

        // A repeated --force with an already-current entry is a success, not a skip.
        let repeated = run_init_agent(
            workspace,
            InitAgentOptions {
                platform: AgentPlatform::Claude,
                force: true,
            },
        )
        .expect("repeated forced init-agent should succeed");
        assert!(
            !repeated
                .skipped_existing_files
                .contains(&std::path::PathBuf::from(".mcp.json")),
            "an up-to-date .mcp.json must not fail a forced regeneration"
        );
        assert_eq!(repeated.exit_code(), 0);
    }

    #[test]
    fn init_agent_omp_platform_writes_agents_md_and_mcp_json_only() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();

        write_config_with_embeddings(workspace, false);

        let outcome = run_init_agent(
            workspace,
            InitAgentOptions {
                platform: AgentPlatform::Omp,
                force: false,
            },
        )
        .expect("init-agent omp should succeed");

        assert_eq!(outcome.exit_code(), 0);
        assert_eq!(
            outcome.written_files,
            vec![
                std::path::PathBuf::from("AGENTS.md"),
                std::path::PathBuf::from(".mcp.json")
            ]
        );
        assert!(!workspace.join("CLAUDE.md").exists());
        let agents = fs::read_to_string(workspace.join("AGENTS.md")).expect("read agents");
        assert!(agents.contains("AETHER Code Intelligence"));
        assert!(agents.contains("aether_verify"));
        let mcp = fs::read_to_string(workspace.join(".mcp.json")).expect("read mcp");
        let parsed: serde_json::Value = serde_json::from_str(&mcp).expect("valid json");
        assert!(parsed["mcpServers"]["aether"]["command"].is_string());
    }

    #[test]
    fn init_agent_skips_existing_files_without_force() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();

        write_config_with_embeddings(workspace, true);
        fs::write(workspace.join("CLAUDE.md"), "custom content\n").expect("seed claude file");

        let outcome = run_init_agent(
            workspace,
            InitAgentOptions {
                platform: AgentPlatform::All,
                force: false,
            },
        )
        .expect("init-agent should succeed with skips");

        assert_eq!(outcome.exit_code(), 2);
        assert!(
            outcome
                .skipped_existing_files
                .contains(&std::path::PathBuf::from("CLAUDE.md"))
        );
        assert_eq!(
            fs::read_to_string(workspace.join("CLAUDE.md")).expect("read claude"),
            "custom content\n"
        );
        assert!(workspace.join(".codex-instructions").exists());
        assert!(workspace.join(".claude/commands/audit.md").exists());
        assert!(workspace.join(".claude/commands/refactor.md").exists());
        assert!(workspace.join(".claude/commands/audit-report.md").exists());
    }

    #[test]
    fn init_agent_overwrites_existing_files_with_force() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();

        write_config_with_embeddings(workspace, true);
        fs::write(workspace.join("CLAUDE.md"), "custom content\n").expect("seed claude file");

        let outcome = run_init_agent(
            workspace,
            InitAgentOptions {
                platform: AgentPlatform::Claude,
                force: true,
            },
        )
        .expect("init-agent should overwrite");

        assert_eq!(outcome.exit_code(), 0);
        assert!(outcome.skipped_existing_files.is_empty());
        let claude = fs::read_to_string(workspace.join("CLAUDE.md")).expect("read claude");
        assert!(claude.contains("AETHER Code Intelligence"));
    }

    #[test]
    fn generated_claude_contains_schema_version() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();

        write_config_with_embeddings(workspace, true);

        run_init_agent(
            workspace,
            InitAgentOptions {
                platform: AgentPlatform::Claude,
                force: false,
            },
        )
        .expect("init-agent should succeed");

        let claude = fs::read_to_string(workspace.join("CLAUDE.md")).expect("read claude");
        assert!(claude.contains(&format!(
            "Agent Schema Version: {}",
            AETHER_AGENT_SCHEMA_VERSION
        )));
    }

    #[test]
    fn generated_claude_contains_audit_workflow() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();

        write_config_with_embeddings(workspace, true);

        run_init_agent(
            workspace,
            InitAgentOptions {
                platform: AgentPlatform::Claude,
                force: false,
            },
        )
        .expect("init-agent should succeed");

        let claude = fs::read_to_string(workspace.join("CLAUDE.md")).expect("read claude");
        assert!(claude.contains("## Audit Workflow"));
    }

    #[test]
    fn generated_commands_contain_expected_content() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_config_with_embeddings(workspace, true);
        run_init_agent(
            workspace,
            InitAgentOptions {
                platform: AgentPlatform::Claude,
                force: false,
            },
        )
        .expect("init-agent should succeed");

        let audit = fs::read_to_string(workspace.join(".claude/commands/audit.md"))
            .expect("read audit command");
        assert!(audit.contains("argument-hint:"));
        assert!(audit.contains("aether_health"));
        assert!(audit.contains("ARITHMETIC"));
        assert!(audit.contains("aether_audit_submit"));

        let refactor = fs::read_to_string(workspace.join(".claude/commands/refactor.md"))
            .expect("read refactor command");
        assert!(refactor.contains("aether_suggest_trait_split"));
        assert!(refactor.contains("aether_refactor_prep"));

        let refactor_deep = fs::read_to_string(workspace.join(".claude/commands/refactor-deep.md"))
            .expect("read refactor-deep command");
        assert!(refactor_deep.contains("argument-hint: [file-path]"));
        assert!(refactor_deep.contains("aether_sir_inject"));
        assert!(refactor_deep.contains("\"file_filter\": \"$ARGUMENTS\""));
        assert!(refactor_deep.contains("aether_refactor_prep"));
        assert!(refactor_deep.contains("refactor-prep"));
        assert!(refactor_deep.contains("aether_verify_intent"));
        assert!(refactor_deep.contains("verify-intent"));

        let report = fs::read_to_string(workspace.join(".claude/commands/audit-report.md"))
            .expect("read audit-report command");
        assert!(report.contains("aether_audit_report"));
        assert!(report.contains("aether_recall"));

        let changes = fs::read_to_string(workspace.join(".claude/commands/audit-changes.md"))
            .expect("read audit-changes command");
        assert!(changes.contains("argument-hint:"));
        assert!(changes.contains("git diff"));
        assert!(changes.contains("aether_search"));
        assert!(changes.contains("aether_get_sir"));
        assert!(changes.contains("aether_sir_inject"));
    }

    #[test]
    fn generated_content_reflects_embeddings_setting() {
        let with_embeddings = tempdir().expect("tempdir");
        write_config_with_embeddings(with_embeddings.path(), true);
        run_init_agent(
            with_embeddings.path(),
            InitAgentOptions {
                platform: AgentPlatform::Claude,
                force: false,
            },
        )
        .expect("init-agent should succeed");
        let enabled = fs::read_to_string(with_embeddings.path().join("CLAUDE.md"))
            .expect("read claude with embeddings");
        assert!(enabled.contains("semantic search is enabled"));

        let without_embeddings = tempdir().expect("tempdir");
        write_config_with_embeddings(without_embeddings.path(), false);
        run_init_agent(
            without_embeddings.path(),
            InitAgentOptions {
                platform: AgentPlatform::Claude,
                force: false,
            },
        )
        .expect("init-agent should succeed");
        let disabled = fs::read_to_string(without_embeddings.path().join("CLAUDE.md"))
            .expect("read claude without embeddings");
        assert!(disabled.contains("lexical only"));
    }

    fn write_config_with_embeddings(workspace: &Path, embeddings_enabled: bool) {
        fs::create_dir_all(workspace.join(".aether")).expect("create config dir");
        fs::write(
            workspace.join(".aether/config.toml"),
            format!(
                r#"[inference]
provider = "qwen3_local"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true

[embeddings]
enabled = {embeddings_enabled}
provider = "qwen3_local"
vector_backend = "sqlite"

[verify]
commands = ["cargo fmt --all --check", "cargo clippy --workspace -- -D warnings", "cargo test --workspace"]
mode = "host"
"#
            ),
        )
        .expect("write config");
    }
}
