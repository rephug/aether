use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use aether_config::{AetherConfig, VerifyMode};
use anyhow::{Context, Result};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VerificationRequest {
    pub commands: Option<Vec<String>>,
    pub mode: Option<VerifyMode>,
    pub fallback_to_host_on_unavailable: Option<bool>,
    pub fallback_to_container_on_unavailable: Option<bool>,
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
    pub mode_requested: String,
    pub mode_used: String,
    pub fallback_reason: Option<String>,
    pub allowlisted_commands: Vec<String>,
    pub requested_commands: Vec<String>,
    pub passed: bool,
    pub error: Option<String>,
    pub command_results: Vec<VerificationCommandResult>,
}

struct ExecutionOutcome {
    run: VerificationRun,
    runtime_unavailable: bool,
}

enum CommandExecutionError {
    RuntimeUnavailable(String),
    Launch(anyhow::Error),
}

pub fn run_verification(
    workspace: &Path,
    config: &AetherConfig,
    request: VerificationRequest,
) -> Result<VerificationRun> {
    let requested_mode = request.mode.unwrap_or(config.verify.mode);

    match requested_mode {
        VerifyMode::Host => {
            run_host_verification_with_metadata(workspace, config, &request, VerifyMode::Host, None)
                .map(|outcome| outcome.run)
        }
        VerifyMode::Container => {
            let fallback_enabled = request
                .fallback_to_host_on_unavailable
                .unwrap_or(config.verify.container.fallback_to_host_on_unavailable);
            let container_outcome = run_container_verification(workspace, config, &request)?;
            if container_outcome.runtime_unavailable && fallback_enabled {
                let fallback_reason = container_outcome.run.error.clone().unwrap_or_else(|| {
                    "container runtime unavailable; fell back to host".to_owned()
                });
                return run_host_verification_with_metadata(
                    workspace,
                    config,
                    &request,
                    VerifyMode::Container,
                    Some(fallback_reason),
                )
                .map(|outcome| outcome.run);
            }
            Ok(container_outcome.run)
        }
        VerifyMode::Microvm => {
            let microvm_outcome = run_microvm_verification(workspace, config, &request)?;
            if !microvm_outcome.runtime_unavailable {
                return Ok(microvm_outcome.run);
            }

            let microvm_reason = microvm_outcome.run.error.clone().unwrap_or_else(|| {
                "microvm runtime unavailable; use verify.mode=host or verify.mode=container"
                    .to_owned()
            });
            let fallback_to_container = request
                .fallback_to_container_on_unavailable
                .unwrap_or(config.verify.microvm.fallback_to_container_on_unavailable);
            let fallback_to_host = request
                .fallback_to_host_on_unavailable
                .unwrap_or(config.verify.microvm.fallback_to_host_on_unavailable);

            if fallback_to_container {
                let container_outcome = run_container_verification_with_metadata(
                    workspace,
                    config,
                    &request,
                    VerifyMode::Microvm,
                    Some(microvm_reason.clone()),
                )?;

                if !container_outcome.runtime_unavailable {
                    return Ok(container_outcome.run);
                }

                if fallback_to_host {
                    let container_reason =
                        container_outcome.run.error.clone().unwrap_or_else(|| {
                            "container runtime unavailable after microvm fallback".to_owned()
                        });
                    let fallback_reason =
                        format!("{microvm_reason}; then {container_reason}; fell back to host");
                    return run_host_verification_with_metadata(
                        workspace,
                        config,
                        &request,
                        VerifyMode::Microvm,
                        Some(fallback_reason),
                    )
                    .map(|outcome| outcome.run);
                }

                return Ok(container_outcome.run);
            }

            if fallback_to_host {
                return run_host_verification_with_metadata(
                    workspace,
                    config,
                    &request,
                    VerifyMode::Microvm,
                    Some(microvm_reason),
                )
                .map(|outcome| outcome.run);
            }

            Ok(microvm_outcome.run)
        }
    }
}

pub fn run_host_verification(
    workspace: &Path,
    config: &AetherConfig,
    request: VerificationRequest,
) -> Result<VerificationRun> {
    run_host_verification_with_metadata(workspace, config, &request, VerifyMode::Host, None)
        .map(|outcome| outcome.run)
}

fn normalize_requested_commands(commands: Vec<String>) -> Vec<String> {
    commands
        .into_iter()
        .map(|command| command.trim().to_owned())
        .filter(|command| !command.is_empty())
        .collect()
}

