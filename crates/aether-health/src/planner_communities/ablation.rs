use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use aether_config::load_workspace_config;
use aether_core::SymbolKind;
use aether_graph_algo::GraphAlgorithmEdge;
use aether_store::{SqliteStore, Store, SurrealGraphStore, open_vector_store};

use super::anchors::{
    build_anchor_groups, count_type_anchored_symbols, rebuild_union_find_from_groups,
    split_large_anchor_groups,
};
use super::graph::{WeightedGraph, build_rep_members, collapse_structural_edges};
use super::merge::{finalize_assignments, merge_small_communities};
use super::rescue::{apply_container_rescue_with_exclusions, apply_semantic_rescue, has_embedding};
use super::{
    DetectionRun, DisjointSet, FileCommunityConfig, FileSymbol, PlannerDiagnostics, SymbolEntry,
    compute_confidence, diagnostics_from_run, pairwise_jaccard, qualified_name_stem,
};

const ABLATION_STOPWORDS: &[&str] = &[
    "default", "new", "from", "into", "load", "save", "get", "set", "is", "has", "with", "for",
    "the", "and", "fn", "test", "mock", "impl", "try", "run", "do",
];

#[derive(Clone, Copy)]
struct AblationOptions {
    filter_tests: bool,
    type_anchor: bool,
    container_rescue: bool,
    semantic_rescue: bool,
    community_resolution: f64,
    merge_small: bool,
}

#[derive(Clone)]
struct AblationInput {
    crate_name: String,
    file_path: String,
    symbols: Vec<FileSymbol>,
    edges: Vec<GraphAlgorithmEdge>,
    config: FileCommunityConfig,
}

fn count_components_and_largest(
    graph: &WeightedGraph,
    rep_to_members: &[Vec<usize>],
    entries: &[SymbolEntry],
) -> (usize, usize) {
    let active_reps: Vec<usize> = rep_to_members
        .iter()
        .enumerate()
        .filter_map(
            |(rep, members)| {
                if members.is_empty() { None } else { Some(rep) }
            },
        )
        .collect();
    if active_reps.is_empty() {
        return (0, 0);
    }
    let components = graph.connected_components(&active_reps, entries);
    let largest = components
        .iter()
        .map(|component| {
            component
                .iter()
                .map(|rep| rep_to_members.get(*rep).map(Vec::len).unwrap_or(0))
                .sum::<usize>()
        })
        .max()
        .unwrap_or(0);
    (components.len(), largest)
}

#[test]
#[ignore]
fn ablation_aether_store() {
    run_ablation_report("aether-store");
}

#[test]
#[ignore]
fn ablation_aether_config() {
    run_ablation_report("aether-config");
}

#[test]
#[ignore]
fn ablation_aether_mcp() {
    run_ablation_report("aether-mcp");
}

