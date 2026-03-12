use aether_sir::{SirAnnotation, validate_sir};
use serde_json::Value;

use crate::types::InferError;

pub(crate) fn build_retry_prompt(
    original_prompt: &str,
    error: &str,
    previous_output: &str,
) -> String {
    format!(
        "{original_prompt}\n\nYour previous response was invalid. Error: {error}. Previous output: {previous_output}. Please respond again with STRICT JSON only, fixing the error above."
    )
}

pub(crate) async fn run_sir_parse_validation_retries<F, Fut>(
    retries: usize,
    mut candidate_json_loader: F,
) -> Result<SirAnnotation, InferError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<String, InferError>>,
{
    let mut last_error = String::from("unknown parse/validation failure");

    for attempt in 0..=retries {
        let candidate_json = candidate_json_loader().await?;

        match parse_and_validate_sir(&candidate_json) {
            Ok(sir) => return Ok(sir),
            Err(message) => {
                last_error = message;
                if attempt == retries {
                    break;
                }
            }
        }
    }

    Err(InferError::ParseValidationExhausted(last_error))
}

pub(crate) async fn run_sir_parse_validation_retries_with_feedback<F, Fut, G, Gfut>(
    retries: usize,
    mut candidate_json_loader: F,
    mut feedback_json_loader: G,
) -> Result<SirAnnotation, InferError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<String, InferError>>,
    G: FnMut(String, String) -> Gfut,
    Gfut: std::future::Future<Output = Result<String, InferError>>,
{
    let mut last_error = String::from("unknown parse/validation failure");
    let mut last_output = String::new();
    let mut retry_with_feedback = false;

    for attempt in 0..=retries {
        let mut attempted_feedback = false;
        let candidate_json = if retry_with_feedback && !last_output.is_empty() {
            attempted_feedback = true;
            match feedback_json_loader(last_output.clone(), last_error.clone()).await {
                Ok(candidate) => candidate,
                Err(_) => candidate_json_loader().await?,
            }
        } else {
            candidate_json_loader().await?
        };

        match parse_and_validate_sir(&candidate_json) {
            Ok(sir) => return Ok(sir),
            Err(message) => {
                last_error = message;
                last_output = candidate_json;
                if attempt == retries {
                    break;
                }
                retry_with_feedback = !attempted_feedback;
            }
        }
    }

    Err(InferError::ParseValidationExhausted(last_error))
}

pub(crate) fn extract_local_text_part(response: &Value) -> Result<String, InferError> {
    if let Some(text) = value_to_candidate_json(response) {
        return Ok(text);
    }

    let candidate_paths = [
        "/response",
        "/text",
        "/output",
        "/message/content",
        "/choices/0/text",
        "/choices/0/message/content",
        "/data/output",
    ];

    for path in candidate_paths {
        if let Some(value) = response.pointer(path)
            && let Some(text) = value_to_candidate_json(value)
        {
            return Ok(text);
        }
    }

    Err(InferError::InvalidResponse(
        "missing local model text/JSON response body".to_owned(),
    ))
}

fn value_to_candidate_json(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_owned());
    }

    if looks_like_sir_shape(value) {
        return Some(value.to_string());
    }

    None
}

fn looks_like_sir_shape(value: &Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };

    [
        "intent",
        "inputs",
        "outputs",
        "side_effects",
        "dependencies",
        "error_modes",
        "confidence",
    ]
    .iter()
    .all(|key| obj.contains_key(*key))
}

fn parse_and_validate_sir(candidate_json: &str) -> Result<SirAnnotation, String> {
    let normalized = normalize_candidate_json(candidate_json);

    let sir: SirAnnotation =
        serde_json::from_str(&normalized).map_err(|err| format!("json parse error: {err}"))?;

    validate_sir(&sir).map_err(|err| format!("sir validation error: {err}"))?;
    Ok(sir)
}

