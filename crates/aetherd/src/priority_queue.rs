use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

/// Heap entries a queue may hold beyond twice its live symbols before the entries that
/// no longer stand for a queued symbol are dropped.
const STALE_ENTRY_SLACK: usize = 32;

#[derive(Debug, Clone)]
struct QueueEntry {
    score: f64,
    sequence: u64,
    symbol_id: String,
}

impl PartialEq for QueueEntry {
    fn eq(&self, other: &Self) -> bool {
        self.score.to_bits() == other.score.to_bits()
            && self.sequence == other.sequence
            && self.symbol_id == other.symbol_id
    }
}

impl Eq for QueueEntry {}

impl PartialOrd for QueueEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for QueueEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .total_cmp(&other.score)
            .then_with(|| other.sequence.cmp(&self.sequence))
            .then_with(|| self.symbol_id.cmp(&other.symbol_id))
    }
}

/// The score and heap entry a queued symbol currently stands at.
#[derive(Debug, Clone, Copy)]
struct LiveEntry {
    score: f64,
    sequence: u64,
}

/// Symbol ids ordered by score (highest first, then first queued). A symbol is queued
/// at most once. Exactly one heap entry stands for each queued symbol, the one whose
/// sequence its `live` record names; the entries a bump or a removal leaves behind are
/// skipped when popped and dropped once they outnumber the live ones, so the heap stays
/// within `2 * len() + STALE_ENTRY_SLACK` entries however often symbols are bumped,
/// removed and queued again.
#[derive(Debug, Clone, Default)]
pub struct SirPriorityQueue {
    heap: BinaryHeap<QueueEntry>,
    live: HashMap<String, LiveEntry>,
    sequence: u64,
}

impl SirPriorityQueue {
    pub fn push(&mut self, symbol_id: String, score: f64) -> bool {
        if symbol_id.trim().is_empty() || !score.is_finite() {
            return false;
        }
        if self.live.contains_key(symbol_id.as_str()) {
            return false;
        }

        self.insert_entry(symbol_id, score.clamp(0.0, 1.0));
        true
    }

    pub fn bump_to_front(&mut self, symbol_id: &str) -> bool {
        match self.live.get(symbol_id) {
            None => return false,
            // Already at the front: a repeated request keeps its place and adds nothing.
            Some(entry) if entry.score.to_bits() == f64::MAX.to_bits() => return true,
            Some(_) => {}
        }

        self.insert_entry(symbol_id.to_owned(), f64::MAX);
        true
    }

    pub fn pop(&mut self) -> Option<(f64, String)> {
        while let Some(entry) = self.heap.pop() {
            if self.is_live(&entry) {
                self.live.remove(entry.symbol_id.as_str());
                self.shed_stale_entries();
                return Some((entry.score, entry.symbol_id));
            }
        }
        None
    }

    pub fn remove(&mut self, symbol_id: &str) -> bool {
        let removed = self.live.remove(symbol_id).is_some();
        self.shed_stale_entries();
        removed
    }

    pub fn contains(&self, symbol_id: &str) -> bool {
        self.live.contains_key(symbol_id)
    }

