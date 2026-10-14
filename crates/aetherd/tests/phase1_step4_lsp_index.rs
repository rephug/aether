use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use aether_lsp::resolve_hover_markdown_for_path;
use aether_store::SqliteStore;
use aetherd::indexer::{IndexerConfig, run_initial_index_once};
use tempfile::tempdir;
use tower_lsp::lsp_types::Position;

#[test]
fn lsp_index_mode_path_generates_sir_and_hover_reads_it() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempdir()?;
    let workspace = temp.path();
    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[inference]
provider = "qwen3_local"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
    )?;

    fs::create_dir_all(workspace.join("src"))?;

    let rust_file = workspace.join("src/lib.rs");
    fs::write(
        &rust_file,
        "fn alpha() -> i32 { 1 }\nfn beta() -> i32 { 2 }\n",
    )?;

    let ts_file = workspace.join("src/app.ts");
    fs::write(
        &ts_file,
        "function gamma(): number { return 1; }\nfunction delta(): number { return 2; }\n",
    )?;

    let config = IndexerConfig {
        workspace: workspace.to_path_buf(),
        debounce_ms: 300,
        print_events: false,
        print_sir: false,
        embeddings_only: false,
        force: false,
        full: false,
        deep: false,
        turbo_concurrency: None,
        dry_run: false,
        sir_concurrency: 2,
        lifecycle_logs: true,
        inference_provider: None,
        inference_model: None,
        inference_endpoint: None,
        inference_api_key_env: None,
        pause_flag: None,
    };

    run_initial_index_once(&config)?;

    assert!(workspace.join(".aether/meta.sqlite").exists());

    let sir_dir = workspace.join(".aether/sir");
    let sir_files = fs::read_dir(&sir_dir)?.count();
    assert_eq!(sir_files, 0);

    let store = SqliteStore::open(workspace)?;

    let rust_hover =
        resolve_hover_markdown_for_path(workspace, &store, &rust_file, Position::new(0, 4))?
            .expect("rust hover should resolve");

    assert!(rust_hover.contains("SIR generation in progress"));
    assert!(rust_hover.contains("**Kind:**"));

    let ts_hover =
        resolve_hover_markdown_for_path(workspace, &store, &ts_file, Position::new(1, 10))?
            .expect("ts hover should resolve");

    assert!(ts_hover.contains("SIR generation in progress"));
    assert!(ts_hover.contains("**Dependencies**"));

    Ok(())
}

/// Kills the spawned server if a test assertion fails before it exits.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// Reads one LSP message and rejects anything on stdout that is not a
/// `Content-Length` framed JSON body. Returns `None` when stdout closes cleanly
/// between frames; closing mid-frame is an error.
fn read_lsp_frame(reader: &mut impl BufRead) -> io::Result<Option<serde_json::Value>> {
    let mut content_length = None;
    let mut header_bytes = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            if header_bytes == 0 {
                return Ok(None);
            }
            return Err(io::Error::other("LSP stdout closed inside a header"));
        }
        header_bytes += line.len();
        if header_bytes > 8192 {
            return Err(io::Error::other("LSP header block exceeded 8 KiB"));
        }
        if line == "\r\n" {
            break;
        }
        if !line.ends_with("\r\n") {
            return Err(io::Error::other(format!("unframed LSP stdout: {line:?}")));
        }
        let (name, value) = line
            .trim_end()
            .split_once(':')
            .ok_or_else(|| io::Error::other(format!("unframed LSP stdout: {line:?}")))?;
        if name.eq_ignore_ascii_case("Content-Length") && content_length.is_none() {
            content_length = Some(value.trim().parse::<usize>().map_err(io::Error::other)?);
        } else if !name.eq_ignore_ascii_case("Content-Type") {
            return Err(io::Error::other(format!(
                "unexpected or duplicate LSP header: {line:?}"
            )));
        }
    }
    let length = content_length.ok_or_else(|| io::Error::other("missing Content-Length"))?;
    if length > 1024 * 1024 {
        return Err(io::Error::other("LSP body exceeded 1 MiB"));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(io::Error::other)
}

fn send_lsp_message(writer: &mut impl Write, message: serde_json::Value) -> io::Result<()> {
    let body = serde_json::to_vec(&message).map_err(io::Error::other)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

fn wait_for_response(
    frames: &mpsc::Receiver<io::Result<serde_json::Value>>,
    id: u64,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let frame = frames.recv_timeout(deadline.saturating_duration_since(Instant::now()))??;
        if frame.get("id").and_then(serde_json::Value::as_u64) == Some(id) {
            assert!(frame.get("error").is_none(), "LSP error response: {frame}");
            return Ok(frame);
        }
    }
}

/// Runs `aetherd --lsp --index` against a fresh workspace and drives a full
/// initialize/hover/shutdown/exit session while checking that stdout carries
/// only LSP frames and the index traces go to stderr.
fn assert_lsp_stdout_is_framed(log_format: &str) -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let workspace = temp.path();
    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[inference]
