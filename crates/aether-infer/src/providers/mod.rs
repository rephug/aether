pub(crate) mod gemini;
pub(crate) mod openai_compat;
pub(crate) mod qwen_local;
pub(crate) mod tiered;

pub use gemini::GeminiProvider;
pub use openai_compat::OpenAiCompatProvider;
pub use qwen_local::Qwen3LocalProvider;
pub use tiered::TieredProvider;

pub(crate) const PARSE_VALIDATION_RETRIES: usize = 2;
