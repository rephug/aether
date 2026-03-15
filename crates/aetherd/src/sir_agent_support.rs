use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_core::{Language, Position, SourceRange, Symbol, SymbolKind};
use aether_parse::SymbolExtractor;
use aether_store::{SqliteStore, SymbolCatalogStore, SymbolRecord};
use anyhow::{Context, Result, anyhow};

pub(crate) struct FreshSymbolSource {
    pub(crate) symbol: Symbol,
    pub(crate) symbol_source: String,
}

pub(crate) fn resolve_symbol(store: &SqliteStore, selector: &str) -> Result<SymbolRecord> {
    let selector = selector.trim();
    if selector.is_empty() {
        return Err(anyhow!("symbol selector must not be empty"));
    }

    if let Some(record) = store
        .get_symbol_record(selector)
        .with_context(|| format!("failed to look up symbol id '{selector}'"))?
    {
        return Ok(record);
    }

    if let Some(record) = store
        .get_symbol_by_qualified_name(selector)
        .with_context(|| format!("failed to look up qualified name '{selector}'"))?
    {
        return Ok(record);
    }

    let matches = store
        .search_symbols(selector, 10)
        .with_context(|| format!("failed to search symbols for '{selector}'"))?;
    match matches.as_slice() {
        [] => Err(anyhow!("symbol not found: {selector}")),
        [only] => store
            .get_symbol_record(only.symbol_id.as_str())
            .with_context(|| {
                format!(
                    "failed to load symbol record for search result '{}'",
                    only.symbol_id
                )
            })?
            .ok_or_else(|| anyhow!("symbol search returned missing record: {}", only.symbol_id)),
        _ => {
            let candidates = matches
                .iter()
                .map(|candidate| {
                    format!(
                        "{} [{}]",
                        candidate.qualified_name.trim(),
                        candidate.file_path.trim()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n  - ");
            Err(anyhow!(
                "ambiguous symbol selector '{selector}'. Candidates:\n  - {candidates}"
            ))
        }
    }
}

pub(crate) fn load_fresh_symbol_source(
    workspace: &Path,
    record: &SymbolRecord,
) -> Result<FreshSymbolSource> {
    let full_path = workspace.join(&record.file_path);
    let file_source = fs::read_to_string(&full_path)
        .with_context(|| format!("failed to read source file {}", full_path.display()))?;

    let mut extractor = SymbolExtractor::new().context("failed to initialize symbol extractor")?;
    let symbols = extractor
        .extract_from_path(Path::new(&record.file_path), &file_source)
        .with_context(|| format!("failed to parse {}", record.file_path))?;

    let symbol = symbols
        .iter()
        .find(|symbol| symbol.id == record.id)
        .or_else(|| {
            symbols
                .iter()
                .find(|symbol| symbol.qualified_name == record.qualified_name)
        })
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "symbol '{}' no longer found in source file {}",
                record.qualified_name,
                record.file_path
            )
        })?;

    let symbol_source =
        extract_symbol_source_text(&file_source, symbol.range).ok_or_else(|| {
            anyhow!(
                "failed to extract source slice for '{}' in {}",
                record.qualified_name,
                record.file_path
            )
        })?;

    Ok(FreshSymbolSource {
        symbol,
        symbol_source,
    })
}

pub(crate) fn symbol_from_record(record: &SymbolRecord) -> Result<Symbol> {
    Ok(Symbol {
        id: record.id.clone(),
        language: parse_language(record.language.as_str())?,
        file_path: record.file_path.clone(),
        kind: parse_symbol_kind(record.kind.as_str())?,
        name: record
            .qualified_name
            .rsplit("::")
            .next()
            .or_else(|| record.qualified_name.rsplit('.').next())
            .unwrap_or(record.qualified_name.as_str())
            .to_owned(),
        qualified_name: record.qualified_name.clone(),
        signature_fingerprint: record.signature_fingerprint.clone(),
        content_hash: String::new(),
        range: SourceRange {
            start: Position { line: 1, column: 1 },
            end: Position { line: 1, column: 1 },
            start_byte: Some(0),
            end_byte: Some(0),
        },
    })
}

