//! Reuse a completed enrichment, without pretending its output was its input.
//!
//! Request fingerprints still cover the exact baseline sent to the provider. A
//! completion instead binds that baseline and request to the accepted output's
//! full write identity. The builder may recognize the completed operation only
//! while that output remains current and all other effective inputs still match.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_sir::{SirAnnotation, sir_hash};
use aether_store::{SirMetaRecord, SqliteStore};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::cli::BatchPass;
use crate::sir_pipeline::{SirIdentity, acquire_inject_write_lock};

use super::key::parse_key;

#[derive(Debug, Serialize, Deserialize)]
struct Completion {
    version: u32,
    pass: String,
    request_hash: String,
    output: SirIdentity,
    baseline_json: String,
}

pub(super) fn path(workspace: &Path, pass: BatchPass, symbol_id: &str) -> PathBuf {
    aether_config::aether_dir(workspace)
        .join("batch-completions")
        .join(pass.as_str())
        .join(format!("{}.json", aether_core::content_hash(symbol_id)))
}

/// Missing, corrupt or obsolete cache data never suppresses a real request.
pub(super) fn previous_baseline(
    workspace: &Path,
    pass: BatchPass,
    meta: &SirMetaRecord,
    identity: &SirIdentity,
    current_baseline: &SirAnnotation,
) -> Option<(String, SirAnnotation)> {
    let completion = match fs::read(path(workspace, pass, &meta.id)) {
        Ok(bytes) => match serde_json::from_slice::<Completion>(&bytes) {
            Ok(completion) => completion,
            Err(err) => {
                tracing::warn!(symbol_id = %meta.id, line = err.line(), column = err.column(), "ignoring invalid batch completion cache");
                return None;
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            tracing::warn!(symbol_id = %meta.id, error = %err, "could not read batch completion cache");
            return None;
        }
    };
    if completion.version != 1
        || completion.pass != pass.as_str()
        || meta.generation_pass != pass.as_str()
        || meta.sir_status != "fresh"
        || meta.prompt_hash.as_deref() != Some(completion.request_hash.as_str())
        || completion.output != *identity
        || sir_hash(current_baseline).as_str() != identity.sir_hash.as_str()
    {
        return None;
    }
    let baseline = serde_json::from_str::<SirAnnotation>(&completion.baseline_json).ok()?;
    Some((completion.request_hash, baseline))
}

/// Called only after ingest's embedding, vector and fingerprint work succeeds.
/// The shared writer lock prevents an older completion from replacing a newer
/// writer's receipt. Superseded outputs get no receipt.
pub(super) fn record(
    workspace: &Path,
    store: &SqliteStore,
    pass: BatchPass,
    request_key: &str,
    output: &SirIdentity,
    baseline_json: &str,
) -> Result<()> {
    if matches!(pass, BatchPass::Scan) {
        return Ok(());
    }
    let (symbol_id, request_hash) = parse_key(request_key)?;
    let _guard = acquire_inject_write_lock(workspace)?;
    let Some(row) = store
        .get_sir_meta_with_blob(symbol_id)
        .context("failed to check completed enrichment's current write")?
    else {
        return Ok(());
    };
    if row.identity != *output
        || row.meta.prompt_hash.as_deref() != Some(request_hash)
        || row.meta.generation_pass != pass.as_str()
        || row.meta.sir_status != "fresh"
    {
        return Ok(());
    }
    let completion = Completion {
        version: 1,
        pass: pass.as_str().to_owned(),
        request_hash: request_hash.to_owned(),
        output: output.clone(),
        baseline_json: baseline_json.to_owned(),
    };
    let dir = aether_config::aether_dir(workspace)
        .join("batch-completions")
        .join(pass.as_str());
    fs::create_dir_all(&dir).context("failed to create batch completion cache directory")?;
    let destination = path(workspace, pass, symbol_id);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("failed to timestamp batch completion cache write")?
        .as_nanos();
    let temporary = destination.with_extension(format!("json.tmp-{}-{nonce}", std::process::id()));
    let bytes =
        serde_json::to_vec(&completion).context("failed to encode batch completion cache")?;
    fs::write(&temporary, bytes).context("failed to write batch completion cache")?;
    fs::rename(&temporary, &destination).context("failed to publish batch completion cache")
}
