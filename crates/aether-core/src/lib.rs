use std::collections::HashMap;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub type SymbolId = String;
pub const NO_SIR_MESSAGE: &str =
    "AETHER: No SIR yet for this symbol. Run aetherd indexing and try again.";

pub const SIR_STATUS_STALE: &str = "stale";
pub const SEARCH_FALLBACK_LOCAL_STORE_NOT_INITIALIZED: &str = "local store not initialized";
pub const SEARCH_FALLBACK_EMBEDDINGS_DISABLED: &str =
    "embeddings are disabled in .aether/config.toml";
pub const SEARCH_FALLBACK_EMBEDDING_EMPTY_QUERY_VECTOR: &str =
    "embedding provider returned an empty query vector";
pub const SEARCH_FALLBACK_SEMANTIC_INDEX_NOT_READY: &str =
    "semantic index not ready for this embedding provider/model";

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    Ord,
    PartialOrd,
    Default,
    JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    #[default]
    Lexical,
    Semantic,
    Hybrid,
}

impl SearchMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
            Self::Semantic => "semantic",
            Self::Hybrid => "hybrid",
        }
    }
}

impl FromStr for SearchMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "lexical" => Ok(Self::Lexical),
            "semantic" => Ok(Self::Semantic),
            "hybrid" => Ok(Self::Hybrid),
            other => Err(format!(
                "invalid search mode '{other}', expected one of: lexical, semantic, hybrid"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchEnvelope<T> {
    pub mode_requested: SearchMode,
    pub mode_used: SearchMode,
    pub fallback_reason: Option<String>,
    pub matches: Vec<T>,
}

impl<T> SearchEnvelope<T> {
    pub fn result_count(&self) -> u32 {
        self.matches.len() as u32
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HoverMarkdownSections {
    pub symbol: String,
    pub intent: String,
    pub confidence: f32,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub side_effects: Vec<String>,
    pub dependencies: Vec<String>,
    pub error_modes: Vec<String>,
}

pub fn stale_warning_message(sir_status: Option<&str>, last_error: Option<&str>) -> Option<String> {
    let status = sir_status?.trim();
    if !status.eq_ignore_ascii_case(SIR_STATUS_STALE) {
        return None;
    }

    let last_error = last_error
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!(" Last error: {value}"))
        .unwrap_or_default();

    Some(format!("AETHER WARNING: SIR is stale.{last_error}"))
}

pub fn format_hover_markdown_sections(
    sections: &HoverMarkdownSections,
    stale_warning: Option<&str>,
) -> String {
    let mut blocks = Vec::with_capacity(8);
    blocks.push(format!("### {}", sections.symbol));
    blocks.push(format!("**Confidence:** {:.2}", sections.confidence));

    if let Some(warning) = stale_warning {
        blocks.push(format!("> {warning}"));
    }

    blocks.push(format!("**Intent**\n{}", sections.intent));
    blocks.push(format!(
        "**Inputs**\n{}",
        format_hover_list(&sections.inputs)
    ));
    blocks.push(format!(
        "**Outputs**\n{}",
        format_hover_list(&sections.outputs)
    ));
    blocks.push(format!(
        "**Side Effects**\n{}",
        format_hover_list(&sections.side_effects)
    ));
    blocks.push(format!(
        "**Dependencies**\n{}",
        format_hover_list(&sections.dependencies)
    ));
    blocks.push(format!(
        "**Error Modes**\n{}",
        format_hover_list(&sections.error_modes)
    ));

    blocks.join("\n\n")
}

fn format_hover_list(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".to_owned()
    } else {
        items
            .iter()
            .map(|item| format!("- {}", item.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Jsx,
}

impl Language {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::JavaScript => "javascript",
            Self::Jsx => "jsx",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Trait,
    Interface,
    TypeAlias,
}

impl SymbolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Method => "method",
            Self::Class => "class",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Trait => "trait",
            Self::Interface => "interface",
            Self::TypeAlias => "type_alias",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRange {
    pub start: Position,
    pub end: Position,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    pub id: SymbolId,
    pub language: Language,
    pub file_path: String,
    pub kind: SymbolKind,
    pub name: String,
    pub qualified_name: String,
    pub signature_fingerprint: String,
    pub content_hash: String,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolChangeEvent {
    pub file_path: String,
    pub language: Language,
    pub added: Vec<Symbol>,
    pub removed: Vec<Symbol>,
    pub updated: Vec<Symbol>,
}

impl SymbolChangeEvent {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.updated.is_empty()
    }
}

pub fn normalize_for_fingerprint(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_whitespace()).collect()
}

pub fn signature_fingerprint(signature_text: &str) -> String {
    let normalized = normalize_for_fingerprint(signature_text);
    blake3_hex(normalized.as_bytes())
}

pub fn content_hash(content: &str) -> String {
    blake3_hex(content.as_bytes())
}

pub fn stable_symbol_id(
    language: Language,
    file_path: &str,
    kind: SymbolKind,
    qualified_name: &str,
    signature_fingerprint: &str,
) -> SymbolId {
    let material = format!(
        "{}\n{}\n{}\n{}\n{}",
        language.as_str(),
        normalize_path(file_path),
        kind.as_str(),
        qualified_name,
        signature_fingerprint,
    );
    blake3_hex(material.as_bytes())
}

pub fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
}

pub fn diff_symbols(
    file_path: &str,
    language: Language,
    previous: &[Symbol],
    current: &[Symbol],
) -> SymbolChangeEvent {
    let previous_by_id: HashMap<&str, &Symbol> =
        previous.iter().map(|s| (s.id.as_str(), s)).collect();
    let current_by_id: HashMap<&str, &Symbol> =
        current.iter().map(|s| (s.id.as_str(), s)).collect();

    let mut added: Vec<Symbol> = current_by_id
        .iter()
        .filter_map(|(id, symbol)| {
            if previous_by_id.contains_key(id) {
                None
            } else {
                Some((*symbol).clone())
            }
        })
        .collect();

    let mut removed: Vec<Symbol> = previous_by_id
        .iter()
        .filter_map(|(id, symbol)| {
            if current_by_id.contains_key(id) {
                None
            } else {
                Some((*symbol).clone())
            }
        })
        .collect();

    let mut updated: Vec<Symbol> = current_by_id
        .iter()
        .filter_map(|(id, symbol)| {
            previous_by_id
                .get(id)
                .filter(|old| old.content_hash != symbol.content_hash)
                .map(|_| (*symbol).clone())
        })
        .collect();

    added.sort_by(|a, b| a.id.cmp(&b.id));
    removed.sort_by(|a, b| a.id.cmp(&b.id));
    updated.sort_by(|a, b| a.id.cmp(&b.id));

    SymbolChangeEvent {
        file_path: normalize_path(file_path),
        language,
        added,
        removed,
        updated,
    }
}

fn blake3_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_symbol(id: &str, name: &str, content_hash: &str) -> Symbol {
        Symbol {
            id: id.to_owned(),
            language: Language::Rust,
            file_path: "src/lib.rs".to_owned(),
            kind: SymbolKind::Function,
            name: name.to_owned(),
            qualified_name: name.to_owned(),
            signature_fingerprint: "sig".to_owned(),
            content_hash: content_hash.to_owned(),
            range: SourceRange {
                start: Position { line: 1, column: 1 },
                end: Position {
                    line: 1,
                    column: 10,
                },
            },
        }
    }

    #[test]
    fn stable_symbol_id_is_whitespace_insensitive_for_signature() {
        let sig_a = signature_fingerprint("fn add(x: i32, y: i32)");
        let sig_b = signature_fingerprint("fn  add( x: i32,  y: i32 )");
        assert_eq!(sig_a, sig_b);

        let id_a = stable_symbol_id(
            Language::Rust,
            "src/lib.rs",
            SymbolKind::Function,
            "add",
            &sig_a,
        );
        let id_b = stable_symbol_id(
            Language::Rust,
            "src/lib.rs",
            SymbolKind::Function,
            "add",
            &sig_b,
        );
        assert_eq!(id_a, id_b);
    }

    #[test]
    fn diff_symbols_tracks_added_removed_and_updated() {
        let previous = vec![
            sample_symbol("same", "same", "content-a"),
            sample_symbol("gone", "gone", "content-b"),
        ];

        let current = vec![
            sample_symbol("same", "same", "content-c"),
            sample_symbol("new", "new", "content-d"),
        ];

        let diff = diff_symbols("src/lib.rs", Language::Rust, &previous, &current);
        assert_eq!(
            diff.added.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["new"]
        );
        assert_eq!(
            diff.removed
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            vec!["gone"]
        );
        assert_eq!(
            diff.updated
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            vec!["same"]
        );
    }

    #[test]
    fn search_mode_parses_and_formats_consistently() {
        assert_eq!(SearchMode::default(), SearchMode::Lexical);
        assert_eq!("semantic".parse::<SearchMode>(), Ok(SearchMode::Semantic));
        assert_eq!(SearchMode::Hybrid.as_str(), "hybrid");
    }

    #[test]
    fn stale_warning_uses_error_when_available() {
        let warning = stale_warning_message(Some("stale"), Some(" provider timeout "));
        assert_eq!(
            warning.as_deref(),
            Some("AETHER WARNING: SIR is stale. Last error: provider timeout")
        );
    }

    #[test]
    fn stale_warning_ignores_non_stale_status() {
        let warning = stale_warning_message(Some("fresh"), Some("timeout"));
        assert_eq!(warning, None);
    }

    #[test]
    fn hover_markdown_is_sectioned() {
        let markdown = format_hover_markdown_sections(
            &HoverMarkdownSections {
                symbol: "demo::alpha".to_owned(),
                intent: "Summarize alpha behavior".to_owned(),
                confidence: 0.75,
                inputs: vec!["x".to_owned()],
                outputs: vec!["y".to_owned()],
                side_effects: vec![],
                dependencies: vec!["serde".to_owned()],
                error_modes: vec![],
            },
            Some("AETHER WARNING: SIR is stale."),
        );

        assert!(markdown.contains("### demo::alpha"));
        assert!(markdown.contains("**Confidence:** 0.75"));
        assert!(markdown.contains("> AETHER WARNING: SIR is stale."));
        assert!(markdown.contains("**Intent**"));
        assert!(markdown.contains("**Side Effects**\n(none)"));
    }
}