    pub fn len(&self) -> usize {
        self.live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// Heap entries held, live or not.
    #[cfg(test)]
    pub(crate) fn heap_len(&self) -> usize {
        self.heap.len()
    }

    fn is_live(&self, entry: &QueueEntry) -> bool {
        self.live
            .get(entry.symbol_id.as_str())
            .is_some_and(|live| live.sequence == entry.sequence)
    }

    /// Make a new heap entry the one that stands for `symbol_id`.
    fn insert_entry(&mut self, symbol_id: String, score: f64) {
        self.sequence = self.sequence.saturating_add(1);
        self.live.insert(
            symbol_id.clone(),
            LiveEntry {
                score,
                sequence: self.sequence,
            },
        );
        self.heap.push(QueueEntry {
            score,
            sequence: self.sequence,
            symbol_id,
        });
        self.shed_stale_entries();
    }

    /// Drop the heap entries that stand for no queued symbol once they outnumber the
    /// live ones by more than the slack. Called after every change, so the bound in the
    /// type's description always holds; each call that sheds is paid for by the
    /// `len() + STALE_ENTRY_SLACK` changes that left those entries behind.
    fn shed_stale_entries(&mut self) {
        if self.heap.len() > 2 * self.live.len() + STALE_ENTRY_SLACK {
            let live = &self.live;
            self.heap.retain(|entry| {
                live.get(entry.symbol_id.as_str())
                    .is_some_and(|current| current.sequence == entry.sequence)
            });
        }
    }
}

pub fn compute_priority_score(
    git_recency: f64,
    page_rank: f64,
    kind_priority: f64,
    size_inverse: f64,
) -> f64 {
    let weighted = 0.4 * git_recency + 0.3 * page_rank + 0.2 * kind_priority + 0.1 * size_inverse;
    weighted.clamp(0.0, 1.0)
}

pub fn kind_priority_score(kind: &str, is_public: bool) -> f64 {
    let kind = kind.trim().to_ascii_lowercase();
    let api_kind = matches!(
        kind.as_str(),
        "function" | "method" | "struct" | "trait" | "impl" | "class" | "interface"
    );
    if is_public && api_kind {
        return 1.0;
    }
    if matches!(
        kind.as_str(),
        "function" | "method" | "struct" | "class" | "interface"
    ) {
        return 0.7;
    }
    if matches!(
        kind.as_str(),
        "const" | "static" | "type" | "type_alias" | "variable"
    ) {
        return 0.5;
    }
    0.3
}

pub fn size_inverse_score(line_count: usize) -> f64 {
    1.0 - (line_count as f64 / 1_000.0).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_pop_returns_highest_score_first() {
        let mut queue = SirPriorityQueue::default();
        queue.push("sym-a".to_owned(), 0.2);
        queue.push("sym-b".to_owned(), 0.9);
        queue.push("sym-c".to_owned(), 0.5);

        assert_eq!(queue.pop(), Some((0.9, "sym-b".to_owned())));
        assert_eq!(queue.pop(), Some((0.5, "sym-c".to_owned())));
        assert_eq!(queue.pop(), Some((0.2, "sym-a".to_owned())));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn queue_bump_to_front_promotes_symbol() {
        let mut queue = SirPriorityQueue::default();
        queue.push("sym-a".to_owned(), 0.2);
        queue.push("sym-b".to_owned(), 0.3);

        assert!(queue.bump_to_front("sym-a"));
        let (score, symbol_id) = queue.pop().expect("popped");
        assert_eq!(symbol_id, "sym-a");
        assert_eq!(score, f64::MAX);
    }

    #[test]
    fn queue_prevents_duplicate_symbol_ids() {
        let mut queue = SirPriorityQueue::default();
        assert!(queue.push("sym-a".to_owned(), 0.2));
        assert!(!queue.push("sym-a".to_owned(), 0.9));
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn queue_remove_then_push_takes_the_new_place_not_the_old_one() {
        let mut queue = SirPriorityQueue::default();
        queue.push("sym-a".to_owned(), 0.9);
        queue.push("sym-b".to_owned(), 0.5);
        assert!(queue.remove("sym-a"));
        assert!(!queue.remove("sym-a"));
        queue.push("sym-a".to_owned(), 0.1);

        assert_eq!(queue.pop(), Some((0.5, "sym-b".to_owned())));
        assert_eq!(queue.pop(), Some((0.1, "sym-a".to_owned())));
        assert_eq!(queue.pop(), None);
        assert!(queue.is_empty());
    }

    #[test]
    fn queue_repeated_bumps_keep_one_place_and_add_no_entries() {
        let mut queue = SirPriorityQueue::default();
        queue.push("sym-a".to_owned(), 0.2);
        queue.push("sym-b".to_owned(), 0.3);
        assert!(queue.bump_to_front("sym-a"));
        assert!(queue.bump_to_front("sym-b"));
        let entries = queue.heap_len();
        for _ in 0..1_000 {
            assert!(queue.bump_to_front("sym-a"));
        }
        assert_eq!(queue.heap_len(), entries);

        // The first symbol bumped is still the first out.
        assert_eq!(queue.pop(), Some((f64::MAX, "sym-a".to_owned())));
        assert_eq!(queue.pop(), Some((f64::MAX, "sym-b".to_owned())));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn queue_heap_stays_bounded_under_remove_and_requeue_churn() {
        let mut queue = SirPriorityQueue::default();
        queue.push("sym-keep".to_owned(), 0.5);
        for round in 0..10_000 {
            queue.push("sym-churn".to_owned(), 0.4);
            if round % 2 == 0 {
                queue.bump_to_front("sym-churn");
            }
            queue.remove("sym-churn");
            assert!(queue.heap_len() <= 2 * queue.len() + STALE_ENTRY_SLACK);
        }
        // Emptying a long queue leaves no heap behind either.
        for index in 0..500 {
            queue.push(format!("sym-{index}"), 0.1);
        }
        for index in 0..500 {
            queue.remove(&format!("sym-{index}"));
            assert!(queue.heap_len() <= 2 * queue.len() + STALE_ENTRY_SLACK);
        }
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.pop(), Some((0.5, "sym-keep".to_owned())));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn queue_pop_returns_score_and_symbol_id_tuple() {
        let mut queue = SirPriorityQueue::default();
        queue.push("sym-a".to_owned(), 0.42);
        let (score, symbol_id) = queue.pop().expect("popped");
        assert_eq!(symbol_id, "sym-a");
        assert!((score - 0.42).abs() < f64::EPSILON);
    }

    #[test]
    fn compute_priority_score_uses_weighted_sum() {
        let score = compute_priority_score(1.0, 0.5, 0.25, 0.0);
        assert!((score - 0.6).abs() < 1e-9);
    }

    #[test]
    fn kind_priority_score_returns_expected_values() {
        assert_eq!(kind_priority_score("function", true), 1.0);
        assert_eq!(kind_priority_score("function", false), 0.7);
        assert_eq!(kind_priority_score("type_alias", false), 0.5);
        assert_eq!(kind_priority_score("enum", false), 0.3);
    }
}
