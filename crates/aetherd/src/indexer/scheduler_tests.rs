//! The live scheduler against an independent reference model.
//!
//! `Model` restates the scheduling rules in a different shape (one explicit phase per
//! symbol, a linear scan for the next task) and carries one piece of bookkeeping the
//! scheduler has no counterpart for: `owed`, the symbols scheduled since a task last
//! started for them. Every operation is applied to both; their answers and their whole
//! observable state must agree, and the invariants listed in `scheduler` must hold
//! after every step.
//!
//! Two drivers: every operation sequence up to a fixed length over a small alphabet
//! (one or two symbols, two workers), and long seeded pseudo-random runs against a
//! simulated store that end with "edits stop and every task succeeds" and then require
//! every symbol still owed a SIR to have one for its latest version.
//!
//! The scheduler sits behind one mutex, so concurrent callers execute these operations
//! in some sequential order; the sequences here stand for those orders. What this does
//! not cover is said in the tests' names: sequences longer than the bound, more symbols
//! or workers than the alphabet has, and anything outside the scheduler (the pipeline,
//! the store, the watcher).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::scheduler::{
    Completion, FRONT_SCORE, MAX_TASK_RETRIES, Scheduled, SemanticScheduler, SymbolTask,
    TaskOutcome,
};
use super::tests::test_symbol;

const IDS: [&str; 2] = ["sym-a", "sym-b"];
const WORKERS: usize = 2;

fn version_symbol(id: &str, version: u8) -> aether_core::Symbol {
    test_symbol(id, &format!("v{version}"))
}

fn version_of(symbol: &aether_core::Symbol) -> u8 {
    symbol
        .signature_fingerprint
        .trim_start_matches('v')
        .parse()
        .expect("a version fingerprint")
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Observe {
        id: usize,
        version: u8,
    },
    Schedule {
        id: usize,
        version: u8,
        score: f64,
    },
    Request {
        id: usize,
    },
    Remove {
        id: usize,
    },
    Start {
        worker: usize,
    },
    Complete {
        worker: usize,
        errored: bool,
        current: bool,
    },
    /// Complete again with the ticket of the task completed last.
    CompleteStale,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Idle,
    /// `score`: infinite at the front. `since`: when it took its place.
    Waiting {
        score: f64,
        since: u64,
    },
    Running {
        ticket: u64,
        again: Option<f64>,
    },
}

/// The reference model.
#[derive(Debug, Clone, Default)]
struct Model {
    version: BTreeMap<usize, u8>,
    phase: BTreeMap<usize, Phase>,
    strikes: BTreeMap<usize, u32>,
    clock: u64,
    tickets: u64,
    /// Scheduled, and no task has started for the symbol since.
    owed: BTreeSet<usize>,
}

