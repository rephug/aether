/// LLM judge for contract clauses in the ambiguous cosine similarity range.
///
/// Uses the configured inference provider via `summarize_text_with_config` to
/// ask a yes/no question about whether the SIR satisfies a contract clause.
use std::path::Path;

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct JudgeResult {
    pub violated: bool,
    #[allow(dead_code)]
    pub reason: String,
}

/// Ask the LLM to judge whether a contract clause is satisfied by the SIR.
///
/// Blocks the current thread to run the async inference call. Returns a
/// default non-violated result if the judge is unavailable.
pub fn judge_clause(
    clause_text: &str,
    clause_type: &str,
    sir_json: &str,
    workspace_root: &Path,
) -> Result<JudgeResult> {
    let system_prompt = "You are verifying whether a code implementation satisfies a semantic \
                         contract. Respond with ONLY a JSON object: \
                         {\"violated\": true/false, \"reason\": \"brief explanation\"}";

    let sir_excerpt = if sir_json.len() > 2000 {
        &sir_json[..2000]
    } else {
        sir_json
    };

    let user_prompt = format!(
        "Contract type: {clause_type}\n\
         Contract clause: \"{clause_text}\"\n\
         Implementation description (from SIR):\n{sir_excerpt}\n\n\
         Does the implementation satisfy this contract clause?"
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to create judge runtime")?;

    let result = runtime.block_on(aether_infer::summarize_text_with_config(
        workspace_root,
        system_prompt,
        &user_prompt,
    ));

    match result {
        Ok(Some(text)) => parse_judge_response(&text),
        Ok(None) => Ok(JudgeResult {
            violated: false,
            reason: "judge returned empty response".to_owned(),
        }),
        Err(err) => {
            tracing::debug!(error = %err, "LLM judge call failed, defaulting to non-violated");
            Ok(JudgeResult {
                violated: false,
                reason: format!("judge unavailable: {err}"),
            })
        }
    }
}

fn parse_judge_response(text: &str) -> Result<JudgeResult> {
    // Try to find JSON in the response (the LLM may wrap it in markdown)
    let json_str = extract_json_object(text).unwrap_or(text);

    #[derive(serde::Deserialize)]
    struct RawJudge {
        #[serde(default)]
        violated: bool,
        #[serde(default)]
        reason: String,
    }

    match serde_json::from_str::<RawJudge>(json_str) {
        Ok(raw) => Ok(JudgeResult {
            violated: raw.violated,
            reason: raw.reason,
        }),
        Err(_) => {
            // Fallback: look for "violated" keyword in raw text
            let lower = text.to_ascii_lowercase();
            let violated =
                lower.contains("\"violated\": true") || lower.contains("\"violated\":true");
            Ok(JudgeResult {
                violated,
                reason: text.chars().take(200).collect(),
            })
        }
    }
}

/// Extract the first `{...}` JSON object from a text that may contain
/// markdown fences or other wrapping.
fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0;
    for (i, ch) in text[start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..start + i + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_clean_json_response() {
        let result =
            parse_judge_response(r#"{"violated": true, "reason": "missing validation"}"#).unwrap();
        assert!(result.violated);
        assert_eq!(result.reason, "missing validation");
    }

    #[test]
    fn parse_json_in_markdown_fence() {
        let text = "```json\n{\"violated\": false, \"reason\": \"looks good\"}\n```";
        let result = parse_judge_response(text).unwrap();
        assert!(!result.violated);
        assert_eq!(result.reason, "looks good");
    }

    #[test]
    fn parse_malformed_response_falls_back() {
        let text = "The implementation violated: true the contract";
        let result = parse_judge_response(text).unwrap();
        // Fallback heuristic shouldn't match since it's not exact JSON pattern
        assert!(!result.violated);
    }

    #[test]
    fn extract_json_object_finds_first_object() {
        let text = "Some text {\"key\": \"value\"} more text";
        assert_eq!(extract_json_object(text), Some("{\"key\": \"value\"}"));
    }

    #[test]
    fn extract_json_object_handles_nested() {
        let text = "{\"outer\": {\"inner\": 1}}";
        assert_eq!(
            extract_json_object(text),
            Some("{\"outer\": {\"inner\": 1}}")
        );
    }

    #[test]
    fn extract_json_object_no_json() {
        assert_eq!(extract_json_object("no json here"), None);
    }
}
