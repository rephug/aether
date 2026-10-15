use std::io::{Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_config::{
    EmbeddingProviderKind, InferenceProviderKind, SearchRerankerKind, ensure_workspace_config,
};
use tempfile::tempdir;

use super::*;
use crate::providers::gemini::GEMINI_DEFAULT_MODEL;

#[test]
fn load_embedding_provider_defaults_to_disabled() {
    let temp = tempdir().expect("tempdir");
    ensure_workspace_config(temp.path()).expect("ensure config");

    let loaded =
        load_embedding_provider_from_config(temp.path(), EmbeddingProviderOverrides::default())
            .expect("load embedding provider");
    assert!(loaded.is_none());
}

#[test]
fn load_embedding_provider_reads_enabled_qwen_settings() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    ensure_workspace_config(workspace).expect("ensure config");

    std::fs::write(
        workspace.join(".aether/config.toml"),
        r#"[inference]
provider = "auto"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true

[embeddings]
enabled = true
provider = "qwen3_local"
model = "qwen3-embeddings-4B"
endpoint = "http://127.0.0.1:11434/api/embeddings"
"#,
    )
    .expect("write config");

    let loaded =
        load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default())
            .expect("load embedding provider")
            .expect("embedding provider should be enabled");

    assert_eq!(
        loaded.provider_name,
        EmbeddingProviderKind::Qwen3Local.as_str()
    );
    assert_eq!(loaded.model_name, "qwen3-embeddings-4B");
}

#[test]
fn load_embedding_provider_reads_enabled_candle_settings() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    ensure_workspace_config(workspace).expect("ensure config");

    std::fs::write(
        workspace.join(".aether/config.toml"),
        r#"[embeddings]
enabled = true
provider = "candle"

[embeddings.candle]
model_dir = ".aether/models"
"#,
    )
    .expect("write config");

    let loaded =
        load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default())
            .expect("load embedding provider")
            .expect("embedding provider should be enabled");

    assert_eq!(loaded.provider_name, EmbeddingProviderKind::Candle.as_str());
    assert_eq!(loaded.model_name, "qwen3-embedding-0.6b");
}

#[test]
fn load_embedding_provider_openai_compat_requires_endpoint() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    ensure_workspace_config(workspace).expect("ensure config");
    let env_name = format!(
        "AETHER_TEST_OPENAI_COMPAT_EMBED_KEY_ENDPOINT_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    );

    let _env = crate::env_vars::overrides::set(&env_name, "test-key");

    std::fs::write(
        workspace.join(".aether/config.toml"),
        format!(
            r#"[embeddings]
enabled = true
provider = "openai_compat"
model = "text-embedding-3-large"
api_key_env = "{env_name}"
"#
        ),
    )
    .expect("write config");

    let result =
        load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default());

    match result {
        Err(InferError::MissingEndpoint) => {}
        Ok(_) => panic!("expected missing endpoint"),
        Err(err) => panic!("expected missing endpoint, got {err}"),
    }
}

#[test]
fn load_embedding_provider_openai_compat_requires_model() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    ensure_workspace_config(workspace).expect("ensure config");
    let env_name = format!(
        "AETHER_TEST_OPENAI_COMPAT_EMBED_KEY_MODEL_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    );

    let _env = crate::env_vars::overrides::set(&env_name, "test-key");

    std::fs::write(
        workspace.join(".aether/config.toml"),
        format!(
            r#"[embeddings]
enabled = true
provider = "openai_compat"
endpoint = "https://api.example.com/v1"
api_key_env = "{env_name}"
"#
        ),
    )
    .expect("write config");

    let result =
        load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default());

    match result {
        Err(InferError::MissingModel) => {}
        Ok(_) => panic!("expected missing model"),
        Err(err) => panic!("expected missing model, got {err}"),
    }
}

