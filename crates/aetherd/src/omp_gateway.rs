//! Bring up (and tear down) the Oh My Pi auth broker + gateway pair that
//! `inference.provider = "omp"` talks to.
//!
//! After `omp` login the credentials live in `~/.omp/agent`. Two omp processes turn them
//! into an OpenAI-compatible endpoint: `omp auth-broker serve` serves the credentials and
//! `omp auth-gateway serve` fronts the broker. `aetherd omp up` starts both (detached, logs
//! under `.aether/omp/`), `aetherd omp down` stops what `up` started, `aetherd omp status`
//! reports health, and `[inference.omp].autostart` runs `up` on demand before indexing.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use aether_config::{
    AetherConfig, DEFAULT_OMP_GATEWAY_TOKEN_ENV, InferenceProviderKind, OMP_GATEWAY_TOKEN_FILE,
    OmpConfig,
};
use anyhow::{Context, Result, anyhow, bail};

use crate::cli::{OmpArgs, OmpCommand};

const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Which of the two processes a pid file / log file belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Service {
    Broker,
    Gateway,
}

impl Service {
    fn name(self) -> &'static str {
        match self {
            Self::Broker => "broker",
            Self::Gateway => "gateway",
        }
    }
}

/// Health of the pair, as reported by their unauthenticated health endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmpGatewayStatus {
    pub broker_url: String,
    pub broker_healthy: bool,
    pub broker_pid: Option<u32>,
    pub gateway_url: String,
    pub gateway_healthy: bool,
    pub gateway_pid: Option<u32>,
    pub token_present: bool,
    /// Routes the gateway can serve, when it is healthy and a token was available.
    pub model_count: Option<usize>,
}

