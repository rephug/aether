use serde::{Deserialize, Serialize};

/// How long opening the SurrealDB graph store may take when
/// `[storage] graph_open_timeout_secs` is unset.
pub const DEFAULT_GRAPH_OPEN_TIMEOUT_SECS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GraphBackend {
    #[default]
    Surreal,
    Cozo,
    Sqlite,
}

impl GraphBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Surreal => "surreal",
            Self::Cozo => "cozo",
            Self::Sqlite => "sqlite",
        }
    }
}

impl std::str::FromStr for GraphBackend {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "surreal" => Ok(Self::Surreal),
            "cozo" => Ok(Self::Cozo),
            "sqlite" => Ok(Self::Sqlite),
            other => Err(format!(
                "invalid graph backend '{other}', expected one of: surreal, cozo, sqlite"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageConfig {
    #[serde(default = "default_mirror_sir_files")]
    pub mirror_sir_files: bool,
    #[serde(default)]
    pub graph_backend: GraphBackend,
    /// Seconds opening the SurrealDB graph store may take before it fails. Unset or 0 keeps
    /// 60. A busy machine can need more than a few seconds to open the store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_open_timeout_secs: Option<u64>,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            mirror_sir_files: default_mirror_sir_files(),
            graph_backend: default_graph_backend(),
            graph_open_timeout_secs: None,
        }
    }
}

fn default_mirror_sir_files() -> bool {
    true
}

fn default_graph_backend() -> GraphBackend {
    GraphBackend::Surreal
}

#[cfg(test)]
mod tests {
    use super::StorageConfig;

    #[test]
    fn graph_open_timeout_secs_is_optional() {
        let unset: StorageConfig = toml::from_str("graph_backend = \"surreal\"").expect("parse");
        assert_eq!(unset.graph_open_timeout_secs, None);
        assert_eq!(StorageConfig::default().graph_open_timeout_secs, None);

        let set: StorageConfig = toml::from_str("graph_open_timeout_secs = 180").expect("parse");
        assert_eq!(set.graph_open_timeout_secs, Some(180));
    }
}
