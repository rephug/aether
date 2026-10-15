//! Claim extraction from SIR history rows.
//!
//! Everything here is pure and deterministic: the same history always yields
//! the same versions, claims and statistics.

use std::collections::{BTreeMap, BTreeSet};

use aether_config::{InferenceProviderKind, MOCK_INTENT_PREFIX};
use aether_store::SirHistoryRecord;
use serde_json::Value;

use super::InferSettings;

/// Maximum claims kept from one SIR version, in field priority order.
pub(crate) const MAX_CLAIMS_PER_VERSION: usize = 40;
/// Claims longer than this many characters are dropped.
const MAX_CLAIM_CHARS: usize = 500;
/// Claims with fewer whitespace-separated words are dropped.
const MIN_CLAIM_WORDS: usize = 3;

/// SIR fields that carry behavioral claims, in priority order. Structural
/// fields (inputs, outputs, dependencies) are deliberately excluded.
const CLAIM_FIELDS: [&str; 5] = [
    "intent",
    "behavior",
    "error_modes",
    "side_effects",
    "edge_cases",
];

/// Comparison keys of filler fragments that never become claims.
const PLACEHOLDER_KEYS: [&str; 9] = [
    "none",
    "n/a",
    "na",
    "nothing",
    "not applicable",
    "unknown",
    "none identified",
    "no side effects",
    "no errors",
];