pub(crate) fn parse_text_list(raw: &str) -> Vec<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Vec::new();
    }

    let split = if raw.contains('\n') {
        raw.lines().collect::<Vec<_>>()
    } else {
        raw.split(';').collect::<Vec<_>>()
    };

    split
        .into_iter()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

pub(crate) fn first_sentence(value: &str) -> String {
    value
        .split(['\n', '.', '!', '?'])
        .map(str::trim)
        .find(|segment| !segment.is_empty())
        .unwrap_or_default()
        .to_owned()
}

pub(crate) fn first_line(value: &str) -> String {
    value
        .lines()
        .map(str::trim)
        .find(|segment| !segment.is_empty())
        .unwrap_or_default()
        .to_owned()
}

pub(crate) fn current_unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

pub(crate) fn format_relative_age(timestamp: i64) -> String {
    if timestamp <= 0 {
        return "unknown".to_owned();
    }

    let delta = current_unix_timestamp_secs().saturating_sub(timestamp);
    if delta >= 86_400 {
        format!("{}d ago", delta / 86_400)
    } else if delta >= 3_600 {
        format!("{}h ago", delta / 3_600)
    } else if delta >= 60 {
        format!("{}m ago", delta / 60)
    } else {
        format!("{delta}s ago")
    }
}

pub(crate) fn read_selector_file(path: &Path) -> Result<Vec<String>> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read selector file {}", path.display()))?;
    let mut selectors = Vec::new();
    for line in raw.lines() {
        let selector = line.trim();
        if selector.is_empty() {
            continue;
        }
        if selectors.iter().any(|existing| existing == selector) {
            continue;
        }
        selectors.push(selector.to_owned());
    }
    Ok(selectors)
}

pub(crate) fn output_path(path: &str) -> PathBuf {
    PathBuf::from(path.trim())
}

pub(crate) fn extract_symbol_source_text(source: &str, range: SourceRange) -> Option<String> {
    let start = range
        .start_byte
        .or_else(|| byte_offset_for_position(source, range.start))?;
    let end = range
        .end_byte
        .or_else(|| byte_offset_for_position(source, range.end))?;

    if start > end || end > source.len() {
        return None;
    }

    source.get(start..end).map(str::to_owned)
}

fn byte_offset_for_position(source: &str, position: Position) -> Option<usize> {
    let mut line = 1usize;
    let mut column = 1usize;

    if position.line == 1 && position.column == 1 {
        return Some(0);
    }

    for (index, ch) in source.char_indices() {
        if line == position.line && column == position.column {
            return Some(index);
        }

        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += ch.len_utf8();
        }
    }

    if line == position.line && column == position.column {
        Some(source.len())
    } else {
        None
    }
}

fn parse_language(raw: &str) -> Result<Language> {
    match raw.trim() {
        "rust" => Ok(Language::Rust),
        "typescript" => Ok(Language::TypeScript),
        "tsx" => Ok(Language::Tsx),
        "javascript" => Ok(Language::JavaScript),
        "jsx" => Ok(Language::Jsx),
        "python" => Ok(Language::Python),
        other => Err(anyhow!("unsupported symbol language '{other}'")),
    }
}

