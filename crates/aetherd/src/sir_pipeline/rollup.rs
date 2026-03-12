use std::sync::Arc;

use aether_core::Language;
use aether_infer::{InferenceProvider, SirContext};
use aether_sir::{FileSir, SirAnnotation};
use anyhow::{Context, Result};
use tokio::runtime::Runtime;

use super::infer::generate_sir_with_retries;

#[derive(Debug, Clone)]
pub(super) struct FileLeafSir {
    pub(super) qualified_name: String,
    pub(super) sir: SirAnnotation,
}

pub(super) fn aggregate_file_sir(
    file_path: &str,
    language: Language,
    leaf_sirs: &[FileLeafSir],
    provider: Arc<dyn InferenceProvider>,
    runtime: &Runtime,
    inference_timeout_secs: u64,
) -> Result<FileSir> {
    let mut sorted = leaf_sirs.to_vec();
    sorted.sort_by(|left, right| left.qualified_name.cmp(&right.qualified_name));

    let intent = if sorted.len() <= 5 {
        concatenate_leaf_intents(&sorted)
    } else {
        match summarize_file_intent(
            file_path,
            language,
            &sorted,
            provider,
            runtime,
            inference_timeout_secs,
        ) {
            Ok(summary) if !summary.trim().is_empty() => summary,
            Ok(_) => concatenate_leaf_intents(&sorted),
            Err(err) => {
                tracing::debug!(
                    file_path = %file_path,
                    error = %err,
                    "file rollup summarization failed, using deterministic concatenation"
                );
                concatenate_leaf_intents(&sorted)
            }
        }
    };

    let mut exports = Vec::new();
    let mut side_effects = Vec::new();
    let mut dependencies = Vec::new();
    let mut error_modes = Vec::new();
    let mut confidence = 1.0f32;

    for entry in &sorted {
        exports.push(entry.qualified_name.clone());
        side_effects.extend(entry.sir.side_effects.clone());
        dependencies.extend(entry.sir.dependencies.clone());
        error_modes.extend(entry.sir.error_modes.clone());
        confidence = confidence.min(entry.sir.confidence);
    }

    sort_and_dedup(&mut exports);
    sort_and_dedup(&mut side_effects);
    sort_and_dedup(&mut dependencies);
    sort_and_dedup(&mut error_modes);

    Ok(FileSir {
        intent,
        exports,
        side_effects,
        dependencies,
        error_modes,
        symbol_count: sorted.len(),
        confidence,
    })
}

fn summarize_file_intent(
    file_path: &str,
    language: Language,
    leaf_sirs: &[FileLeafSir],
    provider: Arc<dyn InferenceProvider>,
    runtime: &Runtime,
    inference_timeout_secs: u64,
) -> Result<String> {
    let mut prompt_sections = Vec::with_capacity(leaf_sirs.len());
    for entry in leaf_sirs {
        prompt_sections.push(format!(
            "symbol: {}\nintent: {}\nside_effects: {}\ndependencies: {}\nerror_modes: {}",
            entry.qualified_name,
            entry.sir.intent,
            join_field(&entry.sir.side_effects),
            join_field(&entry.sir.dependencies),
            join_field(&entry.sir.error_modes),
        ));
    }

    let summary_input = format!(
        "Generate a concise file-level intent summary from the following leaf SIR entries.\n\n{}",
        prompt_sections.join("\n\n")
    );
    let context = SirContext {
        language: language.as_str().to_owned(),
        file_path: file_path.to_owned(),
        qualified_name: format!("file::{file_path}"),
        priority_score: None,
        kind: "file".to_owned(),
        is_public: true,
        line_count: summary_input.lines().count(),
    };

    let summarized = runtime
        .block_on(generate_sir_with_retries(
            provider,
            summary_input,
            context,
            inference_timeout_secs,
        ))
        .with_context(|| format!("failed to summarize file intent for {file_path}"))?;

    Ok(summarized.sir.intent.trim().to_owned())
}

fn concatenate_leaf_intents(leaf_sirs: &[FileLeafSir]) -> String {
    let intents = leaf_sirs
        .iter()
        .map(|entry| entry.sir.intent.trim())
        .filter(|intent| !intent.is_empty())
        .collect::<Vec<_>>();
    if intents.is_empty() {
        "No summarized intent available".to_owned()
    } else {
        intents.join("; ")
    }
}

fn join_field(values: &[String]) -> String {
    if values.is_empty() {
        "(none)".to_owned()
    } else {
        values.join(", ")
    }
}

fn sort_and_dedup(values: &mut Vec<String>) {
    values.sort();
    values.dedup();
}
