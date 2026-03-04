use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

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

#[derive(Debug, Default)]
pub struct SirPriorityQueue {
    heap: BinaryHeap<QueueEntry>,
    enqueued: HashSet<String>,
    scores: HashMap<String, f64>,
    sequence: u64,
}

impl SirPriorityQueue {
    pub fn push(&mut self, symbol_id: String, score: f64) -> bool {
        if symbol_id.trim().is_empty() || !score.is_finite() {
            return false;
        }
        if self.enqueued.contains(symbol_id.as_str()) {
            return false;
        }

        let normalized = score.clamp(0.0, 1.0);
        self.enqueued.insert(symbol_id.clone());
        self.scores.insert(symbol_id.clone(), normalized);
        self.sequence = self.sequence.saturating_add(1);
        self.heap.push(QueueEntry {
            score: normalized,
            sequence: self.sequence,
            symbol_id,
        });
        true
    }

    pub fn bump_to_front(&mut self, symbol_id: &str) -> bool {
        if !self.enqueued.contains(symbol_id) {
            return false;
        }

        self.scores.insert(symbol_id.to_owned(), f64::MAX);
        self.sequence = self.sequence.saturating_add(1);
        self.heap.push(QueueEntry {
            score: f64::MAX,
            sequence: self.sequence,
            symbol_id: symbol_id.to_owned(),
        });
        true
    }

    pub fn pop(&mut self) -> Option<(f64, String)> {
        while let Some(entry) = self.heap.pop() {
            let Some(current_score) = self.scores.get(entry.symbol_id.as_str()) else {
                continue;
            };
            if current_score.to_bits() != entry.score.to_bits() {
                continue;
            }
            self.scores.remove(entry.symbol_id.as_str());
            self.enqueued.remove(entry.symbol_id.as_str());
            return Some((entry.score, entry.symbol_id));
        }
        None
    }

    pub fn remove(&mut self, symbol_id: &str) -> bool {
        self.scores.remove(symbol_id).is_some() && self.enqueued.remove(symbol_id)
    }

    pub fn contains(&self, symbol_id: &str) -> bool {
        self.enqueued.contains(symbol_id)
    }

    pub fn len(&self) -> usize {
        self.enqueued.len()
    }

    pub fn is_empty(&self) -> bool {
        self.enqueued.is_empty()
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
