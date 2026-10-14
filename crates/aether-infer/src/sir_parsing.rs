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

/// Reasoning tag pairs a deep-mode reply may carry: Ollama's native `<think>` block and
/// the `<thinking>` block the repository's CoT prompt (`sir_prompt.rs`) asks for.
const THINK_TAGS: [(&str, &str); 2] = [("<thinking>", "</thinking>"), ("<think>", "</think>")];

/// Find the earliest reasoning tag in `text`, returning its byte offset and tag pair.
fn find_think_open(text: &str) -> Option<(usize, (&'static str, &'static str))> {
    THINK_TAGS
        .iter()
        .filter_map(|tags| text.find(tags.0).map(|index| (index, *tags)))
        .min_by_key(|(index, _)| *index)
}

/// Split an Ollama deep-mode response into its reasoning block(s) and the remainder that
/// carries the SIR JSON.
///
/// Both `<think>...</think>` (Ollama's native tag) and `<thinking>...</thinking>` (what the
/// deep prompt requests) are recognised. Every block is collected (joined by blank lines)
/// and removed from the remainder; an unterminated block swallows the rest of the text as
/// reasoning so a truncated trace never leaks into the JSON parser. Returns `None`
/// reasoning when the response has no block or only an empty one.
pub(crate) fn split_think_block(raw: &str) -> (Option<String>, String) {
    let mut reasoning_parts = Vec::new();
    let mut remainder = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some((start, (open, close))) = find_think_open(rest) {
        remainder.push_str(&rest[..start]);
        let after_open = &rest[start + open.len()..];
        match after_open.find(close) {
            Some(end) => {
                let thought = after_open[..end].trim();
                if !thought.is_empty() {
                    reasoning_parts.push(thought.to_owned());
                }
                rest = &after_open[end + close.len()..];
            }
            None => {
                let thought = after_open.trim();
                if !thought.is_empty() {
                    reasoning_parts.push(thought.to_owned());
                }
                rest = "";
            }
        }
    }
    remainder.push_str(rest);

    let reasoning = if reasoning_parts.is_empty() {
        None
    } else {
        Some(reasoning_parts.join("\n\n"))
    };
    (reasoning, remainder)
}

pub(crate) fn parse_and_validate_sir(candidate_json: &str) -> Result<SirAnnotation, String> {
    let normalized = normalize_candidate_json(candidate_json);

    let sir: SirAnnotation =
        serde_json::from_str(&normalized).map_err(|err| format!("json parse error: {err}"))?;

    validate_sir(&sir).map_err(|err| format!("sir validation error: {err}"))?;

    Ok(sir)
}

pub(crate) fn normalize_candidate_json(candidate_json: &str) -> String {
    let trimmed = candidate_json.trim();
    // A response that already is a JSON object is returned as is: fence markers and
    // `,}` / `,]` may legitimately appear inside its strings, and the repairs below
    // must not touch them.
    if is_json_object(trimmed) {
        return trimmed.to_owned();
    }

    // Otherwise try each way of finding the object, most specific first, and keep the
    // first that yields a JSON object. Fences found by the string-aware scanner come
    // first; the plain text search for fences stays as a fallback for prose the scanner
    // misreads (an unmatched brace followed by a stray quote, for instance).
    let mut candidates = Vec::new();
    let fences = fence_offsets(trimmed);
    let tagged_fence = fences
        .iter()
        .copied()
        .find(|&opening| is_json_tag(trimmed, opening));
    for opening in [tagged_fence, fences.first().copied()]
        .into_iter()
        .flatten()
    {
        candidates.extend(extract_fenced_body(trimmed, opening, &fences));
    }
    let lower = trimmed.to_ascii_lowercase();
    for opening in [lower.find("```json"), trimmed.find("```")]
        .into_iter()
        .flatten()
    {
        candidates.extend(extract_fenced_bodies_plain(trimmed, opening));
    }
    if trimmed.starts_with('{') {
        candidates.push(trimmed.to_owned());
    }
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}'))
        && start < end
    {
        candidates.push(trimmed[start..=end].to_owned());
    }

    let candidates = candidates
        .iter()
        .map(|candidate| cleanup_trailing_commas(candidate))
        .collect::<Vec<_>>();
    candidates
        .iter()
        .find(|candidate| is_json_object(candidate))
        .or(candidates.first())
        .cloned()
        .unwrap_or_else(|| trimmed.to_owned())
}

fn is_json_object(text: &str) -> bool {
    matches!(serde_json::from_str::<Value>(text), Ok(Value::Object(_)))
}

