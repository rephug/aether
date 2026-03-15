use aether_core::{Symbol, content_hash};
use aether_parse::TestIntent;
use aether_sir::SirAnnotation;
use aether_store::{SymbolRecord, TestIntentRecord};
use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

use super::SIR_GENERATION_PASS_SCAN;

#[derive(Debug, Clone)]
pub(crate) struct UpsertSirIntentPayload {
    pub(crate) symbol: Symbol,
    pub(crate) sir: SirAnnotation,
    pub(crate) provider_name: String,
    pub(crate) model_name: String,
    pub(crate) generation_pass: String,
    pub(crate) commit_hash: Option<String>,
}

impl UpsertSirIntentPayload {
    pub(crate) fn to_json_string(&self) -> Result<String> {
        serde_json::to_string(&json!({
            "symbol": self.symbol,
            "sir": self.sir,
            "provider_name": self.provider_name,
            "model_name": self.model_name,
            "generation_pass": self.generation_pass,
            "commit_hash": self.commit_hash,
        }))
        .context("failed to serialize upsert intent payload")
    }

    pub(crate) fn from_json_str(raw: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(raw).context("failed to parse payload JSON")?;
        let object = value
            .as_object()
            .ok_or_else(|| anyhow!("payload must be a JSON object"))?;
        let symbol_value = object
            .get("symbol")
            .cloned()
            .ok_or_else(|| anyhow!("payload missing field 'symbol'"))?;
        let sir_value = object
            .get("sir")
            .cloned()
            .ok_or_else(|| anyhow!("payload missing field 'sir'"))?;
        let provider_name = payload_required_string(object, "provider_name")?;
        let model_name = payload_required_string(object, "model_name")?;
        let generation_pass = payload_required_string(object, "generation_pass")
            .unwrap_or_else(|_| SIR_GENERATION_PASS_SCAN.to_owned());
        let commit_hash = match object.get("commit_hash") {
            Some(Value::String(value)) => Some(value.clone()),
            Some(Value::Null) | None => None,
            Some(_) => {
                return Err(anyhow!(
                    "payload field 'commit_hash' must be a string or null"
                ));
            }
        };

        Ok(Self {
            symbol: serde_json::from_value(symbol_value).context("invalid payload symbol")?,
            sir: serde_json::from_value(sir_value).context("invalid payload sir")?,
            provider_name,
            model_name,
            generation_pass,
            commit_hash,
        })
    }
}

fn payload_required_string(
    payload: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<String> {
    match payload.get(field) {
        Some(Value::String(value)) => Ok(value.clone()),
        Some(_) => Err(anyhow!("payload field '{field}' must be a string")),
        None => Err(anyhow!("payload missing field '{field}'")),
    }
}

pub(super) fn to_symbol_record(symbol: &Symbol, now_ts: i64) -> SymbolRecord {
    SymbolRecord {
        id: symbol.id.clone(),
        file_path: symbol.file_path.clone(),
        language: symbol.language.as_str().to_owned(),
        kind: symbol.kind.as_str().to_owned(),
        qualified_name: symbol.qualified_name.clone(),
        signature_fingerprint: symbol.signature_fingerprint.clone(),
        last_seen_at: now_ts,
    }
}

pub(super) fn flatten_error_line(message: &str) -> String {
    message.lines().next().unwrap_or(message).to_owned()
}

pub(super) fn to_test_intent_record(intent: TestIntent, now_ms: i64) -> TestIntentRecord {
    let material = format!(
        "{}\n{}\n{}",
        intent.file_path.trim(),
        intent.test_name.trim(),
        intent.intent_text.trim(),
    );
    TestIntentRecord {
        intent_id: content_hash(material.as_str()),
        file_path: intent.file_path,
        test_name: intent.test_name,
        intent_text: intent.intent_text,
        group_label: intent.group_label,
        language: intent.language.as_str().to_owned(),
        symbol_id: intent.symbol_id,
        created_at: now_ms.max(0),
        updated_at: now_ms.max(0),
    }
}
