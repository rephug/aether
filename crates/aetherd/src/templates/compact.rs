use super::{TemplateContext, languages_inline, verify_commands_markdown};

/// Agent guidance for the compact MCP tool profile (`aether-mcp --tools compact`),
/// written as `CLAUDE.md`, `AGENTS.md`, `.codex-instructions` and `.cursor/rules`.
///
/// It sits in the agent's context on every request next to the tool definitions the
/// server already lists, so it only says how the tools fit together.
#[derive(Debug, Clone, Copy, Default)]
pub struct CompactGuidanceTemplate;

impl CompactGuidanceTemplate {
    pub fn render(context: &TemplateContext) -> String {
        let search = if context.embeddings_enabled {
            "`aether_search` with mode `hybrid` (by meaning)"
        } else {
            "`aether_search` (lexical only: embeddings are off)"
        };
        format!(
            "# AETHER Code Intelligence\n\nThis project attaches AETHER's MCP server with the compact tool profile (`--tools compact`). Its tools answer what text search can't: resolved callers and callees, what a symbol does, and which tests exercise it.\n\n## Agent Schema Version: {}\n\n## Using the Tools\n- Get a `symbol_id` from `aether_symbol_lookup` (by name or file path) or {search}, then pass it to the other tools.\n- Before changing a function's signature or behavior, call `aether_dependencies` for its callers; `aether_call_chain` shows what it reaches transitively.\n- `aether_sir_context` summarizes a symbol (intent, side effects, error modes) with its callers, callees and related tests; `aether_test_intents` lists what the tests in a file assert.\n- The index can lag your latest edits: when it disagrees with the files, trust the files.\n\n## Languages\n- {}\n\n## Verify Commands\n{}\n",
            context.agent_schema_version,
            languages_inline(context),
            verify_commands_markdown(context),
        )
    }
}