impl Model {
    fn phase(&self, id: usize) -> Phase {
        self.phase.get(&id).copied().unwrap_or(Phase::Idle)
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    fn observe(&mut self, id: usize, version: u8) {
        self.version.insert(id, version);
    }

    fn schedule(&mut self, id: usize, version: u8, score: f64) -> Scheduled {
        self.version.insert(id, version);
        self.strikes.remove(&id);
        self.owed.insert(id);
        match self.phase(id) {
            Phase::Running { ticket, again } => {
                let again = Some(again.map_or(score, |pending| pending.max(score)));
                self.phase.insert(id, Phase::Running { ticket, again });
                Scheduled::Coalesced
            }
            Phase::Waiting { .. } => Scheduled::AlreadyQueued,
            Phase::Idle => {
                let since = self.tick();
                self.phase.insert(id, Phase::Waiting { score, since });
                Scheduled::Queued
            }
        }
    }

    fn request(&mut self, id: usize) -> bool {
        match self.phase(id) {
            Phase::Running { ticket, again } => {
                if again.is_some() {
                    let again = Some(f64::INFINITY);
                    self.phase.insert(id, Phase::Running { ticket, again });
                }
                false
            }
            Phase::Waiting { score, .. } => {
                if score != f64::INFINITY {
                    let since = self.tick();
                    let score = f64::INFINITY;
                    self.phase.insert(id, Phase::Waiting { score, since });
                }
                true
            }
            Phase::Idle if self.version.contains_key(&id) => {
                let since = self.tick();
                let score = f64::INFINITY;
                self.phase.insert(id, Phase::Waiting { score, since });
                true
            }
            Phase::Idle => false,
        }
    }

    fn remove(&mut self, id: usize) {
        self.version.remove(&id);
        self.strikes.remove(&id);
        self.owed.remove(&id);
        match self.phase(id) {
            Phase::Waiting { .. } => {
                self.phase.insert(id, Phase::Idle);
            }
            Phase::Running { ticket, .. } => {
                let again = None;
                self.phase.insert(id, Phase::Running { ticket, again });
            }
            Phase::Idle => {}
        }
    }

    /// The id, version, score and ticket of the task started, if any symbol waits.
    fn start(&mut self) -> Option<(usize, u8, f64, u64)> {
        let mut best: Option<(usize, f64, u64)> = None;
        for (&id, phase) in &self.phase {
            if let Phase::Waiting { score, since } = *phase {
                let better = match best {
                    None => true,
                    Some((_, best_score, best_since)) => {
                        score > best_score || (score == best_score && since < best_since)
                    }
                };
                if better {
                    best = Some((id, score, since));
                }
            }
        }
        let (id, score, _) = best?;
        let version = *self
            .version
            .get(&id)
            .expect("a waiting symbol is a known symbol");
        self.tickets += 1;
        let ticket = self.tickets;
        self.phase.insert(
            id,
            Phase::Running {
                ticket,
                again: None,
            },
        );
        self.owed.remove(&id);
        Some((id, version, score, ticket))
    }

    fn complete(&mut self, id: usize, ticket: u64, errored: bool, current: bool) -> Completion {
        let again = match self.phase(id) {
            Phase::Running {
                ticket: running,
                again,
            } if running == ticket => again,
            _ => return Completion::StaleTicket,
        };
        self.phase.insert(id, Phase::Idle);
        let known = self.version.contains_key(&id);

        if let Some(score) = again {
            self.strikes.remove(&id);
            if known && !current {
                let since = self.tick();
                self.phase.insert(id, Phase::Waiting { score, since });
                return Completion::FollowUpQueued;
            }
            self.owed.remove(&id);
            return Completion::Done;
        }
        if !errored || !known {
            self.strikes.remove(&id);
            return Completion::Done;
        }
        let strikes = self.strikes.get(&id).copied().unwrap_or(0) + 1;
        if strikes > MAX_TASK_RETRIES {
            self.strikes.remove(&id);
            return Completion::GaveUp;
        }
        self.strikes.insert(id, strikes);
        let since = self.tick();
        self.phase.insert(id, Phase::Waiting { score: 0.0, since });
        Completion::RetryQueued { attempt: strikes }
    }
}

/// The model's score for one of the scheduler's (the front is `f64::MAX` there).
fn model_score(score: f64) -> f64 {
    if score == FRONT_SCORE {
        f64::INFINITY
    } else {
        score
    }
}

fn index_of(symbol_id: &str) -> usize {
    IDS.iter()
        .position(|id| *id == symbol_id)
        .expect("a symbol of the alphabet")
}

/// The scheduler and the model side by side, with the tasks the workers hold.
#[derive(Debug, Clone, Default)]
struct Pair {
    real: SemanticScheduler,
    model: Model,
    workers: [Option<SymbolTask>; WORKERS],
    last_completed: Option<SymbolTask>,
}

impl Pair {
    fn applicable(&self, op: Op) -> bool {
        match op {
            Op::Start { worker } => self.workers[worker].is_none(),
            Op::Complete { worker, .. } => self.workers[worker].is_some(),
            Op::CompleteStale => self.last_completed.is_some(),
            _ => true,
        }
    }

    /// Apply `op` to both and compare what they answer; the scheduler's completion.
    fn apply(&mut self, op: Op) -> Option<Completion> {
        let mut completion = None;
        match op {
            Op::Observe { id, version } => {
                self.real.upsert_symbol(version_symbol(IDS[id], version));
                self.model.observe(id, version);
            }
            Op::Schedule { id, version, score } => {
                let real = self.real.schedule(version_symbol(IDS[id], version), score);
                let model = self.model.schedule(id, version, score);
                assert_eq!(real, model, "{op:?}");
            }
            Op::Request { id } => {
                let real = self.real.bump_to_front(IDS[id]);
                let model = self.model.request(id);
                assert_eq!(real, model, "{op:?}");
            }
            Op::Remove { id } => {
                self.real.remove_symbol(IDS[id]);
                self.model.remove(id);
            }
            Op::Start { worker } => {
                let real = self.real.start_next();
                let model = self.model.start();
                let real_view = real.as_ref().map(|task| {
                    (
                        index_of(&task.symbol.id),
                        version_of(&task.symbol),
                        model_score(task.score),
                        task.ticket,
                    )
                });
                assert_eq!(real_view, model, "{op:?}");
                self.workers[worker] = real;
            }
            Op::Complete {
                worker,
                errored,
                current,
            } => {
                let task = self.workers[worker].take().expect("a busy worker");
                completion = Some(self.complete(&task, errored, current));
                self.last_completed = Some(task);
            }
            Op::CompleteStale => {
                let task = self.last_completed.clone().expect("a completed task");
                let done = self.complete(&task, false, false);
                // The same id may be in flight again, but never under this ticket.
                assert_eq!(done, Completion::StaleTicket, "{op:?}");
            }
        }
        self.check(op);
        completion
    }

