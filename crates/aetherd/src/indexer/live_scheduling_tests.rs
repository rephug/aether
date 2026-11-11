//! Live semantic scheduling while edits and inference results arrive concurrently.
//!
//! Each test drives the code the daemon runs: the watcher's path processing
//! (`process_debounced_paths`) for edits, and the semantic worker's step
//! (`WorkerContext::run_next_symbol_task`) over a real store and SIR pipeline. Only the
//! inference provider is a fake, scripted to hold, fail or panic on chosen symbol texts.
//! Ordering comes from the provider's gates (a held call reports that it started and
//! waits to be released), never from sleeps: every timeout below is a hang guard that
//! fails the test, not a step in it.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use aether_core::Symbol;
use aether_infer::{InferError, InferenceProvider, SirContext};
use aether_sir::SirAnnotation;
use aether_store::{SirStateStore, SqliteStore};
use async_trait::async_trait;
use tempfile::{TempDir, tempdir};
use tokio::sync::Semaphore;

use super::git_watch::process_debounced_paths;
use super::index_once_tests::{leaf, named, parse};
use super::structural::StructuralIndexer;
use super::worker::{SharedQueueState, WorkerContext};
use super::{
    IndexerConfig, ObserverState, SIR_GENERATION_PASS_SCAN, SirMetaRecord, SirPipeline,
    acquire_inject_write_lock, start_from_snapshot, unix_timestamp_secs,
};

const FILE: &str = "pkg/live.py";
/// How long a test waits for a step that must happen before it fails as hung.
const HANG_GUARD: Duration = Duration::from_secs(120);
/// Tasks one worker may run in one drain before the test fails as never quiescing.
const MAX_TURNS: usize = 64;

/// The fixture module: a bystander, and `target` with the given body (`None`: `target`
/// is not declared). `target` comes last, so removing it moves nothing else.
fn module(target_body: Option<&str>) -> String {
    let mut source = "def bystander(value):\n    return value * 10\n".to_owned();
    if let Some(body) = target_body {
        source.push_str(&format!("\n\ndef target(value):\n    return {body}\n"));
    }
    source
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A provider call the test holds: it reports when it starts and waits for `release`.
struct Hold {
    marker: String,
    entered: Sender<()>,
    gate: Arc<Semaphore>,
}

struct HeldCalls {
    entered: Receiver<()>,
    gate: Arc<Semaphore>,
}

/// Releases the held calls when dropped: taken first thing inside a thread scope, so a
/// failed assertion there still lets the worker threads finish and the scope end.
struct ReleaseOnExit<'a>(&'a HeldCalls);

impl Drop for ReleaseOnExit<'_> {
    fn drop(&mut self) {
        self.0.release();
    }
}

impl HeldCalls {
    fn release_on_exit(&self) -> ReleaseOnExit<'_> {
        ReleaseOnExit(self)
    }

    /// Block until a held call has reached the provider.
    fn wait_entered(&self) {
        self.entered
            .recv_timeout(HANG_GUARD)
            .expect("the held inference call never started");
    }

    /// Let every held call (those waiting and those still to come) proceed.
    fn release(&self) {
        self.gate.add_permits(1_000);
    }
}

/// A local fake inference provider. A leaf SIR it returns quotes the text it was
/// generated from, so a stored SIR shows which version of a symbol it describes.
#[derive(Default)]
struct ScriptedProvider {
    /// Every leaf call in arrival order: the symbol's name and the text sent.
    calls: Mutex<Vec<(String, String)>>,
    active: Mutex<HashMap<String, usize>>,
    /// Symbols that had two calls in flight at once.
    overlaps: Mutex<Vec<String>>,
    holds: Mutex<Vec<Hold>>,
    failing: Mutex<Vec<String>>,
    panicking: Mutex<Vec<String>>,
}

impl ScriptedProvider {
    fn hold_when(&self, marker: &str) -> HeldCalls {
        let (entered_tx, entered_rx) = channel();
        let gate = Arc::new(Semaphore::new(0));
        lock(&self.holds).push(Hold {
            marker: marker.to_owned(),
            entered: entered_tx,
            gate: gate.clone(),
        });
        HeldCalls {
            entered: entered_rx,
            gate,
        }
    }

    fn fail_when(&self, marker: &str) {
        lock(&self.failing).push(marker.to_owned());
    }

    fn recover(&self) {
        lock(&self.failing).clear();
        lock(&self.panicking).clear();
    }

    fn panic_when(&self, marker: &str) {
        lock(&self.panicking).push(marker.to_owned());
    }

