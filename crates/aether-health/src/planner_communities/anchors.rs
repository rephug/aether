use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use aether_core::SymbolKind;

use super::graph::build_rep_members;
use super::{DisjointSet, SymbolEntry};

const ANCHOR_SPLIT_THRESHOLD: usize = 20;
const ANCHOR_MIN_BUCKET: usize = 3;
const ANCHOR_STOPWORDS: &[&str] = &[
    "get",
    "set",
    "list",
    "find",
    "read",
    "write",
    "upsert",
    "delete",
    "remove",
    "insert",
    "update",
    "create",
    "mark",
    "clear",
    "prune",
    "count",
    "increment",
    "record",
    "load",
    "save",
    "search",
    "open",
    "close",
    "new",
    "default",
    "from",
    "into",
    "with",
    "for",
    "the",
    "and",
    "is",
    "has",
    "all",
    "batch",
    "by",
    "if",
    "or",
    "run",
    "do",
    "try",
    "check",
    "ensure",
    "acknowledge",
    "resolve",
    "as",
    "to",
    "sync",
    "test",
];

fn normalize_anchor_token(token: &str) -> String {
    let normalized = token
        .strip_prefix("r#")
        .unwrap_or(token)
        .to_ascii_lowercase();
    match normalized.as_str() {
        "note" | "notes" => "note".to_owned(),
        "project" | "projects" => "project".to_owned(),
        "migration" | "migrate" | "migrations" => "migration".to_owned(),
        "embedding" | "embeddings" => "embedding".to_owned(),
        "symbol" | "symbols" => "symbol".to_owned(),
        "intent" | "intents" => "intent".to_owned(),
        "store" | "stores" => "store".to_owned(),
        "edge" | "edges" => "edge".to_owned(),
        "version" | "versions" => "version".to_owned(),
        "request" | "requests" => "request".to_owned(),
        "result" | "results" => "result".to_owned(),
        "graph" | "graphs" => "graph".to_owned(),
        "schema" | "schemas" => "schema".to_owned(),
        "module" | "modules" => "module".to_owned(),
        "provider" | "providers" => "provider".to_owned(),
        "model" | "models" => "model".to_owned(),
        "meta" | "metas" => "meta".to_owned(),
        "history" | "histories" => "history".to_owned(),
        other => {
            if other.len() > 3 {
                other.strip_suffix('s').unwrap_or(other).to_owned()
            } else {
                other.to_owned()
            }
        }
    }
}

fn informative_tokens(name: &str) -> Vec<String> {
    let leaf_name = name.rsplit("::").next().unwrap_or(name);
    leaf_name
        .split('_')
        .filter_map(|token| {
            let normalized = normalize_anchor_token(token);
            if normalized.len() <= 1 || ANCHOR_STOPWORDS.contains(&normalized.as_str()) {
                None
            } else {
                Some(normalized)
            }
        })
        .collect()
}

