use std::collections::HashSet;
use std::path::{Component, Path};

use gix::bstr::ByteSlice;

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

impl GitContext {
    pub fn open(workspace: &Path) -> Option<Self> {
        gix::discover(workspace).ok().map(|repo| Self { repo })
    }

    pub fn head_commit_hash(&self) -> Option<String> {
        let head_id = self.repo.head_id().ok()?.detach();
        Some(head_id.to_string().to_ascii_lowercase())
    }

    pub fn file_log(&self, path: &Path, limit: usize) -> Vec<CommitInfo> {
        if limit == 0 {
            return Vec::new();
        }

        let Some(head_id) = self.repo.head_id().ok().map(|id| id.detach()) else {
            return Vec::new();
        };

        let relevant_commits: HashSet<String> = self
            .blame_lines(path)
            .into_iter()
            .map(|line| line.commit_hash)
            .collect();

        if relevant_commits.is_empty() {
            return Vec::new();
        }

        let walk = self
            .repo
            .rev_walk([head_id])
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
            ))
            .all();
        let Ok(walk) = walk else {
            return Vec::new();
        };

        let mut commits = Vec::with_capacity(limit);
        for entry in walk {
            let Ok(info) = entry else {
                continue;
            };

            let hash = info.id.to_string().to_ascii_lowercase();
            if !relevant_commits.contains(&hash) {
                continue;
            }

            if let Some(commit) = self.commit_info(info.id) {
                commits.push(commit);
                if commits.len() >= limit {
                    break;
                }
            }
        }

        commits
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

    fn commit_info(&self, id: gix::ObjectId) -> Option<CommitInfo> {
        let commit = self.repo.find_commit(id).ok()?;

        let author = commit
            .author()
            .ok()
            .map(|signature| decode_text(signature.name.as_ref()))
            .unwrap_or_default();

        let message = first_line(commit.message_raw_sloppy().as_ref());

        let timestamp = commit.time().ok().map(|time| time.seconds).unwrap_or(0);

        Some(CommitInfo {
            hash: id.to_string().to_ascii_lowercase(),
            author,
            message,
            timestamp,
        })
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

fn first_line(bytes: &[u8]) -> String {
    let line = bytes
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    decode_text(line)
}
