//! Whether a model id a provider calls is one its service lists, for `aetherd doctor`.
//! Listing models is the only request made here. Nothing is generated or embedded, so a
//! check spends no tokens.

use aether_core::Secret;
use serde_json::Value;

use crate::http::{fetch_ollama_tags, normalize_ollama_endpoint, normalize_openai_api_base};
use crate::types::InferError;

/// Most pages of the Gemini model list read before giving up.
const MAX_GEMINI_MODEL_PAGES: usize = 20;

/// A service that lists the model ids it serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelCatalog {
    /// The Gemini API: `GET {api_base}/models`, paged.
    Gemini { api_base: String, api_key: Secret },
    /// An Ollama server: `GET {endpoint}/api/tags`, the models pulled there.
    Ollama { endpoint: String },
    /// An OpenAI-compatible API, the omp gateway included: `GET {api_base}/models`.
    OpenAiCompat { api_base: String, api_key: Secret },
}

/// A model id a provider calls, and the catalog that should list it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelListing {
    pub model: String,
    pub catalog: ModelCatalog,
}

impl ModelListing {
    pub fn new(model: impl Into<String>, catalog: ModelCatalog) -> Self {
        Self {
            model: model.into(),
            catalog,
        }
    }
}

impl ModelCatalog {
    /// Where the list comes from, without credentials.
    pub fn describe(&self) -> String {
        match self {
            Self::Gemini { api_base, .. } => format!("Gemini API at {api_base}"),
            Self::Ollama { endpoint } => {
                format!("Ollama at {}", normalize_ollama_endpoint(endpoint))
            }
            Self::OpenAiCompat { api_base, .. } => {
                format!(
                    "OpenAI-compatible API at {}",
                    normalize_openai_api_base(api_base)
                )
            }
        }
    }

    /// The ids the service lists. Gemini's `models/` prefix is removed.
    pub async fn list_model_ids(&self) -> Result<Vec<String>, InferError> {
        let client = listing_http_client();
        match self {
            Self::Gemini { api_base, api_key } => {
                let url = format!("{}/models", api_base.trim_end_matches('/'));
                let mut ids = Vec::new();
                let mut page_token: Option<String> = None;
                for _ in 0..MAX_GEMINI_MODEL_PAGES {
                    let mut request = client
                        .get(&url)
                        .header("x-goog-api-key", api_key.expose())
                        .query(&[("pageSize", "1000")]);
                    if let Some(token) = page_token.as_deref() {
                        request = request.query(&[("pageToken", token)]);
                    }
                    let page: Value = request.send().await?.error_for_status()?.json().await?;
                    ids.extend(string_fields(&page, "models", "name").map(|name| {
                        name.strip_prefix("models/")
                            .unwrap_or(name.as_str())
                            .to_owned()
                    }));
                    page_token = page
                        .get("nextPageToken")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|token| !token.is_empty())
                        .map(str::to_owned);
                    if page_token.is_none() {
                        return Ok(ids);
                    }
                }
                Err(InferError::ModelUnavailable(format!(
                    "the Gemini model list ran past {MAX_GEMINI_MODEL_PAGES} pages"
                )))
            }
            Self::Ollama { endpoint } => {
                let tags = fetch_ollama_tags(endpoint).await?;
                Ok(string_fields(&tags, "models", "name").collect())
            }
            Self::OpenAiCompat { api_base, api_key } => {
                let url = format!("{}/models", normalize_openai_api_base(api_base));
                let body: Value = client
                    .get(url)
                    .bearer_auth(api_key.expose())
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await?;
                Ok(string_fields(&body, "data", "id").collect())
            }
        }
    }

    /// Whether `ids`, as [`Self::list_model_ids`] returned them, include `model`. Ollama
    /// reads a name without a tag as `:latest`. A Gemini id must match exactly: requests
    /// put the configured id after their own `/models/`, so `models/<id>` would address
    /// `/models/models/<id>`.
    pub fn lists(&self, model: &str, ids: &[String]) -> bool {
        let model = model.trim();
        let tagged = match self {
            Self::Ollama { .. } if !model.contains(':') => Some(format!("{model}:latest")),
            _ => None,
        };
        ids.iter()
            .any(|id| id == model || tagged.as_deref() == Some(id.as_str()))
    }
}

