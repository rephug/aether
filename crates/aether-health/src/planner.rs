use std::collections::{BTreeMap, HashMap};

use aether_core::normalize_path;
use aether_graph_algo::GraphAlgorithmEdge;
use serde::{Deserialize, Serialize};

use crate::planner_communities::{
    FileCommunityConfig, FileSymbol, PlannerDiagnostics, detect_file_communities,
};

const STOPWORDS: &[&str] = &[
    "default", "new", "from", "into", "load", "save", "get", "set", "is", "has", "with", "for",
    "the", "and", "fn", "test", "mock", "impl", "try", "run", "do",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitSuggestion {
    pub target_file: String,
    pub suggested_modules: Vec<SuggestedModule>,
    pub expected_score_impact: String,
    pub confidence: SplitConfidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestedModule {
    pub name: String,
    pub suggested_file_path: String,
    pub symbols: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitConfidence {
    High,
    Medium,
    Low,
}

struct CommunityPlan<'a> {
    community_id: usize,
    symbols: Vec<&'a FileSymbol>,
    symbol_names: Vec<String>,
    ranked_tokens: Vec<String>,
}

pub fn suggest_split(
    file_path: &str,
    crate_score: u32,
    structural_edges: &[GraphAlgorithmEdge],
    symbols: &[FileSymbol],
    config: &FileCommunityConfig,
) -> Option<(SplitSuggestion, PlannerDiagnostics)> {
    if crate_score < 50 {
        return None;
    }

    let target_file = normalize_path(file_path.trim());
    if target_file.is_empty() {
        return None;
    }

    let (assignments, diagnostics) = detect_file_communities(structural_edges, symbols, config);
    if assignments.is_empty() {
        return None;
    }

    let mut grouped = BTreeMap::<usize, Vec<&FileSymbol>>::new();
    let symbol_by_id = symbols
        .iter()
        .map(|symbol| (symbol.symbol_id.as_str(), symbol))
        .collect::<HashMap<_, _>>();
    for (symbol_id, community_id) in assignments {
        let Some(symbol) = symbol_by_id.get(symbol_id.as_str()).copied() else {
            continue;
        };
        grouped.entry(community_id).or_default().push(symbol);
    }
    if grouped.len() < 2 {
        return None;
    }

    let mut plans = grouped
        .into_iter()
        .map(|(community_id, mut community_symbols)| {
            community_symbols.sort_by(|left, right| {
                display_symbol_name(left)
                    .cmp(&display_symbol_name(right))
                    .then_with(|| left.symbol_id.cmp(&right.symbol_id))
            });
            let symbol_names = community_symbols
                .iter()
                .map(|symbol| display_symbol_name(symbol))
                .collect::<Vec<_>>();
            let ranked_tokens = ranked_tokens(symbol_names.as_slice());
            CommunityPlan {
                community_id,
                symbols: community_symbols,
                symbol_names,
                ranked_tokens,
            }
        })
        .collect::<Vec<_>>();
    plans.sort_by(|left, right| left.community_id.cmp(&right.community_id));

    let unique_names = disambiguated_module_names(plans.as_slice());
    let suggested_modules = plans
        .into_iter()
        .map(|plan| {
            let module_name = unique_names
                .get(&plan.community_id)
                .cloned()
                .unwrap_or_else(|| fallback_module_name(plan.community_id));
            let reason_token = plan
                .ranked_tokens
                .first()
                .cloned()
                .unwrap_or_else(|| "shared".to_owned());

            SuggestedModule {
                suggested_file_path: suggested_file_path(
                    target_file.as_str(),
                    module_name.as_str(),
                ),
                name: module_name,
                symbols: unique_symbol_names(plan.symbol_names.as_slice()),
                reason: format!(
                    "These {} symbols cluster around {} responsibilities",
                    plan.symbols.len(),
                    reason_token,
                ),
            }
        })
        .collect::<Vec<_>>();

    let symbol_count = suggested_modules
        .iter()
        .map(|module| module.symbols.len())
        .sum::<usize>();
    let expected_score_impact = match (suggested_modules.len(), symbol_count) {
        (modules, symbols) if modules >= 3 || symbols >= 12 => {
            "Likely reduces score by ~15-25 points".to_owned()
        }
        (modules, symbols) if modules >= 2 || symbols >= 8 => {
            "Likely reduces score by ~10-20 points".to_owned()
        }
        _ => "Likely reduces score by ~5-15 points".to_owned(),
    };

    Some((
        SplitSuggestion {
            target_file,
            suggested_modules,
            expected_score_impact,
            confidence: split_confidence(diagnostics.confidence),
        },
        diagnostics,
    ))
}

fn disambiguated_module_names(plans: &[CommunityPlan<'_>]) -> HashMap<usize, String> {
    let mut token_depths = plans
        .iter()
        .map(|plan| {
            (
                plan.community_id,
                if plan.ranked_tokens.is_empty() { 0 } else { 1 },
            )
        })
        .collect::<HashMap<_, _>>();

    loop {
        let mut names = HashMap::<usize, String>::new();
        let mut collisions = HashMap::<String, Vec<usize>>::new();
        for plan in plans {
            let depth = token_depths.get(&plan.community_id).copied().unwrap_or(0);
            let name =
                module_name_for_tokens(plan.community_id, plan.ranked_tokens.as_slice(), depth);
            collisions
                .entry(name.clone())
                .or_default()
                .push(plan.community_id);
            names.insert(plan.community_id, name);
        }

        let mut changed = false;
        for ids in collisions.values() {
            if ids.len() < 2 {
                continue;
            }
            for community_id in ids {
                let Some(plan) = plans.iter().find(|plan| plan.community_id == *community_id)
                else {
                    continue;
                };
                let depth = token_depths.entry(*community_id).or_insert(0);
                if *depth < plan.ranked_tokens.len() {
                    *depth += 1;
                    changed = true;
                }
            }
        }

        if !changed {
            return names;
        }
    }
}

fn ranked_tokens(symbol_names: &[String]) -> Vec<String> {
    let mut counts = BTreeMap::<String, usize>::new();
    for symbol_name in symbol_names {
        for token in symbol_name.to_ascii_lowercase().split('_') {
            if token.is_empty() {
                continue;
            }
            let normalized = normalize_token(token);
            if STOPWORDS.contains(&normalized.as_str()) {
                continue;
            }
            *counts.entry(normalized).or_default() += 1;
        }
    }

    let mut ranked = counts.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    ranked.into_iter().map(|(token, _)| token).collect()
}

fn module_name_for_tokens(community_id: usize, tokens: &[String], depth: usize) -> String {
    if tokens.is_empty() || depth == 0 {
        return fallback_module_name(community_id);
    }
    let selected = tokens
        .iter()
        .take(depth.min(tokens.len()))
        .cloned()
        .collect::<Vec<_>>();
    if selected.is_empty() {
        fallback_module_name(community_id)
    } else {
        format!("{}_ops", selected.join("_"))
    }
}

fn fallback_module_name(community_id: usize) -> String {
    format!("community_{community_id}_ops")
}

fn normalize_token(token: &str) -> String {
    match token {
        "note" | "notes" => "note".to_owned(),
        "migration" | "migrate" => "migration".to_owned(),
        "test" | "tests" => "test".to_owned(),
        "store" | "stores" => "store".to_owned(),
        other => other.to_owned(),
    }
}

fn unique_symbol_names(symbol_names: &[String]) -> Vec<String> {
    let mut unique = symbol_names.to_vec();
    unique.sort();
    unique.dedup();
    unique
}

fn display_symbol_name(symbol: &FileSymbol) -> String {
    if !symbol.name.trim().is_empty() {
        return symbol.name.trim().trim_start_matches("r#").to_owned();
    }

    symbol
        .qualified_name
        .rsplit("::")
        .next()
        .unwrap_or(symbol.qualified_name.as_str())
        .rsplit('.')
        .next()
        .unwrap_or(symbol.qualified_name.as_str())
        .trim_start_matches("r#")
        .to_owned()
}

fn suggested_file_path(file_path: &str, module_name: &str) -> String {
    let normalized = normalize_path(file_path);
    let file_name = if let Some(base) = module_name.strip_suffix("_ops") {
        if base.starts_with("community_") || base.ends_with('s') {
            format!("{base}.rs")
        } else {
            format!("{base}s.rs")
        }
    } else {
        format!("{module_name}.rs")
    };

    if let Some((prefix, _)) = normalized.rsplit_once("/src/") {
        format!("{prefix}/src/{file_name}")
    } else if let Some((dir, _)) = normalized.rsplit_once('/') {
        format!("{dir}/{file_name}")
    } else {
        file_name
    }
}

fn split_confidence(confidence: f32) -> SplitConfidence {
    if confidence >= 0.7 {
        SplitConfidence::High
    } else if confidence >= 0.4 {
        SplitConfidence::Medium
    } else {
        SplitConfidence::Low
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FileCommunityConfig, FileSymbol, SplitConfidence, disambiguated_module_names,
        fallback_module_name, module_name_for_tokens, normalize_token, ranked_tokens,
        split_confidence, suggest_split, suggested_file_path,
    };
    use crate::planner_communities::detect_file_communities;
    use aether_core::SymbolKind;
    use aether_graph_algo::GraphAlgorithmEdge;

    fn config() -> FileCommunityConfig {
        FileCommunityConfig {
            semantic_rescue_threshold: 0.70,
            semantic_rescue_max_k: 3,
            community_resolution: 0.5,
            min_community_size: 1,
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

    fn edge(source_id: &str, target_id: &str) -> GraphAlgorithmEdge {
        GraphAlgorithmEdge {
            source_id: source_id.to_owned(),
            target_id: target_id.to_owned(),
            edge_kind: "calls".to_owned(),
        }
    }

    #[test]
    fn naming_uses_token_frequency_not_prefix() {
        let tokens = ranked_tokens(&[
            "note_index_build".to_owned(),
            "note_index_read".to_owned(),
            "note_merge".to_owned(),
        ]);
        assert_eq!(tokens.first().map(String::as_str), Some("note"));
        assert_eq!(tokens.get(1).map(String::as_str), Some("index"));
    }

    #[test]
    fn naming_skips_stopwords() {
        let tokens = ranked_tokens(&[
            "get_note".to_owned(),
            "set_note".to_owned(),
            "load_note".to_owned(),
        ]);
        assert_eq!(tokens, vec!["note".to_owned()]);
    }

    #[test]
    fn naming_disambiguates_collisions_without_merging() {
        let plans = vec![
            super::CommunityPlan {
                community_id: 1,
                symbols: Vec::new(),
                symbol_names: vec!["note_store".to_owned(), "note_read".to_owned()],
                ranked_tokens: vec!["note".to_owned(), "store".to_owned(), "read".to_owned()],
            },
            super::CommunityPlan {
                community_id: 2,
                symbols: Vec::new(),
                symbol_names: vec!["note_migration".to_owned(), "note_apply".to_owned()],
                ranked_tokens: vec![
                    "note".to_owned(),
                    "migration".to_owned(),
                    "apply".to_owned(),
                ],
            },
        ];

        let names = disambiguated_module_names(plans.as_slice());
        assert_eq!(names.get(&1).map(String::as_str), Some("note_store_ops"));
        assert_eq!(
            names.get(&2).map(String::as_str),
            Some("note_migration_ops")
        );
    }

    #[test]
    fn naming_normalizes_aliases() {
        assert_eq!(normalize_token("notes"), "note");
        assert_eq!(normalize_token("migrate"), "migration");
        assert_eq!(normalize_token("stores"), "store");
    }

    #[test]
    fn naming_generates_file_paths_deterministically() {
        assert_eq!(
            suggested_file_path("crates/aether-store/src/lib.rs", "migration_ops",),
            "crates/aether-store/src/migrations.rs"
        );
    }

    #[test]
    fn split_planner_returns_none_for_one_community() {
        let symbols = vec![
            symbol(
                "sym-a",
                "alpha_note",
                "crate::alpha_note",
                SymbolKind::Function,
            ),
            symbol(
                "sym-b",
                "beta_note",
                "crate::beta_note",
                SymbolKind::Function,
            ),
        ];

        let suggestion = suggest_split(
            "crates/example/src/lib.rs",
            70,
            &[edge("sym-a", "sym-b")],
            &symbols,
            &config(),
        );
        assert!(suggestion.is_none());
    }

    #[test]
    fn split_planner_returns_modules_with_paths() {
        let mut planner_config = config();
        planner_config.min_community_size = 1;
        let mut symbols = vec![
            symbol(
                "sym-a",
                "note_store",
                "crate::note_store",
                SymbolKind::Function,
            ),
            symbol(
                "sym-b",
                "note_read",
                "crate::note_read",
                SymbolKind::Function,
            ),
            symbol(
                "sym-c",
                "migration_apply",
                "crate::migration_apply",
                SymbolKind::Function,
            ),
            symbol(
                "sym-d",
                "migration_plan",
                "crate::migration_plan",
                SymbolKind::Function,
            ),
        ];
        symbols[0].embedding = Some(vec![1.0, 0.0]);
        symbols[1].embedding = Some(vec![1.0, 0.0]);
        symbols[2].embedding = Some(vec![0.0, 1.0]);
        symbols[3].embedding = Some(vec![0.0, 1.0]);

        let (suggestion, diagnostics) = suggest_split(
            "crates/example/src/lib.rs",
            70,
            &[edge("sym-a", "sym-b"), edge("sym-c", "sym-d")],
            &symbols,
            &planner_config,
        )
        .expect("split suggestion");

        assert_eq!(suggestion.suggested_modules.len(), 2);
        assert!(
            suggestion
                .suggested_modules
                .iter()
                .any(|module| module.suggested_file_path.ends_with("/notes.rs"))
        );
        assert!(matches!(
            suggestion.confidence,
            SplitConfidence::High | SplitConfidence::Medium | SplitConfidence::Low
        ));
        assert_eq!(
            split_confidence(diagnostics.confidence),
            suggestion.confidence
        );
    }

    #[test]
    fn planner_communities_integration_produces_multiple_groups() {
        let mut planner_config = config();
        planner_config.min_community_size = 1;
        let mut symbols = vec![
            symbol(
                "sym-a",
                "note_store",
                "crate::note_store",
                SymbolKind::Function,
            ),
            symbol(
                "sym-b",
                "note_read",
                "crate::note_read",
                SymbolKind::Function,
            ),
            symbol(
                "sym-c",
                "migration_apply",
                "crate::migration_apply",
                SymbolKind::Function,
            ),
            symbol(
                "sym-d",
                "migration_plan",
                "crate::migration_plan",
                SymbolKind::Function,
            ),
        ];
        symbols[0].embedding = Some(vec![1.0, 0.0]);
        symbols[1].embedding = Some(vec![1.0, 0.0]);
        symbols[2].embedding = Some(vec![0.0, 1.0]);
        symbols[3].embedding = Some(vec![0.0, 1.0]);

        let (assignments, _) = detect_file_communities(
            &[edge("sym-a", "sym-b"), edge("sym-c", "sym-d")],
            &symbols,
            &planner_config,
        );
        let community_count = assignments
            .iter()
            .map(|(_, community_id)| *community_id)
            .collect::<std::collections::HashSet<_>>()
            .len();
        assert_eq!(community_count, 2);
    }

    #[test]
    fn fallback_module_name_is_deterministic() {
        assert_eq!(fallback_module_name(7), "community_7_ops");
        assert_eq!(module_name_for_tokens(7, &[], 0), "community_7_ops");
    }
}
