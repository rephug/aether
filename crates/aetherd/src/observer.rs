use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use aether_core::{Language, Symbol, SymbolChangeEvent, diff_symbols, normalize_path};
use aether_parse::{SymbolExtractor, language_for_path};
use anyhow::{Context, Result};
use walkdir::WalkDir;

#[derive(Debug, Clone)]
struct FileSnapshot {
    language: Language,
    symbols: Vec<Symbol>,
}

pub struct ObserverState {
    workspace: PathBuf,
    extractor: SymbolExtractor,
    snapshots: HashMap<PathBuf, FileSnapshot>,
}

impl ObserverState {
    pub fn new(workspace: PathBuf) -> Result<Self> {
        Ok(Self {
            workspace,
            extractor: SymbolExtractor::new()?,
            snapshots: HashMap::new(),
        })
    }

    pub fn seed_from_disk(&mut self) -> Result<()> {
        for entry in WalkDir::new(&self.workspace)
            .into_iter()
            .filter_map(std::result::Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }

            let full_path = entry.path();
            if is_ignored_path(full_path) {
                continue;
            }

            let Some(language) = language_for_path(full_path) else {
                continue;
            };

            let relative = relative_workspace_path(&self.workspace, full_path);
            let display_path = normalize_path(&relative.to_string_lossy());

            let source = match fs::read_to_string(full_path) {
                Ok(source) => source,
                Err(_) => continue,
            };

            let symbols = self
                .extractor
                .extract_from_source(language, &display_path, &source)
                .with_context(|| {
                    format!("failed to extract symbols from {}", full_path.display())
                })?;

            self.snapshots
                .insert(relative, FileSnapshot { language, symbols });
        }

        Ok(())
    }

    pub fn process_path(&mut self, path: &Path) -> Result<Option<SymbolChangeEvent>> {
        if is_ignored_path(path) {
            return Ok(None);
        }

        let relative = relative_workspace_path(&self.workspace, path);
        let display_path = normalize_path(&relative.to_string_lossy());

        let previous = self.snapshots.get(&relative).cloned();
        let language =
            language_for_path(&relative).or_else(|| previous.as_ref().map(|snap| snap.language));

        let Some(language) = language else {
            return Ok(None);
        };

        let current_symbols = if path.exists() && path.is_file() {
            match fs::read_to_string(path) {
                Ok(source) => self
                    .extractor
                    .extract_from_source(language, &display_path, &source)
                    .with_context(|| {
                        format!("failed to extract symbols from {}", path.display())
                    })?,
                Err(err) if err.kind() == ErrorKind::NotFound => Vec::new(),
                Err(err) => {
                    return Err(err).with_context(|| {
                        format!("failed to read changed file {}", path.display())
                    });
                }
            }
        } else {
            Vec::new()
        };

        let previous_symbols = previous
            .as_ref()
            .map(|snapshot| snapshot.symbols.as_slice())
            .unwrap_or(&[]);

        let event = diff_symbols(&display_path, language, previous_symbols, &current_symbols);

        if current_symbols.is_empty() {
            self.snapshots.remove(&relative);
        } else {
            self.snapshots.insert(
                relative,
                FileSnapshot {
                    language,
                    symbols: current_symbols,
                },
            );
        }

        if event.is_empty() {
            Ok(None)
        } else {
            Ok(Some(event))
        }
    }

    pub fn initial_symbol_events(&self) -> Vec<SymbolChangeEvent> {
        let mut events = Vec::new();

        for (relative_path, snapshot) in &self.snapshots {
            if snapshot.symbols.is_empty() {
                continue;
            }

            let mut added = snapshot.symbols.clone();
            added.sort_by(|a, b| a.id.cmp(&b.id));

            events.push(SymbolChangeEvent {
                file_path: normalize_path(&relative_path.to_string_lossy()),
                language: snapshot.language,
                added,
                removed: Vec::new(),
                updated: Vec::new(),
            });
        }

        events.sort_by(|a, b| a.file_path.cmp(&b.file_path));
        events
    }
}

#[derive(Debug, Default)]
pub struct DebounceQueue {
    pending: HashMap<PathBuf, Instant>,
}

impl DebounceQueue {
    pub fn mark(&mut self, path: PathBuf, now: Instant) {
        self.pending.insert(path, now);
    }

    pub fn drain_due(&mut self, now: Instant, debounce: Duration) -> Vec<PathBuf> {
        let mut due = Vec::new();

        self.pending.retain(|path, last_seen| {
            if now.duration_since(*last_seen) >= debounce {
                due.push(path.clone());
                false
            } else {
                true
            }
        });

        due.sort();
        due
    }
}

pub fn relative_workspace_path(workspace: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        if let Ok(relative) = path.strip_prefix(workspace) {
            return relative.to_path_buf();
        }
    }
    path.to_path_buf()
}

pub fn is_ignored_path(path: &Path) -> bool {
    path.components().any(|component| {
        let part = component.as_os_str().to_string_lossy();
        part == ".git" || part == ".aether" || part == "target"
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use anyhow::Result;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn debounce_queue_coalesces_multiple_events_for_same_path() {
        let mut queue = DebounceQueue::default();
        let path = PathBuf::from("src/lib.rs");
        let base = Instant::now();

        queue.mark(path.clone(), base);
        queue.mark(path.clone(), base + Duration::from_millis(100));

        let none_due = queue.drain_due(
            base + Duration::from_millis(350),
            Duration::from_millis(300),
        );
        assert!(none_due.is_empty());

        let due = queue.drain_due(
            base + Duration::from_millis(450),
            Duration::from_millis(300),
        );
        assert_eq!(due, vec![path]);
    }

    #[test]
    fn process_path_reports_only_updated_symbol_when_one_function_changes() -> Result<()> {
        let temp = tempdir()?;
        let file = temp.path().join("lib.rs");

        fs::write(&file, "fn keep() -> i32 { 1 }\nfn change() -> i32 { 1 }\n")?;

        let mut observer = ObserverState::new(temp.path().to_path_buf())?;
        observer.seed_from_disk()?;

        fs::write(&file, "fn keep() -> i32 { 1 }\nfn change() -> i32 { 2 }\n")?;

        let event = observer.process_path(&file)?.expect("event expected");

        assert!(event.added.is_empty());
        assert!(event.removed.is_empty());
        assert_eq!(event.updated.len(), 1);
        assert_eq!(event.updated[0].name, "change");

        Ok(())
    }

    #[test]
    fn process_path_reports_removed_symbols_when_file_deleted() -> Result<()> {
        let temp = tempdir()?;
        let file = temp.path().join("lib.rs");

        fs::write(&file, "fn keep() -> i32 { 1 }\n")?;

        let mut observer = ObserverState::new(temp.path().to_path_buf())?;
        observer.seed_from_disk()?;

        fs::remove_file(&file)?;

        let event = observer
            .process_path(&file)?
            .expect("removal event expected");
        assert!(event.added.is_empty());
        assert!(event.updated.is_empty());
        assert_eq!(event.removed.len(), 1);
        assert_eq!(event.removed[0].name, "keep");

        Ok(())
    }
}
