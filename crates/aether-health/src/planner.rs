use std::collections::{BTreeMap, BTreeSet, HashMap};

use aether_core::normalize_path;
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannerCommunityAssignment {
    pub symbol_id: String,
    pub community_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannerSymbolRecord {
    pub id: String,
    pub qualified_name: String,
    pub file_path: String,
}

pub fn suggest_split(
    file_path: &str,
    crate_score: u32,
    community_assignments: &[PlannerCommunityAssignment],
    symbol_records: &[PlannerSymbolRecord],
) -> Option<SplitSuggestion> {
    if crate_score < 50 {
        return None;
    }

    let target_file = normalize_path(file_path.trim());
    if target_file.is_empty() {
        return None;
    }

    let community_by_symbol = community_assignments
        .iter()
        .map(|assignment| (assignment.symbol_id.as_str(), assignment.community_id))
        .collect::<HashMap<_, _>>();

    let mut grouped = BTreeMap::<i64, Vec<&PlannerSymbolRecord>>::new();
    for symbol in symbol_records {
        if normalize_path(symbol.file_path.as_str()) != target_file {
            continue;
        }
        let Some(community_id) = community_by_symbol.get(symbol.id.as_str()).copied() else {
            continue;
        };
        grouped.entry(community_id).or_default().push(symbol);
    }

    if grouped.len() < 2 {
        return None;
    }

    let mut used_module_names = BTreeSet::new();
    let mut community_prefixes = Vec::new();
    let mut used_fallback_name = false;
    let mut suggested_modules = Vec::new();

    for (community_id, symbols) in grouped {
        let symbol_names = symbols
            .iter()
            .map(|symbol| simple_symbol_name(symbol.qualified_name.as_str()))
            .collect::<Vec<_>>();
        let dominant_prefix = dominant_prefix(symbol_names.as_slice());
        let mut module_name = dominant_prefix
            .as_deref()
            .map(module_name_from_prefix)
            .unwrap_or_else(|| {
                used_fallback_name = true;
                format!("community_{community_id}_ops")
            });
        if !used_module_names.insert(module_name.clone()) {
            module_name = format!("{module_name}_{community_id}");
            used_module_names.insert(module_name.clone());
        }

        if dominant_prefix.is_none() {
            used_fallback_name = true;
        }
        community_prefixes.push(dominant_prefix.unwrap_or_default());

        let mut module_symbols = symbols
            .iter()
            .map(|symbol| simple_symbol_name(symbol.qualified_name.as_str()))
            .collect::<Vec<_>>();
        module_symbols.sort();
        module_symbols.dedup();

        suggested_modules.push(SuggestedModule {
            name: module_name,
            symbols: module_symbols.clone(),
            reason: format!(
                "These {} symbols cluster in community {} around {} responsibilities",
                module_symbols.len(),
                community_id,
                module_symbols
                    .first()
                    .and_then(|name| concept_prefix(name))
                    .unwrap_or_else(|| "shared".to_owned())
            ),
        });
    }

    let confidence = confidence_for_groups(
        suggested_modules.as_slice(),
        community_prefixes.as_slice(),
        used_fallback_name,
    );
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

    Some(SplitSuggestion {
        target_file,
        suggested_modules,
        expected_score_impact,
        confidence,
    })
}

fn simple_symbol_name(qualified_name: &str) -> String {
    qualified_name
        .rsplit("::")
        .next()
        .unwrap_or(qualified_name)
        .rsplit('.')
        .next()
        .unwrap_or(qualified_name)
        .trim_start_matches("r#")
        .to_owned()
}

fn dominant_prefix(symbol_names: &[String]) -> Option<String> {
    let mut counts = BTreeMap::<String, usize>::new();
    for name in symbol_names {
        let Some(prefix) = concept_prefix(name) else {
            continue;
        };
        *counts.entry(prefix).or_default() += 1;
    }

    counts
        .into_iter()
        .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(&left.0)))
        .map(|entry| entry.0)
}

fn concept_prefix(name: &str) -> Option<String> {
    let mut normalized = String::new();
    for (index, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 && !normalized.ends_with('_') {
                normalized.push('_');
            }
            normalized.push(ch.to_ascii_lowercase());
        } else if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
        } else if !normalized.ends_with('_') {
            normalized.push('_');
        }
    }

    normalized
        .split('_')
        .find(|part| !part.is_empty())
        .map(|part| match part {
            "embedding" => "embedding".to_owned(),
            "embed" => "embedding".to_owned(),
            other => other.to_owned(),
        })
}

