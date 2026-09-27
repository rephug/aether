use aether_sir::{
    SirAnnotation, canonicalize_sir_json, normalize_complexity_label, normalize_optional_text,
    sir_hash, validate_sir,
};
use aether_store::{SirMetaRecord, SirStateStore, SymbolCatalogStore, SymbolRecord};

use aether_parse::language_for_path;
use aetherd::sir_pipeline::{
    EmbeddingRefresh, acquire_embed_write_lock, acquire_inject_write_lock,
    refresh_local_file_rollup,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;

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

/// `sir_status` recorded when a leaf was written but its file rollup could not be
/// rebuilt: the confidence guard lets such a symbol be re-injected without `force`, and
/// the scan queries keep selecting it, so the retry is never blocked.
pub const SIR_STATUS_ROLLUP_FAILED: &str = "rollup_failed";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSirInjectResponse {
    pub symbol_id: String,
    pub qualified_name: String,
    pub sir_hash: String,
    pub sir_version: i64,
    /// Rounded to 4 decimal places for display; stored precision is untouched.
    #[serde(serialize_with = "serialize_optional_rounded_confidence")]
    pub previous_confidence: Option<f32>,
    /// Rounded to 4 decimal places for display; stored precision is untouched.
    #[serde(serialize_with = "serialize_rounded_confidence")]
    pub new_confidence: f32,
    pub status: String,
    pub note: Option<String>,
    /// What happened to the symbol's embedding: `refreshed`, `unchanged`,
    /// `skipped: <reason>` or `failed: <error>`.
    pub embedding_status: String,
    /// Whether the file rollup (and therefore module reads) was rebuilt from the leaves.
    pub file_rollup_status: String,
}

/// Round an f32 confidence to 4 decimal places as f64, so the JSON response reads
/// `0.97` rather than the widened-f32 artifact `0.9700000286102295`.
pub(crate) fn round_confidence_for_display(value: f32) -> f64 {
    (f64::from(value) * 10_000.0).round() / 10_000.0
}

fn serialize_rounded_confidence<S>(value: &f32, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_f64(round_confidence_for_display(*value))
}

fn serialize_optional_rounded_confidence<S>(
    value: &Option<f32>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match value {
        Some(value) => serializer.serialize_some(&round_confidence_for_display(*value)),
        None => serializer.serialize_none(),
    }
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
        // Read the prior state, apply the confidence guard, merge, write the leaf and
        // rebuild the file rollup under one lock: a concurrent injection into the same
        // symbol (or the same file) must observe this call's write, not the snapshot it
        // started from. Otherwise two injectors of a low-confidence symbol both pass the
        // guard, and the slower one overwrites the faster one's result with fields merged
        // from the stale SIR. The lock (in-process half plus the cross-process file lock,
        // shared with the daemon's rollup writer) lives in the pipeline crate.
        let _inject_guard = acquire_inject_write_lock(&self.state.workspace)
            .map_err(|err| AetherMcpError::Message(format!("{err:#}")))?;
        let previous_meta = store.get_sir_meta(symbol_id.as_str())?;
        let previous_blob = store.read_sir_blob(symbol_id.as_str())?;
        let previous_sir = previous_blob
            .as_deref()
            .map(serde_json::from_str::<SirAnnotation>)
            .transpose()?;
        let previous_confidence = previous_sir.as_ref().map(|sir| sir.confidence);
        let new_confidence = request.confidence.unwrap_or(DEFAULT_INJECT_CONFIDENCE);

        let previous_rollup_failed = previous_meta
            .as_ref()
            .is_some_and(|meta| meta.sir_status == SIR_STATUS_ROLLUP_FAILED);
        if previous_confidence.is_some_and(|confidence| confidence > FORCE_CONFIDENCE_THRESHOLD)
            && !request.force.unwrap_or(false)
            && !previous_rollup_failed
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
                embedding_status: "skipped: inject blocked".to_owned(),
                file_rollup_status: "skipped: inject blocked".to_owned(),
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
        let rollup_identity = (provider.clone(), model.clone(), generation_pass.clone());
        let now = current_unix_timestamp();
        // History version, leaf JSON and metadata land in one SQLite transaction (the
        // same path the daemon's SIR pipeline uses): a failure between separate writes
        // would leave the new JSON under the old hash and a `fresh` status, a leaf that
        // the scan queries no longer select (its confidence is now high) and that the
        // guard blocks from an ordinary retry, so it could never be repaired.
        let mut meta_record = SirMetaRecord {
            id: symbol_id.clone(),
            sir_hash: hash.clone(),
            sir_version: 1,
            provider,
            model,
            generation_pass,
            reasoning_trace: None,
            prompt_hash: None,
            staleness_score: None,
            updated_at: now,
            sir_status: "fresh".to_owned(),
            last_error: None,
            last_attempt_at: now,
        };
        let version_write = store.persist_sir_state_atomically(
            meta_record.clone(),
            canonical_json.as_str(),
            None,
            None,
        )?;
        meta_record.sir_version = version_write.version;
        meta_record.updated_at = version_write.updated_at;
        meta_record.last_attempt_at = version_write.updated_at;

        // Aggregate reads (file and module level) are served from the file rollup, so
        // rebuild it from the leaves now rather than leaving the indexing-time rollup
        // (a [MOCK] concatenation after a mock index) in place.
        let file_rollup_status = self
            .refresh_file_rollup_after_inject(
                symbol.file_path.as_str(),
                &rollup_identity.0,
                &rollup_identity.1,
                &rollup_identity.2,
            )
            .map_err(|err| {
                let message = format!(
                    "SIR for {qualified_name} was written but the file rollup for {} could not be rebuilt: {err:#}; rerun the injection (the confidence guard is lifted for it) so aggregate reads stay consistent",
                    symbol.file_path
                );
                // Leave a retry marker: the leaf keeps its scan-level confidence, but the
                // guard and the scan queries treat this status as "still a target". If even
                // the marker cannot be written, say so: the symbol must then be re-injected
                // with force=true, because nothing in the store records the failure.
                let message = match store.upsert_sir_meta(SirMetaRecord {
                    sir_status: SIR_STATUS_ROLLUP_FAILED.to_owned(),
                    last_error: Some(message.clone()),
                    ..meta_record.clone()
                }) {
                    Ok(()) => message,
                    Err(marker_err) => format!(
                        "{message}; the rollup_failed retry marker could not be written either ({marker_err}), so rerun this injection with force=true once the store accepts writes"
                    ),
                };
                AetherMcpError::Message(message)
            })?;
        // The embedding refresh may call a local or remote model: release the inject lock
        // first so concurrent injections into other files are not serialized behind it.
        drop(_inject_guard);
        let embedding_status =
            self.refresh_embedding_after_inject(symbol_id.as_str(), hash.as_str(), &canonical_json);
        let note = match embedding_status.as_str() {
            "refreshed" | "unchanged" => None,
            _ => Some(format!(
                "embedding {embedding_status}; run 'aetherd --index-once --embeddings-only' \
                 if semantic search accuracy matters"
            )),
        };

        Ok(AetherSirInjectResponse {
            symbol_id,
            qualified_name,
            sir_hash: hash,
            sir_version: version_write.version,
            previous_confidence,
            new_confidence,
            status: "injected".to_owned(),
            note,
            embedding_status,
            file_rollup_status,
        })
    }

    /// Rebuild the file rollup for `file_path`. A failure is returned, not swallowed:
    /// the leaf is already persisted, but reporting the injection as a success while
    /// the aggregate is stale would let a scan claim completion with inconsistent data.
    fn refresh_file_rollup_after_inject(
        &self,
        file_path: &str,
        provider: &str,
        model: &str,
        generation_pass: &str,
    ) -> anyhow::Result<String> {
        let Some(language) = language_for_path(Path::new(file_path)) else {
            return Ok("skipped: unknown language".to_owned());
        };
        let written = refresh_local_file_rollup(
            self.state.store.as_ref(),
            file_path,
            language,
            provider,
            model,
            generation_pass,
        )?;
        Ok(if written {
            "refreshed".to_owned()
        } else {
            "removed: no leaf SIRs".to_owned()
        })
    }

    /// Refresh the symbol's embedding right after an inject so semantic search sees the
    /// enriched SIR immediately (the same path `aetherd sir inject` and the
    /// embeddings-only index pass use). Never fails the inject: problems are reported
    /// in the returned status.
    fn refresh_embedding_after_inject(
        &self,
        symbol_id: &str,
        sir_hash: &str,
        canonical_json: &str,
    ) -> String {
        if !self.state.config.embeddings.enabled {
            return "skipped: embeddings disabled".to_owned();
        }
        // Per-symbol ordering: a slower embedding for an older SIR must never overwrite
        // the embedding of a newer one, so embed under the symbol's embedding lock, which
        // every vector writer takes (other injectors, the daemon's index and regenerate
        // passes, symbol removal), and only while the store still holds the SIR this call
        // wrote. Injections do not wait on this lock, so a newer SIR can land at any point
        // during the provider call: the pipeline re-asks `still_current` right before and
        // right after the vector is stored, and removes a vector the newer SIR would
        // otherwise inherit. That injector's own refresh, queued behind the lock, then
        // embeds the newer SIR.
        let _embed_guard = match acquire_embed_write_lock(&self.state.workspace, symbol_id) {
            Ok(guard) => guard,
            Err(err) => return format!("failed: {err:#}"),
        };
        // One embedding pipeline serves every inject and deep-scan call this server
        // handles, so the provider's model is loaded once, not per call.
        let pipeline = match self.state.embedding_pipeline() {
            Ok(Some(pipeline)) => pipeline,
            Ok(None) => return "skipped: embedding provider not configured".to_owned(),
            Err(err) => return format!("failed: {err}"),
        };
        // Missing metadata counts as superseded too: the indexer removes a symbol's SIR
        // without the inject lock, and a vector written for it afterwards would be an
        // orphan nothing ever cleans up.
        let store = self.state.store.as_ref();
        let mut still_current = || -> anyhow::Result<bool> {
            Ok(store
                .get_sir_meta(symbol_id)?
                .is_some_and(|meta| meta.sir_hash == sir_hash))
        };
        match pipeline.refresh_embedding_if_current(
            symbol_id,
            sir_hash,
            canonical_json,
            None,
            &mut still_current,
        ) {
            Ok(EmbeddingRefresh::Refreshed { .. }) => "refreshed".to_owned(),
            Ok(EmbeddingRefresh::Unchanged) => "unchanged".to_owned(),
            Ok(EmbeddingRefresh::Superseded) => "superseded: a newer SIR was injected".to_owned(),
            Err(err) => format!("failed: {err:#}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use aether_sir::SirAnnotation;
    use aether_store::{SirHistoryStore, SirStateStore, SymbolCatalogStore, SymbolRecord};
    use tempfile::tempdir;

    use super::{
        AetherSirInjectRequest, AetherSirInjectResponse, DEFAULT_INJECT_CONFIDENCE,
        resolve_symbol_selector, round_confidence_for_display,
    };
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
        assert_eq!(response.file_rollup_status, "refreshed");

        let store = aether_store::SqliteStore::open(temp.path()).expect("open store");
        let rollup_id = aether_sir::synthetic_file_sir_id("rust", "src/lib.rs");
        let rollup = store
            .read_sir_blob(&rollup_id)
            .expect("read file rollup")
            .expect("file rollup rebuilt from the injected leaf");
        assert!(
            rollup.contains("Persist a new SIR annotation"),
            "file rollup must reflect the injected leaf: {rollup}"
        );
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
    fn sir_inject_concurrent_injections_of_one_symbol_serialize_through_the_guard() {
        // Two injectors (separate servers over one workspace, as two MCP processes
        // would be) race to replace a low-confidence SIR without force: the one that
        // takes the lock second must see the first's high-confidence write and be
        // blocked by the guard instead of overwriting it.
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        seed_symbol(temp.path(), "sym-race", "crate::raced");
        seed_existing_sir(temp.path(), "sym-race", 0.1);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let workers: Vec<_> = ["first", "second"]
            .into_iter()
            .map(|label| {
                let workspace = temp.path().to_path_buf();
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let server = AetherMcpServer::new(&workspace, false).expect("server");
                    barrier.wait();
                    server
                        .aether_sir_inject_logic(AetherSirInjectRequest {
                            symbol: "sym-race".to_owned(),
                            intent: format!("{label} injector"),
                            behavior: None,
                            edge_cases: None,
                            side_effects: None,
                            dependencies: None,
                            error_modes: None,
                            confidence: Some(0.9),
                            inputs: None,
                            outputs: None,
                            complexity: None,
                            generation_pass: None,
                            model: None,
                            provider: None,
                            force: Some(false),
                        })
                        .expect("inject")
                })
            })
            .collect();
        let responses: Vec<AetherSirInjectResponse> = workers
            .into_iter()
            .map(|worker| worker.join().expect("join"))
            .collect();

        let injected: Vec<_> = responses
            .iter()
            .filter(|response| response.status == "injected")
            .collect();
        let blocked: Vec<_> = responses
            .iter()
            .filter(|response| response.status == "blocked")
            .collect();
        assert_eq!(injected.len(), 1, "{responses:?}");
        assert_eq!(blocked.len(), 1, "{responses:?}");
        assert_eq!(injected[0].previous_confidence, Some(0.1));
        assert_eq!(blocked[0].previous_confidence, Some(0.9));

        let store = aether_store::SqliteStore::open(temp.path()).expect("open store");
        let blob = store
            .read_sir_blob("sym-race")
            .expect("read blob")
            .expect("blob exists");
        let sir: SirAnnotation = serde_json::from_str(&blob).expect("parse sir");
        assert_eq!(sir.confidence, 0.9);
        assert!(
            sir.intent.ends_with(" injector"),
            "winner's intent kept: {}",
            sir.intent
        );
        assert_eq!(
            store.list_sir_history("sym-race").expect("history").len(),
            2,
            "seed plus exactly one injection"
        );
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

    #[test]
    fn inject_response_rounds_confidence_for_display() {
        assert_eq!(round_confidence_for_display(0.97), 0.97);
        assert_eq!(round_confidence_for_display(0.123_456), 0.1235);
        let response = AetherSirInjectResponse {
            symbol_id: "sym".to_owned(),
            qualified_name: "crate::sym".to_owned(),
            sir_hash: "hash".to_owned(),
            sir_version: 1,
            previous_confidence: Some(0.83),
            new_confidence: 0.97,
            status: "injected".to_owned(),
            note: None,
            embedding_status: "skipped: embeddings disabled".to_owned(),
            file_rollup_status: "refreshed".to_owned(),
        };
        let rendered = serde_json::to_string(&response).expect("serialize");
        assert!(rendered.contains("\"new_confidence\":0.97"), "{rendered}");
        assert!(
            rendered.contains("\"previous_confidence\":0.83"),
            "{rendered}"
        );
        assert!(!rendered.contains("0.9700000"), "{rendered}");
        // serde_json::Value goes through f64 too, which is where the artifact used to appear.
        let value = serde_json::to_value(&response).expect("to_value");
        assert_eq!(value["new_confidence"].to_string(), "0.97");
    }
}
