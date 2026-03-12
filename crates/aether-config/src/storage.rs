use serde::{Deserialize, Serialize};

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
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            mirror_sir_files: default_mirror_sir_files(),
            graph_backend: default_graph_backend(),
        }
    }
}

fn default_mirror_sir_files() -> bool {
    true
}

fn default_graph_backend() -> GraphBackend {
    GraphBackend::Surreal
}