fn parse_symbol_kind(raw: &str) -> Result<SymbolKind> {
    match raw.trim() {
        "function" => Ok(SymbolKind::Function),
        "method" => Ok(SymbolKind::Method),
        "class" => Ok(SymbolKind::Class),
        "variable" => Ok(SymbolKind::Variable),
        "struct" => Ok(SymbolKind::Struct),
        "enum" => Ok(SymbolKind::Enum),
        "trait" => Ok(SymbolKind::Trait),
        "interface" => Ok(SymbolKind::Interface),
        "type_alias" => Ok(SymbolKind::TypeAlias),
        other => Err(anyhow!("unsupported symbol kind '{other}'")),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use aether_core::Language;
    use aether_store::{SymbolCatalogStore, SymbolRecord};
    use tempfile::tempdir;

    use super::{load_fresh_symbol_source, parse_text_list, resolve_symbol};

    fn symbol_record(
        id: &str,
        file_path: &str,
        qualified_name: &str,
        signature_fingerprint: &str,
    ) -> SymbolRecord {
        SymbolRecord {
            id: id.to_owned(),
            file_path: file_path.to_owned(),
            language: Language::Rust.as_str().to_owned(),
            kind: "function".to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature_fingerprint: signature_fingerprint.to_owned(),
            last_seen_at: 1_700_000_000,
        }
    }

    fn write_workspace() -> tempfile::TempDir {
        let temp = tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join("src")).expect("create src");
        fs::write(
            temp.path().join("src/lib.rs"),
            "pub fn alpha() -> i32 { 1 }\n\npub fn beta() -> i32 { alpha() }\n",
        )
        .expect("write source");
        temp
    }

    #[test]
    fn resolve_symbol_prefers_exact_id_then_qualified_name() {
        let temp = write_workspace();
        let store = aether_store::SqliteStore::open(temp.path()).expect("open store");
        store
            .upsert_symbol(symbol_record(
                "sym-alpha",
                "src/lib.rs",
                "demo::alpha",
                "sig-alpha",
            ))
            .expect("upsert alpha");
        store
            .upsert_symbol(symbol_record(
                "sym-beta",
                "src/lib.rs",
                "demo::beta",
                "sig-beta",
            ))
            .expect("upsert beta");

        let by_id = resolve_symbol(&store, "sym-alpha").expect("resolve by id");
        assert_eq!(by_id.id, "sym-alpha");

        let by_name = resolve_symbol(&store, "demo::beta").expect("resolve by qname");
        assert_eq!(by_name.id, "sym-beta");
    }

    #[test]
    fn resolve_symbol_reports_ambiguous_search_results() {
        let temp = write_workspace();
        let store = aether_store::SqliteStore::open(temp.path()).expect("open store");
        store
            .upsert_symbol(symbol_record(
                "sym-alpha",
                "src/lib.rs",
                "demo::alpha",
                "sig-alpha",
            ))
            .expect("upsert alpha");
        store
            .upsert_symbol(symbol_record(
                "sym-alpha-two",
                "src/other.rs",
                "demo::alpha_two",
                "sig-alpha-two",
            ))
            .expect("upsert alpha two");

        let err = resolve_symbol(&store, "alpha").expect_err("ambiguous should fail");
        assert!(err.to_string().contains("ambiguous symbol selector"));
    }

    #[test]
    fn load_fresh_symbol_source_extracts_current_slice() {
        let temp = tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join("src")).expect("create src");
        let file_path = "src/lib.rs";
        fs::write(
            temp.path().join(file_path),
            "pub fn alpha() -> i32 {\n    1\n}\n",
        )
        .expect("write source");

        let source = fs::read_to_string(temp.path().join(file_path)).expect("read source");
        let mut extractor = aether_parse::SymbolExtractor::new().expect("extractor");
        let symbols = extractor
            .extract_from_source(Language::Rust, file_path, &source)
            .expect("parse");
        let symbol = symbols.first().expect("symbol");

        let fresh = load_fresh_symbol_source(
            temp.path(),
            &symbol_record(
                symbol.id.as_str(),
                file_path,
                symbol.qualified_name.as_str(),
                symbol.signature_fingerprint.as_str(),
            ),
        )
        .expect("load fresh symbol");

        assert!(fresh.symbol_source.contains("pub fn alpha() -> i32"));
    }

    #[test]
    fn parse_text_list_supports_semicolons_and_lines() {
        assert_eq!(
            parse_text_list("writes cache; logs metrics"),
            vec!["writes cache".to_owned(), "logs metrics".to_owned()]
        );
        assert_eq!(
            parse_text_list("writes cache\nlogs metrics"),
            vec!["writes cache".to_owned(), "logs metrics".to_owned()]
        );
    }
}
