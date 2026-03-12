use aether_config::InferenceProviderKind;
use async_trait::async_trait;

use crate::types::{InferError, InferSirResult, InferenceProvider, SirContext};

pub struct TieredProvider {
    primary: Box<dyn InferenceProvider>,
    fallback: Box<dyn InferenceProvider>,
    threshold: f64,
    retry_with_fallback: bool,
    primary_name: String,
}

impl TieredProvider {
    pub fn new(
        primary: Box<dyn InferenceProvider>,
        fallback: Box<dyn InferenceProvider>,
        threshold: f64,
        retry_with_fallback: bool,
        primary_name: String,
    ) -> Self {
        Self {
            primary,
            fallback,
            threshold,
            retry_with_fallback,
            primary_name,
        }
    }
}

#[async_trait]
impl InferenceProvider for TieredProvider {
    fn provider_name(&self) -> String {
        InferenceProviderKind::Tiered.as_str().to_owned()
    }

    fn model_name(&self) -> String {
        format!(
            "{}|{}",
            self.primary.model_name(),
            self.fallback.model_name()
        )
    }

    async fn generate_sir(
        &self,
        symbol_text: &str,
        context: &SirContext,
    ) -> Result<aether_sir::SirAnnotation, InferError> {
        self.generate_sir_with_meta(symbol_text, context)
            .await
            .map(|result| result.sir)
    }

    async fn generate_sir_with_meta(
        &self,
        symbol_text: &str,
        context: &SirContext,
    ) -> Result<InferSirResult, InferError> {
        let score = context.priority_score.unwrap_or(0.0);
        if score >= self.threshold {
            match self
                .primary
                .generate_sir_with_meta(symbol_text, context)
                .await
            {
                Ok(result) => return Ok(result),
                Err(err) if self.retry_with_fallback => {
                    tracing::warn!(
                        symbol = %context.qualified_name,
                        provider = %self.primary_name,
                        error = %err,
                        "Primary provider failed, falling back to local"
                    );
                }
                Err(err) => return Err(err),
            }
        }

        self.fallback
            .generate_sir_with_meta(symbol_text, context)
            .await
    }

    async fn generate_sir_from_prompt(
        &self,
        prompt: &str,
        context: &SirContext,
        deep_mode: bool,
    ) -> Result<aether_sir::SirAnnotation, InferError> {
        self.generate_sir_from_prompt_with_meta(prompt, context, deep_mode)
            .await
            .map(|result| result.sir)
    }

    async fn generate_sir_from_prompt_with_meta(
        &self,
        prompt: &str,
        context: &SirContext,
        deep_mode: bool,
    ) -> Result<InferSirResult, InferError> {
        let score = context.priority_score.unwrap_or(0.0);
        if score >= self.threshold {
            match self
                .primary
                .generate_sir_from_prompt_with_meta(prompt, context, deep_mode)
                .await
            {
                Ok(result) => return Ok(result),
                Err(err) if self.retry_with_fallback => {
                    tracing::warn!(
                        symbol = %context.qualified_name,
                        provider = %self.primary_name,
                        error = %err,
                        "Primary provider failed on custom prompt, falling back to local"
                    );
                }
                Err(err) => return Err(err),
            }
        }

        self.fallback
            .generate_sir_from_prompt_with_meta(prompt, context, deep_mode)
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    use super::*;

    #[derive(Clone)]
    struct TestProvider {
        intent_prefix: String,
        fail: bool,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl InferenceProvider for TestProvider {
        async fn generate_sir(
            &self,
            _symbol_text: &str,
            context: &SirContext,
        ) -> Result<aether_sir::SirAnnotation, InferError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(InferError::InvalidResponse(
                    "forced provider failure".to_owned(),
                ));
            }
            Ok(aether_sir::SirAnnotation {
                intent: format!("{} {}", self.intent_prefix, context.qualified_name),
                inputs: Vec::new(),
                outputs: Vec::new(),
                side_effects: Vec::new(),
                dependencies: Vec::new(),
                error_modes: Vec::new(),
                confidence: 0.9,
            })
        }
    }