pub(super) fn split_large_anchor_groups(
    entries: &[SymbolEntry],
    original_groups: Vec<Vec<usize>>,
) -> (Vec<Vec<usize>>, HashSet<usize>) {
    let mut split_groups = Vec::new();
    let mut split_members = HashSet::new();

    for mut group in original_groups {
        if group.is_empty() {
            continue;
        }
        group.sort_unstable();

        if group.len() <= ANCHOR_SPLIT_THRESHOLD {
            split_groups.push(group);
            continue;
        }
        if !group
            .iter()
            .any(|index| is_type_anchor(entries[*index].symbol.kind))
        {
            split_groups.push(group);
            continue;
        }

        let mut anchor_indices = Vec::new();
        let mut method_indices = Vec::new();
        for index in &group {
            if is_type_anchor(entries[*index].symbol.kind) {
                anchor_indices.push(*index);
            } else {
                method_indices.push(*index);
            }
        }
        if method_indices.len() < ANCHOR_SPLIT_THRESHOLD {
            split_groups.push(group);
            continue;
        }

        let mut method_tokens = HashMap::<usize, Vec<String>>::new();
        for index in &method_indices {
            let name = entries[*index].symbol.qualified_name.as_str();
            let tokens = informative_tokens(name);
            method_tokens.insert(*index, tokens);
        }

        let mut bucket_members = BTreeMap::<String, Vec<usize>>::new();
        for index in &method_indices {
            let bucket_key = method_tokens
                .get(index)
                .and_then(|tokens| tokens.first())
                .cloned()
                .unwrap_or_else(|| "misc".to_owned());
            bucket_members.entry(bucket_key).or_default().push(*index);
        }

        let has_large_bucket = bucket_members
            .values()
            .any(|members| members.len() >= ANCHOR_MIN_BUCKET);
        if !has_large_bucket {
            split_groups.push(group);
            continue;
        }

        let mut bucket_tokens = bucket_members
            .iter()
            .map(|(key, members)| {
                let tokens = members
                    .iter()
                    .filter_map(|member| method_tokens.get(member))
                    .flat_map(|tokens| tokens.iter().cloned())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                (key.clone(), tokens)
            })
            .collect::<HashMap<_, _>>();
        let small_bucket_keys = bucket_members
            .iter()
            .filter_map(|(key, members)| (members.len() < ANCHOR_MIN_BUCKET).then_some(key.clone()))
            .collect::<Vec<_>>();
        for small_key in small_bucket_keys {
            let Some(small_members) = bucket_members.remove(&small_key) else {
                continue;
            };
            let small_tokens = bucket_tokens.remove(&small_key).unwrap_or_default();
            let target_key = bucket_members
                .iter()
                .filter(|(_, members)| members.len() >= ANCHOR_MIN_BUCKET)
                .map(|(key, members)| {
                    let target_tokens = bucket_tokens
                        .get(key)
                        .map(Vec::as_slice)
                        .unwrap_or_default();
                    let target_tokens = target_tokens.iter().collect::<HashSet<_>>();
                    (
                        key.clone(),
                        small_tokens
                            .iter()
                            .filter(|token| target_tokens.contains(token))
                            .count(),
                        members.len(),
                    )
                })
                .max_by(|left, right| {
                    left.1
                        .cmp(&right.1)
                        .then_with(|| left.2.cmp(&right.2))
                        .then_with(|| right.0.cmp(&left.0))
                })
                .map(|(key, _, _)| key);

            let Some(target_key) = target_key else {
                bucket_members.insert(small_key, small_members);
                continue;
            };

            if let Some(target_members) = bucket_members.get_mut(&target_key) {
                target_members.extend(small_members);
            }
            if let Some(target_tokens) = bucket_tokens.get_mut(&target_key) {
                let mut merged_tokens = target_tokens.iter().cloned().collect::<BTreeSet<_>>();
                merged_tokens.extend(small_tokens);
                *target_tokens = merged_tokens.into_iter().collect();
            }
        }

        if bucket_members.len() <= 1 {
            split_groups.push(group);
            continue;
        }

        split_members.extend(group.iter().copied());

        if !anchor_indices.is_empty()
            && let Some(target_key) = bucket_members
                .iter()
                .max_by(|left, right| {
                    left.1
                        .len()
                        .cmp(&right.1.len())
                        .then_with(|| right.0.cmp(left.0))
                })
                .map(|(key, _)| key.clone())
            && let Some(target_members) = bucket_members.get_mut(&target_key)
        {
            target_members.extend(anchor_indices);
        }

        for (_, mut members) in bucket_members {
            members.sort_unstable();
            split_groups.push(members);
        }
    }

    (split_groups, split_members)
}

pub(super) fn count_type_anchored_symbols(entries: &[SymbolEntry], groups: &[Vec<usize>]) -> usize {
    groups
        .iter()
        .filter(|members| {
            members.len() > 1
                && members
                    .iter()
                    .any(|index| is_type_anchor(entries[*index].symbol.kind))
        })
        .map(Vec::len)
        .sum()
}

