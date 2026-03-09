use crate::models::{Archetype, GitSignals, MetricPenalties, SemanticSignals};

pub fn assign_archetypes(
    _metrics: &crate::models::CrateMetrics,
    penalties: &MetricPenalties,
) -> Vec<Archetype> {
    structural_archetype_candidates(penalties)
        .into_iter()
        .map(|(archetype, _)| archetype)
        .collect()
}

pub fn assign_combined_archetypes(
    _metrics: &crate::models::CrateMetrics,
    penalties: &MetricPenalties,
    git: Option<&GitSignals>,
    semantic: Option<&SemanticSignals>,
) -> Vec<Archetype> {
    let mut candidates = structural_archetype_candidates(penalties);

    if let (Some(git), Some(semantic)) = (git, semantic) {
        let top_semantic = semantic
            .max_centrality
            .max(semantic.drift_density)
            .max(semantic.stale_sir_ratio)
            .max(semantic.test_gap)
            .max(semantic.boundary_leakage);

        if semantic.boundary_leakage > 0.6
            && semantic.boundary_leakage >= top_semantic - f64::EPSILON
        {
            candidates.push((Archetype::BoundaryLeaker, semantic.boundary_leakage));
        }

        if git.churn_30d < 0.1 && semantic.max_centrality > 0.6 {
            candidates.push((
                Archetype::ZombieFile,
                semantic.max_centrality * (1.0 - git.churn_30d),
            ));
        }

        if semantic.drift_density > 0.5 && git.churn_30d < 0.2 {
            candidates.push((
                Archetype::FalseStable,
                semantic.drift_density * (1.0 - git.churn_30d),
            ));
        }
    }

    candidates.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.0.as_str().cmp(right.0.as_str()))
    });

    candidates
        .into_iter()
        .filter(|(_, score)| *score > f64::EPSILON)
        .map(|(archetype, _)| archetype)
        .take(2)
        .collect()
}

fn structural_archetype_candidates(penalties: &MetricPenalties) -> Vec<(Archetype, f64)> {
    let total_score = penalties.total();
    if total_score < 25.0 {
        return Vec::new();
    }

    let mut buckets = [
        (
            Archetype::GodFile,
            penalties.max_file_loc.max(penalties.trait_method_max),
        ),
        (Archetype::BrittleHub, penalties.internal_dep_count),
        (
            Archetype::ChurnMagnet,
            penalties.todo_density.max(penalties.dead_feature_flags),
        ),
        (Archetype::LegacyResidue, penalties.stale_backend_refs),
    ];
    buckets.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.0.as_str().cmp(right.0.as_str()))
    });

    let Some(primary) = buckets.first().copied() else {
        return Vec::new();
    };
    if primary.1 <= f64::EPSILON {
        return Vec::new();
    }

    let mut selected = vec![primary];
    if let Some(secondary) = buckets.get(1).copied()
        && secondary.1 > f64::EPSILON
        && secondary.1 >= primary.1 * 0.9
    {
        selected.push(secondary);
    }

    selected
}

#[cfg(test)]
mod tests {
    use crate::models::{CrateMetrics, GitSignals, MetricPenalties, SemanticSignals};

    use super::{assign_archetypes, assign_combined_archetypes};

    #[test]
    fn archetype_assignment_god_file() {
        let archetypes = assign_archetypes(
            &CrateMetrics::default(),
            &MetricPenalties {
                max_file_loc: 30.0,
                trait_method_max: 22.0,
                ..MetricPenalties::default()
            },
        );

        assert_eq!(archetypes[0].as_str(), "God File");
    }

    #[test]
    fn archetype_assignment_brittle_hub() {
        let archetypes = assign_archetypes(
            &CrateMetrics::default(),
            &MetricPenalties {
                internal_dep_count: 42.0,
                max_file_loc: 20.0,
                ..MetricPenalties::default()
            },
        );

        assert_eq!(archetypes[0].as_str(), "Brittle Hub");
    }

    #[test]
    fn semantic_archetypes_are_limited_to_two() {
        let archetypes = assign_combined_archetypes(
            &CrateMetrics::default(),
            &MetricPenalties {
                max_file_loc: 30.0,
                trait_method_max: 22.0,
                internal_dep_count: 29.0,
                ..MetricPenalties::default()
            },
            Some(&GitSignals {
                churn_30d: 0.05,
                ..GitSignals::default()
            }),
            Some(&SemanticSignals {
                max_centrality: 0.9,
                drift_density: 0.8,
                boundary_leakage: 0.7,
                ..SemanticSignals::default()
            }),
        );

        assert_eq!(archetypes.len(), 2);
    }
}
