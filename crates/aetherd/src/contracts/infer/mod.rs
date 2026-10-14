//! Implicit contract inference from SIR history.
//!
//! `aetherd contract infer <symbol>` reads a symbol's SIR history, extracts
//! sentence-level claims from each version, embeds them in one pass,
//! clusters them with a version-aware DBSCAN on cosine distance, and promotes
//! the medoid sentence of every cluster that persists across enough versions
//! to a `must` contract with `created_by = "inferred"`.
//!
//! The feature is opt-in (`[contracts] implicit_inference_enabled`), runs one
//! symbol at a time, and prefers missing an invariant to creating a false
//! contract: clusters must cover enough versions, the medoid itself must be
//! supported by enough versions, the newest version must still state the
//! claim, and the claim must not already fail verification against the
//! current SIR embedding (read from the configured vector store).
//!
//! The provider decides how many requests one pass costs: Gemini sends up to
//! 100 texts per request, while `openai_compat` and `qwen3_local` send one
//! request per text. `--dry-run` embeds too; it only skips the writes.

mod claims;
mod cluster;
#[cfg(test)]
mod tests;

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use aether_config::{AetherConfig, ContractsConfig};
use aether_infer::{
    EmbeddingProviderOverrides, EmbeddingPurpose, LoadedEmbeddingProvider,
    load_embedding_provider_from_config,
};
use aether_store::{IntentContractRecord, SirHistoryStore, SqliteStore};
use anyhow::{Context, Result, anyhow, bail};

use self::claims::{HistoryStats, comparison_key, prepare_history};
use self::cluster::{
    ClaimSpace, Instance, Neighborhoods, anisotropy_check, core_min_versions, dbscan, distance,
    dot, required_versions, summarize_clusters, unit_vector,
};
use super::verify::cosine_similarity;
use crate::cli::ContractInferArgs;

/// Default clustering radius as cosine distance (similarity >= 0.88).
pub(crate) const DEFAULT_IMPLICIT_CLUSTER_EPS: f64 = 0.12;
/// Largest `--eps` accepted on the command line.
pub(crate) const MAX_IMPLICIT_CLUSTER_EPS: f64 = 0.5;
/// At most this many contracts are created by one run.
pub(crate) const IMPLICIT_MAX_PROMOTED_PER_RUN: usize = 5;
/// Inferred medoids are descriptive statements future SIRs should keep making.
pub(crate) const INFERRED_CLAUSE_TYPE: &str = "must";
pub(crate) const INFERRED_CREATED_BY: &str = "inferred";
/// Fallback when `embedding_fail_threshold` is not a finite number.
const DEFAULT_FAIL_THRESHOLD: f64 = 0.50;
/// Cluster lines printed per run.
const MAX_REPORTED_CLUSTERS: usize = 10;

/// Embeds claim texts. Production uses the configured embedding provider;
/// tests inject fixed vectors.
pub(crate) trait ClaimEmbedder {
    /// Exactly one vector per input text, in input order.
    fn embed_claims(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

/// [`ClaimEmbedder`] backed by the workspace's configured embedding provider.
pub(crate) struct ProviderClaimEmbedder {
    loaded: LoadedEmbeddingProvider,
    runtime: tokio::runtime::Runtime,
}

impl ProviderClaimEmbedder {
    pub(crate) fn new(loaded: LoadedEmbeddingProvider) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("failed to create embedding runtime")?;
        Ok(Self { loaded, runtime })
    }

    pub(crate) fn load(workspace: &Path) -> Result<Self> {
        let loaded =
            load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default())
                .context("failed to load embedding provider")?
                .ok_or_else(|| {
                    anyhow!(
                        "implicit contract inference requires an embedding provider; \
                         set enabled = true under [embeddings]"
                    )
                })?;
        Self::new(loaded)
    }
}

