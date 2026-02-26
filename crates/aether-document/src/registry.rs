use std::collections::HashMap;
use std::sync::Arc;

use crate::{DocumentParser, EdgeTypeRegistry, SemanticAnnotator};

#[derive(Default)]
pub struct VerticalRegistry {
    parsers_by_domain: HashMap<String, Arc<dyn DocumentParser>>,
    annotators_by_domain: HashMap<String, Arc<dyn SemanticAnnotator>>,
    edge_types_by_domain: HashMap<String, Arc<dyn EdgeTypeRegistry>>,
    parsers_by_extension: HashMap<String, Arc<dyn DocumentParser>>,
}

impl VerticalRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_parser(&mut self, parser: Arc<dyn DocumentParser>) {
        let domain_key = normalize_domain(parser.domain());
        for extension in parser.supported_extensions() {
            self.parsers_by_extension
                .insert(normalize_extension(extension), parser.clone());
        }
        self.parsers_by_domain.insert(domain_key, parser);
    }

    pub fn register_annotator(&mut self, annotator: Arc<dyn SemanticAnnotator>) {
        let key = normalize_domain(annotator.domain());
        self.annotators_by_domain.insert(key, annotator);
    }

    pub fn register_edge_types(&mut self, registry: Arc<dyn EdgeTypeRegistry>) {
        let key = normalize_domain(registry.domain());
        self.edge_types_by_domain.insert(key, registry);
    }

    pub fn parser_for_domain(&self, domain: &str) -> Option<Arc<dyn DocumentParser>> {
        self.parsers_by_domain.get(&normalize_domain(domain)).cloned()
    }

    pub fn parser_for_extension(&self, extension: &str) -> Option<Arc<dyn DocumentParser>> {
        self.parsers_by_extension
            .get(&normalize_extension(extension))
            .cloned()
    }

    pub fn annotator_for_domain(&self, domain: &str) -> Option<Arc<dyn SemanticAnnotator>> {
        self.annotators_by_domain
            .get(&normalize_domain(domain))
            .cloned()
    }
}

fn normalize_domain(domain: &str) -> String {
    domain.trim().to_ascii_lowercase()
}

fn normalize_extension(extension: &str) -> String {
    extension
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::json;

    use crate::{DocumentEdge, DocumentParser, GenericRecord, GenericUnit, Result, SemanticAnnotator};

    use super::VerticalRegistry;

    struct MockParser;

    #[async_trait]
    impl DocumentParser for MockParser {
        fn domain(&self) -> &str {
            "Docs"
        }

        fn supported_extensions(&self) -> &'static [&'static str] {
            &["md", ".txt"]
        }

        async fn parse(&self, _path: &Path, _content: &str) -> Result<Vec<GenericUnit>> {
            Ok(Vec::new())
        }

        async fn extract_edges(&self, _units: &[GenericUnit]) -> Result<Vec<DocumentEdge>> {
            Ok(Vec::new())
        }
    }

    struct MockAnnotator;

    #[async_trait]
    impl SemanticAnnotator for MockAnnotator {
        fn domain(&self) -> &str {
            "docs"
        }

        async fn annotate(&self, unit: &GenericUnit) -> Result<GenericRecord> {
            GenericRecord::new(
                unit.unit_id.clone(),
                "docs",
                "entity",
                "v1",
                json!({"name":"demo"}),
                unit.content.clone(),
            )
        }

        async fn summarize(&self, _units: &[GenericUnit], _records: &[GenericRecord]) -> Result<GenericRecord> {
            GenericRecord::new(
                "summary",
                "docs",
                "summary",
                "v1",
                json!({"summary":"ok"}),
                "summary",
            )
        }
    }

    #[test]
    fn registry_registers_and_looks_up_parser_and_annotator() {
        let mut registry = VerticalRegistry::new();
        registry.register_parser(Arc::new(MockParser));
        registry.register_annotator(Arc::new(MockAnnotator));

        assert!(registry.parser_for_domain("docs").is_some());
        assert!(registry.parser_for_domain("DOCS").is_some());
        assert!(registry.parser_for_extension("md").is_some());
        assert!(registry.parser_for_extension(".txt").is_some());
        assert!(registry.annotator_for_domain("docs").is_some());
        assert!(registry.parser_for_domain("unregistered").is_none());
    }
}
