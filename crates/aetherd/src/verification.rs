use std::collections::VecDeque;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

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
        |workspace, config, command| {
            run_command_on_host(workspace, config, command).map_err(CommandExecutionError::Launch)
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
    ) -> std::result::Result<CommandOutput, CommandExecutionError>,
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

        let exit_code = output.exit_code;
        let passed = exit_code == Some(0) && !output.timed_out;

        command_results.push(VerificationCommandResult {
            command: command.clone(),
            exit_code,
            stdout: output.stdout,
            stderr: output.stderr,
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

fn run_command_on_host(
    workspace: &Path,
    config: &AetherConfig,
    command: &str,
) -> Result<CommandOutput> {
    let mut process = if cfg!(windows) {
        let mut process = Command::new("cmd");
        process.arg("/C");
        process
    } else {
        let mut process = Command::new("sh");
        process.arg("-lc");
        process
    };

    process.arg(command).current_dir(workspace);
    run_bounded(process, CommandLimits::from_config(config))
        .with_context(|| format!("failed to execute verification command: {command}"))
}

fn run_command_in_container(
    workspace: &Path,
    config: &AetherConfig,
    command: &str,
) -> std::result::Result<CommandOutput, CommandExecutionError> {
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

    run_bounded(process, CommandLimits::from_config(config)).map_err(|err| {
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
) -> std::result::Result<CommandOutput, CommandExecutionError> {
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

    run_bounded(process, CommandLimits::from_config(config)).map_err(|err| {
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

/// What one verification command produced, within [`CommandLimits`].
struct CommandOutput {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

/// How long a verification command may run and how much of each output stream is kept.
#[derive(Debug, Clone, Copy)]
struct CommandLimits {
    timeout: Duration,
    max_output_bytes: usize,
}

impl CommandLimits {
    fn from_config(config: &AetherConfig) -> Self {
        Self {
            timeout: Duration::from_secs(config.verify.timeout_secs.max(1)),
            max_output_bytes: config.verify.max_output_bytes.max(1),
        }
    }
}

/// How long to wait for the output readers once the process has ended. A background
/// process the command started can keep a pipe open after the command itself exits.
const OUTPUT_DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Runs `process` with its output captured within `limits.max_output_bytes` per stream
/// and stops it, with any processes it started (on Unix), once `limits.timeout` passes.
fn run_bounded(mut process: Command, limits: CommandLimits) -> std::io::Result<CommandOutput> {
    process
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so a timeout can stop everything the command started.
        process.process_group(0);
    }
    let mut child = process.spawn()?;
    let mut stdout = CappedReader::spawn(child.stdout.take(), limits.max_output_bytes);
    let mut stderr = CappedReader::spawn(child.stderr.take(), limits.max_output_bytes);

    // A timeout too large to represent is no deadline at all.
    let deadline = Instant::now().checked_add(limits.timeout);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(err) => {
                stop_process_tree(&mut child);
                return Err(err);
            }
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            stop_process_tree(&mut child);
            break None;
        }
        thread::sleep(Duration::from_millis(20));
    };

    // One grace period for both streams. If something the command started still holds
    // a pipe once it runs out, stop it as well rather than leave it running.
    let drain_deadline = Instant::now() + OUTPUT_DRAIN_GRACE;
    let stdout_drained = stdout.wait_until(drain_deadline);
    let stderr_drained = stderr.wait_until(drain_deadline);
    if !(stdout_drained && stderr_drained) {
        #[cfg(unix)]
        stop_process_group(child.id());
        let settle = Instant::now() + Duration::from_millis(200);
        stdout.wait_until(settle);
        stderr.wait_until(settle);
    }
    let stdout = stdout.text();
    let mut stderr = stderr.text();
    if status.is_none() {
        stderr.push_str(&format!(
            "\n[verification command stopped after {} s timeout]",
            limits.timeout.as_secs()
        ));
    }
    Ok(CommandOutput {
        exit_code: status.and_then(|status| status.code()),
        stdout,
        stderr,
        timed_out: status.is_none(),
    })
}

fn stop_process_tree(child: &mut Child) {
    // The group goes first, while the unreaped child still holds the group id.
    #[cfg(unix)]
    stop_process_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

/// Kills the process group led by `leader` (every command runs in its own group, see
/// `run_bounded`).
#[cfg(unix)]
fn stop_process_group(leader: u32) {
    let _ = Command::new("kill")
        .args(["-s", "KILL", "--"])
        .arg(format!("-{leader}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Reads a child's output stream on its own thread, keeping the first and last
/// `max_bytes / 2` bytes and discarding the middle, so the child never blocks on a
/// full pipe.
struct CappedReader {
    captured: Arc<Mutex<CappedBytes>>,
    done: mpsc::Receiver<()>,
    finished: bool,
}

#[derive(Default)]
struct CappedBytes {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total: usize,
}

impl CappedReader {
    fn spawn(stream: Option<impl Read + Send + 'static>, max_bytes: usize) -> Self {
        let captured = Arc::new(Mutex::new(CappedBytes::default()));
        let (done_tx, done) = mpsc::channel();
        if let Some(mut stream) = stream {
            let captured = Arc::clone(&captured);
            let head_limit = max_bytes.div_ceil(2);
            let tail_limit = max_bytes - head_limit;
            thread::spawn(move || {
                let mut buffer = [0u8; 8192];
                loop {
                    let read = match stream.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => read,
                    };
                    let Ok(mut bytes) = captured.lock() else {
                        break;
                    };
                    bytes.total += read;
                    let chunk = &buffer[..read];
                    let head_room = head_limit.saturating_sub(bytes.head.len()).min(read);
                    let (to_head, rest) = chunk.split_at(head_room);
                    bytes.head.extend_from_slice(to_head);
                    let rest = &rest[rest.len().saturating_sub(tail_limit)..];
                    bytes.tail.extend(rest.iter().copied());
                    let excess = bytes.tail.len().saturating_sub(tail_limit);
                    bytes.tail.drain(..excess);
                }
                let _ = done_tx.send(());
            });
        } else {
            let _ = done_tx.send(());
        }
        Self {
            captured,
            done,
            finished: false,
        }
    }

    /// Waits until the stream ends or `deadline` passes; true when it ended.
    fn wait_until(&mut self, deadline: Instant) -> bool {
        if !self.finished {
            let wait = deadline.saturating_duration_since(Instant::now());
            self.finished = self.done.recv_timeout(wait).is_ok();
        }
        self.finished
    }

    /// The text captured so far.
    fn text(self) -> String {
        let Ok(bytes) = self.captured.lock() else {
            return String::new();
        };
        let mut kept = bytes.head.clone();
        kept.extend(bytes.tail.iter().copied());
        if bytes.total == kept.len() {
            // Nothing omitted: decode as one piece so no character is split.
            return String::from_utf8_lossy(&kept).into_owned();
        }
        let mut text = String::from_utf8_lossy(&bytes.head).into_owned();
        text.push_str(&format!(
            "\n[... {} bytes of output omitted ...]\n",
            bytes.total - kept.len()
        ));
        let tail = bytes.tail.iter().copied().collect::<Vec<_>>();
        text.push_str(&String::from_utf8_lossy(&tail));
        text
    }
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
