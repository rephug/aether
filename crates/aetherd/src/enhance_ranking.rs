use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};

use aether_store::SymbolRecord;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum MatchRank {
    Id,
    QualifiedName,
    OwnerQualified,
    Leaf,
    Fuzzy,
}

pub(super) fn match_rank(query: &str, record: &SymbolRecord) -> MatchRank {
    if query == record.id {
        return MatchRank::Id;
    }
    let query = query.replace("::", ".");
    let name = record.qualified_name.replace("::", ".");
    if name == query {
        MatchRank::QualifiedName
    } else if query.contains('.') && name.ends_with(&format!(".{query}")) {
        MatchRank::OwnerQualified
    } else if !query.contains('.') && name.rsplit('.').next() == Some(query.as_str()) {
        MatchRank::Leaf
    } else {
        MatchRank::Fuzzy
    }
}

pub(super) fn compare_records(left: &SymbolRecord, right: &SymbolRecord) -> Ordering {
    left.qualified_name
        .replace("::", ".")
        .cmp(&right.qualified_name.replace("::", "."))
        .then_with(|| left.file_path.cmp(&right.file_path))
        .then_with(|| left.kind.cmp(&right.kind))
        .then_with(|| left.id.cmp(&right.id))
}

/// Bound the admitted set deterministically, rather than by prompt position.
pub(super) fn admitted_queries(queries: &[String], limit: usize) -> (Vec<&str>, bool) {
    let unique = queries
        .iter()
        .map(|query| query.trim())
        .filter(|query| !query.is_empty())
        .collect::<BTreeSet<_>>();
    let truncated = unique.len() > limit;
    (unique.into_iter().take(limit).collect(), truncated)
}

/// Keep each symbol's strongest provenance before applying the output budget.
pub(super) fn select_ranked_symbols(
    candidates: Vec<(SymbolRecord, MatchRank)>,
    limit: usize,
) -> Vec<SymbolRecord> {
    let mut best = HashMap::<String, (SymbolRecord, MatchRank)>::new();
    for (record, rank) in candidates {
        if let Some((previous, previous_rank)) = best.get_mut(&record.id) {
            if rank < *previous_rank
                || (rank == *previous_rank && compare_records(&record, previous).is_lt())
            {
                *previous = record;
                *previous_rank = rank;
            }
        } else {
            best.insert(record.id.clone(), (record, rank));
        }
    }
    let mut ranked = best.into_values().collect::<Vec<_>>();
    ranked.sort_by(|(left, left_rank), (right, right_rank)| {
        left_rank
            .cmp(right_rank)
            .then_with(|| compare_records(left, right))
    });
    ranked
        .into_iter()
        .take(limit)
        .map(|(record, _)| record)
        .collect()
}

pub(super) fn rank_query_matches(
    query: &str,
    records: Vec<SymbolRecord>,
    limit: usize,
) -> Vec<SymbolRecord> {
    select_ranked_symbols(
        records
            .into_iter()
            .map(|record| {
                let rank = match_rank(query, &record);
                (record, rank)
            })
            .collect(),
        limit,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, name: &str, file: &str) -> SymbolRecord {
        SymbolRecord {
            id: id.into(),
            qualified_name: name.into(),
            file_path: file.into(),
            language: "rust".into(),
            kind: "function".into(),
            signature_fingerprint: "test-signature".into(),
            last_seen_at: 0,
        }
    }

    #[test]
    fn later_exact_targets_beat_broad_decoys_in_either_order() {
        let mut candidates = (0..24)
            .map(|i| {
                (
                    record(&format!("decoy-{i}"), &format!("quasar::A{i}"), "a.rs"),
                    MatchRank::Fuzzy,
                )
            })
            .collect::<Vec<_>>();
        let target = record("target", "quasar::PacketReader::decode", "packet.rs");
        candidates.push((target.clone(), match_rank("PacketReader.decode", &target)));
        let expected = select_ranked_symbols(candidates.clone(), 8);
        candidates.reverse();
        assert_eq!(expected, select_ranked_symbols(candidates, 8));
        assert_eq!(expected[0].id, "target");
        assert_eq!(expected.len(), 8);
    }

    #[test]
    fn duplicate_provenance_keeps_strongest_match_and_stable_ties() {
        let target = record("target", "quasar::commit", "ledger.rs");
        let other = record("other", "quasar::CommitQueue", "a.rs");
        let candidates = vec![
            (target.clone(), MatchRank::Fuzzy),
            (other, MatchRank::Fuzzy),
            (target, MatchRank::Leaf),
            (record("b", "quasar::same", "b.rs"), MatchRank::Leaf),
            (record("a2", "quasar::same", "a.rs"), MatchRank::Leaf),
            (record("a1", "quasar::same", "a.rs"), MatchRank::Leaf),
        ];
        let expected = select_ranked_symbols(candidates.clone(), 8);
        assert_eq!(
            expected.iter().filter(|item| item.id == "target").count(),
            1
        );
        assert_eq!(expected[0].id, "target");
        assert_eq!(expected[1].id, "a1");
        assert_eq!(expected[2].id, "a2");
        let mut reversed = candidates;
        reversed.reverse();
        assert_eq!(expected, select_ranked_symbols(reversed, 8));
    }

    #[test]
    fn exact_match_classes_preserve_case_and_owner_boundaries() {
        let item = record("id-7", "quasar::PacketReader::decode", "packet.rs");
        assert_eq!(match_rank("id-7", &item), MatchRank::Id);
        assert_eq!(
            match_rank("quasar.PacketReader.decode", &item),
            MatchRank::QualifiedName
        );
        assert_eq!(
            match_rank("PacketReader.decode", &item),
            MatchRank::OwnerQualified
        );
        assert_eq!(match_rank("decode", &item), MatchRank::Leaf);
        assert_eq!(match_rank("packetreader.decode", &item), MatchRank::Fuzzy);
        assert_eq!(match_rank("OtherReader.decode", &item), MatchRank::Fuzzy);
    }

    #[test]
    fn normalized_full_name_survives_owner_suffix_decoys_before_query_limit() {
        for query in ["Pack.Reader.decode", "Pack::Reader::decode"] {
            let mut matches = ["A", "B", "C", "D"]
                .iter()
                .map(|prefix| record(prefix, &format!("{prefix}::Pack::Reader::decode"), "a.rs"))
                .collect::<Vec<_>>();
            matches.push(record("exact", "Pack::Reader::decode", "z.rs"));
            let result = rank_query_matches(query, matches, 3);
            assert_eq!(result.len(), 3);
            assert_eq!(result[0].id, "exact");
        }
    }

    #[test]
    fn admission_is_bounded_and_reports_truncation_independent_of_order() {
        let mut queries = (0..80).map(|i| format!("token-{i:02}")).collect::<Vec<_>>();
        queries.push(" token-00 ".into());
        queries.push(" ".into());
        let (expected, truncated) = admitted_queries(&queries, 64);
        let expected = expected.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert!(truncated);
        assert_eq!(expected.len(), 64);
        queries.reverse();
        let (reversed, truncated) = admitted_queries(&queries, 64);
        assert!(truncated);
        assert_eq!(expected, reversed);
    }
}