    /// The `return` line of every text sent for `name`, in call order.
    fn bodies(&self, name: &str) -> Vec<String> {
        lock(&self.calls)
            .iter()
            .filter(|(called, _)| called == name)
            .map(|(_, text)| {
                text.lines()
                    .last()
                    .unwrap_or_default()
                    .trim()
                    .trim_start_matches("return ")
                    .to_owned()
            })
            .collect()
    }

    fn overlaps(&self) -> Vec<String> {
        lock(&self.overlaps).clone()
    }
}

struct ActiveCall<'a> {
    provider: &'a ScriptedProvider,
    name: String,
}

impl Drop for ActiveCall<'_> {
    fn drop(&mut self) {
        if let Some(count) = lock(&self.provider.active).get_mut(&self.name) {
            *count = count.saturating_sub(1);
        }
    }
}

#[async_trait]
impl InferenceProvider for ScriptedProvider {
    fn provider_name(&self) -> String {
        "scripted".to_owned()
    }

    fn model_name(&self) -> String {
        "scripted-model".to_owned()
    }

    async fn generate_sir(
        &self,
        symbol_text: &str,
        context: &SirContext,
    ) -> Result<SirAnnotation, InferError> {
        let name = context
            .qualified_name
            .rsplit(['.', ':'])
            .next()
            .unwrap_or_default()
            .to_owned();
        lock(&self.calls).push((name.clone(), symbol_text.to_owned()));
        {
            let mut active = lock(&self.active);
            let count = active.entry(name.clone()).or_insert(0);
            *count += 1;
            if *count > 1 {
                lock(&self.overlaps).push(name.clone());
            }
        }
        let _active = ActiveCall {
            provider: self,
            name,
        };

        let gate = lock(&self.holds)
            .iter()
            .find(|hold| symbol_text.contains(&hold.marker))
            .map(|hold| {
                let _ = hold.entered.send(());
                hold.gate.clone()
            });
        if let Some(gate) = gate {
            gate.acquire()
                .await
                .expect("the gate stays open for the whole test")
                .forget();
        }

        let panics = lock(&self.panicking)
            .iter()
            .any(|marker| symbol_text.contains(marker));
        if panics {
            panic!("scripted provider crash");
        }
        let fails = lock(&self.failing)
            .iter()
            .any(|marker| symbol_text.contains(marker));
        if fails {
            return Err(InferError::InvalidResponse(
                "scripted provider failure".to_owned(),
            ));
        }

        Ok(SirAnnotation {
            intent: format!("Describes: {symbol_text}"),
            behavior: None,
            inputs: vec!["value".to_owned()],
            outputs: vec!["result".to_owned()],
            side_effects: Vec::new(),
            dependencies: Vec::new(),
            error_modes: Vec::new(),
            confidence: 0.1,
            edge_cases: None,
            complexity: None,
            method_dependencies: None,
        })
    }
}

/// A watched workspace as the daemon holds it after startup, with its watcher half
/// (observer, structural indexer) driven by the test.
struct Live {
    _temp: TempDir,
    workspace: PathBuf,
    config: IndexerConfig,
    observer: ObserverState,
    structural: StructuralIndexer,
    store: Arc<SqliteStore>,
    queue_state: SharedQueueState,
    provider: Arc<ScriptedProvider>,
}

impl Live {
    /// Start the daemon's state over a workspace whose module holds `source`: every
    /// symbol is queued for its first SIR.
    fn start(source: &str) -> Self {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path().to_path_buf();
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[inference]
provider = "mock"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[embeddings]
enabled = false
vector_backend = "sqlite"
"#,
        )
        .expect("write config");
        fs::create_dir_all(workspace.join("pkg")).expect("create pkg");
        fs::write(workspace.join(FILE), source).expect("write module");

