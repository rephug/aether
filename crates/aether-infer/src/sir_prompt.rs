use aether_sir::SirAnnotation;

use crate::SirContext;

const FEW_SHOT_EXAMPLES: &str = r#"Few-shot examples:
1) function
{"intent":"Parse a line-delimited JSON event from a byte stream and return the decoded event or EOF state","inputs":["buffer: pending unread bytes","reader: async byte source"],"outputs":["Ok(Some(Event)) when a complete event was decoded","Ok(None) when EOF is reached cleanly"],"side_effects":["Consumes bytes from reader","Mutates internal buffer"],"dependencies":["serde_json","tokio::io::AsyncReadExt"],"error_modes":["Malformed JSON payload returns error","I/O read failures propagate via ?"],"confidence":0.87}

2) struct
{"intent":"Configuration object that groups retry and timeout knobs so callers can share consistent network policy","inputs":[],"outputs":[],"side_effects":[],"dependencies":["std::time::Duration"],"error_modes":[],"confidence":0.91}

3) test
{"intent":"Verifies that reconnect backoff resets after a successful request to avoid compounding delay across healthy periods","inputs":["mock clock","flaky transport stub"],"outputs":["Pass when next delay equals initial backoff after success"],"side_effects":["Advances simulated clock","Mutates retry state in fixture"],"dependencies":["retry::BackoffPolicy","transport::MockClient"],"error_modes":["Assertion failure when delay is not reset"],"confidence":0.9}"#;

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

pub fn build_sir_prompt_for_kind(symbol_text: &str, context: &SirContext) -> String {
    format!(
        "You are generating a Leaf SIR annotation.\n\
Respond with STRICT JSON only (no markdown, no prose) and exactly these fields: \
intent (string), inputs (array of string), outputs (array of string), side_effects (array of string), dependencies (array of string), error_modes (array of string), confidence (number in [0.0,1.0]).\n\
Do not add any extra keys.\n\n\
Context:\n\
- language: {}\n\
- file_path: {}\n\
- qualified_name: {}\n\n\
{}{}\n\n\
Symbol text:\n{}",
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
    /// File-level rollup intent from triage pass
    pub file_intent: Option<String>,
    /// Intents of neighboring symbols in the same file
    pub neighbor_intents: Vec<(String, String)>,
    /// The triage-pass SIR to improve upon
    pub baseline_sir: Option<SirAnnotation>,
    /// Human-readable explanation of why this symbol was selected for deep pass
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

    prompt.push_str(
        "\n\nRespond with STRICT JSON only. Exactly these fields: intent, inputs, outputs,\n\
side_effects, dependencies, error_modes, confidence.",
    );
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
