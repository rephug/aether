/// Intent Contracts — semantic type checking for code behavior.
///
/// Developers or agents declare behavioral expectations on symbols
/// (must/must_not/preserves clauses), which are verified on every
/// SIR regeneration using a two-stage cascade: embedding cosine
/// pre-filter followed by LLM judge for ambiguous cases.
mod judge;
mod verify;

use std::path::Path;

use aether_config::AetherConfig;
use aether_infer::{EmbeddingProviderOverrides, load_embedding_provider_from_config};
use aether_store::{SirStateStore, SqliteStore};
use anyhow::{Context, Result, anyhow};

pub use verify::{ClauseResult, ClauseStatus, ContractVerifier, VerificationResult};

use crate::cli::{ContractAddArgs, ContractArgs, ContractCheckArgs, ContractCommand};

/// CLI dispatch for `aetherd contract` subcommands.
pub fn run_contract_command(
    workspace: &Path,
    config: &AetherConfig,
    args: ContractArgs,
) -> Result<()> {
    match args.command {
        ContractCommand::Add(add_args) => run_add(workspace, config, &add_args),
        ContractCommand::List(list_args) => run_list(workspace, list_args.symbol.as_deref()),
        ContractCommand::Remove(remove_args) => run_remove(workspace, remove_args.contract_id),
        ContractCommand::Check(check_args) => run_check(workspace, config, &check_args),
    }
}

/// Verify all active contracts for a symbol after SIR persist + embedding refresh.
///
/// This is the post-persist hook entry point. Returns the verification result.
/// All side effects (streak updates, violation inserts) are persisted to the store.
pub fn verify_symbol_contracts(
    store: &SqliteStore,
    symbol_id: &str,
    sir_json: &str,
    sir_embedding: Option<&[f32]>,
    config: &AetherConfig,
    workspace_root: &Path,
) -> Result<VerificationResult> {
    let contracts_config = match config.contracts.as_ref() {
        Some(c) if c.enabled => c,
        _ => {
            return Ok(VerificationResult {
                symbol_id: symbol_id.to_owned(),
                clauses_checked: 0,
                passed: 0,
                failed: 0,
                ambiguous: 0,
                clause_results: Vec::new(),
            });
        }
    };

    let active_contracts = store
        .list_active_contracts_for_symbol(symbol_id)
        .with_context(|| format!("failed to load contracts for {symbol_id}"))?;

    if active_contracts.is_empty() {
        return Ok(VerificationResult {
            symbol_id: symbol_id.to_owned(),
            clauses_checked: 0,
            passed: 0,
            failed: 0,
            ambiguous: 0,
            clause_results: Vec::new(),
        });
    }

    let verifier = ContractVerifier::from_config(contracts_config);
    let sir_version = store
        .get_sir_meta(symbol_id)
        .ok()
        .flatten()
        .map(|m| m.sir_version)
        .unwrap_or(0);

    let mut clause_results = Vec::new();
    let mut passed = 0_usize;
    let mut failed = 0_usize;
    let mut ambiguous = 0_usize;

    for contract in &active_contracts {
        let clause_embedding = contract
            .clause_embedding_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<Vec<f32>>(json).ok());

        // Stage 1: Embedding cosine pre-filter
        let (mut status, similarity) = match (clause_embedding.as_deref(), sir_embedding) {
            (Some(clause_emb), Some(sir_emb)) => {
                verifier.classify_by_embedding(clause_emb, sir_emb)
            }
            _ => (ClauseStatus::Ambiguous, 0.0),
        };

        let mut judge_reason = None;

        // Stage 2: LLM judge for ambiguous cases
        if status == ClauseStatus::Ambiguous {
            let resolved = verifier.resolve_ambiguous_with_judge(
                &contract.clause_text,
                &contract.clause_type,
                sir_json,
                workspace_root,
            );
            if let ClauseStatus::Fail = &resolved {
                judge_reason = Some("LLM judge determined violation".to_owned());
            }
            // Only update status if the judge gave a definitive answer
            if resolved != ClauseStatus::Ambiguous {
                status = resolved;
            }
        }

        // Leaky bucket: update streaks and record violations
        apply_leaky_bucket(
            store,
            contract.id,
            symbol_id,
            sir_version,
            &status,
            contract.violation_streak,
            contracts_config.streak_threshold,
            &contract.clause_text,
            judge_reason.as_deref(),
        )?;

        match status {
            ClauseStatus::Pass => passed += 1,
            ClauseStatus::Fail => failed += 1,
            ClauseStatus::Ambiguous => ambiguous += 1,
        }

        clause_results.push(ClauseResult {
            contract_id: contract.id,
            clause_text: contract.clause_text.clone(),
            clause_type: contract.clause_type.clone(),
            status,
            similarity: Some(similarity),
            judge_reason,
        });
    }

    Ok(VerificationResult {
        symbol_id: symbol_id.to_owned(),
        clauses_checked: clause_results.len(),
        passed,
        failed,
        ambiguous,
        clause_results,
    })
}

