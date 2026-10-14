use std::collections::HashMap;
use std::path::{Path, PathBuf};

use aether_core::{Position, SourceRange, normalize_path};
use aether_parse::SymbolExtractor;
use aether_store::{SqliteStore, SymbolCatalogStore, SymbolRecord};

use crate::AetherMcpError;

/// One symbol's source as the workspace file holds it right now: the text of its range
/// and the content hash of that text, from the same read.
#[derive(Debug, Clone)]
pub(crate) struct LiveSymbolSource {
    pub(crate) source_hash: String,
    pub(crate) source_text: String,
}

/// Symbol sources as the workspace's files hold them right now: each file is read and
/// parsed once per instance, so a lookup that lists a file's symbols costs one parse.
/// `aether_symbol_lookup` reports the hash (and, on request, the text it was computed
/// from) as `source_hash`/`source_text`, and `aether_sir_inject` compares the caller's
/// `source_hash` against a fresh computation under the inject lock, so a SIR written for
/// the text a caller read never lands on a symbol whose body has since changed.
pub(crate) struct LiveSymbolSources {
    workspace: PathBuf,
    extractor: Option<SymbolExtractor>,
    by_file: HashMap<String, Option<HashMap<String, LiveSymbolSource>>>,
}

/// The real path of an indexed, workspace-relative `file_path`, provided it lies inside
/// the workspace once symlinks are resolved. An index entry can outlive the file it
/// named, and a link swapped in later (or a rooted or `..` path) must not make a tool
/// read another project's source. A missing file reports `NotFound`; a path that
/// resolves outside the workspace reports `PermissionDenied`.
pub(crate) fn contained_workspace_file(
    workspace: &Path,
    file_path: &str,
) -> std::io::Result<PathBuf> {
    let root = workspace.canonicalize()?;
    let resolved = root.join(file_path).canonicalize()?;
    if resolved.starts_with(&root) {
        Ok(resolved)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{file_path} resolves outside the workspace"),
        ))
    }
}

impl LiveSymbolSources {
    pub(crate) fn new(workspace: &Path) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
            extractor: None,
            by_file: HashMap::new(),
        }
    }

    /// The current content hash of `symbol_id` in `file_path`: `Ok(None)` when the file
    /// is gone or no longer declares the symbol; an error naming the file when it exists
    /// but cannot be read or parsed.
    pub(crate) fn hash_for(
        &mut self,
        file_path: &str,
        symbol_id: &str,
    ) -> Result<Option<String>, AetherMcpError> {
        Ok(self
            .source_for(file_path, symbol_id)?
            .map(|source| source.source_hash.clone()))
    }

    /// The current text and content hash of `symbol_id` in `file_path`, both from one
    /// read of the file: `Ok(None)` when the file is gone or no longer declares the
    /// symbol; an error naming the file when it exists but cannot be read or parsed.
    pub(crate) fn source_for(
        &mut self,
        file_path: &str,
        symbol_id: &str,
    ) -> Result<Option<&LiveSymbolSource>, AetherMcpError> {
        if !self.by_file.contains_key(file_path) {
            let sources = self.parse_file(file_path)?;
            self.by_file.insert(file_path.to_owned(), sources);
        }
        Ok(self
            .by_file
            .get(file_path)
            .and_then(|sources| sources.as_ref())
            .and_then(|sources| sources.get(symbol_id)))
    }

    fn parse_file(
        &mut self,
        file_path: &str,
    ) -> Result<Option<HashMap<String, LiveSymbolSource>>, AetherMcpError> {
        // Only a file that is gone means the symbol is gone with it. A file that exists
        // but cannot be read (permissions, an I/O error, a directory in its place) is a
        // failure the caller must see by name: read as "removed", `aether_symbol_lookup`
        // would silently report the match without a source and `/scan` would skip the
        // symbol on every pass with nothing pointing at the file or the cause.
        let source = match contained_workspace_file(&self.workspace, file_path)
            .and_then(std::fs::read_to_string)
        {
            Ok(source) => source,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(AetherMcpError::Message(format!(
                    "failed to read {file_path} for its current symbol sources: {err}"
                )));
            }
        };
        if self.extractor.is_none() {
            self.extractor = Some(SymbolExtractor::new().map_err(|err| {
                AetherMcpError::Message(format!("failed to initialize the parser: {err:#}"))
            })?);
        }
        let Some(extractor) = self.extractor.as_mut() else {
            return Err(AetherMcpError::Message(
                "the parser is unavailable after initialization".to_owned(),
            ));
        };
        let symbols = extractor
            .extract_from_path(Path::new(file_path), &source)
            .map_err(|err| {
                AetherMcpError::Message(format!(
                    "failed to parse {file_path} for its current symbol sources: {err:#}"
                ))
            })?;
        Ok(Some(
            symbols
                .into_iter()
                .filter_map(|symbol| {
                    let source_text = extract_symbol_source_text(&source, symbol.range)?;
                    Some((
                        symbol.id,
                        LiveSymbolSource {
                            source_hash: symbol.content_hash,
                            source_text,
                        },
                    ))
                })
                .collect(),
        ))
    }
}