impl ClaimEmbedder for ProviderClaimEmbedder {
    fn embed_claims(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        // Announced before the call: depending on the provider this can be
        // one request per text, and it happens under --dry-run as well.
        eprintln!(
            "Embedding {} text(s) with {}/{}...",
            texts.len(),
            self.loaded.provider_name,
            self.loaded.model_name
        );
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        self.runtime
            .block_on(
                self.loaded
                    .provider
                    .embed_texts_with_purpose(&refs, EmbeddingPurpose::Document),
            )
            .map_err(anyhow::Error::from)
            .with_context(|| {
                format!(
                    "failed to embed {} claim text(s) with {}/{}",
                    texts.len(),
                    self.loaded.provider_name,
                    self.loaded.model_name
                )
            })
    }
}

/// Resolved inference parameters.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct InferSettings {
    /// Minimum considered versions before inference runs.
    pub min_versions: usize,
    /// Newest versions read from history.
    pub window: usize,
    /// Fraction of considered versions a claim must appear in.
    pub threshold: f64,
    /// Clustering radius as cosine distance.
    pub eps: f64,
    /// Verification fail threshold used by the pre-write guard.
    pub fail_threshold: f64,
    /// Cap on contracts created per run.
    pub max_promoted: usize,
}

impl InferSettings {
    pub(crate) fn from_config(cfg: &ContractsConfig, eps_override: Option<f64>) -> Self {
        let eps = eps_override
            .filter(|eps| eps.is_finite() && *eps > 0.0 && *eps <= MAX_IMPLICIT_CLUSTER_EPS)
            .unwrap_or(DEFAULT_IMPLICIT_CLUSTER_EPS);
        let fail_threshold = if cfg.embedding_fail_threshold.is_finite() {
            cfg.embedding_fail_threshold
        } else {
            DEFAULT_FAIL_THRESHOLD
        };
        Self {
            min_versions: cfg.effective_implicit_min_versions() as usize,
            window: cfg.implicit_history_window() as usize,
            threshold: cfg.effective_implicit_persistence_threshold(),
            eps,
            fail_threshold,
            max_promoted: IMPLICIT_MAX_PROMOTED_PER_RUN,
        }
    }
}

