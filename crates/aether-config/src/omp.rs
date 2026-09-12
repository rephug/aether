//! Oh My Pi (omp) model routes and the batch-pricing map.
//!
//! omp addresses models as `provider/model` routes (for example
//! `anthropic/claude-fable-5` or `openai-codex/gpt-5.6-sol`). Requests that go through the
//! omp auth gateway are billed on whatever credential omp holds for that provider, which is
//! normally a subscription login. Batch pricing is a different thing: it is only offered by a
//! provider's own batch API, needs that provider's API key, and is never available through the
//! gateway. This module maps between the two worlds so a single route string can drive both.

/// Split an omp route (`provider/model`) on its first slash.
///
/// Several omp namespaces carry a slash inside the model id (for example provider `nanogpt`,
/// id `anthropic/claude-fable-5`), so only the first slash is a separator.
pub fn split_omp_route(route: &str) -> Option<(&str, &str)> {
    let route = route.trim();
    let index = route.find('/')?;
    let (provider, model) = (&route[..index], &route[index + 1..]);
    if provider.is_empty() || model.is_empty() {
        return None;
    }
    Some((provider, model))
}

/// Batch API providers AETHER can submit to, with the pricing note surfaced in diagnostics.
pub const BATCH_PRICING_PROVIDERS: [(&str, &str); 3] = [
    (
        "anthropic",
        "Message Batches API, 50% off standard token prices",
    ),
    ("openai", "Batch API, 50% off standard token prices"),
    ("gemini", "Batch Mode, 50% off standard token prices"),
];

/// Map an omp provider namespace to the AETHER batch provider that offers batch pricing for it.
///
/// Returns `None` for providers with no batch API (subscription-only or local routes such as
/// `openai-codex`, `opencode-go`, `nanogpt`, `ollama`).
pub fn batch_provider_for_omp_provider(provider: &str) -> Option<&'static str> {
    match provider.trim().to_ascii_lowercase().as_str() {
        "anthropic" => Some("anthropic"),
        "openai" | "azure-openai" => Some("openai"),
        "google" | "gemini" | "google-gemini" | "google-vertex" => Some("gemini"),
        _ => None,
    }
}

/// Resolve which batch provider (and bare model id) an omp route maps to.
///
/// `openai-codex/...` deliberately maps to nothing: the Codex route is a ChatGPT
/// subscription login and OpenAI's batch API bills an API key, so the two are not the same
/// route even when the model name matches.
pub fn batch_target_for_omp_route(route: &str) -> Option<(&'static str, &str)> {
    let (provider, model) = split_omp_route(route)?;
    let batch_provider = batch_provider_for_omp_provider(provider)?;
    Some((batch_provider, model))
}

/// Strip an omp provider prefix from a model string when it belongs to `batch_provider`.
///
/// `anthropic/claude-fable-5` becomes `claude-fable-5` for the anthropic batch provider, while
/// a plain `claude-fable-5` or a route for a different provider is returned unchanged so the
/// provider can reject it with its own error.
pub fn strip_omp_route_prefix<'a>(model: &'a str, batch_provider: &str) -> &'a str {
    match split_omp_route(model) {
        Some((provider, bare))
            if batch_provider_for_omp_provider(provider) == Some(batch_provider) =>
        {
            bare
        }
        _ => model,
    }
}

/// Human-readable list of providers that offer batch pricing, for error messages.
pub fn batch_pricing_providers_list() -> String {
    BATCH_PRICING_PROVIDERS
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_omp_route_splits_on_first_slash_only() {
        assert_eq!(
            split_omp_route("anthropic/claude-fable-5"),
            Some(("anthropic", "claude-fable-5"))
        );
        assert_eq!(
            split_omp_route("nanogpt/anthropic/claude-fable-5"),
            Some(("nanogpt", "anthropic/claude-fable-5"))
        );
        assert_eq!(
            split_omp_route(" openai-codex/gpt-5.6-sol "),
            Some(("openai-codex", "gpt-5.6-sol"))
        );
    }

    #[test]
    fn split_omp_route_rejects_bare_names_and_empty_parts() {
        assert_eq!(split_omp_route("claude-fable-5"), None);
        assert_eq!(split_omp_route("/model"), None);
        assert_eq!(split_omp_route("anthropic/"), None);
        assert_eq!(split_omp_route(""), None);
    }

    #[test]
    fn batch_target_maps_only_providers_with_batch_apis() {
        assert_eq!(
            batch_target_for_omp_route("anthropic/claude-fable-5"),
            Some(("anthropic", "claude-fable-5"))
        );
        assert_eq!(
            batch_target_for_omp_route("openai/gpt-5.6-sol"),
            Some(("openai", "gpt-5.6-sol"))
        );
        assert_eq!(
            batch_target_for_omp_route("google/gemini-3.1-flash-lite-preview"),
            Some(("gemini", "gemini-3.1-flash-lite-preview"))
        );
        assert_eq!(batch_target_for_omp_route("openai-codex/gpt-5.6-sol"), None);
        assert_eq!(
            batch_target_for_omp_route("opencode-go/deepseek-v4-flash"),
            None
        );
        assert_eq!(batch_target_for_omp_route("ollama/qwen3.5:4b"), None);
        assert_eq!(batch_target_for_omp_route("claude-fable-5"), None);
    }

    #[test]
    fn strip_omp_route_prefix_only_for_matching_provider() {
        assert_eq!(
            strip_omp_route_prefix("anthropic/claude-fable-5", "anthropic"),
            "claude-fable-5"
        );
        assert_eq!(
            strip_omp_route_prefix("anthropic/claude-fable-5", "openai"),
            "anthropic/claude-fable-5"
        );
        assert_eq!(
            strip_omp_route_prefix("claude-fable-5", "anthropic"),
            "claude-fable-5"
        );
        assert_eq!(
            strip_omp_route_prefix("google/gemini-3.1-pro", "gemini"),
            "gemini-3.1-pro"
        );
    }

    #[test]
    fn batch_pricing_list_names_every_provider() {
        assert_eq!(batch_pricing_providers_list(), "anthropic, openai, gemini");
    }
}
