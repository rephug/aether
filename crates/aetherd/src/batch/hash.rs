use blake3::Hasher;

pub fn compute_prompt_hash(source: &str, neighbor_intents: &[&str], config: &str) -> String {
    let source_hash = {
        let mut h = Hasher::new();
        h.update(source.as_bytes());
        let hex = h.finalize().to_hex().to_string();
        hex[..16].to_owned()
    };

    let neighbor_hash = {
        let mut h = Hasher::new();
        let mut sorted = neighbor_intents.to_vec();
        sorted.sort();
        for intent in &sorted {
            h.update(intent.as_bytes());
            h.update(b"\n");
        }
        let hex = h.finalize().to_hex().to_string();
        hex[..16].to_owned()
    };

    let config_hash = {
        let mut h = Hasher::new();
        h.update(config.as_bytes());
        let hex = h.finalize().to_hex().to_string();
        hex[..16].to_owned()
    };

    format!("{source_hash}|{neighbor_hash}|{config_hash}")
}

pub fn decompose_prompt_hash(hash: &str) -> (Option<&str>, Option<&str>, Option<&str>) {
    let mut parts = hash.split('|');
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
    use super::{compute_prompt_hash, diff_prompt_hashes};

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
}