/// One inference run for one symbol.
pub(crate) struct InferRequest<'a> {
    pub symbol_id: &'a str,
    pub settings: InferSettings,
    /// The symbol's current SIR embedding, used to skip claims that would
    /// fail verification immediately.
    pub current_sir_embedding: Option<Vec<f32>>,
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct InferenceReport {
    pub symbol_id: String,
    pub stats: HistoryStats,
    pub outcome: InferenceOutcome,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum InferenceOutcome {
    /// The newest SIR is a mock placeholder.
    CurrentSirIsMock,
    /// Fewer usable versions than `implicit_min_versions`.
    InsufficientHistory {
        considered: usize,
        required: usize,
    },
    /// No version produced a single claim.
    NoClaims,
    /// Distinct claims from the same version mostly fall within eps.
    EmbeddingSpaceTooCompressed {
        fraction: f64,
        pairs: usize,
        eps: f64,
    },
    Evaluated(Evaluation),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Evaluation {
    pub claims_extracted: usize,
    pub unique_texts: usize,
    pub dropped_zero_norm: usize,
    pub eps: f64,
    pub core_min_versions: usize,
    pub required_versions: usize,
    /// Whether the verification guard ran (a compatible SIR embedding exists).
    pub guard_active: bool,
    pub dry_run: bool,
    pub clusters: Vec<ClusterReport>,
}

impl Evaluation {
    fn count_decisions(&self, predicate: impl Fn(&Decision) -> bool) -> usize {
        self.clusters
            .iter()
            .filter(|cluster| predicate(&cluster.decision))
            .count()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ClusterReport {
    pub medoid_text: String,
    /// Raw provider vector of the medoid text, stored with a new contract.
    pub medoid_embedding: Vec<f32>,
    pub coverage: usize,
    pub support: usize,
    pub cohesion: f64,
    pub decision: Decision,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Decision {
    Created {
        contract_id: i64,
    },
    WouldCreate,
    /// The insert found the exact text already stored.
    AlreadyPresent,
    DuplicateOfExisting {
        contract_id: i64,
        created_by: String,
        active: bool,
        similarity: f64,
    },
    DuplicateInRun,
    BelowCoverage,
    BelowSupport {
        eps_needed: Option<f64>,
    },
    MissingFromLatest,
    WouldFailVerification {
        similarity: f64,
    },
    OverCap,
}

impl fmt::Display for Decision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Created { contract_id } => write!(f, "created #{contract_id}"),
            Self::WouldCreate => f.write_str("would create"),
            Self::AlreadyPresent => f.write_str("already present"),
            Self::DuplicateOfExisting {
                contract_id,
                created_by,
                active,
                similarity,
            } => {
                let state = if *active { "active" } else { "inactive" };
                write!(
                    f,
                    "duplicate of #{contract_id} ({created_by}, {state}, sim {similarity:.2})"
                )
            }
            Self::DuplicateInRun => f.write_str("duplicate in run"),
            Self::BelowCoverage => f.write_str("below coverage"),
            Self::BelowSupport { eps_needed } => {
                // Round up so the printed value really reaches the support.
                let hint = eps_needed
                    .map(|eps| (eps * 1000.0).ceil() / 1000.0)
                    .filter(|eps| *eps <= MAX_IMPLICIT_CLUSTER_EPS);
                match hint {
                    Some(eps) => write!(f, "below support; would pass at --eps {eps:.3}"),
                    None => f.write_str("below support"),
                }
            }
            Self::MissingFromLatest => f.write_str("missing from latest version"),
            Self::WouldFailVerification { similarity } => {
                write!(f, "would fail verification (sim {similarity:.2})")
            }
            Self::OverCap => f.write_str("over per-run cap"),
        }
    }
}

/// A cluster that passed every invariant check, waiting for deduplication.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Candidate {
    pub medoid_text: String,
    pub medoid_version: i64,
    pub medoid_unit: Vec<f64>,
    pub medoid_embedding: Vec<f32>,
    /// Unit vectors of the cluster's distinct member texts.
    pub member_units: Vec<Vec<f64>>,
    pub coverage: usize,
    pub support: usize,
    pub cohesion: f64,
}

impl Candidate {
    fn into_report(self, decision: Decision) -> ClusterReport {
        ClusterReport {
            medoid_text: self.medoid_text,
            medoid_embedding: self.medoid_embedding,
            coverage: self.coverage,
            support: self.support,
            cohesion: self.cohesion,
            decision,
        }
    }
}

/// An existing contract of the symbol (active or not), for deduplication.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExistingClause {
    pub id: i64,
    pub created_by: String,
    pub active: bool,
    pub key: String,
    /// Fresh embedding of the clause text; `None` disables semantic dedupe
    /// for this clause (text dedupe still applies).
    pub unit: Option<Vec<f64>>,
}

/// `aetherd contract infer` entry point.
pub(super) fn run_infer(
    workspace: &Path,
    config: &AetherConfig,
    args: &ContractInferArgs,
) -> Result<()> {
    let Some(cfg) = config
        .contracts
        .as_ref()
        .filter(|contracts| contracts.implicit_inference_enabled)
    else {
        bail!(
            "implicit contract inference is disabled; set implicit_inference_enabled = true \
             under [contracts] in .aether/config.toml"
        );
    };

    let report = infer_in_workspace(workspace, cfg, args, || {
        ProviderClaimEmbedder::load(workspace)
    })?;
    print_report(&report, cfg);
    Ok(())
}

/// Resolve the symbol, read its current SIR embedding from the configured
/// vector store (as the SIR pipeline does before verifying contracts), and
/// run inference. Split from [`run_infer`] so tests can inject the embedder.
pub(crate) fn infer_in_workspace<E, F>(
    workspace: &Path,
    cfg: &ContractsConfig,
    args: &ContractInferArgs,
    make_embedder: F,
) -> Result<InferenceReport>
where
    E: ClaimEmbedder,
    F: FnOnce() -> Result<E>,
{
    let store = SqliteStore::open(workspace).context("failed to open store")?;
    let symbol_id = super::resolve_symbol_id(&store, &args.symbol)?;
    let settings = InferSettings::from_config(cfg, args.eps);
    let current_sir_embedding = super::load_current_sir_embedding(workspace, &symbol_id)?;
    let request = InferRequest {
        symbol_id: &symbol_id,
        settings,
        current_sir_embedding,
        dry_run: args.dry_run,
    };
    infer_for_symbol(&store, &request, make_embedder)
}

