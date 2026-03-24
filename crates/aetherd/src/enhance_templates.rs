use anyhow::Result;
use serde::Serialize;

use crate::enhance::{AssembledEnhanceContext, EnhanceFileContext, EnhanceSymbolContext, TaskType};

const CHARS_PER_TOKEN: usize = 4;

const SECTION_ALLOCATIONS: [(&str, usize); 4] = [
    ("symbols", 45),
    ("files", 15),
    ("architecture", 20),
    ("conventions", 20),
];

#[derive(Debug, Serialize)]
struct JsonPromptDocument<'a> {
    original_prompt: &'a str,
    task_type: TaskType,
    resolved_symbols: String,
    related_files: String,
    architectural_notes: String,
    conventions: String,
}

pub(crate) fn estimate_tokens(content: &str) -> usize {
    content.len() / CHARS_PER_TOKEN
}

pub(crate) fn render_enhanced_document(
    original_prompt: &str,
    context: &AssembledEnhanceContext,
    budget: usize,
    json_document: bool,
) -> Result<String> {
    if json_document {
        return render_json_document(original_prompt, context, budget);
    }

    Ok(render_markdown_document(original_prompt, context, budget))
}

fn render_json_document(
    original_prompt: &str,
    context: &AssembledEnhanceContext,
    budget: usize,
) -> Result<String> {
    let prompt = original_prompt.trim();
    let sections = build_rendered_sections(context);
    let empty_payload = JsonPromptDocument {
        original_prompt: prompt,
        task_type: context.task_type,
        resolved_symbols: String::new(),
        related_files: String::new(),
        architectural_notes: String::new(),
        conventions: String::new(),
    };
    let empty_rendered = serde_json::to_string_pretty(&empty_payload)?;
    let mut section_budget = budget.saturating_sub(estimate_tokens(empty_rendered.as_str()));

    loop {
        let payload = build_json_payload(prompt, context.task_type, &sections, section_budget);
        let rendered = serde_json::to_string_pretty(&payload)?;
        let used_tokens = estimate_tokens(rendered.as_str());
        if used_tokens <= budget || section_budget == 0 {
            return Ok(rendered);
        }

        let next_budget = section_budget
            .saturating_sub(used_tokens.saturating_sub(budget))
            .saturating_sub(1);
        if next_budget == section_budget {
            return Ok(rendered);
        }
        section_budget = next_budget;
    }
}

fn render_markdown_document(
    original_prompt: &str,
    context: &AssembledEnhanceContext,
    budget: usize,
) -> String {
    let header = format!(
        "## Enhanced Prompt\n\n{}\n\n## Relevant Context\n",
        original_prompt.trim()
    );
    let header_tokens = estimate_tokens(header.as_str());
    let available_budget = budget.saturating_sub(header_tokens).max(1);
    let sections = build_rendered_sections(context);
    let trimmed_sections = trim_sections_to_budget(sections, available_budget);

    let mut output = String::new();
    output.push_str(header.as_str());

    let mut has_content = false;
    for (name, content) in trimmed_sections {
        if content.trim().is_empty() {
            continue;
        }
        has_content = true;
        let title = match name {
            "symbols" => "### Target Symbols",
            "files" => "### Related Files",
            "architecture" => "### Architectural Notes",
            "conventions" => "### Conventions",
            _ => "### Context",
        };
        output.push('\n');
        output.push_str(title);
        output.push_str("\n\n");
        output.push_str(content.trim_end());
        output.push('\n');
    }

    if !has_content {
        output.push('\n');
        output.push_str("_No indexed context was available for this prompt._\n");
    }

    output
}

