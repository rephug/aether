use std::collections::HashMap;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub mod git;
pub mod secret;
pub mod test_code;

pub use git::{BlameLine, CommitInfo, GitContext, GitHistoryError};
pub use secret::Secret;
pub use test_code::{is_test_file, is_test_symbol};

pub type SymbolId = String;
pub const NO_SIR_MESSAGE: &str = "AETHER: No saved explanation is available here yet. Start indexing and hover over a supported function or type, then try again.";
pub const AETHER_AGENT_SCHEMA_VERSION: u32 = 7;

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
        .map(|value| format!(" Last update failed: {value}"))
        .unwrap_or_default();

    Some(format!(
        "AETHER: This explanation may be out of date.{last_error}"
    ))
}

pub fn format_hover_markdown_sections(
    sections: &HoverMarkdownSections,
    stale_warning: Option<&str>,
) -> String {
    let mut blocks = Vec::with_capacity(8);
    blocks.push(format!("### {}", sections.symbol));
    let intent = sections.intent.trim();
    let summary_end = intent.char_indices().nth(280).map(|(end, _)| {
        intent[..end]
            .rfind(char::is_whitespace)
            .filter(|boundary| *boundary > end / 2)
            .unwrap_or(end)
    });
    let summary = summary_end.map(|end| format!("{}…", intent[..end].trim_end()));
    blocks.push(format!(
        "**What this does**\n{}",
        if intent.is_empty() {
            "Not described in this explanation."
        } else {
            summary.as_deref().unwrap_or(intent)
        }
    ));

    if let Some(warning) = stale_warning {
        blocks.push(format!("> {warning}"));
    }

    blocks.push(format!(
        "**Inputs**\n{}",
        format_hover_list(&sections.inputs)
    ));
    blocks.push(format!(
        "**Result**\n{}",
        format_hover_list(&sections.outputs)
    ));
    let cautions = sections
        .side_effects
        .iter()
        .filter(|item| !item.trim().is_empty())
        .map(|item| format!("Changes: {}", item.trim()))
        .chain(
            sections
                .error_modes
                .iter()
                .filter(|item| !item.trim().is_empty())
                .map(|item| format!("Possible failure: {}", item.trim())),
        )
        .collect::<Vec<_>>();
    blocks.push(format!("**Watch out for**\n{}", if cautions.is_empty() {
        "No cautions were recorded. This does not confirm that there are no side effects or errors.".to_owned()
    } else { format_hover_list(&cautions) }));
    blocks.push(format!(
        "---\n**Technical details**\n\n**Uses**\n{}\n\n**Reported confidence:** {:.2} (not a correctness check)",
        format_hover_list(&sections.dependencies), sections.confidence
    ));
    if summary.is_some() {
        blocks.push(format!("**Full explanation**\n{intent}"));
    }

    blocks.join("\n\n")
}

fn format_hover_list(items: &[String]) -> String {
    let items = items
        .iter()
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>();
    if items.is_empty() {
        "Not described in this explanation.".to_owned()
    } else {
        items
            .iter()
            .map(|item| format!("- {item}"))
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
    Python,
    Go,
    Java,
    // Spelled like `as_str`, which is what the store and symbol ids use.
    #[serde(rename = "csharp")]
    CSharp,
}

impl Language {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::JavaScript => "javascript",
            Self::Jsx => "jsx",
            Self::Python => "python",
            Self::Go => "go",
            Self::Java => "java",
            Self::CSharp => "csharp",
        }
    }
}

/// The stored language names (`Language::as_str`) of TypeScript, TSX, JavaScript and
/// JSX, which form one language family.
pub const ECMASCRIPT_LANGUAGES: [&str; 4] = ["typescript", "tsx", "javascript", "jsx"];

/// The family of a stored language name. Code can only call or name symbols of its own
/// family: TypeScript, TSX, JavaScript and JSX form one, and every other language is a
/// family of its own.
pub fn language_family(language: &str) -> &str {
    if ECMASCRIPT_LANGUAGES.contains(&language) {
        "ecmascript"
    } else {
        language
    }
}

