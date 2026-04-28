use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct LatchConfig {
    pub compression: Option<CompressionConfig>,
    pub cache: Option<CacheConfig>,
    pub router: Option<RouterConfig>,
    pub retry: Option<RetryConfig>,
    pub meter: Option<MeterConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompressionConfig {
    pub strategy: CompressionStrategy,
    pub max_turns: usize,
    pub summarization_model: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CompressionStrategy {
    SlidingWindow,
    Summarization,
    Dedup,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CacheConfig {
    pub prompt_caching: PromptCachingConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PromptCachingConfig {
    pub enabled: bool,
    pub provider: PromptCacheProvider,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PromptCacheProvider {
    Anthropic,
    OpenAiCompatible,
    None,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouterConfig {
    pub fast_pool: String,
    pub strong_pool: String,
    pub confidence_threshold: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RetryConfig {
    pub max_attempts: usize,
    pub backoff_ms: u64,
    pub max_backoff_ms: Option<u64>,
    pub fallback_provider: Option<String>,
    pub circuit_breaker: Option<CircuitBreakerConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    pub failure_threshold: usize,
    pub open_ms: u64,
    pub half_open_max_attempts: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MeterConfig {
    pub session_token_limit: Option<u64>,
    pub session_request_limit: Option<u64>,
    pub price_per_1k_input_tokens: f64,
    pub price_per_1k_output_tokens: f64,
    pub currency: String,
}
