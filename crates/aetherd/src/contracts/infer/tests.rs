use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use aether_config::{AetherConfig, ContractsConfig};
use aether_infer::{EmbeddingProvider, EmbeddingPurpose, InferError, LoadedEmbeddingProvider};
use aether_store::{
    SemanticIndexStore, SirHistoryStore, SqliteStore, SymbolCatalogStore, SymbolEmbeddingRecord,
    SymbolRecord, open_vector_store,
};
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use tempfile::{TempDir, tempdir};

use super::*;
use crate::cli::{ContractArgs, ContractCommand, ContractInferArgs};
use crate::contracts::run_contract_command;

const SYMBOL: &str = "sym_transfer_funds";
/// Mapped vectors live in the first `BASE_DIM` slots; unmapped texts get a
/// one-hot vector after them.
const BASE_DIM: usize = 16;

const MODEL_A: &str = "Validates the transfer amount before posting.";
const MODEL_B: &str = "Checks that the transfer amount is valid before it is posted.";
const HALF_CLAIM: &str = "Caches the exchange rate for later calls.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    Fail,
    WrongCount,
    MixedDims,
    NaN,
}

/// Deterministic embedder: mapped texts get their injected vector padded
/// with zeros, unmapped texts get an orthogonal one-hot vector.
struct MapEmbedder {
    vectors: BTreeMap<String, Vec<f32>>,
    calls: RefCell<Vec<Vec<String>>>,
    mode: Mode,
}

impl MapEmbedder {
    fn new(vectors: Vec<(&str, Vec<f32>)>) -> Self {
        Self {
            vectors: vectors
                .into_iter()
                .map(|(text, vector)| (text.to_owned(), vector))
                .collect(),
            calls: RefCell::new(Vec::new()),
            mode: Mode::Normal,
        }
    }

    fn with_mode(mut self, mode: Mode) -> Self {
        self.mode = mode;
        self
    }

    fn padded(&self, text: &str, dim: usize) -> Vec<f32> {
        let mut vector = self.vectors[text].clone();
        vector.resize(dim, 0.0);
        vector
    }
}

impl ClaimEmbedder for &MapEmbedder {
    fn embed_claims(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.calls.borrow_mut().push(texts.to_vec());
        if self.mode == Mode::Fail {
            return Err(anyhow!("injected embedding failure"));
        }
        let dim = BASE_DIM + texts.len();
        let mut vectors: Vec<Vec<f32>> = texts
            .iter()
            .enumerate()
            .map(|(idx, text)| match self.vectors.get(text) {
                Some(vector) => {
                    let mut padded = vector.clone();
                    padded.resize(dim, 0.0);
                    padded
                }
                None => {
                    let mut one_hot = vec![0.0; dim];
                    one_hot[BASE_DIM + idx] = 1.0;
                    one_hot
                }
            })
            .collect();
        match self.mode {
            Mode::WrongCount => {
                vectors.pop();
            }
            Mode::MixedDims => vectors.last_mut().expect("vectors").push(0.5),
            Mode::NaN => vectors[0][0] = f32::NAN,
            Mode::Normal | Mode::Fail => {}
        }
        Ok(vectors)
    }
}

fn never_built() -> Result<&'static MapEmbedder> {
    panic!("embedder must not be built")
}

fn axis(slot: usize) -> Vec<f32> {
    let mut vector = vec![0.0; BASE_DIM];
    vector[slot] = 1.0;
    vector
}

/// Unit vector with cosine `similarity` to `axis(base)`, tilted into `slot`.
fn tilted(base: usize, slot: usize, similarity: f32) -> Vec<f32> {
    let mut vector = vec![0.0; BASE_DIM];
    vector[base] = similarity;
    vector[slot] = (1.0 - similarity * similarity).sqrt();
    vector
}

fn noise(version: usize) -> String {
    format!("Logs diagnostic detail number {version} for tracing.")
}

