use blake3::Hasher;

pub fn compute_source_hash_segment(source: &str) -> String {
    let mut h = Hasher::new();
    h.update(source.as_bytes());
    let hex = h.finalize().to_hex().to_string();
    hex[..16].to_owned()
}

/// Hashes the sorted neighbor intents. The original encoding ends each intent with a
/// newline, which is ambiguous once an intent itself contains one (`["a\nb"]` and
/// `["a", "b"]` hash alike), so such lists use a separate, length-prefixed encoding
/// under its own BLAKE3 key. Lists without a newline in any intent keep the original
/// bytes, so their stored prompt hashes still match.
pub fn compute_neighbor_hash_segment(neighbor_intents: &[&str]) -> String {
    let mut sorted = neighbor_intents.to_vec();
    sorted.sort();
    let h = if sorted.iter().any(|intent| intent.contains('\n')) {
        let mut h = Hasher::new_derive_key("aether prompt hash neighbor intents v2");
        for intent in &sorted {
            h.update(&(intent.len() as u64).to_le_bytes());
            h.update(intent.as_bytes());
        }
        h
    } else {
        let mut h = Hasher::new();
        for intent in &sorted {
            h.update(intent.as_bytes());
            h.update(b"\n");
        }
        h
    };
    let hex = h.finalize().to_hex().to_string();
    hex[..16].to_owned()
}

pub fn compute_config_hash_segment(config: &str) -> String {
    let mut h = Hasher::new();
    h.update(config.as_bytes());
    let hex = h.finalize().to_hex().to_string();
    hex[..16].to_owned()
}

pub fn compute_prompt_hash(source: &str, neighbor_intents: &[&str], config: &str) -> String {
    let source_hash = compute_source_hash_segment(source);
    let neighbor_hash = compute_neighbor_hash_segment(neighbor_intents);
    let config_hash = compute_config_hash_segment(config);

    format!("{source_hash}|{neighbor_hash}|{config_hash}")
}

/// Versioned cache identity for the full source identity, complete dynamic prompt,
/// and effective provider request template (including system/schema/model/settings).
/// Keep delimiters inside the hash distinct from the batch key's `|` separator.
pub fn compute_effective_prompt_hash(
    source_identity: &str,
    user_prompt: &str,
    request_template: &str,
) -> String {
    format!(
        "v3:{}:{}:{}",
        compute_source_hash_segment(source_identity),
        compute_config_hash_segment(user_prompt),
        compute_config_hash_segment(request_template),
    )
}

pub fn decompose_prompt_hash(hash: &str) -> (Option<&str>, Option<&str>, Option<&str>) {
    let mut parts = if let Some(hash) = hash.strip_prefix("v3:") {
        hash.split(':')
    } else {
        hash.split('|')
    };
    (parts.next(), parts.next(), parts.next())
}

