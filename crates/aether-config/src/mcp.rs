use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The tools the compact profile serves: the ones coding agents called in the E18
/// agent comparison (symbol lookup and dependencies made 61% of the calls), plus
/// semantic search, which has no other route in this profile.
pub const COMPACT_MCP_TOOLS: [&str; 6] = [
    "aether_symbol_lookup",
    "aether_search",
    "aether_dependencies",
    "aether_call_chain",
    "aether_sir_context",
    "aether_test_intents",
];

/// `[mcp]`: how `aether-mcp` presents itself to MCP clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct McpConfig {
    /// Which tools `aether-mcp` lists and serves. The `--tools` flag overrides it.
    #[serde(default)]
    pub tools: McpToolProfile,
}

/// Which AETHER tools an MCP server lists and serves.
///
/// Every listed tool's name, description and input schema go into a coding agent's
/// context on every request, whether or not the agent calls the tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpToolProfile {
    /// Every tool.
    #[default]
    Full,
    /// Only [`COMPACT_MCP_TOOLS`], with short descriptions and input schemas.
    Compact,
}

impl McpToolProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Compact => "compact",
        }
    }

    /// Whether this profile serves `tool`. Full serves every tool, so it says yes to
    /// any name.
    pub fn includes(self, tool: &str) -> bool {
        match self {
            Self::Full => true,
            Self::Compact => COMPACT_MCP_TOOLS.contains(&tool),
        }
    }
}

impl fmt::Display for McpToolProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for McpToolProfile {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "full" => Ok(Self::Full),
            "compact" => Ok(Self::Compact),
            other => Err(format!(
                "unknown MCP tool profile `{other}`; expected `full` or `compact`"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{COMPACT_MCP_TOOLS, McpConfig, McpToolProfile};

    #[test]
    fn tool_profile_parses_case_insensitively_and_rejects_unknown_names() {
        assert_eq!("full".parse(), Ok(McpToolProfile::Full));
        assert_eq!(" Compact ".parse(), Ok(McpToolProfile::Compact));
        let err = "tiny".parse::<McpToolProfile>().unwrap_err();
        assert!(err.contains("`tiny`") && err.contains("`compact`"), "{err}");
    }

    #[test]
    fn compact_profile_includes_only_its_tools() {
        for tool in COMPACT_MCP_TOOLS {
            assert!(McpToolProfile::Compact.includes(tool));
        }
        assert!(!McpToolProfile::Compact.includes("aether_health"));
        assert!(McpToolProfile::Full.includes("aether_health"));
    }

    #[test]
    fn mcp_section_reads_tools_and_defaults_to_full() {
        let config: McpConfig = toml::from_str("tools = \"compact\"").expect("parse");
        assert_eq!(config.tools, McpToolProfile::Compact);
        let config: McpConfig = toml::from_str("").expect("parse empty");
        assert_eq!(config.tools, McpToolProfile::Full);
        assert!(toml::from_str::<McpConfig>("tools = \"tiny\"").is_err());
    }
}