#[test]
fn load_embedding_provider_openai_compat_requires_api_key() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    ensure_workspace_config(workspace).expect("ensure config");
    let env_name = "AETHER_TEST_OPENAI_COMPAT_EMBED_KEY_ZZZZZ".to_owned();

    std::fs::write(
        workspace.join(".aether/config.toml"),
        format!(
            r#"[embeddings]
enabled = true
provider = "openai_compat"
model = "text-embedding-3-large"
endpoint = "https://api.example.com/v1"
api_key_env = "{env_name}"
"#
        ),
    )
    .expect("write config");

    let result =
        load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default());

    match result {
        Err(InferError::MissingApiKey(name)) => assert_eq!(name, env_name),
        Ok(_) => panic!("expected missing api key"),
        Err(err) => panic!("expected missing api key, got {err}"),
    }
}

#[test]
fn load_embedding_provider_openai_compat_constructs_with_valid_config() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    ensure_workspace_config(workspace).expect("ensure config");
    let env_name = format!(
        "AETHER_TEST_OPENAI_COMPAT_EMBED_KEY_VALID_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    );

    let _env = crate::env_vars::overrides::set(&env_name, "test-key");

    std::fs::write(
        workspace.join(".aether/config.toml"),
        format!(
            r#"[embeddings]
enabled = true
provider = "openai_compat"
model = "text-embedding-3-large"
endpoint = "https://api.example.com/v1/embeddings"
api_key_env = "{env_name}"
task_type = "CODE_RETRIEVAL"
dimensions = 3072
"#
        ),
    )
    .expect("write config");

    let loaded =
        load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default())
            .expect("load embedding provider")
            .expect("embedding provider should be enabled");

    assert_eq!(
        loaded.provider_name,
        EmbeddingProviderKind::OpenAiCompat.as_str()
    );
    assert_eq!(loaded.model_name, "text-embedding-3-large");
}

#[test]
fn load_embedding_provider_gemini_native_requires_model() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    ensure_workspace_config(workspace).expect("ensure config");
    let env_name = format!(
        "AETHER_TEST_GEMINI_NATIVE_EMBED_KEY_MODEL_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    );

    let _env = crate::env_vars::overrides::set(&env_name, "test-key");

    std::fs::write(
        workspace.join(".aether/config.toml"),
        format!(
            r#"[embeddings]
enabled = true
provider = "gemini_native"
api_key_env = "{env_name}"
"#
        ),
    )
    .expect("write config");

    let result =
        load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default());

    match result {
        Err(InferError::MissingModel) => {}
        Ok(_) => panic!("expected missing model"),
        Err(err) => panic!("expected missing model, got {err}"),
    }
}

#[test]
fn load_embedding_provider_gemini_native_requires_api_key_env() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    ensure_workspace_config(workspace).expect("ensure config");

    std::fs::write(
        workspace.join(".aether/config.toml"),
        r#"[embeddings]
enabled = true
provider = "gemini_native"
model = "gemini-embedding-2-preview"
"#,
    )
    .expect("write config");

    let result =
        load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default());

    match result {
        Err(InferError::InvalidConfig(message)) => {
            assert!(message.contains("embeddings.api_key_env"));
        }
        Ok(_) => panic!("expected invalid config"),
        Err(err) => panic!("expected invalid config, got {err}"),
    }
}

#[test]
fn load_embedding_provider_gemini_native_constructs_with_valid_config() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    ensure_workspace_config(workspace).expect("ensure config");
    let env_name = format!(
        "AETHER_TEST_GEMINI_NATIVE_EMBED_KEY_VALID_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    );

    let _env = crate::env_vars::overrides::set(&env_name, "test-key");

    std::fs::write(
        workspace.join(".aether/config.toml"),
        format!(
            r#"[embeddings]
enabled = true
provider = "gemini_native"
model = "gemini-embedding-2-preview"
api_key_env = "{env_name}"
dimensions = 3072
"#
        ),
    )
    .expect("write config");

    let loaded =
        load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default())
            .expect("load embedding provider")
            .expect("embedding provider should be enabled");

    assert_eq!(
        loaded.provider_name,
        EmbeddingProviderKind::GeminiNative.as_str()
    );
    assert_eq!(loaded.model_name, "gemini-embedding-2-preview");
}

