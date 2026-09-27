//! Reconciliation of stale symbol ids after a full structural snapshot: symbols whose
//! id changed keep their SIR (a migration), stale duplicates are pruned, and the
//! SQLite step runs under the inject lock every symbol-removal path shares.

use super::*;

const RECONCILE_DUPLICATE_STALE_REASON: &str = "duplicate stale symbol matched current snapshot";
const RECONCILE_AMBIGUOUS_CURRENT_REASON: &str =
    "multiple current symbols matched reconciliation tuple";
const RECONCILE_MISSING_CURRENT_REASON: &str = "no current symbol matched reconciliation tuple";

type ReconciliationKey = (String, String, String);

#[derive(Debug, Clone)]
struct PlannedMigration {
    old_symbol: SymbolRecord,
    new_symbol: Symbol,
}

#[derive(Debug, Clone)]
struct PlannedPrune {
    symbol: SymbolRecord,
    reason: &'static str,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ReconciliationPlan {
    migrations: Vec<PlannedMigration>,
    prunes: Vec<PlannedPrune>,
    cleanup_ids: Vec<String>,
}

impl ReconciliationPlan {
    fn is_empty(&self) -> bool {
        self.migrations.is_empty() && self.prunes.is_empty()
    }
}

fn symbol_reconciliation_key(symbol: &Symbol) -> ReconciliationKey {
    (
        symbol.file_path.clone(),
        symbol.qualified_name.clone(),
        symbol.kind.as_str().to_owned(),
    )
}

fn symbol_record_reconciliation_key(symbol: &SymbolRecord) -> ReconciliationKey {
    (
        symbol.file_path.clone(),
        symbol.qualified_name.clone(),
        symbol.kind.clone(),
    )
}

fn stale_symbol_updated_at(store: &SqliteStore, symbol_id: &str) -> Result<i64> {
    Ok(store
        .get_sir_meta(symbol_id)?
        .map(|meta| meta.updated_at.max(0))
        .unwrap_or(0))
}

pub(super) fn plan_symbol_reconciliation(
    store: &SqliteStore,
    symbols_by_id: &HashMap<String, Symbol>,
) -> Result<ReconciliationPlan> {
    let snapshot_ids = symbols_by_id.keys().cloned().collect::<HashSet<_>>();
    let stale_symbols = store
        .list_stale_symbols(&snapshot_ids)
        .context("failed to list stale symbols for reconciliation")?;
    if stale_symbols.is_empty() {
        return Ok(ReconciliationPlan::default());
    }

    let mut current_by_key = BTreeMap::<ReconciliationKey, Vec<Symbol>>::new();
    for symbol in symbols_by_id.values() {
        current_by_key
            .entry(symbol_reconciliation_key(symbol))
            .or_default()
            .push(symbol.clone());
    }

    let mut stale_by_key = BTreeMap::<ReconciliationKey, Vec<SymbolRecord>>::new();
    let mut updated_at_by_id = HashMap::<String, i64>::new();
    for symbol in stale_symbols {
        updated_at_by_id.insert(
            symbol.id.clone(),
            stale_symbol_updated_at(store, symbol.id.as_str())?,
        );
        stale_by_key
            .entry(symbol_record_reconciliation_key(&symbol))
            .or_default()
            .push(symbol);
    }

    let mut plan = ReconciliationPlan::default();
    for (key, mut stale_matches) in stale_by_key {
        let current_matches = current_by_key.get(&key).cloned().unwrap_or_default();
        match current_matches.len() {
            0 => {
                plan.prunes
                    .extend(stale_matches.into_iter().map(|symbol| PlannedPrune {
                        symbol,
                        reason: RECONCILE_MISSING_CURRENT_REASON,
                    }));
            }
            1 => {
                stale_matches.sort_by(|left, right| {
                    updated_at_by_id
                        .get(right.id.as_str())
                        .copied()
                        .unwrap_or(0)
                        .cmp(&updated_at_by_id.get(left.id.as_str()).copied().unwrap_or(0))
                        .then_with(|| right.last_seen_at.cmp(&left.last_seen_at))
                        .then_with(|| left.id.cmp(&right.id))
                });

                let current_symbol = current_matches[0].clone();
                let winner = stale_matches.remove(0);
                if !stale_matches.is_empty() {
                    tracing::warn!(
                        file_path = %winner.file_path,
                        qualified_name = %winner.qualified_name,
                        current_symbol_id = %current_symbol.id,
                        duplicate_count = stale_matches.len(),
                        "reconciliation found duplicate stale symbols for a single current symbol"
                    );
                }
                plan.migrations.push(PlannedMigration {
                    old_symbol: winner,
                    new_symbol: current_symbol,
                });
                plan.prunes
                    .extend(stale_matches.into_iter().map(|symbol| PlannedPrune {
                        symbol,
                        reason: RECONCILE_DUPLICATE_STALE_REASON,
                    }));
            }
            _ => {
                tracing::warn!(
                    file_path = %key.0,
                    qualified_name = %key.1,
                    kind = %key.2,
                    current_match_count = current_matches.len(),
                    stale_match_count = stale_matches.len(),
                    "reconciliation found ambiguous current snapshot matches; pruning stale symbols"
                );
                plan.prunes
                    .extend(stale_matches.into_iter().map(|symbol| PlannedPrune {
                        symbol,
                        reason: RECONCILE_AMBIGUOUS_CURRENT_REASON,
                    }));
            }
        }
    }

    plan.migrations.sort_by(|left, right| {
        left.old_symbol
            .file_path
            .cmp(&right.old_symbol.file_path)
            .then_with(|| {
                left.old_symbol
                    .qualified_name
                    .cmp(&right.old_symbol.qualified_name)
            })
            .then_with(|| left.old_symbol.id.cmp(&right.old_symbol.id))
    });
    plan.prunes.sort_by(|left, right| {
        left.symbol
            .file_path
            .cmp(&right.symbol.file_path)
            .then_with(|| left.symbol.qualified_name.cmp(&right.symbol.qualified_name))
            .then_with(|| left.symbol.id.cmp(&right.symbol.id))
    });

    let mut cleanup_ids = plan
        .migrations
        .iter()
        .map(|entry| entry.old_symbol.id.clone())
        .chain(plan.prunes.iter().map(|entry| entry.symbol.id.clone()))
        .collect::<Vec<_>>();
    cleanup_ids.sort();
    cleanup_ids.dedup();
    plan.cleanup_ids = cleanup_ids;

    Ok(plan)
}