fn build_rendered_sections(context: &AssembledEnhanceContext) -> Vec<(&'static str, String)> {
    let mut sections = Vec::<(&'static str, String)>::new();

    let symbols = render_symbol_section(context.symbols.as_slice());
    if !symbols.is_empty() {
        sections.push(("symbols", symbols));
    }

    let files = render_file_section(context.files.as_slice());
    if !files.is_empty() {
        sections.push(("files", files));
    }

    let architecture = render_string_list(context.architectural_notes.as_slice());
    if !architecture.is_empty() {
        sections.push(("architecture", architecture));
    }

    let conventions = render_string_list(context.conventions.as_slice());
    if !conventions.is_empty() {
        sections.push(("conventions", conventions));
    }

    sections
}

fn build_json_payload<'a>(
    original_prompt: &'a str,
    task_type: TaskType,
    sections: &[(&'static str, String)],
    budget: usize,
) -> JsonPromptDocument<'a> {
    let trimmed_sections = trim_sections_to_budget(sections.to_vec(), budget);

    JsonPromptDocument {
        original_prompt,
        task_type,
        resolved_symbols: section_content(trimmed_sections.as_slice(), "symbols"),
        related_files: section_content(trimmed_sections.as_slice(), "files"),
        architectural_notes: section_content(trimmed_sections.as_slice(), "architecture"),
        conventions: section_content(trimmed_sections.as_slice(), "conventions"),
    }
}

fn section_content(sections: &[(&'static str, String)], name: &str) -> String {
    sections
        .iter()
        .find_map(|(section_name, content)| (*section_name == name).then(|| content.clone()))
        .unwrap_or_default()
}

fn trim_sections_to_budget(
    sections: Vec<(&'static str, String)>,
    budget: usize,
) -> Vec<(&'static str, String)> {
    if budget == 0 {
        return sections
            .into_iter()
            .map(|(name, _)| (name, String::new()))
            .collect();
    }

    let total_tokens = sections
        .iter()
        .map(|(_, content)| estimate_tokens(content.as_str()))
        .sum::<usize>();
    if total_tokens <= budget {
        return sections;
    }

    sections
        .into_iter()
        .map(|(name, content)| {
            let allocation = allocation_for_section(name, budget);
            if estimate_tokens(content.as_str()) <= allocation {
                (name, content)
            } else {
                (name, middle_truncate(content.as_str(), allocation))
            }
        })
        .collect()
}

fn allocation_for_section(name: &str, budget: usize) -> usize {
    let weight = SECTION_ALLOCATIONS
        .iter()
        .find_map(|(section, weight)| (*section == name).then_some(*weight))
        .unwrap_or(25);
    ((budget * weight) / 100).max(1)
}

fn render_symbol_section(symbols: &[EnhanceSymbolContext]) -> String {
    let mut output = String::new();
    for symbol in symbols {
        output.push_str(format_symbol(symbol).as_str());
        output.push('\n');
    }
    output
}

fn format_symbol(symbol: &EnhanceSymbolContext) -> String {
    let mut lines = Vec::<String>::new();
    lines.push(format!(
        "**`{}`** ({} in `{}`)",
        symbol.qualified_name, symbol.kind, symbol.file_path
    ));

    if let Some(intent) = symbol.intent.as_deref() {
        lines.push(format!("- **Intent:** {intent}"));
    }
    if !symbol.inputs.is_empty() {
        lines.push(format!("- **Inputs:** {}", symbol.inputs.join("; ")));
    }
    if !symbol.outputs.is_empty() {
        lines.push(format!("- **Outputs:** {}", symbol.outputs.join("; ")));
    }
    if !symbol.side_effects.is_empty() {
        lines.push(format!(
            "- **Side effects:** {}",
            symbol.side_effects.join("; ")
        ));
    }
    if !symbol.error_modes.is_empty() {
        lines.push(format!(
            "- **Error modes:** {}",
            symbol.error_modes.join("; ")
        ));
    }
    if let Some(score) = symbol.health_score {
        let mut suffix = String::new();
        if !symbol.health_warnings.is_empty() {
            suffix.push(' ');
            suffix.push_str(symbol.health_warnings.join("; ").as_str());
        }
        lines.push(format!("- **Health:** {score}/100{suffix}"));
    }
    if let Some(pass) = symbol.generation_pass.as_deref() {
        lines.push(format!("- **Generation pass:** {pass}"));
    }
    if !symbol.dependencies.is_empty() {
        lines.push(format!(
            "- **Dependencies:** {}",
            symbol.dependencies.join(", ")
        ));
    }
    if !symbol.callers.is_empty() {
        lines.push(format!("- **Callers:** {}", symbol.callers.join(", ")));
    }
    if !symbol.contracts.is_empty() {
        lines.push(format!("- **Contracts:** {}", symbol.contracts.join("; ")));
    }
    if !symbol.drift_warnings.is_empty() {
        lines.push(format!(
            "- **Drift warnings:** {}",
            symbol.drift_warnings.join("; ")
        ));
    }
    if let Some(community_id) = symbol.community_id {
        lines.push(format!("- **Community:** {community_id}"));
    }

    lines.join("\n")
}

fn render_file_section(files: &[EnhanceFileContext]) -> String {
    let mut output = String::new();
    for file in files {
        output.push_str(
            format!(
                "**`{}`** ({} indexed symbols)\n",
                file.file_path, file.symbol_count
            )
            .as_str(),
        );
        for summary in &file.top_symbols {
            output.push_str(format!("- {summary}\n").as_str());
        }
        output.push('\n');
    }
    output
}

fn render_string_list(items: &[String]) -> String {
    items
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn middle_truncate(content: &str, max_tokens: usize) -> String {
    if max_tokens == 0 {
        return String::new();
    }

    let max_chars = max_tokens.saturating_mul(CHARS_PER_TOKEN);
    if content.chars().count() <= max_chars {
        return content.to_owned();
    }

    let marker = "\n[... truncated ...]\n";
    let marker_chars = marker.chars().count();
    if max_chars <= marker_chars + 8 {
        return truncate_by_char_count(marker, max_chars.max(1));
    }

    let prefix_chars = (max_chars - marker_chars) / 2;
    let suffix_chars = max_chars - marker_chars - prefix_chars;
    let prefix = truncate_by_char_count(content, prefix_chars);
    let suffix = take_last_chars(content, suffix_chars);
    format!("{prefix}{marker}{suffix}")
}

fn truncate_by_char_count(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_owned();
    }

    let end = content
        .char_indices()
        .nth(max_chars)
        .map(|(index, _)| index)
        .unwrap_or(content.len());

    content[..end].to_owned()
}

fn take_last_chars(content: &str, max_chars: usize) -> String {
    let total_chars = content.chars().count();
    if total_chars <= max_chars {
        return content.to_owned();
    }

    let start = content
        .char_indices()
        .nth(total_chars.saturating_sub(max_chars))
        .map(|(index, _)| index)
        .unwrap_or(0);

    content[start..].to_owned()
}

#[cfg(test)]
mod tests {
    use super::{estimate_tokens, middle_truncate, render_enhanced_document};
    use crate::enhance::{
        AssembledEnhanceContext, EnhanceFileContext, EnhanceSymbolContext, TaskType,
    };

    #[test]
    fn middle_truncate_counts_multibyte_characters() {
        let content = "prefix ééé very long middle section with café suffix";
        let truncated = middle_truncate(content, 8);

        assert!(truncated.contains("[... truncated ...]"));
        assert!(truncated.is_char_boundary(truncated.len()));
        assert!(truncated.chars().count() <= 32);
    }

    #[test]
    fn render_enhanced_document_keeps_multibyte_sections_valid_utf8() {
        let context = AssembledEnhanceContext {
            task_type: TaskType::BugFix,
            symbols: vec![EnhanceSymbolContext {
                qualified_name: "crate::login::handle_login".to_owned(),
                kind: "function".to_owned(),
                file_path: "src/login.rs".to_owned(),
                intent: Some("Handle résumé parsing for café users.".to_owned()),
                ..EnhanceSymbolContext::default()
            }],
            files: vec![EnhanceFileContext {
                file_path: "src/login.rs".to_owned(),
                symbol_count: 1,
                top_symbols: vec![
                    "`crate::login::handle_login` — Handles naïve retries.".to_owned(),
                ],
            }],
            architectural_notes: vec!["Touches façade and résumé state.".to_owned()],
            conventions: vec!["Preserve UTF-8 handling.".to_owned()],
        };

        let rendered =
            render_enhanced_document("Fix the café login flow", &context, 18, false).unwrap();

        assert!(rendered.is_char_boundary(rendered.len()));
        assert!(rendered.contains("## Enhanced Prompt"));
    }

    #[test]
    fn render_enhanced_document_respects_budget_for_json_output() {
        let context = AssembledEnhanceContext {
            task_type: TaskType::Refactor,
            symbols: vec![EnhanceSymbolContext {
                qualified_name: "crate::auth::refresh_session".to_owned(),
                kind: "function".to_owned(),
                file_path: "src/auth.rs".to_owned(),
                intent: Some(
                    "Refreshes session tokens while coordinating cookie state and retry guards."
                        .to_owned(),
                ),
                side_effects: vec![
                    "Updates cookie jar".to_owned(),
                    "Writes audit log".to_owned(),
                    "Touches session cache".to_owned(),
                ],
                dependencies: vec![
                    "crate::auth::validate_token".to_owned(),
                    "crate::auth::persist_session".to_owned(),
                ],
                callers: vec!["crate::api::login".to_owned()],
                ..EnhanceSymbolContext::default()
            }],
            files: vec![EnhanceFileContext {
                file_path: "src/auth.rs".to_owned(),
                symbol_count: 4,
                top_symbols: vec![
                    "`crate::auth::refresh_session` — Long auth summary that should be trimmed."
                        .to_owned(),
                    "`crate::auth::persist_session` — Another long summary that expands the JSON."
                        .to_owned(),
                ],
            }],
            architectural_notes: vec![
                "Crosses auth, cookie, and cache boundaries with notable coupling.".to_owned(),
                "Recent drift warnings suggest session behavior changed twice this week."
                    .to_owned(),
            ],
            conventions: vec![
                "Preserve session invalidation semantics.".to_owned(),
                "Do not widen auth retry loops.".to_owned(),
            ],
        };

        let rendered =
            render_enhanced_document("Refactor auth refresh flow", &context, 120, true).unwrap();
        let payload: serde_json::Value = serde_json::from_str(rendered.as_str()).unwrap();

        assert!(estimate_tokens(rendered.as_str()) <= 120);
        assert!(payload.get("resolved_symbols").is_some());
        assert!(
            payload
                .get("resolved_symbols")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .contains("[... truncated ...]")
        );
    }
}