pub(crate) fn normalize_candidate_json(candidate_json: &str) -> String {
    let trimmed = candidate_json.trim();
    let lower = trimmed.to_ascii_lowercase();

    let cleanup_trailing_commas = |input: &str| -> String {
        let chars: Vec<char> = input.chars().collect();
        let mut out = String::with_capacity(input.len());
        let mut index = 0usize;

        while index < chars.len() {
            let current = chars[index];
            if current == ',' {
                let mut lookahead = index + 1;
                while lookahead < chars.len() && chars[lookahead].is_whitespace() {
                    lookahead += 1;
                }

                if lookahead < chars.len() && matches!(chars[lookahead], ']' | '}') {
                    index += 1;
                    continue;
                }
            }

            out.push(current);
            index += 1;
        }

        out
    };

    let extract_fenced_body = |input: &str, opening_idx: usize| -> Option<String> {
        let fence_payload = input.get((opening_idx + 3)..)?;
        let newline_idx = fence_payload.find('\n')?;
        let body_start = opening_idx + 3 + newline_idx + 1;
        let after_newline = input.get(body_start..)?;
        let closing_idx = after_newline.find("```")?;
        Some(after_newline[..closing_idx].trim().to_owned())
    };

    if let Some(idx) = lower.find("```json")
        && let Some(extracted) = extract_fenced_body(trimmed, idx)
    {
        return cleanup_trailing_commas(extracted.as_str());
    }

    if let Some(idx) = trimmed.find("```")
        && let Some(extracted) = extract_fenced_body(trimmed, idx)
    {
        return cleanup_trailing_commas(extracted.as_str());
    }

    if trimmed.starts_with('{') {
        return cleanup_trailing_commas(trimmed);
    }

    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}'))
        && start < end
    {
        return cleanup_trailing_commas(&trimmed[start..=end]);
    }

    trimmed.to_owned()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn normalize_candidate_json_extracts_json_with_language_tag() {
        let input = "```json\n{\"purpose\":\"test\"}\n```";
        assert_eq!(normalize_candidate_json(input), "{\"purpose\":\"test\"}");
    }

    #[test]
    fn normalize_candidate_json_extracts_json_from_thinking_preamble() {
        let input = "<thinking>analysis here</thinking>\n{\"intent\":\"test\"}";
        assert_eq!(normalize_candidate_json(input), "{\"intent\":\"test\"}");
    }

    #[test]
    fn normalize_candidate_json_extracts_json_from_plain_preamble() {
        let input = "Here is the SIR:\n{\"intent\":\"test\",\"inputs\":[]}";
        assert_eq!(
            normalize_candidate_json(input),
            "{\"intent\":\"test\",\"inputs\":[]}"
        );
    }

    #[test]
    fn normalize_candidate_json_strips_trailing_commas() {
        let input = "{\"inputs\":[\"a\",\"b\",],\"intent\":\"test\"}";
        assert_eq!(
            normalize_candidate_json(input),
            "{\"inputs\":[\"a\",\"b\"],\"intent\":\"test\"}"
        );
    }

    #[test]
    fn normalize_candidate_json_returns_unmodified_when_already_clean() {
        let input = "{\"intent\":\"test\"}";
        assert_eq!(normalize_candidate_json(input), input);
    }

    #[test]
    fn normalize_candidate_json_returns_unmodified_when_no_json_is_present() {
        let input = "no json here at all";
        assert_eq!(normalize_candidate_json(input), input);
    }

    #[test]
    fn normalize_candidate_json_falls_back_to_bracket_extraction_when_fence_is_unclosed() {
        let input = "```json\n{\"purpose\":\"test\"}";
        assert_eq!(normalize_candidate_json(input), "{\"purpose\":\"test\"}");
    }

    #[test]
    fn normalize_candidate_json_uses_first_fenced_block_when_multiple_present() {
        let input =
            "```json\n{\"purpose\":\"first\"}\n```\n\n```json\n{\"purpose\":\"second\"}\n```";
        assert_eq!(normalize_candidate_json(input), "{\"purpose\":\"first\"}");
    }

    #[test]
    fn normalize_candidate_json_prefers_fenced_block_over_preamble_bracket_fallback() {
        let input = "<thinking>stuff</thinking>\n```json\n{\"intent\":\"test\"}\n```";
        assert_eq!(normalize_candidate_json(input), "{\"intent\":\"test\"}");
    }

    #[tokio::test]
    async fn retry_with_feedback_uses_error_context_on_second_attempt() {
        let valid_json = Arc::new(
            r#"{"intent":"valid","inputs":[],"outputs":[],"side_effects":[],"dependencies":[],"error_modes":[],"confidence":0.9}"#
                .to_owned(),
        );
        let scratch_calls = Arc::new(AtomicUsize::new(0));
        let feedback_inputs = Arc::new(Mutex::new(Vec::<(String, String)>::new()));

        let result = run_sir_parse_validation_retries_with_feedback(
            2,
            {
                let valid_json = valid_json.clone();
                let scratch_calls = scratch_calls.clone();
                move || {
                    let valid_json = valid_json.clone();
                    let scratch_calls = scratch_calls.clone();
                    async move {
                        if scratch_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                            Ok("not json".to_owned())
                        } else {
                            Ok(valid_json.as_ref().clone())
                        }
                    }
                }
            },
            {
                let feedback_inputs = feedback_inputs.clone();
                let valid_json = valid_json.clone();
                move |previous_output: String, error: String| {
                    let feedback_inputs = feedback_inputs.clone();
                    let valid_json = valid_json.clone();
                    async move {
                        let mut guard = feedback_inputs.lock().expect("feedback lock");
                        guard.push((previous_output, error));
                        Ok(valid_json.as_ref().clone())
                    }
                }
            },
        )
        .await
        .expect("retry should succeed");

        assert_eq!(result.intent, "valid");
        assert_eq!(scratch_calls.load(Ordering::SeqCst), 1);

        let captured = feedback_inputs.lock().expect("feedback lock");
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].0, "not json");
        assert!(captured[0].1.contains("json parse error"));
    }
}
