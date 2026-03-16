use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use aether_parse::SymbolExtractor;
use anyhow::{Context, Result, anyhow};

use crate::sir_agent_support::extract_symbol_source_text;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSlice {
    pub file_path: String,
    pub language: String,
    pub sections: Vec<SliceSection>,
    pub omitted_lines: usize,
    pub total_lines: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceSection {
    pub symbol_name: String,
    pub start_line: usize,
    pub end_line: usize,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceNeighbor {
    pub symbol_id: String,
    pub depth: u32,
}

#[derive(Debug, Clone)]
struct PendingRange {
    names: Vec<String>,
    start_line: usize,
    end_line: usize,
}

pub fn slice_file_for_context(
    workspace: &Path,
    file_path: &str,
    target_symbol_ids: &[String],
    neighbor_symbol_ids: &[SliceNeighbor],
    depth: u32,
    context_lines: usize,
) -> Result<FileSlice> {
    let full_path = workspace.join(file_path);
    let source = fs::read_to_string(&full_path)
        .with_context(|| format!("failed to read source file {}", full_path.display()))?;
    let lines = source_lines(source.as_str());
    let total_lines = lines.len();
    let language = language_name(Path::new(file_path));

    if total_lines < 50 {
        return Ok(FileSlice {
            file_path: file_path.to_owned(),
            language,
            sections: vec![SliceSection {
                symbol_name: label_for_whole_file(
                    target_symbol_ids,
                    neighbor_symbol_ids,
                    file_path,
                ),
                start_line: 1,
                end_line: total_lines.max(1),
                source,
            }],
            omitted_lines: 0,
            total_lines,
        });
    }

    let mut extractor = SymbolExtractor::new().context("failed to initialize symbol extractor")?;
    let parsed = extractor
        .extract_from_path(Path::new(file_path), source.as_str())
        .with_context(|| format!("failed to parse {file_path}"))?;

    let target_ids = target_symbol_ids
        .iter()
        .map(|value| value.as_str())
        .collect::<HashSet<_>>();
    let mut neighbor_depths: HashMap<&str, u32> = HashMap::new();
    for neighbor in neighbor_symbol_ids {
        if neighbor.depth <= depth {
            neighbor_depths
                .entry(neighbor.symbol_id.as_str())
                .and_modify(|existing| *existing = (*existing).min(neighbor.depth))
                .or_insert(neighbor.depth);
        }
    }

    let mut selected = Vec::new();
    for symbol in &parsed {
        if target_ids.contains(symbol.id.as_str()) {
            selected.push(full_body_range(&source, symbol)?);
            continue;
        }

        let Some(symbol_depth) = neighbor_depths.get(symbol.id.as_str()).copied() else {
            continue;
        };
        if symbol_depth <= 1 {
            selected.push(immediate_neighbor_range(lines.as_slice(), &source, symbol)?);
        } else {
            selected.push(signature_range(symbol));
        }
    }

    if selected.is_empty() {
        return Err(anyhow!(
            "no selected symbol ranges matched the current parse for {file_path}"
        ));
    }

    selected.sort_by_key(|range| range.start_line);
    let merged = merge_ranges(selected, 5);
    let expanded = merged
        .into_iter()
        .map(|range| expand_range(range, context_lines, total_lines))
        .collect::<Vec<_>>();
    let expanded = merge_overlapping_ranges(expanded);

    let included_lines = expanded
        .iter()
        .map(|range| {
            range
                .end_line
                .saturating_sub(range.start_line)
                .saturating_add(1)
        })
        .sum::<usize>();
    let omitted_lines = total_lines.saturating_sub(included_lines);
    let sections = expanded
        .into_iter()
        .map(|range| slice_section(lines.as_slice(), range))
        .collect::<Vec<_>>();

    Ok(FileSlice {
        file_path: file_path.to_owned(),
        language,
        sections,
        omitted_lines,
        total_lines,
    })
}

pub fn render_file_slice(slice: &FileSlice) -> String {
    let mut out = String::new();
    for (index, section) in slice.sections.iter().enumerate() {
        if index > 0 {
            let previous = &slice.sections[index - 1];
            let omitted = section
                .start_line
                .saturating_sub(previous.end_line)
                .saturating_sub(1);
            if omitted > 0 {
                out.push_str(&format!("// ... ({omitted} lines omitted) ...\n"));
            }
        }
        out.push_str(section.source.as_str());
        if !section.source.ends_with('\n') && index + 1 < slice.sections.len() {
            out.push('\n');
        }
    }
    out
}

fn full_body_range(source: &str, symbol: &aether_core::Symbol) -> Result<PendingRange> {
    let start_line = symbol.range.start.line.max(1);
    let full_source = extract_symbol_source_text(source, symbol.range).ok_or_else(|| {
        anyhow!(
            "failed to extract source for '{}' from {}",
            symbol.qualified_name,
            symbol.file_path
        )
    })?;
    let end_line = start_line
        .saturating_add(count_lines(full_source.as_str()).saturating_sub(1))
        .max(start_line);
    Ok(PendingRange {
        names: vec![symbol.qualified_name.clone()],
        start_line,
        end_line,
    })
}

fn immediate_neighbor_range(
    lines: &[&str],
    source: &str,
    symbol: &aether_core::Symbol,
) -> Result<PendingRange> {
    let full_source = extract_symbol_source_text(source, symbol.range).ok_or_else(|| {
        anyhow!(
            "failed to extract source for '{}' from {}",
            symbol.qualified_name,
            symbol.file_path
        )
    })?;
    let start_line = symbol.range.start.line.max(1);
    let full_line_count = count_lines(full_source.as_str());
    let end_line = start_line
        .saturating_add(full_line_count.saturating_sub(1))
        .max(start_line);
    if full_line_count <= 5 {
        return Ok(PendingRange {
            names: vec![symbol.qualified_name.clone()],
            start_line,
            end_line,
        });
    }

    let symbol_lines = source_lines(full_source.as_str());
    let signature_offset = symbol_lines
        .iter()
        .position(|line| is_signature_line(line.trim_start()))
        .unwrap_or(0);
    let doc_offset = symbol_lines
        .iter()
        .position(|line| is_doc_comment_line(line.trim_start()));
    let signature_line = start_line.saturating_add(signature_offset);
    Ok(PendingRange {
        names: vec![symbol.qualified_name.clone()],
        start_line: doc_offset
            .map(|offset| start_line.saturating_add(offset))
            .or_else(|| first_doc_comment_line(lines, signature_line))
            .unwrap_or(signature_line),
        end_line: signature_line,
    })
}

fn first_doc_comment_line(lines: &[&str], signature_line: usize) -> Option<usize> {
    if signature_line <= 1 {
        return None;
    }

    let mut cursor = signature_line.saturating_sub(1);
    let mut found = None;
    while cursor > 0 {
        let line = lines.get(cursor.saturating_sub(1))?.trim_start();
        if line.is_empty() {
            break;
        }
        if is_doc_comment_line(line) {
            found = Some(cursor);
            cursor = cursor.saturating_sub(1);
            continue;
        }
        break;
    }
    found
}

fn signature_range(symbol: &aether_core::Symbol) -> PendingRange {
    let start_line = symbol.range.start.line.max(1);
    PendingRange {
        names: vec![symbol.qualified_name.clone()],
        start_line,
        end_line: start_line,
    }
}

fn is_doc_comment_line(line: &str) -> bool {
    line.starts_with("///")
        || line.starts_with("//!")
        || line.starts_with("/**")
        || line.starts_with('*')
        || line.starts_with("*/")
        || line.starts_with("##")
}

fn is_signature_line(line: &str) -> bool {
    !line.is_empty()
        && !is_doc_comment_line(line)
        && !line.starts_with("#[")
        && !line.starts_with('@')
}

fn merge_ranges(ranges: Vec<PendingRange>, merge_gap: usize) -> Vec<PendingRange> {
    let mut merged: Vec<PendingRange> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.start_line <= last.end_line.saturating_add(merge_gap).saturating_add(1)
        {
            last.end_line = last.end_line.max(range.end_line);
            merge_names(&mut last.names, range.names);
            continue;
        }
        merged.push(range);
    }
    merged
}

fn expand_range(mut range: PendingRange, context_lines: usize, total_lines: usize) -> PendingRange {
    range.start_line = range.start_line.saturating_sub(context_lines).max(1);
    range.end_line = (range.end_line.saturating_add(context_lines)).min(total_lines.max(1));
    range
}

fn merge_overlapping_ranges(ranges: Vec<PendingRange>) -> Vec<PendingRange> {
    let mut merged: Vec<PendingRange> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.start_line <= last.end_line.saturating_add(1)
        {
            last.end_line = last.end_line.max(range.end_line);
            merge_names(&mut last.names, range.names);
            continue;
        }
        merged.push(range);
    }
    merged
}

