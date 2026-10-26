//! SQLite-only process-death experiment using the real lifecycle API.
//!
//! No model, graph backend, vector backend or active workspace is opened. This
//! checks SQLite transaction recovery, not a cross-store transaction or outbox.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use aether_config::{AetherConfig, GraphBackend, save_workspace_config};
use aether_store::{
    SirFingerprintHistoryRecord, SirHistoryStore, SirMetaRecord, SirStateStore, SqliteStore,
    SymbolCatalogStore, SymbolHistoryStore, SymbolRecord, TestIntentRecord, TestIntentStore,
};
use rusqlite::{Connection, ErrorCode};
use tempfile::tempdir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const CHILD_ENV: &str = "AETHER_RETIREMENT_CRASH_FIXTURE";
const FORCED_DEATHS: usize = 50;
const SYMBOLS: usize = 12;
const HISTORY_VERSIONS: usize = 2;
const PAYLOAD_BYTES: usize = 8192;
const TRIGGER_ITERATIONS: usize = 100_000;
const SEED: u64 = 0xAE7E_2026_1021_0050;

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn symbol_id(index: usize) -> String {
    format!("crash-symbol-{index:02}")
}

fn seed_fixture(workspace: &Path) -> TestResult {
    let mut config = AetherConfig::default();
    config.storage.graph_backend = GraphBackend::Sqlite;
    // Mirrored files have a separate post-commit lifecycle. Exclude that separate
    // durability contract from this explicitly SQLite-only experiment.
    config.storage.mirror_sir_files = false;
    config.embeddings.enabled = false;
    save_workspace_config(workspace, &config)?;
    std::fs::write(
        workspace.join(".retirement-crash-fixture"),
        "isolated SQLite crash fixture\n",
    )?;
    let store = SqliteStore::open(workspace)?;
    for index in 0..SYMBOLS {
        let id = symbol_id(index);
        let file_path = format!("src/{id}.rs");
        store.upsert_symbol(SymbolRecord {
            id: id.clone(),
            file_path: file_path.clone(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: format!("fixture::{id}"),
            signature_fingerprint: format!("signature-{index}"),
            last_seen_at: 1,
        })?;
        for version in 1..=HISTORY_VERSIONS {
            let json = serde_json::json!({
                "intent": format!("{id} recorded version {version}"),
                "payload": "x".repeat(PAYLOAD_BYTES),
            });
            store.persist_sir_state_atomically_with_source(
                SirMetaRecord {
                    id: id.clone(),
                    sir_hash: format!("hash-{id}-{version}"),
                    sir_version: version as i64,
                    provider: "saved-fixture".to_owned(),
                    model: "no-model-call".to_owned(),
                    generation_pass: "scan".to_owned(),
                    reasoning_trace: None,
                    prompt_hash: None,
                    staleness_score: None,
                    updated_at: version as i64,
                    sir_status: "fresh".to_owned(),
                    last_error: None,
                    last_attempt_at: version as i64,
                },
                &json.to_string(),
                Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                None,
                Some(&format!("source-{id}")),
            )?;
        }
        store.insert_sir_fingerprint_history(&SirFingerprintHistoryRecord {
            symbol_id: id.clone(),
            timestamp: 2,
            prompt_hash: format!("prompt-{id}"),
            prompt_hash_previous: None,
            trigger: "crash-fixture".to_owned(),
            source_changed: true,
            neighbor_changed: false,
            config_changed: false,
            generation_model: Some("no-model-call".to_owned()),
            generation_pass: Some("scan".to_owned()),
            delta_sem: None,
            sir_write_generation: None,
        })?;
        let contract_id = store.insert_intent_contract(
            &id,
            "must",
            "Preserve the recorded decision",
            None,
            "crash-fixture",
        )?;
        store.insert_intent_violation(
            contract_id,
            &id,
            2,
            "saved-observation",
            Some(0.5),
            Some("saved reason"),
        )?;
        store.replace_test_intents_for_file(
            &file_path,
            &[TestIntentRecord {
                intent_id: format!("test-{id}"),
                file_path: file_path.clone(),
                test_name: format!("test_{index}"),
                intent_text: "Preserves historical evidence".to_owned(),
                group_label: None,
                language: "rust".to_owned(),
                symbol_id: Some(id),
                created_at: 1,
                updated_at: 2,
            }],
        )?;
    }
    drop(store);
    let conn = Connection::open(workspace.join(".aether/meta.sqlite"))?;
    // Fixture-only work inside the real delete transaction widens the kill window.
    // It never implements or replaces retirement: archive/deletion still run only
    // through SqliteStore::mark_removed. Its row must commit/roll back with them.
    conn.execute_batch(&format!(
        "CREATE TABLE crash_probe (symbol_id TEXT PRIMARY KEY, work_sum INTEGER NOT NULL);
         CREATE TRIGGER slow_fixture_delete BEFORE DELETE ON symbols BEGIN
           INSERT INTO crash_probe(symbol_id, work_sum)
           WITH RECURSIVE work(n) AS (SELECT 0 UNION ALL SELECT n + 1 FROM work WHERE n < {TRIGGER_ITERATIONS})
           SELECT OLD.id, SUM(n) FROM work;
         END;"
    ))?;
    Ok(())
}

/// Invoked separately by the parent; a normal test run performs no child work.
#[test]
fn retirement_crash_child() -> TestResult {
    let Some(workspace) = std::env::var_os(CHILD_ENV) else {
        return Ok(());
    };
    if std::fs::read_to_string(Path::new(&workspace).join(".retirement-crash-fixture"))?
        != "isolated SQLite crash fixture\n"
    {
        return Err("refusing non-fixture crash-test workspace".into());
    }
    let store = SqliteStore::open(Path::new(&workspace))?;
    // Establish committed work before the parent interrupts the next transaction.
    store.mark_removed(&symbol_id(0))?;
    println!("AETHER_RETIREMENT_READY");
    std::io::stdout().flush()?;
    let mut start = String::new();
    std::io::stdin().read_line(&mut start)?;
    if start.trim() != "start" {
        return Err("invalid crash-test start handshake".into());
    }
    for index in 1..SYMBOLS {
        store.mark_removed(&symbol_id(index))?;
    }
    // Never exit successfully before the parent records an actual forced death.
    loop {
        std::thread::park();
    }
}

fn wait_for_ready(child: &mut Child) -> TestResult {
    let stdout = child.stdout.take().ok_or("missing child stdout")?;
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            for line in BufReader::new(stdout).lines() {
                if line
                    .map_err(|error| error.to_string())?
                    .contains("AETHER_RETIREMENT_READY")
                {
                    return Ok(());
                }
            }
            Err("child exited before its ready handshake".to_owned())
        })();
        let _ = sender.send(result);
    });
    receiver.recv_timeout(Duration::from_secs(20))??;
    Ok(())
}