/// Run inference for one symbol. The embedder is built only after the
/// history gates pass, and nothing is written unless the whole evaluation
/// succeeds (and `dry_run` is false).
pub(crate) fn infer_for_symbol<E, F>(
    store: &SqliteStore,
    request: &InferRequest<'_>,
    make_embedder: F,
) -> Result<InferenceReport>
where
    E: ClaimEmbedder,
    F: FnOnce() -> Result<E>,
{
    let settings = &request.settings;
    let records = store
        .list_sir_history(request.symbol_id)
        .with_context(|| format!("failed to read SIR history for {}", request.symbol_id))?;
    let prepared = prepare_history(&records, settings);
    let report = |stats: HistoryStats, outcome: InferenceOutcome| InferenceReport {
        symbol_id: request.symbol_id.to_owned(),
        stats,
        outcome,
    };

    if prepared.newest_is_mock {
        return Ok(report(prepared.stats, InferenceOutcome::CurrentSirIsMock));
    }
    let considered = prepared.versions.len();
    if considered < settings.min_versions {
        return Ok(report(
            prepared.stats,
            InferenceOutcome::InsufficientHistory {
                considered,
                required: settings.min_versions,
            },
        ));
    }
    let claims_extracted: usize = prepared
        .versions
        .iter()
        .map(|version| version.claims.len())
        .sum();
    if claims_extracted == 0 {
        return Ok(report(prepared.stats, InferenceOutcome::NoClaims));
    }

    let existing = store
        .list_intent_contracts_for_symbol(request.symbol_id, true)
        .with_context(|| format!("failed to load contracts for {}", request.symbol_id))?;

    // One embedding pass: every claim text plus every existing clause text,
    // sorted and deduplicated. Existing clauses are re-embedded so their
    // vectors share the current provider's space; they are never written back.
    let mut text_set: BTreeSet<&str> = prepared
        .versions
        .iter()
        .flat_map(|version| version.claims.iter().map(|claim| claim.text.as_str()))
        .collect();
    text_set.extend(
        existing
            .iter()
            .map(|contract| contract.clause_text.as_str())
            .filter(|text| !text.trim().is_empty()),
    );
    let texts: Vec<String> = text_set.into_iter().map(str::to_owned).collect();
    let text_index: BTreeMap<&str, usize> = texts
        .iter()
        .enumerate()
        .map(|(idx, text)| (text.as_str(), idx))
        .collect();

    let embedder = make_embedder()?;
    let vectors = embedder.embed_claims(&texts)?;
    let dim = validate_vectors(&texts, &vectors)?;
    let units: Vec<Option<Vec<f64>>> = vectors.iter().map(|vector| unit_vector(vector)).collect();

    let mut instances = Vec::with_capacity(claims_extracted);
    let mut dropped_zero_norm = 0;
    for (version_idx, version) in prepared.versions.iter().enumerate() {
        for claim in &version.claims {
            let text_idx = text_index[claim.text.as_str()];
            if units[text_idx].is_none() {
                dropped_zero_norm += 1;
                continue;
            }
            instances.push(Instance {
                version_idx,
                text_idx,
            });
        }
    }

    let space = ClaimSpace {
        texts: &texts,
        units: &units,
        instances: &instances,
        considered,
    };
    let anisotropy = anisotropy_check(&space, settings.eps);
    if anisotropy.is_too_compressed() {
        return Ok(report(
            prepared.stats,
            InferenceOutcome::EmbeddingSpaceTooCompressed {
                fraction: anisotropy.fraction(),
                pairs: anisotropy.pairs,
                eps: settings.eps,
            },
        ));
    }

    let required = required_versions(settings.threshold, considered);
    let core_min = core_min_versions(required);
    let neighborhoods = Neighborhoods::build(&space, settings.eps);
    let labels = dbscan(&space, &neighborhoods, core_min);
    let summaries = summarize_clusters(&space, &neighborhoods, &labels, required);

    // The guard compares exactly like contract verification does, so it only
    // runs against a finite, non-zero SIR embedding of the same dimension.
    let guard_embedding = request
        .current_sir_embedding
        .as_deref()
        .filter(|embedding| embedding.len() == dim)
        .filter(|embedding| embedding.iter().all(|value| value.is_finite()))
        .filter(|embedding| unit_vector(embedding).is_some());
    let newest_version_idx = considered - 1;

    let mut rejected = Vec::new();
    let mut candidates = Vec::new();
    for summary in summaries {
        let medoid = instances[summary.medoid];
        let medoid_unit = units[medoid.text_idx].clone().unwrap_or_default();
        let member_texts: BTreeSet<usize> = summary
            .members
            .iter()
            .map(|&member| instances[member].text_idx)
            .collect();
        let candidate = Candidate {
            medoid_text: texts[medoid.text_idx].clone(),
            medoid_version: prepared.versions[medoid.version_idx].version,
            member_units: member_texts
                .iter()
                .filter_map(|&text_idx| units[text_idx].clone())
                .collect(),
            medoid_embedding: vectors[medoid.text_idx].clone(),
            coverage: summary.coverage,
            support: summary.support_versions.len(),
            cohesion: summary.cohesion,
            medoid_unit,
        };

        let rejection = if candidate.coverage < required {
            Some(Decision::BelowCoverage)
        } else if candidate.support < required {
            Some(Decision::BelowSupport {
                eps_needed: summary.eps_needed,
            })
        } else if !summary.support_versions.contains(&newest_version_idx) {
            Some(Decision::MissingFromLatest)
        } else {
            guard_embedding
                .and_then(|sir| cosine_similarity(&candidate.medoid_embedding, sir))
                .filter(|similarity| *similarity < settings.fail_threshold)
                .map(|similarity| Decision::WouldFailVerification { similarity })
        };
        match rejection {
            Some(decision) => rejected.push(candidate.into_report(decision)),
            None => candidates.push(candidate),
        }
    }

    let existing_clauses: Vec<ExistingClause> = existing
        .iter()
        .map(|contract| existing_clause(contract, &text_index, &units))
        .collect();
    let mut clusters = select_promotions(candidates, &existing_clauses, settings);

    if !request.dry_run {
        for cluster in &mut clusters {
            if cluster.decision != Decision::WouldCreate {
                continue;
            }
            let embedding_json = serde_json::to_string(&cluster.medoid_embedding)
                .context("failed to serialize inferred clause embedding")?;
            let inserted = store
                .insert_intent_contract_if_absent(
                    request.symbol_id,
                    INFERRED_CLAUSE_TYPE,
                    &cluster.medoid_text,
                    Some(&embedding_json),
                    INFERRED_CREATED_BY,
                )
                .with_context(|| {
                    format!(
                        "failed to insert inferred contract for {}",
                        request.symbol_id
                    )
                })?;
            cluster.decision = match inserted {
                Some(contract_id) => Decision::Created { contract_id },
                None => Decision::AlreadyPresent,
            };
        }
    }

    clusters.extend(rejected);
    clusters.sort_by(|left, right| {
        right
            .coverage
            .cmp(&left.coverage)
            .then_with(|| right.support.cmp(&left.support))
            .then_with(|| left.medoid_text.cmp(&right.medoid_text))
    });

    Ok(report(
        prepared.stats,
        InferenceOutcome::Evaluated(Evaluation {
            claims_extracted,
            unique_texts: texts.len(),
            dropped_zero_norm,
            eps: settings.eps,
            core_min_versions: core_min,
            required_versions: required,
            guard_active: guard_embedding.is_some(),
            dry_run: request.dry_run,
            clusters,
        }),
    ))
}