#[test]
fn load_provider_auto_errors_when_key_missing() {
    let temp = tempdir().expect("tempdir");
    ensure_workspace_config(temp.path()).expect("ensure config");

    let result = load_provider_from_env_or_mock(
        temp.path(),
        ProviderOverrides {
            provider: Some(InferenceProviderKind::Auto),
            endpoint: Some("http://127.0.0.1:9".to_owned()),
            api_key_env: Some("AETHER_TEST_NONEXISTENT_KEY_ZZZZZ".to_owned()),
            ..ProviderOverrides::default()
        },
    );

    match result {
        Err(InferError::NoProviderAvailable(message)) => {
            assert!(message.contains("configure [inference] provider explicitly"))
        }
        Ok(_) => panic!("expected no provider available error, got Ok result"),
        Err(err) => panic!("expected no provider available error, got {err}"),
    }
}

#[test]
fn resolve_inference_thinking_prefers_override() {
    assert_eq!(
        resolve_inference_thinking(Some(" high ".to_owned()), Some("low".to_owned())),
        Some("high".to_owned())
    );
}

#[test]
fn resolve_inference_thinking_falls_back_to_config() {
    assert_eq!(
        resolve_inference_thinking(None, Some(" medium ".to_owned())),
        Some("medium".to_owned())
    );
}

#[test]
fn load_provider_auto_chooses_gemini_when_key_present() {
    let temp = tempdir().expect("tempdir");
    ensure_workspace_config(temp.path()).expect("ensure config");

    let env_name = format!(
        "AETHER_TEST_GEMINI_KEY_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    );

    let _env = crate::env_vars::overrides::set(&env_name, "test-key");

    let loaded = load_provider_from_env_or_mock(
        temp.path(),
        ProviderOverrides {
            provider: Some(InferenceProviderKind::Auto),
            endpoint: Some("http://127.0.0.1:9".to_owned()),
            api_key_env: Some(env_name.clone()),
            ..ProviderOverrides::default()
        },
    )
    .expect("load provider");

    assert_eq!(loaded.provider_name, InferenceProviderKind::Gemini.as_str());
    assert_eq!(loaded.model_name, GEMINI_DEFAULT_MODEL);
}

#[test]
fn load_provider_auto_prefers_qwen_when_ollama_is_reachable() {
    let temp = tempdir().expect("tempdir");
    ensure_workspace_config(temp.path()).expect("ensure config");

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind test ollama listener");
    let endpoint = format!("http://{}", listener.local_addr().expect("local addr"));
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept health check");
        let mut buffer = [0_u8; 1024];
        let _ = stream.read(&mut buffer);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n[]")
            .expect("write health response");
    });

    let loaded = load_provider_from_env_or_mock(
        temp.path(),
        ProviderOverrides {
            provider: Some(InferenceProviderKind::Auto),
            endpoint: Some(endpoint),
            ..ProviderOverrides::default()
        },
    )
    .expect("load provider");

    assert_eq!(
        loaded.provider_name,
        InferenceProviderKind::Qwen3Local.as_str()
    );
    assert_eq!(loaded.model_name, DEFAULT_QWEN_MODEL);
    server.join().expect("join health server");
}

#[test]
fn load_provider_openai_compat_requires_api_key_with_fabricated_env_var() {
    let temp = tempdir().expect("tempdir");
    ensure_workspace_config(temp.path()).expect("ensure config");
    let env_name = "AETHER_TEST_OPENAI_COMPAT_KEY_ZZZZZ".to_owned();

    let result = load_provider_from_env_or_mock(
        temp.path(),
        ProviderOverrides {
            provider: Some(InferenceProviderKind::OpenAiCompat),
            endpoint: Some("https://api.example.com/v1".to_owned()),
            model: Some("glm-4.7".to_owned()),
            api_key_env: Some(env_name.clone()),
            ..ProviderOverrides::default()
        },
    );

    match result {
        Err(InferError::MissingApiKey(name)) => assert_eq!(name, env_name),
        _ => panic!("expected missing api key"),
    }
}