/// The text a symbol's range covers in `source`, by byte offsets when the parser
/// recorded them, else by line and column.
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

pub(crate) fn effective_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(20).clamp(1, 100)
}

pub(crate) fn symbol_leaf_name(qualified_name: &str) -> &str {
    qualified_name
        .rsplit("::")
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(qualified_name)
}

pub(crate) fn is_type_symbol_kind(kind: &str) -> bool {
    matches!(kind.trim(), "struct" | "trait" | "enum" | "type_alias")
}

pub(crate) fn child_method_symbols(
    store: &SqliteStore,
    symbol: &SymbolRecord,
) -> Result<Vec<SymbolRecord>, AetherMcpError> {
    let prefix = format!("{}::", symbol.qualified_name);
    let mut methods = store
        .list_symbols_for_file(symbol.file_path.as_str())?
        .into_iter()
        .filter(|candidate| {
            candidate.qualified_name.starts_with(&prefix)
                && matches!(candidate.kind.as_str(), "function" | "method")
        })
        .collect::<Vec<_>>();
    methods.sort_by(|left, right| {
        left.qualified_name
            .cmp(&right.qualified_name)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(methods)
}

pub(crate) fn normalize_workspace_relative_path(
    workspace: &Path,
    value: &str,
    field_name: &str,
) -> Result<String, AetherMcpError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AetherMcpError::Message(format!(
            "{field_name} must not be empty"
        )));
    }

    let path = PathBuf::from(trimmed);
    let normalized = if path.is_absolute() {
        if !path.starts_with(workspace) {
            return Err(AetherMcpError::Message(format!(
                "{field_name} must be under workspace {}",
                workspace.display()
            )));
        }

        let relative = path.strip_prefix(workspace).map_err(|_| {
            AetherMcpError::Message(format!(
                "{field_name} must be under workspace {}",
                workspace.display()
            ))
        })?;
        normalize_path(&relative.to_string_lossy())
    } else {
        normalize_path(trimmed)
    };

    let mut normalized = normalized.trim().to_owned();
    while normalized.starts_with("./") {
        normalized = normalized[2..].to_owned();
    }
    if normalized != "/" {
        normalized = normalized.trim_end_matches('/').to_owned();
    }
    if normalized.is_empty() {
        return Err(AetherMcpError::Message(format!(
            "{field_name} must not be empty"
        )));
    }

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use aether_parse::SymbolExtractor;
    use tempfile::tempdir;

    use super::LiveSymbolSources;

    #[cfg(unix)]
    #[test]
    fn live_sources_refuse_files_that_resolve_outside_the_workspace() {
        let workspace = tempdir().expect("tempdir");
        let outside = tempdir().expect("outside tempdir");
        let source = "pub fn alpha() -> i32 {\n    2 // OUTSIDE_WORKSPACE\n}\n";
        fs::write(outside.path().join("other.rs"), source).expect("write outside source");
        fs::create_dir_all(workspace.path().join("src")).expect("mkdir src");
        std::os::unix::fs::symlink(
            outside.path().join("other.rs"),
            workspace.path().join("src/lib.rs"),
        )
        .expect("symlink");
        let symbol_id = SymbolExtractor::new()
            .expect("parser")
            .extract_from_path(Path::new("src/lib.rs"), source)
            .expect("parse")
            .remove(0)
            .id;

        let mut sources = LiveSymbolSources::new(workspace.path());
        let err = sources
            .source_for("src/lib.rs", &symbol_id)
            .expect_err("a file outside the workspace must not be read");
        assert!(err.to_string().contains("outside the workspace"), "{err}");
    }
}