pub fn run_omp_command(workspace: &Path, config: &AetherConfig, args: OmpArgs) -> Result<()> {
    match args.command {
        OmpCommand::Up => {
            ensure_gateway_ready(workspace, config, true)?;
            print_status(&probe_status(workspace, config));
            Ok(())
        }
        OmpCommand::Down => {
            let stopped = stop_services(workspace)?;
            if stopped.is_empty() {
                println!("no omp processes started by aetherd were running");
            } else {
                println!("stopped {}", stopped.join(", "));
            }
            Ok(())
        }
        OmpCommand::Status => {
            let status = probe_status(workspace, config);
            print_status(&status);
            if !status.gateway_healthy {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

/// True when the run needs the omp gateway: the selected provider or a quality-pass provider
/// is `omp`.
pub fn run_uses_omp(config: &AetherConfig, selected_provider: InferenceProviderKind) -> bool {
    let quality_uses_omp = |value: Option<&str>| {
        value
            .map(str::trim)
            .is_some_and(|value| value.eq_ignore_ascii_case(InferenceProviderKind::Omp.as_str()))
    };
    selected_provider == InferenceProviderKind::Omp
        || config
            .inference
            .tiered
            .as_ref()
            .is_some_and(|tiered| quality_uses_omp(Some(tiered.primary.as_str())))
        || quality_uses_omp(config.sir_quality.triage_provider.as_deref())
        || quality_uses_omp(config.sir_quality.deep_provider.as_deref())
}

/// Autostart hook: when the run uses omp and `[inference.omp].autostart` is on, make sure
/// the gateway answers before any inference is attempted. Never fails a run that has
/// autostart disabled; the provider loader reports the unreachable gateway instead.
pub fn autostart_if_needed(
    workspace: &Path,
    config: &AetherConfig,
    selected_provider: InferenceProviderKind,
) -> Result<()> {
    if !run_uses_omp(config, selected_provider) || !config.inference.omp.autostart {
        return Ok(());
    }
    ensure_gateway_ready(workspace, config, false)
}

/// Make the gateway reachable, spawning the broker and gateway when they are not.
///
/// `explicit` is true for `aetherd omp up`, where an already-running pair is reported rather
/// than silently accepted.
fn ensure_gateway_ready(workspace: &Path, config: &AetherConfig, explicit: bool) -> Result<()> {
    let omp = &config.inference.omp;
    let gateway_url = effective_gateway_url(config);
    if is_healthy(&format!("{gateway_url}/healthz")) {
        if explicit {
            println!("omp gateway already healthy at {gateway_url}");
        }
        return Ok(());
    }

    let broker_url = omp.broker_url();
    let timeout = Duration::from_secs(omp.startup_timeout_secs.max(1));
    let state_dir = state_dir(workspace);
    fs::create_dir_all(&state_dir)
        .with_context(|| format!("failed to create {}", state_dir.display()))?;

    if !is_healthy(&format!("{broker_url}/v1/healthz")) {
        tracing::info!(url = %broker_url, "starting omp auth-broker");
        spawn_service(
            omp,
            &state_dir,
            Service::Broker,
            &["auth-broker", "serve", "--bind", omp.broker_bind.trim()],
            &[],
        )?;
        wait_healthy(&format!("{broker_url}/v1/healthz"), timeout)
            .map_err(|_| startup_failure(Service::Broker, &state_dir, &broker_url))?;
    }

    tracing::info!(url = %gateway_url, broker = %broker_url, "starting omp auth-gateway");
    spawn_service(
        omp,
        &state_dir,
        Service::Gateway,
        &["auth-gateway", "serve", "--bind", omp.gateway_bind.trim()],
        &[("OMP_AUTH_BROKER_URL", broker_url.as_str())],
    )?;
    wait_healthy(&format!("{gateway_url}/healthz"), timeout)
        .map_err(|_| startup_failure(Service::Gateway, &state_dir, &gateway_url))?;
    tracing::info!(url = %gateway_url, "omp gateway ready");
    Ok(())
}

fn startup_failure(service: Service, state_dir: &Path, url: &str) -> anyhow::Error {
    anyhow!(
        "omp {} did not become healthy at {url}; see {} (is `omp` installed and logged in? \
         run `omp auth-broker login <provider>` first)",
        service.name(),
        log_path(state_dir, service).display()
    )
}

/// The gateway origin AETHER will actually call: `inference.endpoint` minus its `/v1`
/// suffix when set, otherwise `[inference.omp].gateway_bind`.
pub fn effective_gateway_url(config: &AetherConfig) -> String {
    config
        .inference
        .endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(gateway_origin_from_endpoint)
        .unwrap_or_else(|| config.inference.omp.gateway_url())
}

/// `http://127.0.0.1:4000/v1/` -> `http://127.0.0.1:4000`.
fn gateway_origin_from_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim().trim_end_matches('/');
    trimmed
        .strip_suffix("/v1")
        .unwrap_or(trimmed)
        .trim_end_matches('/')
        .to_owned()
}

fn state_dir(workspace: &Path) -> PathBuf {
    workspace.join(".aether").join("omp")
}

fn pid_path(state_dir: &Path, service: Service) -> PathBuf {
    state_dir.join(format!("{}.pid", service.name()))
}

fn log_path(state_dir: &Path, service: Service) -> PathBuf {
    state_dir.join(format!("{}.log", service.name()))
}

fn spawn_service(
    omp: &OmpConfig,
    state_dir: &Path,
    service: Service,
    args: &[&str],
    env: &[(&str, &str)],
) -> Result<u32> {
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path(state_dir, service))
        .with_context(|| format!("failed to open {}", log_path(state_dir, service).display()))?;
    let stderr = log.try_clone().context("failed to clone omp log handle")?;
    let mut command = Command::new(omp.command.trim());
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr));
    for (key, value) in env {
        command.env(key, value);
    }
    detach(&mut command);
    let child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            anyhow!(
                "`{}` not found; install Oh My Pi (https://omp.sh) or set [inference.omp].command \
                 to the omp binary",
                omp.command
            )
        } else {
            anyhow!(
                "failed to spawn `{} {}`: {error}",
                omp.command,
                args.join(" ")
            )
        }
    })?;
    let pid = child.id();
    write_pid(state_dir, service, pid)?;
    Ok(pid)
}

#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // Own process group: the pair outlives this aetherd invocation and is not killed by the
    // terminal's SIGINT to aetherd. `aetherd omp down` stops it by pid.
    command.process_group(0);
}

#[cfg(not(unix))]
fn detach(_command: &mut Command) {}

fn write_pid(state_dir: &Path, service: Service, pid: u32) -> Result<()> {
    let path = pid_path(state_dir, service);
    let mut file =
        fs::File::create(&path).with_context(|| format!("failed to write {}", path.display()))?;
    writeln!(file, "{pid}")?;
    Ok(())
}

fn read_pid(state_dir: &Path, service: Service) -> Option<u32> {
    fs::read_to_string(pid_path(state_dir, service))
        .ok()
        .and_then(|raw| parse_pid(&raw))
}