/// Apply leaky bucket streak logic and persist results.
#[allow(clippy::too_many_arguments)]
fn apply_leaky_bucket(
    store: &SqliteStore,
    contract_id: i64,
    symbol_id: &str,
    sir_version: i64,
    status: &ClauseStatus,
    current_streak: i64,
    streak_threshold: u32,
    clause_text: &str,
    judge_reason: Option<&str>,
) -> Result<()> {
    match status {
        ClauseStatus::Pass => {
            if current_streak > 0 {
                store
                    .reset_contract_streak(contract_id)
                    .context("failed to reset contract streak")?;
            }
        }
        ClauseStatus::Fail => {
            let new_streak = current_streak + 1;
            store
                .update_contract_streak(contract_id, new_streak)
                .context("failed to update contract streak")?;

            if new_streak >= i64::from(streak_threshold) {
                let violation_type = if judge_reason.is_some() {
                    "llm_judge_fail"
                } else {
                    "embedding_fail"
                };
                store
                    .insert_intent_violation(
                        contract_id,
                        symbol_id,
                        sir_version,
                        violation_type,
                        None,
                        judge_reason,
                    )
                    .context("failed to insert intent violation")?;
                tracing::warn!(
                    symbol_id = symbol_id,
                    contract_id = contract_id,
                    clause = clause_text,
                    streak = new_streak,
                    "Intent contract violation detected"
                );
            } else {
                tracing::debug!(
                    symbol_id = symbol_id,
                    contract_id = contract_id,
                    streak = new_streak,
                    threshold = streak_threshold,
                    "Contract fail streak incremented (below threshold)"
                );
            }
        }
        ClauseStatus::Ambiguous => {
            // Don't change streak for unresolved ambiguous
            tracing::debug!(
                symbol_id = symbol_id,
                contract_id = contract_id,
                "Contract verification ambiguous (judge unavailable)"
            );
        }
    }
    Ok(())
}

// ── CLI command implementations ──────────────────────────────────

fn run_add(workspace: &Path, config: &AetherConfig, args: &ContractAddArgs) -> Result<()> {
    let store = SqliteStore::open(workspace).context("failed to open store")?;
    let symbol_id = resolve_symbol_id(&store, &args.symbol)?;

    // Load embedding provider for clause embedding
    let embedding_provider =
        load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default())
            .context("failed to load embedding provider")?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to create embedding runtime")?;

    let mut inserted = Vec::new();

    for clause_text in &args.must_clauses {
        let id = insert_clause_with_embedding(
            &store,
            &symbol_id,
            "must",
            clause_text,
            "human",
            &embedding_provider,
            &runtime,
        )?;
        inserted.push(("must", clause_text.as_str(), id));
    }
    for clause_text in &args.must_not_clauses {
        let id = insert_clause_with_embedding(
            &store,
            &symbol_id,
            "must_not",
            clause_text,
            "human",
            &embedding_provider,
            &runtime,
        )?;
        inserted.push(("must_not", clause_text.as_str(), id));
    }
    for clause_text in &args.preserves_clauses {
        let id = insert_clause_with_embedding(
            &store,
            &symbol_id,
            "preserves",
            clause_text,
            "human",
            &embedding_provider,
            &runtime,
        )?;
        inserted.push(("preserves", clause_text.as_str(), id));
    }

    if inserted.is_empty() {
        println!("No clauses specified. Use --must, --must-not, or --preserves.");
        return Ok(());
    }

    println!(
        "Added {} contract clause(s) to symbol {symbol_id}:",
        inserted.len()
    );
    for (clause_type, text, id) in &inserted {
        let embedded = if embedding_provider.is_some() {
            " [embedded]"
        } else {
            ""
        };
        println!("  #{id} {clause_type}: \"{text}\"{embedded}");
    }
    let _ = config; // available for future judge config
    Ok(())
}

fn insert_clause_with_embedding(
    store: &SqliteStore,
    symbol_id: &str,
    clause_type: &str,
    clause_text: &str,
    created_by: &str,
    embedding_provider: &Option<aether_infer::LoadedEmbeddingProvider>,
    runtime: &tokio::runtime::Runtime,
) -> Result<i64> {
    let embedding_json = if let Some(provider) = embedding_provider {
        match runtime.block_on(provider.provider.embed_text(clause_text)) {
            Ok(embedding) => Some(serde_json::to_string(&embedding)?),
            Err(err) => {
                tracing::warn!(error = %err, "Failed to embed contract clause, storing without embedding");
                None
            }
        }
    } else {
        None
    };

    store
        .insert_intent_contract(
            symbol_id,
            clause_type,
            clause_text,
            embedding_json.as_deref(),
            created_by,
        )
        .with_context(|| format!("failed to insert {clause_type} contract"))
}

