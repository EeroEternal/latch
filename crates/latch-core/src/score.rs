use serde::{Deserialize, Serialize};
use std::time::SystemTime;

// ── Observation types ──

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequestObservation {
    pub endpoint_id: String,
    pub pool_id: String,
    pub started_at: SystemTime,
    pub success: bool,
    pub error: Option<ObservationError>,
    pub was_retry: bool,
    pub latency: LatencyBreakdown,
    pub tokens: TokenStats,
    pub stream: Option<StreamMetrics>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LatencyBreakdown {
    pub total_ms: u64,
    pub ttft_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TokenStats {
    pub input: u64,
    pub output: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreamMetrics {
    pub ttft_ms: u64,
    pub tokens_per_second: Option<f64>,
    pub max_inter_chunk_ms: Option<u64>,
    pub chunk_count: u64,
    pub completed_normally: bool,
    pub stream_broken: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ObservationError {
    Timeout,
    RateLimited,
    Upstream5xx,
    Upstream4xx,
    ConnectionFailure,
    EmptyResponse,
    TruncatedStream,
    InvalidResponse,
    Other { code: u16, message: String },
}

// ── Scoring config ──

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScoreConfig {
    pub window_size: usize,
    pub decay_period_secs: u64,
    pub baseline_score: f64,
    pub availability_weight: f64,
    pub latency_weight: f64,
    pub quality_weight: f64,
    pub cost_weight: f64,
    pub good_ttft_ms: u64,
    pub acceptable_ttft_ms: u64,
    pub good_tps: f64,
    pub max_error_rate: f64,
    pub max_truncation_rate: f64,
    pub max_empty_response_rate: f64,
}

impl Default for ScoreConfig {
    fn default() -> Self {
        Self {
            window_size: 100,
            decay_period_secs: 300,
            baseline_score: 60.0,
            availability_weight: 0.35,
            latency_weight: 0.25,
            quality_weight: 0.25,
            cost_weight: 0.15,
            good_ttft_ms: 500,
            acceptable_ttft_ms: 2000,
            good_tps: 50.0,
            max_error_rate: 0.20,
            max_truncation_rate: 0.10,
            max_empty_response_rate: 0.05,
        }
    }
}

// ── Scoring results ──

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EndpointScore {
    pub endpoint_id: String,
    pub pool_id: String,
    pub score: f64,
    pub tier: ScoreTier,
    pub observation_count: usize,
    pub breakdown: ScoreBreakdown,
    pub excluded: bool,
    pub exclusion_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ScoreTier {
    Poor,    // < 40
    Bronze,  // 40-69
    Silver,  // 70-89
    Gold,    // >= 90
}

// Note: ScoreTier ordering: Poor < Bronze < Silver < Gold
impl PartialOrd for ScoreTier {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScoreTier {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let self_val = match self {
            ScoreTier::Poor => 0,
            ScoreTier::Bronze => 1,
            ScoreTier::Silver => 2,
            ScoreTier::Gold => 3,
        };
        let other_val = match other {
            ScoreTier::Poor => 0,
            ScoreTier::Bronze => 1,
            ScoreTier::Silver => 2,
            ScoreTier::Gold => 3,
        };
        self_val.cmp(&other_val)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScoreBreakdown {
    pub availability: f64,
    pub latency: f64,
    pub quality: f64,
    pub cost: f64,
    pub penalty: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolRanking {
    pub pool_id: String,
    pub ranked_endpoints: Vec<EndpointScore>,
    pub recommended: Option<EndpointScore>,
    pub recommended_fallback: Option<EndpointScore>,
    pub excluded_endpoints: Vec<EndpointScore>,
}

/// Injectable clock for testability.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}

/// Default clock that delegates to `SystemTime::now()`.
#[derive(Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}