        let config = IndexerConfig {
            workspace: workspace.clone(),
            debounce_ms: 300,
            print_events: false,
            print_sir: false,
            embeddings_only: false,
            force: false,
            full: false,
            deep: false,
            turbo_concurrency: None,
            dry_run: false,
            sir_concurrency: 2,
            lifecycle_logs: false,
            inference_provider: None,
            inference_model: None,
            inference_endpoint: None,
            inference_api_key_env: None,
            pause_flag: None,
        };
        let mut observer = ObserverState::new(workspace.clone()).expect("observer");
        observer.seed_from_disk().expect("seed observer");
        let store = Arc::new(SqliteStore::open(&workspace).expect("open store"));
        let mut structural = StructuralIndexer::new(workspace.clone()).expect("structural");
        let queue_state = start_from_snapshot(&workspace, &observer, &store, &mut structural)
            .expect("start from snapshot");
        Self {
            _temp: temp,
            workspace,
            config,
            observer,
            structural,
            store,
            queue_state,
            provider: Arc::new(ScriptedProvider::default()),
        }
    }

    /// A semantic worker over the shared queue, as `spawn_semantic_worker` builds one,
    /// but around the scripted provider.
    fn worker(&self) -> WorkerContext {
        let pipeline = SirPipeline::new_with_provider(
            self.workspace.clone(),
            1,
            self.provider.clone(),
            "scripted",
            "scripted-model",
        )
        .expect("pipeline")
        .with_structural_updates(false);
        WorkerContext {
            pipeline,
            store: self.store.clone(),
            queue_state: self.queue_state.clone(),
            workspace_root: self.workspace.clone(),
            generation_pass: SIR_GENERATION_PASS_SCAN,
            prompt_config_fingerprint: "scripted-model:none:test".to_owned(),
        }
    }

    /// Save the module and let the watcher process the change, as it does once the
    /// file's debounce window has passed.
    fn edit(&mut self, source: &str) {
        fs::write(self.workspace.join(FILE), source).expect("edit module");
        process_debounced_paths(
            &self.config,
            &mut self.observer,
            &mut self.structural,
            self.store.as_ref(),
            &self.queue_state,
            vec![self.workspace.join(FILE)],
        );
    }

    fn symbol(&self, name: &str) -> Symbol {
        named(&parse(&self.workspace, FILE), name)
    }

    /// The stored SIR of `name` as it is declared on disk now: its status, the hash of
    /// the source it was generated from, and its intent.
    fn stored(&self, name: &str) -> Option<(String, Option<String>, String)> {
        let symbol = self.symbol(name);
        let leaf = leaf(&self.store, &symbol.id)?;
        let blob = self.store.read_sir_blob(&symbol.id).expect("read SIR")?;
        let sir: SirAnnotation = serde_json::from_str(&blob).expect("SIR JSON");
        Some((leaf.status, leaf.source_hash, sir.intent))
    }

    /// Assert that `name` holds a fresh SIR generated from its text on disk, whose
    /// body is `body`.
    fn assert_current(&self, name: &str, body: &str) {
        let symbol = self.symbol(name);
        let (status, source_hash, intent) = self
            .stored(name)
            .unwrap_or_else(|| panic!("{name} has no stored SIR"));
        assert_eq!(status, "fresh", "{name}'s SIR must be fresh");
        assert_eq!(
            source_hash.as_deref(),
            Some(symbol.content_hash.as_str()),
            "{name}'s SIR must record the hash of the source on disk"
        );
        assert!(
            intent.contains(&format!("return {body}")),
            "{name}'s SIR must describe the body on disk (`{body}`), got: {intent}"
        );
    }

    fn inject_manual(&self, name: &str, body: &str) {
        let symbol = self.symbol(name);
        let manual = SirAnnotation {
            intent: format!("Manual: return {body}"),
            behavior: None,
            inputs: vec!["value".to_owned()],
            outputs: vec!["result".to_owned()],
            side_effects: Vec::new(),
            dependencies: Vec::new(),
            error_modes: Vec::new(),
            confidence: 0.95,
            edge_cases: None,
            complexity: None,
            method_dependencies: None,
        };
        let _guard = acquire_inject_write_lock(&self.workspace).expect("inject lock");
        let now = unix_timestamp_secs();
        self.store
            .persist_sir_state_atomically_with_source(
                SirMetaRecord {
                    id: symbol.id.clone(),
                    sir_hash: aether_sir::sir_hash(&manual),
                    sir_version: 1,
                    provider: "manual".to_owned(),
                    model: "manual".to_owned(),
                    generation_pass: "injected".to_owned(),
                    reasoning_trace: None,
                    prompt_hash: None,
                    staleness_score: None,
                    updated_at: now,
                    sir_status: "fresh".to_owned(),
                    last_error: None,
                    last_attempt_at: now,
                },
                &aether_sir::canonicalize_sir_json(&manual),
                None,
                None,
                Some(symbol.content_hash.as_str()),
            )
            .expect("store manual SIR");
    }

    /// Assert that no ready, delayed or in-flight work remains.
    fn assert_idle(&self) {
        assert_eq!(
            self.queue_state.delayed_work(),
            0,
            "no provider retry debt remains"
        );
        assert_eq!(
            self.queue_state.pending_work(),
            (0, 0),
            "no symbol may be left queued or in flight (queued, in flight)"
        );
    }
}

/// Every test ends by printing the provider calls it made, per symbol (shown with
/// `--nocapture`, and in the output of a failing test).
impl Drop for Live {
    fn drop(&mut self) {
        let mut names = lock(&self.provider.calls)
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        for name in names {
            let bodies = self.provider.bodies(&name);
            println!("provider calls for {name}: {} {bodies:?}", bodies.len());
        }
    }
}

