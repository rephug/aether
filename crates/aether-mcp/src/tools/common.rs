use std::path::{Path, PathBuf};

use aether_core::normalize_path;
use aether_store::{SqliteStore, SymbolCatalogStore, SymbolRecord};

use crate::AetherMcpError;

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