/// Whether the fence opening at `opening` is tagged `json` (case-insensitive).
fn is_json_tag(input: &str, opening: usize) -> bool {
    input
        .get(opening + 3..opening + 7)
        .is_some_and(|tag| tag.eq_ignore_ascii_case("json"))
}

/// Removes commas that directly precede a closing `]` or `}`, leaving everything inside
/// JSON strings untouched.
fn cleanup_trailing_commas(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;

    for (index, &current) in chars.iter().enumerate() {
        if in_string {
            out.push(current);
            if escaped {
                escaped = false;
            } else if current == '\\' {
                escaped = true;
            } else if current == '"' {
                in_string = false;
            }
        } else if current == '"' {
            in_string = true;
            out.push(current);
        } else if current == ',' {
            let next = chars[index + 1..]
                .iter()
                .find(|candidate| !candidate.is_whitespace());
            if !matches!(next, Some(']' | '}')) {
                out.push(current);
            }
        } else {
            out.push(current);
        }
    }

    out
}

/// Byte offsets of the ```` ``` ```` markers outside JSON strings. Quotes only open a
/// string inside a `{...}` or `[...]` container, so balanced quotes in surrounding
/// prose don't hide a real fence.
fn fence_offsets(input: &str) -> Vec<usize> {
    let bytes = input.as_bytes();
    let mut offsets = Vec::new();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut index = 0usize;

    while index < bytes.len() {
        let current = bytes[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if current == b'\\' {
                escaped = true;
            } else if current == b'"' {
                in_string = false;
            }
        } else if current == b'"' && depth > 0 {
            in_string = true;
        } else if matches!(current, b'{' | b'[') {
            depth += 1;
        } else if matches!(current, b'}' | b']') {
            depth = depth.saturating_sub(1);
        } else if bytes.get(index..index + 3) == Some(b"```".as_slice()) {
            offsets.push(index);
            index += 3;
            continue;
        }
        index += 1;
    }

    offsets
}

/// The trimmed body between the fence that opens at `opening` (after its info line)
/// and the next fence marker the scanner found.
fn extract_fenced_body(input: &str, opening: usize, fences: &[usize]) -> Option<String> {
    let info_and_body = input.get(opening + 3..)?;
    let body_start = opening + 3 + info_and_body.find('\n')? + 1;
    let closing = fences
        .iter()
        .copied()
        .find(|offset| *offset >= body_start)?;
    Some(input.get(body_start..closing)?.trim().to_owned())
}

