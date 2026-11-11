use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use aether_parse::SymbolExtractor;
use anyhow::{Context, Result, ensure};
use serde::Serialize;

const MAX_SOURCE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct SourceLocation {
    pub start_line: usize,
    pub end_line: usize,
}

type FileLocations = Result<HashMap<String, SourceLocation>, String>;

#[derive(Default)]
pub(super) struct SourceNavigation {
    extractor: Option<SymbolExtractor>,
    files: HashMap<PathBuf, FileLocations>,
}

impl SourceNavigation {
    /// Match the current parse by indexed identity. Never guess by leaf name.
    pub(super) fn location(
        &mut self,
        workspace: &Path,
        file_path: &str,
        symbol_id: &str,
        warnings: &mut Vec<String>,
    ) -> Option<SourceLocation> {
        let key = workspace.join(file_path);
        if !self.files.contains_key(&key) {
            let parsed = self
                .parse_file(workspace, file_path)
                .map_err(|error| error.to_string());
            self.files.insert(key.clone(), parsed);
        }
        let warning = match self.files.get(&key) {
            Some(Ok(locations)) => {
                if let Some(location) = locations.get(symbol_id) {
                    return Some(*location);
                }
                format!(
                    "Source location omitted for indexed symbol '{symbol_id}' in '{file_path}': the current declaration does not match its indexed identity. Re-index changed source."
                )
            }
            Some(Err(error)) => {
                format!("Source locations unavailable for '{file_path}': {error}")
            }
            None => format!("Source locations unavailable for '{file_path}'"),
        };
        if !warnings.contains(&warning) {
            warnings.push(warning);
        }
        None
    }

    fn parse_file(
        &mut self,
        workspace: &Path,
        file_path: &str,
    ) -> Result<HashMap<String, SourceLocation>> {
        let relative = Path::new(file_path);
        ensure!(
            !file_path.is_empty()
                && relative
                    .components()
                    .all(|part| matches!(part, Component::Normal(_))),
            "indexed source path must be relative and remain inside the workspace"
        );
        let root = workspace
            .canonicalize()
            .context("could not resolve workspace")?;
        let path = workspace
            .join(relative)
            .canonicalize()
            .context("current source file could not be resolved")?;
        ensure!(path.starts_with(root), "source path leaves the workspace");
        ensure!(
            path.metadata()
                .context("could not inspect current source path")?
                .is_file(),
            "source is not a regular file"
        );
        let file = File::open(&path).context("could not open current source")?;
        ensure!(
            file.metadata()
                .context("could not inspect source")?
                .is_file(),
            "source is not a regular file"
        );
        let mut source = String::new();
        file.take(MAX_SOURCE_BYTES + 1)
            .read_to_string(&mut source)
            .context("could not read bounded UTF-8 source")?;
        ensure!(
            source.len() as u64 <= MAX_SOURCE_BYTES,
            "source exceeds the navigation size limit"
        );
        if self.extractor.is_none() {
            self.extractor =
                Some(SymbolExtractor::new().context("could not initialize source parser")?);
        }
        let parsed = self
            .extractor
            .as_mut()
            .context("source parser unavailable")?
            .extract_from_path(relative, &source)
            .context("could not parse current source")?;
        let mut locations = HashMap::new();
        for symbol in parsed {
            let (Some(start), Some(end)) = (symbol.range.start_byte, symbol.range.end_byte) else {
                continue;
            };
            let (Some(prefix), Some(fragment)) = (source.get(..start), source.get(start..end))
            else {
                continue;
            };
            let start_line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
            let end_line = start_line + fragment.lines().count().saturating_sub(1);
            locations.insert(
                symbol.id,
                SourceLocation {
                    start_line,
                    end_line,
                },
            );
        }
        Ok(locations)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn current_identity_selects_the_right_declaration_and_inclusive_lines() {
        let workspace = tempfile::tempdir().expect("temporary independent workspace");
        let source = "// independent café packet fixture\r\n\r\npub fn checksum(data: &[u8]) -> usize {\r\n    data.len()\r\n}\r\n\r\npub fn other() {}\r\n";
        fs::write(workspace.path().join("packet.rs"), source).expect("fixture source");
        let parsed = SymbolExtractor::new()
            .expect("parser")
            .extract_from_path(Path::new("packet.rs"), source)
            .expect("parse fixture");
        let symbol = parsed
            .iter()
            .find(|symbol| symbol.name == "checksum")
            .expect("checksum");
        let mut navigation = SourceNavigation::default();
        let mut warnings = Vec::new();
        assert_eq!(
            navigation.location(workspace.path(), "packet.rs", &symbol.id, &mut warnings),
            Some(SourceLocation {
                start_line: 3,
                end_line: 5
            })
        );
        assert!(warnings.is_empty());
        assert!(
            navigation
                .location(
                    workspace.path(),
                    "packet.rs",
                    "outdated-identity",
                    &mut warnings
                )
                .is_none()
        );
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("Re-index"));
        assert_eq!(navigation.files.len(), 1);
    }

    #[test]
    fn decorated_python_location_uses_current_parser_identity() {
        let workspace = tempfile::tempdir().expect("temporary independent workspace");
        let source = "# packet fixture\n@trace(\"café\")\ndef decode(packet):\n    return packet\n";
        fs::write(workspace.path().join("packet.py"), source).expect("fixture source");
        let parsed = SymbolExtractor::new()
            .expect("parser")
            .extract_from_path(Path::new("packet.py"), source)
            .expect("parse fixture");
        let symbol = parsed
            .iter()
            .find(|symbol| symbol.name == "decode")
            .expect("decode");
        let location = SourceNavigation::default()
            .location(workspace.path(), "packet.py", &symbol.id, &mut Vec::new())
            .expect("source location");
        assert_eq!(
            location,
            SourceLocation {
                // Python declaration ranges start at `def`, after decorators.
                start_line: 3,
                end_line: 4
            }
        );
    }

    #[test]
    fn unavailable_and_outside_paths_never_invent_locations() {
        let workspace = tempfile::tempdir().expect("temporary independent workspace");
        let mut navigation = SourceNavigation::default();
        let mut warnings = Vec::new();
        fs::create_dir(workspace.path().join("directory.rs")).expect("nonregular fixture");
        for file in ["missing.rs", "../outside.rs", "directory.rs"] {
            assert!(
                navigation
                    .location(workspace.path(), file, "some-id", &mut warnings)
                    .is_none()
            );
        }
        assert_eq!(warnings.len(), 3);
        assert!(warnings[1].contains("relative"));
        assert!(warnings[2].contains("not a regular file"));
    }

    #[test]
    fn oversized_source_is_omitted_before_parsing() {
        let workspace = tempfile::tempdir().expect("temporary independent workspace");
        fs::write(
            workspace.path().join("large.rs"),
            vec![b' '; MAX_SOURCE_BYTES as usize + 1],
        )
        .expect("bounded size fixture");
        let mut navigation = SourceNavigation::default();
        let mut warnings = Vec::new();
        assert!(
            navigation
                .location(workspace.path(), "large.rs", "some-id", &mut warnings)
                .is_none()
        );
        assert!(navigation.extractor.is_none());
        assert!(warnings[0].contains("size limit"));
    }
}
