use std::path::{Component, Path};

use gix::bstr::ByteSlice;
use std::process::Command;
use thiserror::Error;

pub struct GitContext {
    repo: gix::Repository,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub hash: String,
    pub author: String,
    pub message: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    pub line_number: u32,
    pub commit_hash: String,
    pub author: String,
}

#[derive(Debug, Error)]
pub enum GitHistoryError {
    #[error("history requires a worktree and a path within it")]
    InvalidPath,
    #[error("failed to run git history: {0}")]
    Io(#[from] std::io::Error),
    #[error("git history failed: {0}")]
    Command(String),
    #[error("invalid git history record: {0}")]
    InvalidRecord(String),
}

impl GitContext {
    pub fn open(workspace: &Path) -> Option<Self> {
        gix::discover(workspace).ok().map(|repo| Self { repo })
    }

    pub fn head_commit_hash(&self) -> Option<String> {
        let head_id = self.repo.head_id().ok()?.detach();
        Some(head_id.to_string().to_ascii_lowercase())
    }

    /// Compatibility wrapper; callers that need truthful degradation should use
    /// `file_history`, which distinguishes unavailable history from an empty log.
    pub fn file_log(&self, path: &Path, limit: usize) -> Vec<CommitInfo> {
        self.file_history(path, limit).unwrap_or_default()
    }

    /// Path history, including commits that removed code and rename predecessors.
    /// Messages are cited recorded text; their presence does not prove causation.
    pub fn file_history(
        &self,
        path: &Path,
        limit: usize,
    ) -> Result<Vec<CommitInfo>, GitHistoryError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let workdir = self.repo.workdir().ok_or(GitHistoryError::InvalidPath)?;
        let git_path = self
            .repo_relative_git_path(path)
            .ok_or(GitHistoryError::InvalidPath)?;
        let output = Command::new("git")
            .current_dir(workdir)
            .args([
                "--no-pager",
                "--literal-pathspecs",
                "log",
                "--follow",
                "--format=%H%x00%an%x00%at%x00%B%x00",
            ])
            .arg(format!("-{}", limit.min(1000)))
            .args(["HEAD", "--"])
            .arg(git_path)
            .output()?;
        if !output.status.success() {
            return Err(GitHistoryError::Command(decode_text(&output.stderr)));
        }
        let mut fields = output.stdout.split(|byte| *byte == 0);
        let mut commits = Vec::new();
        while let Some(hash) = fields.next() {
            let hash = decode_text(hash);
            if hash.is_empty() {
                continue;
            }
            let author = fields
                .next()
                .ok_or_else(|| GitHistoryError::InvalidRecord("missing author".into()))?;
            let timestamp = fields
                .next()
                .ok_or_else(|| GitHistoryError::InvalidRecord("missing timestamp".into()))?;
            let message = fields
                .next()
                .ok_or_else(|| GitHistoryError::InvalidRecord("missing message".into()))?;
            let timestamp = decode_text(timestamp)
                .parse::<i64>()
                .map_err(|error| GitHistoryError::InvalidRecord(format!("timestamp: {error}")))?;
            commits.push(CommitInfo {
                hash,
                author: decode_text(author),
                message: decode_text(message),
                timestamp,
            });
        }
        Ok(commits)
    }

    pub fn blame_lines(&self, path: &Path) -> Vec<BlameLine> {
        let Some(head_id) = self.repo.head_id().ok().map(|id| id.detach()) else {
            return Vec::new();
        };

        let Some(git_path) = self.repo_relative_git_path(path) else {
            return Vec::new();
        };

        let outcome = self
            .repo
            .blame_file(
                git_path.as_bytes().as_bstr(),
                head_id,
                gix::repository::blame_file::Options::default(),
            )
            .ok();
        let Some(outcome) = outcome else {
            return Vec::new();
        };

        let estimated_lines = outcome
            .entries
            .iter()
            .map(|entry| entry.len.get() as usize)
            .sum();
        let mut lines = Vec::with_capacity(estimated_lines);

        for entry in outcome.entries {
            let hash = entry.commit_id.to_string().to_ascii_lowercase();
            let author = self.commit_author(entry.commit_id).unwrap_or_default();
            let start_line = entry.start_in_blamed_file + 1;

            for offset in 0..entry.len.get() {
                lines.push(BlameLine {
                    line_number: start_line + offset,
                    commit_hash: hash.clone(),
                    author: author.clone(),
                });
            }
        }

        lines
    }