/// Ten versions: the invariant in model-A phrasing in v1-5 and model-B
/// phrasing in v6-10, a claim in v1-5 only, and a unique noise sentence per
/// version so nothing collapses.
fn happy_history() -> Vec<Value> {
    (1..=10)
        .map(|version| {
            let mut sir = json!({
                "intent": if version <= 5 { MODEL_A } else { MODEL_B },
                "inputs": ["amount: Money value in minor units"],
                "outputs": ["Result of the posting"],
                "side_effects": [noise(version)],
                "dependencies": ["ledger::post"],
                "error_modes": [],
                "confidence": 0.8,
            });
            if version <= 5 {
                sir["behavior"] = json!(HALF_CLAIM);
            }
            sir
        })
        .collect()
}

fn happy_embedder() -> MapEmbedder {
    MapEmbedder::new(vec![
        (MODEL_A, axis(0)),
        (MODEL_B, tilted(0, 1, 0.99)),
        (HALF_CLAIM, axis(2)),
    ])
}

fn open_store() -> (TempDir, SqliteStore) {
    let dir = tempdir().expect("tempdir");
    let store = SqliteStore::open(dir.path()).expect("open store");
    (dir, store)
}

fn seed(store: &SqliteStore, sirs: &[Value]) {
    for (idx, sir) in sirs.iter().enumerate() {
        let write = store
            .record_sir_version_if_changed(
                SYMBOL,
                &format!("hash-{idx}"),
                "test-provider",
                "model-x",
                &sir.to_string(),
                1_700_000_000 + idx as i64,
                None,
            )
            .expect("record history");
        assert!(write.changed, "history row {idx} should be new");
    }
}

fn enabled_config() -> ContractsConfig {
    ContractsConfig {
        implicit_inference_enabled: true,
        ..ContractsConfig::default()
    }
}

fn request(dry_run: bool) -> InferRequest<'static> {
    InferRequest {
        symbol_id: SYMBOL,
        settings: InferSettings::from_config(&enabled_config(), None),
        current_sir_embedding: None,
        dry_run,
    }
}

fn evaluation(report: &InferenceReport) -> &Evaluation {
    match &report.outcome {
        InferenceOutcome::Evaluated(evaluation) => evaluation,
        other => panic!("expected an evaluation, got {other:?}"),
    }
}

fn cluster_for<'a>(evaluation: &'a Evaluation, text: &str) -> &'a ClusterReport {
    evaluation
        .clusters
        .iter()
        .find(|cluster| cluster.medoid_text == text)
        .unwrap_or_else(|| panic!("no cluster with medoid {text:?}: {evaluation:#?}"))
}

fn stored_contracts(store: &SqliteStore) -> Vec<aether_store::IntentContractRecord> {
    store
        .list_intent_contracts_for_symbol(SYMBOL, true)
        .expect("list contracts")
}

fn infer_args() -> ContractArgs {
    ContractArgs {
        command: ContractCommand::Infer(ContractInferArgs {
            symbol: SYMBOL.to_owned(),
            dry_run: false,
            eps: None,
        }),
    }
}

// ── Configuration and gates ─────────────────────────────────────────

#[test]
fn disabled_inference_refuses_before_opening_the_store() {
    let dir = tempdir().expect("tempdir");

    let err = run_contract_command(dir.path(), &AetherConfig::default(), infer_args())
        .expect_err("inference without [contracts] must be refused");
    assert!(
        format!("{err:#}").contains("implicit_inference_enabled"),
        "{err:#}"
    );

    let config = AetherConfig {
        contracts: Some(ContractsConfig {
            enabled: true,
            ..ContractsConfig::default()
        }),
        ..AetherConfig::default()
    };
    let err = run_contract_command(dir.path(), &config, infer_args())
        .expect_err("inference with implicit_inference_enabled = false must be refused");
    assert!(
        format!("{err:#}").contains("implicit_inference_enabled"),
        "{err:#}"
    );

    assert!(!dir.path().join(".aether").exists());
}

