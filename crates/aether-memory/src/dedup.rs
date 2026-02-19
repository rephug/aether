pub fn normalize_content_for_hash(content: &str) -> String {
    content
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

pub fn compute_content_hash(content: &str) -> String {
    let normalized = normalize_content_for_hash(content);
    blake3::hash(normalized.as_bytes()).to_hex().to_string()
}

pub fn compute_note_id(content: &str, created_at: i64) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(content.as_bytes());
    hasher.update(b":");
    hasher.update(created_at.max(0).to_string().as_bytes());
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::{compute_content_hash, compute_note_id, normalize_content_for_hash};

    #[test]
    fn normalizes_whitespace_and_case_for_hashing() {
        let left = normalize_content_for_hash("  Hello\n  World  ");
        let right = normalize_content_for_hash("hello world");
        assert_eq!(left, "hello world");
        assert_eq!(left, right);
        assert_eq!(
            compute_content_hash("Hello\nWorld"),
            compute_content_hash(" hello world ")
        );
    }

    #[test]
    fn note_id_changes_when_timestamp_changes() {
        let first = compute_note_id("hello", 1);
        let second = compute_note_id("hello", 2);
        assert_ne!(first, second);
    }
}