/// Check the embedder's output shape; returns the shared dimension.
fn validate_vectors(texts: &[String], vectors: &[Vec<f32>]) -> Result<usize> {
    if vectors.len() != texts.len() {
        bail!(
            "embedding provider returned {} vector(s) for {} text(s)",
            vectors.len(),
            texts.len()
        );
    }
    let dim = vectors.first().map_or(0, Vec::len);
    for (text, vector) in texts.iter().zip(vectors) {
        if vector.is_empty() {
            bail!("embedding provider returned an empty vector for \"{text}\"");
        }
        if vector.len() != dim {
            bail!(
                "embedding provider returned mixed dimensions ({} and {dim})",
                vector.len()
            );
        }
        if vector.iter().any(|value| !value.is_finite()) {
            bail!("embedding provider returned a non-finite value for \"{text}\"");
        }
    }
    Ok(dim)
}

fn existing_clause(
    contract: &IntentContractRecord,
    text_index: &BTreeMap<&str, usize>,
    units: &[Option<Vec<f64>>],
) -> ExistingClause {
    ExistingClause {
        id: contract.id,
        created_by: contract.created_by.clone(),
        active: contract.active,
        key: comparison_key(&contract.clause_text),
        unit: text_index
            .get(contract.clause_text.as_str())
            .and_then(|&idx| units[idx].clone()),
    }
}