#[test]
fn load_provider_mock_needs_no_key_or_model() {
    let temp = tempdir().expect("tempdir");
    ensure_workspace_config(temp.path()).expect("ensure config");
    let loaded = load_provider_from_env_or_mock(
        temp.path(),
        ProviderOverrides {
            provider: Some(InferenceProviderKind::Mock),
            ..ProviderOverrides::default()
        },
    )
    .expect("mock provider always loads");
    assert_eq!(loaded.provider_name, "mock");
    assert_eq!(loaded.model_name, "tree-sitter");
}

#[test]
fn load_provider_omp_requires_route_model() {
    let temp = tempdir().expect("tempdir");
    ensure_workspace_config(temp.path()).expect("ensure config");
    let env_name = "AETHER_TEST_OMP_TOKEN_ROUTE_ZZZZZ";
    let _env = crate::env_vars::overrides::set(env_name, "gateway-token");

    let missing = load_provider_from_env_or_mock(
        temp.path(),
        ProviderOverrides {
            provider: Some(InferenceProviderKind::Omp),
            api_key_env: Some(env_name.to_owned()),
            ..ProviderOverrides::default()
        },
    );
    match missing {
        Err(InferError::InvalidConfig(message)) => {
            assert!(message.contains("requires inference.model"), "{message}")
        }
        Ok(_) => panic!("expected invalid config, got a provider"),
        Err(other) => panic!("expected invalid config, got {other}"),
    }

    let bare = load_provider_from_env_or_mock(
        temp.path(),
        ProviderOverrides {
            provider: Some(InferenceProviderKind::Omp),
            model: Some("claude-fable-5".to_owned()),
            api_key_env: Some(env_name.to_owned()),
            ..ProviderOverrides::default()
        },
    );
    match bare {
        Err(InferError::InvalidConfig(message)) => {
            assert!(message.contains("not an omp route"), "{message}")
        }
        Ok(_) => panic!("expected invalid config, got a provider"),
        Err(other) => panic!("expected invalid config, got {other}"),
    }
}

#[test]
fn load_provider_omp_constructs_gateway_provider_from_env_token() {
    let temp = tempdir().expect("tempdir");
    ensure_workspace_config(temp.path()).expect("ensure config");
    let env_name = "AETHER_TEST_OMP_TOKEN_OK_ZZZZZ";
    let _env = crate::env_vars::overrides::set(env_name, "gateway-token");

    let loaded = load_provider_from_env_or_mock(
        temp.path(),
        ProviderOverrides {
            provider: Some(InferenceProviderKind::Omp),
            model: Some("anthropic/claude-fable-5".to_owned()),
            api_key_env: Some(env_name.to_owned()),
            thinking: Some("high".to_owned()),
            ..ProviderOverrides::default()
        },
    )
    .expect("omp provider should load with a token and a route");

    assert_eq!(loaded.provider_name, InferenceProviderKind::Omp.as_str());
    assert_eq!(loaded.model_name, "anthropic/claude-fable-5");
    assert_eq!(loaded.provider.provider_name(), "omp");
}

#[test]
fn omp_gateway_token_falls_back_to_token_file() {
    let temp = tempdir().expect("tempdir");
    let token_path = temp.path().join("auth-gateway.token");
    std::fs::write(&token_path, "file-token\n").expect("write token");
    let env_name = "AETHER_TEST_OMP_TOKEN_UNSET_ZZZZZ";

    let token = resolve_omp_gateway_token_from(env_name, Some(token_path.clone()))
        .expect("token file should be used");
    assert_eq!(token, "file-token");

    let missing = resolve_omp_gateway_token_from(env_name, Some(temp.path().join("absent")));
    match missing {
        Err(InferError::InvalidConfig(message)) => {
            assert!(message.contains("omp auth-gateway token"), "{message}");
            assert!(message.contains(env_name), "{message}");
        }
        other => panic!("expected invalid config, got {other:?}"),
    }
}