#[test]
fn settings_resolve_config_and_eps_override() {
    let settings = InferSettings::from_config(&enabled_config(), None);
    assert_eq!(settings.min_versions, 10);
    assert_eq!(settings.window, 20);
    assert_eq!(settings.threshold, 0.80);
    assert_eq!(settings.eps, DEFAULT_IMPLICIT_CLUSTER_EPS);
    assert_eq!(settings.fail_threshold, 0.50);
    assert_eq!(settings.max_promoted, IMPLICIT_MAX_PROMOTED_PER_RUN);

    assert_eq!(
        InferSettings::from_config(&enabled_config(), Some(0.2)).eps,
        0.2
    );
    let raw = ContractsConfig {
        implicit_min_versions: 1,
        implicit_persistence_threshold: 0.1,
        embedding_fail_threshold: f64::NAN,
        ..enabled_config()
    };
    let settings = InferSettings::from_config(&raw, Some(0.9));
    assert_eq!(settings.min_versions, 3);
    assert_eq!(settings.window, 6);
    assert_eq!(settings.threshold, 0.5);
    assert_eq!(settings.eps, DEFAULT_IMPLICIT_CLUSTER_EPS);
    assert_eq!(settings.fail_threshold, 0.50);
}

#[test]
fn too_few_versions_reports_insufficient_history_without_embedding() {
    let (_dir, store) = open_store();
    seed(&store, &happy_history()[..9]);

    let report = infer_for_symbol(&store, &request(false), never_built).expect("infer");
    assert_eq!(
        report.outcome,
        InferenceOutcome::InsufficientHistory {
            considered: 9,
            required: 10,
        }
    );
    assert_eq!(report.stats.considered, 9);
    assert!(stored_contracts(&store).is_empty());
}

#[test]
fn newest_mock_sir_stops_inference() {
    let (_dir, store) = open_store();
    let mut history = happy_history();
    history.push(json!({"intent": "[MOCK] placeholder summary for this symbol"}));
    seed(&store, &history);

    let report = infer_for_symbol(&store, &request(false), never_built).expect("infer");
    assert_eq!(report.outcome, InferenceOutcome::CurrentSirIsMock);
    assert_eq!(report.stats.skipped_mock, 1);
    assert!(stored_contracts(&store).is_empty());
}

#[test]
fn history_without_claims_reports_no_claims() {
    let (_dir, store) = open_store();
    // Consecutive claim-free versions collapse into one, so the gate needs a
    // hand-built minimum below anything the config allows.
    seed(
        &store,
        &[
            json!({"intent": "Short."}),
            json!({"intent": "None.", "side_effects": ["No side effects."]}),
            json!({"intent": "Does it."}),
        ],
    );
    let mut request = request(false);
    request.settings.min_versions = 1;

    let report = infer_for_symbol(&store, &request, never_built).expect("infer");
    assert_eq!(report.outcome, InferenceOutcome::NoClaims);
    assert_eq!(report.stats.collapsed, 2);
}

// ── Promotion ───────────────────────────────────────────────────────

