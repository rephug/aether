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

use super::{AetherMcpServer, LiveSymbolSources, current_unix_timestamp};
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
    /// The content hash of the symbol's source as read for this SIR (`source_hash` from
    /// `aether_symbol_lookup`). When given, the injection is refused, under the inject
    /// lock, if the file no longer declares the symbol or its body no longer hashes to
    /// this value: the SIR would describe text that has changed since it was read, and
    /// writing it would pre-empt the daemon's regeneration from the new source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hash: Option<String>,
}

/// `sir_status` recorded when a leaf was written but its file rollup could not be
/// rebuilt: the confidence guard lets the same injection be rerun without `force` (a
/// request that reconstructs the stored SIR; any other still needs `force`), and the
/// scan queries keep selecting the symbol, so the retry is never blocked.
pub const SIR_STATUS_ROLLUP_FAILED: &str = "rollup_failed";
/// `sir_status` a leaf is written with, in the same transaction as the leaf itself,
/// until its file rollup has been rebuilt; it is cleared to `fresh` only after the
/// rebuild succeeds. A process that exits between the two leaves this marker behind,
/// and the guard and the scan queries treat it exactly like `rollup_failed`, so the
/// documented unchanged rerun repairs the rollup instead of the leaf passing as done.
pub const SIR_STATUS_ROLLUP_PENDING: &str = "rollup_pending";

