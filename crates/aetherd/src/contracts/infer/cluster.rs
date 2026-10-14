//! Version-aware density clustering of claim embeddings.
//!
//! Points are claim instances (one claim text in one SIR version). Distance is
//! cosine distance between unit-normalized embeddings. A point is a core point
//! when its eps-neighborhood spans enough distinct versions, so one verbose
//! version cannot form a cluster on its own. Everything iterates `Vec`s and
//! `BTree*` collections in a fixed order, so results are reproducible.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Scores closer than this are treated as ties when picking a medoid.
const MEDOID_TIE_TOLERANCE: f64 = 1e-12;
/// The anisotropy guard needs at least this many within-version pairs.
pub(crate) const ANISOTROPY_MIN_PAIRS: usize = 10;
/// The anisotropy guard trips when more than this fraction of
/// within-version pairs fall inside eps.
pub(crate) const ANISOTROPY_MAX_CLOSE_FRACTION: f64 = 0.5;

/// One claim text observed in one considered SIR version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Instance {
    /// Position of the version among the considered versions, oldest first.
    pub version_idx: usize,
    /// Index into the run's sorted unique-text list.
    pub text_idx: usize,
}

/// Borrowed view over one run's claims and their embeddings.
pub(crate) struct ClaimSpace<'a> {
    /// Sorted unique texts that were embedded.
    pub texts: &'a [String],
    /// Unit vector per text, `None` for zero-norm vectors.
    pub units: &'a [Option<Vec<f64>>],
    /// Claim instances ordered by version, field order and sentence index.
    /// Every instance's text has a unit vector.
    pub instances: &'a [Instance],
    /// Number of considered versions (the persistence denominator).
    pub considered: usize,
}

impl ClaimSpace<'_> {
    fn unit(&self, text_idx: usize) -> &[f64] {
        self.units[text_idx].as_deref().unwrap_or_default()
    }

    fn instance_unit(&self, instance_idx: usize) -> &[f64] {
        self.unit(self.instances[instance_idx].text_idx)
    }

    fn versions_of(&self, instance_indices: &[usize]) -> BTreeSet<usize> {
        instance_indices
            .iter()
            .map(|&idx| self.instances[idx].version_idx)
            .collect()
    }
}

/// Normalize a raw embedding to unit length in f64; `None` when its norm is
/// effectively zero.
pub(crate) fn unit_vector(raw: &[f32]) -> Option<Vec<f64>> {
    let values: Vec<f64> = raw.iter().map(|&value| f64::from(value)).collect();
    let norm = values.iter().map(|value| value * value).sum::<f64>().sqrt();
    if !norm.is_finite() || norm <= f64::EPSILON {
        return None;
    }
    Some(values.into_iter().map(|value| value / norm).collect())
}

pub(crate) fn dot(left: &[f64], right: &[f64]) -> f64 {
    left.iter().zip(right).map(|(a, b)| a * b).sum()
}

/// Cosine distance between two unit vectors.
pub(crate) fn distance(left: &[f64], right: &[f64]) -> f64 {
    (1.0 - dot(left, right)).clamp(0.0, 2.0)
}

/// Number of distinct versions a cluster must cover to count as an invariant.
pub(crate) fn required_versions(threshold: f64, considered: usize) -> usize {
    let raw = (threshold * considered as f64 - 1e-9).ceil();
    let required = if raw.is_finite() && raw > 0.0 {
        raw as usize
    } else {
        0
    };
    required.max(2)
}

