//! The live semantic scheduler: which version of which symbol a worker runs next.
//!
//! The watcher thread reports edits and removals, any number of worker threads start and
//! complete tasks. All of them go through one [`SemanticScheduler`] behind one mutex, so
//! every transition below is atomic and the scheduler's history is some sequential order
//! of them. The type does no I/O and takes no lock itself.
//!
//! # State, per symbol id
//!
//! * `symbols[id]`: the latest version of the symbol the indexer has observed.
//! * queued: `id` waits in the priority queue (at most once).
//! * `in_flight[id]`: a worker is running a task for `id`, identified by a ticket no
//!   other task ever gets. `follow_up` records that the symbol was scheduled again
//!   since that task started, and at what priority.
//! * `retries[id]`: how many times in a row the symbol's task has errored.
//!
//! Provider failures have a separate delayed obligation per symbol, indexed by deadline.
//! Cooldown grows from 5 seconds to a 300 second cap and never abandons the obligation.
//! Recovery task starts are globally spaced by at least one second; when both kinds of
//! work are ready, recovery and ordinary tasks alternate. This bounds recovery starts,
//! not provider calls within a task or first attempts. Explicit requests preserve the
//! cooldown so repeated reads cannot defeat pacing. Edits and removals cancel old debt.
//!
//! # Transitions
//!
//! * **observe** (`upsert_symbol`): `symbols[id]` becomes the given version.
//! * **schedule** (an edit, a symbol found without a current SIR): observes the version,
//!   forgets earlier errors, then
//!   - in flight: sets `follow_up` (keeping the higher priority if one is already set);
//!     any number of edits during one flight leave this one mark;
//!   - queued: nothing more, the queued task will carry the version just observed;
//!   - otherwise: queues the id.
//! * **request** (`bump_to_front`, an explicit SIR request): moves a queued id to the
//!   front; queues an idle known id at the front; for an id in flight, only raises a
//!   follow-up that is already pending (the flight is already serving the request).
//! * **start** (`start_next`): takes the highest-priority queued id and puts it in
//!   flight with a fresh ticket, handing out the latest observed version.
//! * **complete**: ends the flight the ticket belongs to (any other ticket is ignored).
//!   - `follow_up` set: the id is queued again unless the symbol is gone or the store
//!     already holds a fresh SIR for the latest observed version (`is_current`). This
//!     does not depend on how the task ended: a result rejected as stale, a provider
//!     failure and an error all leave the newer version owed its own task.
//!   - otherwise, finished: done. Errored: queued again behind every prioritised symbol,
//!     at most [`MAX_TASK_RETRIES`] times in a row; after that the symbol is dropped
//!     until it is scheduled or requested again (or the daemon restarts).
//! * **remove**: forgets the symbol, its queue entry, its errors and any pending
//!   follow-up. A flight for the id is left in place: the worker is still running, and
//!   keeping its entry is what makes a symbol recreated under the same id wait for (and
//!   be scheduled after) that flight instead of running beside it.
//!
//! # Invariants (checked against a reference model in `scheduler_tests`)
//!
//! 1. An id is never queued and in flight at once, and never in flight twice.
//! 2. Every queued id is a known symbol; a started task carries its latest version.
//! 3. No scheduled version is dropped: from `schedule` until a task starts with that
//!    version (or the symbol is removed, or the store is found current), the id is
//!    queued or has a pending follow-up.
//! 4. A completion only ever ends its own flight.
//! 5. `retries[id] <= MAX_TASK_RETRIES`, and only for ids that are queued or in flight.
//! 6. The queue holds at most one entry per known symbol, whatever the number of edits.

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

use aether_core::Symbol;

use crate::priority_queue::SirPriorityQueue;

/// How many times in a row a symbol's task is run again after it errored.
pub(super) const MAX_TASK_RETRIES: u32 = 2;
/// Where a retry is queued: behind every symbol with a computed priority.
const RETRY_SCORE: f64 = 0.0;
/// The score the queue gives an id bumped to its front.
pub(super) const FRONT_SCORE: f64 = f64::MAX;

