use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde_json::json;

use super::{BatchPollStatus, BatchProvider, BatchResultLine};

const ANTHROPIC_API_BASE: &str = "https://api.anthropic.com/v1";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const ANTHROPIC_BETA_EXTENDED_CACHE_TTL: &str = "extended-cache-ttl-2025-04-11";
const ANTHROPIC_CACHE_TTL: &str = "1h";

/// Maximum requests per Anthropic batch job.
const MAX_REQUESTS_PER_BATCH: usize = 10_000;
/// Safety margin below Anthropic's 32MB body limit.
const MAX_BODY_BYTES: usize = 30_000_000;

pub(crate) struct AnthropicBatchProvider {
    client: reqwest::Client,
    api_key: String,
}

impl AnthropicBatchProvider {
    pub(crate) fn new(api_key: String) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            api_key,
        }
    }

    /// Map raw thinking string to Anthropic budget_tokens.
    /// Returns `None` for "off"/"none"/"" → omit thinking parameter entirely.
    fn thinking_budget(thinking: &str) -> Option<u64> {
        match thinking.trim().to_ascii_lowercase().as_str() {
            "low" => Some(2048),
            "medium" => Some(4096),
            "high" => Some(8192),
            _ => None,
        }
    }

    fn with_anthropic_headers(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("anthropic-beta", ANTHROPIC_BETA_EXTENDED_CACHE_TTL)
    }
}

#[async_trait::async_trait]
impl BatchProvider for AnthropicBatchProvider {
    fn format_request(
        &self,
        key: &str,
        system_prompt: &str,
        user_prompt: &str,
        model: &str,
        thinking: &str,
    ) -> Result<String> {
        let budget = Self::thinking_budget(thinking);
        let max_tokens: u64 = match budget {
            Some(b) => b + 4096,
            None => 4096,
        };

        // System prompt uses array-of-blocks form with cache_control for prompt caching.
        // Every request in the batch shares the same system prompt, so Anthropic caches
        // it after the first read; a 1-hour TTL keeps the cache warm for long-running batches.
        let mut params = json!({
            "model": model,
            "max_tokens": max_tokens,
            "system": [
                {
                    "type": "text",
                    "text": system_prompt,
                    "cache_control": {
                        "type": "ephemeral",
                        "ttl": ANTHROPIC_CACHE_TTL
                    }
                }
            ],
            "messages": [
                { "role": "user", "content": user_prompt }
            ]
        });

        if let Some(b) = budget {
            params["thinking"] = json!({
                "type": "enabled",
                "budget_tokens": b
            });
        }

        // Anthropic limits custom_id to 64 chars matching ^[a-zA-Z0-9_-]{1,64}$.
        // Compound key is symbol_id|hashes... which exceeds 64 chars.
        // Use just the symbol_id (64 hex chars) which fits exactly.
        let truncated_key: String = match key.find('|') {
            Some(pos) => key[..pos].to_string(),
            None => {
                if key.len() > 64 {
                    key[..64].to_string()
                } else {
                    key.to_string()
                }
            }
        };
        let line = json!({
            "custom_id": truncated_key,
            "params": params
        });

        serde_json::to_string(&line).context("failed to serialize Anthropic batch request line")
    }