#[test]
fn stable_claim_becomes_one_inferred_contract_with_medoid_text() {
    let (_dir, store) = open_store();
    seed(&store, &happy_history());
    let embedder = happy_embedder();

    let report = infer_for_symbol(&store, &request(false), || Ok(&embedder)).expect("infer");
    let evaluation = evaluation(&report);
    assert_eq!(report.stats.considered, 10);
    assert_eq!(evaluation.required_versions, 8);
    assert_eq!(evaluation.core_min_versions, 4);
    assert!(!evaluation.guard_active);

    let invariant = cluster_for(evaluation, MODEL_B);
    assert_eq!(invariant.coverage, 10);
    assert_eq!(invariant.support, 10);
    let Decision::Created { contract_id } = invariant.decision else {
        panic!("expected a created contract, got {:?}", invariant.decision);
    };
    let half = cluster_for(evaluation, HALF_CLAIM);
    assert_eq!(half.coverage, 5);
    assert_eq!(half.decision, Decision::BelowCoverage);

    let contracts = stored_contracts(&store);
    assert_eq!(contracts.len(), 1);
    let contract = &contracts[0];
    assert_eq!(contract.id, contract_id);
    assert_eq!(contract.created_by, "inferred");
    assert_eq!(contract.clause_type, "must");
    assert_eq!(contract.clause_text, MODEL_B);
    assert!(contract.active);

    let calls = embedder.calls.borrow();
    assert_eq!(calls.len(), 1, "exactly one batch embed call");
    let expected: BTreeSet<String> = [MODEL_A, MODEL_B, HALF_CLAIM]
        .into_iter()
        .map(str::to_owned)
        .chain((1..=10).map(noise))
        .collect();
    assert_eq!(calls[0], expected.into_iter().collect::<Vec<_>>());

    let stored: Vec<f32> = serde_json::from_str(
        contract
            .clause_embedding_json
            .as_deref()
            .expect("embedding stored"),
    )
    .expect("embedding json");
    assert_eq!(stored, embedder.padded(MODEL_B, BASE_DIM + calls[0].len()));
}

#[test]
fn rerun_does_not_duplicate_contracts() {
    let (_dir, store) = open_store();
    seed(&store, &happy_history());
    let embedder = happy_embedder();
    infer_for_symbol(&store, &request(false), || Ok(&embedder)).expect("first run");
    assert_eq!(stored_contracts(&store).len(), 1);

    let report = infer_for_symbol(&store, &request(false), || Ok(&embedder)).expect("rerun");
    let invariant = cluster_for(evaluation(&report), MODEL_B);
    assert!(
        matches!(
            &invariant.decision,
            Decision::DuplicateOfExisting { created_by, active: true, .. } if created_by == "inferred"
        ) || invariant.decision == Decision::AlreadyPresent,
        "unexpected decision {:?}",
        invariant.decision
    );
    assert_eq!(stored_contracts(&store).len(), 1);
}

#[test]
fn dry_run_reports_without_writing() {
    let (_dir, store) = open_store();
    seed(&store, &happy_history());
    let embedder = happy_embedder();

    let report = infer_for_symbol(&store, &request(true), || Ok(&embedder)).expect("infer");
    let evaluation = evaluation(&report);
    assert!(evaluation.dry_run);
    assert_eq!(
        cluster_for(evaluation, MODEL_B).decision,
        Decision::WouldCreate
    );
    assert!(stored_contracts(&store).is_empty());
}

#[test]
fn claim_missing_from_newest_version_is_not_promoted() {
    let (_dir, store) = open_store();
    let history: Vec<Value> = (1..=10)
        .map(|version| {
            json!({
                "intent": if version <= 9 { MODEL_A } else { "Something else entirely happens now." },
                "side_effects": [noise(version)],
            })
        })
        .collect();
    seed(&store, &history);
    let embedder = MapEmbedder::new(vec![(MODEL_A, axis(0))]);

    let report = infer_for_symbol(&store, &request(false), || Ok(&embedder)).expect("infer");
    let invariant = cluster_for(evaluation(&report), MODEL_A);
    assert_eq!(invariant.coverage, 9);
    assert_eq!(invariant.decision, Decision::MissingFromLatest);
    assert!(stored_contracts(&store).is_empty());
}

