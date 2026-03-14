use aether_sir::SirAnnotation;

use crate::types::SirContext;

const FEW_SHOT_EXAMPLES: &str = r#"Few-shot examples:
1) function
{"intent":"Parse a line-delimited JSON event from a byte stream and return the decoded event or EOF state","inputs":["buffer: pending unread bytes","reader: async byte source"],"outputs":["Ok(Some(Event)) when a complete event was decoded","Ok(None) when EOF is reached cleanly"],"side_effects":["Consumes bytes from reader","Mutates internal buffer"],"dependencies":["serde_json","tokio::io::AsyncReadExt"],"error_modes":["Malformed JSON payload returns error","I/O read failures propagate via ?"],"confidence":0.87}

2) struct
{"intent":"Configuration object that groups retry and timeout knobs so callers can share consistent network policy","inputs":[],"outputs":[],"side_effects":[],"dependencies":["std::time::Duration"],"error_modes":[],"confidence":0.91}

3) test
{"intent":"Verifies that reconnect backoff resets after a successful request to avoid compounding delay across healthy periods","inputs":["mock clock","flaky transport stub"],"outputs":["Pass when next delay equals initial backoff after success"],"side_effects":["Advances simulated clock","Mutates retry state in fixture"],"dependencies":["retry::BackoffPolicy","transport::MockClient"],"error_modes":["Assertion failure when delay is not reset"],"confidence":0.9}

4) trait
{"intent":"Storage abstraction providing typed persistence operations for domain records","inputs":[],"outputs":[],"side_effects":["Persists records to underlying storage backend"],"dependencies":["Record","StorageError"],"error_modes":["Storage backend unavailable","Serialization failure"],"method_dependencies":{"delete":["StorageError"],"load":["Record","StorageError"],"save":["Record","StorageError"]},"confidence":0.91}"#;

fn is_type_definition(kind: &str) -> bool {
    matches!(kind, "struct" | "enum" | "trait" | "type_alias")
}

fn is_function_like(kind: &str) -> bool {
    matches!(kind, "function" | "method")
}

fn is_test_like(context: &SirContext) -> bool {
    context
        .qualified_name
        .to_ascii_lowercase()
        .contains("test_")
        || context.kind.to_ascii_lowercase().contains("test")
}

fn kind_specific_guidance(context: &SirContext) -> String {
    let mut sections = Vec::new();

    if is_type_definition(context.kind.as_str()) {
        sections.push(
            "For type definitions: describe WHY this type exists, not just WHAT it is.\n\
List contained or extended types as dependencies.\n\
Inputs and outputs should be empty arrays for type definitions.\n\
Good intent example: 'Database handle struct that holds a reference-counted pointer to shared database state, enabling multiple owners including a background task'\n\
Bad intent example: 'Define a database structure'"
                .to_owned(),
        );
        sections.push(
            "If this type has methods (trait methods, impl methods): include a \
\"method_dependencies\" field that maps each method name to its specific \
dependencies as an array of strings. The flat \"dependencies\" array must \
still contain the union of all method dependencies. Example:\n\
\"method_dependencies\": {\n\
  \"upsert_symbol\": [\"SymbolRecord\", \"StoreError\"],\n\
  \"read_sir_blob\": [\"StoreError\"]\n\
}\n\
If the type has no methods (pure data struct, fieldless enum), omit \
\"method_dependencies\" entirely."
                .to_owned(),
        );
    }

    if is_function_like(context.kind.as_str()) {
        if context.is_public && context.line_count > 30 {
            sections.push(
                "For complex public methods: enumerate each distinct return path in outputs\n\
(Ok/Err/None) with WHEN each occurs. Describe what each input parameter represents\n\
and its purpose. For error_modes, describe specific failure conditions with\n\
propagation details - follow the ? operator chain.\n\
Good outputs example: ['Ok(Some(Frame)) when a complete frame has been parsed', 'Ok(None) when the remote peer cleanly closed the connection', 'Err when the connection was reset mid-frame']\n\
Bad outputs example: ['crate::Result<Option<Frame>>']"
                    .to_owned(),
            );
        } else {
            sections.push(
                "Even for simple functions, provide a descriptive intent - not just a single word\n\
like 'getter' or 'constructor'. Describe what the function accomplishes and why it exists."
                    .to_owned(),
            );
        }

        if is_test_like(context) {
            sections.push(
                "For test functions: describe what behavior is being verified and under what conditions.\n\
For dependencies, list the production code under test."
                    .to_owned(),
            );
        }
    }

    if sections.is_empty() {
        String::new()
    } else {
        format!("\n\nKind-specific guidance:\n{}", sections.join("\n\n"))
    }
}