fn run_ablation_report(crate_name: &str) {
    let Some(input) = load_ablation_input(crate_name) else {
        return;
    };
    let variants = [
        (
            "1. baseline",
            AblationOptions {
                filter_tests: false,
                type_anchor: false,
                container_rescue: false,
                semantic_rescue: false,
                community_resolution: 1.0,
                merge_small: false,
            },
        ),
        (
            "2. + test filtering",
            AblationOptions {
                filter_tests: true,
                type_anchor: false,
                container_rescue: false,
                semantic_rescue: false,
                community_resolution: 1.0,
                merge_small: false,
            },
        ),
        (
            "3. + type-anchor",
            AblationOptions {
                filter_tests: true,
                type_anchor: true,
                container_rescue: false,
                semantic_rescue: false,
                community_resolution: 1.0,
                merge_small: false,
            },
        ),
        (
            "4. + rescue",
            AblationOptions {
                filter_tests: true,
                type_anchor: true,
                container_rescue: true,
                semantic_rescue: true,
                community_resolution: 1.0,
                merge_small: false,
            },
        ),
        (
            "5. + lower gamma",
            AblationOptions {
                filter_tests: true,
                type_anchor: true,
                container_rescue: true,
                semantic_rescue: true,
                community_resolution: 0.5,
                merge_small: false,
            },
        ),
        (
            "6. full pipeline",
            AblationOptions {
                filter_tests: true,
                type_anchor: true,
                container_rescue: true,
                semantic_rescue: true,
                community_resolution: input.config.community_resolution,
                merge_small: true,
            },
        ),
    ];

    println!("\nAblation for {} ({})", input.crate_name, input.file_path);
    println!(
        "{:<28} {:>11} {:>8} {:>9} {:>8} {:>8} {:>10} {}",
        "configuration",
        "communities",
        "largest",
        "smallest",
        "loners",
        "conf",
        "stability",
        "top modules"
    );

    for (label, options) in variants {
        let (assignments, diagnostics) =
            run_ablation_detection(&input.edges, &input.symbols, &input.config, options);
        let sizes = community_sizes(&assignments);
        let largest = sizes.iter().copied().max().unwrap_or(0);
        let smallest = sizes.iter().copied().min().unwrap_or(0);
        let top_modules = ablation_module_names(&input.symbols, &assignments)
            .into_iter()
            .take(3)
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "{:<28} {:>11} {:>8} {:>9} {:>8} {:>8.2} {:>10.2} {}",
            label,
            sizes.len(),
            largest,
            smallest,
            diagnostics.symbols_loner,
            diagnostics.confidence,
            diagnostics.stability_score,
            if top_modules.is_empty() {
                "-".to_owned()
            } else {
                top_modules
            }
        );
    }
}

fn load_ablation_input(crate_name: &str) -> Option<AblationInput> {
    let workspace = std::path::Path::new("/home/rephu/projects/aether");
    let aether_dir = workspace.join(".aether");
    if !aether_dir.exists() {
        eprintln!(
            "ablation skipped for {crate_name}: {} is missing",
            aether_dir.display()
        );
        return None;
    }

    let config = match load_workspace_config(workspace) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("ablation skipped for {crate_name}: failed to load config: {err}");
            return None;
        }
    };
    let Some(model_name) = config.embeddings.model.clone() else {
        eprintln!("ablation skipped for {crate_name}: embeddings.model is not configured");
        return None;
    };

    let file_path = format!("crates/{crate_name}/src/lib.rs");
    let store = match SqliteStore::open_readonly(workspace) {
        Ok(store) => store,
        Err(err) => {
            eprintln!("ablation skipped for {crate_name}: failed to open sqlite store: {err}");
            return None;
        }
    };
    let symbol_records = match store.list_symbols_for_file(file_path.as_str()) {
        Ok(records) if !records.is_empty() => records,
        Ok(_) => {
            eprintln!("ablation skipped for {crate_name}: no indexed symbols for {file_path}");
            return None;
        }
        Err(err) => {
            eprintln!("ablation skipped for {crate_name}: failed to load symbols: {err}");
            return None;
        }
    };

    let symbol_ids = symbol_records
        .iter()
        .map(|record| record.id.clone())
        .collect::<Vec<_>>();
    let symbol_id_set = symbol_ids
        .iter()
        .map(|symbol_id| symbol_id.as_str())
        .collect::<HashSet<_>>();

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("ablation skipped for {crate_name}: failed to build runtime: {err}");
            return None;
        }
    };

    let graph = match runtime.block_on(SurrealGraphStore::open_readonly(workspace)) {
        Ok(graph) => graph,
        Err(err) => {
            eprintln!("ablation skipped for {crate_name}: failed to open surreal graph: {err}");
            return None;
        }
    };
    let edges = match runtime.block_on(graph.list_dependency_edges()) {
        Ok(records) => records
            .into_iter()
            .filter(|edge| {
                symbol_id_set.contains(edge.source_symbol_id.as_str())
                    && symbol_id_set.contains(edge.target_symbol_id.as_str())
            })
            .map(|edge| GraphAlgorithmEdge {
                source_id: edge.source_symbol_id,
                target_id: edge.target_symbol_id,
                edge_kind: edge.edge_kind,
            })
            .collect::<Vec<_>>(),
        Err(err) => {
            eprintln!("ablation skipped for {crate_name}: failed to load dependency edges: {err}");
            return None;
        }
    };
    let vector_store = match runtime.block_on(open_vector_store(workspace)) {
        Ok(store) => store,
        Err(err) => {
            eprintln!("ablation skipped for {crate_name}: failed to open vector store: {err}");
            return None;
        }
    };
    let embedding_by_id = match runtime.block_on(vector_store.list_embeddings_for_symbols(
        config.embeddings.provider.as_str(),
        model_name.as_str(),
        symbol_ids.as_slice(),
    )) {
        Ok(records) => records
            .into_iter()
            .map(|record| (record.symbol_id, record.embedding))
            .collect::<HashMap<_, _>>(),
        Err(err) => {
            eprintln!("ablation skipped for {crate_name}: failed to load embeddings: {err}");
            return None;
        }
    };

    let symbols = symbol_records
        .into_iter()
        .map(|record| FileSymbol {
            symbol_id: record.id.clone(),
            name: ablation_symbol_name(record.qualified_name.as_str()),
            qualified_name: record.qualified_name.clone(),
            kind: ablation_parse_symbol_kind(record.kind.as_str()),
            is_test: ablation_is_test(record.qualified_name.as_str(), record.file_path.as_str()),
            embedding: embedding_by_id.get(record.id.as_str()).cloned(),
        })
        .collect::<Vec<_>>();

    Some(AblationInput {
        crate_name: crate_name.to_owned(),
        file_path,
        symbols,
        edges,
        config: FileCommunityConfig {
            semantic_rescue_threshold: config.planner.semantic_rescue_threshold,
            semantic_rescue_max_k: config.planner.semantic_rescue_max_k,
            community_resolution: config.planner.community_resolution,
            min_community_size: config.planner.min_community_size,
        },
    })
}

