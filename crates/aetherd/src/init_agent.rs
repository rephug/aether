use std::fs;
use std::path::{Path, PathBuf};

use aether_config::{AetherConfig, config_path, load_workspace_config};
use aether_core::AETHER_AGENT_SCHEMA_VERSION;
use aether_parse::LanguageRegistry;
use anyhow::{Context, Result};
use clap::ValueEnum;

use crate::templates::{
    ClaudeTemplate, CodexInstructionsTemplate, CursorRulesTemplate, SkillTemplate, TemplateContext,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum AgentPlatform {
    Claude,
    Codex,
    Cursor,
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

    let context = build_template_context(&config);
    let files = files_for_platform(options.platform, &context);

    let mut written_files = Vec::new();
    let mut skipped_existing_files = Vec::new();

    for file in files {
        let absolute_path = workspace.join(&file.relative_path);
        if absolute_path.exists() && !options.force {
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
}

fn files_for_platform(platform: AgentPlatform, context: &TemplateContext) -> Vec<GeneratedFile> {
    let mut files = Vec::new();

    if matches!(platform, AgentPlatform::Claude | AgentPlatform::All) {
        files.push(GeneratedFile {
            relative_path: PathBuf::from("CLAUDE.md"),
            content: ClaudeTemplate::render(context),
        });
        files.push(GeneratedFile {
            relative_path: PathBuf::from(".agents/skills/aether-context/SKILL.md"),
            content: SkillTemplate::render(context),
        });
    }

    if matches!(platform, AgentPlatform::Codex | AgentPlatform::All) {
        files.push(GeneratedFile {
            relative_path: PathBuf::from(".codex-instructions"),
            content: CodexInstructionsTemplate::render(context),
        });
    }

    if matches!(platform, AgentPlatform::Cursor | AgentPlatform::All) {
        files.push(GeneratedFile {
            relative_path: PathBuf::from(".cursor/rules"),
            content: CursorRulesTemplate::render(context),
        });
    }

    files
}

fn build_template_context(config: &AetherConfig) -> TemplateContext {
    TemplateContext {
        languages: detected_languages(),
        verify_commands: config.verify.commands.clone(),
        embeddings_enabled: config.embeddings.enabled,
        inference_provider: config.inference.provider.as_str().to_owned(),
        agent_schema_version: AETHER_AGENT_SCHEMA_VERSION,
        mcp_binary_hint: default_mcp_binary_hint(),
    }
}

fn default_mcp_binary_hint() -> String {
    "./target/debug/aether-mcp".to_owned()
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
        assert!(
            workspace
                .join(".agents/skills/aether-context/SKILL.md")
                .exists()
        );
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
provider = "mock"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true

[embeddings]
enabled = {embeddings_enabled}
provider = "mock"
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