    fn complete(&mut self, task: &SymbolTask, errored: bool, current: bool) -> Completion {
        let outcome = if errored {
            TaskOutcome::Errored
        } else {
            TaskOutcome::Finished
        };
        let real = self.real.complete(task, outcome, |_| current);
        let model = self
            .model
            .complete(index_of(&task.symbol.id), task.ticket, errored, current);
        assert_eq!(real, model, "completing {task:?}");
        real
    }

    /// The scheduler's observable state equals the model's, and the invariants hold.
    fn check(&self, op: Op) {
        let mut queued = 0usize;
        let mut flights = 0usize;
        for (id, symbol_id) in IDS.iter().enumerate() {
            let is_queued = self.real.is_queued(symbol_id);
            let flight = self.real.flight(symbol_id);
            let known = self.real.symbol(symbol_id).map(version_of);
            let retries = self.real.retries(symbol_id);
            queued += usize::from(is_queued);
            flights += usize::from(flight.is_some());

            // Agreement with the model.
            assert_eq!(known, self.model.version.get(&id).copied(), "{op:?}");
            assert_eq!(
                retries,
                self.model.strikes.get(&id).copied().unwrap_or(0),
                "{op:?}"
            );
            match self.model.phase(id) {
                Phase::Idle => assert!(!is_queued && flight.is_none(), "{op:?}"),
                Phase::Waiting { .. } => assert!(is_queued && flight.is_none(), "{op:?}"),
                Phase::Running { ticket, again } => {
                    assert!(!is_queued, "{op:?}");
                    let flight = flight.map(|(ticket, pending)| (ticket, pending.map(model_score)));
                    assert_eq!(flight, Some((ticket, again)), "{op:?}");
                }
            }

            // Invariant 1: never queued and in flight at once.
            assert!(!(is_queued && flight.is_some()), "{op:?}");
            // Invariant 2: a queued id is a known symbol.
            assert!(!is_queued || known.is_some(), "{op:?}");
            // Invariant 3: a scheduled version is queued or pending as a follow-up.
            if self.model.owed.contains(&id) {
                let pending = flight.is_some_and(|(_, follow_up)| follow_up.is_some());
                assert!(
                    is_queued || pending,
                    "{symbol_id} was scheduled and then dropped by {op:?}"
                );
            }
            // Invariant 5: retries are bounded, and only kept for work that exists.
            assert!(retries <= MAX_TASK_RETRIES, "{op:?}");
            assert!(retries == 0 || is_queued || flight.is_some(), "{op:?}");
        }
        assert!(
            self.real
                .retry_ids()
                .iter()
                .all(|id| IDS.contains(&id.as_str())),
            "{op:?}"
        );

        // Invariants 1 and 4: one flight per busy worker, each under its own ticket.
        let held = self.workers.iter().flatten().collect::<Vec<_>>();
        assert_eq!(self.real.in_flight_len(), held.len(), "{op:?}");
        assert_eq!(flights, held.len(), "{op:?}");
        let mut tickets = BTreeSet::new();
        for task in held {
            assert!(tickets.insert(task.ticket), "{op:?}");
            let flight = self.real.flight(&task.symbol.id);
            assert_eq!(
                flight.map(|(ticket, _)| ticket),
                Some(task.ticket),
                "{op:?}"
            );
        }

        // Invariant 6: the queue is bounded by the symbols, its heap by the queue.
        assert_eq!(self.real.queued_len(), queued, "{op:?}");
        assert!(self.real.queued_len() <= IDS.len(), "{op:?}");
        assert!(
            self.real.queue_heap_len() <= 2 * self.real.queued_len() + 32,
            "{op:?}"
        );
    }
}

/// Every operation over `ids` symbols: two versions each (so `A → B → A` is covered).
fn alphabet(ids: usize) -> Vec<Op> {
    let mut ops = Vec::new();
    for id in 0..ids {
        // Different symbols get different priorities; one symbol gets two.
        let score = if id == 0 { 0.4 } else { 0.7 };
        for version in 0..2 {
            ops.push(Op::Schedule { id, version, score });
        }
        if ids == 1 {
            let (version, score) = (0, 0.9);
            ops.push(Op::Schedule { id, version, score });
        }
        ops.push(Op::Observe { id, version: 1 });
        ops.push(Op::Request { id });
        ops.push(Op::Remove { id });
    }
    for worker in 0..WORKERS {
        ops.push(Op::Start { worker });
        for errored in [false, true] {
            for current in [false, true] {
                ops.push(Op::Complete {
                    worker,
                    errored,
                    current,
                });
            }
        }
    }
    ops.push(Op::CompleteStale);
    ops
}

/// Apply every applicable sequence of at most `depth` more operations; how many
/// sequences were applied in full.
fn explore(pair: &Pair, ops: &[Op], depth: usize) -> u64 {
    if depth == 0 {
        return 1;
    }
    let mut sequences = 0;
    for op in ops {
        if !pair.applicable(*op) {
            continue;
        }
        let mut next = pair.clone();
        next.apply(*op);
        sequences += explore(&next, ops, depth - 1);
    }
    sequences
}

#[test]
fn every_sequence_of_up_to_seven_operations_on_one_symbol_matches_the_model() {
    let sequences = explore(&Pair::default(), &alphabet(1), 7);
    println!("one symbol, two workers: {sequences} sequences of 7 operations");
    assert!(sequences > 100_000);
}

#[test]
fn every_sequence_of_up_to_six_operations_on_two_symbols_matches_the_model() {
    let sequences = explore(&Pair::default(), &alphabet(2), 6);
    println!("two symbols, two workers: {sequences} sequences of 6 operations");
    assert!(sequences > 100_000);
}

/// xorshift64*: a fixed, seeded sequence, the same on every run.
struct Random(u64);

impl Random {
    fn below(&mut self, bound: u64) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) % bound
    }
}

