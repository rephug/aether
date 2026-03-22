use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use serde_json::json;

use super::{BatchPollStatus, BatchProvider, BatchResultLine};

const OPENAI_API_BASE: &str = "https://api.openai.com/v1";
const OPENAI_RESPONSES_ENDPOINT: &str = "/v1/responses";

pub(crate) struct OpenAiBatchProvider {
    client: reqwest::Client,
    api_key: String,
}

impl OpenAiBatchProvider {
    pub(crate) fn new(api_key: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
        }
    }

    /// Map raw thinking string to OpenAI reasoning effort (lowercase).
    /// Returns None for "off"/"none"/"" → omit reasoning block.
    fn openai_reasoning_effort(thinking: &str) -> Option<&'static str> {
        match thinking.trim().to_ascii_lowercase().as_str() {
            "low" => Some("low"),
            "medium" => Some("medium"),
            "high" => Some("high"),
            _ => None,
        }
    }

    fn auth_header(&self) -> String {
        format!("Bearer {}", self.api_key)
    }
}

#[async_trait::async_trait]
impl BatchProvider for OpenAiBatchProvider {
    fn format_request(
        &self,
        key: &str,
        system_prompt: &str,
        user_prompt: &str,
        model: &str,
        thinking: &str,
    ) -> Result<String> {
        let mut body = json!({
            "model": model,
            "instructions": system_prompt,
            "input": [
                { "role": "user", "content": format!("{user_prompt}\n\nRespond with ONLY valid JSON.") }
            ],
            "text": {
                "format": { "type": "json_object" }
            },
            "store": false
        });

        if let Some(effort) = Self::openai_reasoning_effort(thinking) {
            body["reasoning"] = json!({ "effort": effort });
        } else {
            body["temperature"] = json!(0.0);
        }

        let line = json!({
            "custom_id": key,
            "method": "POST",
            "url": OPENAI_RESPONSES_ENDPOINT,
            "body": body
        });

        serde_json::to_string(&line).context("failed to serialize OpenAI batch request line")
    }

    async fn submit(
        &self,
        input_path: &Path,
        _model: &str,
        _batch_dir: &Path,
        _poll_interval_secs: u64,
    ) -> Result<Vec<String>> {
        let file_bytes = std::fs::read(input_path)
            .with_context(|| format!("failed to read batch input {}", input_path.display()))?;
        let file_name = input_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("batch_input.jsonl")
            .to_owned();

        // --- Phase 1: Upload file ---
        let file_part = reqwest::multipart::Part::bytes(file_bytes)
            .file_name(file_name)
            .mime_str("application/jsonl")
            .context("failed to set MIME type for OpenAI file upload")?;

        let form = reqwest::multipart::Form::new()
            .text("purpose", "batch")
            .part("file", file_part);

        let upload_resp = self
            .client
            .post(format!("{}/files", OPENAI_API_BASE))
            .header("Authorization", self.auth_header())
            .multipart(form)
            .send()
            .await
            .context("OpenAI file upload request failed")?;

        if !upload_resp.status().is_success() {
            let status = upload_resp.status();
            let body = upload_resp
                .text()
                .await
                .unwrap_or_else(|_| "<no body>".to_owned());
            return Err(anyhow!(
                "OpenAI file upload failed (HTTP {}): {}",
                status,
                body
            ));
        }

        let upload_json: serde_json::Value = upload_resp
            .json()
            .await
            .context("failed to parse OpenAI file upload response")?;
        let file_id = upload_json["id"]
            .as_str()
            .ok_or_else(|| anyhow!("OpenAI file upload response missing id"))?
            .to_owned();

        tracing::info!(file_id = %file_id, "uploaded batch input to OpenAI");

        // --- Phase 2: Create batch ---
        let create_body = json!({
            "input_file_id": file_id,
            "endpoint": OPENAI_RESPONSES_ENDPOINT,
            "completion_window": "24h"
        });

        let create_resp = self
            .client
            .post(format!("{}/batches", OPENAI_API_BASE))
            .header("Authorization", self.auth_header())
            .header("Content-Type", "application/json")
            .body(create_body.to_string())
            .send()
            .await
            .context("OpenAI batch create request failed")?;

        if !create_resp.status().is_success() {
            let status = create_resp.status();
            let body = create_resp
                .text()
                .await
                .unwrap_or_else(|_| "<no body>".to_owned());
            return Err(anyhow!(
                "OpenAI batch create failed (HTTP {}): {}",
                status,
                body
            ));
        }

        let create_json: serde_json::Value = create_resp
            .json()
            .await
            .context("failed to parse OpenAI batch create response")?;
        let batch_id = create_json["id"]
            .as_str()
            .ok_or_else(|| anyhow!("OpenAI batch create response missing id"))?
            .to_owned();

        tracing::info!(batch_id = %batch_id, "OpenAI batch job created");
        Ok(vec![batch_id])
    }

