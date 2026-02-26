use std::path::Path;

use async_trait::async_trait;

use crate::{DocumentEdge, GenericUnit, Result};

#[async_trait]
pub trait DocumentParser: Send + Sync {
    fn domain(&self) -> &str;
    fn supported_extensions(&self) -> &'static [&'static str];
    async fn parse(&self, path: &Path, content: &str) -> Result<Vec<GenericUnit>>;
    async fn extract_edges(&self, units: &[GenericUnit]) -> Result<Vec<DocumentEdge>>;
}