fn run_ablation_detection(
    structural_edges: &[GraphAlgorithmEdge],
    symbols: &[FileSymbol],
    config: &FileCommunityConfig,
    options: AblationOptions,
) -> (Vec<(String, usize)>, PlannerDiagnostics) {
    let baseline = run_ablation_pass(structural_edges, symbols, config, options);
    if baseline.non_test_count == 0 || baseline.assignments.is_empty() {
        let diagnostics = diagnostics_from_run(&baseline, 0.0, 0.0);
        return (baseline.assignments, diagnostics);
    }

    let mut threshold_config = config.clone();
    threshold_config.semantic_rescue_threshold =
        (threshold_config.semantic_rescue_threshold + 0.05).clamp(0.3, 0.95);
    let threshold_run = run_ablation_pass(structural_edges, symbols, &threshold_config, options);

    let mut resolution_config = config.clone();
    resolution_config.community_resolution = (options.community_resolution + 0.1).clamp(0.1, 3.0);
    let resolution_options = AblationOptions {
        community_resolution: resolution_config.community_resolution,
        ..options
    };
    let resolution_run = run_ablation_pass(
        structural_edges,
        symbols,
        &resolution_config,
        resolution_options,
    );

    let stability_score = pairwise_jaccard(
        baseline.assignments.as_slice(),
        threshold_run.assignments.as_slice(),
    )
    .min(pairwise_jaccard(
        baseline.assignments.as_slice(),
        resolution_run.assignments.as_slice(),
    ));
    let confidence = compute_confidence(&baseline, stability_score);
    let diagnostics = diagnostics_from_run(&baseline, stability_score, confidence);
    (baseline.assignments, diagnostics)
}