    async fn poll(&self, job_ids: &[String]) -> Result<BatchPollStatus> {
        let batch_id = job_ids
            .first()
            .ok_or_else(|| anyhow!("no OpenAI batch job ID to poll"))?;

        let resp = self
            .client
            .get(format!("{}/batches/{}", OPENAI_API_BASE, batch_id))
            .header("Authorization", self.auth_header())
            .send()
            .await
            .with_context(|| format!("OpenAI batch poll failed for {}", batch_id))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_else(|_| "<no body>".to_owned());
            return Err(anyhow!(
                "OpenAI batch poll failed (HTTP {}): {}",
                status,
                body
            ));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .context("failed to parse OpenAI batch poll response")?;

        let status_str = json["status"].as_str().unwrap_or("");
        match status_str {
            "completed" => Ok(BatchPollStatus::Completed),
            "failed" => {
                let msg = json
                    .pointer("/errors/data/0/message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("batch job failed");
                Ok(BatchPollStatus::Failed {
                    message: msg.to_owned(),
                })
            }
            "expired" | "cancelled" | "cancelling" => Ok(BatchPollStatus::Failed {
                message: format!("batch job {}", status_str),
            }),
            _ => {
                let completed = json
                    .pointer("/request_counts/completed")
                    .and_then(|v| v.as_u64());
                let total = json
                    .pointer("/request_counts/total")
                    .and_then(|v| v.as_u64());
                Ok(BatchPollStatus::InProgress { completed, total })
            }
        }
    }

    async fn download_results(
        &self,
        job_ids: &[String],
        output_dir: &Path,
    ) -> Result<Vec<PathBuf>> {
        let batch_id = job_ids
            .first()
            .ok_or_else(|| anyhow!("no OpenAI batch job ID for download"))?;

        // Get batch details to extract output_file_id
        let resp = self
            .client
            .get(format!("{}/batches/{}", OPENAI_API_BASE, batch_id))
            .header("Authorization", self.auth_header())
            .send()
            .await
            .context("OpenAI batch status request failed for download")?;

        let json: serde_json::Value = resp
            .json()
            .await
            .context("failed to parse OpenAI batch status for download")?;

        let output_file_id = json["output_file_id"]
            .as_str()
            .ok_or_else(|| anyhow!("OpenAI batch response missing output_file_id"))?;

        let mut result_paths = Vec::new();

        // Download output file
        let result_path = output_dir.join(format!("{}.results.jsonl", batch_id));
        self.download_file(output_file_id, &result_path).await?;
        result_paths.push(result_path);

        // Download error file if present
        if let Some(error_file_id) = json["error_file_id"].as_str()
            && !error_file_id.is_empty()
        {
            let error_path = output_dir.join(format!("{}.errors.jsonl", batch_id));
            if let Err(err) = self.download_file(error_file_id, &error_path).await {
                tracing::warn!(error = %err, "failed to download OpenAI error file");
            }
        }

        tracing::info!(
            path = %result_paths[0].display(),
            "downloaded OpenAI batch results"
        );
        Ok(result_paths)
    }

    fn parse_result_line(&self, line: &str) -> Result<BatchResultLine> {
        let parsed: OpenAiResponseLine =
            serde_json::from_str(line).context("failed to parse OpenAI batch response line")?;

        let Some(ref response) = parsed.response else {
            return Ok(BatchResultLine::Error {
                key: Some(parsed.custom_id),
                message: "OpenAI response line missing response field".to_owned(),
            });
        };

        if response.status_code != 200 {
            return Ok(BatchResultLine::Error {
                key: Some(parsed.custom_id),
                message: format!("OpenAI response status {}", response.status_code),
            });
        }

        let text = response
            .body
            .as_ref()
            .and_then(Self::extract_text_from_body)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| anyhow!("OpenAI response line missing output text"))?;

        Ok(BatchResultLine::Success {
            key: parsed.custom_id,
            text: text.to_owned(),
        })
    }

    fn name(&self) -> &str {
        "openai"
    }
}

impl OpenAiBatchProvider {
    fn extract_text_from_body(body: &serde_json::Value) -> Option<&serde_json::Value> {
        Self::extract_responses_output_text(body)
            .or_else(|| body.pointer("/choices/0/message/content"))
    }

    fn extract_responses_output_text(body: &serde_json::Value) -> Option<&serde_json::Value> {
        body.get("output")
            .and_then(|output| output.as_array())
            .and_then(|output| {
                output.iter().find_map(|item| {
                    if item.get("type").and_then(|value| value.as_str()) != Some("message") {
                        return None;
                    }

                    item.get("content")
                        .and_then(|content| content.as_array())
                        .and_then(|content| {
                            content.iter().find_map(|part| {
                                if part.get("type").and_then(|value| value.as_str())
                                    == Some("output_text")
                                {
                                    part.get("text")
                                } else {
                                    None
                                }
                            })
                        })
                })
            })
    }

    async fn download_file(&self, file_id: &str, dest: &Path) -> Result<()> {
        let resp = self
            .client
            .get(format!("{}/files/{}/content", OPENAI_API_BASE, file_id))
            .header("Authorization", self.auth_header())
            .send()
            .await
            .with_context(|| format!("OpenAI file download failed for {}", file_id))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_else(|_| "<no body>".to_owned());
            return Err(anyhow!(
                "OpenAI file download failed (HTTP {}): {}",
                status,
                body
            ));
        }