pub(super) fn build_anchor_groups(entries: &[SymbolEntry]) -> (DisjointSet, Vec<Vec<usize>>) {
    let mut union_find = DisjointSet::new(entries.len());
    let mut stem_to_indices = HashMap::<String, Vec<usize>>::new();
    for (index, entry) in entries.iter().enumerate() {
        stem_to_indices
            .entry(entry.stem.clone())
            .or_default()
            .push(index);
    }

    for (index, entry) in entries.iter().enumerate() {
        if !is_type_anchor(entry.symbol.kind) {
            continue;
        }
        let mut members = vec![index];
        if let Some(group) = stem_to_indices.get(entry.symbol.qualified_name.as_str()) {
            for other in group {
                if *other != index {
                    members.push(*other);
                }
            }
        }
        if members.len() < 2 {
            continue;
        }
        let anchor = members[0];
        for member in members.into_iter().skip(1) {
            union_find.union(anchor, member);
        }
    }

    let rep_to_members = build_rep_members(entries.len(), &mut union_find);
    (union_find, rep_to_members)
}

pub(super) fn rebuild_union_find_from_groups(
    num_entries: usize,
    groups: &[Vec<usize>],
) -> DisjointSet {
    let mut union_find = DisjointSet::new(num_entries);
    for group in groups {
        let Some(anchor) = group.first().copied() else {
            continue;
        };
        for member in group.iter().copied().skip(1) {
            union_find.union(anchor, member);
        }
    }
    union_find
}

fn is_type_anchor(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Struct | SymbolKind::Enum | SymbolKind::Trait | SymbolKind::TypeAlias
    )
}

#[cfg(test)]
mod tests {
    use aether_core::SymbolKind;

    use super::super::{FileCommunityConfig, FileSymbol, qualified_name_stem};
    use super::{
        build_anchor_groups, count_type_anchored_symbols, rebuild_union_find_from_groups,
        split_large_anchor_groups,
    };

    fn config() -> FileCommunityConfig {
        FileCommunityConfig {
            semantic_rescue_threshold: 0.70,
            semantic_rescue_max_k: 3,
            community_resolution: 0.5,
            min_community_size: 3,
        }
    }

    fn symbol(symbol_id: &str, name: &str, qualified_name: &str, kind: SymbolKind) -> FileSymbol {
        FileSymbol {
            symbol_id: symbol_id.to_owned(),
            name: name.to_owned(),
            qualified_name: qualified_name.to_owned(),
            kind,
            is_test: false,
            embedding: None,
        }
    }

    fn entries(symbols: &[FileSymbol]) -> Vec<super::super::SymbolEntry> {
        symbols
            .iter()
            .cloned()
            .map(|symbol| super::super::SymbolEntry {
                stem: qualified_name_stem(symbol.qualified_name.as_str()),
                symbol,
            })
            .collect()
    }

    fn non_empty_groups(groups: &[Vec<usize>]) -> Vec<Vec<usize>> {
        groups
            .iter()
            .filter(|members| !members.is_empty())
            .cloned()
            .collect()
    }

    fn split_anchor_groups(
        entries: &[super::super::SymbolEntry],
    ) -> (Vec<Vec<usize>>, std::collections::HashSet<usize>) {
        let (_, anchor_groups) = build_anchor_groups(entries);
        split_large_anchor_groups(entries, anchor_groups)
    }

    fn group_index_for_symbol_id(
        groups: &[Vec<usize>],
        entries: &[super::super::SymbolEntry],
        symbol_id: &str,
    ) -> Option<usize> {
        groups.iter().position(|members| {
            members.iter().any(|index| {
                entries
                    .get(*index)
                    .map(|entry| entry.symbol.symbol_id.as_str() == symbol_id)
                    .unwrap_or(false)
            })
        })
    }