/// A store and pipeline reduced to what scheduling sees: which version of each symbol
/// has a fresh SIR. A finished task stores its version when that is still the latest
/// (the persist-time source check); a symbol's SIR goes when the symbol does.
#[derive(Default)]
struct Simulation {
    pair: Pair,
    stored: HashMap<usize, u8>,
    /// Scheduled and not since abandoned (removed, given up on, or failed by the
    /// provider on its latest version): these must end with a SIR for that version.
    must_be_current: BTreeSet<usize>,
    starts: u64,
}

impl Simulation {
    fn latest(&self, id: usize) -> Option<u8> {
        self.pair.model.version.get(&id).copied()
    }

    fn schedule(&mut self, id: usize, version: u8, score: f64) {
        self.pair.apply(Op::Schedule { id, version, score });
        self.must_be_current.insert(id);
    }

    fn remove(&mut self, id: usize) {
        self.pair.apply(Op::Remove { id });
        self.stored.remove(&id);
        self.must_be_current.remove(&id);
    }

    fn start(&mut self, worker: usize) -> bool {
        self.pair.apply(Op::Start { worker });
        let started = self.pair.workers[worker].is_some();
        self.starts += u64::from(started);
        started
    }

    /// Complete the worker's task as a success, a provider failure (the task finishes
    /// and stores nothing) or an error.
    // Models a deliberately non-storing Finished outcome, not a provider failure.
    fn complete(&mut self, worker: usize, finished_without_storage: bool, errored: bool) {
        let task = self.pair.workers[worker].clone().expect("a busy worker");
        let id = index_of(&task.symbol.id);
        let version = version_of(&task.symbol);
        let ran_latest = self.latest(id) == Some(version);
        if !finished_without_storage && !errored && ran_latest {
            self.stored.insert(id, version);
        }
        let current = self.latest(id).is_some() && self.stored.get(&id).copied() == self.latest(id);
        let completion = self
            .pair
            .apply(Op::Complete {
                worker,
                errored,
                current,
            })
            .expect("a completion");
        let abandoned = match completion {
            Completion::GaveUp => true,
            Completion::Done => finished_without_storage && ran_latest && !current,
            _ => false,
        };
        if abandoned {
            self.must_be_current.remove(&id);
        }
    }

    /// Edits have stopped and every task now succeeds: run until nothing is queued or
    /// in flight, within a bound, then require every symbol still owed a SIR to hold
    /// one for its latest version.
    fn quiesce(&mut self, seed: u64) {
        for worker in 0..WORKERS {
            if self.pair.workers[worker].is_some() {
                self.complete(worker, false, false);
            }
        }
        let mut turns = 0;
        while self.start(0) {
            self.complete(0, false, false);
            turns += 1;
            assert!(
                turns <= 2 * IDS.len(),
                "seed {seed}: tasks keep being queued after the edits stopped"
            );
        }
        assert_eq!(self.pair.real.queued_len(), 0, "seed {seed}");
        assert_eq!(self.pair.real.in_flight_len(), 0, "seed {seed}");
        for id in &self.must_be_current {
            assert_eq!(
                self.stored.get(id).copied(),
                self.latest(*id),
                "seed {seed}: {} was scheduled but ended without a SIR for its latest version",
                IDS[*id]
            );
        }
    }
}

