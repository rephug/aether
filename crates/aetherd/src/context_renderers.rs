use crate::sir_context::{BudgetStatus, ExportDocument, TargetSection};

pub fn render_xml(document: &ExportDocument) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "<aether_context workspace=\"{}\" generated_at=\"{}\">\n",
        escape_attr(document.project_overview.workspace.as_str()),
        document.generated_at
    ));
    out.push_str(&format!(
        "  <overview symbols=\"{}\" sir_coverage=\"{:.1}\"{}{} />\n",
        document.project_overview.total_symbols,
        document.project_overview.sir_coverage_percent,
        document
            .project_overview
            .health
            .as_ref()
            .map(|health| format!(" health=\"{}\"", health.workspace_score))
            .unwrap_or_default(),
        document
            .project_overview
            .drift
            .as_ref()
            .map(|drift| format!(" drift_findings=\"{}\"", drift.active_findings))
            .unwrap_or_default()
    ));
    for notice in &document.project_overview.notices {
        out.push_str(&format!(
            "  <overview_notice>{}</overview_notice>\n",
            escape_text(notice)
        ));
    }
    for section in &document.target_sections {
        render_xml_target(&mut out, section);
    }
    for notice in &document.notices {
        out.push_str(&format!("  <notice>{}</notice>\n", escape_text(notice)));
    }
    out.push_str(&format!(
        "  <budget used=\"{}\" max=\"{}\">\n",
        document.budget_usage.used_tokens, document.budget_usage.max_tokens
    ));
    for line in &document.budget_usage.layers {
        out.push_str(&format!(
            "    <layer name=\"{}\" suggested=\"{}\" used=\"{}\" status=\"{}\" included=\"{}\" omitted=\"{}\" />\n",
            escape_attr(line.layer.as_str()),
            line.suggested_tokens,
            line.used_tokens,
            budget_status(line.status),
            line.included_items,
            line.omitted_items
        ));
    }
    out.push_str("  </budget>\n");
    out.push_str("</aether_context>\n");
    out
}

pub fn render_compact(document: &ExportDocument) -> String {
    let target = document
        .target_sections
        .first()
        .map(|section| section.target_label.as_str())
        .unwrap_or("workspace");
    let symbol_count = document
        .target_sections
        .iter()
        .map(|section| section.symbols.len())
        .sum::<usize>();
    let health = document
        .project_overview
        .health
        .as_ref()
        .map(|value| value.workspace_score.to_string())
        .unwrap_or_else(|| "n/a".to_owned());

    let mut out = String::new();
    out.push_str(&format!("=== AETHER Context: {target} ===\n"));
    out.push_str(&format!(
        "Budget: {}/{} | Symbols: {} | Health: {}\n",
        document.budget_usage.used_tokens, document.budget_usage.max_tokens, symbol_count, health
    ));
    if !document.project_overview.notices.is_empty() {
        out.push_str(&format!(
            "Overview: {}\n",
            document.project_overview.notices.join(" | ")
        ));
    }
    for section in &document.target_sections {
        render_compact_target(&mut out, section);
    }
    if !document.notices.is_empty() {
        out.push_str(&format!("Notices: {}\n", document.notices.join(" | ")));
    }
    out.push_str(&format!(
        "BudgetLayers: {}\n",
        document
            .budget_usage
            .layers
            .iter()
            .map(|line| format!(
                "{}={}/{}:{}",
                line.layer,
                line.used_tokens,
                line.suggested_tokens,
                budget_status(line.status)
            ))
            .collect::<Vec<_>>()
            .join(" | ")
    ));
    out
}

