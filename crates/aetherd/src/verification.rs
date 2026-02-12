use std::path::Path;
use std::process::Command;

use aether_config::AetherConfig;
use anyhow::{Context, Result};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VerificationRequest {
    pub commands: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationCommandResult {
    pub command: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub passed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationRun {
    pub mode: String,
    pub allowlisted_commands: Vec<String>,
    pub requested_commands: Vec<String>,
    pub passed: bool,
    pub error: Option<String>,
    pub command_results: Vec<VerificationCommandResult>,
}

pub fn run_host_verification(
    workspace: &Path,
    config: &AetherConfig,
    request: VerificationRequest,
) -> Result<VerificationRun> {
    let allowlisted_commands = config.verify.commands.clone();
    let requested_commands = request
        .commands
        .map(normalize_requested_commands)
        .unwrap_or_else(|| allowlisted_commands.clone());

    if allowlisted_commands.is_empty() {
        return Ok(VerificationRun {
            mode: "host".to_owned(),
            allowlisted_commands,
            requested_commands,
            passed: false,
            error: Some("verify.commands is empty; no commands to run".to_owned()),
            command_results: Vec::new(),
        });
    }

    if requested_commands.is_empty() {
        return Ok(VerificationRun {
            mode: "host".to_owned(),
            allowlisted_commands,
            requested_commands,
            passed: false,
            error: Some("no verification commands selected".to_owned()),
            command_results: Vec::new(),
        });
    }

    if let Some(command) = requested_commands
        .iter()
        .find(|command| {
            !allowlisted_commands
                .iter()
                .any(|allowed| allowed == *command)
        })
        .cloned()
    {
        return Ok(VerificationRun {
            mode: "host".to_owned(),
            allowlisted_commands,
            requested_commands,
            passed: false,
            error: Some(format!("requested command is not allowlisted: {command}")),
            command_results: Vec::new(),
        });
    }

    let mut command_results = Vec::new();
    for command in &requested_commands {
        let output = run_command_on_host(workspace, command)?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let exit_code = output.status.code();
        let passed = exit_code == Some(0);

        command_results.push(VerificationCommandResult {
            command: command.clone(),
            exit_code,
            stdout,
            stderr,
            passed,
        });

        if !passed {
            break;
        }
    }

    let passed = !command_results.is_empty() && command_results.iter().all(|item| item.passed);
    Ok(VerificationRun {
        mode: "host".to_owned(),
        allowlisted_commands,
        requested_commands,
        passed,
        error: None,
        command_results,
    })
}

fn normalize_requested_commands(commands: Vec<String>) -> Vec<String> {
    commands
        .into_iter()
        .map(|command| command.trim().to_owned())
        .filter(|command| !command.is_empty())
        .collect()
}

fn run_command_on_host(workspace: &Path, command: &str) -> Result<std::process::Output> {
    let mut process = if cfg!(windows) {
        let mut process = Command::new("cmd");
        process.arg("/C");
        process
    } else {
        let mut process = Command::new("sh");
        process.arg("-lc");
        process
    };

    process
        .arg(command)
        .current_dir(workspace)
        .output()
        .with_context(|| format!("failed to execute verification command: {command}"))
}