#[test]
fn seeded_finished_histories_preserve_nonabandoned_latest_versions() {
    const RUNS: u64 = 3_000;
    const STEPS: u64 = 80;
    let mut starts = 0;
    for seed in 1..=RUNS {
        let mut random = Random(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut simulation = Simulation::default();
        for _ in 0..STEPS {
            let id = random.below(IDS.len() as u64) as usize;
            let worker = random.below(WORKERS as u64) as usize;
            match random.below(10) {
                0..=2 => {
                    let version = random.below(3) as u8;
                    let score = [0.1, 0.5, 0.9][random.below(3) as usize];
                    simulation.schedule(id, version, score);
                }
                3 => {
                    simulation.pair.apply(Op::Request { id });
                }
                4 => simulation.remove(id),
                5 | 6 => {
                    if simulation.pair.workers[worker].is_none() {
                        simulation.start(worker);
                    }
                }
                7 => {
                    if simulation.pair.last_completed.is_some() {
                        simulation.pair.apply(Op::CompleteStale);
                    }
                }
                _ => {
                    if simulation.pair.workers[worker].is_some() {
                        let kind = random.below(5);
                        simulation.complete(worker, kind == 0, kind == 1);
                    }
                }
            }
        }
        simulation.quiesce(seed);
        starts += simulation.starts;
    }
    println!("{RUNS} seeded histories of {STEPS} operations, {starts} tasks started");
}

fn finished(scheduler: &mut SemanticScheduler, task: &SymbolTask, current: bool) -> Completion {
    scheduler.complete(task, TaskOutcome::Finished, |_| current)
}

#[test]
fn an_edit_during_a_flight_is_queued_when_the_flight_completes() {
    let mut scheduler = SemanticScheduler::default();
    assert_eq!(
        scheduler.schedule(version_symbol("sym-a", 1), 0.5),
        Scheduled::Queued
    );
    let first = scheduler.start_next().expect("first task");
    assert_eq!(version_of(&first.symbol), 1);

    assert_eq!(
        scheduler.schedule(version_symbol("sym-a", 2), 0.5),
        Scheduled::Coalesced
    );
    assert!(!scheduler.is_queued("sym-a"), "one flight per symbol");
    assert!(scheduler.start_next().is_none(), "one flight per symbol");

    assert_eq!(
        finished(&mut scheduler, &first, false),
        Completion::FollowUpQueued
    );
    let second = scheduler.start_next().expect("follow-up task");
    assert_eq!(
        version_of(&second.symbol),
        2,
        "the follow-up runs the latest"
    );
    assert_eq!(finished(&mut scheduler, &second, true), Completion::Done);
    assert!(scheduler.start_next().is_none());
}

#[test]
fn many_edits_during_a_flight_leave_one_follow_up_for_the_latest_version() {
    let mut scheduler = SemanticScheduler::default();
    scheduler.schedule(version_symbol("sym-a", 0), 0.5);
    let first = scheduler.start_next().expect("first task");
    for version in 1..=200 {
        assert_eq!(
            scheduler.schedule(version_symbol("sym-a", version), 0.5),
            Scheduled::Coalesced
        );
        scheduler.bump_to_front("sym-a");
        assert_eq!(scheduler.queued_len(), 0);
    }
    assert_eq!(
        finished(&mut scheduler, &first, false),
        Completion::FollowUpQueued
    );
    assert_eq!(scheduler.queued_len(), 1);
    assert!(scheduler.queue_heap_len() <= 2 + 32);
    let second = scheduler.start_next().expect("follow-up task");
    assert_eq!(version_of(&second.symbol), 200);
    assert_eq!(
        second.score, FRONT_SCORE,
        "the request moved it to the front"
    );
    assert_eq!(finished(&mut scheduler, &second, true), Completion::Done);
    assert!(scheduler.start_next().is_none(), "exactly one follow-up");
}

#[test]
fn a_follow_up_is_skipped_when_the_store_already_describes_the_latest_version() {
    let mut scheduler = SemanticScheduler::default();
    scheduler.schedule(version_symbol("sym-a", 1), 0.5);
    let task = scheduler.start_next().expect("task");
    scheduler.schedule(version_symbol("sym-a", 2), 0.5);
    scheduler.schedule(version_symbol("sym-a", 1), 0.5);

    let mut asked = None;
    let completion = scheduler.complete(&task, TaskOutcome::Finished, |latest| {
        asked = Some(version_of(latest));
        true
    });
    assert_eq!(completion, Completion::Done);
    assert_eq!(
        asked,
        Some(1),
        "the store is asked about the latest version"
    );
    assert!(scheduler.start_next().is_none());
}

#[test]
fn a_late_completion_cannot_clear_the_work_of_a_recreated_symbol() {
    let mut scheduler = SemanticScheduler::default();
    scheduler.schedule(version_symbol("sym-a", 1), 0.5);
    let old = scheduler.start_next().expect("old task");

    scheduler.remove_symbol("sym-a");
    assert_eq!(
        scheduler.schedule(version_symbol("sym-a", 3), 0.5),
        Scheduled::Coalesced,
        "the recreated symbol waits for the flight its id still has"
    );
    assert!(
        scheduler.start_next().is_none(),
        "a second worker must not run the id beside its old flight"
    );

    assert_eq!(
        finished(&mut scheduler, &old, false),
        Completion::FollowUpQueued
    );
    let recreated = scheduler.start_next().expect("recreated task");
    assert_eq!(version_of(&recreated.symbol), 3);

    // The old task completing again (or any task with a ticket not in flight) changes
    // nothing for the flight the id has now.
    assert_eq!(
        finished(&mut scheduler, &old, false),
        Completion::StaleTicket
    );
    assert_eq!(
        scheduler.flight("sym-a").map(|(ticket, _)| ticket),
        Some(recreated.ticket)
    );
    assert_eq!(
        scheduler.schedule(version_symbol("sym-a", 4), 0.5),
        Scheduled::Coalesced,
        "the recreated symbol is still in flight"
    );
}

#[test]
fn a_removed_symbol_gets_no_follow_up() {
    let mut scheduler = SemanticScheduler::default();
    scheduler.schedule(version_symbol("sym-a", 1), 0.5);
    let task = scheduler.start_next().expect("task");
    scheduler.schedule(version_symbol("sym-a", 2), 0.5);
    scheduler.remove_symbol("sym-a");

    assert_eq!(
        scheduler.complete(&task, TaskOutcome::Errored, |_| false),
        Completion::Done
    );
    assert!(scheduler.start_next().is_none());
    assert!(!scheduler.bump_to_front("sym-a"));
}

#[test]
fn an_erroring_task_is_retried_a_bounded_number_of_times_behind_other_symbols() {
    let mut scheduler = SemanticScheduler::default();
    scheduler.schedule(version_symbol("sym-a", 1), 0.9);
    scheduler.schedule(version_symbol("sym-b", 1), 0.1);

    let mut order = Vec::new();
    let mut completions = Vec::new();
    while let Some(task) = scheduler.start_next() {
        order.push(task.symbol.id.clone());
        let outcome = if task.symbol.id == "sym-a" {
            TaskOutcome::Errored
        } else {
            TaskOutcome::Finished
        };
        completions.push(scheduler.complete(&task, outcome, |_| false));
        assert!(order.len() <= 8, "the erroring symbol is requeued forever");
    }
    assert_eq!(
        order,
        ["sym-a", "sym-b", "sym-a", "sym-a"],
        "the retry goes behind the other symbol"
    );
    assert_eq!(
        completions,
        [
            Completion::RetryQueued { attempt: 1 },
            Completion::Done,
            Completion::RetryQueued { attempt: 2 },
            Completion::GaveUp,
        ]
    );

    // The next edit gives the symbol a new set of attempts.
    assert_eq!(
        scheduler.schedule(version_symbol("sym-a", 2), 0.9),
        Scheduled::Queued
    );
    let task = scheduler.start_next().expect("task after the edit");
    assert_eq!(
        scheduler.complete(&task, TaskOutcome::Errored, |_| false),
        Completion::RetryQueued { attempt: 1 }
    );
}

#[test]
fn an_edit_during_an_erroring_flight_replaces_the_retry() {
    let mut scheduler = SemanticScheduler::default();
    scheduler.schedule(version_symbol("sym-a", 1), 0.5);
    let task = scheduler.start_next().expect("task");
    scheduler.schedule(version_symbol("sym-a", 2), 0.5);
    assert_eq!(
        scheduler.complete(&task, TaskOutcome::Errored, |_| false),
        Completion::FollowUpQueued
    );
    assert_eq!(scheduler.retries("sym-a"), 0);
    let task = scheduler.start_next().expect("follow-up");
    assert_eq!(version_of(&task.symbol), 2);
}

#[test]
fn duplicate_schedules_and_requests_never_start_a_second_flight() {
    let mut scheduler = SemanticScheduler::default();
    for _ in 0..3 {
        scheduler.schedule(version_symbol("sym-a", 1), 0.5);
        scheduler.bump_to_front("sym-a");
    }
    assert_eq!(scheduler.queued_len(), 1);
    let first_worker = scheduler.start_next().expect("the one task");
    assert!(
        scheduler.start_next().is_none(),
        "the second worker finds nothing"
    );

    // A request for a symbol in flight is served by that flight.
    assert!(!scheduler.bump_to_front("sym-a"));
    assert!(scheduler.start_next().is_none());
    assert_eq!(
        finished(&mut scheduler, &first_worker, true),
        Completion::Done
    );
    assert!(scheduler.start_next().is_none());
}

/// Real threads over the shared state: a watcher scheduling bursts of edits and four
/// workers. Which interleavings occur varies from run to run, so this samples them
/// rather than enumerating them; what it asserts holds for all of them: no symbol is
/// ever in two flights, and whenever the edits pause and the work runs out, every
/// symbol has its latest version stored.
#[test]
fn threads_sharing_the_scheduler_keep_one_flight_per_symbol_and_lose_no_edit() {
    use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use super::worker::SharedQueueState;

    const SYMBOLS: usize = 6;
    const ROUNDS: u8 = 50;
    const BURST: u8 = 5;
    const THREADS: usize = 4;
    let ids = (0..SYMBOLS)
        .map(|index| format!("sym-{index}"))
        .collect::<Vec<_>>();
    let index_of = |symbol_id: &str| {
        ids.iter()
            .position(|id| id == symbol_id)
            .expect("a known symbol")
    };
    let state = SharedQueueState::new(HashMap::new());
    // The version on disk, the version with a stored SIR (0: none), flights per symbol.
    let latest = (0..SYMBOLS).map(|_| AtomicU8::new(0)).collect::<Vec<_>>();
    let stored = (0..SYMBOLS).map(|_| AtomicU8::new(0)).collect::<Vec<_>>();
    let active = (0..SYMBOLS)
        .map(|_| AtomicUsize::new(0))
        .collect::<Vec<_>>();
    let stop = AtomicBool::new(false);
    let tasks = AtomicUsize::new(0);

    std::thread::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|| {
                while !stop.load(Ordering::SeqCst) {
                    let Some(task) = state.pop_task() else {
                        std::thread::yield_now();
                        continue;
                    };
                    let id = index_of(&task.symbol.id);
                    let version = version_of(&task.symbol);
                    let flights = active[id].fetch_add(1, Ordering::SeqCst);
                    tasks.fetch_add(1, Ordering::SeqCst);
                    std::thread::yield_now();
                    // The persist-time source check: store only what is still on disk.
                    if latest[id].load(Ordering::SeqCst) == version {
                        stored[id].store(version, Ordering::SeqCst);
                    }
                    active[id].fetch_sub(1, Ordering::SeqCst);
                    state.complete_task(&task, TaskOutcome::Finished, |symbol| {
                        stored[id].load(Ordering::SeqCst) == version_of(symbol)
                    });
                    if flights != 0 {
                        stop.store(true, Ordering::SeqCst);
                        panic!("{} was in two flights at once", task.symbol.id);
                    }
                }
            });
        }

        let mut version = 0;
        let mut failure = None;
        'rounds: for round in 0..ROUNDS {
            for _ in 0..BURST {
                version += 1;
                for (id, symbol_id) in ids.iter().enumerate() {
                    // The edit reaches the disk first, the watcher's event after it.
                    latest[id].store(version, Ordering::SeqCst);
                    state.schedule(version_symbol(symbol_id, version), 0.5);
                }
            }
            // The edits pause: wait (bounded only as a hang guard) for the work to run
            // out. With nothing queued or in flight, no task can appear on its own.
            let waiting_since = Instant::now();
            while state.pending_work() != (0, 0) {
                if waiting_since.elapsed() > Duration::from_secs(120) {
                    failure = Some(format!("round {round}: the work never ran out"));
                    break 'rounds;
                }
                std::thread::yield_now();
            }
            for (id, symbol_id) in ids.iter().enumerate() {
                if stored[id].load(Ordering::SeqCst) != version {
                    failure = Some(format!(
                        "round {round}: {symbol_id} is idle without its latest version stored"
                    ));
                    break 'rounds;
                }
            }
        }
        stop.store(true, Ordering::SeqCst);
        if let Some(failure) = failure {
            panic!("{failure}");
        }
    });

    let edits = usize::from(ROUNDS) * usize::from(BURST) * SYMBOLS;
    let tasks = tasks.load(Ordering::SeqCst);
    println!("{edits} edits over {SYMBOLS} symbols ran as {tasks} tasks on {THREADS} threads");
    assert!(tasks <= edits);
}