#[test]
fn verification_guard_blocks_claims_the_current_sir_contradicts() {
    let (_dir, store) = open_store();
    seed(&store, &happy_history());
    let embedder = happy_embedder();
    let probe = infer_for_symbol(&store, &request(true), || Ok(&embedder)).expect("probe");
    let dim = BASE_DIM + evaluation(&probe).unique_texts;

    let mut orthogonal = vec![0.0_f32; dim];
    orthogonal[BASE_DIM - 1] = 1.0;
    let mut guarded = request(false);
    guarded.current_sir_embedding = Some(orthogonal);
    let report = infer_for_symbol(&store, &guarded, || Ok(&embedder)).expect("infer");
    let evaluation_with_guard = evaluation(&report);
    assert!(evaluation_with_guard.guard_active);
    let invariant = cluster_for(evaluation_with_guard, MODEL_B);
    assert!(
        matches!(invariant.decision, Decision::WouldFailVerification { similarity } if similarity.abs() < 1e-9),
        "unexpected decision {:?}",
        invariant.decision
    );
    assert!(stored_contracts(&store).is_empty());

    let mut agreeing = request(false);
    agreeing.current_sir_embedding = Some(embedder.padded(MODEL_B, dim));
    let report = infer_for_symbol(&store, &agreeing, || Ok(&embedder)).expect("infer");
    assert!(evaluation(&report).guard_active);
    assert!(matches!(
        cluster_for(evaluation(&report), MODEL_B).decision,
        Decision::Created { .. }
    ));

    // An embedding of another dimension cannot be compared: guard skipped.
    let mut mismatched = request(true);
    mismatched.current_sir_embedding = Some(vec![1.0, 0.0, 0.0]);
    let report = infer_for_symbol(&store, &mismatched, || Ok(&embedder)).expect("infer");
    assert!(!evaluation(&report).guard_active);
}

#[test]
fn compressed_embedding_space_creates_nothing() {
    let (_dir, store) = open_store();
    seed(&store, &happy_history());
    let mut vectors = vec![
        (MODEL_A, tilted(0, 1, 0.99)),
        (MODEL_B, tilted(0, 2, 0.99)),
        (HALF_CLAIM, tilted(0, 3, 0.99)),
    ];
    let noises: Vec<String> = (1..=10).map(noise).collect();
    for (offset, text) in noises.iter().enumerate() {
        vectors.push((text.as_str(), tilted(0, 4 + offset, 0.99)));
    }
    let embedder = MapEmbedder::new(vectors);

    let report = infer_for_symbol(&store, &request(false), || Ok(&embedder)).expect("infer");
    match report.outcome {
        InferenceOutcome::EmbeddingSpaceTooCompressed {
            fraction, pairs, ..
        } => {
            assert_eq!(pairs, 16);
            assert!(fraction > 0.99, "fraction {fraction}");
        }
        other => panic!("expected a compressed-space outcome, got {other:?}"),
    }
    assert!(stored_contracts(&store).is_empty());
}

// ── Run wiring ──────────────────────────────────────────────────────

const QUALIFIED_NAME: &str = "payments::transfer_funds";

fn seed_symbol(store: &SqliteStore) {
    store
        .upsert_symbol(SymbolRecord {
            id: SYMBOL.to_owned(),
            file_path: "src/payments.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: QUALIFIED_NAME.to_owned(),
            signature_fingerprint: "sig-transfer-funds".to_owned(),
            last_seen_at: 1_700_000_000,
        })
        .expect("upsert symbol");
}

fn sir_vector(embedding: Vec<f32>) -> SymbolEmbeddingRecord {
    SymbolEmbeddingRecord {
        symbol_id: SYMBOL.to_owned(),
        sir_hash: "hash-current".to_owned(),
        provider: "test-embedding".to_owned(),
        model: "test-model".to_owned(),
        embedding,
        updated_at: 1_700_000_100,
    }
}

/// Store the symbol's SIR vector the way the SIR pipeline does: through the
/// configured vector store (LanceDB in a default workspace).
fn store_sir_vector(workspace: &Path, embedding: Vec<f32>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let vectors = runtime
        .block_on(open_vector_store(workspace))
        .expect("open vector store");
    runtime
        .block_on(vectors.upsert_embedding(sir_vector(embedding)))
        .expect("upsert SIR vector");
}