        let bytes = resp
            .bytes()
            .await
            .context("failed to read OpenAI file download body")?;
        std::fs::write(dest, &bytes)
            .with_context(|| format!("failed to write OpenAI results to {}", dest.display()))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// OpenAI-specific response structs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct OpenAiResponseLine {
    custom_id: String,
    #[serde(default)]
    response: Option<OpenAiResponse>,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    status_code: u16,
    #[serde(default)]
    body: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> OpenAiBatchProvider {
        OpenAiBatchProvider::new("test-key".to_owned())
    }

    #[test]
    fn format_request_separates_system_user() {
        let p = provider();
        let line = p
            .format_request(
                "sym1|hash1",
                "System instructions.",
                "User content.",
                "gpt-4o",
                "off",
            )
            .unwrap();
        let json: serde_json::Value = serde_json::from_str(&line).unwrap();

        assert_eq!(json["custom_id"], "sym1|hash1");
        assert_eq!(json["method"], "POST");
        assert_eq!(json["url"], OPENAI_RESPONSES_ENDPOINT);
        assert_eq!(json["body"]["model"], "gpt-4o");
        assert_eq!(json["body"]["instructions"], "System instructions.");
        assert_eq!(json["body"]["input"][0]["role"], "user");
        assert_eq!(
            json["body"]["input"][0]["content"],
            "User content.\n\nRespond with ONLY valid JSON."
        );
        assert_eq!(json["body"]["text"]["format"]["type"], "json_object");
        assert_eq!(json["body"]["store"], false);
        assert_eq!(json["body"]["temperature"], 0.0);
        assert!(json["body"].get("reasoning").is_none());
    }

    #[test]
    fn format_request_includes_reasoning_effort() {
        let p = provider();
        let line = p
            .format_request("sym2|hash2", "Sys.", "Usr.", "gpt-4o", "medium")
            .unwrap();
        let json: serde_json::Value = serde_json::from_str(&line).unwrap();

        assert_eq!(json["body"]["reasoning"]["effort"], "medium");
        assert!(json["body"].get("temperature").is_none());
    }

    #[test]
    fn format_request_omits_reasoning_for_none() {
        let p = provider();
        for level in ["none", "off", ""] {
            let line = p
                .format_request("sym3|hash3", "Sys.", "Usr.", "gpt-4o", level)
                .unwrap();
            let json: serde_json::Value = serde_json::from_str(&line).unwrap();

            assert!(
                json["body"].get("reasoning").is_none(),
                "reasoning should be absent for level '{}'",
                level
            );
            assert_eq!(json["body"]["temperature"], 0.0);
        }
    }

    #[test]
    fn parse_result_line_success_responses_api() {
        let p = provider();
        let input = r#"{"custom_id":"sym1|hash1","response":{"status_code":200,"body":{"output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"{\"intent\":\"test\"}"}]}]}}}"#;
        match p.parse_result_line(input).unwrap() {
            BatchResultLine::Success { key, text } => {
                assert_eq!(key, "sym1|hash1");
                assert_eq!(text, r#"{"intent":"test"}"#);
            }
            BatchResultLine::Error { .. } => panic!("expected success"),
        }
    }

    #[test]
    fn parse_result_line_success_responses_api_skips_reasoning_blocks() {
        let p = provider();
        let input = r#"{"custom_id":"sym1|hash1","response":{"status_code":200,"body":{"output":[{"type":"reasoning","summary":[{"type":"summary_text","text":"thinking"}]},{"type":"message","role":"assistant","content":[{"type":"output_text","text":"{\"intent\":\"reasoned\"}"}]}]}}}"#;
        match p.parse_result_line(input).unwrap() {
            BatchResultLine::Success { key, text } => {
                assert_eq!(key, "sym1|hash1");
                assert_eq!(text, r#"{"intent":"reasoned"}"#);
            }
            BatchResultLine::Error { .. } => panic!("expected success"),
        }
    }

    #[test]
    fn parse_result_line_success_legacy_chat_completions() {
        let p = provider();
        let input = r#"{"custom_id":"sym1|hash1","response":{"status_code":200,"body":{"choices":[{"message":{"content":"{\"intent\":\"legacy\"}"}}]}}}"#;
        match p.parse_result_line(input).unwrap() {
            BatchResultLine::Success { key, text } => {
                assert_eq!(key, "sym1|hash1");
                assert_eq!(text, r#"{"intent":"legacy"}"#);
            }
            BatchResultLine::Error { .. } => panic!("expected success"),
        }
    }

    #[test]
    fn parse_result_line_error_status() {
        let p = provider();
        let input = r#"{"custom_id":"sym1|hash1","response":{"status_code":429,"body":{"error":{"message":"rate limited"}}}}"#;
        match p.parse_result_line(input).unwrap() {
            BatchResultLine::Error { key, message } => {
                assert_eq!(key, Some("sym1|hash1".to_owned()));
                assert!(message.contains("429"));
            }
            BatchResultLine::Success { .. } => panic!("expected error"),
        }
    }
}