/// The trimmed bodies between the fence that opens at `opening` and each later
/// ```` ``` ```` in the text, nearest first, without regard to JSON strings.
fn extract_fenced_bodies_plain(input: &str, opening: usize) -> Vec<String> {
    let Some(info_and_body) = input.get(opening + 3..) else {
        return Vec::new();
    };
    let Some(newline) = info_and_body.find('\n') else {
        return Vec::new();
    };
    let body_start = opening + 3 + newline + 1;
    let after = &input[body_start..];
    after
        .match_indices("```")
        .map(|(closing, _)| after[..closing].trim().to_owned())
        .collect()
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

    #[test]
    fn normalize_candidate_json_passes_valid_json_through_unchanged() {
        for input in [
            r###"{
  "intent": "Rejects the literal ,} token"
}"###,
            r###"{
  "intent": "Returns the literal ,] token"
}"###,
            r###"{
  "intent": "Rejects the literal ,  } token"
}"###,
            r###"{
  "intent": "Rejects literal ,} and ,] tokens, escaped \"quotes\", backslashes \\, and Unicode λ."
}"###,
            r###"{
  "intent": "Contains ```json and ``` fence text",
  "inputs": [
    "untouched"
  ]
}"###,
            r###"{
  "intent": "```json",
  "inputs": ["untouched"],
  "outputs": ["```"]
}"###,
        ] {
            assert_eq!(normalize_candidate_json(input), input);
        }
    }

    #[test]
    fn normalize_candidate_json_repairs_only_structural_trailing_commas() {
        let input = r###"{"intent":"Literal ,} and ,] plus \"quoted\" and \\ slashes λ","inputs":["a,]",],}"###;
        let expected = r###"{"intent":"Literal ,} and ,] plus \"quoted\" and \\ slashes λ","inputs":["a,]"]}"###;
        assert_eq!(normalize_candidate_json(input), expected);
        let fenced = format!("```json\n{input}\n```");
        assert_eq!(normalize_candidate_json(&fenced), expected);
        assert!(serde_json::from_str::<Value>(expected).is_ok());
    }

    /// A complete annotation whose string fields hold fence markers and the comma
    /// sequences the repair step looks for.
    fn quoted_annotation(intent: &str) -> Value {
        serde_json::json!({
            "intent": intent,
            "behavior": "preserve this optional field",
            "inputs": ["literal ,] input", "second input"],
            "outputs": ["literal ``` output", "result"],
            "side_effects": ["none"],
            "dependencies": ["module::thing"],
            "error_modes": ["described error"],
            "confidence": 0.87,
            "edge_cases": "preserve this too",
            "complexity": "medium",
            "method_dependencies": {"method": ["callee", "literal ```json"]}
        })
    }

    fn assert_parses_to(candidate: &str, expected: &Value) {
        let expected: SirAnnotation =
            serde_json::from_value(expected.clone()).expect("fixture is a SIR");
        let actual = parse_and_validate_sir(candidate).expect("a complete valid SIR must parse");
        assert_eq!(actual, expected, "annotation fields must survive unchanged");
    }

    #[test]
    fn parse_and_validate_sir_keeps_fence_markers_inside_strings() {
        let value = quoted_annotation("```json and ``` are literal string content");
        assert_parses_to(&serde_json::to_string_pretty(&value).unwrap(), &value);

        let mut split = quoted_annotation("```json");
        split["outputs"] = serde_json::json!(["```"]);
        assert_parses_to(&serde_json::to_string_pretty(&split).unwrap(), &split);
    }

    #[test]
    fn parse_and_validate_sir_unwraps_fences_whose_json_quotes_fence_markers() {
        let value = quoted_annotation("literal ``` closing marker and ,} punctuation");
        let text = serde_json::to_string_pretty(&value).unwrap();
        assert_parses_to(&format!("preface\n```JSON\n{text}\n```\nafter"), &value);
        assert_parses_to(&format!("```\n{text}\n```"), &value);

        let unicode = quoted_annotation("λ Ω literal ``` and escaped \"quotes\" plus \\ backslash");
        let text = serde_json::to_string_pretty(&unicode).unwrap();
        assert_parses_to(
            &format!("λ preliminary text\n```json\n{text}\n```"),
            &unicode,
        );
    }

    #[test]
    fn parse_and_validate_sir_finds_fences_after_quoted_prose() {
        let value = quoted_annotation("selected annotation");
        let text = serde_json::to_string(&value).unwrap();
        assert_parses_to(
            &format!("A prose \"quote\" precedes this response:\n```json\n{text}\n```"),
            &value,
        );
    }

    #[test]
    fn parse_and_validate_sir_prefers_the_json_tagged_fence() {
        let decoy = quoted_annotation("generic decoy");
        let selected = quoted_annotation("JSON tagged selection");
        assert_parses_to(
            &format!("```text\n{decoy}\n```\n```json\n{selected}\n```"),
            &selected,
        );
    }

    #[test]
    fn parse_and_validate_sir_finds_fences_after_prose_the_scanner_misreads() {
        // An unmatched brace followed by an odd number of quotes leaves the
        // string-aware scanner inside a "string"; the plain fence search still works.
        let value = quoted_annotation("selected annotation");
        let text = serde_json::to_string_pretty(&value).unwrap();
        assert_parses_to(
            &format!("Treats \"{{\" as an opener.\n```json\n{text}\n```"),
            &value,
        );
        assert_parses_to(
            &format!("```rust\nfn q(c: char) -> bool {{ c == '\"' }}\n```\n```json\n{text}\n```"),
            &value,
        );
    }

    #[test]
    fn parse_and_validate_sir_unwraps_an_annotation_inside_a_bare_array() {
        let value = quoted_annotation("wrapped in an array");
        let text = serde_json::to_string(&value).unwrap();
        assert_parses_to(&format!("[{text}]"), &value);
    }

    #[test]
    fn parse_and_validate_sir_repairs_trailing_commas_without_touching_strings() {
        let value = quoted_annotation("literal ,} and ,] punctuation");
        let mut text = serde_json::to_string(&value).unwrap();
        text.insert(text.len() - 1, ',');
        assert_parses_to(&text, &value);
        assert_parses_to(&format!("```json\n{text}\n```"), &value);
    }

    #[test]
    fn parse_and_validate_sir_still_rejects_invalid_annotations() {
        let mut wrong_type = quoted_annotation("valid intent");
        wrong_type["inputs"] = serde_json::json!(42);
        for input in [
            "not JSON".to_owned(),
            "{}".to_owned(),
            wrong_type.to_string(),
        ] {
            let error = parse_and_validate_sir(&input).unwrap_err();
            assert!(error.starts_with("json parse error:"), "{error}");
        }
        let mut out_of_range = quoted_annotation("valid intent");
        out_of_range["confidence"] = serde_json::json!(2.0);
        let error = parse_and_validate_sir(&out_of_range.to_string()).unwrap_err();
        assert!(error.starts_with("sir validation error:"), "{error}");
    }

    #[tokio::test]
    async fn valid_quoted_annotation_does_not_spend_a_feedback_retry() {
        let value = quoted_annotation("literal ```json and ``` markers plus ,] punctuation");
        let expected: SirAnnotation = serde_json::from_value(value.clone()).unwrap();
        let candidate = Arc::new(serde_json::to_string_pretty(&value).unwrap());
        let first_calls = Arc::new(AtomicUsize::new(0));
        let feedback_calls = Arc::new(AtomicUsize::new(0));

        let actual = run_sir_parse_validation_retries_with_feedback(
            2,
            {
                let candidate = candidate.clone();
                let calls = first_calls.clone();
                move || {
                    let candidate = candidate.clone();
                    let calls = calls.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Ok(candidate.as_ref().clone())
                    }
                }
            },
            {
                let candidate = candidate.clone();
                let calls = feedback_calls.clone();
                move |_previous: String, _error: String| {
                    let candidate = candidate.clone();
                    let calls = calls.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Ok(candidate.as_ref().clone())
                    }
                }
            },
        )
        .await
        .expect("a valid annotation must be accepted on the first attempt");

        assert_eq!(actual, expected);
        assert_eq!(first_calls.load(Ordering::SeqCst), 1);
        assert_eq!(feedback_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn parse_and_validate_sir_accepts_method_dependencies() {
        let candidate = r#"{"intent":"valid","inputs":[],"outputs":[],"side_effects":[],"dependencies":["Record","StoreError"],"error_modes":[],"confidence":0.9,"method_dependencies":{"load":["Record","StoreError"]}}"#;

        let sir = parse_and_validate_sir(candidate).expect("candidate should validate");
        let method_dependencies = sir
            .method_dependencies
            .expect("method dependencies should be present");
        assert_eq!(
            method_dependencies.get("load"),
            Some(&vec!["Record".to_owned(), "StoreError".to_owned()])
        );
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

#[cfg(test)]
mod think_block_tests {
    use super::{parse_and_validate_sir, split_think_block};

    const SIR_JSON: &str = r#"{"intent":"Parses config","inputs":["path"],"outputs":["Config"],"side_effects":[],"dependencies":[],"error_modes":["io error"],"confidence":0.8}"#;

    #[test]
    fn split_think_block_extracts_reasoning_and_leaves_json() {
        let raw = format!(
            "<think>\nFirst I look at the signature.\nThen the body.\n</think>\n{SIR_JSON}"
        );
        let (reasoning, remainder) = split_think_block(&raw);
        assert_eq!(
            reasoning.as_deref(),
            Some("First I look at the signature.\nThen the body.")
        );
        assert_eq!(remainder.trim(), SIR_JSON);
        assert!(parse_and_validate_sir(&remainder).is_ok());
    }

    #[test]
    fn split_think_block_without_block_is_passthrough() {
        let (reasoning, remainder) = split_think_block(SIR_JSON);
        assert_eq!(reasoning, None);
        assert_eq!(remainder, SIR_JSON);
        let (reasoning, remainder) = split_think_block("<think></think>  {}");
        assert_eq!(reasoning, None);
        assert_eq!(remainder.trim(), "{}");
    }

    #[test]
    fn split_think_block_accepts_the_prompted_thinking_tag() {
        // sir_prompt.rs asks deep runs to wrap analysis in <thinking> tags.
        let raw = format!("<thinking>\nCompare with neighbours.\n</thinking>\n{SIR_JSON}");
        let (reasoning, remainder) = split_think_block(&raw);
        assert_eq!(reasoning.as_deref(), Some("Compare with neighbours."));
        assert_eq!(remainder.trim(), SIR_JSON);
        // Mixed tags in one reply are all captured, in order.
        let (reasoning, remainder) = split_think_block("<think>a</think><thinking>b</thinking>{}");
        assert_eq!(reasoning.as_deref(), Some("a\n\nb"));
        assert_eq!(remainder, "{}");
    }

    #[test]
    fn split_think_block_handles_multiple_and_unterminated_blocks() {
        let (reasoning, remainder) = split_think_block("<think>a</think>x<think>b</think>y");
        assert_eq!(reasoning.as_deref(), Some("a\n\nb"));
        assert_eq!(remainder, "xy");
        let (reasoning, remainder) = split_think_block("{}<think>never closed");
        assert_eq!(reasoning.as_deref(), Some("never closed"));
        assert_eq!(remainder, "{}");
    }
}