#[test]
fn reasoning_effort_for_thinking_maps_known_levels_only() {
    assert_eq!(
        reasoning_effort_for_thinking(Some(" High ")),
        Some("high".to_owned())
    );
    assert_eq!(
        reasoning_effort_for_thinking(Some("xhigh")),
        Some("xhigh".to_owned())
    );
    assert_eq!(reasoning_effort_for_thinking(Some("dynamic")), None);
    assert_eq!(reasoning_effort_for_thinking(Some("off")), None);
    assert_eq!(reasoning_effort_for_thinking(None), None);
}

#[test]
fn resolve_inference_api_key_env_defaults_omp_to_gateway_token() {
    assert_eq!(
        resolve_inference_api_key_env(InferenceProviderKind::Omp, None, None),
        DEFAULT_OMP_GATEWAY_TOKEN_ENV
    );
    assert_eq!(
        resolve_inference_api_key_env(
            InferenceProviderKind::Omp,
            None,
            Some(DEFAULT_GEMINI_API_KEY_ENV.to_owned())
        ),
        DEFAULT_OMP_GATEWAY_TOKEN_ENV
    );
    assert_eq!(
        resolve_inference_api_key_env(
            InferenceProviderKind::Omp,
            Some("MY_TOKEN".to_owned()),
            None
        ),
        "MY_TOKEN"
    );
}

#[test]
fn load_provider_openai_compat_requires_endpoint() {
    let temp = tempdir().expect("tempdir");
    ensure_workspace_config(temp.path()).expect("ensure config");
    let env_name = format!(
        "AETHER_TEST_OPENAI_COMPAT_KEY_ENDPOINT_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    );
    let _env = crate::env_vars::overrides::set(&env_name, "test-key");

    let result = load_provider_from_env_or_mock(
        temp.path(),
        ProviderOverrides {
            provider: Some(InferenceProviderKind::OpenAiCompat),
            model: Some("glm-4.7".to_owned()),
            api_key_env: Some(env_name.clone()),
            ..ProviderOverrides::default()
        },
    );

    match result {
        Err(InferError::MissingEndpoint) => {}
        _ => panic!("expected missing endpoint"),
    }
}

#[test]
fn load_provider_openai_compat_requires_model() {
    let temp = tempdir().expect("tempdir");
    ensure_workspace_config(temp.path()).expect("ensure config");
    let env_name = format!(
        "AETHER_TEST_OPENAI_COMPAT_KEY_MODEL_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    );
    let _env = crate::env_vars::overrides::set(&env_name, "test-key");

    let result = load_provider_from_env_or_mock(
        temp.path(),
        ProviderOverrides {
            provider: Some(InferenceProviderKind::OpenAiCompat),
            endpoint: Some("https://api.example.com/v1".to_owned()),
            api_key_env: Some(env_name.clone()),
            ..ProviderOverrides::default()
        },
    );

    match result {
        Err(InferError::MissingModel) => {}
        _ => panic!("expected missing model"),
    }
}

#[test]
fn openai_compat_provider_follows_thinking_and_attempt_timeout() {
    let build = |thinking: Option<&str>, attempt_timeout_secs: Option<u64>| {
        openai_compat_provider(
            Secret::new("test-key".to_owned()),
            "http://127.0.0.1:9/v1".to_owned(),
            "gpt-6-luna".to_owned(),
            thinking,
            attempt_timeout_secs,
        )
    };

    let lifted = build(Some("max"), Some(400));
    assert_eq!(lifted.reasoning_effort(), Some("max"));
    assert_eq!(lifted.request_timeout_secs(), 410);

    let default = build(None, None);
    assert_eq!(default.reasoning_effort(), None);
    assert_eq!(default.request_timeout_secs(), 120);
    // A short attempt limit never lowers the request timeout.
    assert_eq!(build(Some("low"), Some(30)).request_timeout_secs(), 120);
}

#[test]
fn load_provider_openai_compat_sends_configured_thinking_as_reasoning_effort() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    ensure_workspace_config(workspace).expect("ensure config");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake endpoint");
    let endpoint = format!("http://{}/v1", listener.local_addr().expect("local addr"));
    let env_name = "AETHER_TEST_OPENAI_COMPAT_THINKING_KEY_ZZZZZ";
    let _env = crate::env_vars::overrides::set(env_name, "test-key");
    std::fs::write(
        workspace.join(".aether/config.toml"),
        format!(
            "[inference]\nprovider = \"openai_compat\"\nmodel = \"gpt-6-luna\"\n\
             endpoint = \"{endpoint}\"\napi_key_env = \"{env_name}\"\nthinking = \"max\"\n"
        ),
    )
    .expect("write config");

    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept request");
        let body = read_http_request_body(&mut stream);
        let sir = r#"{"intent":"Run the demo","inputs":[],"outputs":[],"side_effects":[],"dependencies":[],"error_modes":[],"confidence":0.9}"#;
        let reply = serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": sir}}]
        })
        .to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
            reply.len()
        )
        .expect("write response");
        body
    });

    let loaded = load_provider_from_env_or_mock(workspace, ProviderOverrides::default())
        .expect("openai_compat provider should load");
    let context = crate::types::SirContext {
        language: "rust".to_owned(),
        file_path: "src/lib.rs".to_owned(),
        qualified_name: "demo::run".to_owned(),
        priority_score: None,
        kind: "function".to_owned(),
        is_public: false,
        line_count: 1,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let sir = runtime
        .block_on(loaded.provider.generate_sir("fn run() {}", &context))
        .expect("generate sir");
    assert_eq!(sir.intent, "Run the demo");

    let body: serde_json::Value =
        serde_json::from_slice(&server.join().expect("join fake endpoint")).expect("json body");
    assert_eq!(body["model"], "gpt-6-luna");
    assert_eq!(body["reasoning_effort"], "max");
}