#[test]
fn run_reads_the_guard_embedding_from_the_vector_store() {
    let (dir, store) = open_store();
    seed(&store, &happy_history());
    seed_symbol(&store);
    let embedder = happy_embedder();
    let cfg = enabled_config();
    let args = ContractInferArgs {
        symbol: QUALIFIED_NAME.to_owned(),
        dry_run: true,
        eps: None,
    };

    let report = infer_in_workspace(dir.path(), &cfg, &args, || Ok(&embedder)).expect("infer");
    assert_eq!(report.symbol_id, SYMBOL);
    let unguarded = evaluation(&report);
    assert!(!unguarded.guard_active, "no SIR vector is stored yet");
    assert_eq!(
        cluster_for(unguarded, MODEL_B).decision,
        Decision::WouldCreate
    );
    let dim = BASE_DIM + unguarded.unique_texts;

    // A row left in the SQLite table (as after a migration to LanceDB)
    // agrees with the claim, but the vector store holds the current SIR
    // vector, which contradicts it. The guard must use the latter.
    store
        .upsert_symbol_embedding(sir_vector(embedder.padded(MODEL_B, dim)))
        .expect("seed stale SQLite row");
    let mut orthogonal = vec![0.0_f32; dim];
    orthogonal[BASE_DIM - 1] = 1.0;
    store_sir_vector(dir.path(), orthogonal);

    let args = ContractInferArgs {
        dry_run: false,
        ..args
    };
    let report = infer_in_workspace(dir.path(), &cfg, &args, || Ok(&embedder)).expect("infer");
    let guarded = evaluation(&report);
    assert!(guarded.guard_active);
    let decision = &cluster_for(guarded, MODEL_B).decision;
    assert!(
        matches!(decision, Decision::WouldFailVerification { similarity } if similarity.abs() < 1e-9),
        "unexpected decision {decision:?}"
    );
    assert!(stored_contracts(&store).is_empty());
}

// ── Deduplication against existing contracts ────────────────────────

#[test]
fn equivalent_human_contract_suppresses_inference() {
    let (_dir, store) = open_store();
    seed(&store, &happy_history());
    let human_text = "Rejects invalid transfer amounts up front.";
    let human_id = store
        .insert_intent_contract(SYMBOL, "must", human_text, None, "human")
        .expect("insert human contract");
    let before = store
        .get_intent_contract(human_id)
        .expect("get")
        .expect("row");
    let mut embedder = happy_embedder();
    embedder
        .vectors
        .insert(human_text.to_owned(), tilted(0, 3, 0.97));

    let report = infer_for_symbol(&store, &request(false), || Ok(&embedder)).expect("infer");
    let invariant = cluster_for(evaluation(&report), MODEL_B);
    match &invariant.decision {
        Decision::DuplicateOfExisting {
            contract_id,
            created_by,
            active,
            similarity,
        } => {
            assert_eq!(*contract_id, human_id);
            assert_eq!(created_by, "human");
            assert!(*active);
            assert!((similarity - 0.97).abs() < 1e-6, "similarity {similarity}");
        }
        other => panic!("expected a duplicate of the human contract, got {other:?}"),
    }

    assert!(
        embedder.calls.borrow()[0].contains(&human_text.to_owned()),
        "existing clause text must be embedded in the same batch"
    );
    let contracts = stored_contracts(&store);
    assert_eq!(contracts, vec![before]);
}

#[test]
fn inactive_inferred_contract_is_never_resurrected() {
    let (_dir, store) = open_store();
    seed(&store, &happy_history());
    let removed = store
        .insert_intent_contract(SYMBOL, "must", MODEL_B, None, "inferred")
        .expect("insert");
    store.deactivate_contract(removed).expect("deactivate");
    let embedder = happy_embedder();

    let report = infer_for_symbol(&store, &request(false), || Ok(&embedder)).expect("infer");
    assert_eq!(
        cluster_for(evaluation(&report), MODEL_B).decision,
        Decision::DuplicateOfExisting {
            contract_id: removed,
            created_by: "inferred".to_owned(),
            active: false,
            similarity: 1.0,
        }
    );
    let contracts = stored_contracts(&store);
    assert_eq!(contracts.len(), 1);
    assert!(!contracts[0].active);
}

// ── Failure handling ────────────────────────────────────────────────

