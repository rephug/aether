use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::Command;

use aether_core::GitContext;
use tempfile::tempdir;

#[test]
fn head_commit_hash_matches_expected_sha_in_git_repo() -> Result<(), Box<dyn Error>> {
    let temp = tempdir()?;
    let workspace = temp.path();

    fs::create_dir_all(workspace.join("src"))?;
    fs::write(workspace.join("src/lib.rs"), "fn alpha() {}\n")?;

    init_git_repo(workspace)?;
    let expected = commit_all(workspace, "initial commit")?.to_ascii_lowercase();

    let context = GitContext::open(workspace).ok_or("expected git context")?;
    let actual = context
        .head_commit_hash()
        .ok_or("expected head commit hash")?;

    assert_eq!(actual, expected);
    Ok(())
}

#[test]
fn non_git_workspace_returns_none() -> Result<(), Box<dyn Error>> {
    let temp = tempdir()?;
    assert!(GitContext::open(temp.path()).is_none());
    Ok(())
}

#[test]
fn blame_lines_returns_correct_line_attribution() -> Result<(), Box<dyn Error>> {
    let temp = tempdir()?;
    let workspace = temp.path();

    fs::create_dir_all(workspace.join("src"))?;
    init_git_repo(workspace)?;

    let tracked_file = workspace.join("src/lib.rs");
    fs::write(&tracked_file, "line 1\nline 2\nline 3\n")?;
    let first_commit =
        commit_all_as(workspace, "initial", "Alice", "alice@example.com")?.to_ascii_lowercase();

    fs::write(&tracked_file, "line 1\nline two changed\nline 3\n")?;
    let second_commit = commit_all_as(workspace, "change second line", "Bob", "bob@example.com")?
        .to_ascii_lowercase();

    let context = GitContext::open(workspace).ok_or("expected git context")?;
    let lines = context.blame_lines(Path::new("src/lib.rs"));

    assert_eq!(lines.len(), 3);

    let by_line: BTreeMap<u32, _> = lines
        .into_iter()
        .map(|line| (line.line_number, line))
        .collect();

    assert_eq!(
        by_line.get(&1).map(|line| line.commit_hash.as_str()),
        Some(first_commit.as_str())
    );
    assert_eq!(
        by_line.get(&2).map(|line| line.commit_hash.as_str()),
        Some(second_commit.as_str())
    );
    assert_eq!(
        by_line.get(&3).map(|line| line.commit_hash.as_str()),
        Some(first_commit.as_str())
    );

    assert_eq!(
        by_line.get(&1).map(|line| line.author.as_str()),
        Some("Alice")
    );
    assert_eq!(
        by_line.get(&2).map(|line| line.author.as_str()),
        Some("Bob")
    );
    assert_eq!(
        by_line.get(&3).map(|line| line.author.as_str()),
        Some("Alice")
    );

    Ok(())
}

fn run_git(workspace: &Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git {:?} failed: {}", args, stderr.trim()).into());
    }

    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn init_git_repo(workspace: &Path) -> Result<(), Box<dyn Error>> {
    run_git(workspace, &["init"])?;
    run_git(workspace, &["config", "user.name", "Aether Test"])?;
    run_git(
        workspace,
        &["config", "user.email", "aether-test@example.com"],
    )?;
    Ok(())
}

fn commit_all(workspace: &Path, message: &str) -> Result<String, Box<dyn Error>> {
    run_git(workspace, &["add", "."])?;
    run_git(workspace, &["commit", "-m", message])?;
    run_git(workspace, &["rev-parse", "--verify", "HEAD"])
}

fn commit_all_as(
    workspace: &Path,
    message: &str,
    author_name: &str,
    author_email: &str,
) -> Result<String, Box<dyn Error>> {
    run_git(workspace, &["add", "."])?;

    let output = Command::new("git")
        .arg("-c")
        .arg(format!("user.name={author_name}"))
        .arg("-c")
        .arg(format!("user.email={author_email}"))
        .args(["commit", "-m", message])
        .current_dir(workspace)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git commit failed: {}", stderr.trim()).into());
    }

    run_git(workspace, &["rev-parse", "--verify", "HEAD"])
}
