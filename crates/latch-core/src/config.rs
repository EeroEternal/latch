use serde::{Deserialize, Serialize};

use crate::TokenEstimator;

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct LatchConfig {
    pub compression: Option<CompressionConfig>,
    pub cache: Option<CacheConfig>,
    pub router: Option<RouterConfig>,
    pub retry: Option<RetryConfig>,
    pub meter: Option<MeterConfig>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CompressionConfig {
    /// Below this token count, no compression is applied
    pub min_tokens_to_compress: usize,
    /// SlidingWindow: maximum number of turns to keep
    pub max_turns: usize,
    /// SlidingWindow: maximum system prompt tokens (0 = no limit)
    pub max_system_tokens: usize,
    /// Compression strategy to use
    pub strategy: CompressionStrategy,
    /// Dedup: maximum merged chars for adjacent same-role messages (0 = no limit)
    pub dedup_max_merged_chars: usize,
    /// Injectable token estimator (skipped during serialization)
    #[serde(skip, default = "super::default_token_estimator")]
    pub token_estimator: TokenEstimator,
}

impl std::fmt::Debug for CompressionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompressionConfig")
            .field("min_tokens_to_compress", &self.min_tokens_to_compress)
            .field("max_turns", &self.max_turns)
            .field("max_system_tokens", &self.max_system_tokens)
            .field("strategy", &self.strategy)
            .field("dedup_max_merged_chars", &self.dedup_max_merged_chars)
            .field("token_estimator", &"<TokenEstimator>")
            .finish()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CompressionStrategy {
    None,
    SlidingWindow,
    Dedup,
    DedupThenWindow,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CacheConfig {
    pub prompt_caching: PromptCachingConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default = "PromptCachingConfig::default")]
pub struct PromptCachingConfig {
    pub enabled: bool,
    pub provider: PromptCacheProvider,
    /// Roles that should receive cache markers (default: ["system"])
    pub cache_roles: Vec<String>,
    /// Minimum content length for caching to be meaningful (default: 0)
    pub min_content_chars: usize,
}

impl Default for PromptCachingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: PromptCacheProvider::default(), // None
            cache_roles: vec!["system".to_string()],
            min_content_chars: 0,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum PromptCacheProvider {
    #[default]
    None,
    Anthropic,
    OpenAiCompatible,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RouterConfig {
    /// All available pool definitions
    pub pools: Vec<PoolRoute>,
    /// Return Uncertain if confidence is below this threshold
    pub confidence_threshold: f32,
    /// Requests with more tokens than this get Premium preference
    pub long_request_tokens: usize,
    /// Penalty multiplier per failure: 1 failure → 0.8, 2 → 0.64, etc.
    #[serde(default = "default_penalty_per_failure")]
    pub penalty_per_failure: f32,
    /// Injectable token estimator (skipped during serialization)
    #[serde(skip, default = "super::default_token_estimator")]
    pub token_estimator: TokenEstimator,
}

fn default_penalty_per_failure() -> f32 {
    0.8
}

impl std::fmt::Debug for RouterConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouterConfig")
            .field("pools", &self.pools)
            .field("confidence_threshold", &self.confidence_threshold)
            .field("long_request_tokens", &self.long_request_tokens)
            .field("token_estimator", &"<TokenEstimator>")
            .finish()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolRoute {
    /// Pool identifier, corresponds to ugate's pool_id
    pub pool_id: String,
    /// Pool tier (affects scoring)
    pub tier: PoolTier,
    /// Priority weight when matching (0.0-1.0)
    pub weight: f32,
    /// Keywords that increase this pool's score when matched
    pub match_keywords: Vec<String>,
    /// Score bonus per matched keyword
    pub keyword_score: f32,
    /// Whether this pool is suitable for images (None = no restriction)
    pub images: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PoolTier {
    Fast,
    Standard,
    Premium,
    Backup,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolFeedback {
    /// Number of recent failures for each pool
    #[serde(default)]
    pub recent_failures: std::collections::HashMap<String, u32>,
    /// Endpoint scores from latch-score (endpoint_id -> score in 0.0..=100.0)
    #[serde(default)]
    pub endpoint_scores: std::collections::HashMap<String, f64>,
}

impl Default for PoolFeedback {
    fn default() -> Self {
        PoolFeedback {
            recent_failures: std::collections::HashMap::new(),
            endpoint_scores: std::collections::HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RetryConfig {
    pub max_attempts: usize,
    pub backoff_ms: u64,
    pub max_backoff_ms: Option<u64>,
    pub fallback_provider: Option<String>,
    pub circuit_breaker: Option<CircuitBreakerConfig>,
    /// Jitter range ratio 0.0-1.0. 0.2 means random factor in [0.8, 1.2]
    #[serde(default = "default_jitter_ratio")]
    pub jitter_ratio: f64,
}

fn default_jitter_ratio() -> f64 {
    0.2
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