fn run_ablation_pass(
    structural_edges: &[GraphAlgorithmEdge],
    symbols: &[FileSymbol],
    config: &FileCommunityConfig,
    options: AblationOptions,
) -> DetectionRun {
    let symbols_total = symbols.len();
    let mut filtered_symbols = if options.filter_tests {
        symbols
            .iter()
            .filter(|symbol| !symbol.is_test)
            .cloned()
            .collect::<Vec<_>>()
    } else {
        symbols.to_vec()
    };
    filtered_symbols.sort_by(|left, right| left.symbol_id.cmp(&right.symbol_id));
    let symbols_filtered_test = if options.filter_tests {
        symbols_total.saturating_sub(filtered_symbols.len())
    } else {
        0
    };
    let non_test_count = filtered_symbols.len();

    let entries = filtered_symbols
        .into_iter()
        .map(|symbol| SymbolEntry {
            stem: qualified_name_stem(symbol.qualified_name.as_str()),
            symbol,
        })
        .collect::<Vec<_>>();
    if entries.is_empty() {
        return DetectionRun {
            assignments: Vec::new(),
            symbols_total,
            symbols_filtered_test,
            symbols_anchored_type: 0,
            symbols_rescued_container: 0,
            symbols_rescued_semantic: 0,
            symbols_loner: 0,
            communities_before_merge: 0,
            communities_after_merge: 0,
            embedding_coverage_pct: 0.0,
            non_test_count: 0,
            unmerged_small_penalty: 0.0,
        };
    }

    let id_to_index = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.symbol.symbol_id.clone(), index))
        .collect::<HashMap<_, _>>();
    let filtered_structural_edges = structural_edges
        .iter()
        .filter(|edge| {
            id_to_index.contains_key(edge.source_id.as_str())
                && id_to_index.contains_key(edge.target_id.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();

    let (mut union_find, initial_anchor_groups, split_anchor_exclusions) = if options.type_anchor {
        let (_anchor_union_find, anchor_groups) = build_anchor_groups(entries.as_slice());
        let (anchor_groups, split_anchor_exclusions) =
            split_large_anchor_groups(entries.as_slice(), anchor_groups);
        let union_find = rebuild_union_find_from_groups(entries.len(), anchor_groups.as_slice());
        (union_find, anchor_groups, split_anchor_exclusions)
    } else {
        let mut union_find = DisjointSet::new(entries.len());
        let anchor_groups = build_rep_members(entries.len(), &mut union_find);
        (union_find, anchor_groups, HashSet::new())
    };
    let symbols_anchored_type = if options.type_anchor {
        count_type_anchored_symbols(entries.as_slice(), initial_anchor_groups.as_slice())
    } else {
        0
    };
    let rep_to_members_diag = build_rep_members(entries.len(), &mut union_find);
    let (nc, nl) =
        count_components_and_largest(&WeightedGraph::default(), &rep_to_members_diag, &entries);
    eprintln!(
        "[diag] after_anchor_split: groups={} largest_group={}",
        nc, nl
    );

    let structural_graph = collapse_structural_edges(
        filtered_structural_edges.as_slice(),
        &id_to_index,
        &mut union_find,
    );
    let mut enriched_graph = structural_graph.clone();
    let rep_to_members = build_rep_members(entries.len(), &mut union_find);
    let rep_by_index = (0..entries.len())
        .map(|index| union_find.find(index))
        .collect::<Vec<_>>();
    let (nc, nl) = count_components_and_largest(&enriched_graph, &rep_to_members, &entries);
    eprintln!(
        "[diag] after_structural_edges: components={} largest_component={}",
        nc, nl
    );

    let symbols_rescued_container = if options.container_rescue {
        apply_container_rescue_with_exclusions(
            entries.as_slice(),
            rep_by_index.as_slice(),
            rep_to_members.as_slice(),
            &mut enriched_graph,
            &split_anchor_exclusions,
        )
    } else {
        0
    };
    let (nc, nl) = count_components_and_largest(&enriched_graph, &rep_to_members, &entries);
    eprintln!(
        "[diag] after_container_rescue: components={} largest_component={} rescued={}",
        nc, nl, symbols_rescued_container
    );
    let symbols_rescued_semantic = if options.semantic_rescue {
        apply_semantic_rescue(
            entries.as_slice(),
            rep_by_index.as_slice(),
            &mut enriched_graph,
            config,
        )
    } else {
        0
    };
    let (nc, nl) = count_components_and_largest(&enriched_graph, &rep_to_members, &entries);
    eprintln!(
        "[diag] after_semantic_rescue: components={} largest_component={} rescued={}",
        nc, nl, symbols_rescued_semantic
    );

    let loner_reps = rep_to_members
        .iter()
        .enumerate()
        .filter_map(|(rep, members)| {
            if members.is_empty() || enriched_graph.degree(rep) > 0 {
                None
            } else {
                Some(rep)
            }
        })
        .collect::<HashSet<_>>();
    let symbols_loner = loner_reps
        .iter()
        .filter_map(|rep| rep_to_members.get(*rep))
        .map(Vec::len)
        .sum();
    let embedding_coverage_pct = if non_test_count == 0 {
        0.0
    } else {
        entries
            .iter()
            .filter(|entry| has_embedding(entry.symbol.embedding.as_deref()))
            .count() as f32
            / non_test_count as f32
    };

    let active_reps = rep_to_members
        .iter()
        .enumerate()
        .filter_map(|(rep, members)| {
            if members.is_empty() || loner_reps.contains(&rep) {
                None
            } else {
                Some(rep)
            }
        })
        .collect::<Vec<_>>();
    if active_reps.is_empty() {
        return DetectionRun {
            assignments: Vec::new(),
            symbols_total,
            symbols_filtered_test,
            symbols_anchored_type,
            symbols_rescued_container,
            symbols_rescued_semantic,
            symbols_loner,
            communities_before_merge: 0,
            communities_after_merge: 0,
            embedding_coverage_pct,
            non_test_count,
            unmerged_small_penalty: 0.0,
        };
    }

    let components =
        enriched_graph.connected_components(active_reps.as_slice(), entries.as_slice());
    let component_sizes: Vec<usize> = components.iter().map(Vec::len).collect();
    eprintln!(
        "[diag] connected_components: count={} sizes={:?}",
        components.len(),
        component_sizes
    );
    let component_of_rep = components
        .iter()
        .enumerate()
        .flat_map(|(component_id, reps)| reps.iter().copied().map(move |rep| (rep, component_id)))
        .collect::<HashMap<_, _>>();
    let mut rep_to_community = HashMap::<usize, usize>::new();
    let mut next_community_id = 1usize;
    for component in &components {
        let local_edges = enriched_graph.repeated_component_edges(component.as_slice());
        let local_assignments = aether_graph_algo::louvain_with_resolution_sync(
            local_edges.as_slice(),
            options.community_resolution,
        );
        let mut local_to_global = BTreeMap::<usize, usize>::new();
        for (_, local_id) in &local_assignments {
            local_to_global.entry(*local_id).or_insert_with(|| {
                let assigned = next_community_id;
                next_community_id += 1;
                assigned
            });
        }

        for (rep_name, local_id) in local_assignments {
            let Some(rep) = rep_name
                .strip_prefix("rep-")
                .and_then(|value| value.parse::<usize>().ok())
            else {
                continue;
            };
            if let Some(global_id) = local_to_global.get(&local_id).copied() {
                rep_to_community.insert(rep, global_id);
            }
        }
    }
    let communities_before_merge = rep_to_community
        .values()
        .copied()
        .collect::<HashSet<_>>()
        .len();
    eprintln!(
        "[diag] after_louvain: communities={}",
        communities_before_merge
    );

    let (rep_to_community, unmerged_small_penalty, communities_after_merge) = if options.merge_small
    {
        let (merged, penalty) = merge_small_communities(
            rep_to_community,
            entries.as_slice(),
            rep_to_members.as_slice(),
            &component_of_rep,
            &structural_graph,
            config.min_community_size,
        );
        let count = merged.values().copied().collect::<HashSet<_>>().len();
        (merged, penalty, count)
    } else {
        (rep_to_community, 0.0, communities_before_merge)
    };

    let assignments = finalize_assignments(
        entries.as_slice(),
        rep_by_index.as_slice(),
        &loner_reps,
        &rep_to_community,
    );

    DetectionRun {
        assignments,
        symbols_total,
        symbols_filtered_test,
        symbols_anchored_type,
        symbols_rescued_container,
        symbols_rescued_semantic,
        symbols_loner,
        communities_before_merge,
        communities_after_merge,
        embedding_coverage_pct,
        non_test_count,
        unmerged_small_penalty,
    }
}

fn community_sizes(assignments: &[(String, usize)]) -> Vec<usize> {
    let mut sizes = BTreeMap::<usize, usize>::new();
    for (_, community_id) in assignments {
        *sizes.entry(*community_id).or_default() += 1;
    }
    sizes.into_values().collect()
}

fn ablation_module_names(symbols: &[FileSymbol], assignments: &[(String, usize)]) -> Vec<String> {
    let symbol_by_id = symbols
        .iter()
        .map(|symbol| (symbol.symbol_id.as_str(), symbol))
        .collect::<HashMap<_, _>>();
    let mut grouped = BTreeMap::<usize, Vec<String>>::new();
    for (symbol_id, community_id) in assignments {
        if let Some(symbol) = symbol_by_id.get(symbol_id.as_str()) {
            grouped
                .entry(*community_id)
                .or_default()
                .push(symbol.name.clone());
        }
    }

    let mut seen = BTreeSet::new();
    grouped
        .into_iter()
        .map(|(community_id, names)| {
            let mut counts = BTreeMap::<String, usize>::new();
            for name in names {
                for token in name.to_ascii_lowercase().split('_') {
                    if token.is_empty() {
                        continue;
                    }
                    let normalized = match token {
                        "note" | "notes" => "note",
                        "migration" | "migrate" => "migration",
                        "test" | "tests" => "test",
                        "store" | "stores" => "store",
                        other => other,
                    };
                    if ABLATION_STOPWORDS.contains(&normalized) {
                        continue;
                    }
                    *counts.entry(normalized.to_owned()).or_default() += 1;
                }
            }
            let mut ranked = counts.into_iter().collect::<Vec<_>>();
            ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
            let base = ranked
                .first()
                .map(|(token, _)| token.clone())
                .unwrap_or_else(|| format!("community_{community_id}"));
            let name = format!("{base}_ops");
            if seen.insert(name.clone()) {
                name
            } else {
                format!("{base}_{community_id}_ops")
            }
        })
        .collect()
}

fn ablation_symbol_name(qualified_name: &str) -> String {
    qualified_name
        .rsplit("::")
        .next()
        .unwrap_or(qualified_name)
        .trim_start_matches("r#")
        .to_owned()
}

fn ablation_parse_symbol_kind(raw: &str) -> SymbolKind {
    match raw.trim().to_ascii_lowercase().as_str() {
        "method" => SymbolKind::Method,
        "class" => SymbolKind::Class,
        "variable" => SymbolKind::Variable,
        "struct" => SymbolKind::Struct,
        "enum" => SymbolKind::Enum,
        "trait" => SymbolKind::Trait,
        "interface" => SymbolKind::Interface,
        "type_alias" | "typealias" => SymbolKind::TypeAlias,
        _ => SymbolKind::Function,
    }
}

fn ablation_is_test(qualified_name: &str, file_path: &str) -> bool {
    let leaf = ablation_symbol_name(qualified_name).to_ascii_lowercase();
    leaf.starts_with("test_") || file_path.contains("/tests/") || file_path.contains("\\tests\\")
}
