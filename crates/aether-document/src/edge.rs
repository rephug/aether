use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentEdge {
    pub source_id: String,
    pub target_id: String,
    pub edge_type: String,
    pub domain: String,
    pub weight: f32,
    pub metadata: serde_json::Value,
}

pub trait EdgeTypeRegistry: Send + Sync {
    fn domain(&self) -> &str;
    fn valid_edge_types(&self) -> &'static [&'static str];

    fn is_valid(&self, edge_type: &str) -> bool {
        self.valid_edge_types()
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(edge_type))
    }
}