fn run_host_verification_with_metadata(
    workspace: &Path,
    config: &AetherConfig,
    request: &VerificationRequest,
    mode_requested: VerifyMode,
    fallback_reason: Option<String>,
) -> Result<ExecutionOutcome> {
    run_verification_with_executor(
        workspace,
        config,
        request,
        mode_requested,
        VerifyMode::Host,
        fallback_reason,
        |workspace, _config, command| {
            run_command_on_host(workspace, command).map_err(CommandExecutionError::Launch)
        },
    )
}

fn run_container_verification(
    workspace: &Path,
    config: &AetherConfig,
    request: &VerificationRequest,
) -> Result<ExecutionOutcome> {
    run_container_verification_with_metadata(
        workspace,
        config,
        request,
        VerifyMode::Container,
        None,
    )
}

fn run_container_verification_with_metadata(
    workspace: &Path,
    config: &AetherConfig,
    request: &VerificationRequest,
    mode_requested: VerifyMode,
    fallback_reason: Option<String>,
) -> Result<ExecutionOutcome> {
    run_verification_with_executor(
        workspace,
        config,
        request,
        mode_requested,
        VerifyMode::Container,
        fallback_reason,
        run_command_in_container,
    )
}

fn run_microvm_verification(
    workspace: &Path,
    config: &AetherConfig,
    request: &VerificationRequest,
) -> Result<ExecutionOutcome> {
    run_verification_with_executor(
        workspace,
        config,
        request,
        VerifyMode::Microvm,
        VerifyMode::Microvm,
        None,
        run_command_in_microvm,
    )
}