/// Read one HTTP/1.1 request from `stream` and return its body.
fn read_http_request_body(stream: &mut std::net::TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut buffer).expect("read request");
        assert!(
            read > 0,
            "connection closed before the request headers ended"
        );
        request.extend_from_slice(&buffer[..read]);
        if let Some(position) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let headers = String::from_utf8_lossy(&request[..header_end]).to_ascii_lowercase();
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .expect("content-length header");
    while request.len() < header_end + content_length {
        let read = stream.read(&mut buffer).expect("read request body");
        assert!(read > 0, "connection closed before the request body ended");
        request.extend_from_slice(&buffer[..read]);
    }
    request[header_end..header_end + content_length].to_vec()
}

#[test]
fn load_reranker_provider_defaults_to_none() {
    let temp = tempdir().expect("tempdir");
    ensure_workspace_config(temp.path()).expect("ensure config");

    let loaded =
        load_reranker_provider_from_config(temp.path(), RerankerProviderOverrides::default())
            .expect("load reranker provider");
    assert!(loaded.is_none());
}

#[test]
fn load_reranker_provider_reads_enabled_candle_settings() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    ensure_workspace_config(workspace).expect("ensure config");

    std::fs::write(
        workspace.join(".aether/config.toml"),
        r#"[search]
reranker = "candle"

[search.candle]
model_dir = ".aether/models"
"#,
    )
    .expect("write config");

    let loaded =
        load_reranker_provider_from_config(workspace, RerankerProviderOverrides::default())
            .expect("load reranker provider")
            .expect("reranker provider should be enabled");

    assert_eq!(loaded.provider_name, SearchRerankerKind::Candle.as_str());
    assert_eq!(loaded.model_name, "qwen3-reranker-0.6b");
}

#[test]
fn load_reranker_provider_requires_cohere_api_key() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    ensure_workspace_config(workspace).expect("ensure config");

    std::fs::write(
        workspace.join(".aether/config.toml"),
        r#"[search]
reranker = "cohere"
"#,
    )
    .expect("write config");

    let env_name = format!(
        "AETHER_TEST_COHERE_KEY_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    );
    let _env = crate::env_vars::overrides::unset(&env_name);

    let result = load_reranker_provider_from_config(
        workspace,
        RerankerProviderOverrides {
            cohere_api_key_env: Some(env_name.clone()),
            ..RerankerProviderOverrides::default()
        },
    );

    match result {
        Err(InferError::MissingCohereApiKey(var)) => assert_eq!(var, env_name),
        _ => panic!("expected missing cohere key error"),
    }
}