    fn commit_author(&self, id: gix::ObjectId) -> Option<String> {
        let commit = self.repo.find_commit(id).ok()?;
        let signature = commit.author().ok()?;
        Some(decode_text(signature.name.as_ref()))
    }

    fn repo_relative_git_path(&self, path: &Path) -> Option<String> {
        let relative_path = match self.repo.workdir() {
            Some(workdir) => {
                let absolute = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    workdir.join(path)
                };
                absolute.strip_prefix(workdir).ok()?.to_path_buf()
            }
            None => {
                if path.is_absolute() {
                    return None;
                }
                path.to_path_buf()
            }
        };

        normalize_git_path(&relative_path)
    }
}

fn normalize_git_path(path: &Path) -> Option<String> {
    let mut segments = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => segments.push(segment.to_str()?.to_owned()),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    if segments.is_empty() {
        None
    } else {
        Some(segments.join("/"))
    }
}

fn decode_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(root)
            .args([
                "-c",
                "user.name=History fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args(args)
            .output()
            .expect("git fixture command");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    #[test]
    fn path_history_retains_removed_guards_full_reasons_and_rename_predecessors() {
        let temp = tempdir().expect("temp dir");
        let root = temp.path();
        git(root, &["init", "-q"]);
        fs::write(root.join("old.rs"), "fn run() { guard(); work(); }\n").expect("initial source");
        git(root, &["add", "old.rs"]);
        git(
            root,
            &[
                "commit",
                "-q",
                "-m",
                "Add guard",
                "-m",
                "Recorded reason: reject stale writes before updating dependencies.",
            ],
        );
        let initial = git(root, &["rev-parse", "HEAD"]);
        git(root, &["mv", "old.rs", "new.rs"]);
        git(root, &["commit", "-q", "-m", "Move module"]);
        fs::write(root.join("new.rs"), "fn run() { work(); }\n").expect("remove guard");
        git(
            root,
            &[
                "commit",
                "-qam",
                "Remove guard",
                "-m",
                "Recorded reason: guard now lives at the transaction boundary.",
            ],
        );
        let removal = git(root, &["rev-parse", "HEAD"]);
        let context = GitContext::open(root).expect("git context");
        let history = context
            .file_history(Path::new("new.rs"), 10)
            .expect("path history");
        assert_eq!(history.len(), 3);
        assert!(
            history
                .iter()
                .any(|commit| commit.hash == initial
                    && commit.message.contains("reject stale writes"))
        );
        assert_eq!(history[0].hash, removal);
        assert!(history[0].message.contains("transaction boundary"));
        git(root, &["rm", "new.rs"]);
        git(root, &["commit", "-q", "-m", "Delete module"]);
        assert_eq!(
            context
                .file_history(Path::new("new.rs"), 10)
                .expect("deleted path history")
                .len(),
            4
        );
    }

    #[test]
    fn history_is_literal_bounded_and_rejects_paths_outside_worktree() {
        let temp = tempdir().expect("temp dir");
        let root = temp.path();
        git(root, &["init", "-q"]);
        fs::write(root.join("file.rs"), "fn run() {}\n").expect("source");
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "Initial"]);
        let context = GitContext::open(root).expect("git context");
        assert!(
            context
                .file_history(Path::new(":(glob)*"), 10)
                .expect("literal path")
                .is_empty()
        );
        assert!(context.file_history(Path::new("../outside"), 10).is_err());
        assert!(
            context
                .file_history(Path::new("file.rs"), 0)
                .expect("zero limit")
                .is_empty()
        );
        assert_eq!(
            context
                .file_history(Path::new("file.rs"), 1)
                .expect("bounded history")
                .len(),
            1
        );
    }
}
