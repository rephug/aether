use std::collections::{BTreeMap, HashMap, HashSet};

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

#[derive(Debug, Clone)]
pub struct TraitMethod {
    pub name: String,
    pub qualified_name: String,
    pub symbol_id: String,
}

#[derive(Debug, Clone)]
pub struct ConsumerMethodUsage {
    pub consumer_file: String,
    pub methods_used: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraitSplitSuggestion {
    pub trait_name: String,
    pub trait_file: String,
    pub method_count: usize,
    pub suggested_traits: Vec<SuggestedSubTrait>,
    pub cross_cutting_methods: Vec<CrossCuttingMethod>,
    pub uncalled_methods: Vec<String>,
    pub confidence: SplitConfidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestedSubTrait {
    pub name: String,
    pub methods: Vec<String>,
    pub consumer_files: Vec<String>,
    pub consumer_isolation: f32,
    pub dominant_dependencies: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossCuttingMethod {
    pub method: String,
    pub overlapping_clusters: Vec<String>,
    pub reason: String,
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
    if crate_score > 50 {
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

pub fn suggest_trait_split(
    trait_name: &str,
    trait_file: &str,
    methods: &[TraitMethod],
    consumer_matrix: &[ConsumerMethodUsage],
    method_dependencies: Option<&HashMap<String, Vec<String>>>,
) -> Option<TraitSplitSuggestion> {
    if methods.len() < 2 || consumer_matrix.is_empty() {
        return None;
    }

    let trait_name = display_trait_name(trait_name);
    let trait_file = normalize_path(trait_file.trim());
    if trait_name.is_empty() || trait_file.is_empty() {
        return None;
    }

    let method_names = methods
        .iter()
        .map(display_trait_method_name)
        .collect::<Vec<_>>();
    let method_index = method_names
        .iter()
        .enumerate()
        .map(|(index, name)| (name.as_str(), index))
        .collect::<HashMap<_, _>>();

    let mut method_consumers = vec![HashSet::<usize>::new(); methods.len()];
    let mut consumer_method_sets = Vec::with_capacity(consumer_matrix.len());
    for (consumer_index, usage) in consumer_matrix.iter().enumerate() {
        let mut used_methods = HashSet::new();
        for method_name in &usage.methods_used {
            let normalized = method_name.trim().trim_start_matches("r#");
            let Some(&method_index) = method_index.get(normalized) else {
                continue;
            };
            method_consumers[method_index].insert(consumer_index);
            used_methods.insert(method_index);
        }
        consumer_method_sets.push(used_methods);
    }

    let mut uncalled_methods = method_consumers
        .iter()
        .enumerate()
        .filter(|(_, consumers)| consumers.is_empty())
        .map(|(index, _)| method_names[index].clone())
        .collect::<Vec<_>>();
    uncalled_methods.sort();

    let mut clusters_by_consumers = BTreeMap::<Vec<usize>, Vec<usize>>::new();
    for (method_index, consumers) in method_consumers.iter().enumerate() {
        if consumers.is_empty() {
            continue;
        }
        let mut key = consumers.iter().copied().collect::<Vec<_>>();
        key.sort_unstable();
        clusters_by_consumers
            .entry(key)
            .or_default()
            .push(method_index);
    }

    let mut clusters = clusters_by_consumers.into_values().collect::<Vec<_>>();
    for cluster in &mut clusters {
        cluster.sort_by(|left, right| method_names[*left].cmp(&method_names[*right]));
    }

    merge_similar_trait_clusters(&mut clusters, &method_consumers, &method_names);

    if clusters.is_empty() || (clusters.len() < 2 && uncalled_methods.is_empty()) {
        return None;
    }

    let cluster_consumers = clusters
        .iter()
        .map(|cluster| cluster_consumer_union(cluster, &method_consumers))
        .collect::<Vec<_>>();
    let cluster_method_sets = clusters
        .iter()
        .map(|cluster| cluster.iter().copied().collect::<HashSet<_>>())
        .collect::<Vec<_>>();
    let dominant_dependencies = clusters
        .iter()
        .map(|cluster| dominant_dependencies_for_cluster(cluster, methods, method_dependencies))
        .collect::<Vec<_>>();
    let cluster_name_tokens = clusters
        .iter()
        .enumerate()
        .map(|(index, cluster)| {
            if !dominant_dependencies[index].is_empty() {
                ranked_identifier_tokens(dominant_dependencies[index].as_slice())
            } else {
                let names = cluster
                    .iter()
                    .map(|method_index| method_names[*method_index].clone())
                    .collect::<Vec<_>>();
                ranked_identifier_tokens(names.as_slice())
            }
        })
        .collect::<Vec<_>>();
    let cluster_names =
        disambiguated_trait_names(trait_name.as_str(), cluster_name_tokens.as_slice());

    let mut suggested_traits = Vec::with_capacity(clusters.len());
    for (index, cluster) in clusters.iter().enumerate() {
        let mut cluster_methods = cluster
            .iter()
            .map(|method_index| method_names[*method_index].clone())
            .collect::<Vec<_>>();
        cluster_methods.sort();

        let mut consumer_files = cluster_consumers[index]
            .iter()
            .filter_map(|consumer_index| {
                consumer_matrix
                    .get(*consumer_index)
                    .map(|usage| usage.consumer_file.clone())
            })
            .collect::<Vec<_>>();
        consumer_files.sort();
        consumer_files.dedup();

        let consumer_isolation = cluster_isolation(
            &cluster_consumers[index],
            &consumer_method_sets,
            &cluster_method_sets[index],
        );

        let reason = if dominant_dependencies[index].is_empty() {
            format!(
                "Co-consumed by {} files with {:.0}% isolated usage",
                consumer_files.len(),
                consumer_isolation * 100.0
            )
        } else {
            format!(
                "Co-consumed by {} files with {:.0}% isolated usage; dominant deps: {}",
                consumer_files.len(),
                consumer_isolation * 100.0,
                dominant_dependencies[index].join(", ")
            )
        };

        suggested_traits.push(SuggestedSubTrait {
            name: cluster_names[index].clone(),
            methods: cluster_methods,
            consumer_files,
            consumer_isolation,
            dominant_dependencies: dominant_dependencies[index].clone(),
            reason,
        });
    }

    let mut cross_cutting_methods = Vec::new();
    for (method_index, consumers) in method_consumers.iter().enumerate() {
        if consumers.is_empty() {
            continue;
        }

        let mut overlapping_clusters = Vec::new();
        for (cluster_index, cluster_consumer_set) in cluster_consumers.iter().enumerate() {
            if cluster_method_sets[cluster_index].contains(&method_index) {
                continue;
            }
            if significant_overlap_ratio(consumers, cluster_consumer_set) >= 0.5 {
                overlapping_clusters.push(cluster_names[cluster_index].clone());
            }
        }

        if overlapping_clusters.len() >= 2 {
            overlapping_clusters.sort();
            cross_cutting_methods.push(CrossCuttingMethod {
                method: method_names[method_index].clone(),
                reason: format!("Consumers overlap {} clusters", overlapping_clusters.len()),
                overlapping_clusters,
            });
        }
    }
    cross_cutting_methods.sort_by(|left, right| left.method.cmp(&right.method));

    let low_isolation_count = suggested_traits
        .iter()
        .filter(|suggestion| suggestion.consumer_isolation < 0.3)
        .count();
    let confidence = if !suggested_traits.is_empty()
        && suggested_traits
            .iter()
            .all(|suggestion| suggestion.consumer_isolation >= 0.6)
        && cross_cutting_methods.is_empty()
    {
        SplitConfidence::High
    } else if cross_cutting_methods.len() >= 3
        || low_isolation_count >= 2
        || low_isolation_count * 2 >= suggested_traits.len().max(1)
    {
        SplitConfidence::Low
    } else {
        SplitConfidence::Medium
    };

    Some(TraitSplitSuggestion {
        trait_name,
        trait_file,
        method_count: methods.len(),
        suggested_traits,
        cross_cutting_methods,
        uncalled_methods,
        confidence,
    })
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

fn merge_similar_trait_clusters(
    clusters: &mut Vec<Vec<usize>>,
    method_consumers: &[HashSet<usize>],
    method_names: &[String],
) {
    loop {
        let mut best_merge: Option<(usize, usize)> = None;
        let mut best_score = 0.0_f32;

        for from_index in 0..clusters.len() {
            if clusters[from_index].len() != 1 {
                continue;
            }
            let from_consumers = cluster_consumer_union(&clusters[from_index], method_consumers);
            if from_consumers.is_empty() {
                continue;
            }

            for to_index in 0..clusters.len() {
                if from_index == to_index {
                    continue;
                }
                let to_consumers = cluster_consumer_union(&clusters[to_index], method_consumers);
                let score = jaccard_similarity(&from_consumers, &to_consumers);
                if score < 0.8 {
                    continue;
                }

                let is_better = score > best_score
                    || (score == best_score
                        && best_merge
                            .map(|(best_from, best_to)| {
                                let from_key = trait_cluster_merge_key(
                                    clusters[from_index].as_slice(),
                                    method_names,
                                );
                                let best_from_key = trait_cluster_merge_key(
                                    clusters[best_from].as_slice(),
                                    method_names,
                                );
                                let to_key = trait_cluster_merge_key(
                                    clusters[to_index].as_slice(),
                                    method_names,
                                );
                                let best_to_key = trait_cluster_merge_key(
                                    clusters[best_to].as_slice(),
                                    method_names,
                                );
                                from_key < best_from_key
                                    || (from_key == best_from_key && to_key < best_to_key)
                            })
                            .unwrap_or(true));
                if is_better {
                    best_score = score;
                    best_merge = Some((from_index, to_index));
                }
            }
        }

        let Some((from_index, to_index)) = best_merge else {
            break;
        };

        let moved = clusters.remove(from_index);
        let target_index = if from_index < to_index {
            to_index.saturating_sub(1)
        } else {
            to_index
        };
        clusters[target_index].extend(moved);
        clusters[target_index]
            .sort_by(|left, right| method_names[*left].cmp(&method_names[*right]));
        clusters[target_index].dedup();
    }

    clusters.sort_by(|left, right| {
        trait_cluster_merge_key(left, method_names)
            .cmp(&trait_cluster_merge_key(right, method_names))
            .then_with(|| right.len().cmp(&left.len()))
    });
}

fn trait_cluster_merge_key(cluster: &[usize], method_names: &[String]) -> String {
    cluster
        .iter()
        .map(|method_index| method_names[*method_index].as_str())
        .min()
        .unwrap_or("")
        .to_owned()
}

fn cluster_consumer_union(
    cluster: &[usize],
    method_consumers: &[HashSet<usize>],
) -> HashSet<usize> {
    let mut consumers = HashSet::new();
    for method_index in cluster {
        if let Some(method_consumer_set) = method_consumers.get(*method_index) {
            consumers.extend(method_consumer_set.iter().copied());
        }
    }
    consumers
}

fn cluster_isolation(
    cluster_consumers: &HashSet<usize>,
    consumer_method_sets: &[HashSet<usize>],
    cluster_methods: &HashSet<usize>,
) -> f32 {
    if cluster_consumers.is_empty() {
        return 0.0;
    }

    let exclusive_consumers = cluster_consumers
        .iter()
        .filter(|consumer_index| {
            consumer_method_sets
                .get(**consumer_index)
                .map(|methods| {
                    methods
                        .iter()
                        .all(|method_index| cluster_methods.contains(method_index))
                })
                .unwrap_or(false)
        })
        .count();

    exclusive_consumers as f32 / cluster_consumers.len() as f32
}

fn jaccard_similarity(left: &HashSet<usize>, right: &HashSet<usize>) -> f32 {
    if left.is_empty() && right.is_empty() {
        return 1.0;
    }
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }

    let intersection = left.intersection(right).count();
    let union = left.union(right).count();
    if union == 0 {
        0.0
    } else {
        intersection as f32 / union as f32
    }
}

fn significant_overlap_ratio(left: &HashSet<usize>, right: &HashSet<usize>) -> f32 {
    let denominator = left.len().min(right.len());
    if denominator == 0 {
        return 0.0;
    }

    left.intersection(right).count() as f32 / denominator as f32
}

fn dominant_dependencies_for_cluster(
    cluster: &[usize],
    methods: &[TraitMethod],
    method_dependencies: Option<&HashMap<String, Vec<String>>>,
) -> Vec<String> {
    let Some(method_dependencies) = method_dependencies else {
        return Vec::new();
    };

    let mut counts = BTreeMap::<String, usize>::new();
    for method_index in cluster {
        let Some(method) = methods.get(*method_index) else {
            continue;
        };
        for dependency in method_dependency_values(method, method_dependencies) {
            let display = display_dependency_name(dependency);
            if display.is_empty() {
                continue;
            }
            *counts.entry(display).or_default() += 1;
        }
    }

    let mut ranked = counts.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    ranked
        .into_iter()
        .map(|(dependency, _)| dependency)
        .take(3)
        .collect()
}

fn method_dependency_values<'a>(
    method: &'a TraitMethod,
    method_dependencies: &'a HashMap<String, Vec<String>>,
) -> &'a [String] {
    let display_name = display_trait_method_name(method);
    let qualified_leaf = method
        .qualified_name
        .rsplit("::")
        .next()
        .unwrap_or(method.qualified_name.as_str())
        .trim_start_matches("r#");

    method_dependencies
        .get(method.name.as_str())
        .or_else(|| method_dependencies.get(display_name.as_str()))
        .or_else(|| method_dependencies.get(method.qualified_name.as_str()))
        .or_else(|| method_dependencies.get(qualified_leaf))
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn display_dependency_name(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let without_generics = trimmed.split('<').next().unwrap_or(trimmed);
    let leaf = without_generics
        .rsplit("::")
        .next()
        .unwrap_or(without_generics)
        .rsplit('.')
        .next()
        .unwrap_or(without_generics)
        .trim_matches(|character: char| !character.is_alphanumeric() && character != '_');

    leaf.to_owned()
}

fn ranked_identifier_tokens(values: &[String]) -> Vec<String> {
    let mut counts = BTreeMap::<String, usize>::new();
    for value in values {
        for token in identifier_tokens(value) {
            *counts.entry(token).or_default() += 1;
        }
    }

    let mut ranked = counts.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    ranked.into_iter().map(|(token, _)| token).collect()
}

fn identifier_tokens(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let characters = value.chars().collect::<Vec<_>>();

    for (index, character) in characters.iter().enumerate() {
        if !character.is_ascii_alphanumeric() {
            push_identifier_token(&mut tokens, &mut current);
            continue;
        }

        let previous = if index > 0 {
            Some(characters[index - 1])
        } else {
            None
        };
        let next = characters.get(index + 1).copied();
        let starts_new_token = character.is_ascii_uppercase()
            && !current.is_empty()
            && previous.is_some_and(|previous| {
                previous.is_ascii_lowercase()
                    || (previous.is_ascii_uppercase()
                        && next.is_some_and(|next| next.is_ascii_lowercase()))
            });
        if starts_new_token {
            push_identifier_token(&mut tokens, &mut current);
        }

        current.push(character.to_ascii_lowercase());
    }

    push_identifier_token(&mut tokens, &mut current);
    tokens
}

fn push_identifier_token(tokens: &mut Vec<String>, current: &mut String) {
    if current.is_empty() {
        return;
    }

    let normalized = normalize_token(current.as_str());
    if !STOPWORDS.contains(&normalized.as_str()) {
        tokens.push(normalized);
    }
    current.clear();
}

fn disambiguated_trait_names(trait_name: &str, token_lists: &[Vec<String>]) -> Vec<String> {
    let mut names = Vec::with_capacity(token_lists.len());
    let mut used = HashSet::<String>::new();

    for (index, tokens) in token_lists.iter().enumerate() {
        let mut depth = if tokens.is_empty() { 0 } else { 1 };
        loop {
            let candidate = trait_name_for_tokens(trait_name, tokens, depth, index);
            if used.insert(candidate.clone()) {
                names.push(candidate);
                break;
            }

            if depth < tokens.len() {
                depth += 1;
                continue;
            }

            let suffixed = format!("{candidate}{}", index + 1);
            if used.insert(suffixed.clone()) {
                names.push(suffixed);
                break;
            }
        }
    }

    names
}

fn trait_name_for_tokens(
    trait_name: &str,
    tokens: &[String],
    depth: usize,
    fallback_index: usize,
) -> String {
    if tokens.is_empty() || depth == 0 {
        return format!("{trait_name}Cluster{}", fallback_index + 1);
    }

    let prefix = tokens
        .iter()
        .take(depth.min(tokens.len()))
        .map(|token| pascal_case_token(token))
        .collect::<String>();
    format!("{prefix}{trait_name}")
}

fn pascal_case_token(token: &str) -> String {
    let mut characters = token.chars();
    let Some(first) = characters.next() else {
        return String::new();
    };

    let mut value = String::new();
    value.push(first.to_ascii_uppercase());
    value.push_str(characters.as_str());
    value
}

fn display_trait_name(trait_name: &str) -> String {
    trait_name
        .trim()
        .rsplit("::")
        .next()
        .unwrap_or(trait_name.trim())
        .trim_start_matches("r#")
        .to_owned()
}

fn display_trait_method_name(method: &TraitMethod) -> String {
    if !method.name.trim().is_empty() {
        return method.name.trim().trim_start_matches("r#").to_owned();
    }

    method
        .qualified_name
        .rsplit("::")
        .next()
        .unwrap_or(method.qualified_name.as_str())
        .trim_start_matches("r#")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::{
        ConsumerMethodUsage, FileCommunityConfig, FileSymbol, SplitConfidence, TraitMethod,
        disambiguated_module_names, fallback_module_name, module_name_for_tokens, normalize_token,
        ranked_tokens, split_confidence, suggest_split, suggest_trait_split, suggested_file_path,
    };
    use crate::planner_communities::detect_file_communities;
    use aether_core::SymbolKind;
    use aether_graph_algo::GraphAlgorithmEdge;
    use std::collections::HashMap;

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

    fn trait_method(name: &str) -> TraitMethod {
        TraitMethod {
            name: name.to_owned(),
            qualified_name: format!("crate::Store::{name}"),
            symbol_id: format!("sym-{name}"),
        }
    }

    fn consumer_usage(consumer_file: &str, methods_used: &[&str]) -> ConsumerMethodUsage {
        ConsumerMethodUsage {
            consumer_file: consumer_file.to_owned(),
            methods_used: methods_used
                .iter()
                .map(|method| (*method).to_owned())
                .collect(),
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
            30,
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
            30,
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

    #[test]
    fn trait_split_basic_clustering_forms_two_groups() {
        let methods = vec![
            trait_method("alpha"),
            trait_method("beta"),
            trait_method("gamma"),
            trait_method("delta"),
        ];
        let consumers = vec![
            consumer_usage("src/consumer_a.rs", &["alpha", "beta"]),
            consumer_usage("src/consumer_b.rs", &["gamma", "delta"]),
        ];

        let suggestion = suggest_trait_split("Store", "src/store.rs", &methods, &consumers, None)
            .expect("trait split suggestion");

        assert_eq!(suggestion.suggested_traits.len(), 2);
        assert!(
            suggestion
                .suggested_traits
                .iter()
                .any(|cluster| cluster.methods == vec!["alpha".to_owned(), "beta".to_owned()])
        );
        assert!(
            suggestion
                .suggested_traits
                .iter()
                .any(|cluster| cluster.methods == vec!["delta".to_owned(), "gamma".to_owned()])
        );
    }

    #[test]
    fn trait_split_overlapping_consumers_preserves_middle_cluster() {
        let methods = vec![
            trait_method("alpha"),
            trait_method("beta"),
            trait_method("gamma"),
            trait_method("delta"),
        ];
        let consumers = vec![
            consumer_usage("src/consumer_a.rs", &["alpha", "beta", "gamma"]),
            consumer_usage("src/consumer_b.rs", &["beta", "gamma", "delta"]),
        ];

        let suggestion = suggest_trait_split("Store", "src/store.rs", &methods, &consumers, None)
            .expect("trait split suggestion");

        let mut clusters = suggestion
            .suggested_traits
            .iter()
            .map(|cluster| cluster.methods.clone())
            .collect::<Vec<_>>();
        clusters.sort();
        assert_eq!(
            clusters,
            vec![
                vec!["alpha".to_owned()],
                vec!["beta".to_owned(), "gamma".to_owned()],
                vec!["delta".to_owned()],
            ]
        );
    }

    #[test]
    fn trait_split_flags_cross_cutting_methods() {
        let methods = vec![
            trait_method("alpha"),
            trait_method("beta"),
            trait_method("gamma"),
        ];
        let consumers = vec![
            consumer_usage("src/consumer_a.rs", &["alpha", "beta"]),
            consumer_usage("src/consumer_b.rs", &["alpha", "gamma"]),
            consumer_usage("src/consumer_c.rs", &["alpha"]),
        ];

        let suggestion = suggest_trait_split("Store", "src/store.rs", &methods, &consumers, None)
            .expect("trait split suggestion");

        assert_eq!(suggestion.cross_cutting_methods.len(), 1);
        assert_eq!(suggestion.cross_cutting_methods[0].method, "alpha");
        assert_eq!(
            suggestion.cross_cutting_methods[0]
                .overlapping_clusters
                .len(),
            2
        );
    }

    #[test]
    fn trait_split_reports_uncalled_methods_separately() {
        let methods = vec![
            trait_method("alpha"),
            trait_method("beta"),
            trait_method("gamma"),
            trait_method("delta"),
            trait_method("omega"),
        ];
        let consumers = vec![
            consumer_usage("src/consumer_a.rs", &["alpha", "beta"]),
            consumer_usage("src/consumer_b.rs", &["gamma", "delta"]),
        ];

        let suggestion = suggest_trait_split("Store", "src/store.rs", &methods, &consumers, None)
            .expect("trait split suggestion");

        assert_eq!(suggestion.uncalled_methods, vec!["omega".to_owned()]);
        assert_eq!(suggestion.suggested_traits.len(), 2);
    }

    #[test]
    fn trait_split_names_clusters_from_method_dependencies() {
        let methods = vec![
            trait_method("write_blob"),
            trait_method("read_blob"),
            trait_method("upsert_symbol"),
            trait_method("list_symbols"),
        ];
        let consumers = vec![
            consumer_usage("src/sir_consumer.rs", &["write_blob", "read_blob"]),
            consumer_usage("src/symbol_consumer.rs", &["upsert_symbol", "list_symbols"]),
        ];
        let method_dependencies = HashMap::from([
            (
                "write_blob".to_owned(),
                vec!["SirMetaRecord".to_owned(), "SirBlob".to_owned()],
            ),
            (
                "read_blob".to_owned(),
                vec!["SirMetaRecord".to_owned(), "SirBlob".to_owned()],
            ),
            ("upsert_symbol".to_owned(), vec!["SymbolRecord".to_owned()]),
            ("list_symbols".to_owned(), vec!["SymbolRecord".to_owned()]),
        ]);

        let suggestion = suggest_trait_split(
            "Store",
            "src/store.rs",
            &methods,
            &consumers,
            Some(&method_dependencies),
        )
        .expect("trait split suggestion");

        assert!(
            suggestion
                .suggested_traits
                .iter()
                .any(|cluster| cluster.name.contains("Sir") && cluster.name.ends_with("Store"))
        );
        assert!(
            suggestion
                .suggested_traits
                .iter()
                .any(|cluster| cluster.dominant_dependencies
                    == vec!["SirBlob".to_owned(), "SirMetaRecord".to_owned()]
                    || cluster.dominant_dependencies
                        == vec!["SirMetaRecord".to_owned(), "SirBlob".to_owned()])
        );
    }
}