fn render_xml_target(out: &mut String, section: &TargetSection) {
    out.push_str(&format!(
        "  <target kind=\"{}\" label=\"{}\"{}{}>\n",
        escape_attr(section.target_kind.as_str()),
        escape_attr(section.target_label.as_str()),
        section
            .file_path
            .as_ref()
            .map(|path| format!(" path=\"{}\"", escape_attr(path.as_str())))
            .unwrap_or_default(),
        section
            .language
            .as_ref()
            .map(|language| format!(" language=\"{}\"", escape_attr(language.as_str())))
            .unwrap_or_default()
    ));
    if let Some(file_sir) = &section.file_sir {
        out.push_str("    <file_sir>\n");
        out.push_str(&format!(
            "      <intent><![CDATA[{}]]></intent>\n",
            escape_cdata(file_sir.intent.as_str())
        ));
        render_xml_list(
            out,
            "exports",
            "item",
            file_sir.exports.iter().map(String::as_str),
        );
        render_xml_list(
            out,
            "side_effects",
            "item",
            file_sir.side_effects.iter().map(String::as_str),
        );
        render_xml_list(
            out,
            "dependencies",
            "item",
            file_sir.dependencies.iter().map(String::as_str),
        );
        render_xml_list(
            out,
            "error_modes",
            "item",
            file_sir.error_modes.iter().map(String::as_str),
        );
        out.push_str(&format!(
            "      <meta confidence=\"{:.2}\" symbol_count=\"{}\" />\n",
            file_sir.confidence, file_sir.symbol_count
        ));
        out.push_str("    </file_sir>\n");
    }
    for symbol in &section.symbols {
        out.push_str(&format!(
            "    <symbol name=\"{}\" kind=\"{}\" file=\"{}\" language=\"{}\"{}>\n",
            escape_attr(symbol.qualified_name.as_str()),
            escape_attr(symbol.kind.as_str()),
            escape_attr(symbol.file_path.as_str()),
            escape_attr(symbol.language.as_str()),
            symbol
                .staleness_score
                .map(|value| format!(" staleness=\"{value:.2}\""))
                .unwrap_or_default()
        ));
        out.push_str("      <sir>\n");
        out.push_str(&format!(
            "        <intent><![CDATA[{}]]></intent>\n",
            escape_cdata(symbol.intent.as_str())
        ));
        render_xml_list(
            out,
            "behavior",
            "item",
            symbol.behavior.iter().map(String::as_str),
        );
        if let Some(status) = &symbol.sir_status {
            out.push_str(&format!(
                "        <status>{}</status>\n",
                escape_text(status.as_str())
            ));
        }
        out.push_str("      </sir>\n");
        out.push_str("    </symbol>\n");
    }
    if let Some(source) = &section.source {
        out.push_str(&format!(
            "    <source language=\"{}\"><![CDATA[{}]]></source>\n",
            escape_attr(source.language.as_str()),
            escape_cdata(source.content.as_str())
        ));
    }
    if !section.immediate_graph.is_empty() || !section.broader_graph.is_empty() {
        out.push_str("    <graph>\n");
        for neighbor in &section.immediate_graph {
            out.push_str(&format!(
                "      <neighbor relationship=\"{}\" name=\"{}\" file=\"{}\" depth=\"{}\"><![CDATA[{}]]></neighbor>\n",
                escape_attr(neighbor.relationship.as_str()),
                escape_attr(neighbor.qualified_name.as_str()),
                escape_attr(neighbor.file_path.as_str()),
                neighbor.depth,
                escape_cdata(neighbor.intent_summary.as_str())
            ));
        }
        for neighbor in &section.broader_graph {
            out.push_str(&format!(
                "      <neighbor relationship=\"{}\" name=\"{}\" file=\"{}\" depth=\"{}\"><![CDATA[{}]]></neighbor>\n",
                escape_attr(neighbor.relationship.as_str()),
                escape_attr(neighbor.qualified_name.as_str()),
                escape_attr(neighbor.file_path.as_str()),
                neighbor.depth,
                escape_cdata(neighbor.intent_summary.as_str())
            ));
        }
        out.push_str("    </graph>\n");
    }
    if !section.coupling.is_empty() {
        out.push_str("    <coupling>\n");
        for pair in &section.coupling {
            out.push_str(&format!(
                "      <pair file=\"{}\" score=\"{:.2}\" />\n",
                escape_attr(pair.file_path.as_str()),
                pair.fused_score,
            ));
        }
        out.push_str("    </coupling>\n");
    }
    if !section.tests.is_empty() {
        out.push_str("    <tests>\n");
        for guard in &section.tests {
            out.push_str(&format!(
                "      <guard name=\"{}\"><![CDATA[{}]]></guard>\n",
                escape_attr(guard.test_name.as_str()),
                escape_cdata(guard.description.as_str())
            ));
        }
        out.push_str("    </tests>\n");
    }
    if !section.memory.is_empty() {
        out.push_str("    <memory>\n");
        for note in &section.memory {
            out.push_str(&format!(
                "      <note source_type=\"{}\" created_at=\"{}\"><![CDATA[{}]]></note>\n",
                escape_attr(note.source_type.as_str()),
                note.created_at,
                escape_cdata(note.first_line.as_str())
            ));
        }
        out.push_str("    </memory>\n");
    }
    if let Some(health) = &section.health {
        out.push_str("    <health>\n");
        out.push_str(&format!(
            "      <summary><![CDATA[{}]]></summary>\n",
            escape_cdata(health.summary.as_str())
        ));
        render_xml_list(
            out,
            "warnings",
            "warning",
            health.warnings.iter().map(String::as_str),
        );
        out.push_str("    </health>\n");
    }
    if !section.drift.is_empty() {
        out.push_str("    <drift>\n");
        for entry in &section.drift {
            out.push_str(&format!(
                "      <entry symbol=\"{}\" type=\"{}\" detected_at=\"{}\"{}><![CDATA[{}]]></entry>\n",
                escape_attr(entry.symbol_name.as_str()),
                escape_attr(entry.drift_type.as_str()),
                entry.detected_at,
                entry
                    .drift_magnitude
                    .map(|value| format!(" magnitude=\"{value:.2}\""))
                    .unwrap_or_default(),
                escape_cdata(entry.summary.as_str())
            ));
        }
        out.push_str("    </drift>\n");
    }
    for notice in &section.notices {
        out.push_str(&format!(
            "    <notice>{}</notice>\n",
            escape_text(notice.as_str())
        ));
    }
    out.push_str("  </target>\n");
}