pub(super) fn print_reconciliation_dry_run(
    plan: &ReconciliationPlan,
    out: &mut dyn Write,
) -> Result<()> {
    if plan.is_empty() {
        writeln!(out, "DRY_RUN reconciliation: no stale symbols found")
            .context("failed to write dry-run reconciliation summary")?;
        return Ok(());
    }

    for migration in &plan.migrations {
        writeln!(
            out,
            "DRY_RUN migrate file={} symbol={} old_id={} new_id={}",
            migration.old_symbol.file_path,
            migration.old_symbol.qualified_name,
            migration.old_symbol.id,
            migration.new_symbol.id
        )
        .context("failed to write dry-run migration line")?;
    }

    for prune in &plan.prunes {
        writeln!(
            out,
            "DRY_RUN prune file={} symbol={} old_id={} reason={}",
            prune.symbol.file_path, prune.symbol.qualified_name, prune.symbol.id, prune.reason
        )
        .context("failed to write dry-run prune line")?;
    }

    writeln!(
        out,
        "DRY_RUN reconciliation summary: {} migrations, {} prunes",
        plan.migrations.len(),
        plan.prunes.len()
    )
    .context("failed to write dry-run reconciliation summary")?;
    Ok(())
}

pub(super) fn execute_symbol_reconciliation<GraphCleanup, VectorCleanup>(
    workspace: &Path,
    store: &SqliteStore,
    plan: &ReconciliationPlan,
    graph_cleanup: GraphCleanup,
    vector_cleanup: VectorCleanup,
) -> Result<(usize, usize)>
where
    GraphCleanup: FnOnce(&[String]) -> Result<()>,
    VectorCleanup: FnOnce(&[String]) -> Result<()>,
{
    let migrations = plan
        .migrations
        .iter()
        .map(|entry| (entry.old_symbol.id.clone(), entry.new_symbol.id.clone()))
        .collect::<Vec<_>>();
    let prunes = plan
        .prunes
        .iter()
        .map(|entry| entry.symbol.id.clone())
        .collect::<Vec<_>>();

    // Reconciliation migrates and deletes symbol rows with their SIRs. Like the other
    // symbol-removal paths it runs under the inject lock: an injection that found the
    // symbol under that lock must not have it pruned before its leaf write lands, or
    // the write would recreate an orphan SIR row for an id the index no longer holds.
    // The lock covers only the SQLite reconciliation; the graph and vector cleanup
    // below runs after it is released, so a slow backend does not hold every
    // injection and generated-SIR persist in the workspace for the cleanup's duration.
    let (migrated, pruned) = {
        let _inject_guard = acquire_inject_write_lock(workspace)?;
        store
            .reconcile_and_prune(&migrations, &prunes)
            .context("failed to reconcile stale symbols")?
    };

    for migration in &plan.migrations {
        tracing::info!(
            qualified_name = %migration.old_symbol.qualified_name,
            file_path = %migration.old_symbol.file_path,
            old_id = %migration.old_symbol.id,
            new_id = %migration.new_symbol.id,
            "reconciled stale symbol ID"
        );
    }

    if !plan.cleanup_ids.is_empty() {
        graph_cleanup(&plan.cleanup_ids).context("failed to clean stale graph symbols")?;
        if let Err(err) = vector_cleanup(&plan.cleanup_ids) {
            tracing::warn!(
                error = %err,
                cleanup_count = plan.cleanup_ids.len(),
                "vector cleanup failed during reconciliation; embeddings will regenerate on the next pass"
            );
        }
    }

    tracing::info!(
        migrated,
        pruned,
        "Reconciliation complete: {migrated} migrated, {pruned} pruned"
    );
    Ok((migrated, pruned))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tempfile::tempdir;

    use super::super::tests::{test_symbol, write_default_config};
    use super::*;

    fn upsert_symbol_snapshot(store: &SqliteStore, symbol: &Symbol, last_seen_at: i64) {
        store
            .upsert_symbol(to_symbol_record(symbol, last_seen_at))
            .expect("upsert symbol snapshot");
    }

    fn upsert_sir_state(
        store: &SqliteStore,
        symbol_id: &str,
        sir_hash: &str,
        sir_json: &str,
        updated_at: i64,
    ) {
        let version = store
            .record_sir_version_if_changed(
                symbol_id,
                sir_hash,
                "mock",
                "mock-model",
                sir_json,
                updated_at,
                None,
            )
            .expect("record SIR history");
        store
            .write_sir_blob(symbol_id, sir_json)
            .expect("write SIR blob");
        store
            .upsert_sir_meta(aether_store::SirMetaRecord {
                id: symbol_id.to_owned(),
                sir_hash: sir_hash.to_owned(),
                sir_version: version.version,
                provider: "mock".to_owned(),
                model: "mock-model".to_owned(),
                generation_pass: "scan".to_owned(),
                reasoning_trace: None,
                prompt_hash: None,
                staleness_score: None,
                updated_at: version.updated_at,
                sir_status: "fresh".to_owned(),
                last_error: None,
                last_attempt_at: version.updated_at,
            })
            .expect("upsert SIR metadata");
    }

    #[test]
    fn reconcile_picks_most_recent_on_ambiguity() {
        let temp = tempdir().expect("tempdir");
        write_default_config(temp.path());
        let store = SqliteStore::open(temp.path()).expect("open store");

        let old_a = test_symbol("sym-old-a", "sig-old-a");
        let old_b = test_symbol("sym-old-b", "sig-old-b");
        let new_symbol = test_symbol("sym-new", "sig-new");

        upsert_symbol_snapshot(&store, &old_a, 10);
        upsert_symbol_snapshot(&store, &old_b, 20);
        upsert_symbol_snapshot(&store, &new_symbol, 30);
        upsert_sir_state(
            &store,
            old_a.id.as_str(),
            "hash-old-a",
            r#"{"intent":"old-a"}"#,
            100,
        );
        upsert_sir_state(
            &store,
            old_b.id.as_str(),
            "hash-old-b",
            r#"{"intent":"old-b"}"#,
            200,
        );

        let symbols_by_id = HashMap::from_iter([(new_symbol.id.clone(), new_symbol.clone())]);
        let plan = plan_symbol_reconciliation(&store, &symbols_by_id).expect("plan reconcile");

        assert_eq!(plan.migrations.len(), 1);
        assert_eq!(plan.migrations[0].old_symbol.id, old_b.id);
        assert_eq!(plan.migrations[0].new_symbol.id, new_symbol.id);
        assert_eq!(plan.prunes.len(), 1);
        assert_eq!(plan.prunes[0].symbol.id, old_a.id);
        assert_eq!(plan.prunes[0].reason, RECONCILE_DUPLICATE_STALE_REASON);
    }

    #[test]
    fn dry_run_reports_without_mutating() {
        let temp = tempdir().expect("tempdir");
        write_default_config(temp.path());
        let store = SqliteStore::open(temp.path()).expect("open store");

        let old_symbol = test_symbol("sym-old", "sig-old");
        let new_symbol = test_symbol("sym-new", "sig-new");
        upsert_symbol_snapshot(&store, &old_symbol, 10);
        upsert_sir_state(
            &store,
            old_symbol.id.as_str(),
            "hash-old",
            r#"{"intent":"old"}"#,
            100,
        );

        let before = store
            .count_symbols_with_sir()
            .expect("count symbols before dry run");
        let symbols_by_id = HashMap::from_iter([(new_symbol.id.clone(), new_symbol.clone())]);
        let plan = plan_symbol_reconciliation(&store, &symbols_by_id).expect("plan reconcile");

        let mut output = Vec::<u8>::new();
        print_reconciliation_dry_run(&plan, &mut output).expect("print dry run report");
        let rendered = String::from_utf8(output).expect("utf8 output");
        assert!(rendered.contains("DRY_RUN migrate"));

        let after = store
            .count_symbols_with_sir()
            .expect("count symbols after dry run");
        assert_eq!(before, after);
        assert!(
            store
                .get_symbol_record(old_symbol.id.as_str())
                .expect("query old symbol")
                .is_some()
        );
        assert!(
            store
                .get_symbol_record(new_symbol.id.as_str())
                .expect("query new symbol")
                .is_none()
        );
    }

    #[test]
    fn coverage_reaches_100_after_reconcile() {
        let temp = tempdir().expect("tempdir");
        write_default_config(temp.path());
        let store = SqliteStore::open(temp.path()).expect("open store");

        let old_symbol = test_symbol("sym-old", "sig-old");
        let new_symbol = test_symbol("sym-new", "sig-new");
        upsert_symbol_snapshot(&store, &old_symbol, 10);
        upsert_symbol_snapshot(&store, &new_symbol, 20);
        upsert_sir_state(
            &store,
            old_symbol.id.as_str(),
            "hash-old",
            r#"{"intent":"old"}"#,
            100,
        );

        let symbols_by_id = HashMap::from_iter([(new_symbol.id.clone(), new_symbol.clone())]);
        let plan = plan_symbol_reconciliation(&store, &symbols_by_id).expect("plan reconcile");
        let (migrated, pruned) =
            execute_symbol_reconciliation(temp.path(), &store, &plan, |_| Ok(()), |_| Ok(()))
                .expect("execute reconcile");
        assert_eq!((migrated, pruned), (1, 0));

        let (total_symbols, symbols_with_sir) = store
            .count_symbols_with_sir()
            .expect("count coverage after reconcile");
        assert_eq!(total_symbols, 1);
        assert_eq!(symbols_with_sir, 1);
    }

    #[test]
    fn full_reconcile_is_idempotent() {
        let temp = tempdir().expect("tempdir");
        write_default_config(temp.path());
        let store = SqliteStore::open(temp.path()).expect("open store");

        let old_symbol = test_symbol("sym-old", "sig-old");
        let new_symbol = test_symbol("sym-new", "sig-new");
        upsert_symbol_snapshot(&store, &old_symbol, 10);
        upsert_symbol_snapshot(&store, &new_symbol, 20);
        upsert_sir_state(
            &store,
            old_symbol.id.as_str(),
            "hash-old",
            r#"{"intent":"old"}"#,
            100,
        );

        let symbols_by_id = HashMap::from_iter([(new_symbol.id.clone(), new_symbol.clone())]);
        let first_plan = plan_symbol_reconciliation(&store, &symbols_by_id).expect("first plan");
        execute_symbol_reconciliation(temp.path(), &store, &first_plan, |_| Ok(()), |_| Ok(()))
            .expect("first reconcile");

        let second_plan = plan_symbol_reconciliation(&store, &symbols_by_id).expect("second plan");
        assert!(second_plan.is_empty());
        let (migrated, pruned) = execute_symbol_reconciliation(
            temp.path(),
            &store,
            &second_plan,
            |_| Ok(()),
            |_| Ok(()),
        )
        .expect("second reconcile");
        assert_eq!((migrated, pruned), (0, 0));

        let (total_symbols, symbols_with_sir) = store
            .count_symbols_with_sir()
            .expect("count coverage after second reconcile");
        assert_eq!((total_symbols, symbols_with_sir), (1, 1));
    }
}