#[test]
fn provider_retry_deadlines_cap_and_recovery_are_clock_driven() {
    use std::time::{Duration, Instant};
    let mut scheduler = SemanticScheduler::default();
    let mut now = Instant::now();
    scheduler.schedule(version_symbol("a", 1), 1.0);
    let mut task = scheduler.start_next_at(now).unwrap();
    for attempt in 1..=12 {
        let delay = Duration::from_secs((5_u64 << (attempt - 1).min(6)).min(300));
        assert_eq!(
            scheduler.complete_at(&task, TaskOutcome::ProviderFailed, |_| false, now),
            Completion::ProviderRetryScheduled { attempt, delay }
        );
        assert_eq!(scheduler.delayed_len(), 1);
        for _ in 0..8 {
            assert!(!scheduler.bump_to_front("a"));
        }
        assert_eq!(scheduler.delayed_len(), 1);
        assert!(scheduler.deadlines_coherent());
        assert!(
            scheduler
                .start_next_at(now + delay - Duration::from_nanos(1))
                .is_none()
        );
        now += delay;
        task = scheduler.start_next_at(now).unwrap();
        assert!(task.is_provider_retry);
    }
    assert_eq!(
        scheduler.complete_at(&task, TaskOutcome::Finished, |_| true, now),
        Completion::Done
    );
    assert_eq!(scheduler.delayed_len(), 0);
}