/// Run tasks until the queue has none to start; the number of tasks run.
fn drain(worker: &WorkerContext) -> usize {
    for turn in 0..MAX_TURNS {
        if !worker.run_next_symbol_task() {
            return turn;
        }
    }
    panic!("the worker still had tasks after {MAX_TURNS} turns: a symbol is requeued forever");
}

/// Drain only work due at this deterministic monotonic instant. Clock advancement is
/// explicit in recovery tests; a test never waits for the production cooldown.
fn drain_at(worker: &WorkerContext, now: Instant) -> usize {
    for turn in 0..MAX_TURNS {
        if !worker.run_next_symbol_task_at(now) {
            return turn;
        }
    }
    panic!("the worker kept retrying without clock advancement");
}

fn join<T>(handle: std::thread::ScopedJoinHandle<'_, T>) -> T {
    handle.join().expect("worker thread panicked")
}

/// The main regression. `target` is edited while its first inference is in flight. The
/// old result is rejected as stale (it describes a body the symbol no longer has), and
/// the edited body must then get its own SIR with no further edit and no restart.
#[test]
fn an_edit_during_inference_gets_its_own_sir_without_another_edit_or_restart() {
    let mut live = Live::start(&module(Some("value + 1")));
    let before = live.symbol("target");
    let held = live.provider.hold_when("value + 1");
    let worker = live.worker();

    std::thread::scope(|scope| {
        let _release = held.release_on_exit();
        let draining = scope.spawn(move || drain(&worker));
        held.wait_entered();

        live.edit(&module(Some("value + 2")));
        let after = live.symbol("target");
        assert_eq!(after.id, before.id, "a body edit keeps the symbol id");
        assert_ne!(after.content_hash, before.content_hash);

        held.release();
        join(draining);
    });

    live.assert_current("target", "value + 2");
    live.assert_current("bystander", "value * 10");
    assert_eq!(
        live.provider.bodies("target"),
        ["value + 1", "value + 2"],
        "one call for the body in flight, one follow-up for the edited body"
    );
    assert_eq!(live.provider.bodies("bystander"), ["value * 10"]);
    assert!(live.provider.overlaps().is_empty());
    live.assert_idle();
}

/// Many edits during one inference leave one follow-up, for the latest body only.
#[test]
fn edits_during_one_inference_coalesce_into_one_follow_up_for_the_latest_body() {
    let mut live = Live::start(&module(Some("value + 1")));
    let held = live.provider.hold_when("value + 1");
    let worker = live.worker();

    std::thread::scope(|scope| {
        let _release = held.release_on_exit();
        let draining = scope.spawn(move || drain(&worker));
        held.wait_entered();

        for step in 2..=9 {
            live.edit(&module(Some(&format!("value + {step}"))));
            let (queued, in_flight) = live.queue_state.pending_work();
            assert_eq!(in_flight, 1, "the first inference is still the only flight");
            assert!(
                queued <= 1,
                "edits to a symbol in flight must not grow the queue (queued: {queued})"
            );
        }

        held.release();
        join(draining);
    });

    live.assert_current("target", "value + 9");
    assert_eq!(
        live.provider.bodies("target"),
        ["value + 1", "value + 9"],
        "the intermediate bodies are never sent"
    );
    assert!(live.provider.overlaps().is_empty());
    live.assert_idle();
}

/// A → B → A while the inference for A is in flight: the result still describes the
/// body on disk, is stored, and no second call is made for the same text.
#[test]
fn a_change_reverted_during_inference_is_served_by_the_call_in_flight() {
    let mut live = Live::start(&module(Some("value + 1")));
    let held = live.provider.hold_when("value + 1");
    let worker = live.worker();

    std::thread::scope(|scope| {
        let _release = held.release_on_exit();
        let draining = scope.spawn(move || drain(&worker));
        held.wait_entered();
        live.edit(&module(Some("value + 2")));
        live.edit(&module(Some("value + 1")));
        held.release();
        join(draining);
    });

    live.assert_current("target", "value + 1");
    assert_eq!(live.provider.bodies("target"), ["value + 1"]);
    live.assert_idle();
}

/// A → B → A where A's SIR is stored and the inference in flight is for B: B's result
/// is rejected as stale, and A's stored SIR, which describes the body on disk again,
/// stands without another call.
#[test]
fn a_revert_to_the_stored_body_during_inference_keeps_the_stored_sir() {
    let mut live = Live::start(&module(Some("value + 1")));
    let worker = live.worker();
    drain(&worker);
    live.assert_current("target", "value + 1");

    let held = live.provider.hold_when("value + 2");
    live.edit(&module(Some("value + 2")));
    std::thread::scope(|scope| {
        let _release = held.release_on_exit();
        let draining = scope.spawn(move || drain(&worker));
        held.wait_entered();
        live.edit(&module(Some("value + 1")));
        held.release();
        join(draining);
    });

    live.assert_current("target", "value + 1");
    assert_eq!(
        live.provider.bodies("target"),
        ["value + 1", "value + 2"],
        "the rejected call for the reverted body is the last one"
    );
    live.assert_idle();
}