fn run_list(workspace: &Path, symbol: Option<&str>) -> Result<()> {
    let store = SqliteStore::open(workspace).context("failed to open store")?;

    let contracts = if let Some(symbol) = symbol {
        let symbol_id = resolve_symbol_id(&store, symbol)?;
        store
            .list_active_contracts_for_symbol(&symbol_id)
            .context("failed to list contracts")?
    } else {
        store
            .list_all_active_contracts()
            .context("failed to list contracts")?
    };

    if contracts.is_empty() {
        println!("No active contracts found.");
        return Ok(());
    }

    let mut current_symbol = String::new();
    for contract in &contracts {
        if contract.symbol_id != current_symbol {
            if !current_symbol.is_empty() {
                println!();
            }
            current_symbol.clone_from(&contract.symbol_id);
            println!("Symbol: {current_symbol}");
        }
        let streak_indicator = if contract.violation_streak > 0 {
            format!(" [streak: {}]", contract.violation_streak)
        } else {
            String::new()
        };
        let embedded = if contract.clause_embedding_json.is_some() {
            " [embedded]"
        } else {
            ""
        };
        println!(
            "  #{} {}: \"{}\"{embedded}{streak_indicator}",
            contract.id, contract.clause_type, contract.clause_text
        );
    }
    println!("\n{} active contract(s) total.", contracts.len());
    Ok(())
}

fn run_remove(workspace: &Path, contract_id: i64) -> Result<()> {
    let store = SqliteStore::open(workspace).context("failed to open store")?;

    let contract = store
        .get_intent_contract(contract_id)
        .context("failed to look up contract")?
        .ok_or_else(|| anyhow!("contract #{contract_id} not found"))?;

    if !contract.active {
        println!("Contract #{contract_id} is already inactive.");
        return Ok(());
    }

    store
        .deactivate_contract(contract_id)
        .context("failed to deactivate contract")?;
    println!(
        "Deactivated contract #{contract_id}: {} \"{}\"",
        contract.clause_type, contract.clause_text
    );
    Ok(())
}

fn run_check(workspace: &Path, config: &AetherConfig, args: &ContractCheckArgs) -> Result<()> {
    let store = SqliteStore::open(workspace).context("failed to open store")?;

    let contracts = if let Some(symbol) = args.symbol.as_deref() {
        let symbol_id = resolve_symbol_id(&store, symbol)?;
        store
            .list_active_contracts_for_symbol(&symbol_id)
            .context("failed to list contracts")?
    } else {
        store
            .list_all_active_contracts()
            .context("failed to list contracts")?
    };

    if contracts.is_empty() {
        println!("No active contracts to check.");
        return Ok(());
    }

    // Collect unique symbol IDs
    let mut symbol_ids: Vec<String> = contracts.iter().map(|c| c.symbol_id.clone()).collect();
    symbol_ids.sort();
    symbol_ids.dedup();

    let mut total_checked = 0_usize;
    let mut total_passed = 0_usize;
    let mut total_failed = 0_usize;
    let mut total_ambiguous = 0_usize;

    for symbol_id in &symbol_ids {
        // Load SIR JSON
        let sir_json = store
            .read_sir_blob(symbol_id)
            .with_context(|| format!("failed to read SIR for {symbol_id}"))?
            .unwrap_or_default();

        // Load SIR embedding
        let embedding: Option<Vec<f32>> = load_sir_embedding(&store, symbol_id, config)?;

        let result = verify_symbol_contracts(
            &store,
            symbol_id,
            &sir_json,
            embedding.as_deref(),
            config,
            workspace,
        )?;

        if result.clauses_checked > 0 {
            println!("Symbol: {symbol_id}");
            for clause in &result.clause_results {
                let status_str = match clause.status {
                    ClauseStatus::Pass => "PASS",
                    ClauseStatus::Fail => "FAIL",
                    ClauseStatus::Ambiguous => "AMBIGUOUS",
                };
                let sim_str = clause
                    .similarity
                    .map(|s| format!(" (sim: {s:.3})"))
                    .unwrap_or_default();
                println!(
                    "  [{status_str}] {} \"{}\"{sim_str}",
                    clause.clause_type, clause.clause_text
                );
            }
            println!();
        }

        total_checked += result.clauses_checked;
        total_passed += result.passed;
        total_failed += result.failed;
        total_ambiguous += result.ambiguous;
    }

    println!(
        "Checked {total_checked} clause(s): {total_passed} passed, \
         {total_failed} failed, {total_ambiguous} ambiguous"
    );
    Ok(())
}