#[test]
fn provider_retry_latest_delete_current_and_stale_ticket() {
    use std::time::{Duration, Instant};
    let now = Instant::now();
    let mut s = SemanticScheduler::default();
    s.schedule(version_symbol("a", 1), 1.0);
    let old = s.start_next_at(now).unwrap();
    s.complete_at(&old, TaskOutcome::ProviderFailed, |_| false, now);
    s.schedule(version_symbol("a", 2), 1.0);
    assert_eq!(s.delayed_len(), 0);
    let new = s.start_next_at(now).unwrap();
    assert_eq!(version_of(&new.symbol), 2);
    assert!(!new.is_provider_retry);
    assert_eq!(
        s.complete_at(&old, TaskOutcome::ProviderFailed, |_| false, now),
        Completion::StaleTicket
    );
    assert_eq!(
        s.complete_at(&new, TaskOutcome::ProviderFailed, |_| true, now),
        Completion::Done
    );
    s.schedule(version_symbol("a", 3), 1.0);
    let task = s.start_next_at(now).unwrap();
    s.complete_at(&task, TaskOutcome::ProviderFailed, |_| false, now);
    s.remove_symbol("a");
    assert_eq!(s.delayed_len(), 0);
    assert!(s.start_next_at(now + Duration::from_secs(600)).is_none());
}