/// `target` is deleted and declared again (same id) while its old inference is still in
/// flight. A second worker must not start the recreated symbol alongside the old flight,
/// the late completion must neither store the old body nor drop the recreated symbol's
/// work, and the recreated body gets exactly one call.
#[test]
fn a_late_completion_cannot_clear_or_overwrite_a_deleted_and_recreated_symbol() {
    let mut live = Live::start(&module(Some("value + 1")));
    let before = live.symbol("target");
    let held = live.provider.hold_when("value + 1");
    let first_worker = live.worker();
    let second_worker = live.worker();

    std::thread::scope(|scope| {
        let _release = held.release_on_exit();
        let draining = scope.spawn(move || drain(&first_worker));
        held.wait_entered();

        live.edit(&module(None));
        live.edit(&module(Some("value + 3")));
        assert_eq!(
            live.symbol("target").id,
            before.id,
            "the recreated symbol reuses the id"
        );

        // The old flight is still held, so this worker runs whatever else is queued.
        drain(&second_worker);
        assert_eq!(
            live.provider.bodies("target"),
            ["value + 1"],
            "the recreated symbol must wait for the flight its id still has"
        );

        held.release();
        join(draining);
    });

    live.assert_current("target", "value + 3");
    assert_eq!(live.provider.bodies("target"), ["value + 1", "value + 3"]);
    assert!(live.provider.overlaps().is_empty());
    live.assert_idle();
}

/// The inference in flight fails (after the pipeline's own retries) and the symbol was
/// edited meanwhile: the edited body still gets its SIR.
#[test]
fn a_failed_inference_with_a_pending_edit_recovers_on_the_latest_body() {
    let mut live = Live::start(&module(Some("value + 1")));
    live.provider.fail_when("value + 1");
    let held = live.provider.hold_when("value + 1");
    let worker = live.worker();

    std::thread::scope(|scope| {
        let _release = held.release_on_exit();
        let draining = scope.spawn(move || drain(&worker));
        held.wait_entered();
        live.edit(&module(Some("value + 2")));
        held.release();
        join(draining);
    });

    live.assert_current("target", "value + 2");
    assert_eq!(
        live.provider.bodies("target"),
        ["value + 1", "value + 1", "value + 1", "value + 2"],
        "the failing body is tried the pipeline's bounded number of times, then the edit runs"
    );
    live.assert_idle();
}

/// A symbol whose inference keeps failing is not retried forever, does not keep other
/// symbols from being processed, and gets a SIR after its next edit.
#[test]
fn a_failing_symbol_does_not_starve_others_and_recovers_after_its_next_edit() {
    let mut live = Live::start(&module(Some("value + 1")));
    live.provider.fail_when("value + 1");
    let worker = live.worker();

    drain(&worker);
    live.assert_current("bystander", "value * 10");
    assert_ne!(
        live.stored("target").map(|(status, _, _)| status),
        Some("fresh".to_owned()),
        "the failed symbol has no fresh SIR"
    );
    assert_eq!(live.provider.bodies("target").len(), 3);
    assert_eq!(live.queue_state.pending_work(), (0, 0));
    assert_eq!(live.queue_state.delayed_work(), 1);

    live.edit(&module(Some("value + 2")));
    drain(&worker);
    live.assert_current("target", "value + 2");
    assert_eq!(live.provider.bodies("target").len(), 4);
    live.assert_idle();
}

/// A task that errors (here: the provider call crashes) is retried a bounded number of
/// times behind the other queued symbols, then dropped until its next edit.
#[test]
fn a_crashing_task_is_retried_a_bounded_number_of_times_and_recovers_after_an_edit() {
    let mut live = Live::start(&module(Some("value + 1")));
    live.provider.panic_when("value + 1");
    let worker = live.worker();

    drain(&worker);
    live.assert_current("bystander", "value * 10");
    assert!(live.stored("target").is_none());
    assert_eq!(
        live.provider.bodies("target").len(),
        3,
        "one attempt and two retries"
    );
    live.assert_idle();

    live.edit(&module(Some("value + 2")));
    drain(&worker);
    live.assert_current("target", "value + 2");
    assert_eq!(live.provider.bodies("target").len(), 4);
    live.assert_idle();
}