/// Rank order for accepted candidates: support, coverage, cohesion and
/// medoid version descending, then text ascending.
fn rank_candidates(left: &Candidate, right: &Candidate) -> Ordering {
    right
        .support
        .cmp(&left.support)
        .then_with(|| right.coverage.cmp(&left.coverage))
        .then_with(|| right.cohesion.total_cmp(&left.cohesion))
        .then_with(|| right.medoid_version.cmp(&left.medoid_version))
        .then_with(|| left.medoid_text.cmp(&right.medoid_text))
}

/// Deduplicate accepted candidates against existing contracts and each
/// other, then apply the per-run cap. Kept candidates get
/// [`Decision::WouldCreate`].
pub(crate) fn select_promotions(
    mut candidates: Vec<Candidate>,
    existing: &[ExistingClause],
    settings: &InferSettings,
) -> Vec<ClusterReport> {
    candidates.sort_by(rank_candidates);
    let mut kept: Vec<(String, Vec<f64>)> = Vec::new();
    let mut reports = Vec::with_capacity(candidates.len());

    for candidate in candidates {
        let key = comparison_key(&candidate.medoid_text);
        let decision = if let Some(duplicate) = text_duplicate(&key, existing) {
            duplicate
        } else if let Some(duplicate) = semantic_duplicate(&candidate, existing, settings.eps) {
            duplicate
        } else if kept.iter().any(|(kept_key, kept_unit)| {
            *kept_key == key || distance(kept_unit, &candidate.medoid_unit) <= settings.eps
        }) {
            Decision::DuplicateInRun
        } else if kept.len() >= settings.max_promoted {
            Decision::OverCap
        } else {
            kept.push((key, candidate.medoid_unit.clone()));
            Decision::WouldCreate
        };
        reports.push(candidate.into_report(decision));
    }
    reports
}

fn text_duplicate(key: &str, existing: &[ExistingClause]) -> Option<Decision> {
    existing
        .iter()
        .find(|clause| clause.key == key)
        .map(|clause| Decision::DuplicateOfExisting {
            contract_id: clause.id,
            created_by: clause.created_by.clone(),
            active: clause.active,
            similarity: 1.0,
        })
}

/// An existing clause duplicates a candidate when it lies within eps of any
/// cluster member, i.e. it would have joined the cluster.
fn semantic_duplicate(
    candidate: &Candidate,
    existing: &[ExistingClause],
    eps: f64,
) -> Option<Decision> {
    let mut best: Option<(&ExistingClause, f64)> = None;
    for clause in existing {
        let Some(unit) = clause.unit.as_deref() else {
            continue;
        };
        let similarity = candidate
            .member_units
            .iter()
            .map(|member| dot(member, unit))
            .max_by(f64::total_cmp)
            .unwrap_or(f64::NEG_INFINITY);
        if 1.0 - similarity > eps {
            continue;
        }
        if best.is_none_or(|(_, best_similarity)| similarity > best_similarity) {
            best = Some((clause, similarity));
        }
    }
    best.map(|(clause, similarity)| Decision::DuplicateOfExisting {
        contract_id: clause.id,
        created_by: clause.created_by.clone(),
        active: clause.active,
        similarity: similarity.min(1.0),
    })
}