/// Distinct versions a point's neighborhood must span for it to be a core
/// point. Half of `required`, so paraphrase sub-clouds from different models
/// can still join; chains are rejected later by the medoid-support check.
pub(crate) fn core_min_versions(required: usize) -> usize {
    required.div_ceil(2).max(2)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AnisotropyCheck {
    /// Distinct unordered text pairs that co-occur in some version.
    pub pairs: usize,
    /// How many of those pairs are within eps.
    pub close: usize,
}

impl AnisotropyCheck {
    pub(crate) fn fraction(&self) -> f64 {
        if self.pairs == 0 {
            0.0
        } else {
            self.close as f64 / self.pairs as f64
        }
    }

    /// True when the embedding space is too compressed for eps: distinct
    /// claims from the same version mostly look like paraphrases.
    pub(crate) fn is_too_compressed(&self) -> bool {
        self.pairs >= ANISOTROPY_MIN_PAIRS && self.fraction() > ANISOTROPY_MAX_CLOSE_FRACTION
    }
}

/// Measure how many distinct claims stated together in one version are
/// within eps of each other.
pub(crate) fn anisotropy_check(space: &ClaimSpace<'_>, eps: f64) -> AnisotropyCheck {
    let mut texts_by_version: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for instance in space.instances {
        texts_by_version
            .entry(instance.version_idx)
            .or_default()
            .insert(instance.text_idx);
    }
    let mut pairs: BTreeSet<(usize, usize)> = BTreeSet::new();
    for texts in texts_by_version.values() {
        let texts: Vec<usize> = texts.iter().copied().collect();
        for (position, &left) in texts.iter().enumerate() {
            for &right in &texts[position + 1..] {
                pairs.insert((left, right));
            }
        }
    }
    let close = pairs
        .iter()
        .filter(|&&(left, right)| distance(space.unit(left), space.unit(right)) <= eps)
        .count();
    AnisotropyCheck {
        pairs: pairs.len(),
        close,
    }
}

/// Eps-neighborhoods per claim text. Every instance of a text shares the
/// same neighborhood, so it is computed once per unique text.
pub(crate) struct Neighborhoods {
    by_text: BTreeMap<usize, Vec<usize>>,
}

impl Neighborhoods {
    pub(crate) fn build(space: &ClaimSpace<'_>, eps: f64) -> Self {
        let mut instances_by_text: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (idx, instance) in space.instances.iter().enumerate() {
            instances_by_text
                .entry(instance.text_idx)
                .or_default()
                .push(idx);
        }
        let claim_texts: Vec<usize> = instances_by_text.keys().copied().collect();

        let mut close: BTreeMap<usize, Vec<usize>> =
            claim_texts.iter().map(|&text| (text, vec![text])).collect();
        for (position, &left) in claim_texts.iter().enumerate() {
            for &right in &claim_texts[position + 1..] {
                if distance(space.unit(left), space.unit(right)) <= eps {
                    close.entry(left).or_default().push(right);
                    close.entry(right).or_default().push(left);
                }
            }
        }

        let by_text = close
            .into_iter()
            .map(|(text, near_texts)| {
                let mut neighbors: Vec<usize> = near_texts
                    .iter()
                    .flat_map(|near| instances_by_text.get(near).into_iter().flatten())
                    .copied()
                    .collect();
                neighbors.sort_unstable();
                neighbors.dedup();
                (text, neighbors)
            })
            .collect();
        Self { by_text }
    }

    /// Instances within eps of `text_idx`, ascending, including the text's
    /// own instances.
    pub(crate) fn of_text(&self, text_idx: usize) -> &[usize] {
        self.by_text
            .get(&text_idx)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

/// Generalized DBSCAN where a point is core when its neighborhood spans at
/// least `core_min_versions` distinct versions. Returns one label per
/// instance; `None` is noise. Cluster ids follow discovery order.
pub(crate) fn dbscan(
    space: &ClaimSpace<'_>,
    neighborhoods: &Neighborhoods,
    core_min_versions: usize,
) -> Vec<Option<usize>> {
    let mut core_by_text: BTreeMap<usize, bool> = BTreeMap::new();
    let is_core: Vec<bool> = space
        .instances
        .iter()
        .map(|instance| {
            *core_by_text.entry(instance.text_idx).or_insert_with(|| {
                space
                    .versions_of(neighborhoods.of_text(instance.text_idx))
                    .len()
                    >= core_min_versions
            })
        })
        .collect();

    let count = space.instances.len();
    let mut labels: Vec<Option<usize>> = vec![None; count];
    let mut visited = vec![false; count];
    let mut next_cluster = 0;

    for start in 0..count {
        if visited[start] {
            continue;
        }
        visited[start] = true;
        if !is_core[start] {
            continue;
        }
        let cluster = next_cluster;
        next_cluster += 1;
        labels[start] = Some(cluster);
        let mut queue: VecDeque<usize> = neighborhoods
            .of_text(space.instances[start].text_idx)
            .iter()
            .copied()
            .collect();
        while let Some(point) = queue.pop_front() {
            if labels[point].is_none() {
                labels[point] = Some(cluster);
            }
            if visited[point] {
                continue;
            }
            visited[point] = true;
            if is_core[point] {
                queue.extend(
                    neighborhoods
                        .of_text(space.instances[point].text_idx)
                        .iter()
                        .copied(),
                );
            }
        }
    }
    labels
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ClusterSummary {
    /// Member instance indices, ascending.
    pub members: Vec<usize>,
    /// Distinct versions among members.
    pub coverage: usize,
    /// Instance index of the medoid.
    pub medoid: usize,
    /// Distinct versions in the medoid's full eps-neighborhood.
    pub support_versions: BTreeSet<usize>,
    /// Mean similarity of members to the medoid.
    pub cohesion: f64,
    /// Smallest eps at which the medoid's neighborhood would reach the
    /// required number of versions, if any.
    pub eps_needed: Option<f64>,
}

/// Summarize every cluster in label order.
pub(crate) fn summarize_clusters(
    space: &ClaimSpace<'_>,
    neighborhoods: &Neighborhoods,
    labels: &[Option<usize>],
    required: usize,
) -> Vec<ClusterSummary> {
    let mut members_by_cluster: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (idx, label) in labels.iter().enumerate() {
        if let Some(cluster) = label {
            members_by_cluster.entry(*cluster).or_default().push(idx);
        }
    }

    let mut instances_by_version: Vec<Vec<usize>> = vec![Vec::new(); space.considered];
    for (idx, instance) in space.instances.iter().enumerate() {
        if let Some(bucket) = instances_by_version.get_mut(instance.version_idx) {
            bucket.push(idx);
        }
    }

    members_by_cluster
        .into_values()
        .map(|members| {
            let medoid = select_medoid(space, &members);
            let medoid_unit = space.instance_unit(medoid);
            let cohesion = members
                .iter()
                .map(|&member| dot(space.instance_unit(member), medoid_unit))
                .sum::<f64>()
                / members.len() as f64;
            ClusterSummary {
                coverage: space.versions_of(&members).len(),
                support_versions: space
                    .versions_of(neighborhoods.of_text(space.instances[medoid].text_idx)),
                cohesion,
                eps_needed: eps_needed(space, &instances_by_version, medoid_unit, required),
                medoid,
                members,
            }
        })
        .collect()
}

/// Pick the member closest to the version-balanced centroid. Ties go to the
/// newer version, then the shorter text, then the lexicographically smaller
/// text.
fn select_medoid(space: &ClaimSpace<'_>, members: &[usize]) -> usize {
    let dim = space.instance_unit(members[0]).len();
    let mut per_version: BTreeMap<usize, (Vec<f64>, usize)> = BTreeMap::new();
    for &member in members {
        let entry = per_version
            .entry(space.instances[member].version_idx)
            .or_insert_with(|| (vec![0.0; dim], 0));
        for (sum, value) in entry.0.iter_mut().zip(space.instance_unit(member)) {
            *sum += value;
        }
        entry.1 += 1;
    }
    let mut centroid = vec![0.0; dim];
    for (sum, count) in per_version.values() {
        for (target, value) in centroid.iter_mut().zip(sum) {
            *target += value / *count as f64;
        }
    }
    let norm = dot(&centroid, &centroid).sqrt();
    if !norm.is_finite() || norm <= f64::EPSILON {
        return members[0];
    }

    let mut best = members[0];
    let mut best_score = dot(space.instance_unit(best), &centroid);
    for &member in &members[1..] {
        let score = dot(space.instance_unit(member), &centroid);
        if medoid_beats(space, (member, score), (best, best_score)) {
            best = member;
            best_score = score;
        }
    }
    best
}

fn medoid_beats(
    space: &ClaimSpace<'_>,
    (candidate, candidate_score): (usize, f64),
    (best, best_score): (usize, f64),
) -> bool {
    if (candidate_score - best_score).abs() > MEDOID_TIE_TOLERANCE {
        return candidate_score > best_score;
    }
    let candidate_version = space.instances[candidate].version_idx;
    let best_version = space.instances[best].version_idx;
    if candidate_version != best_version {
        return candidate_version > best_version;
    }
    let candidate_text = &space.texts[space.instances[candidate].text_idx];
    let best_text = &space.texts[space.instances[best].text_idx];
    let (candidate_len, best_len) = (candidate_text.chars().count(), best_text.chars().count());
    if candidate_len != best_len {
        return candidate_len < best_len;
    }
    candidate_text < best_text
}

/// For each considered version take the closest claim to the medoid, then
/// return the `required`-th smallest of those distances.
fn eps_needed(
    space: &ClaimSpace<'_>,
    instances_by_version: &[Vec<usize>],
    medoid_unit: &[f64],
    required: usize,
) -> Option<f64> {
    let mut closest: Vec<f64> = instances_by_version
        .iter()
        .map(|bucket| {
            bucket
                .iter()
                .map(|&idx| distance(space.instance_unit(idx), medoid_unit))
                .min_by(f64::total_cmp)
                .unwrap_or(f64::INFINITY)
        })
        .collect();
    closest.sort_by(f64::total_cmp);
    closest
        .get(required.checked_sub(1)?)
        .copied()
        .filter(|value| value.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture that owns the texts, units and instances a `ClaimSpace`
    /// borrows.
    struct Fixture {
        texts: Vec<String>,
        units: Vec<Option<Vec<f64>>>,
        instances: Vec<Instance>,
        considered: usize,
    }

    impl Fixture {
        /// `claims` lists, per version, (text, vector) pairs. Texts are
        /// interned in first-seen order.
        fn new(claims: &[Vec<(&str, Vec<f32>)>]) -> Self {
            let mut texts: Vec<String> = Vec::new();
            let mut units = Vec::new();
            let mut instances = Vec::new();
            for (version_idx, version) in claims.iter().enumerate() {
                for (text, vector) in version {
                    let text_idx = match texts.iter().position(|known| known == text) {
                        Some(idx) => idx,
                        None => {
                            texts.push((*text).to_owned());
                            units.push(unit_vector(vector));
                            texts.len() - 1
                        }
                    };
                    instances.push(Instance {
                        version_idx,
                        text_idx,
                    });
                }
            }
            Self {
                texts,
                units,
                instances,
                considered: claims.len(),
            }
        }

        fn space(&self) -> ClaimSpace<'_> {
            ClaimSpace {
                texts: &self.texts,
                units: &self.units,
                instances: &self.instances,
                considered: self.considered,
            }
        }

        fn cluster(&self, eps: f64, required: usize) -> (Vec<Option<usize>>, Vec<ClusterSummary>) {
            let space = self.space();
            let neighborhoods = Neighborhoods::build(&space, eps);
            let labels = dbscan(&space, &neighborhoods, core_min_versions(required));
            let summaries = summarize_clusters(&space, &neighborhoods, &labels, required);
            (labels, summaries)
        }

        fn medoid_text(&self, summary: &ClusterSummary) -> &str {
            &self.texts[self.instances[summary.medoid].text_idx]
        }
    }

    fn angle(degrees: f64) -> Vec<f32> {
        let radians = degrees.to_radians();
        vec![radians.cos() as f32, radians.sin() as f32]
    }

    fn axis(dim: usize, slot: usize) -> Vec<f32> {
        let mut vector = vec![0.0; dim];
        vector[slot] = 1.0;
        vector
    }

    /// A vector with cosine `similarity` to `axis(dim, 0)`, tilted into `slot`.
    fn near_first_axis(dim: usize, slot: usize, similarity: f32) -> Vec<f32> {
        let mut vector = vec![0.0; dim];
        vector[0] = similarity;
        vector[slot] = (1.0 - similarity * similarity).sqrt();
        vector
    }

    #[test]
    fn required_and_core_thresholds() {
        assert_eq!(required_versions(0.7, 10), 7);
        assert_eq!(required_versions(0.8, 10), 8);
        assert_eq!(required_versions(0.5, 3), 2);
        assert_eq!(required_versions(1.0, 10), 10);
        assert_eq!(required_versions(0.5, 1), 2);
        assert_eq!(core_min_versions(8), 4);
        assert_eq!(core_min_versions(7), 4);
        assert_eq!(core_min_versions(2), 2);
    }

    #[test]
    fn unit_vector_rejects_zero_norm() {
        assert!(unit_vector(&[0.0, 0.0]).is_none());
        let unit = unit_vector(&[3.0, 4.0]).unwrap();
        assert!((unit[0] - 0.6).abs() < 1e-12 && (unit[1] - 0.8).abs() < 1e-12);
    }

    #[test]
    fn separated_groups_form_two_clusters_and_noise_stays_unlabelled() {
        let dim = 32;
        let claims: Vec<Vec<(&str, Vec<f32>)>> = (0..10)
            .map(|version| {
                let first = if version % 2 == 0 {
                    ("group one phrasing a", near_first_axis(dim, 3, 1.0))
                } else {
                    ("group one phrasing b", near_first_axis(dim, 3, 0.99))
                };
                let second = if version < 5 {
                    ("group two phrasing a", axis(dim, 1))
                } else {
                    ("group two phrasing b", {
                        let mut v = axis(dim, 1);
                        v[4] = 0.1;
                        v
                    })
                };
                let noise = (NOISE[version], axis(dim, 10 + version));
                vec![first, second, noise]
            })
            .collect();
        let fixture = Fixture::new(&claims);
        let (labels, summaries) = fixture.cluster(0.12, 8);

        assert_eq!(summaries.len(), 2);
        for (idx, instance) in fixture.instances.iter().enumerate() {
            let text = &fixture.texts[instance.text_idx];
            if text.starts_with("noise") {
                assert_eq!(labels[idx], None, "{text} should be noise");
            } else {
                assert!(labels[idx].is_some(), "{text} should be clustered");
            }
        }
        assert!(summaries.iter().all(|summary| summary.coverage == 10));

        let (again, _) = fixture.cluster(0.12, 8);
        assert_eq!(labels, again);
    }

    const NOISE: [&str; 10] = [
        "noise zero",
        "noise one",
        "noise two",
        "noise three",
        "noise four",
        "noise five",
        "noise six",
        "noise seven",
        "noise eight",
        "noise nine",
    ];

    #[test]
    fn many_near_duplicates_from_one_version_do_not_form_a_core() {
        let dim = 8;
        let claims = vec![
            vec![
                ("verbose one", near_first_axis(dim, 1, 0.99)),
                ("verbose two", near_first_axis(dim, 2, 0.99)),
                ("verbose three", near_first_axis(dim, 3, 0.99)),
                ("verbose four", near_first_axis(dim, 4, 0.99)),
                ("verbose five", axis(dim, 0)),
            ],
            vec![("other version", axis(dim, 6))],
            vec![("third version", axis(dim, 7))],
        ];
        let fixture = Fixture::new(&claims);
        let (labels, summaries) = fixture.cluster(0.12, 2);
        assert!(summaries.is_empty());
        assert!(labels.iter().all(Option::is_none));
    }

    #[test]
    fn chained_cluster_fails_medoid_support() {
        let mut claims: Vec<Vec<(&str, Vec<f32>)>> = Vec::new();
        for version in 1..=10 {
            let mut claims_in_version = Vec::new();
            if version <= 5 {
                claims_in_version.push(("concept a", angle(0.0)));
            }
            if version == 5 {
                claims_in_version.push(("bridge one", angle(25.0)));
            }
            if version == 6 {
                claims_in_version.push(("bridge two", angle(50.0)));
            }
            if version >= 6 {
                claims_in_version.push(("concept b", angle(75.0)));
            }
            claims.push(claims_in_version);
        }
        let fixture = Fixture::new(&claims);
        let required = required_versions(0.8, 10);
        let (_, summaries) = fixture.cluster(0.12, required);

        assert_eq!(summaries.len(), 1);
        let summary = &summaries[0];
        assert_eq!(summary.coverage, 10);
        assert_eq!(summary.support_versions.len(), 6);
        assert!(summary.support_versions.len() < required);
        let needed = summary.eps_needed.expect("eps hint");
        assert!((needed - 0.357).abs() < 0.001, "eps_needed {needed}");
    }

    #[test]
    fn medoid_uses_version_balanced_centroid() {
        let mut claims: Vec<Vec<(&str, Vec<f32>)>> = vec![vec![
            ("verbose phrasing one", angle(9.0)),
            ("verbose phrasing two", angle(10.0)),
            ("verbose phrasing three", angle(10.5)),
            ("verbose phrasing four", angle(11.0)),
        ]];
        for _ in 2..=10 {
            claims.push(vec![("steady phrasing", angle(0.0))]);
        }
        let fixture = Fixture::new(&claims);
        let (_, summaries) = fixture.cluster(0.12, 8);
        assert_eq!(summaries.len(), 1);
        assert_eq!(fixture.medoid_text(&summaries[0]), "steady phrasing");
        assert_eq!(fixture.instances[summaries[0].medoid].version_idx, 9);
    }

    #[test]
    fn medoid_tie_prefers_newer_version() {
        let mut claims: Vec<Vec<(&str, Vec<f32>)>> = Vec::new();
        for version in 0..10 {
            if version < 5 {
                claims.push(vec![("older model phrasing", angle(5.0))]);
            } else {
                claims.push(vec![("newer model phrasing", angle(-5.0))]);
            }
        }
        let fixture = Fixture::new(&claims);
        let (_, summaries) = fixture.cluster(0.12, 8);
        assert_eq!(summaries.len(), 1);
        assert_eq!(fixture.medoid_text(&summaries[0]), "newer model phrasing");
        assert_eq!(fixture.instances[summaries[0].medoid].version_idx, 9);
        assert!(summaries[0].cohesion > 0.98);
    }

    #[test]
    fn anisotropy_counts_deduped_within_version_pairs() {
        let dim = 16;
        // Two versions share the same three texts: 3 distinct pairs only.
        let shared = vec![
            ("alpha claim", near_first_axis(dim, 1, 0.99)),
            ("beta claim", near_first_axis(dim, 2, 0.99)),
            ("gamma claim", near_first_axis(dim, 3, 0.99)),
        ];
        let fixture = Fixture::new(&[shared.clone(), shared]);
        let check = anisotropy_check(&fixture.space(), 0.12);
        assert_eq!(check.pairs, 3);
        assert_eq!(check.close, 3);
        assert!(!check.is_too_compressed(), "fewer than 10 pairs");

        // Five compressed texts in one version: 10 pairs, all close.
        let compressed: Vec<(&str, Vec<f32>)> = ["c1", "c2", "c3", "c4", "c5"]
            .into_iter()
            .enumerate()
            .map(|(slot, text)| (text, near_first_axis(dim, slot + 1, 0.99)))
            .collect();
        let fixture = Fixture::new(&[compressed]);
        let check = anisotropy_check(&fixture.space(), 0.12);
        assert_eq!(check.pairs, 10);
        assert!(check.is_too_compressed());

        // Five orthogonal texts: 10 pairs, none close.
        let spread: Vec<(&str, Vec<f32>)> = ["s1", "s2", "s3", "s4", "s5"]
            .into_iter()
            .enumerate()
            .map(|(slot, text)| (text, axis(dim, slot)))
            .collect();
        let fixture = Fixture::new(&[spread]);
        let check = anisotropy_check(&fixture.space(), 0.12);
        assert_eq!(check.pairs, 10);
        assert_eq!(check.close, 0);
        assert!(!check.is_too_compressed());
    }
}
