use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use serde_json::json;

use super::{BatchPollStatus, BatchProvider, BatchResultLine};

const GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com";

pub(crate) struct GeminiBatchProvider {
    client: reqwest::Client,
    api_key: String,
}

impl GeminiBatchProvider {
    pub(crate) fn new(api_key: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
        }
    }

    /// Map raw thinking string to Gemini's thinkingLevel constant.
    fn gemini_thinking_level(thinking: &str) -> Option<&'static str> {
        match thinking.trim().to_ascii_lowercase().as_str() {
            "low" => Some("LOW"),
            "medium" => Some("MEDIUM"),
            "high" => Some("HIGH"),
            "dynamic" => Some("DYNAMIC"),
            // "off", "none", "" → omit thinkingConfig entirely
            _ => None,
        }
    }
}

#[async_trait::async_trait]
impl BatchProvider for GeminiBatchProvider {
    fn format_request(
        &self,
        key: &str,
        system_prompt: &str,
        user_prompt: &str,
        _model: &str,
        thinking: &str,
    ) -> Result<String> {
        // Gemini batch: concatenate system+user into a single text field
        // (matches pre-trait behavior where one prompt blob was sent).
        let combined = format!("{}\n\n{}", system_prompt, user_prompt);

        let mut gen_config = json!({
            "responseMimeType": "application/json",
            "temperature": 0.0,
        });

        if let Some(level) = Self::gemini_thinking_level(thinking) {
            gen_config["thinkingConfig"] = json!({ "thinkingLevel": level });
        }

        let line = json!({
            "key": key,
            "request": {
                "contents": [{
                    "parts": [{
                        "text": combined
                    }]
                }],
                "generationConfig": gen_config
            }
        });

        serde_json::to_string(&line).context("failed to serialize Gemini batch request line")
    }

