use aether_store::{SirHistorySelector, Store};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AetherMcpServer, effective_limit};
use crate::AetherMcpError;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSymbolTimelineRequest {
    pub symbol_id: String,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSymbolTimelineEntry {
    pub version: i64,
    pub sir_hash: String,
    pub provider: String,
    pub model: String,
    pub created_at: i64,
    pub commit_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSymbolTimelineResponse {
    pub symbol_id: String,
    pub limit: u32,
    pub found: bool,
    pub result_count: u32,
    pub timeline: Vec<AetherSymbolTimelineEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AetherWhySelectorMode {
    Auto,
    Version,
    Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AetherWhyChangedReason {
    NoHistory,
    SingleVersionOnly,
    SelectorNotFound,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherWhyChangedRequest {
    pub symbol_id: String,
    pub from_version: Option<i64>,
    pub to_version: Option<i64>,
    pub from_created_at: Option<i64>,
    pub to_created_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AetherWhySnapshot {
    pub version: i64,
    pub created_at: i64,
    pub sir_hash: String,
    pub provider: String,
    pub model: String,
    pub commit_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AetherWhyChangedResponse {
    pub symbol_id: String,
    pub found: bool,
    pub reason: Option<AetherWhyChangedReason>,
    pub selector_mode: AetherWhySelectorMode,
    pub from: Option<AetherWhySnapshot>,
    pub to: Option<AetherWhySnapshot>,
    pub prior_summary: Option<String>,
    pub current_summary: Option<String>,
    pub fields_added: Vec<String>,
    pub fields_removed: Vec<String>,
    pub fields_modified: Vec<String>,
}

impl AetherWhySnapshot {
    fn from_history_record(record: &aether_store::SirHistoryRecord) -> Self {
        Self {
            version: record.version,
            created_at: record.created_at,
            sir_hash: record.sir_hash.clone(),
            provider: record.provider.clone(),
            model: record.model.clone(),
            commit_hash: record.commit_hash.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WhySelector {
    Auto,
    Version {
        from_version: i64,
        to_version: i64,
    },
    Timestamp {
        from_created_at: i64,
        to_created_at: i64,
    },
}

impl WhySelector {
    fn mode(self) -> AetherWhySelectorMode {
        match self {
            Self::Auto => AetherWhySelectorMode::Auto,
            Self::Version { .. } => AetherWhySelectorMode::Version,
            Self::Timestamp { .. } => AetherWhySelectorMode::Timestamp,
        }
    }
}

impl AetherMcpServer {
    pub fn aether_symbol_timeline_logic(
        &self,
        request: AetherSymbolTimelineRequest,
    ) -> Result<AetherSymbolTimelineResponse, AetherMcpError> {
        let symbol_id = request.symbol_id.trim();
        let limit = effective_limit(request.limit);
        if symbol_id.is_empty() {
            return Ok(AetherSymbolTimelineResponse {
                symbol_id: String::new(),
                limit,
                found: false,
                result_count: 0,
                timeline: Vec::new(),
            });
        }

        if !self.sqlite_path().exists() {
            return Ok(AetherSymbolTimelineResponse {
                symbol_id: symbol_id.to_owned(),
                limit,
                found: false,
                result_count: 0,
                timeline: Vec::new(),
            });
        }

        let store = self.state.store.as_ref();
        let mut history = store.list_sir_history(symbol_id)?;
        if history.len() > limit as usize {
            let split_idx = history.len().saturating_sub(limit as usize);
            history = history.split_off(split_idx);
        }

        let timeline = history
            .into_iter()
            .map(|record| AetherSymbolTimelineEntry {
                version: record.version,
                sir_hash: record.sir_hash,
                provider: record.provider,
                model: record.model,
                created_at: record.created_at,
                commit_hash: record.commit_hash,
            })
            .collect::<Vec<_>>();
        let result_count = timeline.len() as u32;

        Ok(AetherSymbolTimelineResponse {
            symbol_id: symbol_id.to_owned(),
            limit,
            found: result_count > 0,
            result_count,
            timeline,
        })
    }

    pub fn aether_why_changed_logic(
        &self,
        request: AetherWhyChangedRequest,
    ) -> Result<AetherWhyChangedResponse, AetherMcpError> {
        let selector = parse_why_selector(&request)?;
        let selector_mode = selector.mode();
        let symbol_id = request.symbol_id.trim();

        if symbol_id.is_empty() {
            return Ok(empty_why_changed_response(
                String::new(),
                selector_mode,
                AetherWhyChangedReason::NoHistory,
            ));
        }

        if !self.sqlite_path().exists() {
            return Ok(empty_why_changed_response(
                symbol_id.to_owned(),
                selector_mode,
                AetherWhyChangedReason::NoHistory,
            ));
        }

        let store = self.state.store.as_ref();
        let history = store.list_sir_history(symbol_id)?;
        if history.is_empty() {
            return Ok(empty_why_changed_response(
                symbol_id.to_owned(),
                selector_mode,
                AetherWhyChangedReason::NoHistory,
            ));
        }

        let pair = match selector {
            WhySelector::Auto => store.latest_sir_history_pair(symbol_id)?,
            WhySelector::Version {
                from_version,
                to_version,
            } => store.resolve_sir_history_pair(
                symbol_id,
                SirHistorySelector::Version(from_version),
                SirHistorySelector::Version(to_version),
            )?,
            WhySelector::Timestamp {
                from_created_at,
                to_created_at,
            } => store.resolve_sir_history_pair(
                symbol_id,
                SirHistorySelector::CreatedAt(from_created_at),
                SirHistorySelector::CreatedAt(to_created_at),
            )?,
        };

        let Some(pair) = pair else {
            return Ok(empty_why_changed_response(
                symbol_id.to_owned(),
                selector_mode,
                AetherWhyChangedReason::SelectorNotFound,
            ));
        };

        let from_fields = parse_sir_history_json_fields(&pair.from.sir_json)?;
        let to_fields = parse_sir_history_json_fields(&pair.to.sir_json)?;
        let (fields_added, fields_removed, fields_modified) =
            diff_top_level_field_names(&from_fields, &to_fields);

        let reason = (selector_mode == AetherWhySelectorMode::Auto && history.len() == 1)
            .then_some(AetherWhyChangedReason::SingleVersionOnly);

        Ok(AetherWhyChangedResponse {
            symbol_id: symbol_id.to_owned(),
            found: true,
            reason,
            selector_mode,
            from: Some(AetherWhySnapshot::from_history_record(&pair.from)),
            to: Some(AetherWhySnapshot::from_history_record(&pair.to)),
            prior_summary: extract_intent_field(&from_fields),
            current_summary: extract_intent_field(&to_fields),
            fields_added,
            fields_removed,
            fields_modified,
        })
    }
}

fn parse_why_selector(request: &AetherWhyChangedRequest) -> Result<WhySelector, AetherMcpError> {
    let has_any_version = request.from_version.is_some() || request.to_version.is_some();
    let has_any_timestamp = request.from_created_at.is_some() || request.to_created_at.is_some();

    if has_any_version && has_any_timestamp {
        return Err(AetherMcpError::Message(
            "provide either version selectors or timestamp selectors, not both".to_owned(),
        ));
    }

    if has_any_version {
        let from_version = request.from_version.ok_or_else(|| {
            AetherMcpError::Message(
                "from_version is required when using version selectors".to_owned(),
            )
        })?;
        let to_version = request.to_version.ok_or_else(|| {
            AetherMcpError::Message(
                "to_version is required when using version selectors".to_owned(),
            )
        })?;
        if from_version < 1 || to_version < 1 {
            return Err(AetherMcpError::Message(
                "version selectors must be >= 1".to_owned(),
            ));
        }

        return Ok(WhySelector::Version {
            from_version,
            to_version,
        });
    }

    if has_any_timestamp {
        let from_created_at = request.from_created_at.ok_or_else(|| {
            AetherMcpError::Message(
                "from_created_at is required when using timestamp selectors".to_owned(),
            )
        })?;
        let to_created_at = request.to_created_at.ok_or_else(|| {
            AetherMcpError::Message(
                "to_created_at is required when using timestamp selectors".to_owned(),
            )
        })?;
        if from_created_at < 0 || to_created_at < 0 {
            return Err(AetherMcpError::Message(
                "timestamp selectors must be >= 0".to_owned(),
            ));
        }

        return Ok(WhySelector::Timestamp {
            from_created_at,
            to_created_at,
        });
    }

    Ok(WhySelector::Auto)
}

fn empty_why_changed_response(
    symbol_id: String,
    selector_mode: AetherWhySelectorMode,
    reason: AetherWhyChangedReason,
) -> AetherWhyChangedResponse {
    AetherWhyChangedResponse {
        symbol_id,
        found: false,
        reason: Some(reason),
        selector_mode,
        from: None,
        to: None,
        prior_summary: None,
        current_summary: None,
        fields_added: Vec::new(),
        fields_removed: Vec::new(),
        fields_modified: Vec::new(),
    }
}

fn parse_sir_history_json_fields(
    value: &str,
) -> Result<serde_json::Map<String, Value>, AetherMcpError> {
    let parsed: Value = serde_json::from_str(value)?;
    let Value::Object(fields) = parsed else {
        return Err(AetherMcpError::Message(
            "sir_history row contains non-object sir_json".to_owned(),
        ));
    };
    Ok(fields)
}

fn extract_intent_field(fields: &serde_json::Map<String, Value>) -> Option<String> {
    fields
        .get("intent")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn diff_top_level_field_names(
    from: &serde_json::Map<String, Value>,
    to: &serde_json::Map<String, Value>,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut fields_added = to
        .keys()
        .filter(|key| !from.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();
    let mut fields_removed = from
        .keys()
        .filter(|key| !to.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();
    let mut fields_modified = from
        .iter()
        .filter_map(|(key, from_value)| {
            let to_value = to.get(key)?;
            (from_value != to_value).then(|| key.clone())
        })
        .collect::<Vec<_>>();

    fields_added.sort_unstable();
    fields_removed.sort_unstable();
    fields_modified.sort_unstable();

    (fields_added, fields_removed, fields_modified)
}