pub fn diff_prompt_hashes(old: &str, new: &str) -> (bool, bool, bool) {
    let (old_source, old_neighbors, old_config) = decompose_prompt_hash(old);
    let (new_source, new_neighbors, new_config) = decompose_prompt_hash(new);
    (
        old_source != new_source,
        old_neighbors != new_neighbors,
        old_config != new_config,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        compute_config_hash_segment, compute_neighbor_hash_segment, compute_prompt_hash,
        compute_source_hash_segment, diff_prompt_hashes,
    };
    use super::{compute_effective_prompt_hash, decompose_prompt_hash};

    #[test]
    fn effective_identity_covers_context_and_request_and_is_a_single_key_token() {
        let base = compute_effective_prompt_hash(
            "full-source-hash",
            "complete user context",
            "effective settings and system/schema",
        );
        assert!(!base.contains('|'));
        assert!(base.starts_with("v3:"));
        assert_eq!(
            decompose_prompt_hash(&base).0,
            Some(compute_source_hash_segment("full-source-hash").as_str())
        );
        assert_ne!(
            base,
            compute_effective_prompt_hash(
                "changed-source-hash",
                "complete user context",
                "effective settings and system/schema"
            )
        );
        assert_ne!(
            base,
            compute_effective_prompt_hash(
                "full-source-hash",
                "changed user context",
                "effective settings and system/schema"
            )
        );
        assert_ne!(
            base,
            compute_effective_prompt_hash(
                "full-source-hash",
                "complete user context",
                "changed settings or system/schema"
            )
        );
        assert_ne!(
            base,
            compute_prompt_hash(
                "full-source-hash",
                &["complete user context"],
                "effective settings and system/schema"
            )
        );
    }

    #[test]
    fn compute_prompt_hash_is_stable_for_identical_inputs() {
        let first =
            compute_prompt_hash("fn run() {}", &["intent a", "intent b"], "model:low:10000");
        let second =
            compute_prompt_hash("fn run() {}", &["intent b", "intent a"], "model:low:10000");
        assert_eq!(first, second);
    }

    #[test]
    fn compute_prompt_hash_changes_when_inputs_change() {
        let base = compute_prompt_hash("fn run() {}", &["intent a"], "model:low:10000");
        let changed_source =
            compute_prompt_hash("fn run(x: i32) {}", &["intent a"], "model:low:10000");
        let changed_neighbors =
            compute_prompt_hash("fn run() {}", &["intent b"], "model:low:10000");
        let changed_config = compute_prompt_hash("fn run() {}", &["intent a"], "model:high:10000");

        assert_ne!(base, changed_source);
        assert_ne!(base, changed_neighbors);
        assert_ne!(base, changed_config);
    }

    #[test]
    fn diff_prompt_hashes_reports_which_segment_changed() {
        let base = compute_prompt_hash("fn run() {}", &["intent a"], "model:low:10000");
        let changed_source =
            compute_prompt_hash("fn run(x: i32) {}", &["intent a"], "model:low:10000");
        let changed_neighbors =
            compute_prompt_hash("fn run() {}", &["intent b"], "model:low:10000");
        let changed_config = compute_prompt_hash("fn run() {}", &["intent a"], "model:high:10000");

        assert_eq!(
            diff_prompt_hashes(&base, &changed_source),
            (true, false, false)
        );
        assert_eq!(
            diff_prompt_hashes(&base, &changed_neighbors),
            (false, true, false)
        );
        assert_eq!(
            diff_prompt_hashes(&base, &changed_config),
            (false, false, true)
        );
    }

    #[test]
    fn prompt_hash_segments_match_composed_hash() {
        let source_hash = compute_source_hash_segment("fn run() {}");
        let neighbor_hash = compute_neighbor_hash_segment(&["intent b", "intent a"]);
        let config_hash = compute_config_hash_segment("model:low:10000");

        assert_eq!(
            compute_prompt_hash("fn run() {}", &["intent a", "intent b"], "model:low:10000"),
            format!("{source_hash}|{neighbor_hash}|{config_hash}")
        );
    }

    #[test]
    fn neighbor_segment_keeps_the_original_bytes_without_embedded_newlines() {
        // The original encoding, spelled out: stored hashes must keep matching.
        let mut legacy = blake3::Hasher::new();
        for intent in ["intent a", "intent b"] {
            legacy.update(intent.as_bytes());
            legacy.update(b"\n");
        }
        let legacy = legacy.finalize().to_hex().to_string()[..16].to_owned();
        assert_eq!(
            compute_neighbor_hash_segment(&["intent b", "intent a"]),
            legacy
        );
    }

    #[test]
    fn neighbor_segment_distinguishes_an_embedded_newline_from_two_intents() {
        assert_ne!(
            compute_neighbor_hash_segment(&["a\nb"]),
            compute_neighbor_hash_segment(&["a", "b"])
        );
        // Both lists encoded to "x\ny\nz\n" before.
        assert_ne!(
            compute_neighbor_hash_segment(&["x\ny", "z"]),
            compute_neighbor_hash_segment(&["x", "y\nz"])
        );
    }
}
