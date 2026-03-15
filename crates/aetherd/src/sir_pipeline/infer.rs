use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use aether_core::{Position, SourceRange, Symbol};
use aether_infer::{InferError, InferSirResult, InferenceProvider, SirContext};
use aether_sir::SirAnnotation;
use anyhow::{Context, Result, anyhow};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout};

use super::{
    INFERENCE_BACKOFF_BASE_MS, INFERENCE_BACKOFF_MAX_MS, INFERENCE_MAX_RETRIES,
    MAX_SYMBOL_TEXT_CHARS,
};

#[derive(Debug)]
pub(crate) struct SirJob {
    pub(crate) symbol: Symbol,
    pub(crate) symbol_text: String,
    pub(crate) context: SirContext,
    pub(crate) custom_prompt: Option<String>,
    pub(crate) deep_mode: bool,
}

#[derive(Debug)]
pub(super) struct GeneratedSir {
    pub(super) symbol: Symbol,
    pub(super) sir: SirAnnotation,
    pub(super) provider_name: String,
    pub(super) model_name: String,
}

#[derive(Debug)]
pub(super) struct FailedSirGeneration {
    pub(super) symbol: Symbol,
    pub(super) error_message: String,
}

#[derive(Debug)]
pub(super) enum SirGenerationOutcome {
    Success(Box<GeneratedSir>),
    Failure(Box<FailedSirGeneration>),
}

pub(crate) fn build_job(
    workspace_root: &Path,
    symbol: Symbol,
    priority_score: Option<f64>,
    max_chars: Option<usize>,
) -> Result<SirJob> {
    let full_path = workspace_root.join(&symbol.file_path);
    let source = fs::read_to_string(&full_path)
        .with_context(|| format!("failed to read symbol source file {}", full_path.display()))?;

    let mut symbol_text = extract_symbol_source_text(&source, symbol.range)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| {
            anyhow!(
                "tree-sitter extraction failed for {} in {} — skipping to avoid whole-file hallucination",
                symbol.qualified_name,
                symbol.file_path,
            )
        })?;
    let effective_limit = match max_chars {
        Some(0) | None => MAX_SYMBOL_TEXT_CHARS,
        Some(value) => value,
    };
    if effective_limit > 0 && symbol_text.len() > effective_limit {
        let truncated = symbol_text
            .char_indices()
            .take_while(|(index, _)| *index < effective_limit)
            .last()
            .map(|(index, ch)| index + ch.len_utf8())
            .unwrap_or(0);
        tracing::warn!(
            symbol = %symbol.name,
            original_len = symbol_text.len(),
            truncated_len = truncated,
            "symbol text truncated for inference"
        );
        symbol_text = symbol_text[..truncated].to_owned();
    }

    let context = SirContext {
        language: symbol.language.as_str().to_owned(),
        file_path: symbol.file_path.clone(),
        qualified_name: symbol.qualified_name.clone(),
        priority_score,
        kind: symbol.kind.as_str().to_owned(),
        is_public: infer_symbol_text_is_public(&symbol_text),
        line_count: symbol_text.lines().count(),
    };

    Ok(SirJob {
        symbol,
        symbol_text,
        context,
        custom_prompt: None,
        deep_mode: false,
    })
}

fn infer_symbol_text_is_public(symbol_text: &str) -> bool {
    let trimmed = symbol_text.trim_start();
    trimmed.starts_with("pub ")
        || trimmed.starts_with("pub(")
        || trimmed.starts_with("export ")
        || trimmed.starts_with("export default ")
}

fn extract_symbol_source_text(source: &str, range: SourceRange) -> Option<String> {
    let start = range
        .start_byte
        .or_else(|| byte_offset_for_position(source, range.start))?;
    let end = range
        .end_byte
        .or_else(|| byte_offset_for_position(source, range.end))?;

    if start > end || end > source.len() {
        return None;
    }

    source.get(start..end).map(|slice| slice.to_owned())
}

fn byte_offset_for_position(source: &str, position: Position) -> Option<usize> {
    let mut line = 1usize;
    let mut column = 1usize;

    if position.line == 1 && position.column == 1 {
        return Some(0);
    }

    for (index, ch) in source.char_indices() {
        if line == position.line && column == position.column {
            return Some(index);
        }

        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += ch.len_utf8();
        }
    }

    if line == position.line && column == position.column {
        Some(source.len())
    } else {
        None
    }
}