/// Two workers over one queue, with an edit landing during a held flight: every symbol
/// ends with a SIR for its text on disk and no symbol ever has two calls in flight.
#[test]
fn two_workers_keep_one_flight_per_symbol_and_serve_the_latest_body() {
    let source = |target_body: &str| {
        format!(
            "{}\n\ndef extra_one(value):\n    return value - 1\n\n\ndef extra_two(value):\n    return value - 2\n",
            module(Some(target_body)).trim_end()
        )
    };
    let mut live = Live::start(&source("value + 1"));
    let held = live.provider.hold_when("value + 1");
    let first_worker = live.worker();
    let second_worker = live.worker();

    std::thread::scope(|scope| {
        let _release = held.release_on_exit();
        let first = scope.spawn(move || drain(&first_worker));
        held.wait_entered();
        let second = scope.spawn(move || drain(&second_worker));

        live.edit(&source("value + 2"));
        // A duplicate of the same change must not start a second flight either.
        live.edit(&source("value + 2"));

        // With the first worker held, the second empties the rest of the queue and
        // must leave `target` to the flight it already has.
        join(second);
        assert_eq!(live.provider.bodies("target"), ["value + 1"]);

        held.release();
        join(first);
    });

    live.assert_current("target", "value + 2");
    live.assert_current("bystander", "value * 10");
    live.assert_current("extra_one", "value - 1");
    live.assert_current("extra_two", "value - 2");
    assert_eq!(live.provider.bodies("target"), ["value + 1", "value + 2"]);
    for name in ["bystander", "extra_one", "extra_two"] {
        assert_eq!(live.provider.bodies(name).len(), 1, "{name} is sent once");
    }
    assert!(live.provider.overlaps().is_empty());
    live.assert_idle();
}

/// A SIR written by hand (an injection) for the edited body while the old inference is
/// in flight is kept: the late result for the old body does not replace it, and no
/// regeneration is queued over it, since it already describes the body on disk.
#[test]
fn a_late_completion_does_not_overwrite_a_newer_manual_sir_or_regenerate_over_it() {
    let mut live = Live::start(&module(Some("value + 1")));
    let held = live.provider.hold_when("value + 1");
    let worker = live.worker();

    std::thread::scope(|scope| {
        let _release = held.release_on_exit();
        let draining = scope.spawn(move || drain(&worker));
        held.wait_entered();

        live.edit(&module(Some("value + 2")));
        let edited = live.symbol("target");
        let manual = SirAnnotation {
            intent: "Manual: return value + 2".to_owned(),
            behavior: None,
            inputs: vec!["value".to_owned()],
            outputs: vec!["result".to_owned()],
            side_effects: Vec::new(),
            dependencies: Vec::new(),
            error_modes: Vec::new(),
            confidence: 0.95,
            edge_cases: None,
            complexity: None,
            method_dependencies: None,
        };
        {
            let _inject_guard = acquire_inject_write_lock(&live.workspace).expect("inject lock");
            let now = unix_timestamp_secs();
            live.store
                .persist_sir_state_atomically_with_source(
                    SirMetaRecord {
                        id: edited.id.clone(),
                        sir_hash: aether_sir::sir_hash(&manual),
                        sir_version: 1,
                        provider: "manual".to_owned(),
                        model: "manual".to_owned(),
                        generation_pass: "injected".to_owned(),
                        reasoning_trace: None,
                        prompt_hash: None,
                        staleness_score: None,
                        updated_at: now,
                        sir_status: "fresh".to_owned(),
                        last_error: None,
                        last_attempt_at: now,
                    },
                    &aether_sir::canonicalize_sir_json(&manual),
                    None,
                    None,
                    Some(edited.content_hash.as_str()),
                )
                .expect("store the manual SIR");
        }

        held.release();
        join(draining);
    });

    let (status, source_hash, intent) = live.stored("target").expect("the manual SIR");
    assert_eq!(status, "fresh");
    assert_eq!(
        source_hash.as_deref(),
        Some(live.symbol("target").content_hash.as_str())
    );
    assert_eq!(intent, "Manual: return value + 2", "the manual SIR stands");
    assert_eq!(
        live.provider.bodies("target"),
        ["value + 1"],
        "nothing is regenerated over a SIR that already describes the body on disk"
    );
    live.assert_idle();
}