fn strict_response_contract(context: &SirContext) -> &'static str {
    if is_type_definition(context.kind.as_str()) {
        "Respond with STRICT JSON only (no markdown, no prose) and these fields: \
intent (string), inputs (array of string), outputs (array of string), side_effects (array of string), dependencies (array of string), error_modes (array of string), confidence (number in [0.0,1.0]).\n\
For type definitions, you may additionally include method_dependencies (object mapping method names to arrays of string) when the type has methods.\n\
Do not add any other keys.\n\n\
"
    } else {
        "Respond with STRICT JSON only (no markdown, no prose) and exactly these fields: \
intent (string), inputs (array of string), outputs (array of string), side_effects (array of string), dependencies (array of string), error_modes (array of string), confidence (number in [0.0,1.0]).\n\
Do not add any extra keys.\n\n\
"
    }
}

fn enriched_response_contract(context: &SirContext) -> &'static str {
    if is_type_definition(context.kind.as_str()) {
        "\n\nRespond with STRICT JSON only. Use fields intent, inputs, outputs,\n\
side_effects, dependencies, error_modes, confidence. For type definitions,\n\
you may additionally include method_dependencies when the type has methods.\n\
Do not add any other keys."
    } else {
        "\n\nRespond with STRICT JSON only. Exactly these fields: intent, inputs, outputs,\n\
side_effects, dependencies, error_modes, confidence."
    }
}

pub fn build_sir_prompt_for_kind(symbol_text: &str, context: &SirContext) -> String {
    format!(
        "You are generating a Leaf SIR annotation.\n\
{}\
Context:\n\
- language: {}\n\
- file_path: {}\n\
- qualified_name: {}\n\n\
{}{}\n\n\
Symbol text:\n{}",
        strict_response_contract(context),
        context.language,
        context.file_path,
        context.qualified_name,
        FEW_SHOT_EXAMPLES,
        kind_specific_guidance(context),
        symbol_text,
    )
}

#[derive(Debug, Clone)]
pub struct SirEnrichmentContext {
    /// File-level rollup intent available to enriched quality passes
    pub file_intent: Option<String>,
    /// Intents of neighboring symbols in the same file
    pub neighbor_intents: Vec<(String, String)>,
    /// The baseline SIR to improve upon
    pub baseline_sir: Option<SirAnnotation>,
    /// Human-readable explanation of why this symbol was selected for enriched analysis
    pub priority_reason: String,
}

fn format_baseline_sir(baseline_sir: &Option<SirAnnotation>) -> String {
    match baseline_sir {
        Some(sir) => serde_json::to_string(sir).unwrap_or_else(|_| "(not available)".to_owned()),
        None => "(not available)".to_owned(),
    }
}