#[test]
fn embedding_failures_write_nothing() {
    for mode in [Mode::Fail, Mode::WrongCount, Mode::MixedDims, Mode::NaN] {
        let (_dir, store) = open_store();
        seed(&store, &happy_history());
        let embedder = happy_embedder().with_mode(mode);

        let result = infer_for_symbol(&store, &request(false), || Ok(&embedder));
        assert!(result.is_err(), "{mode:?} should fail the run");
        assert!(stored_contracts(&store).is_empty(), "{mode:?} wrote rows");
    }
}

#[test]
fn embedder_construction_failure_writes_nothing() {
    let (_dir, store) = open_store();
    seed(&store, &happy_history());
    let result = infer_for_symbol(&store, &request(false), || -> Result<&MapEmbedder> {
        Err(anyhow!("no provider"))
    });
    assert!(result.is_err());
    assert!(stored_contracts(&store).is_empty());
}

#[test]
fn zero_norm_vector_drops_only_that_claim() {
    let (_dir, store) = open_store();
    seed(&store, &happy_history());
    let mut embedder = happy_embedder();
    embedder.vectors.insert(noise(3), Vec::new());

    let report = infer_for_symbol(&store, &request(false), || Ok(&embedder)).expect("infer");
    let evaluation = evaluation(&report);
    assert_eq!(evaluation.dropped_zero_norm, 1);
    assert!(matches!(
        cluster_for(evaluation, MODEL_B).decision,
        Decision::Created { .. }
    ));
    assert_eq!(stored_contracts(&store).len(), 1);
}

// ── Pure promotion selection ────────────────────────────────────────

fn unit_axis(dim: usize, slot: usize) -> Vec<f64> {
    let mut vector = vec![0.0; dim];
    vector[slot] = 1.0;
    vector
}

fn candidate(text: &str, unit: Vec<f64>, support: usize) -> Candidate {
    Candidate {
        medoid_text: text.to_owned(),
        medoid_version: 10,
        member_units: vec![unit.clone()],
        medoid_embedding: unit.iter().map(|&value| value as f32).collect(),
        medoid_unit: unit,
        coverage: support,
        support,
        cohesion: 0.95,
    }
}

fn existing(id: i64, text: &str, unit: Option<Vec<f64>>, active: bool) -> ExistingClause {
    ExistingClause {
        id,
        created_by: "human".to_owned(),
        active,
        key: comparison_key(text),
        unit,
    }
}

#[test]
fn select_promotions_dedupes_against_existing_text_and_vectors() {
    let settings = InferSettings::from_config(&enabled_config(), None);
    let dim = 8;

    let by_text = select_promotions(
        vec![candidate("Rejects zero amounts.", unit_axis(dim, 0), 10)],
        &[existing(7, "  rejects ZERO amounts ", None, true)],
        &settings,
    );
    assert_eq!(
        by_text[0].decision,
        Decision::DuplicateOfExisting {
            contract_id: 7,
            created_by: "human".to_owned(),
            active: true,
            similarity: 1.0,
        }
    );

    // The existing vector is far from the medoid but close to another member.
    let mut clustered = candidate("Rejects zero amounts.", unit_axis(dim, 0), 10);
    clustered.member_units.push(unit_axis(dim, 1));
    let near_member = {
        let mut vector = unit_axis(dim, 1);
        vector[2] = 0.2;
        let norm = dot(&vector, &vector).sqrt();
        vector.iter().map(|value| value / norm).collect::<Vec<_>>()
    };
    let semantic = select_promotions(
        vec![clustered],
        &[
            existing(3, "Unrelated clause text.", Some(unit_axis(dim, 5)), true),
            existing(4, "Different wording here.", Some(near_member), false),
        ],
        &settings,
    );
    match &semantic[0].decision {
        Decision::DuplicateOfExisting {
            contract_id,
            active,
            similarity,
            ..
        } => {
            assert_eq!(*contract_id, 4);
            assert!(!*active);
            assert!(*similarity > 0.95);
        }
        other => panic!("expected a semantic duplicate, got {other:?}"),
    }
}