pub(super) async fn generate_sir_jobs(
    provider: Arc<dyn InferenceProvider>,
    tiered_parse_fallback_provider: Option<Arc<dyn InferenceProvider>>,
    tiered_parse_fallback_model: Option<String>,
    jobs: Vec<SirJob>,
    concurrency: usize,
    timeout_secs: u64,
) -> Result<Vec<SirGenerationOutcome>> {
    let total_jobs = jobs.len();
    let semaphore = Arc::new(Semaphore::new(concurrency.max(1)));
    let mut join_set = JoinSet::new();

    for job in jobs {
        let provider = provider.clone();
        let tiered_parse_fallback_provider = tiered_parse_fallback_provider.clone();
        let tiered_parse_fallback_model = tiered_parse_fallback_model.clone();
        let semaphore = semaphore.clone();

        join_set.spawn(async move {
            let SirJob {
                symbol,
                symbol_text,
                context,
                custom_prompt,
                deep_mode,
            } = job;
            let qualified_name = symbol.qualified_name.clone();

            let permit = semaphore.acquire_owned().await;
            let _permit = match permit {
                Ok(permit) => permit,
                Err(_) => {
                    return SirGenerationOutcome::Failure(Box::new(FailedSirGeneration {
                        symbol,
                        error_message: "inference semaphore closed".to_owned(),
                    }));
                }
            };

            let generated = match custom_prompt.as_ref() {
                Some(prompt) => generate_sir_from_prompt_with_retries(
                    provider.clone(),
                    prompt.clone(),
                    context.clone(),
                    deep_mode,
                    timeout_secs,
                )
                .await
                .with_context(|| {
                    format!("failed to generate deep/custom SIR for symbol {qualified_name}")
                }),
                None => generate_sir_with_retries(
                    provider.clone(),
                    symbol_text.clone(),
                    context.clone(),
                    timeout_secs,
                )
                .await
                .with_context(|| format!("failed to generate SIR for symbol {qualified_name}")),
            };

            match generated {
                Ok(result) => SirGenerationOutcome::Success(Box::new(GeneratedSir {
                    symbol,
                    sir: result.sir,
                    provider_name: result.provider,
                    model_name: result.model,
                })),
                Err(err)
                    if is_parse_validation_exhausted_error(&err)
                        && tiered_parse_fallback_provider.is_some() =>
                {
                    let fallback_model =
                        tiered_parse_fallback_model.as_deref().unwrap_or("fallback");
                    tracing::warn!(
                        "WARN: Primary model parse failure for {}. Falling back to {}.",
                        qualified_name,
                        fallback_model
                    );

                    let fallback_provider = tiered_parse_fallback_provider
                        .as_ref()
                        .expect("checked is_some above")
                        .clone();
                    let fallback_generated = generate_sir_with_retries(
                        fallback_provider.clone(),
                        symbol_text.clone(),
                        context.clone(),
                        timeout_secs,
                    );
                    let fallback_generated = match custom_prompt {
                        Some(prompt) => generate_sir_from_prompt_with_retries(
                            fallback_provider,
                            prompt,
                            context,
                            deep_mode,
                            timeout_secs,
                        )
                        .await
                        .with_context(|| {
                            format!(
                                "failed fallback deep/custom SIR generation after primary parse failure for {qualified_name}"
                            )
                        }),
                        None => fallback_generated.await.with_context(|| {
                            format!(
                                "failed fallback SIR generation after primary parse failure for {qualified_name}"
                            )
                        }),
                    };

                    match fallback_generated {
                        Ok(result) => SirGenerationOutcome::Success(Box::new(GeneratedSir {
                            symbol,
                            sir: result.sir,
                            provider_name: result.provider,
                            model_name: result.model,
                        })),
                        Err(fallback_err) => {
                            SirGenerationOutcome::Failure(Box::new(FailedSirGeneration {
                            symbol,
                            error_message: format!("{fallback_err:#}"),
                            }))
                        }
                    }
                }
                Err(err) => SirGenerationOutcome::Failure(Box::new(FailedSirGeneration {
                    symbol,
                    error_message: format!("{err:#}"),
                })),
            }
        });
    }

    let log_thresholds: Vec<usize> = if total_jobs < 20 {
        (1..=total_jobs).collect()
    } else {
        let pcts = [
            1usize, 2, 3, 4, 5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55, 60, 65, 70, 75, 80, 85, 90,
            95, 100,
        ];
        let mut thresholds = pcts
            .iter()
            .map(|pct| (total_jobs * pct / 100).max(1))
            .collect::<Vec<_>>();
        thresholds.dedup();
        if thresholds.last().copied() != Some(total_jobs) {
            thresholds.push(total_jobs);
        }
        thresholds
    };

    let mut results = Vec::with_capacity(total_jobs);
    let mut completed = 0usize;
    let mut successes = 0usize;
    let mut failures = 0usize;
    let mut next_threshold_index = 0usize;

    while let Some(joined) = join_set.join_next().await {
        match joined {
            Ok(result) => {
                completed += 1;
                match &result {
                    SirGenerationOutcome::Success(_) => successes += 1,
                    SirGenerationOutcome::Failure(_) => failures += 1,
                }

                if log_thresholds.get(next_threshold_index).copied() == Some(completed) {
                    let pct = (completed * 100) / total_jobs.max(1);
                    tracing::info!(
                        completed,
                        total_jobs,
                        successes,
                        failures,
                        pct,
                        "SIR batch progress: {completed}/{total_jobs} ({pct}%) - {successes} ok, {failures} failed"
                    );
                    next_threshold_index += 1;
                }

                results.push(result);
            }
            Err(err) => return Err(anyhow!("inference task join error: {err}")),
        }
    }

    Ok(results)
}

