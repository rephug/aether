use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::DynError;

pub const DEFAULT_CONFIG_FILENAME: &str = "aether-query.toml";

#[derive(Debug, Clone, Deserialize, Default)]
pub struct QueryConfig {
    #[serde(default)]
    pub query: QuerySection,
    #[serde(default)]
    pub staleness: StalenessSection,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QuerySection {
    #[serde(default = "default_index_path")]
    pub index_path: PathBuf,
    #[serde(default = "default_bind")]
    pub bind_address: String,
    #[serde(default)]
    pub auth_token: String,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_queries: usize,
    #[serde(default = "default_timeout")]
    pub read_timeout_ms: u64,
}

impl Default for QuerySection {
    fn default() -> Self {
        Self {
            index_path: default_index_path(),
            bind_address: default_bind(),
            auth_token: String::new(),
            max_concurrent_queries: default_max_concurrent(),
            read_timeout_ms: default_timeout(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct StalenessSection {
    #[serde(default = "default_warn_minutes")]
    pub warn_after_minutes: u64,
}

impl Default for StalenessSection {
    fn default() -> Self {
        Self {
            warn_after_minutes: default_warn_minutes(),
        }
    }
}

fn default_index_path() -> PathBuf {
    PathBuf::from(".")
}

fn default_bind() -> String {
    "127.0.0.1:9731".to_owned()
}

fn default_max_concurrent() -> usize {
    32
}

fn default_timeout() -> u64 {
    5_000
}

fn default_warn_minutes() -> u64 {
    30
}

pub fn load_query_config() -> Result<QueryConfig, DynError> {
    load_query_config_from_path(Path::new(DEFAULT_CONFIG_FILENAME))
}

pub fn load_query_config_from_path(path: &Path) -> Result<QueryConfig, DynError> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(QueryConfig::default()),
        Err(err) => return Err(Box::new(err)),
    };

    if contents.trim().is_empty() {
        return Ok(QueryConfig::default());
    }

    Ok(toml::from_str(&contents)?)
}

pub fn apply_serve_overrides(
    config: &mut QueryConfig,
    index_path: Option<PathBuf>,
    bind_address: Option<String>,
    auth_token: Option<String>,
) {
    if let Some(index_path) = index_path {
        config.query.index_path = index_path;
    }
    if let Some(bind_address) = bind_address {
        config.query.bind_address = bind_address;
    }
    if let Some(auth_token) = auth_token {
        config.query.auth_token = auth_token;
    }
}

pub fn apply_client_overrides(
    config: &mut QueryConfig,
    bind_address: Option<String>,
    auth_token: Option<String>,
) {
    if let Some(bind_address) = bind_address {
        config.query.bind_address = bind_address;
    }
    if let Some(auth_token) = auth_token {
        config.query.auth_token = auth_token;
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{
        QueryConfig, apply_client_overrides, apply_serve_overrides, load_query_config_from_path,
    };

    #[test]
    fn query_config_toml_defaults_and_overrides() {
        let config: QueryConfig = toml::from_str(
            r#"
[query]
bind_address = "0.0.0.0:9000"
max_concurrent_queries = 8

[staleness]
warn_after_minutes = 10
"#,
        )
        .expect("parse query config");

        assert_eq!(config.query.index_path, std::path::PathBuf::from("."));
        assert_eq!(config.query.bind_address, "0.0.0.0:9000");
        assert_eq!(config.query.max_concurrent_queries, 8);
        assert_eq!(config.query.read_timeout_ms, 5_000);
        assert_eq!(config.staleness.warn_after_minutes, 10);
    }

    #[test]
    fn query_config_defaults_when_file_missing_or_empty() {
        let temp = tempdir().expect("tempdir");
        let missing = temp.path().join("missing.toml");
        let empty = temp.path().join("empty.toml");
        fs::write(&empty, "").expect("write empty file");

        let missing_cfg = load_query_config_from_path(&missing).expect("missing defaults");
        let empty_cfg = load_query_config_from_path(&empty).expect("empty defaults");

        assert_eq!(missing_cfg.query.index_path, std::path::PathBuf::from("."));
        assert_eq!(missing_cfg.query.bind_address, "127.0.0.1:9731");
        assert!(missing_cfg.query.auth_token.is_empty());
        assert_eq!(empty_cfg.query.max_concurrent_queries, 32);
        assert_eq!(empty_cfg.staleness.warn_after_minutes, 30);
    }

    #[test]
    fn cli_overrides_apply() {
        let mut config = QueryConfig::default();
        apply_serve_overrides(
            &mut config,
            Some(std::path::PathBuf::from("/workspace")),
            Some("127.0.0.1:9999".to_owned()),
            Some("secret".to_owned()),
        );
        assert_eq!(
            config.query.index_path,
            std::path::PathBuf::from("/workspace")
        );
        assert_eq!(config.query.bind_address, "127.0.0.1:9999");
        assert_eq!(config.query.auth_token, "secret");

        apply_client_overrides(
            &mut config,
            Some("127.0.0.1:1111".to_owned()),
            Some("new-secret".to_owned()),
        );
        assert_eq!(config.query.bind_address, "127.0.0.1:1111");
        assert_eq!(config.query.auth_token, "new-secret");
    }
}