#[test]
fn provider_recovery_needs_no_edit_request_or_restart() {
    let live = Live::start(&module(Some("value + 1")));
    live.provider.fail_when("value + 1");
    let worker = live.worker();
    let mut now = Instant::now();

    drain_at(&worker, now);
    live.assert_current("bystander", "value * 10");
    assert_eq!(live.provider.bodies("target").len(), 3);
    assert_eq!(live.queue_state.delayed_work(), 1);
    for _ in 0..20 {
        assert!(!worker.run_next_symbol_task_at(now));
    }
    assert!(!worker.run_next_symbol_task_at(now + Duration::from_millis(4_999)));
    assert_eq!(live.provider.bodies("target").len(), 3);

    // Stay unavailable beyond the original three-task exhaustion limit. The
    // obligation must survive; repeated polling cannot turn it into a hot loop.
    for (delay, expected_calls) in [(5, 6), (10, 9), (20, 12)] {
        now += Duration::from_secs(delay);
        drain_at(&worker, now);
        assert_eq!(live.provider.bodies("target").len(), expected_calls);
        assert_eq!(live.queue_state.delayed_work(), 1);
        assert!(!worker.run_next_symbol_task_at(now));
    }

    live.provider.recover();
    now += Duration::from_secs(40);
    drain_at(&worker, now);
    live.assert_current("target", "value + 1");
    assert_eq!(live.provider.bodies("target").len(), 13);
    live.assert_idle();
}

#[test]
fn an_edit_replaces_provider_retry_debt_with_only_the_latest_body() {
    let mut live = Live::start(&module(Some("value + 1")));
    live.provider.fail_when("value + 1");
    let worker = live.worker();
    let now = Instant::now();
    drain_at(&worker, now);
    assert_eq!(live.queue_state.delayed_work(), 1);

    live.edit(&module(Some("value + 2")));
    drain_at(&worker, now);
    live.assert_current("target", "value + 2");
    assert_eq!(
        live.provider.bodies("target"),
        ["value + 1", "value + 1", "value + 1", "value + 2"]
    );
    assert!(!worker.run_next_symbol_task_at(now + Duration::from_secs(3_600)));
    live.assert_idle();
}

#[test]
fn removing_a_symbol_cancels_its_provider_retry() {
    let mut live = Live::start(&module(Some("value + 1")));
    live.provider.fail_when("value + 1");
    let worker = live.worker();
    let now = Instant::now();
    drain_at(&worker, now);
    assert_eq!(live.queue_state.delayed_work(), 1);

    live.edit(&module(None));
    drain_at(&worker, now + Duration::from_secs(3_600));
    assert_eq!(live.provider.bodies("target").len(), 3);
    live.assert_idle();
}

#[test]
fn a_manual_sir_written_during_provider_cooldown_cancels_reinference() {
    let live = Live::start(&module(Some("value + 1")));
    live.provider.fail_when("value + 1");
    let worker = live.worker();
    let now = Instant::now();
    drain_at(&worker, now);
    live.inject_manual("target", "value + 1");
    let manual = live.stored("target");

    drain_at(&worker, now + Duration::from_secs(5));
    assert_eq!(live.stored("target"), manual);
    assert_eq!(live.provider.bodies("target").len(), 3);
    live.assert_idle();
}

#[test]
fn a_manual_sir_written_during_failed_inference_leaves_no_provider_retry() {
    let live = Live::start(&module(Some("value + 1")));
    live.provider.fail_when("value + 1");
    let held = live.provider.hold_when("value + 1");
    let worker = live.worker();
    let now = Instant::now();
    std::thread::scope(|scope| {
        let _release = held.release_on_exit();
        let draining = scope.spawn(move || drain_at(&worker, now));
        held.wait_entered();
        live.inject_manual("target", "value + 1");
        held.release();
        join(draining);
    });

    live.assert_current("target", "value + 1");
    assert_eq!(
        live.stored("target").expect("manual SIR").2,
        "Manual: return value + 1"
    );
    assert_eq!(live.provider.bodies("target").len(), 3);
    live.assert_idle();
}

#[test]
fn python_identity_split_and_rejoin_during_inference_serves_only_the_latest_singleton() {
    let mut live = Live::start(&module(Some("value + 1")));
    let original_id = live.symbol("target").id;
    let held = live.provider.hold_when("value + 1");
    let worker = live.worker();
    std::thread::scope(|scope| {
        let _release = held.release_on_exit();
        let draining = scope.spawn(move || drain(&worker));
        held.wait_entered();
        live.edit("def bystander(value):\n    return value * 10\n\nif ENABLED:\n    def target(value):\n        return value + 2\nelse:\n    def target(value):\n        return value + 3\n");
        let variants = parse(&live.workspace, FILE);
        let variant_ids = variants
            .iter()
            .filter(|symbol| symbol.name == "target")
            .map(|symbol| symbol.id.clone())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(
            variant_ids.len(),
            2,
            "the integrated Python identity fix is required"
        );
        assert!(!variant_ids.contains(&original_id));
        live.edit(&module(Some("value + 4")));
        assert_eq!(live.symbol("target").id, original_id);
        held.release();
        join(draining);
    });

    live.assert_current("target", "value + 4");
    assert_eq!(live.provider.bodies("target"), ["value + 1", "value + 4"]);
    assert!(live.provider.overlaps().is_empty());
    live.assert_idle();
}

