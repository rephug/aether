use super::{ClaudeTemplate, TemplateContext};

/// `AGENTS.md` for Oh My Pi (omp) projects.
///
/// omp discovers `AGENTS.md` at the project root the same way Claude Code discovers
/// `CLAUDE.md`, and the guidance is identical, so the markdown body is shared.
#[derive(Debug, Clone, Copy, Default)]
pub struct OmpAgentsTemplate;

impl OmpAgentsTemplate {
    pub fn render(context: &TemplateContext) -> String {
        ClaudeTemplate::render(context)
    }
}

/// Project-root `.mcp.json` registering the AETHER MCP server for omp (and any other
/// client that reads the `mcpServers` shape).
#[derive(Debug, Clone, Copy, Default)]
pub struct McpJsonTemplate;

impl McpJsonTemplate {
    pub fn render(context: &TemplateContext) -> String {
        let value = serde_json::json!({
            "mcpServers": {
                "aether": {
                    "command": context.mcp_binary_hint,
                    "args": ["--workspace", "."]
                }
            }
        });
        let mut rendered =
            serde_json::to_string_pretty(&value).expect("static MCP config serializes");
        rendered.push('\n');
        rendered
    }
}
