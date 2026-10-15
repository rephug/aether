//! Tool profiles: which tools the server lists and serves ([`McpToolProfile`]).
//!
//! An MCP client puts every listed tool's name, description and input schema into the
//! model's context on each request, whether or not the model calls the tool. The full
//! profile lists every tool as declared in `router.rs`. The compact profile lists only
//! [`aether_config::COMPACT_MCP_TOOLS`], each with the short description and input
//! schema below. A compact schema is a narrower view of the same request type: it
//! advertises the parameters a coding agent needs, and a parameter it leaves out keeps
//! its default. Compact tools also drop their output schema, which a model never needs
//! to call them; results are unchanged.

use std::sync::Arc;

use aether_config::McpToolProfile;
use rmcp::ErrorData as McpError;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::JsonObject;
use serde_json::{Value, json};

use super::AetherMcpServer;

/// A compact tool's description and input schema.
pub(crate) struct CompactToolDefinition {
    pub(crate) description: &'static str,
    pub(crate) input_schema: JsonObject,
}

/// The compact definition of `name`, or `None` for a tool outside the compact profile.
pub(crate) fn compact_tool_definition(name: &str) -> Option<CompactToolDefinition> {
    let (description, properties, required): (&str, Value, &[&str]) = match name {
        "aether_symbol_lookup" => (
            "Find symbols by name or file path (lexical). Returns each match's symbol_id, which the other aether tools take, with its qualified_name, file_path and kind.",
            json!({
                "query": {"type": "string", "description": "Name fragment or file path"},
                "limit": {"type": "integer", "description": "Default 20"},
            }),
            &["query"],
        ),
        "aether_search" => (
            "Search symbols by meaning (mode semantic or hybrid) or by name (lexical, the default). mode_used shows a fallback to lexical.",
            json!({
                "query": {"type": "string"},
                "mode": {"type": "string", "enum": ["lexical", "semantic", "hybrid"]},
                "limit": {"type": "integer", "description": "Default 20"},
            }),
            &["query"],
        ),
        "aether_dependencies" => (
            "Callers of a symbol and the symbols it calls, resolved across files. For a type, combines the callers and callees of its methods.",
            json!({
                "symbol_id": {"type": "string"},
            }),
            &["symbol_id"],
        ),
        "aether_call_chain" => (
            "What a symbol calls, transitively, grouped by depth (level 1: direct callees).",
            json!({
                "symbol_id": {"type": "string"},
                "qualified_name": {"type": "string", "description": "Instead of symbol_id"},
                "max_depth": {"type": "integer", "description": "Default 3, max 10"},
            }),
            &[],
        ),
        "aether_sir_context" => (
            "Markdown briefing on a symbol, cut to max_tokens: source, SIR (intent, side effects, error modes), callers and callees, related tests, health.",
            json!({
                "symbol": {"type": "string", "description": "symbol_id or qualified name"},
                "max_tokens": {"type": "integer", "description": "Default 8000"},
                "include_layers": {
                    "type": "array",
                    "items": {"type": "string", "enum": ["source", "sir", "graph", "tests", "health", "reasoning"]},
                },
            }),
            &["symbol"],
        ),
        "aether_test_intents" => (
            "Behaviors that tests assert, for a file containing tests or one test's symbol_id.",
            json!({
                "file": {"type": "string", "description": "Workspace-relative path"},
                "symbol_id": {"type": "string", "description": "A test's symbol_id"},
            }),
            &[],
        ),
        _ => return None,
    };

    let mut input_schema = JsonObject::new();
    input_schema.insert("type".to_owned(), json!("object"));
    input_schema.insert("properties".to_owned(), properties);
    if !required.is_empty() {
        input_schema.insert("required".to_owned(), json!(required));
    }
    Some(CompactToolDefinition {
        description,
        input_schema,
    })
}

impl AetherMcpServer {
    /// Every tool the server can serve, as declared in `router.rs`.
    pub(crate) fn full_tool_router() -> ToolRouter<Self> {
        let tool_router = Self::tool_router();
        #[cfg(feature = "verification")]
        let tool_router =
            tool_router.with_route((Self::aether_verify_tool_attr(), Self::aether_verify));
        tool_router
    }

    /// The tools `profile` lists and serves.
    pub(crate) fn tool_router_for(profile: McpToolProfile) -> ToolRouter<Self> {
        let full = Self::full_tool_router();
        if profile == McpToolProfile::Full {
            return full;
        }

        let mut compact = ToolRouter::new();
        for mut route in full {
            let Some(definition) = compact_tool_definition(route.name()) else {
                continue;
            };
            route.attr.description = Some(definition.description.into());
            route.attr.input_schema = Arc::new(definition.input_schema);
            route.attr.output_schema = None;
            compact.add_route(route);
        }
        compact
    }

