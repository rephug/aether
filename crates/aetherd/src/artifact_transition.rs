//! Retire generated Full-profile entry points before selecting Compact.
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use super::GeneratedFile;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedAgentArtifact {
    pub original_path: PathBuf,
    pub archive_path: PathBuf,
}

pub(super) struct Transition {
    pub archived: Vec<ArchivedAgentArtifact>,
    pub refreshed: Vec<GeneratedFile>,
}

pub(super) fn check_output_paths(workspace: &Path, files: &[GeneratedFile]) -> Result<()> {
    let root = workspace
        .canonicalize()
        .context("resolve agent workspace")?;
    for file in files {
        checked_file(&root, &file.relative_path)?;
    }
    Ok(())
}

/// Only exact templates from this build/configuration are managed. Unknown bytes
/// require manual review. All candidates are checked before any file is moved.
pub(super) fn retire_full_artifacts(
    workspace: &Path,
    full: Vec<GeneratedFile>,
    compact: Vec<GeneratedFile>,
    selected: &BTreeSet<PathBuf>,
    force: bool,
) -> Result<Transition> {
    let root = workspace
        .canonicalize()
        .context("resolve agent workspace")?;
    let mut pending = Vec::new();
    let mut refreshed = Vec::new();
    for file in full {
        // Shared registrations are merged by the caller, never archived wholesale.
        if selected.contains(&file.relative_path) || file.relative_path == Path::new(".mcp.json") {
            continue;
        }
        let path = root.join(&file.relative_path);
        let Some(metadata) = checked_file(&root, &file.relative_path)? else {
            continue;
        };
        let replacement = compact
            .iter()
            .find(|item| item.relative_path == file.relative_path);
        let limit = replacement.map_or(file.content.len(), |item| {
            item.content.len().max(file.content.len())
        });
        if metadata.len() > limit as u64 {
            bail!(
                "{} is customized or from another template/configuration; review it before selecting Compact",
                path.display()
            );
        }
        let bytes = read_bounded(&path, limit)?;
        if replacement.is_some_and(|item| bytes == item.content.as_bytes()) {
            continue;
        }
        if bytes != file.content.as_bytes() {
            bail!(
                "{} is customized or from another template/configuration; review it before selecting Compact",
                path.display()
            );
        }
        if let Some(item) = replacement {
            // A global profile switch also refreshes recognized guidance for other clients.
            refreshed.push(GeneratedFile::new(
                &item.relative_path,
                item.content.clone(),
            ));
        }
        pending.push((file.relative_path, bytes));
    }
    if pending.is_empty() {
        return Ok(Transition {
            archived: Vec::new(),
            refreshed,
        });
    }
    if !force {
        bail!(
            "Full-profile generated artifacts are still discoverable; re-run with --force to archive them before selecting Compact"
        );
    }

    let backup_parent = Path::new(".aether/agent-artifact-backups");
    checked_directories(&root, backup_parent)?;
    fs::create_dir_all(root.join(backup_parent))
        .context("create agent artifact backup directory")?;
    let backup = create_backup(&root, backup_parent)?;
    let mut archived = Vec::new();
    for (relative, bytes) in pending {
        checked_file(&root, &relative)?.context("artifact disappeared before archival")?;
        let source = root.join(&relative);
        if read_bounded(&source, bytes.len())? != bytes {
            bail!(
                "{} changed during profile transition; profile registration has not been changed",
                source.display()
            );
        }
        let archive_path = backup.join(&relative);
        let destination = root.join(&archive_path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).context("create artifact backup parent")?;
        }
        fs::rename(&source, &destination).with_context(|| {
            format!("archive {} to {}", source.display(), destination.display())
        })?;
        archived.push(ArchivedAgentArtifact {
            original_path: relative,
            archive_path,
        });
    }
    Ok(Transition {
        archived,
        refreshed,
    })
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .with_context(|| format!("open {}", path.display()))?
        .take((limit as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {}", path.display()))?;
    Ok(bytes)
}

fn checked_file(root: &Path, relative: &Path) -> Result<Option<fs::Metadata>> {
    if let Some(parent) = relative.parent() {
        checked_directories(root, parent)?;
    }
    let path = root.join(relative);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(Some(metadata)),
        Ok(_) => bail!(
            "{} is not a regular file; profile transition refused",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn checked_directories(root: &Path, relative: &Path) -> Result<()> {
    let mut path = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(part) = component else {
            bail!("invalid generated artifact path {}", relative.display());
        };
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => bail!(
                "{} is not a regular directory; profile transition refused",
                path.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error).with_context(|| format!("inspect {}", path.display())),
        }
    }
    Ok(())
}

fn create_backup(root: &Path, parent: &Path) -> Result<PathBuf> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("artifact backup clock")?
        .as_nanos();
    for _ in 0..8 {
        let relative = parent.join(format!(
            "{timestamp}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(root.join(&relative)) {
            Ok(()) => return Ok(relative),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("create exclusive artifact backup"),
        }
    }
    bail!("could not allocate an exclusive artifact backup directory")
}
