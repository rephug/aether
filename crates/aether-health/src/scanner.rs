use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use toml::Value;
use walkdir::WalkDir;

use crate::Result;
use crate::models::HealthError;

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceCrate {
    pub name: String,
    pub root: PathBuf,
    pub cargo_toml: Value,
    pub cargo_toml_raw: String,
    pub source_files: Vec<PathBuf>,
}

pub(crate) fn scan_workspace(root: &Path) -> Result<Vec<WorkspaceCrate>> {
    let workspace_root = root.canonicalize().map_err(|err| {
        HealthError::Message(format!(
            "failed to resolve workspace root {}: {err}",
            root.display()
        ))
    })?;
    let manifest_path = workspace_root.join("Cargo.toml");
    let (_, manifest) = read_toml(&manifest_path)?;
    let members = manifest
        .get("workspace")
        .and_then(Value::as_table)
        .and_then(|workspace| workspace.get("members"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            HealthError::Message(format!(
                "workspace manifest {} is missing [workspace].members",
                manifest_path.display()
            ))
        })?;

    let mut seen = HashSet::new();
    let mut crates = Vec::new();
    for member in members {
        let pattern = member.as_str().ok_or_else(|| {
            HealthError::Message(format!(
                "workspace member entry in {} must be a string",
                manifest_path.display()
            ))
        })?;
        let paths = expand_member_pattern(&workspace_root, pattern)?;
        if paths.is_empty() {
            return Err(HealthError::Message(format!(
                "workspace member pattern '{pattern}' did not match any paths"
            )));
        }

        for path in paths {
            if seen.insert(path.clone()) {
                crates.push(scan_crate(&path)?);
            }
        }
    }

    crates.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(crates)
}

pub(crate) fn scan_crate(root: &Path) -> Result<WorkspaceCrate> {
    let crate_root = root.canonicalize().map_err(|err| {
        HealthError::Message(format!(
            "failed to resolve crate root {}: {err}",
            root.display()
        ))
    })?;
    let manifest_path = crate_root.join("Cargo.toml");
    let (cargo_toml_raw, cargo_toml) = read_toml(&manifest_path)?;
    let name = cargo_toml
        .get("package")
        .and_then(Value::as_table)
        .and_then(|package| package.get("name"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            HealthError::Message(format!(
                "crate manifest {} is missing [package].name",
                manifest_path.display()
            ))
        })?
        .to_owned();

    let source_files = collect_source_files(&crate_root.join("src"))?;

    Ok(WorkspaceCrate {
        name,
        root: crate_root,
        cargo_toml,
        cargo_toml_raw,
        source_files,
    })
}

fn read_toml(path: &Path) -> Result<(String, Value)> {
    let raw = fs::read_to_string(path)
        .map_err(|err| HealthError::Message(format!("failed to read {}: {err}", path.display())))?;
    let parsed = toml::from_str(&raw).map_err(|err| {
        HealthError::Message(format!("failed to parse {}: {err}", path.display()))
    })?;
    Ok((raw, parsed))
}

fn collect_source_files(src_root: &Path) -> Result<Vec<PathBuf>> {
    if !src_root.exists() {
        return Ok(Vec::new());
    }

    let mut source_files = Vec::new();
    for entry in WalkDir::new(src_root) {
        let entry = entry.map_err(|err| {
            HealthError::Message(format!(
                "failed while walking {}: {err}",
                src_root.display()
            ))
        })?;
        if entry.file_type().is_file()
            && entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext == "rs")
        {
            source_files.push(entry.path().to_path_buf());
        }
    }

    source_files.sort();
    Ok(source_files)
}

fn expand_member_pattern(workspace_root: &Path, pattern: &str) -> Result<Vec<PathBuf>> {
    let mut segments = Vec::new();
    for component in Path::new(pattern).components() {
        match component {
            Component::Normal(value) => segments.push(value.to_string_lossy().to_string()),
            Component::CurDir => {}
            other => {
                return Err(HealthError::Message(format!(
                    "unsupported workspace member component '{other:?}' in pattern '{pattern}'"
                )));
            }
        }
    }

    let mut matches = Vec::new();
    expand_member_segments(workspace_root, &segments, 0, &mut matches)?;
    matches.sort();
    Ok(matches)
}

fn expand_member_segments(
    base: &Path,
    segments: &[String],
    index: usize,
    matches: &mut Vec<PathBuf>,
) -> Result<()> {
    if index == segments.len() {
        if base.exists() {
            matches.push(base.to_path_buf());
        }
        return Ok(());
    }

    let segment = &segments[index];
    if has_wildcards(segment) {
        let entries = fs::read_dir(base).map_err(|err| {
            HealthError::Message(format!(
                "failed to read directory {}: {err}",
                base.display()
            ))
        })?;
        for entry in entries {
            let entry = entry?;
            let file_name = entry.file_name();
            let candidate = file_name.to_string_lossy();
            if wildcard_matches(segment, &candidate) {
                expand_member_segments(&entry.path(), segments, index + 1, matches)?;
            }
        }
    } else {
        expand_member_segments(&base.join(segment), segments, index + 1, matches)?;
    }

    Ok(())
}

fn has_wildcards(segment: &str) -> bool {
    segment.contains('*') || segment.contains('?')
}

fn wildcard_matches(pattern: &str, candidate: &str) -> bool {
    let pattern_bytes = pattern.as_bytes();
    let candidate_bytes = candidate.as_bytes();
    let mut dp = vec![vec![false; candidate_bytes.len() + 1]; pattern_bytes.len() + 1];
    dp[0][0] = true;

    for pattern_index in 0..pattern_bytes.len() {
        let byte = pattern_bytes[pattern_index];
        for candidate_index in 0..=candidate_bytes.len() {
            if !dp[pattern_index][candidate_index] {
                continue;
            }
            match byte {
                b'*' => {
                    let mut next_index = candidate_index;
                    while next_index <= candidate_bytes.len() {
                        dp[pattern_index + 1][next_index] = true;
                        next_index += 1;
                    }
                }
                b'?' => {
                    if candidate_index < candidate_bytes.len() {
                        dp[pattern_index + 1][candidate_index + 1] = true;
                    }
                }
                _ => {
                    if candidate_index < candidate_bytes.len()
                        && byte == candidate_bytes[candidate_index]
                    {
                        dp[pattern_index + 1][candidate_index + 1] = true;
                    }
                }
            }
        }
    }

    dp[pattern_bytes.len()][candidate_bytes.len()]
}
