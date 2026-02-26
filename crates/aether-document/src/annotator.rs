use async_trait::async_trait;

use crate::{GenericRecord, GenericUnit, Result};

#[async_trait]
pub trait SemanticAnnotator: Send + Sync {
    fn domain(&self) -> &str;
    async fn annotate(&self, unit: &GenericUnit) -> Result<GenericRecord>;
    async fn summarize(&self, units: &[GenericUnit], records: &[GenericRecord]) -> Result<GenericRecord>;
}