#[test]
fn provider_retries_are_fair_and_globally_spaced_without_blocking_fresh_work() {
    use std::time::{Duration, Instant};
    let now = Instant::now();
    let mut s = SemanticScheduler::default();
    for id in ["a", "b"] {
        s.schedule(version_symbol(id, 1), 1.0);
        let task = s.start_next_at(now).unwrap();
        s.complete_at(&task, TaskOutcome::ProviderFailed, |_| false, now);
    }
    let due = now + Duration::from_secs(5);
    s.schedule(version_symbol("fresh", 1), 1.0);
    let retry = s.start_next_at(due).unwrap();
    assert!(retry.is_provider_retry);
    let fresh = s.start_next_at(due).unwrap();
    assert_eq!(fresh.symbol.id, "fresh");
    assert!(s.start_next_at(due).is_none());
    let next = s.start_next_at(due + Duration::from_secs(1)).unwrap();
    assert!(next.is_provider_retry);
    assert_ne!(retry.symbol.id, next.symbol.id);
}

#[test]
fn seeded_provider_outages_converge_after_quiescence_without_rescheduling() {
    use std::time::{Duration, Instant};
    for seed in 1_u64..=16 {
        let mut rng = seed;
        let mut now = Instant::now();
        let mut s = SemanticScheduler::default();
        let mut latest = HashMap::new();
        let mut current = HashMap::new();
        for step in 0..500 {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            let id = format!("sym-{}", (rng >> 32) % 16);
            match rng % 5 {
                0 => {
                    s.remove_symbol(&id);
                    latest.remove(&id);
                    current.remove(&id);
                }
                1 | 2 => {
                    let version = (step % 250) as u8;
                    s.schedule(version_symbol(&id, version), 1.0);
                    latest.insert(id, version);
                }
                _ => {}
            }
            if let Some(task) = s.start_next_at(now) {
                let failed = rng % 7 != 0;
                if !failed {
                    current.insert(task.symbol.id.clone(), version_of(&task.symbol));
                }
                s.complete_at(
                    &task,
                    if failed {
                        TaskOutcome::ProviderFailed
                    } else {
                        TaskOutcome::Finished
                    },
                    |symbol| current.get(&symbol.id) == Some(&version_of(symbol)),
                    now,
                );
            }
            assert!(s.deadlines_coherent());
            assert!(s.delayed_len() <= latest.len());
            now += Duration::from_secs(1);
        }
        // No more edits or requests. Healthy providers must discharge all extant debt.
        for _ in 0..1000 {
            now += Duration::from_secs(1);
            if let Some(task) = s.start_next_at(now) {
                current.insert(task.symbol.id.clone(), version_of(&task.symbol));
                s.complete_at(&task, TaskOutcome::Finished, |_| true, now);
            }
            assert!(s.deadlines_coherent());
        }
        assert_eq!(s.delayed_len(), 0);
        assert_eq!(s.queued_len(), 0);
        for (id, version) in latest {
            assert_eq!(current.get(&id), Some(&version));
        }
    }
}