#[test]
fn select_promotions_dedupes_within_run_and_applies_cap() {
    let settings = InferSettings::from_config(&enabled_config(), None);
    let dim = 16;

    let near_first = {
        let mut vector = unit_axis(dim, 0);
        vector[1] = 0.1;
        let norm = dot(&vector, &vector).sqrt();
        vector.iter().map(|value| value / norm).collect::<Vec<_>>()
    };
    let in_run = select_promotions(
        vec![
            candidate("Strong claim wins first.", unit_axis(dim, 0), 10),
            candidate("Weaker paraphrase of it.", near_first, 9),
        ],
        &[],
        &settings,
    );
    assert_eq!(in_run[0].medoid_text, "Strong claim wins first.");
    assert_eq!(in_run[0].decision, Decision::WouldCreate);
    assert_eq!(in_run[1].decision, Decision::DuplicateInRun);

    let many: Vec<Candidate> = (0..7)
        .map(|slot| {
            candidate(
                &format!("Distinct invariant number {slot}."),
                unit_axis(dim, slot),
                10 - slot,
            )
        })
        .collect();
    let capped = select_promotions(many, &[], &settings);
    let kept: Vec<&str> = capped
        .iter()
        .filter(|report| report.decision == Decision::WouldCreate)
        .map(|report| report.medoid_text.as_str())
        .collect();
    assert_eq!(kept.len(), 5);
    assert_eq!(kept[0], "Distinct invariant number 0.");
    assert_eq!(
        capped
            .iter()
            .filter(|report| report.decision == Decision::OverCap)
            .count(),
        2
    );
}

#[test]
fn decision_display_includes_tuning_hints() {
    assert_eq!(
        Decision::BelowSupport {
            eps_needed: Some(0.3572)
        }
        .to_string(),
        "below support; would pass at --eps 0.358"
    );
    assert_eq!(
        Decision::BelowSupport {
            eps_needed: Some(0.8)
        }
        .to_string(),
        "below support"
    );
    assert_eq!(
        Decision::DuplicateOfExisting {
            contract_id: 3,
            created_by: "human".to_owned(),
            active: false,
            similarity: 0.934,
        }
        .to_string(),
        "duplicate of #3 (human, inactive, sim 0.93)"
    );
}

// ── Production embedder adapter ─────────────────────────────────────

struct RecordingProvider {
    purposes: Arc<Mutex<Vec<EmbeddingPurpose>>>,
}

#[async_trait::async_trait]
impl EmbeddingProvider for RecordingProvider {
    async fn embed_text(&self, text: &str) -> Result<Vec<f32>, InferError> {
        Ok(vec![text.len() as f32, 1.0])
    }

    async fn embed_text_with_purpose(
        &self,
        text: &str,
        purpose: EmbeddingPurpose,
    ) -> Result<Vec<f32>, InferError> {
        self.purposes.lock().expect("purposes").push(purpose);
        self.embed_text(text).await
    }
}

#[test]
fn provider_claim_embedder_preserves_order_and_uses_document_purpose() {
    let purposes = Arc::new(Mutex::new(Vec::new()));
    let embedder = ProviderClaimEmbedder::new(LoadedEmbeddingProvider {
        provider: Box::new(RecordingProvider {
            purposes: Arc::clone(&purposes),
        }),
        provider_name: "fake".to_owned(),
        model_name: "fake-model".to_owned(),
    })
    .expect("embedder");

    let vectors = embedder
        .embed_claims(&["a".to_owned(), "abc".to_owned(), "ab".to_owned()])
        .expect("embed");
    assert_eq!(
        vectors,
        vec![vec![1.0, 1.0], vec![3.0, 1.0], vec![2.0, 1.0]]
    );
    assert_eq!(
        *purposes.lock().expect("purposes"),
        vec![EmbeddingPurpose::Document; 3]
    );
}