#[test]
fn a_manual_sir_injected_after_retry_preflight_is_not_overwritten() {
    let live = Live::start(&module(Some("value + 1")));
    live.provider.fail_when("value + 1");
    let worker = live.worker();
    let now = Instant::now();
    drain_at(&worker, now);
    live.provider.recover();

    assert!(
        worker.run_next_symbol_task_after_preflight(now + Duration::from_secs(5), || live
            .inject_manual("target", "value + 1"),)
    );
    live.assert_current("target", "value + 1");
    assert_eq!(
        live.stored("target").expect("manual SIR").2,
        "Manual: return value + 1"
    );
    assert_eq!(live.provider.bodies("target").len(), 3);
    live.assert_idle();
}

#[test]
fn task_errors_during_provider_recovery_do_not_erase_retry_debt() {
    let live = Live::start(&module(Some("value + 1")));
    live.provider.fail_when("value + 1");
    let worker = live.worker();
    let mut now = Instant::now();
    drain_at(&worker, now);
    live.provider.recover();
    live.provider.panic_when("value + 1");

    for delay in [5, 10, 20, 40] {
        now += Duration::from_secs(delay);
        drain_at(&worker, now);
        assert_eq!(live.queue_state.delayed_work(), 1);
        assert!(!worker.run_next_symbol_task_at(now));
    }
    live.provider.recover();
    now += Duration::from_secs(80);
    drain_at(&worker, now);
    live.assert_current("target", "value + 1");
    live.assert_idle();
}

#[test]
fn an_empty_blob_does_not_satisfy_provider_retry_debt() {
    let live = Live::start(&module(Some("value + 1")));
    live.provider.fail_when("value + 1");
    let worker = live.worker();
    let now = Instant::now();
    drain_at(&worker, now);
    live.inject_manual("target", "value + 1");
    live.store
        .write_sir_blob(&live.symbol("target").id, "")
        .expect("empty fixture blob");
    live.provider.recover();
    drain_at(&worker, now + Duration::from_secs(5));
    live.assert_current("target", "value + 1");
    assert_eq!(live.provider.bodies("target").len(), 4);
    live.assert_idle();
}

#[test]
fn a_temporary_source_read_failure_keeps_provider_recovery_pending() {
    let live = Live::start(&module(Some("value + 1")));
    live.provider.fail_when("value + 1");
    let worker = live.worker();
    let now = Instant::now();
    drain_at(&worker, now);
    live.provider.recover();

    // Simulate a temporarily unavailable source without scheduling another edit.
    let source = live.workspace.join(FILE);
    let hidden = live.workspace.join("temporarily-hidden-source");
    fs::rename(&source, &hidden).expect("hide source for one recovery turn");
    drain_at(&worker, now + Duration::from_secs(5));
    assert_eq!(live.provider.bodies("target").len(), 3);
    assert_eq!(live.queue_state.delayed_work(), 1);
    assert!(!worker.run_next_symbol_task_at(now + Duration::from_secs(14)));

    fs::rename(&hidden, &source).expect("restore the same source");
    drain_at(&worker, now + Duration::from_secs(15));
    live.assert_current("target", "value + 1");
    assert_eq!(live.provider.bodies("target").len(), 4);
    live.assert_idle();
}

#[test]
fn a_temporary_persistence_failure_keeps_provider_recovery_pending() {
    let live = Live::start(&module(Some("value + 1")));
    live.provider.fail_when("value + 1");
    let worker = live.worker();
    let now = Instant::now();
    drain_at(&worker, now);
    live.provider.recover();
    let connection = rusqlite::Connection::open(live.workspace.join(".aether/meta.sqlite"))
        .expect("open fixture database");
    connection
        .execute_batch(
            "CREATE TRIGGER reject_recovery_intent BEFORE INSERT ON write_intents BEGIN
         SELECT RAISE(FAIL, 'fixture persistence outage'); END;",
        )
        .expect("install temporary write failure");

    drain_at(&worker, now + Duration::from_secs(5));
    assert_eq!(live.provider.bodies("target").len(), 4);
    assert_eq!(live.queue_state.delayed_work(), 1);
    assert!(!worker.run_next_symbol_task_at(now + Duration::from_secs(14)));

    connection
        .execute_batch("DROP TRIGGER reject_recovery_intent;")
        .expect("restore fixture persistence");
    drain_at(&worker, now + Duration::from_secs(15));
    live.assert_current("target", "value + 1");
    assert_eq!(live.provider.bodies("target").len(), 5);
    live.assert_idle();
}