    #[tokio::test]
    async fn tiered_provider_routes_high_score_to_primary() {
        let primary_calls = Arc::new(AtomicUsize::new(0));
        let fallback_calls = Arc::new(AtomicUsize::new(0));
        let provider = TieredProvider::new(
            Box::new(TestProvider {
                intent_prefix: "primary".to_owned(),
                fail: false,
                calls: primary_calls.clone(),
            }),
            Box::new(TestProvider {
                intent_prefix: "fallback".to_owned(),
                fail: false,
                calls: fallback_calls.clone(),
            }),
            0.8,
            true,
            "primary".to_owned(),
        );
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
            priority_score: Some(0.95),
            kind: "function".to_owned(),
            is_public: true,
            line_count: 64,
        };

        let sir = provider
            .generate_sir("fn run() {}", &context)
            .await
            .expect("tiered should succeed");
        assert!(sir.intent.starts_with("primary "));
        assert_eq!(primary_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn tiered_provider_routes_low_score_to_fallback() {
        let primary_calls = Arc::new(AtomicUsize::new(0));
        let fallback_calls = Arc::new(AtomicUsize::new(0));
        let provider = TieredProvider::new(
            Box::new(TestProvider {
                intent_prefix: "primary".to_owned(),
                fail: false,
                calls: primary_calls.clone(),
            }),
            Box::new(TestProvider {
                intent_prefix: "fallback".to_owned(),
                fail: false,
                calls: fallback_calls.clone(),
            }),
            0.8,
            true,
            "primary".to_owned(),
        );
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
            priority_score: Some(0.3),
            kind: "function".to_owned(),
            is_public: false,
            line_count: 8,
        };

        let sir = provider
            .generate_sir("fn run() {}", &context)
            .await
            .expect("tiered should succeed");
        assert!(sir.intent.starts_with("fallback "));
        assert_eq!(primary_calls.load(Ordering::SeqCst), 0);
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn tiered_provider_falls_back_on_primary_error_when_enabled() {
        let primary_calls = Arc::new(AtomicUsize::new(0));
        let fallback_calls = Arc::new(AtomicUsize::new(0));
        let provider = TieredProvider::new(
            Box::new(TestProvider {
                intent_prefix: "primary".to_owned(),
                fail: true,
                calls: primary_calls.clone(),
            }),
            Box::new(TestProvider {
                intent_prefix: "fallback".to_owned(),
                fail: false,
                calls: fallback_calls.clone(),
            }),
            0.8,
            true,
            "primary".to_owned(),
        );
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
            priority_score: Some(0.9),
            kind: "function".to_owned(),
            is_public: true,
            line_count: 64,
        };

        let sir = provider
            .generate_sir("fn run() {}", &context)
            .await
            .expect("tiered fallback should succeed");
        assert!(sir.intent.starts_with("fallback "));
        assert_eq!(primary_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn tiered_provider_propagates_primary_error_when_disabled() {
        let primary_calls = Arc::new(AtomicUsize::new(0));
        let fallback_calls = Arc::new(AtomicUsize::new(0));
        let provider = TieredProvider::new(
            Box::new(TestProvider {
                intent_prefix: "primary".to_owned(),
                fail: true,
                calls: primary_calls.clone(),
            }),
            Box::new(TestProvider {
                intent_prefix: "fallback".to_owned(),
                fail: false,
                calls: fallback_calls.clone(),
            }),
            0.8,
            false,
            "primary".to_owned(),
        );
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
            priority_score: Some(0.9),
            kind: "function".to_owned(),
            is_public: true,
            line_count: 64,
        };

        let err = provider
            .generate_sir("fn run() {}", &context)
            .await
            .expect_err("tiered should propagate primary error");
        match err {
            InferError::InvalidResponse(message) => assert!(message.contains("forced")),
            other => panic!("unexpected error: {other}"),
        }
        assert_eq!(primary_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 0);
    }
}