/// Abbreviations whose trailing period never ends a sentence.
const NON_TERMINAL_ABBREVIATIONS: [&str; 5] = ["e.g", "i.e", "etc", "vs", "cf"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaimText {
    /// Display text, kept verbatim so a promoted contract is a real
    /// historical sentence.
    pub text: String,
    /// Normalized comparison key (see [`comparison_key`]).
    pub key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PreparedVersion {
    pub version: i64,
    pub provider: String,
    pub model: String,
    pub claims: Vec<ClaimText>,
}

impl PreparedVersion {
    fn key_set(&self) -> BTreeSet<&str> {
        self.claims.iter().map(|claim| claim.key.as_str()).collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HistoryStats {
    /// History rows read for the symbol.
    pub raw: usize,
    /// Rows written by the mock provider or carrying a mock intent.
    pub skipped_mock: usize,
    /// Rows that are not JSON or have no non-empty `intent`.
    pub skipped_unparseable: usize,
    /// Versions merged into a newer consecutive version with identical claims.
    pub collapsed: usize,
    /// Usable versions older than the history window.
    pub outside_window: usize,
    /// Versions that remain after all of the above.
    pub considered: usize,
    /// `provider/model` counts over the considered versions.
    pub models: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PreparedHistory {
    /// Considered versions, oldest first.
    pub versions: Vec<PreparedVersion>,
    pub stats: HistoryStats,
    /// True when the newest raw history row is a mock placeholder.
    pub newest_is_mock: bool,
}

enum RowClass {
    Mock,
    Unparseable,
    Usable(PreparedVersion),
}

fn classify_record(record: &SirHistoryRecord) -> RowClass {
    if record.provider == InferenceProviderKind::Mock.as_str() {
        return RowClass::Mock;
    }
    let Ok(value) = serde_json::from_str::<Value>(&record.sir_json) else {
        return RowClass::Unparseable;
    };
    let Some(intent) = value
        .get("intent")
        .and_then(Value::as_str)
        .filter(|intent| !intent.trim().is_empty())
    else {
        return RowClass::Unparseable;
    };
    if intent.trim_start().starts_with(MOCK_INTENT_PREFIX) {
        return RowClass::Mock;
    }
    RowClass::Usable(PreparedVersion {
        version: record.version,
        provider: record.provider.clone(),
        model: record.model.clone(),
        claims: extract_claims(&value),
    })
}

/// Filter, collapse and window a symbol's SIR history (ordered by version
/// ascending, as the store returns it).
pub(crate) fn prepare_history(
    records: &[SirHistoryRecord],
    settings: &InferSettings,
) -> PreparedHistory {
    let mut stats = HistoryStats {
        raw: records.len(),
        ..HistoryStats::default()
    };
    let mut newest_is_mock = false;
    let mut versions: Vec<PreparedVersion> = Vec::new();

    for record in records {
        newest_is_mock = false;
        match classify_record(record) {
            RowClass::Mock => {
                stats.skipped_mock += 1;
                newest_is_mock = true;
            }
            RowClass::Unparseable => stats.skipped_unparseable += 1,
            RowClass::Usable(version) => {
                let same_claims = versions
                    .last()
                    .is_some_and(|previous| previous.key_set() == version.key_set());
                if same_claims {
                    stats.collapsed += 1;
                    versions.pop();
                }
                versions.push(version);
            }
        }
    }

    if versions.len() > settings.window {
        stats.outside_window = versions.len() - settings.window;
        versions.drain(..stats.outside_window);
    }
    stats.considered = versions.len();
    for version in &versions {
        *stats
            .models
            .entry(format!("{}/{}", version.provider, version.model))
            .or_insert(0) += 1;
    }

    PreparedHistory {
        versions,
        stats,
        newest_is_mock,
    }
}

/// Extract discrete behavioral claims from one SIR JSON value.
///
/// Fields are read in [`CLAIM_FIELDS`] order; strings and arrays of strings
/// are accepted, anything else is ignored. Claims are deduplicated by
/// comparison key and capped at [`MAX_CLAIMS_PER_VERSION`].
pub(crate) fn extract_claims(value: &Value) -> Vec<ClaimText> {
    let mut claims = Vec::new();
    let mut seen = BTreeSet::new();

    for field in CLAIM_FIELDS {
        let texts: Vec<&str> = match value.get(field) {
            Some(Value::String(text)) => vec![text.as_str()],
            Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        for text in texts {
            for sentence in split_sentences(text) {
                if claims.len() >= MAX_CLAIMS_PER_VERSION {
                    return claims;
                }
                if !is_claim_candidate(&sentence) {
                    continue;
                }
                let key = comparison_key(&sentence);
                if PLACEHOLDER_KEYS.contains(&key.as_str()) || !seen.insert(key.clone()) {
                    continue;
                }
                claims.push(ClaimText {
                    text: sentence,
                    key,
                });
            }
        }
    }

    claims
}

fn is_claim_candidate(sentence: &str) -> bool {
    sentence.split_whitespace().count() >= MIN_CLAIM_WORDS
        && sentence.chars().count() <= MAX_CLAIM_CHARS
}

/// Normalized key used for every text equality check: lowercase, internal
/// whitespace collapsed, trailing punctuation trimmed.
pub(crate) fn comparison_key(text: &str) -> String {
    let collapsed = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    collapsed
        .trim_end_matches(['.', '!', '?', ';', ':', ','])
        .trim_end()
        .to_owned()
}

/// Split free text into sentence-level fragments.
///
/// Splits on newlines, semicolons, and on `.`/`!`/`?` when followed by
/// whitespace and a character that is not lowercase, unless the period ends
/// a known abbreviation. Leading bullets, numbering and a wrapping bold
/// marker are stripped. Whitespace is collapsed; other characters are kept.
pub(crate) fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    for line in text.split('\n') {
        let line = strip_emphasis(strip_list_marker(line.trim()));
        for clause in line.split(';') {
            for fragment in split_on_terminators(clause) {
                let cleaned = fragment.split_whitespace().collect::<Vec<_>>().join(" ");
                if !cleaned.is_empty() {
                    sentences.push(cleaned);
                }
            }
        }
    }
    sentences
}

fn strip_list_marker(line: &str) -> &str {
    for bullet in ["- ", "* ", "• "] {
        if let Some(rest) = line.strip_prefix(bullet) {
            return rest.trim_start();
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &line[digits..];
        for marker in [". ", ") "] {
            if let Some(rest) = rest.strip_prefix(marker) {
                return rest.trim_start();
            }
        }
    }
    line
}

fn strip_emphasis(line: &str) -> &str {
    for wrapper in ["**", "__"] {
        if line.len() > 2 * wrapper.len()
            && let Some(inner) = line
                .strip_prefix(wrapper)
                .and_then(|rest| rest.strip_suffix(wrapper))
        {
            return inner.trim();
        }
    }
    line
}

fn split_on_terminators(text: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut pieces = Vec::new();
    let mut start = 0;

    for (position, &(offset, ch)) in chars.iter().enumerate() {
        if !matches!(ch, '.' | '!' | '?') {
            continue;
        }
        let Some(&(_, following)) = chars.get(position + 1) else {
            continue;
        };
        if !following.is_whitespace() {
            continue;
        }
        let next_visible = chars[position + 1..]
            .iter()
            .map(|&(_, c)| c)
            .find(|c| !c.is_whitespace());
        if next_visible.is_some_and(char::is_lowercase) {
            continue;
        }
        if ch == '.' && ends_with_abbreviation(&text[start..offset]) {
            continue;
        }
        let end = offset + ch.len_utf8();
        pieces.push(&text[start..end]);
        start = end;
    }
    if start < text.len() {
        pieces.push(&text[start..]);
    }
    pieces
}

fn ends_with_abbreviation(before_period: &str) -> bool {
    let token = before_period
        .rsplit(char::is_whitespace)
        .next()
        .unwrap_or_default()
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase();
    NON_TERMINAL_ABBREVIATIONS.contains(&token.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(window: usize) -> InferSettings {
        InferSettings {
            min_versions: 3,
            window,
            threshold: 0.8,
            eps: 0.12,
            fail_threshold: 0.5,
            max_promoted: 5,
        }
    }

    fn record(version: i64, provider: &str, sir_json: &str) -> SirHistoryRecord {
        SirHistoryRecord {
            symbol_id: "sym".to_owned(),
            version,
            sir_hash: format!("hash-{version}"),
            provider: provider.to_owned(),
            model: "model-x".to_owned(),
            created_at: version,
            sir_json: sir_json.to_owned(),
            commit_hash: None,
        }
    }

    fn texts(claims: &[ClaimText]) -> Vec<&str> {
        claims.iter().map(|claim| claim.text.as_str()).collect()
    }

    #[test]
    fn split_keeps_abbreviations_decimals_and_paths_together() {
        for sentence in [
            "Uses e.g. a cache.",
            "Clamps to 0.5 before use.",
            "Calls std::fs::read and foo.bar() first.",
            "Retries vs. fails fast here.",
        ] {
            assert_eq!(split_sentences(sentence), vec![sentence.to_owned()]);
        }
        assert_eq!(
            split_sentences("Lists items etc. Then exits."),
            vec!["Lists items etc. Then exits.".to_owned()]
        );
    }

    #[test]
    fn split_breaks_on_semicolons_newlines_and_sentence_ends() {
        assert_eq!(
            split_sentences("Reads the file; parses the header"),
            vec!["Reads the file".to_owned(), "parses the header".to_owned()]
        );
        assert_eq!(
            split_sentences("First line here\nSecond line here"),
            vec!["First line here".to_owned(), "Second line here".to_owned()]
        );
        assert_eq!(
            split_sentences("Validates input. Next it writes output!  Done? Yes."),
            vec![
                "Validates input.".to_owned(),
                "Next it writes output!".to_owned(),
                "Done?".to_owned(),
                "Yes.".to_owned(),
            ]
        );
    }

    #[test]
    fn split_strips_bullets_numbering_and_bold_wrappers() {
        let raw = "- dash item text\n* star item text\n• dot item text\n1. first numbered item\n2) second numbered item\n**Bold wrapped claim.**";
        assert_eq!(
            split_sentences(raw),
            vec![
                "dash item text".to_owned(),
                "star item text".to_owned(),
                "dot item text".to_owned(),
                "first numbered item".to_owned(),
                "second numbered item".to_owned(),
                "Bold wrapped claim.".to_owned(),
            ]
        );
        assert_eq!(
            split_sentences("  Collapses   inner \t whitespace  "),
            vec!["Collapses inner whitespace".to_owned()]
        );
    }

    #[test]
    fn comparison_key_normalizes_case_space_and_trailing_punctuation() {
        assert_eq!(
            comparison_key("  Rejects   ZERO amounts.!; "),
            "rejects zero amounts"
        );
        assert_eq!(comparison_key("No side effects."), "no side effects");
    }

    #[test]
    fn extract_claims_drops_placeholders_short_and_long_fragments() {
        let long = format!("Long claim {}", "word ".repeat(120));
        let value = serde_json::json!({
            "intent": "Validates the payment amount.",
            "behavior": "None. Too short. N/A",
            "side_effects": ["No side effects.", "Not applicable", long],
            "error_modes": ["Returns an error for zero amounts."],
            "edge_cases": "nothing",
        });
        assert_eq!(
            texts(&extract_claims(&value)),
            vec![
                "Validates the payment amount.",
                "Returns an error for zero amounts.",
            ]
        );
    }

    #[test]
    fn extract_claims_ignores_structural_fields() {
        let value = serde_json::json!({
            "intent": "Computes the order total.",
            "inputs": ["order: Order with many line items"],
            "outputs": ["Total price in minor units"],
            "dependencies": ["pricing::tax_rate is called here"],
            "confidence": 0.9,
            "complexity": "Linear in the number of items",
        });
        assert_eq!(
            texts(&extract_claims(&value)),
            vec!["Computes the order total."]
        );
    }

    #[test]
    fn extract_claims_follows_field_order_dedupes_and_caps() {
        let value = serde_json::json!({
            "edge_cases": "Empty carts return zero.",
            "side_effects": ["Writes an audit log entry."],
            "error_modes": ["Fails when the cart is locked.", "fails when the cart is LOCKED"],
            "behavior": "Sums every line item.",
            "intent": "Computes the order total.",
        });
        assert_eq!(
            texts(&extract_claims(&value)),
            vec![
                "Computes the order total.",
                "Sums every line item.",
                "Fails when the cart is locked.",
                "Writes an audit log entry.",
                "Empty carts return zero.",
            ]
        );

        let many: Vec<String> = (0..60)
            .map(|index| format!("Distinct side effect number {index}"))
            .collect();
        let value = serde_json::json!({
            "intent": "Computes the order total.",
            "side_effects": many,
        });
        let claims = extract_claims(&value);
        assert_eq!(claims.len(), MAX_CLAIMS_PER_VERSION);
        assert_eq!(claims[0].text, "Computes the order total.");
    }

    #[test]
    fn extract_claims_is_lenient_about_shape() {
        let value = serde_json::json!({
            "intent": "Computes the order total.",
            "side_effects": [42, "Writes an audit log entry.", null, {"x": 1}],
            "unexpected": "Extra keys are ignored here.",
        });
        assert_eq!(
            texts(&extract_claims(&value)),
            vec!["Computes the order total.", "Writes an audit log entry."]
        );
    }

    #[test]
    fn prepare_history_counts_unparseable_and_partial_rows() {
        let records = vec![
            record(1, "test-provider", "not json"),
            record(
                2,
                "test-provider",
                r#"{"behavior":"Has no intent field here."}"#,
            ),
            record(3, "test-provider", r#"{"intent":"   "}"#),
            record(
                4,
                "test-provider",
                r#"{"intent":"Partial row still counts."}"#,
            ),
        ];
        let prepared = prepare_history(&records, &settings(20));
        assert_eq!(prepared.stats.skipped_unparseable, 3);
        assert_eq!(prepared.stats.considered, 1);
        assert_eq!(
            texts(&prepared.versions[0].claims),
            vec!["Partial row still counts."]
        );
        assert!(!prepared.newest_is_mock);
    }

    #[test]
    fn prepare_history_skips_mock_rows_and_flags_newest_mock() {
        let records = vec![
            record(1, "test-provider", r#"{"intent":"Real claim number one."}"#),
            record(2, "mock", r#"{"intent":"Provider says mock here."}"#),
            record(
                3,
                "test-provider",
                r#"{"intent":"[MOCK] placeholder intent text"}"#,
            ),
        ];
        let prepared = prepare_history(&records, &settings(20));
        assert_eq!(prepared.stats.raw, 3);
        assert_eq!(prepared.stats.skipped_mock, 2);
        assert_eq!(prepared.stats.considered, 1);
        assert!(prepared.newest_is_mock);

        let mut records = records;
        records.push(record(
            4,
            "test-provider",
            r#"{"intent":"Real claim again here."}"#,
        ));
        let prepared = prepare_history(&records, &settings(20));
        assert!(!prepared.newest_is_mock);
        assert_eq!(prepared.stats.considered, 2);
    }

    #[test]
    fn prepare_history_collapses_consecutive_identical_claim_sets() {
        let a = r#"{"intent":"Concept A holds here.","confidence":0.5}"#;
        let a_bumped = r#"{"intent":"concept a holds here","confidence":0.9}"#;
        let b = r#"{"intent":"Concept B holds here."}"#;
        let records = vec![
            record(1, "test-provider", a),
            record(2, "test-provider", a_bumped),
            record(3, "test-provider", b),
            record(4, "test-provider", a),
        ];
        let prepared = prepare_history(&records, &settings(20));
        assert_eq!(prepared.stats.collapsed, 1);
        assert_eq!(prepared.stats.considered, 3);
        assert_eq!(
            prepared
                .versions
                .iter()
                .map(|v| v.version)
                .collect::<Vec<_>>(),
            vec![2, 3, 4]
        );
    }

    #[test]
    fn prepare_history_applies_window_and_counts_models() {
        let records: Vec<SirHistoryRecord> = (1..=6)
            .map(|version| {
                let mut row = record(
                    version,
                    if version % 2 == 0 { "gemini" } else { "openai" },
                    &format!(r#"{{"intent":"Distinct claim number {version}."}}"#),
                );
                row.model = "m1".to_owned();
                row
            })
            .collect();
        let prepared = prepare_history(&records, &settings(4));
        assert_eq!(prepared.stats.outside_window, 2);
        assert_eq!(prepared.stats.considered, 4);
        assert_eq!(prepared.versions[0].version, 3);
        assert_eq!(prepared.stats.models.get("gemini/m1"), Some(&2));
        assert_eq!(prepared.stats.models.get("openai/m1"), Some(&2));
    }
}