/// Whether a leaf's status says its file rollup still has to be rebuilt.
pub(crate) fn rollup_outstanding(status: &str) -> bool {
    status == SIR_STATUS_ROLLUP_FAILED || status == SIR_STATUS_ROLLUP_PENDING
}

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
        // The selector was resolved before the lock. The daemon removes symbols (row and
        // SIR together) under this same lock, so once it is held the symbol either still
        // exists or is gone for good; a leaf written for a removed symbol would be an
        // orphan the `sir` table has no foreign key to reject.
        if store.get_symbol_record(symbol_id.as_str())?.is_none() {
            return Err(AetherMcpError::Message(format!(
                "symbol '{qualified_name}' ({symbol_id}) was removed from the index while the request was being resolved; nothing was injected"
            )));
        }
        // The SIR describes the text the caller read. When the caller says which text
        // that was (`source_hash`), require, under the lock, that the file still declares
        // the symbol with exactly that body: after an edit the daemon re-indexes the
        // symbol and regenerates its SIR from the new source, and a leaf written now for
        // the old text would advance the identity that regeneration was planned against
        // and leave a SIR of the old implementation standing.
        let request_source_hash = request
            .source_hash
            .as_deref()
            .map(str::trim)
            .filter(|hash| !hash.is_empty())
            .map(str::to_owned);
        if let Some(expected) = request_source_hash.as_deref() {
            let mut live = LiveSymbolSources::new(&self.state.workspace);
            match live.hash_for(symbol.file_path.as_str(), symbol_id.as_str())? {
                Some(current) if current == expected => {}
                Some(_) => {
                    return Err(AetherMcpError::Message(format!(
                        "source for {qualified_name} ({}) changed since it was read; nothing was injected: re-read the symbol (aether_symbol_lookup reports its current source_hash) and describe the new text, or leave it to the daemon's regeneration",
                        symbol.file_path
                    )));
                }
                None => {
                    return Err(AetherMcpError::Message(format!(
                        "{} no longer declares {qualified_name} as indexed (or cannot be read); nothing was injected: wait for re-indexing and look the symbol up again",
                        symbol.file_path
                    )));
                }
            }
        }
        let previous_meta = store.get_sir_meta(symbol_id.as_str())?;
        let previous_blob = store.read_sir_blob(symbol_id.as_str())?;
        let previous_sir = previous_blob
            .as_deref()
            .map(serde_json::from_str::<SirAnnotation>)
            .transpose()?;
        let previous_confidence = previous_sir.as_ref().map(|sir| sir.confidence);
        let new_confidence = request.confidence.unwrap_or(DEFAULT_INJECT_CONFIDENCE);

        let intent = request.intent.trim();
        if intent.is_empty() {
            return Err(AetherMcpError::Message(
                "intent must not be empty".to_owned(),
            ));
        }

        let mut updated = previous_sir
            .clone()
            .unwrap_or_else(|| empty_sir_annotation(new_confidence));
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
        let previous_rollup_failed = previous_meta
            .as_ref()
            .is_some_and(|meta| rollup_outstanding(&meta.sir_status));
        // A `rollup_failed` or `rollup_pending` marker lifts the guard only for the
        // retry of the injection that left it: the request that reconstructs the stored
        // SIR exactly. Any other request queued against the earlier placeholder still
        // needs `force` to replace the reviewed SIR; instead of merely being blocked,
        // though, it repairs what the marker records as outstanding (below): the stored
        // SIR is kept, its file rollup rebuilt and the marker cleared, so a fresh scan
        // session that cannot reproduce the earlier annotation still completes the
        // symbol rather than leaving it pending round after round.
        let retries_failed_rollup = previous_rollup_failed
            && previous_sir
                .as_ref()
                .is_some_and(|previous| sir_hash(previous) == hash);
        // A leaf left with an outstanding rollup describes the source its own injection
        // was bound to (recorded with the leaf). When this request is bound to a
        // different, current source, the symbol's body has changed since: the stored
        // leaf is a SIR of the old text and is replaced rather than repaired, or it would
        // pass as `fresh` while describing code that no longer exists. A stored leaf
        // that never recorded its source (an injection without `source_hash`) offers no
        // evidence that it describes the current text either, so a request that is
        // bound to the current text replaces it too; only a request without a hash of
        // its own cannot tell and repairs.
        let pending_describes_older_source = previous_rollup_failed
            && match (
                store.get_sir_source_hash(symbol_id.as_str())?,
                request_source_hash.as_deref(),
            ) {
                (Some(stored), Some(current)) => stored != current,
                (None, Some(_)) => true,
                (_, None) => false,
            };
        if previous_confidence.is_some_and(|confidence| confidence > FORCE_CONFIDENCE_THRESHOLD)
            && !request.force.unwrap_or(false)
            && !retries_failed_rollup
            && !pending_describes_older_source
        {
            if let Some(previous_meta) = previous_meta
                .as_ref()
                .filter(|meta| rollup_outstanding(&meta.sir_status))
            {
                let mut repaired = self.repair_outstanding_rollup(
                    store,
                    &symbol,
                    previous_meta,
                    previous_confidence,
                    new_confidence,
                )?;
                // The interrupted injection never reached its embedding refresh (that
                // runs only after the rollup succeeds), so the stored leaf's vector is
                // refreshed here, after the inject lock is released, exactly as a
                // completed injection's would be.
                drop(_inject_guard);
                repaired.embedding_status = match previous_blob.as_deref() {
                    Some(stored_json) => self.refresh_embedding_after_inject(
                        symbol_id.as_str(),
                        previous_meta.sir_hash.as_str(),
                        stored_json,
                    ),
                    None => "skipped: no stored SIR text".to_owned(),
                };
                return Ok(repaired);
            }
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

        let provider = normalize_optional_text_with_default(request.provider, "manual");
        let model = normalize_optional_text_with_default(request.model, "manual");
        let generation_pass = normalize_optional_text_with_default(request.generation_pass, "deep");
        let rollup_identity = (provider.clone(), model.clone(), generation_pass.clone());
        let now = current_unix_timestamp();
        // History version, leaf JSON and metadata land in one SQLite transaction (the
        // same path the daemon's SIR pipeline uses): a failure between separate writes
        // would leave the new JSON under the old hash and a `fresh` status, a leaf that
        // the scan queries no longer select (its confidence is now high) and that the
        // guard blocks from an ordinary retry, so it could never be repaired. The leaf
        // is written as `rollup_pending` in that same transaction and becomes `fresh`
        // only once the rollup below has been rebuilt: a process that exits in between
        // leaves a marker the scan queries select and the guard admits a rerun for,
        // rather than a high-confidence leaf over a stale rollup that reads as done.
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
            sir_status: SIR_STATUS_ROLLUP_PENDING.to_owned(),
            last_error: None,
            last_attempt_at: now,
        };
        let version_write = store.persist_sir_state_atomically_with_source(
            meta_record.clone(),
            canonical_json.as_str(),
            None,
            None,
            request_source_hash.as_deref(),
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
        // The rollup is rebuilt: the leaf is complete. Clearing the marker is the last
        // write under the lock; if it fails, the leaf stays `rollup_pending` and the
        // documented unchanged rerun (admitted by the guard) rebuilds and clears it.
        store
            .upsert_sir_meta(SirMetaRecord {
                sir_status: "fresh".to_owned(),
                ..meta_record.clone()
            })
            .map_err(|err| {
                AetherMcpError::Message(format!(
                    "SIR for {qualified_name} and its file rollup were written but the leaf's rollup_pending marker could not be cleared: {err}; rerun the same injection"
                ))
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

    /// Rollup-only repair of a leaf whose earlier injection left its file rollup
    /// outstanding (`rollup_pending` or `rollup_failed`): the stored, high-confidence SIR
    /// is kept as it is, the file rollup is rebuilt from the current leaves under the
    /// inject lock the caller holds, and the marker is cleared to `fresh`. Nothing about
    /// the leaf itself changes, so the request's own annotation is not written; the
    /// response says so (`status: "rollup_repaired"`). The caller refreshes the stored
    /// leaf's embedding once the lock is released and fills in `embedding_status`.
    fn repair_outstanding_rollup(
        &self,
        store: &aether_store::SqliteStore,
        symbol: &SymbolRecord,
        previous_meta: &SirMetaRecord,
        previous_confidence: Option<f32>,
        new_confidence: f32,
    ) -> Result<AetherSirInjectResponse, AetherMcpError> {
        let file_rollup_status = self
            .refresh_file_rollup_after_inject(
                symbol.file_path.as_str(),
                previous_meta.provider.as_str(),
                previous_meta.model.as_str(),
                previous_meta.generation_pass.as_str(),
            )
            .map_err(|err| {
                AetherMcpError::Message(format!(
                    "the stored SIR for {} was kept but its outstanding file rollup for {} could not be rebuilt: {err:#}; rerun the injection",
                    symbol.qualified_name, symbol.file_path
                ))
            })?;
        store
            .upsert_sir_meta(SirMetaRecord {
                sir_status: "fresh".to_owned(),
                last_error: None,
                ..previous_meta.clone()
            })
            .map_err(|err| {
                AetherMcpError::Message(format!(
                    "the file rollup for {} was rebuilt but the {} marker on {} could not be cleared: {err}; rerun the injection",
                    symbol.file_path, previous_meta.sir_status, symbol.qualified_name
                ))
            })?;
        let note = previous_confidence.map(|confidence| {
            format!(
                "existing SIR confidence {confidence:.2} exceeds {FORCE_CONFIDENCE_THRESHOLD:.2} threshold and was kept; its file rollup, left {} by an earlier injection, was rebuilt and the marker cleared. Rerun with force=true to replace the SIR itself",
                previous_meta.sir_status
            )
        });
        Ok(AetherSirInjectResponse {
            symbol_id: symbol.id.clone(),
            qualified_name: symbol.qualified_name.clone(),
            sir_hash: previous_meta.sir_hash.clone(),
            sir_version: previous_meta.sir_version,
            previous_confidence,
            new_confidence,
            status: "rollup_repaired".to_owned(),
            note,
            embedding_status: "pending".to_owned(),
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
mod tests;