    fn large_anchor_symbols() -> Vec<FileSymbol> {
        let mut symbols = vec![symbol(
            "sym-store",
            "SqliteStore",
            "crate::SqliteStore",
            SymbolKind::Struct,
        )];
        let methods = [
            ("sym-project-note-upsert", "upsert_project_note"),
            ("sym-project-note-list", "list_project_notes"),
            ("sym-project-note-delete", "delete_project_note"),
            ("sym-project-note-find", "find_project_note"),
            ("sym-project-note-read", "read_project_notes"),
            ("sym-project-note-write", "write_project_note"),
            ("sym-project-note-save", "save_project_note"),
            ("sym-project-note-load", "load_project_notes"),
            ("sym-project-note-count", "count_project_notes"),
            ("sym-sir-get", "get_sir"),
            ("sym-sir-save", "save_sir"),
            ("sym-sir-load", "load_sir"),
            ("sym-sir-list", "list_sirs"),
            ("sym-sir-meta", "upsert_sir_meta"),
            ("sym-sir-history", "list_sir_history"),
            ("sym-sir-version", "get_sir_version"),
            ("sym-sir-schema", "read_sir_schema"),
            ("sym-migration-run", "run_migrations"),
            ("sym-migration-list", "list_migrations"),
            ("sym-migration-delete", "delete_migrations"),
            ("sym-migration-clear", "clear_migrations"),
            ("sym-migration-rename", "migration_v6_renames"),
            ("sym-migration-from", "migration_from_v2"),
            ("sym-intent-list", "list_intents"),
            ("sym-intent-find", "find_intents"),
            ("sym-intent-delete", "delete_intents"),
            ("sym-intent-write", "create_write_intent"),
            ("sym-intent-status", "update_intent_status"),
            ("sym-intent-failed", "mark_intent_failed"),
            ("sym-intent-count", "count_intents"),
        ];
        for (symbol_id, method_name) in methods {
            symbols.push(symbol(
                symbol_id,
                method_name,
                &format!("crate::SqliteStore::{method_name}"),
                SymbolKind::Method,
            ));
        }
        symbols
    }

    #[test]
    fn type_anchor_precollapse_keeps_type_and_methods_together() {
        let symbols = vec![
            symbol("sym-type", "Widget", "crate::Widget", SymbolKind::Struct),
            symbol(
                "sym-method",
                "render",
                "crate::Widget::render",
                SymbolKind::Method,
            ),
            symbol(
                "sym-helper",
                "helper",
                "crate::helper",
                SymbolKind::Function,
            ),
        ];

        let (mut union_find, groups) = build_anchor_groups(entries(&symbols).as_slice());
        let type_rep = union_find.find(0);
        let method_rep = union_find.find(1);
        let helper_rep = union_find.find(2);

        assert_eq!(type_rep, method_rep);
        assert_ne!(type_rep, helper_rep);
        assert_eq!(
            count_type_anchored_symbols(entries(&symbols).as_slice(), groups.as_slice()),
            2
        );
    }

    #[test]
    fn type_anchor_does_not_cross_types() {
        let symbols = vec![
            symbol("sym-widget", "Widget", "crate::Widget", SymbolKind::Struct),
            symbol(
                "sym-widget-method",
                "render",
                "crate::Widget::render",
                SymbolKind::Method,
            ),
            symbol("sym-gadget", "Gadget", "crate::Gadget", SymbolKind::Struct),
            symbol(
                "sym-gadget-method",
                "run",
                "crate::Gadget::run",
                SymbolKind::Method,
            ),
        ];

        let (mut union_find, _) = build_anchor_groups(entries(&symbols).as_slice());
        let widget_rep = union_find.find(0);
        let widget_method_rep = union_find.find(1);
        let gadget_rep = union_find.find(2);
        let gadget_method_rep = union_find.find(3);

        assert_eq!(widget_rep, widget_method_rep);
        assert_eq!(gadget_rep, gadget_method_rep);
        assert_ne!(widget_rep, gadget_rep);
    }