    async fn submit(
        &self,
        input_path: &Path,
        model: &str,
        _batch_dir: &Path,
        _poll_interval_secs: u64,
    ) -> Result<Vec<String>> {
        let file_bytes = std::fs::read(input_path)
            .with_context(|| format!("failed to read batch input {}", input_path.display()))?;
        let num_bytes = file_bytes.len();
        let display_name = input_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("batch_input");

        // --- Phase 1: Start resumable upload ---
        let start_resp = self
            .client
            .post(format!("{}/upload/v1beta/files", GEMINI_API_BASE))
            .header("x-goog-api-key", &self.api_key)
            .header("X-Goog-Upload-Protocol", "resumable")
            .header("X-Goog-Upload-Command", "start")
            .header("X-Goog-Upload-Header-Content-Length", num_bytes.to_string())
            .header("X-Goog-Upload-Header-Content-Type", "application/jsonl")
            .header("Content-Type", "application/json")
            .body(json!({"file": {"display_name": display_name}}).to_string())
            .send()
            .await
            .context("Gemini resumable upload start request failed")?;

        if !start_resp.status().is_success() {
            let status = start_resp.status();
            let body = start_resp
                .text()
                .await
                .unwrap_or_else(|_| "<no body>".to_owned());
            return Err(anyhow!(
                "Gemini upload start failed (HTTP {}): {}",
                status,
                body
            ));
        }

        // Extract upload URL (case-insensitive header lookup)
        let upload_url = start_resp
            .headers()
            .get("x-goog-upload-url")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
            .ok_or_else(|| {
                anyhow!("Gemini upload start response missing x-goog-upload-url header")
            })?;

        // --- Phase 2: Upload + finalize ---
        let upload_resp = self
            .client
            .put(&upload_url)
            .header("Content-Length", num_bytes.to_string())
            .header("X-Goog-Upload-Offset", "0")
            .header("X-Goog-Upload-Command", "upload, finalize")
            .body(file_bytes)
            .send()
            .await
            .context("Gemini file upload finalize failed")?;

        if !upload_resp.status().is_success() {
            let status = upload_resp.status();
            let body = upload_resp
                .text()
                .await
                .unwrap_or_else(|_| "<no body>".to_owned());
            return Err(anyhow!(
                "Gemini file upload finalize failed (HTTP {}): {}",
                status,
                body
            ));
        }

        let upload_json: serde_json::Value = upload_resp
            .json()
            .await
            .context("failed to parse Gemini upload response JSON")?;
        let file_name = upload_json
            .pointer("/file/name")
            .or_else(|| upload_json.get("name"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Gemini upload response missing file name"))?
            .to_owned();

        // --- Phase 3: Create batch job ---
        let create_body = json!({
            "batch": {
                "display_name": display_name,
                "input_config": {
                    "file_name": file_name
                }
            }
        });

        let create_resp = self
            .client
            .post(format!(
                "{}/v1beta/models/{}:batchGenerateContent",
                GEMINI_API_BASE, model
            ))
            .header("x-goog-api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .body(create_body.to_string())
            .send()
            .await
            .context("Gemini batch create request failed")?;

        if !create_resp.status().is_success() {
            let status = create_resp.status();
            let body = create_resp
                .text()
                .await
                .unwrap_or_else(|_| "<no body>".to_owned());
            return Err(anyhow!(
                "Gemini batch create failed (HTTP {}): {}",
                status,
                body
            ));
        }

        let create_json: serde_json::Value = create_resp
            .json()
            .await
            .context("failed to parse Gemini batch create response")?;
        let batch_name = create_json
            .pointer("/batch/name")
            .or_else(|| create_json.get("name"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Gemini batch create response missing batch name"))?
            .to_owned();

        tracing::info!(batch_name = %batch_name, "Gemini batch job created");
        Ok(vec![batch_name])
    }

    async fn poll(&self, job_ids: &[String]) -> Result<BatchPollStatus> {
        let batch_name = job_ids
            .first()
            .ok_or_else(|| anyhow!("no Gemini batch job ID to poll"))?;

        let resp = self
            .client
            .get(format!("{}/v1beta/{}", GEMINI_API_BASE, batch_name))
            .header("x-goog-api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .send()
            .await
            .with_context(|| format!("Gemini batch poll failed for {}", batch_name))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_else(|_| "<no body>".to_owned());
            return Err(anyhow!(
                "Gemini batch poll failed (HTTP {}): {}",
                status,
                body
            ));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .context("failed to parse Gemini batch poll response")?;

        let state = json
            .pointer("/metadata/state")
            .or_else(|| json.get("state"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        match state {
            "BATCH_STATE_SUCCEEDED" => Ok(BatchPollStatus::Completed),
            "BATCH_STATE_FAILED" => {
                let msg = json
                    .pointer("/error/message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("batch job failed");
                Ok(BatchPollStatus::Failed {
                    message: msg.to_owned(),
                })
            }
            "BATCH_STATE_CANCELLED" | "BATCH_STATE_EXPIRED" => Ok(BatchPollStatus::Failed {
                message: format!("batch job {}", state),
            }),
            "" => Err(anyhow!("Gemini batch poll response missing state field")),
            _ => Ok(BatchPollStatus::InProgress {
                completed: None,
                total: None,
            }),
        }
    }

    async fn download_results(
        &self,
        job_ids: &[String],
        output_dir: &Path,
    ) -> Result<Vec<PathBuf>> {
        let batch_name = job_ids
            .first()
            .ok_or_else(|| anyhow!("no Gemini batch job ID for download"))?;

        // Get batch status to extract responses file
        let resp = self
            .client
            .get(format!("{}/v1beta/{}", GEMINI_API_BASE, batch_name))
            .header("x-goog-api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("Gemini batch status request failed for download")?;

        let json: serde_json::Value = resp
            .json()
            .await
            .context("failed to parse Gemini batch status for download")?;

        let responses_file = json
            .pointer("/response/responsesFile")
            .or_else(|| json.pointer("/output/responsesFile"))
            .or_else(|| json.pointer("/dest/fileName"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Gemini batch response missing responsesFile"))?;

        // Extract display name from batch name for output filename
        let display_name = json
            .pointer("/batch/displayName")
            .or_else(|| json.get("displayName"))
            .and_then(|v| v.as_str())
            .unwrap_or("batch_results");

        let result_path = output_dir.join(format!("{}.results.jsonl", display_name));

        let download_resp = self
            .client
            .get(format!(
                "{}/download/v1beta/{}:download?alt=media",
                GEMINI_API_BASE, responses_file
            ))
            .header("x-goog-api-key", &self.api_key)
            .send()
            .await
            .context("Gemini results download failed")?;

        if !download_resp.status().is_success() {
            let status = download_resp.status();
            let body = download_resp
                .text()
                .await
                .unwrap_or_else(|_| "<no body>".to_owned());
            return Err(anyhow!(
                "Gemini results download failed (HTTP {}): {}",
                status,
                body
            ));
        }

        let bytes = download_resp
            .bytes()
            .await
            .context("failed to read Gemini results download body")?;
        std::fs::write(&result_path, &bytes).with_context(|| {
            format!(
                "failed to write Gemini results to {}",
                result_path.display()
            )
        })?;

        tracing::info!(path = %result_path.display(), "downloaded Gemini batch results");
        Ok(vec![result_path])
    }

    fn parse_result_line(&self, line: &str) -> Result<BatchResultLine> {
        let parsed: GeminiResponseLine =
            serde_json::from_str(line).context("failed to parse Gemini batch response line")?;

        if parsed.error.is_some() {
            return Ok(BatchResultLine::Error {
                key: Some(parsed.key),
                message: "batch response line contains an error envelope".to_owned(),
            });
        }

        let text = parsed
            .response
            .as_ref()
            .and_then(|r| r.candidates.first())
            .and_then(|c| c.content.parts.first())
            .and_then(|p| p.text.as_ref())
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
            .ok_or_else(|| anyhow!("Gemini response line missing candidate text"))?;

        Ok(BatchResultLine::Success {
            key: parsed.key,
            text: text.to_owned(),
        })
    }

    fn name(&self) -> &str {
        "gemini"
    }
}

// ---------------------------------------------------------------------------
// Gemini-specific response structs (moved from ingest.rs)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GeminiResponseLine {
    key: String,
    #[serde(default)]
    response: Option<GeminiResponse>,
    #[serde(default)]
    error: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct GeminiResponse {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: GeminiContent,
}

#[derive(Debug, Deserialize)]
struct GeminiContent {
    #[serde(default)]
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Deserialize)]
struct GeminiPart {
    #[serde(default)]
    text: Option<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> GeminiBatchProvider {
        GeminiBatchProvider::new("test-key".to_owned())
    }

    #[test]
    fn format_request_with_thinking() {
        let p = provider();
        let line = p
            .format_request(
                "sym1|hash1",
                "System prompt.",
                "User prompt.",
                "gemini-2.0-flash",
                "high",
            )
            .unwrap();
        let json: serde_json::Value = serde_json::from_str(&line).unwrap();

        assert_eq!(json["key"], "sym1|hash1");
        let text = json["request"]["contents"][0]["parts"][0]["text"]
            .as_str()
            .unwrap();
        assert!(text.contains("System prompt."));
        assert!(text.contains("User prompt."));
        assert_eq!(
            json["request"]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "HIGH"
        );
        assert_eq!(json["request"]["generationConfig"]["temperature"], 0.0);
    }

    #[test]
    fn format_request_omits_thinking_when_off() {
        let p = provider();
        let line = p
            .format_request("sym2|hash2", "Sys.", "Usr.", "gemini-2.0-flash", "off")
            .unwrap();
        let json: serde_json::Value = serde_json::from_str(&line).unwrap();

        assert!(
            json["request"]["generationConfig"]
                .get("thinkingConfig")
                .is_none()
        );
    }

    #[test]
    fn format_request_omits_thinking_when_none() {
        let p = provider();
        let line = p
            .format_request("sym3|hash3", "Sys.", "Usr.", "model", "none")
            .unwrap();
        let json: serde_json::Value = serde_json::from_str(&line).unwrap();

        assert!(
            json["request"]["generationConfig"]
                .get("thinkingConfig")
                .is_none()
        );
    }

    #[test]
    fn parse_result_line_success() {
        let p = provider();
        let input = r#"{"key":"sym1|hash1","response":{"candidates":[{"content":{"parts":[{"text":"{\"intent\":\"test\"}"}]}}]}}"#;
        match p.parse_result_line(input).unwrap() {
            BatchResultLine::Success { key, text } => {
                assert_eq!(key, "sym1|hash1");
                assert_eq!(text, r#"{"intent":"test"}"#);
            }
            BatchResultLine::Error { .. } => panic!("expected success"),
        }
    }

    #[test]
    fn parse_result_line_error() {
        let p = provider();
        let input = r#"{"key":"sym1|hash1","error":{"message":"rate limited"}}"#;
        match p.parse_result_line(input).unwrap() {
            BatchResultLine::Error { key, .. } => {
                assert_eq!(key, Some("sym1|hash1".to_owned()));
            }
            BatchResultLine::Success { .. } => panic!("expected error"),
        }
    }
}
