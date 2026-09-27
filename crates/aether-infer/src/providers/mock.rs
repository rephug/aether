use aether_config::{InferenceProviderKind, MOCK_INTENT_PREFIX, MOCK_SIR_CONFIDENCE};
use aether_sir::SirAnnotation;
use async_trait::async_trait;

use crate::types::{InferError, InferenceProvider, SirContext};

/// Key-free placeholder provider for zero-API-key onboarding (Phase WF.0, Decision #121).
///
/// It never calls a model: each symbol gets a `[MOCK]` SIR built from tree-sitter facts
/// (kind, qualified name, file, line count) at confidence 0.1, low enough that
/// `aether_audit_candidates` surfaces every unscanned symbol first so a `/scan` session
/// can replace the placeholders at subscription cost.
#[derive(Debug, Clone, Copy, Default)]
pub struct MockInferenceProvider;

impl MockInferenceProvider {
    pub fn new() -> Self {
        Self
    }

    /// The placeholder SIR for a symbol. Public so tests and the pipeline can recognise
    /// mock output by content, not just by provider name.
    pub fn placeholder_sir(context: &SirContext) -> SirAnnotation {
        let kind = context.kind.trim();
        let kind = if kind.is_empty() { "symbol" } else { kind };
        let name = context.qualified_name.trim();
        let name = if name.is_empty() { "<unnamed>" } else { name };
        let file = context.file_path.trim();
        let location = if file.is_empty() {
            String::new()
        } else {
            format!(" in {file}")
        };
        SirAnnotation {
            intent: format!(
                "{MOCK_INTENT_PREFIX} {kind} `{name}`{location} ({} lines); placeholder from \
                 tree-sitter only, no model has read this symbol yet — run /scan to replace",
                context.line_count
            ),
            behavior: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            side_effects: Vec::new(),
            dependencies: Vec::new(),
            error_modes: Vec::new(),
            confidence: MOCK_SIR_CONFIDENCE,
            edge_cases: None,
            complexity: None,
            method_dependencies: None,
        }
    }
}

#[async_trait]
impl InferenceProvider for MockInferenceProvider {
    fn provider_name(&self) -> String {
        InferenceProviderKind::Mock.as_str().to_owned()
    }

    fn model_name(&self) -> String {
        "tree-sitter".to_owned()
    }

    async fn generate_sir(
        &self,
        _symbol_text: &str,
        context: &SirContext,
    ) -> Result<SirAnnotation, InferError> {
        Ok(Self::placeholder_sir(context))
    }

    async fn generate_sir_from_prompt(
        &self,
        _prompt: &str,
        context: &SirContext,
        _deep_mode: bool,
    ) -> Result<SirAnnotation, InferError> {
        Ok(Self::placeholder_sir(context))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_sir::validate_sir;

    fn context() -> SirContext {
        SirContext {
            language: "rust".to_owned(),
            file_path: "crates/demo/src/lib.rs".to_owned(),
            qualified_name: "demo::parse_config".to_owned(),
            priority_score: None,
            kind: "function".to_owned(),
            is_public: true,
            line_count: 42,
        }
    }

    #[tokio::test]
    async fn mock_provider_emits_low_confidence_placeholder() {
        let provider = MockInferenceProvider::new();
        let sir = provider
            .generate_sir("fn parse_config() {}", &context())
            .await
            .expect("mock never fails");
        assert!(
            sir.intent
                .starts_with("[MOCK] function `demo::parse_config`"),
            "{}",
            sir.intent
        );
        assert!((sir.confidence - 0.1).abs() < f32::EPSILON);
        assert!(sir.inputs.is_empty() && sir.error_modes.is_empty());
        validate_sir(&sir).expect("placeholder must validate");
        assert_eq!(provider.provider_name(), "mock");
        let from_prompt = provider
            .generate_sir_from_prompt("ignored", &context(), true)
            .await
            .expect("mock never fails");
        assert_eq!(from_prompt, sir);
    }
}