fn parse_pid(raw: &str) -> Option<u32> {
    raw.trim().parse::<u32>().ok().filter(|pid| *pid > 0)
}

/// Stop the processes recorded by `up`, gateway first, and remove their pid files.
fn stop_services(workspace: &Path) -> Result<Vec<String>> {
    let state_dir = state_dir(workspace);
    let mut stopped = Vec::new();
    for service in [Service::Gateway, Service::Broker] {
        let Some(pid) = read_pid(&state_dir, service) else {
            continue;
        };
        if terminate(pid) {
            stopped.push(format!("{} (pid {pid})", service.name()));
        }
        let _ = fs::remove_file(pid_path(&state_dir, service));
    }
    Ok(stopped)
}

#[cfg(unix)]
fn terminate(pid: u32) -> bool {
    Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn terminate(pid: u32) -> bool {
    Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn probe_status(workspace: &Path, config: &AetherConfig) -> OmpGatewayStatus {
    let state_dir = state_dir(workspace);
    let broker_url = config.inference.omp.broker_url();
    let gateway_url = effective_gateway_url(config);
    let gateway_healthy = is_healthy(&format!("{gateway_url}/healthz"));
    let token = gateway_token(&config.inference.api_key_env);
    let model_count = if gateway_healthy {
        token
            .as_deref()
            .and_then(|token| count_models(&format!("{gateway_url}/v1/models"), token))
    } else {
        None
    };
    OmpGatewayStatus {
        broker_healthy: is_healthy(&format!("{broker_url}/v1/healthz")),
        broker_url,
        broker_pid: read_pid(&state_dir, Service::Broker),
        gateway_healthy,
        gateway_url,
        gateway_pid: read_pid(&state_dir, Service::Gateway),
        token_present: token.is_some(),
        model_count,
    }
}

fn print_status(status: &OmpGatewayStatus) {
    let flag = |healthy: bool| if healthy { "healthy" } else { "unreachable" };
    let pid = |pid: Option<u32>| {
        pid.map(|pid| format!(" (pid {pid}, started by aetherd)"))
            .unwrap_or_default()
    };
    println!(
        "broker  {}  {}{}",
        status.broker_url,
        flag(status.broker_healthy),
        pid(status.broker_pid)
    );
    println!(
        "gateway {}  {}{}",
        status.gateway_url,
        flag(status.gateway_healthy),
        pid(status.gateway_pid)
    );
    println!(
        "token   {}",
        if status.token_present {
            "present"
        } else {
            "missing (run `omp auth-gateway token`)"
        }
    );
    match status.model_count {
        Some(count) => println!("routes  {count} model(s) served by the gateway"),
        None if status.gateway_healthy => {
            println!("routes  unknown (no token or /v1/models unavailable)")
        }
        None => println!("routes  n/a (start with `aetherd omp up`)"),
    }
}

/// Same lookup order as the inference loader: env var, then `~/.omp/auth-gateway.token`.
fn gateway_token(api_key_env: &str) -> Option<String> {
    let env_name = if api_key_env.trim().is_empty() || api_key_env == "GEMINI_API_KEY" {
        DEFAULT_OMP_GATEWAY_TOKEN_ENV
    } else {
        api_key_env
    };
    if let Some(token) = env_non_empty(env_name) {
        return Some(token);
    }
    let path = env_non_empty("OMP_GATEWAY_TOKEN_FILE")
        .map(PathBuf::from)
        .or_else(|| {
            env_non_empty("HOME")
                .or_else(|| env_non_empty("USERPROFILE"))
                .map(|home| Path::new(&home).join(OMP_GATEWAY_TOKEN_FILE))
        })?;
    fs::read_to_string(path)
        .ok()
        .map(|raw| raw.trim().to_owned())
        .filter(|token| !token.is_empty())
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn wait_healthy(url: &str, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if is_healthy(url) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("timed out waiting for {url}");
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn is_healthy(url: &str) -> bool {
    blocking_get(url, None).is_some_and(|(status, _)| status.is_success())
}

fn count_models(url: &str, token: &str) -> Option<usize> {
    let (status, body) = blocking_get(url, Some(token))?;
    if !status.is_success() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(&body).ok()?;
    value
        .get("data")
        .and_then(|data| data.as_array())
        .map(|models| models.len())
}

/// One GET on a throwaway current-thread runtime, so callers stay synchronous like the rest
/// of the CLI.
fn blocking_get(url: &str, bearer: Option<&str>) -> Option<(reqwest::StatusCode, String)> {
    let url = url.to_owned();
    let bearer = bearer.map(str::to_owned);
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        runtime.block_on(async move {
            let client = reqwest::Client::builder()
                .timeout(HEALTH_TIMEOUT)
                .build()
                .ok()?;
            let mut request = client.get(&url);
            if let Some(token) = bearer {
                request = request.bearer_auth(token);
            }
            let response = request.send().await.ok()?;
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            Some((status, body))
        })
    })
    .join()
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_config::SirQualityConfig;
    use tempfile::tempdir;

    fn omp_config() -> AetherConfig {
        let mut config = AetherConfig::default();
        config.inference.provider = InferenceProviderKind::Omp;
        config.inference.model = Some("anthropic/claude-fable-5".to_owned());
        config
    }

    #[test]
    fn gateway_origin_strips_v1_suffix() {
        assert_eq!(
            gateway_origin_from_endpoint("http://127.0.0.1:4000/v1/"),
            "http://127.0.0.1:4000"
        );
        assert_eq!(
            gateway_origin_from_endpoint("http://gateway.local:4100"),
            "http://gateway.local:4100"
        );
        let mut config = omp_config();
        assert_eq!(effective_gateway_url(&config), "http://127.0.0.1:4000");
        config.inference.endpoint = Some("http://127.0.0.1:4100/v1".to_owned());
        assert_eq!(effective_gateway_url(&config), "http://127.0.0.1:4100");
    }

    #[test]
    fn run_uses_omp_covers_quality_passes_and_tiered_primary() {
        let plain = AetherConfig::default();
        assert!(!run_uses_omp(&plain, InferenceProviderKind::Gemini));
        assert!(run_uses_omp(&plain, InferenceProviderKind::Omp));

        let mut quality = AetherConfig::default();
        quality.sir_quality = SirQualityConfig {
            deep_provider: Some(" OMP ".to_owned()),
            ..SirQualityConfig::default()
        };
        assert!(run_uses_omp(&quality, InferenceProviderKind::Gemini));

        let mut tiered = AetherConfig::default();
        tiered.inference.tiered = Some(aether_config::TieredConfig {
            primary: "omp".to_owned(),
            ..aether_config::TieredConfig::default()
        });
        assert!(run_uses_omp(&tiered, InferenceProviderKind::Tiered));
    }

    #[test]
    fn pid_files_round_trip_and_reject_garbage() {
        let temp = tempdir().expect("tempdir");
        let state = temp.path().join("omp");
        fs::create_dir_all(&state).expect("mkdir");
        write_pid(&state, Service::Gateway, 4242).expect("write pid");
        assert_eq!(read_pid(&state, Service::Gateway), Some(4242));
        assert_eq!(read_pid(&state, Service::Broker), None);
        assert_eq!(parse_pid("0"), None);
        assert_eq!(parse_pid("abc"), None);
    }

    #[test]
    fn autostart_is_a_no_op_when_disabled_or_unused() {
        let temp = tempdir().expect("tempdir");
        let mut config = omp_config();
        config.inference.omp.autostart = false;
        autostart_if_needed(temp.path(), &config, InferenceProviderKind::Omp)
            .expect("disabled autostart never fails");
        let gemini = AetherConfig::default();
        autostart_if_needed(temp.path(), &gemini, InferenceProviderKind::Gemini)
            .expect("non-omp runs never touch the gateway");
    }

    #[test]
    fn missing_omp_binary_is_reported_clearly() {
        let temp = tempdir().expect("tempdir");
        let mut config = omp_config();
        config.inference.omp.command = "aether-test-no-such-omp-binary".to_owned();
        // Unused loopback ports so no real gateway answers the health probe.
        config.inference.omp.broker_bind = "127.0.0.1:1".to_owned();
        config.inference.omp.gateway_bind = "127.0.0.1:1".to_owned();
        config.inference.omp.startup_timeout_secs = 1;
        let error = ensure_gateway_ready(temp.path(), &config, false)
            .expect_err("missing binary must fail");
        assert!(error.to_string().contains("not found"), "{error}");
        assert!(error.to_string().contains("omp.sh"), "{error}");
    }

    #[test]
    fn down_with_nothing_recorded_is_empty() {
        let temp = tempdir().expect("tempdir");
        assert!(stop_services(temp.path()).expect("down").is_empty());
    }
}