fn wait_for_writer(conn: &Connection, child: &mut Child) -> TestResult {
    let started = Instant::now();
    loop {
        match conn.execute_batch("BEGIN IMMEDIATE") {
            Ok(()) => conn.execute_batch("ROLLBACK")?,
            Err(rusqlite::Error::SqliteFailure(error, _))
                if matches!(
                    error.code,
                    ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
                ) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        }
        if let Some(status) = child.try_wait()? {
            return Err(format!("child exited before writer observation: {status}").into());
        }
        if started.elapsed() > Duration::from_secs(10) {
            return Err("no child write transaction observed before timeout".into());
        }
        std::thread::yield_now();
    }
}

fn verify_states(store: &SqliteStore) -> TestResult<(usize, usize)> {
    let mut active = 0;
    let mut retired = 0;
    for index in 0..SYMBOLS {
        let id = symbol_id(index);
        let archives = store.list_symbol_retirements(&id)?;
        let history = store.list_sir_history(&id)?;
        let contracts = store.list_active_contracts_for_symbol(&id)?;
        let violations = store.list_violations_for_symbol(&id, 100)?;
        let fingerprints = store.list_sir_fingerprint_history(&id)?;
        let tests = store.list_test_intents_for_file(&format!("src/{id}.rs"))?;
        if store.get_symbol_record(&id)?.is_some() {
            active += 1;
            assert!(archives.is_empty(), "half archive for active {id}");
            assert!(
                store.get_sir_meta(&id)?.is_some(),
                "lost current SIR for {id}"
            );
            assert!(
                store.read_sir_blob(&id)?.is_some(),
                "lost current blob for {id}"
            );
            assert_eq!(history.len(), HISTORY_VERSIONS, "lost history for {id}");
            assert_eq!(
                (
                    contracts.len(),
                    violations.len(),
                    fingerprints.len(),
                    tests.len()
                ),
                (1, 1, 1, 1),
                "lost active evidence for {id}"
            );
        } else {
            retired += 1;
            assert_eq!(
                archives.len(),
                1,
                "unreachable/duplicate retirement for {id}"
            );
            assert!(store.get_sir_meta(&id)?.is_none());
            assert!(store.read_sir_blob(&id)?.is_none());
            assert!(
                history.is_empty()
                    && contracts.is_empty()
                    && violations.is_empty()
                    && fingerprints.is_empty()
                    && tests.is_empty(),
                "half deletion for {id}"
            );
            let archive = &archives[0];
            assert_eq!(
                archive.symbol.as_ref().map(|symbol| symbol.id.as_str()),
                Some(id.as_str())
            );
            assert_eq!(
                archive.source_hash.as_deref(),
                Some(format!("source-{id}").as_str())
            );
            for (table, count) in [
                ("sir", 1),
                ("sir_history", HISTORY_VERSIONS),
                ("intent_contracts", 1),
                ("intent_violations", 1),
                ("sir_fingerprint_history", 1),
                ("test_intents", 1),
            ] {
                assert_eq!(
                    archive.evidence[table].len(),
                    count,
                    "incomplete {table} archive for {id}"
                );
            }
        }
    }
    Ok((active, retired))
}