// ── Helpers ──────────────────────────────────

/// Resolve a symbol by qualified name or direct ID.
fn resolve_symbol_id(store: &SqliteStore, selector: &str) -> Result<String> {
    // Try direct ID first
    if let Ok(Some(_)) = store.get_symbol_record(selector) {
        return Ok(selector.to_owned());
    }
    // Try qualified name lookup
    if let Ok(Some(record)) = store.get_symbol_by_qualified_name(selector) {
        return Ok(record.id);
    }
    Err(anyhow!(
        "symbol '{selector}' not found (tried as ID and qualified name)"
    ))
}

/// Load the SIR embedding for a symbol from the store's embedding table.
fn load_sir_embedding(
    store: &SqliteStore,
    symbol_id: &str,
    config: &AetherConfig,
) -> Result<Option<Vec<f32>>> {
    let provider = config.embeddings.provider.as_str();
    let model = config
        .embeddings
        .model
        .as_deref()
        .unwrap_or("gemini-embedding-2-preview");
    let records = store
        .list_symbol_embeddings_for_ids(provider, model, &[symbol_id.to_owned()])
        .context("failed to load symbol embeddings")?;
    Ok(records.into_iter().next().map(|r| r.embedding))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn open_test_store() -> (SqliteStore, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let store = SqliteStore::open(dir.path()).unwrap();
        (store, dir)
    }

    #[test]
    fn first_fail_increments_streak_no_violation() {
        let (store, _dir) = open_test_store();
        let cid = store
            .insert_intent_contract("sym_a", "must", "clause", None, "human")
            .unwrap();

        apply_leaky_bucket(
            &store,
            cid,
            "sym_a",
            1,
            &ClauseStatus::Fail,
            0,
            2,
            "clause",
            None,
        )
        .unwrap();

        let contract = store.get_intent_contract(cid).unwrap().unwrap();
        assert_eq!(contract.violation_streak, 1);

        // No violation should be recorded
        let violations = store.list_violations_for_contract(cid, 10).unwrap();
        assert!(violations.is_empty());
    }

    #[test]
    fn second_fail_creates_violation_record() {
        let (store, _dir) = open_test_store();
        let cid = store
            .insert_intent_contract("sym_a", "must", "clause", None, "human")
            .unwrap();

        // First fail: streak → 1
        apply_leaky_bucket(
            &store,
            cid,
            "sym_a",
            1,
            &ClauseStatus::Fail,
            0,
            2,
            "clause",
            None,
        )
        .unwrap();
        // Second fail: streak → 2, triggers violation
        apply_leaky_bucket(
            &store,
            cid,
            "sym_a",
            2,
            &ClauseStatus::Fail,
            1,
            2,
            "clause",
            None,
        )
        .unwrap();

        let contract = store.get_intent_contract(cid).unwrap().unwrap();
        assert_eq!(contract.violation_streak, 2);

        let violations = store.list_violations_for_contract(cid, 10).unwrap();
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].violation_type, "embedding_fail");
    }

    #[test]
    fn pass_after_fail_resets_streak() {
        let (store, _dir) = open_test_store();
        let cid = store
            .insert_intent_contract("sym_a", "must", "clause", None, "human")
            .unwrap();

        // Fail once
        apply_leaky_bucket(
            &store,
            cid,
            "sym_a",
            1,
            &ClauseStatus::Fail,
            0,
            2,
            "clause",
            None,
        )
        .unwrap();
        assert_eq!(
            store
                .get_intent_contract(cid)
                .unwrap()
                .unwrap()
                .violation_streak,
            1
        );

        // Pass resets
        apply_leaky_bucket(
            &store,
            cid,
            "sym_a",
            2,
            &ClauseStatus::Pass,
            1,
            2,
            "clause",
            None,
        )
        .unwrap();
        assert_eq!(
            store
                .get_intent_contract(cid)
                .unwrap()
                .unwrap()
                .violation_streak,
            0
        );
    }

    #[test]
    fn ambiguous_does_not_change_streak() {
        let (store, _dir) = open_test_store();
        let cid = store
            .insert_intent_contract("sym_a", "must", "clause", None, "human")
            .unwrap();

        // Set streak to 1
        store.update_contract_streak(cid, 1).unwrap();

        apply_leaky_bucket(
            &store,
            cid,
            "sym_a",
            1,
            &ClauseStatus::Ambiguous,
            1,
            2,
            "clause",
            None,
        )
        .unwrap();

        // Streak unchanged
        assert_eq!(
            store
                .get_intent_contract(cid)
                .unwrap()
                .unwrap()
                .violation_streak,
            1
        );
    }
}