fn is_parse_validation_exhausted_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<InferError>()
            .is_some_and(|inner| matches!(inner, InferError::ParseValidationExhausted(_)))
    })
}

pub(super) async fn generate_sir_with_retries(
    provider: Arc<dyn InferenceProvider>,
    symbol_text: String,
    context: SirContext,
    timeout_secs: u64,
) -> Result<InferSirResult> {
    let total_attempts = INFERENCE_MAX_RETRIES + 1;
    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 0..total_attempts {
        let timeout_result = timeout(
            Duration::from_secs(timeout_secs.max(1)),
            provider.generate_sir_with_meta(&symbol_text, &context),
        )
        .await;

        match timeout_result {
            Ok(Ok(sir)) => return Ok(sir),
            Ok(Err(err)) => {
                last_error = Some(anyhow::Error::new(err).context(format!(
                    "attempt {}/{} failed",
                    attempt + 1,
                    total_attempts
                )));
            }
            Err(_) => {
                last_error = Some(anyhow!(
                    "attempt {}/{} timed out after {}s",
                    attempt + 1,
                    total_attempts,
                    timeout_secs.max(1)
                ));
            }
        }

        if attempt + 1 < total_attempts {
            let backoff_ms = (INFERENCE_BACKOFF_BASE_MS << attempt).min(INFERENCE_BACKOFF_MAX_MS);
            sleep(Duration::from_millis(backoff_ms)).await;
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow!("inference failed without an error message")))
}

async fn generate_sir_from_prompt_with_retries(
    provider: Arc<dyn InferenceProvider>,
    prompt: String,
    context: SirContext,
    deep_mode: bool,
    timeout_secs: u64,
) -> Result<InferSirResult> {
    let total_attempts = INFERENCE_MAX_RETRIES + 1;
    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 0..total_attempts {
        let timeout_result = timeout(
            Duration::from_secs(timeout_secs.max(1)),
            provider.generate_sir_from_prompt_with_meta(prompt.as_str(), &context, deep_mode),
        )
        .await;

        match timeout_result {
            Ok(Ok(sir)) => return Ok(sir),
            Ok(Err(err)) => {
                last_error = Some(anyhow::Error::new(err).context(format!(
                    "attempt {}/{} failed",
                    attempt + 1,
                    total_attempts
                )));
            }
            Err(_) => {
                last_error = Some(anyhow!(
                    "attempt {}/{} timed out after {}s",
                    attempt + 1,
                    total_attempts,
                    timeout_secs.max(1)
                ));
            }
        }

        if attempt + 1 < total_attempts {
            let backoff_ms = (INFERENCE_BACKOFF_BASE_MS << attempt).min(INFERENCE_BACKOFF_MAX_MS);
            sleep(Duration::from_millis(backoff_ms)).await;
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow!("inference failed without an error message")))
}