fn format_neighbor_intents(neighbor_intents: &[(String, String)]) -> String {
    if neighbor_intents.is_empty() {
        return "(none)".to_owned();
    }

    neighbor_intents
        .iter()
        .map(|(name, intent)| format!("- {name}: {intent}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_enriched_prompt_core(
    symbol_text: &str,
    context: &SirContext,
    enrichment: &SirEnrichmentContext,
    include_cot: bool,
) -> String {
    let mut prompt = format!(
        "You are improving an existing SIR annotation with deeper analysis.\n\n\
Context:\n\
- language: {}\n\
- file_path: {}\n\
- qualified_name: {}\n\
- priority: {}\n\n\
File purpose: {}\n\n\
Other symbols in this file:\n{}\n\n\
Previous SIR (improve upon this):\n{}{}\n\n\
Symbol text:\n{}\n\n\
Focus on: more specific intent, complete error propagation paths, all side effects\n\
including conditional mutations, correct confidence reflecting your certainty.",
        context.language,
        context.file_path,
        context.qualified_name,
        enrichment.priority_reason,
        enrichment
            .file_intent
            .as_deref()
            .unwrap_or("(not available)"),
        format_neighbor_intents(&enrichment.neighbor_intents),
        format_baseline_sir(&enrichment.baseline_sir),
        kind_specific_guidance(context),
        symbol_text,
    );

    if include_cot {
        prompt.push_str(
            "\n\nBefore outputting JSON, you MUST wrap your analysis inside <thinking> tags:\n\
1. What crucial runtime behaviors are missing from the previous SIR?\n\
2. How do the neighboring symbols dictate how this symbol should be used?\n\
3. What fields will you expand?\n\
\nAfter closing your </thinking> tag, output the final JSON.",
        );
    }

    prompt.push_str(enriched_response_contract(context));
    prompt
}

pub fn build_enriched_sir_prompt(
    symbol_text: &str,
    context: &SirContext,
    enrichment: &SirEnrichmentContext,
) -> String {
    build_enriched_prompt_core(symbol_text, context, enrichment, false)
}

pub fn build_enriched_sir_prompt_with_cot(
    symbol_text: &str,
    context: &SirContext,
    enrichment: &SirEnrichmentContext,
) -> String {
    build_enriched_prompt_core(symbol_text, context, enrichment, true)
}

#[cfg(test)]
mod tests {
    use aether_sir::SirAnnotation;

    use super::*;

    #[test]
    fn build_sir_prompt_for_kind_includes_type_guidance_for_structs() {
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::State".to_owned(),
            priority_score: None,
            kind: "struct".to_owned(),
            is_public: true,
            line_count: 12,
        };
        let prompt = build_sir_prompt_for_kind("pub struct State {}", &context);
        assert!(prompt.contains("For type definitions: describe WHY this type exists"));
        assert!(prompt.contains("\"method_dependencies\" field"));
        assert!(prompt.contains("you may additionally include method_dependencies"));
    }

    #[test]
    fn build_sir_prompt_for_kind_keeps_function_schema_strict() {
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
            priority_score: None,
            kind: "function".to_owned(),
            is_public: true,
            line_count: 8,
        };

        let prompt = build_sir_prompt_for_kind("pub fn run() {}", &context);
        assert!(prompt.contains("exactly these fields"));
        assert!(!prompt.contains("you may additionally include method_dependencies"));
        assert!(!prompt.contains("If this type has methods (trait methods, impl methods)"));
    }

    #[test]
    fn build_sir_prompt_for_kind_includes_complex_public_guidance_for_large_functions() {
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
            priority_score: Some(0.92),
            kind: "function".to_owned(),
            is_public: true,
            line_count: 55,
        };
        let prompt = build_sir_prompt_for_kind("pub fn run() -> Result<()> {}", &context);
        assert!(prompt.contains("For complex public methods"));
        assert!(prompt.contains("Bad outputs example"));
    }

    #[test]
    fn build_sir_prompt_for_kind_includes_test_guidance_for_tests() {
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::test_retry_reset".to_owned(),
            priority_score: None,
            kind: "function".to_owned(),
            is_public: false,
            line_count: 14,
        };
        let prompt = build_sir_prompt_for_kind("fn test_retry_reset() {}", &context);
        assert!(prompt.contains("For test functions: describe what behavior is being verified"));
    }

    #[test]
    fn build_enriched_prompt_includes_context_sections() {
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
            priority_score: Some(0.9),
            kind: "function".to_owned(),
            is_public: true,
            line_count: 60,
        };
        let enrichment = SirEnrichmentContext {
            file_intent: Some("Coordinates request lifecycle".to_owned()),
            neighbor_intents: vec![
                (
                    "demo::parse".to_owned(),
                    "Parses raw bytes into frames".to_owned(),
                ),
                (
                    "demo::flush".to_owned(),
                    "Flushes pending writes".to_owned(),
                ),
            ],
            baseline_sir: Some(SirAnnotation {
                intent: "Baseline".to_owned(),
                inputs: Vec::new(),
                outputs: Vec::new(),
                side_effects: Vec::new(),
                dependencies: Vec::new(),
                error_modes: Vec::new(),
                confidence: 0.6,
                method_dependencies: None,
            }),
            priority_reason: "high PageRank + public method".to_owned(),
        };

        let prompt = build_enriched_sir_prompt("pub fn run() {}", &context, &enrichment);
        assert!(prompt.contains("You are improving an existing SIR annotation"));
        assert!(prompt.contains("high PageRank + public method"));
        assert!(prompt.contains("Other symbols in this file"));
        assert!(prompt.contains("Previous SIR (improve upon this):"));
    }

    #[test]
    fn build_enriched_prompt_allows_method_dependencies_for_type_definitions() {
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::Store".to_owned(),
            priority_score: Some(0.9),
            kind: "trait".to_owned(),
            is_public: true,
            line_count: 60,
        };
        let enrichment = SirEnrichmentContext {
            file_intent: None,
            neighbor_intents: Vec::new(),
            baseline_sir: None,
            priority_reason: "low confidence triage output".to_owned(),
        };

        let prompt = build_enriched_sir_prompt("pub trait Store {}", &context, &enrichment);
        assert!(prompt.contains("you may additionally include method_dependencies"));
        assert!(prompt.contains("\"method_dependencies\" field"));
    }

    #[test]
    fn build_enriched_prompt_with_cot_contains_thinking_instructions() {
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
            priority_score: Some(0.9),
            kind: "function".to_owned(),
            is_public: true,
            line_count: 60,
        };
        let enrichment = SirEnrichmentContext {
            file_intent: None,
            neighbor_intents: Vec::new(),
            baseline_sir: None,
            priority_reason: "low confidence triage output".to_owned(),
        };

        let prompt = build_enriched_sir_prompt_with_cot("pub fn run() {}", &context, &enrichment);
        assert!(prompt.contains("<thinking>"));
        assert!(prompt.contains("After closing your </thinking> tag, output the final JSON."));
    }
}