    #[test]
    fn split_large_anchor_skips_small_groups() {
        let mut symbols = vec![symbol(
            "sym-store",
            "SqliteStore",
            "crate::SqliteStore",
            SymbolKind::Struct,
        )];
        for index in 0..9 {
            symbols.push(symbol(
                &format!("sym-method-{index}"),
                &format!("op_{index}"),
                &format!("crate::SqliteStore::op_{index}"),
                SymbolKind::Method,
            ));
        }

        let entries = entries(&symbols);
        let (_, anchor_groups) = build_anchor_groups(entries.as_slice());
        let expected_groups = non_empty_groups(anchor_groups.as_slice());
        let (split_groups, split_members) =
            split_large_anchor_groups(entries.as_slice(), anchor_groups);

        assert_eq!(split_groups, expected_groups);
        assert!(split_members.is_empty());
    }

    #[test]
    fn split_large_anchor_partitions_by_domain_token() {
        let entries = entries(&large_anchor_symbols());
        let (split_groups, split_members) = split_anchor_groups(entries.as_slice());

        assert!(split_groups.len() > 1);
        assert_eq!(split_members.len(), entries.len());

        let sir_group =
            group_index_for_symbol_id(split_groups.as_slice(), entries.as_slice(), "sym-sir-meta");
        assert_eq!(
            sir_group,
            group_index_for_symbol_id(
                split_groups.as_slice(),
                entries.as_slice(),
                "sym-sir-history"
            )
        );
        assert_eq!(
            sir_group,
            group_index_for_symbol_id(
                split_groups.as_slice(),
                entries.as_slice(),
                "sym-sir-version"
            )
        );

        let project_group = group_index_for_symbol_id(
            split_groups.as_slice(),
            entries.as_slice(),
            "sym-project-note-upsert",
        );
        assert_eq!(
            project_group,
            group_index_for_symbol_id(
                split_groups.as_slice(),
                entries.as_slice(),
                "sym-project-note-list",
            )
        );
        assert_eq!(
            project_group,
            group_index_for_symbol_id(
                split_groups.as_slice(),
                entries.as_slice(),
                "sym-project-note-delete",
            )
        );

        let migration_group = group_index_for_symbol_id(
            split_groups.as_slice(),
            entries.as_slice(),
            "sym-migration-run",
        );
        assert_eq!(
            migration_group,
            group_index_for_symbol_id(
                split_groups.as_slice(),
                entries.as_slice(),
                "sym-migration-rename",
            )
        );
        assert_eq!(
            migration_group,
            group_index_for_symbol_id(
                split_groups.as_slice(),
                entries.as_slice(),
                "sym-migration-from",
            )
        );

        assert_ne!(sir_group, project_group);
        assert_ne!(sir_group, migration_group);
        assert_eq!(
            group_index_for_symbol_id(split_groups.as_slice(), entries.as_slice(), "sym-store"),
            project_group
        );
    }

    #[test]
    fn split_large_anchor_type_not_singleton() {
        let entries = entries(&large_anchor_symbols());
        let (split_groups, _) = split_anchor_groups(entries.as_slice());
        let store_group =
            group_index_for_symbol_id(split_groups.as_slice(), entries.as_slice(), "sym-store");

        assert!(store_group.is_some());
        if let Some(group_index) = store_group {
            assert!(
                split_groups
                    .get(group_index)
                    .map(Vec::len)
                    .unwrap_or_default()
                    > 1
            );
        }
    }

    #[test]
    fn rebuild_union_find_preserves_anchor_partitioning() {
        let entries = entries(&large_anchor_symbols());
        let (_, anchor_groups) = build_anchor_groups(entries.as_slice());
        let (split_groups, _) = split_large_anchor_groups(entries.as_slice(), anchor_groups);
        let mut rebuilt = rebuild_union_find_from_groups(entries.len(), split_groups.as_slice());
        let group_count = split_groups.len();

        let reps = (0..entries.len())
            .map(|index| rebuilt.find(index))
            .collect::<std::collections::HashSet<_>>();

        assert!(group_count >= 2);
        assert_eq!(reps.len(), group_count);
        assert_eq!(config().min_community_size, 3);
    }
}