#[test]
fn fifty_forced_deaths_preserve_atomic_retirement_and_converge_on_restart() -> TestResult {
    let fixture_root = tempdir()?;
    let started = Instant::now();
    let mut random_state = SEED;
    let mut killed = 0;
    let mut active_after_kill = 0;
    let mut retired_after_kill = 0;
    let mut kill_offsets_us = Vec::new();
    for round in 0..FORCED_DEATHS {
        let workspace = fixture_root.path().join(format!("round-{round:02}"));
        seed_fixture(&workspace)?;
        let mut child = ChildGuard(
            Command::new(std::env::current_exe()?)
                .args(["--exact", "retirement_crash_child", "--nocapture"])
                .env(CHILD_ENV, &workspace)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()?,
        );
        wait_for_ready(&mut child.0)?;
        let conn = Connection::open(workspace.join(".aether/meta.sqlite"))?;
        conn.busy_timeout(Duration::ZERO)?;
        let stdin = child.0.stdin.as_mut().ok_or("missing child stdin")?;
        stdin.write_all(b"start\n")?;
        stdin.flush()?;
        wait_for_writer(&conn, &mut child.0)?;
        random_state = random_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1);
        let offset_us = (random_state >> 32) % 2500;
        kill_offsets_us.push(offset_us);
        std::thread::sleep(Duration::from_micros(offset_us));
        child.0.kill()?;
        let status = child.0.wait()?;
        assert_eq!(
            status.signal(),
            Some(9),
            "round {round}: actual SIGKILL required"
        );
        killed += 1;
        let store = SqliteStore::open(&workspace)?;
        let (active, retired) = verify_states(&store)?;
        assert!(retired >= 1, "round {round}: committed pre-kill work lost");
        active_after_kill += active;
        retired_after_kill += retired;
        let integrity: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        assert_eq!(integrity, "ok", "round {round}");
        let probe_rows: usize =
            conn.query_row("SELECT COUNT(*) FROM crash_probe", [], |row| row.get(0))?;
        assert_eq!(
            probe_rows, retired,
            "round {round}: uncommitted fixture work escaped rollback"
        );
        conn.execute_batch("DROP TRIGGER slow_fixture_delete")?;
        for index in 0..SYMBOLS {
            store.mark_removed(&symbol_id(index))?;
        }
        assert_eq!(
            verify_states(&store)?,
            (0, SYMBOLS),
            "round {round}: restart convergence"
        );
        assert_eq!(
            conn.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))?,
            "ok"
        );
    }
    assert_eq!(killed, FORCED_DEATHS);
    println!(
        "AETHER_RETIREMENT_CRASH_RESULT {}",
        serde_json::json!({
            "scope": "sqlite_only", "seed": SEED, "forced_deaths": killed,
            "writer_windows_observed": FORCED_DEATHS, "rounds": FORCED_DEATHS,
            "symbols_per_round": SYMBOLS, "history_versions": HISTORY_VERSIONS,
            "sir_payload_bytes": PAYLOAD_BYTES, "fixture_trigger_iterations": TRIGGER_ITERATIONS,
            "active_after_kill": active_after_kill, "retired_after_kill": retired_after_kill,
            "restart_convergences": FORCED_DEATHS, "integrity_checks": FORCED_DEATHS * 2,
            "kill_offsets_us": kill_offsets_us, "elapsed_ms": started.elapsed().as_millis(),
            "provider_calls": 0, "mirrored_sir_files": false,
        })
    );
    Ok(())
}