provider = "mock"

[storage]
graph_backend = "sqlite"

[embeddings]
enabled = false
vector_backend = "sqlite"

[dashboard]
enabled = false
"#,
    )?;
    let source = workspace.join("numbers.ts");
    fs::write(
        &source,
        "export function add(a: number, b: number): number {\n  return a + b;\n}\n",
    )?;
    // Migrate the store up front: the LSP and the index thread both open it at
    // startup, and this test is about stdout, not that race.
    drop(SqliteStore::open(workspace)?);

    // Keep the log outside the workspace so the indexer doesn't watch it.
    let log_dir = tempdir()?;
    let stderr_path = log_dir.path().join("aetherd-stderr.log");
    let read_stderr = || -> io::Result<String> {
        Ok(String::from_utf8_lossy(&fs::read(&stderr_path)?).into_owned())
    };
    let mut command = Command::new(env!("CARGO_BIN_EXE_aetherd"));
    command
        .arg("--workspace")
        .arg(workspace)
        .args(["--lsp", "--index", "--log-format", log_format])
        .env_clear()
        .env("HOME", workspace)
        .env("TMPDIR", workspace);
    for name in ["PATH", "LD_LIBRARY_PATH"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let mut child = ChildGuard(
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(fs::File::create(&stderr_path)?)
            .spawn()?,
    );
    let stdout = child.0.stdout.take().expect("piped stdout");
    let mut stdin = child.0.stdin.take().expect("piped stdin");

    let (sender, frames) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut stdout = BufReader::new(stdout);
        loop {
            let frame = match read_lsp_frame(&mut stdout) {
                Ok(Some(frame)) => Ok(frame),
                Ok(None) => break,
                Err(err) => Err(err),
            };
            let stop = frame.is_err();
            if sender.send(frame).is_err() || stop {
                break;
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let logs = read_stderr()?;
        if logs.contains("Structural index complete:") {
            break;
        }
        // The server must not write anything to stdout before `initialize`.
        match frames.try_recv() {
            Err(mpsc::TryRecvError::Empty) => {}
            Ok(Err(err)) => return Err(format!("stdout before initialize: {err}").into()),
            Ok(Ok(frame)) => return Err(format!("unsolicited LSP frame: {frame}").into()),
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(format!("stdout closed before initialize; stderr: {logs}").into());
            }
        }
        if let Some(status) = child.0.try_wait()? {
            return Err(format!("aetherd exited before indexing ({status}): {logs}").into());
        }
        if Instant::now() >= deadline {
            return Err(format!("index trace never reached stderr; stderr was: {logs:?}").into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let uri = tower_lsp::lsp_types::Url::from_file_path(&source)
        .map_err(|_| io::Error::other("invalid source URI"))?;
    let root_uri = tower_lsp::lsp_types::Url::from_directory_path(workspace)
        .map_err(|_| io::Error::other("invalid root URI"))?;
    send_lsp_message(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"rootUri": root_uri, "capabilities": {}}}),
    )?;
    let initialized = wait_for_response(&frames, 1)?;
    assert_eq!(initialized["result"]["capabilities"]["hoverProvider"], true);
    send_lsp_message(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
    )?;
    send_lsp_message(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "textDocument/hover",
            "params": {"textDocument": {"uri": uri}, "position": {"line": 0, "character": 18}}}),
    )?;
    let hover = wait_for_response(&frames, 2)?;
    // Both the "SIR pending" and the generated-SIR hover start with the symbol
    // heading; which one we get depends on how far the mock generation got.
    assert!(
        hover["result"]["contents"]["value"]
            .as_str()
            .is_some_and(|value| value.starts_with("### add")),
        "unexpected hover response: {hover}"
    );
    // `shutdown` and `exit` take no params; sending `params: null` is invalid.
    send_lsp_message(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "shutdown"}),
    )?;
    wait_for_response(&frames, 3)?;
    send_lsp_message(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "method": "exit"}),
    )?;
    drop(stdin);

    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.0.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            return Err("aetherd did not exit after `exit`".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "aetherd exited with {status}");

    // Anything left on stdout after the shutdown response must also be framed,
    // and stdout must close cleanly between frames.
    reader
        .join()
        .map_err(|_| io::Error::other("stdout reader panicked"))?;
    for frame in frames.try_iter() {
        if let Err(err) = frame {
            return Err(format!("bad stdout after shutdown: {err}").into());
        }
    }
    Ok(())
}

#[test]
fn lsp_index_mode_keeps_human_logs_off_stdout() -> Result<(), Box<dyn std::error::Error>> {
    assert_lsp_stdout_is_framed("human")
}

#[test]
fn lsp_index_mode_keeps_json_logs_off_stdout() -> Result<(), Box<dyn std::error::Error>> {
    assert_lsp_stdout_is_framed("json")
}
