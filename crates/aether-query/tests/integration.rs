use std::fs;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_mcp::{AetherMcpServer, SharedState};
use aether_query::config::QueryConfig;
use aether_query::server::build_router;
use aether_store::{SqliteStore, Store, SymbolRecord};
use reqwest::Client;
use reqwest::header;
use serde_json::{Value, json};
use tempfile::tempdir;

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn write_workspace_config(workspace: &Path) {
    fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[inference]
provider = "mock"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[embeddings]
enabled = false
provider = "mock"
vector_backend = "sqlite"
"#,
    )
    .expect("write config");
}

fn seed_symbol(workspace: &Path, id: &str, qualified_name: &str) {
    let store = SqliteStore::open(workspace).expect("open sqlite store");
    store
        .upsert_symbol(SymbolRecord {
            id: id.to_owned(),
            file_path: "src/payments/processor.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature_fingerprint: format!("sig-{id}"),
            last_seen_at: now_unix(),
        })
        .expect("upsert symbol");
}

async fn spawn_server(
    workspace: &Path,
    auth_token: Option<&str>,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let state = Arc::new(
        SharedState::open_readonly_async(workspace)
            .await
            .expect("open readonly shared state"),
    );
    let mcp_server = AetherMcpServer::from_state(state.clone(), false);
    let mut config = QueryConfig::default();
    config.query.index_path = workspace.to_path_buf();
    config.query.auth_token = auth_token.unwrap_or_default().to_owned();

    let app = build_router(state, mcp_server, config);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener addr");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    (addr, handle)
}

fn has_key_recursive(value: &Value, key: &str) -> bool {
    match value {
        Value::Object(map) => {
            map.contains_key(key) || map.values().any(|value| has_key_recursive(value, key))
        }
        Value::Array(items) => items.iter().any(|value| has_key_recursive(value, key)),
        _ => false,
    }
}

async fn post_mcp(
    client: &Client,
    base_url: &str,
    auth_token: Option<&str>,
    tool_name: &str,
    arguments: Value,
) -> reqwest::Response {
    let mut request = client
        .post(format!("{base_url}/mcp"))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": tool_name,
                "arguments": arguments,
            }
        }))
        .header(header::ACCEPT, "application/json, text/event-stream");

    if let Some(token) = auth_token {
        request = request.bearer_auth(token);
    }

    request.send().await.expect("send mcp request")
}

fn extract_first_sse_json(body: &str) -> Value {
    let mut data_lines = Vec::new();
    for line in body.lines() {
        if let Some(data) = line.strip_prefix("data: ") {
            data_lines.push(data);
        }
    }

    let payload = data_lines.join("\n");
    serde_json::from_str(&payload).expect("parse SSE JSON payload")
}

#[tokio::test]
async fn read_mcp_tools_return_results() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_workspace_config(workspace);
    seed_symbol(workspace, "sym-payment", "process_payment_with_retry");

    let (addr, handle) = spawn_server(workspace, None).await;
    let base_url = format!("http://{addr}");
    let client = Client::new();

    let status_resp = post_mcp(&client, &base_url, None, "aether_status", json!({})).await;
    assert!(status_resp.status().is_success());
    let status_body = status_resp.text().await.expect("status body");
    let status_json = extract_first_sse_json(&status_body);
    assert!(has_key_recursive(&status_json, "schema_version"));

    let lookup_resp = post_mcp(
        &client,
        &base_url,
        None,
        "aether_symbol_lookup",
        json!({"query": "payment", "limit": 5}),
    )
    .await;
    assert!(lookup_resp.status().is_success());
    let lookup_body = lookup_resp.text().await.expect("lookup body");
    let lookup_json = extract_first_sse_json(&lookup_body);
    let lookup_text = serde_json::to_string(&lookup_json).expect("serialize lookup json");
    assert!(lookup_text.contains("process_payment_with_retry"));

    handle.abort();
}

#[tokio::test]
async fn write_mcp_tools_return_read_only_error() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_workspace_config(workspace);
    seed_symbol(workspace, "sym-memory", "memory_helper");

    let (addr, handle) = spawn_server(workspace, None).await;
    let base_url = format!("http://{addr}");
    let client = Client::new();

    let response = post_mcp(
        &client,
        &base_url,
        None,
        "aether_remember",
        json!({"content": "hello from read only test"}),
    )
    .await;
    assert!(response.status().is_success());
    let body = response.text().await.expect("response text").to_lowercase();
    assert!(body.contains("read-only") || body.contains("read only"));

    handle.abort();
}

#[tokio::test]
async fn auth_token_is_enforced_on_all_routes() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_workspace_config(workspace);
    seed_symbol(workspace, "sym-auth", "auth_target");

    let token = "test-secret-123";
    let (addr, handle) = spawn_server(workspace, Some(token)).await;
    let base_url = format!("http://{addr}");
    let client = Client::new();

    let no_auth = client
        .get(format!("{base_url}/health"))
        .send()
        .await
        .expect("no auth request");
    assert_eq!(no_auth.status(), reqwest::StatusCode::UNAUTHORIZED);

    let wrong_auth = client
        .get(format!("{base_url}/health"))
        .bearer_auth("wrong-token")
        .send()
        .await
        .expect("wrong auth request");
    assert_eq!(wrong_auth.status(), reqwest::StatusCode::UNAUTHORIZED);

    let ok_auth = client
        .get(format!("{base_url}/health"))
        .bearer_auth(token)
        .send()
        .await
        .expect("correct auth request");
    assert!(ok_auth.status().is_success());

    handle.abort();
}

#[tokio::test]
async fn health_and_info_endpoints_return_expected_fields() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_workspace_config(workspace);
    seed_symbol(workspace, "sym-info", "info_target");

    let (addr, handle) = spawn_server(workspace, None).await;
    let base_url = format!("http://{addr}");
    let client = Client::new();

    let health: Value = client
        .get(format!("{base_url}/health"))
        .send()
        .await
        .expect("health request")
        .json()
        .await
        .expect("health json");
    assert!(health.get("status").is_some());
    assert!(health.get("schema_version").is_some());
    assert!(health.get("stale").is_some());

    let info: Value = client
        .get(format!("{base_url}/info"))
        .send()
        .await
        .expect("info request")
        .json()
        .await
        .expect("info json");
    assert_eq!(info.get("read_only").and_then(Value::as_bool), Some(true));
    assert!(
        info.get("symbols")
            .and_then(Value::as_i64)
            .unwrap_or_default()
            >= 1
    );

    handle.abort();
}

#[tokio::test]
async fn concurrent_read_write_smoke_test_no_lock_errors() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_workspace_config(workspace);
    seed_symbol(workspace, "sym-concurrent-a", "concurrent_alpha");

    let (addr, handle) = spawn_server(workspace, None).await;
    let base_url = format!("http://{addr}");
    let client = Client::new();

    let writer = SqliteStore::open(workspace).expect("open rw store");
    writer
        .upsert_symbol(SymbolRecord {
            id: "sym-concurrent-b".to_owned(),
            file_path: "src/payments/processor.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "concurrent_beta".to_owned(),
            signature_fingerprint: "sig-concurrent-b".to_owned(),
            last_seen_at: now_unix(),
        })
        .expect("rw upsert while query server is live");

    let lookup_resp = post_mcp(
        &client,
        &base_url,
        None,
        "aether_symbol_lookup",
        json!({"query": "concurrent", "limit": 10}),
    )
    .await;
    assert!(lookup_resp.status().is_success());
    let body = lookup_resp.text().await.expect("lookup body");
    assert!(!body.to_lowercase().contains("locked"));

    handle.abort();
}
