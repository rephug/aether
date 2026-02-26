use aether_core::normalize_path;
use blake3::Hasher;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub trait DocumentUnit: Send + Sync {
    fn unit_id(&self) -> &str;
    fn display_name(&self) -> &str;
    fn content(&self) -> &str;
    fn unit_kind(&self) -> &str;
    fn source_path(&self) -> &str;
    fn byte_range(&self) -> (u64, u64);
    fn parent_id(&self) -> Option<&str>;
    fn domain(&self) -> &str;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenericUnit {
    pub unit_id: String,
    pub display_name: String,
    pub content: String,
    pub unit_kind: String,
    pub source_path: String,
    pub byte_range: (u64, u64),
    pub parent_id: Option<String>,
    pub domain: String,
    pub metadata_json: serde_json::Value,
}

impl GenericUnit {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        display_name: impl Into<String>,
        content: impl Into<String>,
        unit_kind: impl Into<String>,
        source_path: impl Into<String>,
        byte_range: (u64, u64),
        parent_id: Option<String>,
        domain: impl Into<String>,
    ) -> Self {
        let display_name = display_name.into();
        let content = content.into();
        let unit_kind = unit_kind.into();
        let source_path = normalize_path(source_path.into().as_str());
        let domain = domain.into();
        let collapsed_content = collapse_whitespace_runs(&content);
        let normalized_content_prefix = collapsed_content.chars().take(200).collect::<String>();
        let unit_id = stable_unit_id(
            domain.as_str(),
            source_path.as_str(),
            unit_kind.as_str(),
            normalized_content_prefix.as_str(),
        );

        Self {
            unit_id,
            display_name,
            content,
            unit_kind,
            source_path,
            byte_range,
            parent_id,
            domain,
            metadata_json: json!({}),
        }
    }
}

impl DocumentUnit for GenericUnit {
    fn unit_id(&self) -> &str {
        self.unit_id.as_str()
    }

    fn display_name(&self) -> &str {
        self.display_name.as_str()
    }

    fn content(&self) -> &str {
        self.content.as_str()
    }

    fn unit_kind(&self) -> &str {
        self.unit_kind.as_str()
    }

    fn source_path(&self) -> &str {
        self.source_path.as_str()
    }

    fn byte_range(&self) -> (u64, u64) {
        self.byte_range
    }

    fn parent_id(&self) -> Option<&str> {
        self.parent_id.as_deref()
    }

    fn domain(&self) -> &str {
        self.domain.as_str()
    }
}

fn stable_unit_id(domain: &str, source_path: &str, unit_kind: &str, normalized_content: &str) -> String {
    let mut hasher = Hasher::new();
    hasher.update(domain.as_bytes());
    hasher.update(b"\n");
    hasher.update(source_path.as_bytes());
    hasher.update(b"\n");
    hasher.update(unit_kind.as_bytes());
    hasher.update(b"\n");
    hasher.update(normalized_content.as_bytes());
    hasher.finalize().to_hex().to_string()
}

fn collapse_whitespace_runs(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut pending_space = false;
    for ch in input.chars() {
        if ch.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && !output.is_empty() {
            output.push(' ');
        }
        pending_space = false;
        output.push(ch);
    }
    output.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_unit(content: &str) -> GenericUnit {
        GenericUnit::new(
            "Demo Unit",
            content,
            "paragraph",
            "docs\\guide.md",
            (0, 42),
            None,
            "docs",
        )
    }

    #[test]
    fn generic_unit_new_generates_deterministic_ids() {
        let first = make_unit("same content");
        let second = make_unit("same content");
        assert_eq!(first.unit_id, second.unit_id);
    }

    #[test]
    fn generic_unit_new_changes_id_when_hash_inputs_change() {
        let base = make_unit("same content");
        let different_content = make_unit("different content");
        assert_ne!(base.unit_id, different_content.unit_id);

        let different_kind = GenericUnit::new(
            "Demo Unit",
            "same content",
            "heading",
            "docs/guide.md",
            (0, 42),
            None,
            "docs",
        );
        assert_ne!(base.unit_id, different_kind.unit_id);

        let different_domain = GenericUnit::new(
            "Demo Unit",
            "same content",
            "paragraph",
            "docs/guide.md",
            (0, 42),
            None,
            "wiki",
        );
        assert_ne!(base.unit_id, different_domain.unit_id);
    }

    #[test]
    fn generic_unit_new_normalizes_path_and_collapses_whitespace_for_id() {
        let unix_style = GenericUnit::new(
            "Demo Unit",
            "hello   world\nfrom\tAETHER",
            "paragraph",
            "docs/guide.md",
            (0, 1),
            None,
            "docs",
        );
        let windows_style = GenericUnit::new(
            "Demo Unit",
            "hello world from AETHER",
            "paragraph",
            "docs\\guide.md",
            (0, 1),
            None,
            "docs",
        );

        assert_eq!(unix_style.source_path, "docs/guide.md");
        assert_eq!(unix_style.unit_id, windows_style.unit_id);
    }

    #[test]
    fn generic_unit_new_defaults_metadata_json_to_empty_object() {
        let unit = make_unit("metadata");
        assert_eq!(unit.metadata_json, json!({}));
    }
}