fn history_line(stats: &HistoryStats) -> String {
    let models = if stats.models.is_empty() {
        "none".to_owned()
    } else {
        stats
            .models
            .iter()
            .map(|(model, count)| format!("{model} x{count}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "History: {} versions; considered {} ({} mock, {} unparseable, {} collapsed, {} outside window); models: {models}",
        stats.raw,
        stats.considered,
        stats.skipped_mock,
        stats.skipped_unparseable,
        stats.collapsed,
        stats.outside_window,
    )
}

fn print_report(report: &InferenceReport, cfg: &ContractsConfig) {
    let symbol_id = &report.symbol_id;
    let evaluation = match &report.outcome {
        InferenceOutcome::CurrentSirIsMock => {
            println!(
                "Implicit contract inference for {symbol_id}: current SIR is a mock placeholder; regenerate it first"
            );
            println!("{}", history_line(&report.stats));
            return;
        }
        InferenceOutcome::InsufficientHistory {
            considered,
            required,
        } => {
            println!(
                "Not enough SIR history for {symbol_id}: {considered} usable version(s), need {required} (implicit_min_versions)"
            );
            println!("{}", history_line(&report.stats));
            return;
        }
        InferenceOutcome::NoClaims => {
            println!(
                "No claims could be extracted from the SIR history of {symbol_id}; nothing to infer"
            );
            println!("{}", history_line(&report.stats));
            return;
        }
        InferenceOutcome::EmbeddingSpaceTooCompressed {
            fraction,
            pairs,
            eps,
        } => {
            println!(
                "Embedding space too compressed for {symbol_id}: {:.0}% of {pairs} distinct within-version claim pairs are within eps {eps:.3}; \
                 no contracts inferred. Try a smaller --eps.",
                fraction * 100.0
            );
            println!("{}", history_line(&report.stats));
            return;
        }
        InferenceOutcome::Evaluated(evaluation) => evaluation,
    };

    let considered = report.stats.considered;
    println!("Implicit contract inference for {symbol_id}");
    println!("{}", history_line(&report.stats));
    println!(
        "Claims: {} extracted, {} unique texts embedded ({} dropped: zero-norm vector)",
        evaluation.claims_extracted, evaluation.unique_texts, evaluation.dropped_zero_norm
    );
    let guard = if evaluation.guard_active {
        "on".to_owned()
    } else {
        "skipped (no compatible SIR embedding)".to_owned()
    };
    println!(
        "Parameters: eps {:.3}, core_min_versions {}, required_versions {} of {considered}; verification guard: {guard}",
        evaluation.eps, evaluation.core_min_versions, evaluation.required_versions
    );
    if evaluation.clusters.is_empty() {
        println!("  No clusters found.");
    }
    for cluster in evaluation.clusters.iter().take(MAX_REPORTED_CLUSTERS) {
        println!(
            "  [{}] {INFERRED_CLAUSE_TYPE}: \"{}\" (coverage {}/{considered}, support {}/{considered}, cohesion {:.2})",
            cluster.decision,
            cluster.medoid_text,
            cluster.coverage,
            cluster.support,
            cluster.cohesion
        );
    }
    if evaluation.clusters.len() > MAX_REPORTED_CLUSTERS {
        println!(
            "  ... {} more cluster(s) not shown",
            evaluation.clusters.len() - MAX_REPORTED_CLUSTERS
        );
    }

    if evaluation.dry_run {
        let would_create =
            evaluation.count_decisions(|decision| matches!(decision, Decision::WouldCreate));
        println!("Dry run: {would_create} contract(s) would be created.");
    } else {
        let created =
            evaluation.count_decisions(|decision| matches!(decision, Decision::Created { .. }));
        println!("Created {created} inferred contract(s).");
    }
    if !cfg.enabled {
        println!(
            "Note: [contracts] enabled = false; inferred contracts are stored but not verified."
        );
    }
}