/// What `schedule` did with a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Scheduled {
    /// Newly queued.
    Queued,
    /// Already queued; the queued task will carry this version.
    AlreadyQueued,
    /// In flight; a follow-up for this version runs once the flight completes.
    Coalesced,
}

/// How a symbol task ended, as far as scheduling is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TaskOutcome {
    /// The pipeline ran the task to its end (whether or not a SIR was stored).
    Finished,
    /// The task could not be run to its end.
    Errored,
    /// Provider generation failed; keep retrying with a bounded cooldown.
    ProviderFailed,
}

/// What `complete` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Completion {
    /// Nothing further is scheduled for the symbol.
    Done,
    /// The symbol was scheduled again during the flight and is queued for its latest
    /// version.
    FollowUpQueued,
    /// The task errored and is queued for retry number `attempt`.
    RetryQueued {
        attempt: u32,
    },
    /// The task errored once more than it is retried; the symbol was dropped.
    GaveUp,
    ProviderRetryScheduled {
        attempt: u32,
        delay: Duration,
    },
    /// The ticket is not the one in flight for the symbol; nothing changed.
    StaleTicket,
}

/// A symbol task handed to a worker.
#[derive(Debug, Clone)]
pub(super) struct SymbolTask {
    /// Identifies this flight to `complete`.
    pub(super) ticket: u64,
    pub(super) is_provider_retry: bool,
    pub(super) score: f64,
    /// The latest observed version of the symbol when the task started.
    pub(super) symbol: Symbol,
}

#[derive(Debug, Clone)]
struct Flight {
    ticket: u64,
    /// The priority of the follow-up owed to the symbol, if it was scheduled again
    /// since the flight started.
    follow_up: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct SemanticScheduler {
    queue: SirPriorityQueue,
    symbols: HashMap<String, Symbol>,
    in_flight: HashMap<String, Flight>,
    retries: HashMap<String, u32>,
    last_ticket: u64,
    provider_attempts: HashMap<String, u32>,
    delayed: HashMap<String, Instant>,
    deadlines: BTreeSet<(Instant, String)>,
    next_provider_start: Option<Instant>,
    last_start_was_provider: bool,
}

impl SemanticScheduler {
    pub(super) fn new(symbols: HashMap<String, Symbol>) -> Self {
        Self {
            symbols,
            ..Self::default()
        }
    }

    /// Record `symbol` as the latest observed version of its id.
    pub(super) fn upsert_symbol(&mut self, symbol: Symbol) {
        self.symbols.insert(symbol.id.clone(), symbol);
    }

