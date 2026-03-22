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
    let sorted = sorted_leaf_sirs(leaf_sirs);

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

    Ok(assemble_file_sir(&sorted, intent))
}

pub(super) fn concatenate_file_sir(leaf_sirs: &[FileLeafSir]) -> FileSir {
    let sorted = sorted_leaf_sirs(leaf_sirs);
    let intent = concatenate_leaf_intents(&sorted);
    assemble_file_sir(&sorted, intent)
}

pub(super) fn file_sir_from_summary(
    leaf_sirs: &[FileLeafSir],
    summary: impl Into<String>,
) -> FileSir {
    let sorted = sorted_leaf_sirs(leaf_sirs);
    let summary = summary.into();
    let intent = if summary.trim().is_empty() {
        concatenate_leaf_intents(&sorted)
    } else {
        summary.trim().to_owned()
    };

    assemble_file_sir(&sorted, intent)
}

pub(super) async fn summarize_file_intent_async(
    file_path: &str,
    language: Language,
    leaf_sirs: &[FileLeafSir],
    provider: Arc<dyn InferenceProvider>,
    inference_timeout_secs: u64,
) -> Result<String> {
    let sorted = sorted_leaf_sirs(leaf_sirs);
    summarize_file_intent_sorted_async(
        file_path,
        language,
        &sorted,
        provider,
        inference_timeout_secs,
    )
    .await
}

fn assemble_file_sir(leaf_sirs: &[FileLeafSir], intent: String) -> FileSir {
    let mut exports = Vec::new();
    let mut side_effects = Vec::new();
    let mut dependencies = Vec::new();
    let mut error_modes = Vec::new();
    let mut confidence = 1.0f32;

    for entry in leaf_sirs {
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

    FileSir {
        intent,
        exports,
        side_effects,
        dependencies,
        error_modes,
        symbol_count: leaf_sirs.len(),
        confidence,
    }
}

fn summarize_file_intent(
    file_path: &str,
    language: Language,
    leaf_sirs: &[FileLeafSir],
    provider: Arc<dyn InferenceProvider>,
    runtime: &Runtime,
    inference_timeout_secs: u64,
) -> Result<String> {
    let sorted = sorted_leaf_sirs(leaf_sirs);
    runtime.block_on(summarize_file_intent_sorted_async(
        file_path,
        language,
        &sorted,
        provider,
        inference_timeout_secs,
    ))
}

async fn summarize_file_intent_sorted_async(
    file_path: &str,
    language: Language,
    leaf_sirs: &[FileLeafSir],
    provider: Arc<dyn InferenceProvider>,
    inference_timeout_secs: u64,
) -> Result<String> {
    let summary_input = build_file_rollup_prompt(leaf_sirs);
    let context = build_file_rollup_context(file_path, language, &summary_input);

    let summarized =
        generate_sir_with_retries(provider, summary_input, context, inference_timeout_secs)
            .await
            .with_context(|| format!("failed to summarize file intent for {file_path}"))?;

    Ok(summarized.sir.intent.trim().to_owned())
}

fn build_file_rollup_prompt(leaf_sirs: &[FileLeafSir]) -> String {
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

    format!(
        "Generate a concise file-level intent summary from the following leaf SIR entries.\n\n{}",
        prompt_sections.join("\n\n")
    )
}

fn build_file_rollup_context(
    file_path: &str,
    language: Language,
    summary_input: &str,
) -> SirContext {
    SirContext {
        language: language.as_str().to_owned(),
        file_path: file_path.to_owned(),
        qualified_name: format!("file::{file_path}"),
        priority_score: None,
        kind: "file".to_owned(),
        is_public: true,
        line_count: summary_input.lines().count(),
    }
}

fn sorted_leaf_sirs(leaf_sirs: &[FileLeafSir]) -> Vec<FileLeafSir> {
    let mut sorted = leaf_sirs.to_vec();
    sorted.sort_by(|left, right| left.qualified_name.cmp(&right.qualified_name));
    sorted
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
