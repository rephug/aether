use std::collections::HashMap;
use std::path::{Path, PathBuf};

use aether_core::normalize_path;
use aether_parse::SymbolExtractor;
use aether_store::{SqliteStore, SymbolCatalogStore, SymbolRecord};

use crate::AetherMcpError;

/// Content hashes of symbols as the workspace's files hold them right now: each file is
/// read and parsed once per instance, so a lookup that lists a file's symbols costs one
/// parse. `aether_symbol_lookup` reports them as `source_hash`, and `aether_sir_inject`
/// compares the caller's `source_hash` against a fresh computation under the inject
/// lock, so a SIR written for the text a caller read never lands on a symbol whose body
/// has since changed.
pub(crate) struct LiveSourceHashes {
    workspace: PathBuf,
    extractor: Option<SymbolExtractor>,
    by_file: HashMap<String, Option<HashMap<String, String>>>,
}

impl LiveSourceHashes {
    pub(crate) fn new(workspace: &Path) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
            extractor: None,
            by_file: HashMap::new(),
        }
    }

    /// The current content hash of `symbol_id` in `file_path`: `Ok(None)` when the file
    /// cannot be read or parsed or no longer declares the symbol.
    pub(crate) fn hash_for(
        &mut self,
        file_path: &str,
        symbol_id: &str,
    ) -> Result<Option<String>, AetherMcpError> {
        if !self.by_file.contains_key(file_path) {
            let hashes = self.parse_file(file_path)?;
            self.by_file.insert(file_path.to_owned(), hashes);
        }
        Ok(self
            .by_file
            .get(file_path)
            .and_then(|hashes| hashes.as_ref())
            .and_then(|hashes| hashes.get(symbol_id).cloned()))
    }

    fn parse_file(
        &mut self,
        file_path: &str,
    ) -> Result<Option<HashMap<String, String>>, AetherMcpError> {
        let Ok(source) = std::fs::read_to_string(self.workspace.join(file_path)) else {
            return Ok(None);
        };
        if self.extractor.is_none() {
            self.extractor = Some(SymbolExtractor::new().map_err(|err| {
                AetherMcpError::Message(format!("failed to initialize the parser: {err:#}"))
            })?);
        }
        let extractor = self
            .extractor
            .as_mut()
            .expect("extractor initialized above");
        let Ok(symbols) = extractor.extract_from_path(Path::new(file_path), &source) else {
            return Ok(None);
        };
        Ok(Some(
            symbols
                .into_iter()
                .map(|symbol| (symbol.id, symbol.content_hash))
                .collect(),
        ))
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
