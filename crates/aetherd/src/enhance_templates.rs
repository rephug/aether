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
    resolved_symbols: &'a [EnhanceSymbolContext],
    related_files: &'a [EnhanceFileContext],
    architectural_notes: &'a [String],
    conventions: &'a [String],
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
        let payload = JsonPromptDocument {
            original_prompt,
            task_type: context.task_type,
            resolved_symbols: context.symbols.as_slice(),
            related_files: context.files.as_slice(),
            architectural_notes: context.architectural_notes.as_slice(),
            conventions: context.conventions.as_slice(),
        };
        return serde_json::to_string_pretty(&payload).map_err(Into::into);
    }

    Ok(render_markdown_document(original_prompt, context, budget))
}

fn render_markdown_document(
    original_prompt: &str,
    context: &AssembledEnhanceContext,
    budget: usize,
) -> String {
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

    let header = format!(
        "## Enhanced Prompt\n\n{}\n\n## Relevant Context\n",
        original_prompt.trim()
    );
    let header_tokens = estimate_tokens(header.as_str());
    let available_budget = budget.saturating_sub(header_tokens).max(1);
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

fn trim_sections_to_budget(
    sections: Vec<(&'static str, String)>,
    budget: usize,
) -> Vec<(&'static str, String)> {
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
    if content.len() <= max_chars {
        return content.to_owned();
    }

    let marker = "\n[... truncated ...]\n";
    if max_chars <= marker.len() + 8 {
        return marker.chars().take(max_chars.max(1)).collect::<String>();
    }

    let prefix_chars = (max_chars - marker.len()) / 2;
    let suffix_chars = max_chars - marker.len() - prefix_chars;
    let prefix = truncate_by_char_count(content, prefix_chars);

    let mut suffix_start = content.len();
    let mut remaining = suffix_chars;
    for (index, ch) in content.char_indices().rev() {
        let char_len = ch.len_utf8();
        if remaining < char_len {
            break;
        }
        remaining -= char_len;
        suffix_start = index;
        if remaining == 0 {
            break;
        }
    }

    let suffix = &content[suffix_start..];
    format!("{prefix}{marker}{suffix}")
}

fn truncate_by_char_count(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_owned();
    }

    let mut end = 0usize;
    for (index, ch) in content.char_indices() {
        let next = index + ch.len_utf8();
        if next > max_chars {
            break;
        }
        end = next;
    }

    content[..end].to_owned()
}