/// `body[array][*][field]`, for each element that has it as a string.
fn string_fields<'a>(
    body: &'a Value,
    array: &str,
    field: &'a str,
) -> impl Iterator<Item = String> + 'a {
    body.get(array)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(move |entry| entry.get(field).and_then(Value::as_str))
        .map(str::to_owned)
}

fn listing_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use aether_core::Secret;

    use super::ModelCatalog;

    /// Serves one canned JSON body per request, in order, and returns the request lines.
    fn serve(bodies: Vec<&'static str>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let base = format!("http://{}", listener.local_addr().expect("local addr"));
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for body in bodies {
                let (mut stream, _) = listener.accept().expect("accept request");
                let mut buffer = [0_u8; 4096];
                let read = stream.read(&mut buffer).expect("read request");
                requests.push(String::from_utf8_lossy(&buffer[..read]).to_string());
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
                .expect("write response");
            }
            requests
        });
        (base, server)
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
    }

    #[test]
    fn gemini_listing_follows_pages_and_strips_the_models_prefix() {
        let (base, server) = serve(vec![
            r#"{"models":[{"name":"models/gemini-3.1-flash-lite"}],"nextPageToken":"p2"}"#,
            r#"{"models":[{"name":"models/gemini-embedding-2"}]}"#,
        ]);
        let catalog = ModelCatalog::Gemini {
            api_base: format!("{base}/v1beta"),
            api_key: Secret::new("test-key".to_owned()),
        };

        let ids = runtime()
            .block_on(catalog.list_model_ids())
            .expect("list models");
        assert_eq!(ids, ["gemini-3.1-flash-lite", "gemini-embedding-2"]);
        assert!(catalog.lists("gemini-3.1-flash-lite", &ids));
        // Requests would address `/models/models/...` for this form.
        assert!(!catalog.lists("models/gemini-embedding-2", &ids));
        assert!(!catalog.lists("gemini-3.1-flash-lite-preview", &ids));

        let requests = server.join().expect("server");
        // Only the list endpoint is called; nothing is generated.
        assert!(requests[0].starts_with("GET /v1beta/models?pageSize=1000 "));
        assert!(requests[1].starts_with("GET /v1beta/models?pageSize=1000&pageToken=p2 "));
        for request in &requests {
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("x-goog-api-key: test-key")
            );
            assert!(!request.contains("generateContent"));
        }
    }

    #[test]
    fn ollama_listing_reads_tags_and_matches_untagged_names_as_latest() {
        let (base, server) = serve(vec![
            r#"{"models":[{"name":"qwen3.5:4b"},{"name":"nomic-embed-text:latest"}]}"#,
        ]);
        let catalog = ModelCatalog::Ollama {
            endpoint: format!("{base}/api/embeddings"),
        };

        let ids = runtime()
            .block_on(catalog.list_model_ids())
            .expect("list models");
        assert!(catalog.lists("qwen3.5:4b", &ids));
        assert!(catalog.lists("nomic-embed-text", &ids));
        assert!(!catalog.lists("qwen3.5", &ids));
        assert!(!catalog.lists("qwen3.5:9b", &ids));

        let requests = server.join().expect("server");
        assert!(requests[0].starts_with("GET /api/tags "));
    }

    #[test]
    fn openai_compatible_listing_reads_data_ids() {
        let (base, server) = serve(vec![r#"{"data":[{"id":"anthropic/claude-fable-5"}]}"#]);
        let catalog = ModelCatalog::OpenAiCompat {
            api_base: format!("{base}/v1/chat/completions"),
            api_key: Secret::new("token".to_owned()),
        };

        let ids = runtime()
            .block_on(catalog.list_model_ids())
            .expect("list models");
        assert!(catalog.lists("anthropic/claude-fable-5", &ids));
        assert!(!catalog.lists("claude-fable-5", &ids));
        assert_eq!(
            catalog.describe(),
            format!("OpenAI-compatible API at {base}/v1")
        );

        let requests = server.join().expect("server");
        assert!(requests[0].starts_with("GET /v1/models "));
        assert!(
            requests[0]
                .to_ascii_lowercase()
                .contains("authorization: bearer token")
        );
    }
}