fn render_compact_target(out: &mut String, section: &TargetSection) {
    out.push_str(&format!(
        "\n[Target] {} ({})\n",
        section.target_label, section.target_kind
    ));
    if let Some(file_sir) = &section.file_sir {
        out.push_str(&format!(
            "FileIntent: {}\n",
            first_sentence(file_sir.intent.as_str())
        ));
    }
    for symbol in &section.symbols {
        out.push_str(&format!("[{}] {}\n", symbol.qualified_name, symbol.kind));
        out.push_str(&format!(
            "Intent: {}\n",
            first_sentence(symbol.intent.as_str())
        ));
        if let Some(staleness) = symbol.staleness_score {
            out.push_str(&format!("Staleness: {staleness:.2}\n"));
        }
        if let Some(status) = &symbol.sir_status {
            out.push_str(&format!("SIR: {status}\n"));
        }
    }
    if !section.immediate_graph.is_empty() {
        let deps = section
            .immediate_graph
            .iter()
            .filter(|entry| entry.relationship.contains("dependency"))
            .map(|entry| entry.qualified_name.as_str())
            .collect::<Vec<_>>();
        let callers = section
            .immediate_graph
            .iter()
            .filter(|entry| entry.relationship.contains("caller"))
            .map(|entry| format!("{} ({})", entry.qualified_name, entry.file_path))
            .collect::<Vec<_>>();
        if !deps.is_empty() {
            out.push_str(&format!("Deps: {}\n", deps.join(", ")));
        }
        if !callers.is_empty() {
            out.push_str(&format!("Callers: {}\n", callers.join(", ")));
        }
    }
    if !section.tests.is_empty() {
        out.push_str(&format!(
            "Tests: {}\n",
            section
                .tests
                .iter()
                .map(|guard| format!(
                    "{}: {}",
                    guard.test_name,
                    first_sentence(guard.description.as_str())
                ))
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    if !section.coupling.is_empty() {
        out.push_str(&format!(
            "Coupling: {}\n",
            section
                .coupling
                .iter()
                .map(|entry| format!("{} {:.2}", entry.file_path, entry.fused_score))
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    if !section.memory.is_empty() {
        out.push_str(&format!(
            "Memory: {}\n",
            section
                .memory
                .iter()
                .map(|entry| format!("{} ({})", entry.first_line, entry.source_type))
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    if let Some(health) = &section.health {
        out.push_str(&format!("Health: {}\n", health.summary));
        if !health.warnings.is_empty() {
            out.push_str(&format!("Warnings: {}\n", health.warnings.join(" | ")));
        }
    }
    if !section.drift.is_empty() {
        out.push_str(&format!(
            "Drift: {}\n",
            section
                .drift
                .iter()
                .map(|entry| format!(
                    "{} {} {}",
                    entry.symbol_name,
                    entry.drift_type,
                    first_sentence(entry.summary.as_str())
                ))
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    if let Some(source) = &section.source {
        out.push_str(&format!("[Source {}]\n", source.language));
        out.push_str(source.content.as_str());
        if !source.content.ends_with('\n') {
            out.push('\n');
        }
    }
    if !section.notices.is_empty() {
        out.push_str(&format!("Notices: {}\n", section.notices.join(" | ")));
    }
}

fn render_xml_list<'a>(
    out: &mut String,
    container: &str,
    item_name: &str,
    items: impl IntoIterator<Item = &'a str>,
) {
    let values = items
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>();
    if values.is_empty() {
        return;
    }
    out.push_str(&format!("      <{container}>\n"));
    for value in values {
        out.push_str(&format!(
            "        <{item_name}><![CDATA[{}]]></{item_name}>\n",
            escape_cdata(value)
        ));
    }
    out.push_str(&format!("      </{container}>\n"));
}

fn first_sentence(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Some((sentence, _)) = trimmed.split_once(". ") {
        return sentence.to_owned();
    }
    trimmed.lines().next().unwrap_or(trimmed).trim().to_owned()
}

fn budget_status(status: BudgetStatus) -> &'static str {
    match status {
        BudgetStatus::Included => "included",
        BudgetStatus::Truncated => "truncated",
        BudgetStatus::Omitted => "omitted",
    }
}

fn escape_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_cdata(value: &str) -> String {
    value.replace("]]>", "]]]]><![CDATA[>")
}

#[cfg(test)]
mod tests {
    use crate::sir_context::{
        BudgetUsage, ContextFormat, DriftContext, ExportDocument, ExportHealthContext,
        ExportSymbolContext, FileSirContext, LayerBudgetLine, MemoryContext, NeighborSummary,
        ProjectOverview, SourceBlock, TargetSection, TestGuard, WorkspaceHealthSummary,
        render_export_document,
    };

    use super::{render_compact, render_xml};

    fn sample_document() -> ExportDocument {
        ExportDocument {
            generated_at: 1_700_000_000,
            project_overview: ProjectOverview {
                workspace: "/tmp/aether".to_owned(),
                total_symbols: 3,
                symbols_with_sir: 3,
                sir_coverage_percent: 100.0,
                health: Some(WorkspaceHealthSummary {
                    workspace_score: 42,
                    severity: "watch".to_owned(),
                    worst_crate: Some("aetherd".to_owned()),
                }),
                drift: None,
                notices: vec!["overview notice".to_owned()],
            },
            target_sections: vec![TargetSection {
                target_kind: "file".to_owned(),
                target_label: "src/lib.rs".to_owned(),
                selector: None,
                file_path: Some("src/lib.rs".to_owned()),
                language: Some("rust".to_owned()),
                file_sir: Some(FileSirContext {
                    intent: "File intent.".to_owned(),
                    exports: vec!["alpha".to_owned()],
                    side_effects: vec!["writes cache".to_owned()],
                    dependencies: vec!["std".to_owned()],
                    error_modes: vec!["io".to_owned()],
                    symbol_count: 1,
                    confidence: 0.8,
                }),
                symbols: vec![ExportSymbolContext {
                    qualified_name: "demo::alpha".to_owned(),
                    kind: "function".to_owned(),
                    file_path: "src/lib.rs".to_owned(),
                    language: "rust".to_owned(),
                    staleness_score: Some(0.1),
                    intent: "Alpha intent. Additional detail.".to_owned(),
                    behavior: vec!["writes cache".to_owned()],
                    sir_status: Some("fresh".to_owned()),
                }],
                source: Some(SourceBlock {
                    language: "rust".to_owned(),
                    content: "fn alpha() { /* ]]> */ }\n".to_owned(),
                }),
                immediate_graph: vec![NeighborSummary {
                    relationship: "dependency".to_owned(),
                    qualified_name: "demo::beta".to_owned(),
                    file_path: "src/dep.rs".to_owned(),
                    intent_summary: "Beta intent".to_owned(),
                    depth: 1,
                }],
                broader_graph: Vec::new(),
                tests: vec![TestGuard {
                    test_name: "test_alpha".to_owned(),
                    description: "guards alpha".to_owned(),
                }],
                coupling: vec![crate::sir_context::CouplingContext {
                    file_path: "src/dep.rs".to_owned(),
                    fused_score: 0.9,
                }],
                memory: vec![MemoryContext {
                    first_line: "Decision note".to_owned(),
                    source_type: "manual".to_owned(),
                    created_at: 1_700_000_001,
                }],
                health: Some(ExportHealthContext {
                    summary: "health summary".to_owned(),
                    warnings: vec!["warning".to_owned()],
                }),
                drift: vec![DriftContext {
                    symbol_name: "demo::alpha".to_owned(),
                    drift_type: "semantic".to_owned(),
                    drift_magnitude: Some(0.7),
                    summary: "changed purpose".to_owned(),
                    detected_at: 1_700_000_002,
                }],
                notices: vec!["section notice".to_owned()],
            }],
            budget_usage: BudgetUsage {
                max_tokens: 1000,
                used_tokens: 400,
                layers: vec![LayerBudgetLine {
                    layer: "source".to_owned(),
                    suggested_tokens: 300,
                    used_tokens: 120,
                    status: crate::sir_context::BudgetStatus::Included,
                    included_items: 1,
                    omitted_items: 0,
                }],
            },
            notices: vec!["global notice".to_owned()],
        }
    }

    #[test]
    fn xml_output_has_expected_root_and_budget_metadata() {
        let rendered = render_xml(&sample_document());
        assert!(rendered.contains("<aether_context "));
        assert!(rendered.contains("<budget used=\"400\" max=\"1000\">"));
        assert!(rendered.contains("<![CDATA[fn alpha() { /* ]]]]><![CDATA[> */ }"));
    }

    #[test]
    fn compact_output_is_dense_and_contains_budget_metadata() {
        let document = sample_document();
        let compact = render_compact(&document);
        let markdown = render_export_document(&document, ContextFormat::Markdown);

        assert!(compact.contains("Budget: 400/1000"));
        assert!(compact.len() < markdown.len());
    }
}