    async fn submit(
        &self,
        input_path: &Path,
        _model: &str,
        _batch_dir: &Path,
        _poll_interval_secs: u64,
    ) -> Result<Vec<String>> {
        let content = std::fs::read_to_string(input_path)
            .with_context(|| format!("failed to read batch input {}", input_path.display()))?;
        let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();

        if lines.is_empty() {
            return Err(anyhow!("batch input file is empty"));
        }

        let mut batch_ids = Vec::new();
        let mut chunk_start = 0;

        while chunk_start < lines.len() {
            let mut requests = Vec::new();
            let mut total_bytes = 0usize;

            for line in &lines[chunk_start..] {
                if requests.len() >= MAX_REQUESTS_PER_BATCH {
                    break;
                }
                if total_bytes + line.len() > MAX_BODY_BYTES && !requests.is_empty() {
                    break;
                }
                let parsed: serde_json::Value = serde_json::from_str(line).with_context(|| {
                    format!(
                        "failed to parse JSONL line {} as JSON",
                        chunk_start + requests.len()
                    )
                })?;
                total_bytes += line.len();
                requests.push(parsed);
            }

            let count = requests.len();
            if count == 0 {
                break;
            }

            let body = json!({ "requests": requests });

            let response = self
                .with_anthropic_headers(
                    self.client
                        .post(format!("{}/messages/batches", ANTHROPIC_API_BASE)),
                )
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
                .context("Anthropic batch create request failed")?;

            if !response.status().is_success() {
                let status = response.status();
                let resp_body = response
                    .text()
                    .await
                    .unwrap_or_else(|_| "<no body>".to_owned());
                return Err(anyhow!(
                    "Anthropic batch create failed (HTTP {}): {}",
                    status,
                    resp_body
                ));
            }

            let result: serde_json::Value = response
                .json()
                .await
                .context("failed to parse Anthropic batch create response")?;
            let batch_id = result["id"]
                .as_str()
                .ok_or_else(|| anyhow!("missing batch id in Anthropic response"))?
                .to_owned();

            tracing::info!(batch_id = %batch_id, requests = count, "submitted Anthropic batch");
            batch_ids.push(batch_id);
            chunk_start += count;
        }

        Ok(batch_ids)
    }

    async fn poll(&self, job_ids: &[String]) -> Result<BatchPollStatus> {
        if job_ids.is_empty() {
            return Err(anyhow!("no Anthropic batch job IDs to poll"));
        }

        let mut all_ended = true;
        let mut total_succeeded: u64 = 0;
        let mut total_errored: u64 = 0;
        let mut total_processing: u64 = 0;
        let mut total_canceled: u64 = 0;
        let mut total_expired: u64 = 0;

        for batch_id in job_ids {
            let resp = self
                .with_anthropic_headers(self.client.get(format!(
                    "{}/messages/batches/{}",
                    ANTHROPIC_API_BASE, batch_id
                )))
                .send()
                .await
                .with_context(|| format!("Anthropic batch poll failed for {}", batch_id))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_else(|_| "<no body>".to_owned());
                return Err(anyhow!(
                    "Anthropic batch poll failed (HTTP {}): {}",
                    status,
                    body
                ));
            }

            let json: serde_json::Value = resp
                .json()
                .await
                .context("failed to parse Anthropic batch poll response")?;

            let processing_status = json["processing_status"].as_str().unwrap_or("");
            match processing_status {
                "ended" => {
                    if let Some(counts) = json.get("request_counts") {
                        total_succeeded += counts["succeeded"].as_u64().unwrap_or(0);
                        total_errored += counts["errored"].as_u64().unwrap_or(0);
                        total_processing += counts["processing"].as_u64().unwrap_or(0);
                        total_canceled += counts["canceled"].as_u64().unwrap_or(0);
                        total_expired += counts["expired"].as_u64().unwrap_or(0);
                    }
                }
                _ => {
                    all_ended = false;
                    if let Some(counts) = json.get("request_counts") {
                        total_succeeded += counts["succeeded"].as_u64().unwrap_or(0);
                        total_processing += counts["processing"].as_u64().unwrap_or(0);
                    }
                }
            }
        }

