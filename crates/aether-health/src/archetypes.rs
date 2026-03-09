use crate::models::{Archetype, MetricPenalties};

pub fn assign_archetypes(
    _metrics: &crate::models::CrateMetrics,
    penalties: &MetricPenalties,
) -> Vec<Archetype> {
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

    let mut archetypes = Vec::new();
    let primary = buckets[0];
    if primary.1 <= f64::EPSILON {
        return archetypes;
    }
    archetypes.push(primary.0);

    let secondary = buckets[1];
    if secondary.1 > f64::EPSILON && secondary.1 >= primary.1 * 0.9 {
        archetypes.push(secondary.0);
    }

    archetypes
}

#[cfg(test)]
mod tests {
    use crate::models::{CrateMetrics, MetricPenalties};

    use super::assign_archetypes;

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
}