fn merge_names(existing: &mut Vec<String>, new_names: Vec<String>) {
    for name in new_names {
        if !existing.iter().any(|value| value == &name) {
            existing.push(name);
        }
    }
}

fn slice_section(lines: &[&str], range: PendingRange) -> SliceSection {
    let start_index = range.start_line.saturating_sub(1);
    let end_index = range.end_line.min(lines.len());
    let source = lines[start_index..end_index].concat();
    SliceSection {
        symbol_name: range.names.join(", "),
        start_line: range.start_line,
        end_line: range.end_line,
        source,
    }
}

fn label_for_whole_file(
    target_symbol_ids: &[String],
    neighbor_symbol_ids: &[SliceNeighbor],
    file_path: &str,
) -> String {
    if !target_symbol_ids.is_empty() {
        return target_symbol_ids.join(", ");
    }
    if !neighbor_symbol_ids.is_empty() {
        return neighbor_symbol_ids
            .iter()
            .map(|neighbor| neighbor.symbol_id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
    }
    file_path.to_owned()
}

fn language_name(path: &Path) -> String {
    match path.extension().and_then(|value| value.to_str()) {
        Some("rs") => "rust",
        Some("ts") => "typescript",
        Some("tsx") => "tsx",
        Some("js") => "javascript",
        Some("jsx") => "jsx",
        Some("py") => "python",
        _ => "text",
    }
    .to_owned()
}

fn source_lines(source: &str) -> Vec<&str> {
    let mut lines = source.split_inclusive('\n').collect::<Vec<_>>();
    if !source.is_empty() && !source.ends_with('\n') {
        if let Some(last) = source.rsplit_once('\n').map(|(_, tail)| tail) {
            if !tail_present(lines.last().copied(), last) {
                lines.push(last);
            }
        } else if lines.is_empty() {
            lines.push(source);
        }
    }
    if lines.is_empty() {
        lines.push("");
    }
    lines
}

fn tail_present(current: Option<&str>, tail: &str) -> bool {
    current == Some(tail)
}

fn count_lines(source: &str) -> usize {
    if source.is_empty() {
        0
    } else {
        source.lines().count()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use aether_core::Language;
    use tempfile::tempdir;

    use super::{SliceNeighbor, render_file_slice, slice_file_for_context};

    fn write_large_source() -> (tempfile::TempDir, String, Vec<aether_core::Symbol>) {
        let temp = tempdir().expect("tempdir");
        let relative = "src/lib.rs".to_owned();
        fs::create_dir_all(temp.path().join("src")).expect("create src");
        let mut source = String::new();
        for index in 0..25 {
            source.push_str(&format!("// filler {index}\n"));
        }
        source.push_str("/// alpha docs\npub fn alpha() -> i32 {\n    1\n}\n\n");
        for index in 25..45 {
            source.push_str(&format!("// filler {index}\n"));
        }
        source.push_str(
            "/// beta docs\npub fn beta() -> i32 {\n    let value = alpha();\n    let doubled = value + 1;\n    let offset = doubled + 1;\n    offset\n}\n\n",
        );
        source.push_str("pub fn gamma() -> i32 {\n    beta()\n}\n");
        for index in 45..60 {
            source.push_str(&format!("// filler {index}\n"));
        }
        fs::write(temp.path().join(&relative), &source).expect("write source");

        let mut extractor = aether_parse::SymbolExtractor::new().expect("extractor");
        let symbols = extractor
            .extract_from_source(Language::Rust, relative.as_str(), &source)
            .expect("parse");
        (temp, relative, symbols)
    }

    #[test]
    fn single_symbol_extraction_returns_target_body() {
        let (temp, relative, symbols) = write_large_source();
        let beta = symbols
            .iter()
            .find(|symbol| symbol.qualified_name.ends_with("beta"))
            .expect("beta");

        let slice = slice_file_for_context(
            temp.path(),
            relative.as_str(),
            std::slice::from_ref(&beta.id),
            &[],
            0,
            0,
        )
        .expect("slice file");
        let rendered = render_file_slice(&slice);

        assert!(rendered.contains("pub fn beta() -> i32"));
        assert!(rendered.contains("alpha()"));
        assert!(!rendered.contains("pub fn gamma()"));
    }

    #[test]
    fn adjacent_symbols_within_five_lines_merge() {
        let (temp, relative, symbols) = write_large_source();
        let beta = symbols
            .iter()
            .find(|symbol| symbol.qualified_name.ends_with("beta"))
            .expect("beta");
        let gamma = symbols
            .iter()
            .find(|symbol| symbol.qualified_name.ends_with("gamma"))
            .expect("gamma");

        let slice = slice_file_for_context(
            temp.path(),
            relative.as_str(),
            &[beta.id.clone(), gamma.id.clone()],
            &[],
            0,
            0,
        )
        .expect("slice file");

        assert_eq!(slice.sections.len(), 1);
        let rendered = render_file_slice(&slice);
        assert!(rendered.contains("pub fn beta() -> i32"));
        assert!(rendered.contains("pub fn gamma() -> i32"));
    }

    #[test]
    fn elision_markers_report_omitted_lines() {
        let (temp, relative, symbols) = write_large_source();
        let alpha = symbols
            .iter()
            .find(|symbol| symbol.qualified_name.ends_with("alpha"))
            .expect("alpha");
        let gamma = symbols
            .iter()
            .find(|symbol| symbol.qualified_name.ends_with("gamma"))
            .expect("gamma");

        let slice = slice_file_for_context(
            temp.path(),
            relative.as_str(),
            &[alpha.id.clone(), gamma.id.clone()],
            &[],
            0,
            0,
        )
        .expect("slice file");
        let rendered = render_file_slice(&slice);

        assert!(rendered.contains("// ... ("));
        assert!(rendered.contains("lines omitted"));
    }

    #[test]
    fn files_under_fifty_lines_return_whole_file() {
        let temp = tempdir().expect("tempdir");
        let relative = "src/lib.rs";
        fs::create_dir_all(temp.path().join("src")).expect("create src");
        let source = "pub fn alpha() {}\n\npub fn beta() {}\n";
        fs::write(temp.path().join(relative), source).expect("write source");

        let slice =
            slice_file_for_context(temp.path(), relative, &[], &[], 0, 3).expect("slice file");

        assert_eq!(slice.omitted_lines, 0);
        assert_eq!(render_file_slice(&slice), source);
    }

    #[test]
    fn context_lines_expand_slice_padding() {
        let (temp, relative, symbols) = write_large_source();
        let beta = symbols
            .iter()
            .find(|symbol| symbol.qualified_name.ends_with("beta"))
            .expect("beta");

        let tight = slice_file_for_context(
            temp.path(),
            relative.as_str(),
            std::slice::from_ref(&beta.id),
            &[],
            0,
            0,
        )
        .expect("tight slice");
        let padded = slice_file_for_context(
            temp.path(),
            relative.as_str(),
            std::slice::from_ref(&beta.id),
            &[],
            0,
            2,
        )
        .expect("padded slice");

        assert!(render_file_slice(&padded).len() > render_file_slice(&tight).len());
    }

    #[test]
    fn immediate_neighbors_use_signature_and_doc_line() {
        let (temp, relative, symbols) = write_large_source();
        let beta = symbols
            .iter()
            .find(|symbol| symbol.qualified_name.ends_with("beta"))
            .expect("beta");

        let slice = slice_file_for_context(
            temp.path(),
            relative.as_str(),
            &[],
            &[SliceNeighbor {
                symbol_id: beta.id.clone(),
                depth: 1,
            }],
            1,
            0,
        )
        .expect("slice file");
        let rendered = render_file_slice(&slice);

        assert!(rendered.contains("/// beta docs"));
        assert!(rendered.contains("pub fn beta() -> i32"));
        assert!(!rendered.contains("alpha()"));
    }
}