/// Whether two stored language names belong to the same [`language_family`].
pub fn same_language_family(left: &str, right: &str) -> bool {
    language_family(left) == language_family(right)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Variable,
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
            Self::Variable => "variable",
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
    #[serde(default)]
    pub start_byte: Option<usize>,
    #[serde(default)]
    pub end_byte: Option<usize>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Calls,
    DependsOn,
    TypeRef,
    Implements,
}

impl EdgeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Calls => "calls",
            Self::DependsOn => "depends_on",
            Self::TypeRef => "type_ref",
            Self::Implements => "implements",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolEdge {
    pub source_id: String,
    pub target_qualified_name: String,
    /// Exact target established by the parser. Missing IDs never fall back to a name.
    #[serde(default)]
    pub target_symbol_id: Option<String>,
    /// Explicit import provenance: an exact normalized indexed file path.
    #[serde(default)]
    pub target_file_path: Option<String>,
    pub edge_kind: EdgeKind,
    pub file_path: String,
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

pub fn file_source_id(file_path: &str) -> String {
    format!("file::{}", normalize_path(file_path))
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

    #[test]
    fn typescript_and_javascript_share_a_language_family() {
        for left in ECMASCRIPT_LANGUAGES {
            for right in ECMASCRIPT_LANGUAGES {
                assert!(same_language_family(left, right), "{left} and {right}");
            }
        }
        for language in [
            Language::Rust,
            Language::Python,
            Language::Go,
            Language::Java,
            Language::CSharp,
        ] {
            let language = language.as_str();
            assert!(same_language_family(language, language));
            assert!(!same_language_family(language, "typescript"));
        }
        assert!(!same_language_family("java", "csharp"));
        assert!(!same_language_family("python", "go"));
    }

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
                start_byte: None,
                end_byte: None,
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
            Some(
                "AETHER: This explanation may be out of date. Last update failed: provider timeout"
            )
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
            Some("AETHER: This explanation may be out of date."),
        );

        assert!(markdown.contains("### demo::alpha"));
        assert!(markdown.contains("**Reported confidence:** 0.75"));
        assert!(markdown.contains("> AETHER: This explanation may be out of date."));
        assert!(markdown.contains("**What this does**"));
        assert!(markdown.contains("**Watch out for**\nNo cautions were recorded."));
        assert!(markdown.find("What this does") < markdown.find("Inputs"));
        assert!(markdown.find("**Result**") < markdown.find("Technical details"));
        assert!(markdown.contains("not a correctness check"));
        assert!(!markdown.contains("(none)"));
    }

    #[test]
    fn hover_preserves_meaningful_cautions_and_discloses_unrecorded_fields() {
        let mut sections = HoverMarkdownSections {
            symbol: "save".into(),
            intent: "Writes the supplied text to a file.".into(),
            confidence: 0.8,
            inputs: vec!["text: contents to save".into()],
            outputs: vec![" ".into()],
            side_effects: vec!["Replaces the file contents".into()],
            dependencies: vec![],
            error_modes: vec!["Returns an error if the file is not writable".into()],
        };
        let markdown = format_hover_markdown_sections(&sections, None);
        assert!(markdown.contains("Changes: Replaces the file contents"));
        assert!(
            markdown.contains("Possible failure: Returns an error if the file is not writable")
        );
        assert!(markdown.contains("**Result**\nNot described in this explanation."));
        sections.side_effects = vec![" ".into()];
        sections.error_modes.clear();
        assert!(
            format_hover_markdown_sections(&sections, None).contains("No cautions were recorded")
        );
        sections.intent = "Writes café names to the requested file. ".repeat(20);
        let markdown = format_hover_markdown_sections(&sections, None);
        assert!(markdown.contains("…\n\n**Inputs**"));
        assert!(markdown.contains(&format!("**Full explanation**\n{}", sections.intent.trim())));
    }

    #[test]
    fn secret_redacts_debug_and_display() {
        let secret = Secret::new("top-secret-token".to_owned());
        assert_eq!(format!("{secret:?}"), "[REDACTED]");
        assert_eq!(format!("{secret}"), "[REDACTED]");
        assert_eq!(secret.expose(), "top-secret-token");
    }
}