    /// The error for a call to `tool` when the active profile doesn't serve it but the
    /// full profile does; `None` for a served tool or a name no profile knows.
    pub(crate) fn tool_outside_profile_error(&self, tool: &str) -> Option<McpError> {
        if self.tool_router.has_route(tool) || !Self::full_tool_router().has_route(tool) {
            return None;
        }
        let mut served = self
            .tool_router
            .list_all()
            .into_iter()
            .map(|served| served.name)
            .collect::<Vec<_>>();
        served.sort();
        Some(McpError::invalid_params(
            format!(
                "{tool} is not available: aether-mcp is serving the {profile} tool profile ({tools}). Start aether-mcp with --tools full, or set tools = \"full\" under [mcp] in .aether/config.toml, to use every tool.",
                profile = self.tool_profile,
                tools = served.join(", "),
            ),
            None,
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use aether_config::{COMPACT_MCP_TOOLS, McpToolProfile};
    use rmcp::model::{JsonObject, Tool};
    use serde_json::{Value, json};

    use super::{AetherMcpServer, compact_tool_definition};

    fn required(schema: &JsonObject) -> BTreeSet<String> {
        schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|name| name.as_str().map(str::to_owned))
            .collect()
    }

    fn listed(profile: McpToolProfile) -> Vec<Tool> {
        let mut tools = AetherMcpServer::tool_router_for(profile).list_all();
        tools.sort_by(|left, right| left.name.cmp(&right.name));
        tools
    }

    /// What `tools/list` puts in front of the model: the full tool objects, and only
    /// the name, description and input schema that clients send as tool definitions.
    fn listed_bytes(tools: &[Tool]) -> (usize, usize) {
        let list = serde_json::to_string(&json!({ "tools": tools })).expect("serialize");
        let definitions = tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.input_schema,
                })
            })
            .collect::<Vec<_>>();
        let definitions = serde_json::to_string(&definitions).expect("serialize");
        (list.len(), definitions.len())
    }

    #[test]
    fn every_compact_tool_has_a_definition_and_a_full_route() {
        let full = AetherMcpServer::full_tool_router();
        for name in COMPACT_MCP_TOOLS {
            assert!(full.has_route(name), "{name} is not a tool");
            assert!(
                compact_tool_definition(name).is_some(),
                "{name} has no definition"
            );
        }
        assert!(compact_tool_definition("aether_health").is_none());
    }

    /// Each compact schema advertises only request fields, with their types, and
    /// still requires every field the request type requires.
    #[test]
    fn compact_schemas_are_narrower_views_of_the_request_types() {
        let full = listed(McpToolProfile::Full);
        for name in COMPACT_MCP_TOOLS {
            let compact = compact_tool_definition(name)
                .expect("definition")
                .input_schema;
            let declared = &full
                .iter()
                .find(|tool| tool.name == name)
                .expect("full route")
                .input_schema;
            let declared_properties = declared["properties"].as_object().expect("properties");
            let compact_properties = compact["properties"].as_object().expect("properties");
            assert!(
                !compact_properties.is_empty(),
                "{name} advertises no parameters"
            );
            for (property, schema) in compact_properties {
                let declared = declared_properties
                    .get(property)
                    .unwrap_or_else(|| panic!("{name}.{property} is not a request field"));
                if let Some(declared_type) = declared.get("type") {
                    assert_eq!(schema.get("type"), Some(declared_type), "{name}.{property}");
                }
            }
            let compact_required = required(&compact);
            assert!(
                required(declared).is_subset(&compact_required),
                "{name} must require every field its request type requires"
            );
            assert!(
                compact_required
                    .iter()
                    .all(|field| compact_properties.contains_key(field)),
                "{name} requires a field it does not advertise"
            );
        }
    }

    /// The compact profile is for saving context, so no compact definition may be
    /// larger than the one the full profile lists for the same tool.
    #[test]
    fn compact_definitions_are_no_larger_than_full_ones() {
        let full = listed(McpToolProfile::Full);
        for compact in listed(McpToolProfile::Compact) {
            let declared = full
                .iter()
                .find(|tool| tool.name == compact.name)
                .expect("full route");
            let (_, compact_bytes) = listed_bytes(std::slice::from_ref(&compact));
            let (_, declared_bytes) = listed_bytes(std::slice::from_ref(declared));
            assert!(
                compact_bytes <= declared_bytes,
                "{}: compact {compact_bytes} bytes, full {declared_bytes} bytes",
                compact.name
            );
        }
    }

    #[test]
    fn compact_tool_list_is_a_fraction_of_the_full_one() {
        let (full_list, full_definitions) = listed_bytes(&listed(McpToolProfile::Full));
        let (compact_list, compact_definitions) = listed_bytes(&listed(McpToolProfile::Compact));
        eprintln!(
            "tools/list bytes: full {full_list}, compact {compact_list}; name+description+input_schema bytes: full {full_definitions}, compact {compact_definitions}"
        );
        assert!(
            compact_definitions * 10 < full_definitions,
            "compact tool definitions ({compact_definitions} bytes) should be under a tenth of the full ones ({full_definitions} bytes)"
        );
    }
}