fn run_verification_with_executor(
    workspace: &Path,
    config: &AetherConfig,
    request: &VerificationRequest,
    mode_requested: VerifyMode,
    mode_used: VerifyMode,
    fallback_reason: Option<String>,
    mut execute: impl FnMut(
        &Path,
        &AetherConfig,
        &str,
    ) -> std::result::Result<Output, CommandExecutionError>,
) -> Result<ExecutionOutcome> {
    let allowlisted_commands = config.verify.commands.clone();
    let requested_commands = request
        .commands
        .clone()
        .map(normalize_requested_commands)
        .unwrap_or_else(|| allowlisted_commands.clone());

    let mode_requested_label = mode_requested.as_str().to_owned();
    let mode_used_label = mode_used.as_str().to_owned();

    if allowlisted_commands.is_empty() {
        return Ok(ExecutionOutcome {
            run: failed_run(
                &mode_requested_label,
                &mode_used_label,
                fallback_reason,
                allowlisted_commands,
                requested_commands,
                "verify.commands is empty; no commands to run".to_owned(),
                Vec::new(),
            ),
            runtime_unavailable: false,
        });
    }

    if requested_commands.is_empty() {
        return Ok(ExecutionOutcome {
            run: failed_run(
                &mode_requested_label,
                &mode_used_label,
                fallback_reason,
                allowlisted_commands,
                requested_commands,
                "no verification commands selected".to_owned(),
                Vec::new(),
            ),
            runtime_unavailable: false,
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
        return Ok(ExecutionOutcome {
            run: failed_run(
                &mode_requested_label,
                &mode_used_label,
                fallback_reason,
                allowlisted_commands,
                requested_commands,
                format!("requested command is not allowlisted: {command}"),
                Vec::new(),
            ),
            runtime_unavailable: false,
        });
    }

    let mut command_results = Vec::new();
    for command in &requested_commands {
        let output = match execute(workspace, config, command) {
            Ok(output) => output,
            Err(CommandExecutionError::RuntimeUnavailable(message)) => {
                return Ok(ExecutionOutcome {
                    run: failed_run(
                        &mode_requested_label,
                        &mode_used_label,
                        fallback_reason,
                        allowlisted_commands,
                        requested_commands,
                        message,
                        command_results,
                    ),
                    runtime_unavailable: true,
                });
            }
            Err(CommandExecutionError::Launch(err)) => {
                return Ok(ExecutionOutcome {
                    run: failed_run(
                        &mode_requested_label,
                        &mode_used_label,
                        fallback_reason,
                        allowlisted_commands,
                        requested_commands,
                        err.to_string(),
                        command_results,
                    ),
                    runtime_unavailable: false,
                });
            }
        };

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
    Ok(ExecutionOutcome {
        run: VerificationRun {
            mode: mode_used_label.clone(),
            mode_requested: mode_requested_label,
            mode_used: mode_used_label,
            fallback_reason,
            allowlisted_commands,
            requested_commands,
            passed,
            error: None,
            command_results,
        },
        runtime_unavailable: false,
    })
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

fn run_command_in_container(
    workspace: &Path,
    config: &AetherConfig,
    command: &str,
) -> std::result::Result<std::process::Output, CommandExecutionError> {
    let runtime = config.verify.container.runtime.trim();
    let image = config.verify.container.image.trim();
    let workdir = config.verify.container.workdir.trim();
    let mount_arg = format!("{}:{workdir}", workspace.to_string_lossy());

    let mut process = Command::new(runtime);
    process
        .arg("run")
        .arg("--rm")
        .arg("-w")
        .arg(workdir)
        .arg("-v")
        .arg(mount_arg)
        .arg(image)
        .arg("sh")
        .arg("-lc")
        .arg(command);

    process.output().map_err(|err| {
        if matches!(
            err.kind(),
            ErrorKind::NotFound | ErrorKind::PermissionDenied
        ) {
            CommandExecutionError::RuntimeUnavailable(format!(
                "container runtime unavailable: failed to execute '{runtime}': {err}"
            ))
        } else {
            CommandExecutionError::Launch(anyhow::Error::new(err).context(format!(
                "failed to execute container verification command: {command}"
            )))
        }
    })
}

fn run_command_in_microvm(
    workspace: &Path,
    config: &AetherConfig,
    command: &str,
) -> std::result::Result<std::process::Output, CommandExecutionError> {
    if !cfg!(target_os = "linux") {
        return Err(CommandExecutionError::RuntimeUnavailable(
            "microvm mode is unsupported on this host; currently only Linux hosts are supported. Use verify.mode=host or verify.mode=container".to_owned(),
        ));
    }

    let runtime = config.verify.microvm.runtime.trim();
    let workdir = config.verify.microvm.workdir.trim();
    let Some(kernel_image) = config.verify.microvm.kernel_image.as_deref() else {
        return Err(CommandExecutionError::Launch(anyhow::anyhow!(
            "microvm kernel_image is required; set verify.microvm.kernel_image or use host/container mode"
        )));
    };
    let Some(rootfs_image) = config.verify.microvm.rootfs_image.as_deref() else {
        return Err(CommandExecutionError::Launch(anyhow::anyhow!(
            "microvm rootfs_image is required; set verify.microvm.rootfs_image or use host/container mode"
        )));
    };
    let kernel_path = resolve_microvm_asset_path(workspace, kernel_image);
    let rootfs_path = resolve_microvm_asset_path(workspace, rootfs_image);

    if !kernel_path.exists() {
        return Err(CommandExecutionError::Launch(anyhow::anyhow!(
            "microvm kernel image not found at '{}'",
            kernel_path.display()
        )));
    }
    if !rootfs_path.exists() {
        return Err(CommandExecutionError::Launch(anyhow::anyhow!(
            "microvm rootfs image not found at '{}'",
            rootfs_path.display()
        )));
    }

    let mut process = Command::new(runtime);
    process
        .arg("--workspace")
        .arg(workspace.to_string_lossy().to_string())
        .arg("--workdir")
        .arg(workdir)
        .arg("--kernel")
        .arg(kernel_path.to_string_lossy().to_string())
        .arg("--rootfs")
        .arg(rootfs_path.to_string_lossy().to_string())
        .arg("--vcpu-count")
        .arg(config.verify.microvm.vcpu_count.to_string())
        .arg("--memory-mib")
        .arg(config.verify.microvm.memory_mib.to_string())
        .arg("--command")
        .arg(command);

    process.output().map_err(|err| {
        if matches!(
            err.kind(),
            ErrorKind::NotFound | ErrorKind::PermissionDenied
        ) {
            CommandExecutionError::RuntimeUnavailable(format!(
                "microvm runtime unavailable: failed to execute '{runtime}': {err}"
            ))
        } else {
            CommandExecutionError::Launch(anyhow::Error::new(err).context(format!(
                "failed to execute microvm verification command: {command}"
            )))
        }
    })
}

fn resolve_microvm_asset_path(workspace: &Path, asset: &str) -> PathBuf {
    let path = Path::new(asset);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    }
}

fn failed_run(
    mode_requested: &str,
    mode_used: &str,
    fallback_reason: Option<String>,
    allowlisted_commands: Vec<String>,
    requested_commands: Vec<String>,
    error: String,
    command_results: Vec<VerificationCommandResult>,
) -> VerificationRun {
    VerificationRun {
        mode: mode_used.to_owned(),
        mode_requested: mode_requested.to_owned(),
        mode_used: mode_used.to_owned(),
        fallback_reason,
        allowlisted_commands,
        requested_commands,
        passed: false,
        error: Some(error),
        command_results,
    }
}