fn module_name_from_prefix(prefix: &str) -> String {
    match prefix {
        "sir" => "sir_ops".to_owned(),
        "note" => "note_ops".to_owned(),
        "embedding" => "embedding_ops".to_owned(),
        other if !other.is_empty() => format!("{other}_ops"),
        _ => "shared_ops".to_owned(),
    }
}

fn confidence_for_groups(
    modules: &[SuggestedModule],
    prefixes: &[String],
    used_fallback_name: bool,
) -> SplitConfidence {
    if used_fallback_name {
        return SplitConfidence::Low;
    }

    let unique_prefixes = prefixes
        .iter()
        .filter(|prefix| !prefix.is_empty())
        .collect::<BTreeSet<_>>();
    let distinct_prefixes = unique_prefixes.len() == modules.len();
    let mut max_overlap = 0.0_f64;
    for (index, left) in modules.iter().enumerate() {
        let left_set = left.symbols.iter().collect::<BTreeSet<_>>();
        for right in modules.iter().skip(index + 1) {
            let right_set = right.symbols.iter().collect::<BTreeSet<_>>();
            let union = left_set.union(&right_set).count();
            if union == 0 {
                continue;
            }
            let shared = left_set.intersection(&right_set).count();
            max_overlap = max_overlap.max(shared as f64 / union as f64);
        }
    }

    if distinct_prefixes && max_overlap <= 0.15 {
        SplitConfidence::High
    } else if max_overlap <= 0.4 {
        SplitConfidence::Medium
    } else {
        SplitConfidence::Low
    }
}

#[cfg(test)]
mod tests {
    use super::{PlannerCommunityAssignment, PlannerSymbolRecord, SplitConfidence, suggest_split};

    fn symbol(id: &str, qualified_name: &str, file_path: &str) -> PlannerSymbolRecord {
        PlannerSymbolRecord {
            id: id.to_owned(),
            qualified_name: qualified_name.to_owned(),
            file_path: file_path.to_owned(),
        }
    }

    #[test]
    fn split_planner_no_suggestion_for_healthy_file() {
        let suggestion = suggest_split(
            "crates/example/src/lib.rs",
            42,
            &[PlannerCommunityAssignment {
                symbol_id: "sym-a".to_owned(),
                community_id: 1,
            }],
            &[symbol(
                "sym-a",
                "crate::sir_alpha",
                "crates/example/src/lib.rs",
            )],
        );
        assert_eq!(suggestion, None);
    }

    #[test]
    fn split_planner_groups_by_community() {
        let suggestion = suggest_split(
            "crates/example/src/lib.rs",
            68,
            &[
                PlannerCommunityAssignment {
                    symbol_id: "sym-a".to_owned(),
                    community_id: 1,
                },
                PlannerCommunityAssignment {
                    symbol_id: "sym-b".to_owned(),
                    community_id: 1,
                },
                PlannerCommunityAssignment {
                    symbol_id: "sym-c".to_owned(),
                    community_id: 2,
                },
            ],
            &[
                symbol("sym-a", "crate::sir_alpha", "crates/example/src/lib.rs"),
                symbol("sym-b", "crate::sir_beta", "crates/example/src/lib.rs"),
                symbol("sym-c", "crate::note_gamma", "crates/example/src/lib.rs"),
            ],
        )
        .expect("split suggestion");

        assert_eq!(suggestion.suggested_modules.len(), 2);
        assert!(suggestion
            .suggested_modules
            .iter()
            .any(|module| module.symbols == vec!["sir_alpha".to_owned(), "sir_beta".to_owned()]));
        assert!(
            suggestion
                .suggested_modules
                .iter()
                .any(|module| module.symbols == vec!["note_gamma".to_owned()])
        );
    }

    #[test]
    fn split_planner_names_modules_from_symbols() {
        let suggestion = suggest_split(
            "crates/example/src/lib.rs",
            70,
            &[
                PlannerCommunityAssignment {
                    symbol_id: "sym-a".to_owned(),
                    community_id: 1,
                },
                PlannerCommunityAssignment {
                    symbol_id: "sym-b".to_owned(),
                    community_id: 1,
                },
                PlannerCommunityAssignment {
                    symbol_id: "sym-c".to_owned(),
                    community_id: 2,
                },
            ],
            &[
                symbol("sym-a", "crate::sir_render", "crates/example/src/lib.rs"),
                symbol("sym-b", "crate::sir_parse", "crates/example/src/lib.rs"),
                symbol("sym-c", "crate::note_index", "crates/example/src/lib.rs"),
            ],
        )
        .expect("split suggestion");

        assert!(
            suggestion
                .suggested_modules
                .iter()
                .any(|module| module.name == "sir_ops")
        );
        assert_eq!(suggestion.confidence, SplitConfidence::High);
    }
}