        if all_ended {
            let total = total_succeeded + total_errored + total_canceled + total_expired;
            if total_succeeded == 0 && total > 0 {
                Ok(BatchPollStatus::Failed {
                    message: format!(
                        "all {} requests failed/canceled/expired (errored={}, canceled={}, expired={})",
                        total, total_errored, total_canceled, total_expired
                    ),
                })
            } else {
                if total_errored > 0 || total_canceled > 0 || total_expired > 0 {
                    tracing::warn!(
                        succeeded = total_succeeded,
                        errored = total_errored,
                        canceled = total_canceled,
                        expired = total_expired,
                        "Anthropic batch completed with some failures"
                    );
                }
                Ok(BatchPollStatus::Completed)
            }
        } else {
            let completed = total_succeeded;
            let total = total_succeeded + total_processing + total_errored;
            Ok(BatchPollStatus::InProgress {
                completed: Some(completed),
                total: if total > 0 { Some(total) } else { None },
            })
        }
    }

    async fn download_results(
        &self,
        job_ids: &[String],
        output_dir: &Path,
    ) -> Result<Vec<PathBuf>> {
        if job_ids.is_empty() {
            return Err(anyhow!("no Anthropic batch job IDs for download"));
        }

        let mut result_paths = Vec::new();

        for batch_id in job_ids {
            let resp = self
                .with_anthropic_headers(self.client.get(format!(
                    "{}/messages/batches/{}/results",
                    ANTHROPIC_API_BASE, batch_id
                )))
                .send()
                .await
                .with_context(|| {
                    format!("Anthropic batch results download failed for {}", batch_id)
                })?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_else(|_| "<no body>".to_owned());
                return Err(anyhow!(
                    "Anthropic batch results download failed (HTTP {}): {}",
                    status,
                    body
                ));
            }

            let bytes = resp
                .bytes()
                .await
                .context("failed to read Anthropic batch results body")?;

            let result_path = output_dir.join(format!("anthropic_{}.results.jsonl", batch_id));
            std::fs::write(&result_path, &bytes).with_context(|| {
                format!(
                    "failed to write Anthropic results to {}",
                    result_path.display()
                )
            })?;

            tracing::info!(
                batch_id = %batch_id,
                path = %result_path.display(),
                "downloaded Anthropic batch results"
            );
            result_paths.push(result_path);
        }

        Ok(result_paths)
    }

    fn parse_result_line(&self, line: &str) -> Result<BatchResultLine> {
        let v: serde_json::Value =
            serde_json::from_str(line).context("failed to parse Anthropic batch result line")?;

        let custom_id = v["custom_id"].as_str().map(String::from);
        let result_type = v["result"]["type"].as_str().unwrap_or("unknown");

        match result_type {
            "succeeded" => {
                let content = v["result"]["message"]["content"]
                    .as_array()
                    .ok_or_else(|| anyhow!("missing content array in Anthropic response"))?;

                // When thinking is enabled, content contains {"type": "thinking"} blocks
                // before the text block. Filter for the text block only.
                let text = content
                    .iter()
                    .find(|block| block["type"].as_str() == Some("text"))
                    .and_then(|block| block["text"].as_str())
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .ok_or_else(|| anyhow!("no text block in Anthropic response content"))?;

                Ok(BatchResultLine::Success {
                    key: custom_id
                        .ok_or_else(|| anyhow!("missing custom_id in Anthropic result"))?,
                    text: text.to_owned(),
                })
            }
            "errored" => {
                let message = v["result"]["error"]["message"]
                    .as_str()
                    .unwrap_or("unknown error")
                    .to_owned();
                Ok(BatchResultLine::Error {
                    key: custom_id,
                    message,
                })
            }
            other => Ok(BatchResultLine::Error {
                key: custom_id,
                message: format!("request {}", other),
            }),
        }
    }

    fn name(&self) -> &str {
        "anthropic"
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> AnthropicBatchProvider {
        AnthropicBatchProvider::new("test-key".to_owned())
    }

    #[test]
    fn format_request_uses_system_array_with_cache_control() {
        let p = provider();
        let line = p
            .format_request(
                "sym1|hash1",
                "System instructions.",
                "User content.",
                "claude-haiku-4-5-20251001",
                "off",
            )
            .unwrap();
        let json: serde_json::Value = serde_json::from_str(&line).unwrap();

        // custom_id is truncated to the symbol_id portion (before first '|')
        assert_eq!(json["custom_id"], "sym1");
        assert_eq!(json["params"]["model"], "claude-haiku-4-5-20251001");
        assert_eq!(json["params"]["max_tokens"], 4096);

        // System must be array-of-blocks form with cache_control
        let system = json["params"]["system"].as_array().unwrap();
        assert_eq!(system.len(), 1);
        assert_eq!(system[0]["type"], "text");
        assert_eq!(system[0]["text"], "System instructions.");
        assert_eq!(system[0]["cache_control"]["type"], "ephemeral");
        assert_eq!(
            system[0]["cache_control"]["ttl"].as_str(),
            Some(ANTHROPIC_CACHE_TTL)
        );

        // User message
        assert_eq!(json["params"]["messages"][0]["role"], "user");
        assert_eq!(json["params"]["messages"][0]["content"], "User content.");

        // No thinking when off
        assert!(json["params"].get("thinking").is_none());
    }

    #[test]
    fn format_request_includes_thinking_with_budget() {
        let p = provider();

        for (level, expected_budget, expected_max) in [
            ("low", 2048, 6144),
            ("medium", 4096, 8192),
            ("high", 8192, 12288),
        ] {
            let line = p
                .format_request("sym2|hash2", "Sys.", "Usr.", "claude-sonnet-4-6", level)
                .unwrap();
            let json: serde_json::Value = serde_json::from_str(&line).unwrap();

            assert_eq!(
                json["params"]["thinking"]["type"], "enabled",
                "thinking type for level '{}'",
                level
            );
            assert_eq!(
                json["params"]["thinking"]["budget_tokens"], expected_budget,
                "budget_tokens for level '{}'",
                level
            );
            assert_eq!(
                json["params"]["max_tokens"], expected_max,
                "max_tokens for level '{}'",
                level
            );
        }
    }

    #[test]
    fn format_request_omits_thinking_for_none() {
        let p = provider();
        for level in ["none", "off", ""] {
            let line = p
                .format_request(
                    "sym3|hash3",
                    "Sys.",
                    "Usr.",
                    "claude-haiku-4-5-20251001",
                    level,
                )
                .unwrap();
            let json: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert!(
                json["params"].get("thinking").is_none(),
                "thinking should be absent for level '{}'",
                level
            );
            assert_eq!(json["params"]["max_tokens"], 4096);
        }
    }

    #[test]
    fn parse_result_line_success_without_thinking() {
        let p = provider();
        let input = r#"{"custom_id":"sym1|hash1","result":{"type":"succeeded","message":{"content":[{"type":"text","text":"{\"intent\":\"test\"}"}]}}}"#;
        match p.parse_result_line(input).unwrap() {
            BatchResultLine::Success { key, text } => {
                assert_eq!(key, "sym1|hash1");
                assert_eq!(text, r#"{"intent":"test"}"#);
            }
            BatchResultLine::Error { .. } => panic!("expected success"),
        }
    }

    #[test]
    fn parse_result_line_success_with_thinking_blocks() {
        let p = provider();
        let input = r#"{"custom_id":"sym2|hash2","result":{"type":"succeeded","message":{"content":[{"type":"thinking","thinking":"Let me analyze..."},{"type":"text","text":"{\"intent\":\"analyzed\"}"}]}}}"#;
        match p.parse_result_line(input).unwrap() {
            BatchResultLine::Success { key, text } => {
                assert_eq!(key, "sym2|hash2");
                assert_eq!(text, r#"{"intent":"analyzed"}"#);
            }
            BatchResultLine::Error { .. } => panic!("expected success"),
        }
    }

    #[test]
    fn parse_result_line_error() {
        let p = provider();
        let input = r#"{"custom_id":"sym3|hash3","result":{"type":"errored","error":{"type":"invalid_request_error","message":"model not found"}}}"#;
        match p.parse_result_line(input).unwrap() {
            BatchResultLine::Error { key, message } => {
                assert_eq!(key, Some("sym3|hash3".to_owned()));
                assert_eq!(message, "model not found");
            }
            BatchResultLine::Success { .. } => panic!("expected error"),
        }
    }

    #[test]
    fn parse_result_line_canceled() {
        let p = provider();
        let input = r#"{"custom_id":"sym4|hash4","result":{"type":"canceled"}}"#;
        match p.parse_result_line(input).unwrap() {
            BatchResultLine::Error { key, message } => {
                assert_eq!(key, Some("sym4|hash4".to_owned()));
                assert!(message.contains("canceled"));
            }
            BatchResultLine::Success { .. } => panic!("expected error"),
        }
    }

    #[test]
    fn parse_result_line_expired() {
        let p = provider();
        let input = r#"{"custom_id":"sym5|hash5","result":{"type":"expired"}}"#;
        match p.parse_result_line(input).unwrap() {
            BatchResultLine::Error { key, message } => {
                assert_eq!(key, Some("sym5|hash5".to_owned()));
                assert!(message.contains("expired"));
            }
            BatchResultLine::Success { .. } => panic!("expected error"),
        }
    }
}
