use aether_sir::{
    SirAnnotation, canonicalize_sir_json, normalize_complexity_label, normalize_optional_text,
    sir_hash, validate_sir,
};
use aether_store::{
    SirHistoryStore, SirMetaRecord, SirStateStore, SymbolCatalogStore, SymbolRecord,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{AetherMcpServer, current_unix_timestamp};
use crate::AetherMcpError;

const FORCE_CONFIDENCE_THRESHOLD: f32 = 0.5;
const DEFAULT_INJECT_CONFIDENCE: f32 = 0.95;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSirInjectRequest {
    /// Symbol ID or qualified name selector
    pub symbol: String,
    /// New intent text (required)
    pub intent: String,
    /// Free-text behavior summary (optional; replaced if provided, preserved when omitted)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<String>,
    /// Free-text edge case notes (optional; replaced if provided, preserved when omitted)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_cases: Option<String>,
    /// Side effects (optional; replaced if provided, preserved when omitted)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side_effects: Option<Vec<String>>,
    /// Dependencies (optional; replaced if provided, preserved when omitted)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<String>>,
    /// Error modes (optional; replaced if provided, preserved when omitted)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_modes: Option<Vec<String>>,
    /// Inputs (optional; replaced if provided, preserved when omitted)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Vec<String>>,
    /// Outputs (optional; replaced if provided, preserved when omitted)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Vec<String>>,
    /// Complexity label (optional; replaced if provided, preserved when omitted)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub complexity: Option<String>,
    /// Confidence score (0.0-1.0, default 0.95)
    pub confidence: Option<f32>,
    /// Generation pass label (default "deep")
    pub generation_pass: Option<String>,
    /// Model name for provenance (default "manual")
    pub model: Option<String>,
    /// Provider name for provenance (default "manual")
    pub provider: Option<String>,
    /// Force overwrite even if existing SIR has higher confidence
    pub force: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSirInjectResponse {
    pub symbol_id: String,
    pub qualified_name: String,
    pub sir_hash: String,
    pub sir_version: i64,
    pub previous_confidence: Option<f32>,
    pub new_confidence: f32,
    pub status: String,
    pub note: Option<String>,
}

fn empty_sir_annotation(confidence: f32) -> SirAnnotation {
    SirAnnotation {
        intent: String::new(),
        behavior: None,
        inputs: Vec::new(),
        outputs: Vec::new(),
        side_effects: Vec::new(),
        dependencies: Vec::new(),
        error_modes: Vec::new(),
        confidence,
        edge_cases: None,
        complexity: None,
        method_dependencies: None,
    }
}

fn normalize_optional_text_with_default(value: Option<String>, default: &str) -> String {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

fn normalize_optional_string_list(values: Option<Vec<String>>) -> Option<Vec<String>> {
    values.map(|values| {
        values
            .into_iter()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
    })
}

fn normalize_optional_note(value: Option<String>) -> Option<Option<String>> {
    value
        .as_deref()
        .map(|value| normalize_optional_text(Some(value)))
}

fn normalize_optional_complexity(
    value: Option<String>,
) -> Result<Option<Option<String>>, AetherMcpError> {
    match value {
        None => Ok(None),
        Some(value) => {
            let value = value.trim();
            if value.is_empty() {
                return Ok(Some(None));
            }
            let normalized = normalize_complexity_label(Some(value))
                .ok_or_else(|| AetherMcpError::Message(format!("invalid complexity '{value}'")))?;
            Ok(Some(Some(normalized)))
        }
    }
}

fn resolve_symbol_selector(
    store: &aether_store::SqliteStore,
    selector: &str,
) -> Result<SymbolRecord, AetherMcpError> {
    let selector = selector.trim();
    if selector.is_empty() {
        return Err(AetherMcpError::Message(
            "symbol selector must not be empty".to_owned(),
        ));
    }

    if let Some(record) = store.get_symbol_record(selector)? {
        return Ok(record);
    }

    let exact_matches = store.find_symbol_search_results_by_qualified_name(selector)?;
    match exact_matches.as_slice() {
        [only] => {
            return store
                .get_symbol_record(only.symbol_id.as_str())?
                .ok_or_else(|| {
                    AetherMcpError::Message(format!(
                        "symbol search returned missing record: {}",
                        only.symbol_id
                    ))
                });
        }
        [] => {}
        many => {
            let candidates = many
                .iter()
                .map(|candidate| {
                    format!(
                        "{} [{}]",
                        candidate.qualified_name.trim(),
                        candidate.file_path.trim()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n  - ");
            return Err(AetherMcpError::Message(format!(
                "ambiguous symbol selector '{selector}'. Candidates:\n  - {candidates}"
            )));
        }
    }

    let matches = store.search_symbols(selector, 10)?;
    match matches.as_slice() {
        [] => Err(AetherMcpError::Message(format!(
            "symbol not found: {selector}"
        ))),
        [only] => store
            .get_symbol_record(only.symbol_id.as_str())?
            .ok_or_else(|| {
                AetherMcpError::Message(format!(
                    "symbol search returned missing record: {}",
                    only.symbol_id
                ))
            }),
        many => {
            let candidates = many
                .iter()
                .map(|candidate| {
                    format!(
                        "{} [{}]",
                        candidate.qualified_name.trim(),
                        candidate.file_path.trim()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n  - ");
            Err(AetherMcpError::Message(format!(
                "ambiguous symbol selector '{selector}'. Candidates:\n  - {candidates}"
            )))
        }
    }
}

impl AetherMcpServer {
    pub fn aether_sir_inject_logic(
        &self,
        request: AetherSirInjectRequest,
    ) -> Result<AetherSirInjectResponse, AetherMcpError> {
        self.state.require_writable()?;

        let store = self.state.store.as_ref();
        let symbol = resolve_symbol_selector(store, request.symbol.as_str())?;
        let symbol_id = symbol.id.clone();
        let qualified_name = symbol.qualified_name.clone();
        let previous_meta = store.get_sir_meta(symbol_id.as_str())?;
        let previous_blob = store.read_sir_blob(symbol_id.as_str())?;
        let previous_sir = previous_blob
            .as_deref()
            .map(serde_json::from_str::<SirAnnotation>)
            .transpose()?;
        let previous_confidence = previous_sir.as_ref().map(|sir| sir.confidence);
        let new_confidence = request.confidence.unwrap_or(DEFAULT_INJECT_CONFIDENCE);

        if previous_confidence.is_some_and(|confidence| confidence > FORCE_CONFIDENCE_THRESHOLD)
            && !request.force.unwrap_or(false)
        {
            let note = previous_confidence.map(|confidence| {
                format!(
                    "existing SIR confidence {confidence:.2} exceeds {FORCE_CONFIDENCE_THRESHOLD:.2} threshold; rerun with force=true to override"
                )
            });
            return Ok(AetherSirInjectResponse {
                symbol_id,
                qualified_name,
                sir_hash: previous_meta
                    .as_ref()
                    .map(|meta| meta.sir_hash.clone())
                    .unwrap_or_default(),
                sir_version: previous_meta
                    .as_ref()
                    .map(|meta| meta.sir_version)
                    .unwrap_or(0),
                previous_confidence,
                new_confidence,
                status: "blocked".to_owned(),
                note,
            });
        }

        let intent = request.intent.trim();
        if intent.is_empty() {
            return Err(AetherMcpError::Message(
                "intent must not be empty".to_owned(),
            ));
        }

        let mut updated = previous_sir.unwrap_or_else(|| empty_sir_annotation(new_confidence));
        updated.intent = intent.to_owned();
        if let Some(behavior) = normalize_optional_note(request.behavior) {
            updated.behavior = behavior;
        }
        if let Some(edge_cases) = normalize_optional_note(request.edge_cases) {
            updated.edge_cases = edge_cases;
        }
        if let Some(side_effects) = normalize_optional_string_list(request.side_effects) {
            updated.side_effects = side_effects;
        }
        if let Some(dependencies) = normalize_optional_string_list(request.dependencies) {
            updated.dependencies = dependencies;
        }
        if let Some(error_modes) = normalize_optional_string_list(request.error_modes) {
            updated.error_modes = error_modes;
        }
        if let Some(inputs) = normalize_optional_string_list(request.inputs) {
            updated.inputs = inputs;
        }
        if let Some(outputs) = normalize_optional_string_list(request.outputs) {
            updated.outputs = outputs;
        }
        if let Some(complexity) = normalize_optional_complexity(request.complexity)? {
            updated.complexity = complexity;
        }
        updated.confidence = new_confidence;
        validate_sir(&updated)?;

        let canonical_json = canonicalize_sir_json(&updated);
        let hash = sir_hash(&updated);
        let provider = normalize_optional_text_with_default(request.provider, "manual");
        let model = normalize_optional_text_with_default(request.model, "manual");
        let generation_pass = normalize_optional_text_with_default(request.generation_pass, "deep");
        let now = current_unix_timestamp();
        let version_write = store.record_sir_version_if_changed(
            symbol_id.as_str(),
            hash.as_str(),
            provider.as_str(),
            model.as_str(),
            canonical_json.as_str(),
            now,
            None,
        )?;

        store.write_sir_blob(symbol_id.as_str(), canonical_json.as_str())?;
        store.upsert_sir_meta(SirMetaRecord {
            id: symbol_id.clone(),
            sir_hash: hash.clone(),
            sir_version: version_write.version,
            provider,
            model,
            generation_pass,
            reasoning_trace: None,
            prompt_hash: None,
            staleness_score: None,
            updated_at: version_write.updated_at,
            sir_status: "fresh".to_owned(),
            last_error: None,
            last_attempt_at: version_write.updated_at,
        })?;

        Ok(AetherSirInjectResponse {
            symbol_id,
            qualified_name,
            sir_hash: hash,
            sir_version: version_write.version,
            previous_confidence,
            new_confidence,
            status: "injected".to_owned(),
            note: Some(
                "Embeddings not refreshed - run 'aetherd regenerate --embed-only' if semantic search accuracy matters"
                    .to_owned(),
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use aether_sir::SirAnnotation;
    use aether_store::{SirHistoryStore, SirStateStore, SymbolCatalogStore, SymbolRecord};
    use tempfile::tempdir;

    use super::{AetherSirInjectRequest, DEFAULT_INJECT_CONFIDENCE, resolve_symbol_selector};
    use crate::AetherMcpServer;

    fn write_test_config(workspace: &Path) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[inference]
provider = "qwen3_local"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
        )
        .expect("write config");
    }

    fn seed_symbol(workspace: &Path, symbol_id: &str, qualified_name: &str) {
        let store = aether_store::SqliteStore::open(workspace).expect("open store");
        store
            .upsert_symbol(SymbolRecord {
                id: symbol_id.to_owned(),
                file_path: "src/lib.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                qualified_name: qualified_name.to_owned(),
                signature_fingerprint: format!("sig-{symbol_id}"),
                last_seen_at: 1_700_000_000,
            })
            .expect("upsert symbol");
    }

    fn seed_existing_sir(workspace: &Path, symbol_id: &str, confidence: f32) {
        let store = aether_store::SqliteStore::open(workspace).expect("open store");
        let sir_json = format!(
            r#"{{
                "intent":"existing intent",
                "inputs":[],
                "outputs":[],
                "side_effects":["writes cache"],
                "dependencies":[],
                "error_modes":["timeout"],
                "confidence":{confidence}
            }}"#
        );
        let history = store
            .record_sir_version_if_changed(
                symbol_id,
                "seed-hash",
                "seed",
                "seed",
                sir_json.as_str(),
                1_700_000_100,
                None,
            )
            .expect("record seed history");
        store
            .write_sir_blob(symbol_id, sir_json.as_str())
            .expect("write seed sir");
        store
            .upsert_sir_meta(aether_store::SirMetaRecord {
                id: symbol_id.to_owned(),
                sir_hash: "seed-hash".to_owned(),
                sir_version: history.version,
                provider: "seed".to_owned(),
                model: "seed".to_owned(),
                generation_pass: "scan".to_owned(),
                reasoning_trace: None,
                prompt_hash: None,
                staleness_score: None,
                updated_at: history.updated_at,
                sir_status: "fresh".to_owned(),
                last_error: None,
                last_attempt_at: history.updated_at,
            })
            .expect("upsert seed meta");
    }

    #[test]
    fn resolve_symbol_selector_prefers_exact_matches() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        seed_symbol(temp.path(), "sym-alpha", "crate::alpha");
        let store = aether_store::SqliteStore::open(temp.path()).expect("open store");

        let resolved = resolve_symbol_selector(&store, "crate::alpha").expect("resolve symbol");
        assert_eq!(resolved.id, "sym-alpha");
    }

    #[test]
    fn sir_inject_creates_new_sir_when_missing() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        seed_symbol(temp.path(), "sym-new", "crate::new_symbol");
        let server = AetherMcpServer::new(temp.path(), false).expect("server");

        let response = server
            .aether_sir_inject_logic(AetherSirInjectRequest {
                symbol: "sym-new".to_owned(),
                intent: "Persist a new SIR annotation".to_owned(),
                behavior: Some("Reads inputs and stores audit metadata".to_owned()),
                edge_cases: Some("Blank inputs are rejected before persistence".to_owned()),
                side_effects: Some(vec!["writes audit history".to_owned()]),
                dependencies: Some(vec!["sqlx::PgPool".to_owned()]),
                error_modes: Some(vec!["io".to_owned()]),
                confidence: Some(0.6),
                inputs: Some(vec!["payload: CreateRequest".to_owned()]),
                outputs: Some(vec!["Result<(), io::Error>".to_owned()]),
                complexity: Some("medium".to_owned()),
                generation_pass: None,
                model: None,
                provider: None,
                force: None,
            })
            .expect("inject sir");

        assert_eq!(response.status, "injected");
        assert_eq!(response.sir_version, 1);
        assert_eq!(response.previous_confidence, None);
        assert_eq!(response.new_confidence, 0.6);
        assert!(!response.sir_hash.is_empty());

        let store = aether_store::SqliteStore::open(temp.path()).expect("open store");
        let history = store.list_sir_history("sym-new").expect("list sir history");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].version, 1);
        let meta = store
            .get_sir_meta("sym-new")
            .expect("get sir meta")
            .expect("sir meta exists");
        assert_eq!(meta.sir_hash, response.sir_hash);
        assert_eq!(meta.sir_version, response.sir_version);
        assert_eq!(meta.model, "manual");
    }

    #[test]
    fn sir_inject_blocks_when_existing_confidence_is_high_without_force() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        seed_symbol(temp.path(), "sym-block", "crate::blocked");
        seed_existing_sir(temp.path(), "sym-block", 0.95);
        let server = AetherMcpServer::new(temp.path(), false).expect("server");

        let response = server
            .aether_sir_inject_logic(AetherSirInjectRequest {
                symbol: "sym-block".to_owned(),
                intent: "Attempted overwrite".to_owned(),
                behavior: None,
                edge_cases: None,
                side_effects: None,
                dependencies: None,
                error_modes: None,
                confidence: Some(0.95),
                inputs: None,
                outputs: None,
                complexity: None,
                generation_pass: None,
                model: None,
                provider: None,
                force: Some(false),
            })
            .expect("inject sir");

        assert_eq!(response.status, "blocked");
        assert_eq!(response.previous_confidence, Some(0.95));

        let store = aether_store::SqliteStore::open(temp.path()).expect("open store");
        let history = store
            .list_sir_history("sym-block")
            .expect("list sir history");
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn sir_inject_force_overrides_existing_high_confidence_sir() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        seed_symbol(temp.path(), "sym-force", "crate::forced");
        seed_existing_sir(temp.path(), "sym-force", 0.9);
        let server = AetherMcpServer::new(temp.path(), false).expect("server");

        let response = server
            .aether_sir_inject_logic(AetherSirInjectRequest {
                symbol: "crate::forced".to_owned(),
                intent: "Forced overwrite".to_owned(),
                behavior: Some("Overwrites existing SIR metadata".to_owned()),
                edge_cases: Some("Existing history row is preserved".to_owned()),
                side_effects: Some(vec!["updates sir row".to_owned()]),
                dependencies: Some(vec!["sqlite".to_owned()]),
                error_modes: Some(vec!["network".to_owned()]),
                confidence: Some(0.4),
                inputs: Some(vec!["symbol selector".to_owned()]),
                outputs: Some(vec!["AetherSirInjectResponse".to_owned()]),
                complexity: Some("High".to_owned()),
                generation_pass: Some("deep".to_owned()),
                model: Some("claude-opus-4-6".to_owned()),
                provider: Some("manual".to_owned()),
                force: Some(true),
            })
            .expect("inject sir");

        assert_eq!(response.status, "injected");
        assert_eq!(response.previous_confidence, Some(0.9));
        assert_eq!(response.new_confidence, 0.4);
        assert!(response.sir_version >= 2);

        let store = aether_store::SqliteStore::open(temp.path()).expect("open store");
        let blob = store
            .read_sir_blob("sym-force")
            .expect("read sir blob")
            .expect("sir blob exists");
        assert!(blob.contains("Forced overwrite"));
        assert!(blob.contains("\"behavior\":\"Overwrites existing SIR metadata\""));
        assert!(blob.contains("\"complexity\":\"High\""));
        let meta = store
            .get_sir_meta("sym-force")
            .expect("get sir meta")
            .expect("sir meta exists");
        assert_eq!(meta.sir_hash, response.sir_hash);
        assert_eq!(meta.sir_version, response.sir_version);
        assert_eq!(meta.model, "claude-opus-4-6");
    }

    #[test]
    fn sir_inject_defaults_confidence_to_agent_default_when_omitted_for_existing_sir() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        seed_symbol(temp.path(), "sym-default", "crate::defaulted");
        seed_existing_sir(temp.path(), "sym-default", 0.4);
        let server = AetherMcpServer::new(temp.path(), false).expect("server");

        let response = server
            .aether_sir_inject_logic(AetherSirInjectRequest {
                symbol: "sym-default".to_owned(),
                intent: "Default confidence overwrite".to_owned(),
                behavior: None,
                edge_cases: None,
                side_effects: None,
                dependencies: None,
                error_modes: None,
                confidence: None,
                inputs: None,
                outputs: None,
                complexity: None,
                generation_pass: None,
                model: None,
                provider: None,
                force: Some(false),
            })
            .expect("inject sir");

        assert_eq!(response.status, "injected");
        assert_eq!(response.new_confidence, DEFAULT_INJECT_CONFIDENCE);

        let store = aether_store::SqliteStore::open(temp.path()).expect("open store");
        let blob = store
            .read_sir_blob("sym-default")
            .expect("read sir blob")
            .expect("sir blob exists");
        let sir: SirAnnotation = serde_json::from_str(&blob).expect("parse sir");
        assert_eq!(sir.confidence, DEFAULT_INJECT_CONFIDENCE);
    }
}