    /// Record `symbol` as the latest observed version of its id and make sure a task
    /// runs for it.
    pub(super) fn schedule(&mut self, symbol: Symbol, score: f64) -> Scheduled {
        let score = if score.is_finite() {
            score.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let symbol_id = symbol.id.clone();
        self.symbols.insert(symbol_id.clone(), symbol);
        self.retries.remove(symbol_id.as_str());
        self.clear_delay(symbol_id.as_str());
        self.provider_attempts.remove(symbol_id.as_str());

        if let Some(flight) = self.in_flight.get_mut(symbol_id.as_str()) {
            flight.follow_up = Some(match flight.follow_up {
                Some(pending) => pending.max(score),
                None => score,
            });
            return Scheduled::Coalesced;
        }
        if self.queue.push(symbol_id, score) {
            Scheduled::Queued
        } else {
            Scheduled::AlreadyQueued
        }
    }

    /// Move `symbol_id` to the front of the queue, queueing it if it is known and idle.
    /// `false`: the id is unknown, or in flight (a pending follow-up is moved to the
    /// front instead).
    pub(super) fn bump_to_front(&mut self, symbol_id: &str) -> bool {
        if let Some(flight) = self.in_flight.get_mut(symbol_id) {
            if let Some(pending) = flight.follow_up.as_mut() {
                *pending = FRONT_SCORE;
            }
            return false;
        }
        // Explicit requests do not bypass provider cooldown.
        if self.delayed.contains_key(symbol_id) {
            return false;
        }
        if self.queue.bump_to_front(symbol_id) {
            return true;
        }
        if !self.symbols.contains_key(symbol_id) {
            return false;
        }
        self.enqueue(symbol_id, FRONT_SCORE)
    }

    /// Start the highest-priority queued symbol: it is in flight until `complete` is
    /// called with the returned task.
    #[cfg(test)]
    pub(super) fn start_next(&mut self) -> Option<SymbolTask> {
        self.start_next_at(Instant::now())
    }

    pub(super) fn start_next_at(&mut self, now: Instant) -> Option<SymbolTask> {
        loop {
            // Oldest due obligation gets service even under continuous fresh arrivals.
            // Space recovery starts globally; normal work remains available in between.
            let due = if (!self.last_start_was_provider || self.queue.is_empty())
                && self.next_provider_start.is_none_or(|next| now >= next)
            {
                self.deadlines
                    .first()
                    .filter(|(deadline, _)| *deadline <= now)
                    .map(|(_, id)| id.clone())
            } else {
                None
            };
            let is_provider_retry = due.is_some();
            let (score, symbol_id) = if let Some(id) = due {
                self.clear_delay(&id);
                self.next_provider_start = Some(now + Duration::from_secs(1));
                (RETRY_SCORE, id)
            } else {
                self.queue.pop()?
            };
            if let Some(flight) = self.in_flight.get_mut(symbol_id.as_str()) {
                // Invariant 1 rules this out. Should it ever happen, keep the work as
                // the flight's follow-up rather than drop it or run the symbol twice.
                flight.follow_up.get_or_insert(score);
                continue;
            }
            let Some(symbol) = self.symbols.get(symbol_id.as_str()).cloned() else {
                self.retries.remove(symbol_id.as_str());
                continue;
            };
            self.last_start_was_provider = is_provider_retry;
            self.last_ticket = self.last_ticket.saturating_add(1);
            self.in_flight.insert(
                symbol_id,
                Flight {
                    ticket: self.last_ticket,
                    follow_up: None,
                },
            );
            return Some(SymbolTask {
                ticket: self.last_ticket,
                is_provider_retry,
                score,
                symbol,
            });
        }
    }

    /// End `task`'s flight. `is_current` is asked, only when the symbol was scheduled
    /// again during the flight, whether the store already holds a fresh SIR for the
    /// given (latest observed) version.
    #[cfg(test)]
    pub(super) fn complete(
        &mut self,
        task: &SymbolTask,
        outcome: TaskOutcome,
        is_current: impl FnOnce(&Symbol) -> bool,
    ) -> Completion {
        self.complete_at(task, outcome, is_current, Instant::now())
    }

    pub(super) fn complete_at(
        &mut self,
        task: &SymbolTask,
        outcome: TaskOutcome,
        is_current: impl FnOnce(&Symbol) -> bool,
        now: Instant,
    ) -> Completion {
        let symbol_id = task.symbol.id.as_str();
        match self.in_flight.get(symbol_id) {
            Some(flight) if flight.ticket == task.ticket => {}
            _ => return Completion::StaleTicket,
        }
        let follow_up = self
            .in_flight
            .remove(symbol_id)
            .and_then(|flight| flight.follow_up);

        if let Some(score) = follow_up {
            self.provider_attempts.remove(symbol_id);
            self.retries.remove(symbol_id);
            let owed = self
                .symbols
                .get(symbol_id)
                .is_some_and(|latest| !is_current(latest));
            if owed {
                self.enqueue(symbol_id, score);
                return Completion::FollowUpQueued;
            }
            return Completion::Done;
        }

        if outcome == TaskOutcome::Finished || !self.symbols.contains_key(symbol_id) {
            self.retries.remove(symbol_id);
            self.provider_attempts.remove(symbol_id);
            return Completion::Done;
        }
        if outcome == TaskOutcome::ProviderFailed {
            self.retries.remove(symbol_id);
            if self.symbols.get(symbol_id).is_some_and(is_current) {
                self.provider_attempts.remove(symbol_id);
                return Completion::Done;
            }
            let attempt = self
                .provider_attempts
                .get(symbol_id)
                .copied()
                .unwrap_or(0)
                .saturating_add(1);
            let delay = Duration::from_secs((5_u64 << attempt.saturating_sub(1).min(6)).min(300));
            self.provider_attempts.insert(symbol_id.to_owned(), attempt);
            self.clear_delay(symbol_id);
            self.delayed.insert(symbol_id.to_owned(), now + delay);
            self.deadlines.insert((now + delay, symbol_id.to_owned()));
            return Completion::ProviderRetryScheduled { attempt, delay };
        }
        self.provider_attempts.remove(symbol_id);
        let attempt = self.retries.get(symbol_id).copied().unwrap_or(0) + 1;
        if attempt > MAX_TASK_RETRIES {
            self.retries.remove(symbol_id);
            return Completion::GaveUp;
        }
        self.retries.insert(symbol_id.to_owned(), attempt);
        self.enqueue(symbol_id, RETRY_SCORE);
        Completion::RetryQueued { attempt }
    }

    /// Forget a symbol the index no longer holds. A flight for its id stays in place
    /// until its worker completes it.
    pub(super) fn remove_symbol(&mut self, symbol_id: &str) {
        self.symbols.remove(symbol_id);
        self.clear_delay(symbol_id);
        self.provider_attempts.remove(symbol_id);
        self.queue.remove(symbol_id);
        self.retries.remove(symbol_id);
        if let Some(flight) = self.in_flight.get_mut(symbol_id) {
            flight.follow_up = None;
        }
    }

    pub(super) fn is_queued(&self, symbol_id: &str) -> bool {
        self.queue.contains(symbol_id)
    }

    #[cfg(test)]
    pub(super) fn delayed_len(&self) -> usize {
        self.delayed.len()
    }

    #[cfg(test)]
    pub(super) fn queued_len(&self) -> usize {
        self.queue.len()
    }

    #[cfg(test)]
    pub(super) fn in_flight_len(&self) -> usize {
        self.in_flight.len()
    }

    /// The ticket in flight for `symbol_id` and the priority of its pending follow-up.
    #[cfg(test)]
    pub(super) fn flight(&self, symbol_id: &str) -> Option<(u64, Option<f64>)> {
        self.in_flight
            .get(symbol_id)
            .map(|flight| (flight.ticket, flight.follow_up))
    }

    #[cfg(test)]
    pub(super) fn retries(&self, symbol_id: &str) -> u32 {
        self.retries.get(symbol_id).copied().unwrap_or(0)
    }

    #[cfg(test)]
    pub(super) fn retry_ids(&self) -> Vec<String> {
        self.retries.keys().cloned().collect()
    }

    #[cfg(test)]
    pub(super) fn symbol(&self, symbol_id: &str) -> Option<&Symbol> {
        self.symbols.get(symbol_id)
    }

    #[cfg(test)]
    pub(super) fn queue_heap_len(&self) -> usize {
        self.queue.heap_len()
    }

    fn clear_delay(&mut self, symbol_id: &str) {
        if let Some(deadline) = self.delayed.remove(symbol_id) {
            self.deadlines.remove(&(deadline, symbol_id.to_owned()));
        }
    }

    #[cfg(test)]
    pub(super) fn deadlines_coherent(&self) -> bool {
        self.deadlines.len() == self.delayed.len()
            && self
                .delayed
                .iter()
                .all(|(id, deadline)| self.deadlines.contains(&(*deadline, id.clone())))
    }

    /// Queue a known, idle id at `score` (`FRONT_SCORE`: at the front).
    fn enqueue(&mut self, symbol_id: &str, score: f64) -> bool {
        if score == FRONT_SCORE {
            let _ = self.queue.push(symbol_id.to_owned(), 1.0);
            self.queue.bump_to_front(symbol_id)
        } else {
            self.queue.push(symbol_id.to_owned(), score)
        }
    }
}
