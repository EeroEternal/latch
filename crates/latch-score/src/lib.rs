use latch_core::{
    config::PoolFeedback,
    score::{
        Clock, EndpointScore, ObservationError, PoolRanking,
        RequestObservation, ScoreBreakdown, ScoreConfig, ScoreTier, SystemClock,
    },
};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::{Duration, SystemTime};

/// Serializable snapshot of the entire scoring engine state.
#[derive(Serialize, Deserialize)]
pub struct ScoreSnapshot {
    pub endpoints: std::collections::HashMap<String, EndpointState>,
    pub last_decay_at_secs: u64,
}

/// Per-endpoint rolling window state.
#[derive(Clone, Serialize, Deserialize)]
pub struct EndpointState {
    observations: VecDeque<RequestObservation>,
    last_score: EndpointScore,
    last_breakdown: ScoreBreakdown,
    consecutive_timeouts: u32,
    consecutive_stream_breaks: u32,
}

impl EndpointState {
    fn new(endpoint_id: String, pool_id: String) -> Self {
        Self {
            observations: VecDeque::new(),
            last_score: EndpointScore {
                endpoint_id,
                pool_id,
                score: 60.0, // baseline
                tier: ScoreTier::Bronze,
                observation_count: 0,
                breakdown: ScoreBreakdown {
                    availability: 1.0,
                    latency: 1.0,
                    quality: 1.0,
                    cost: 1.0,
                    penalty: 0.0,
                },
                excluded: false,
                exclusion_reason: None,
            },
            last_breakdown: ScoreBreakdown {
                availability: 1.0,
                latency: 1.0,
                quality: 1.0,
                cost: 1.0,
                penalty: 0.0,
            },
            consecutive_timeouts: 0,
            consecutive_stream_breaks: 0,
        }
    }
}

/// Scoring engine for calculating endpoint quality scores.
/// Generic over `Clock` for testability.
pub struct ScoringEngine<C: Clock> {
    config: ScoreConfig,
    clock: C,
    endpoints: std::collections::HashMap<String, EndpointState>,
    last_decay_at: SystemTime,
}

impl ScoringEngine<SystemClock> {
    /// Create a new ScoringEngine with the default system clock.
    pub fn new(config: ScoreConfig) -> Self {
        Self::with_clock(config, SystemClock::default())
    }
}

impl<C: Clock> ScoringEngine<C> {
    /// Create a new ScoringEngine with an injectable clock (for testing).
    pub fn with_clock(config: ScoreConfig, clock: C) -> Self {
        Self {
            config,
            clock,
            endpoints: std::collections::HashMap::new(),
            last_decay_at: SystemTime::now(),
        }
    }

    /// Feed a request observation into the engine.
    /// This updates the rolling window and recomputes the endpoint score.
    pub fn observe(&mut self, obs: RequestObservation) {
        let endpoint_id = obs.endpoint_id.clone();
        let pool_id = obs.pool_id.clone();

        // Ensure state exists, then push observation
        let state = self
            .endpoints
            .entry(endpoint_id.clone())
            .or_insert_with(|| EndpointState::new(endpoint_id.clone(), pool_id));

        // Track consecutive failures for exclusion
        if obs.success {
            state.consecutive_timeouts = 0;
            state.consecutive_stream_breaks = 0;
        } else {
            if let Some(ref err) = obs.error {
                if matches!(err, ObservationError::Timeout) {
                    state.consecutive_timeouts += 1;
                }
            }
            if let Some(ref s) = obs.stream {
                if s.stream_broken {
                    state.consecutive_stream_breaks += 1;
                }
            }
        }

        state.observations.push_back(obs);
        while state.observations.len() > self.config.window_size {
            state.observations.pop_front();
        }

        // Drop the mutable borrow of `state` before calling compute_score,
        // which borrows `self` immutably.
        let now = self.clock.now();
        let window = &state.observations;
        let (score, breakdown) =
            Self::compute_score(&self.config, now, window);

        // Re-borrow state to update scores
        let state = self.endpoints.get_mut(&endpoint_id).unwrap();
        state.last_score.score = score;
        state.last_score.tier = Self::score_to_tier(score);
        state.last_score.observation_count = state.observations.len();
        state.last_score.breakdown = breakdown.clone();
        state.last_breakdown = breakdown;

        // Exclusion logic
        if state.consecutive_timeouts >= 3 || state.consecutive_stream_breaks >= 3 {
            state.last_score.excluded = true;
            if state.consecutive_timeouts >= 3 {
                state.last_score.exclusion_reason = Some("consecutive timeouts".to_string());
            } else {
                state.last_score.exclusion_reason = Some("consecutive stream breaks".to_string());
            }
        }
    }

    /// Explicitly trigger time-decay on all endpoint windows.
    /// Host systems should call this periodically (e.g. every 30-60s).
    pub fn decay(&mut self) {
        let now = self.clock.now();
        for state in self.endpoints.values_mut() {
            let (score, breakdown) =
                Self::compute_score(&self.config, now, &state.observations);
            state.last_score.score = score;
            state.last_score.tier = Self::score_to_tier(score);
            state.last_score.observation_count = state.observations.len();
            state.last_score.breakdown = breakdown.clone();
            state.last_breakdown = breakdown;
        }
        self.last_decay_at = now;
    }

    /// Get the latest cached score for an endpoint.
    pub fn get_score(&self, endpoint_id: &str) -> Option<EndpointScore> {
        self.endpoints
            .get(endpoint_id)
            .map(|s| s.last_score.clone())
    }

    /// Get the latest breakdown for an endpoint.
    pub fn get_breakdown(&self, endpoint_id: &str) -> Option<ScoreBreakdown> {
        self.endpoints
            .get(endpoint_id)
            .map(|s| s.last_breakdown.clone())
    }

    // --- Internal helpers ---

    fn compute_score(
        config: &ScoreConfig,
        now: SystemTime,
        window: &VecDeque<RequestObservation>,
    ) -> (f64, ScoreBreakdown) {
        if window.is_empty() {
            return (config.baseline_score, ScoreBreakdown {
                availability: 1.0,
                latency: 1.0,
                quality: 1.0,
                cost: 1.0,
                penalty: 0.0,
            });
        }

        // --- Time-decayed weighting ---
        let lambda = (2.0f64).ln() / config.decay_period_secs as f64;
        let mut total_weight: f64 = 0.0;
        let mut weighted_success: f64 = 0.0;
        let mut weighted_latency: f64 = 0.0;
        let mut latency_count: f64 = 0.0;
        let mut weighted_quality: f64 = 0.0;
        let mut quality_count: f64 = 0.0;
        let mut penalty: f64 = 0.0;

        for obs in window {
            let age = now
                .duration_since(obs.started_at)
                .unwrap_or(Duration::ZERO)
                .as_secs() as f64;
            let weight = (-lambda * age).exp();

            total_weight += weight;

            // Availability
            if obs.success {
                weighted_success += weight;
            } else {
                // Penalize failures: large enough to move the score meaningfully
                penalty += 80.0 * weight;
            }

            // Latency (TTFT)
            if let Some(ttft) = obs.latency.ttft_ms {
                let latency_score = if ttft <= config.good_ttft_ms {
                    1.0
                } else if ttft <= config.acceptable_ttft_ms {
                    0.5
                } else {
                    0.1
                };
                weighted_latency += latency_score * weight;
                latency_count += weight;
            }

            // Retry penalty
            if obs.was_retry {
                penalty += 20.0 * weight;
            }

            // Quality dimension
            let mut obs_quality = 1.0;
            if let Some(ref err) = obs.error {
                match err {
                    ObservationError::EmptyResponse | ObservationError::TruncatedStream => {
                        obs_quality = 0.0;
                    }
                    _ => {}
                }
            }
            if let Some(ref s) = obs.stream {
                if s.stream_broken {
                    obs_quality = 0.0;
                } else {
                    if !s.completed_normally {
                        obs_quality *= 0.5;
                    }
                    if let Some(tps) = s.tokens_per_second {
                        let tps_score = (tps / config.good_tps).min(1.0);
                        obs_quality *= tps_score;
                    }
                }
            }
            weighted_quality += obs_quality * weight;
            quality_count += weight;
        }

        let availability = if total_weight > 0.0 {
            (weighted_success / total_weight).clamp(0.0, 1.0)
        } else {
            1.0
        };

        let latency = if latency_count > 0.0 {
            (weighted_latency / latency_count).clamp(0.0, 1.0)
        } else {
            1.0
        };

        let quality = if quality_count > 0.0 {
            (weighted_quality / quality_count).clamp(0.0, 1.0)
        } else {
            1.0
        };

        // Cost: Phase 1 placeholder (always 1.0)
        let cost = 1.0;

        // Penalty is applied as a direct subtraction from the weighted sum.
        // Scale penalty so that a few failures can meaningfully move the score.
        let penalty_scaled = penalty;

        let score = (config.baseline_score
            + availability * config.availability_weight * 100.0
            + latency * config.latency_weight * 100.0
            + quality * config.quality_weight * 100.0
            + cost * config.cost_weight * 100.0)
            - penalty_scaled;

        let score = score.clamp(0.0, 100.0);

        let breakdown = ScoreBreakdown {
            availability,
            latency,
            quality,
            cost,
            penalty,
        };

        (score, breakdown)
    }

    fn score_to_tier(score: f64) -> ScoreTier {
        if score >= 90.0 {
            ScoreTier::Gold
        } else if score >= 70.0 {
            ScoreTier::Silver
        } else if score >= 40.0 {
            ScoreTier::Bronze
        } else {
            ScoreTier::Poor
        }
    }

    /// Rank all endpoints in a pool by score (descending).
    /// Returns `None` if the pool has no endpoints.
    pub fn rank_pool(&self, pool_id: &str) -> Option<PoolRanking> {
        let mut endpoints: Vec<&EndpointScore> = self
            .endpoints
            .values()
            .filter(|s| s.last_score.pool_id == pool_id)
            .map(|s| &s.last_score)
            .collect();

        if endpoints.is_empty() {
            return None;
        }

        // Sort by score descending
        endpoints.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let excluded: Vec<EndpointScore> = endpoints
            .iter()
            .filter(|ep| ep.excluded)
            .map(|ep| (*ep).clone())
            .collect();
        let ranked: Vec<EndpointScore> = endpoints
            .iter()
            .filter(|ep| !ep.excluded)
            .map(|ep| (*ep).clone())
            .collect();

        let recommended = ranked.first().cloned();
        let recommended_fallback = if ranked.len() >= 2 {
            ranked.get(1).cloned()
        } else {
            None
        };

        Some(PoolRanking {
            pool_id: pool_id.to_string(),
            ranked_endpoints: ranked,
            recommended,
            recommended_fallback,
            excluded_endpoints: excluded,
        })
    }

    /// Rank all pools.
    pub fn rank_all(&self) -> Vec<PoolRanking> {
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut results: Vec<PoolRanking> = Vec::new();

        for ep in self.endpoints.values() {
            let pid = &ep.last_score.pool_id;
            if seen.contains(pid) {
                continue;
            }
            seen.insert(pid.clone());
            if let Some(ranking) = self.rank_pool(pid) {
                results.push(ranking);
            }
        }
        results
    }

    /// Generate PoolFeedback for a pool (for routing layer consumption).
    pub fn get_pool_feedback(&self, pool_id: &str) -> crate::PoolFeedback {
        let mut fb = crate::PoolFeedback::default();
        for (id, state) in &self.endpoints {
            if state.last_score.pool_id == pool_id {
                fb.endpoint_scores
                    .insert(id.clone(), state.last_score.score);
            }
        }
        fb
    }

    /// Export the full engine state as a serializable snapshot.
    /// Host systems can persist this snapshot and restore it later
    /// (e.g. across restarts).
    pub fn export_snapshot(&self) -> ScoreSnapshot {
        ScoreSnapshot {
            endpoints: self.endpoints.clone(),
            last_decay_at_secs: self.last_decay_at
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or(Duration::ZERO)
                .as_secs(),
        }
    }

    /// Restore the engine state from a previously exported snapshot.
    pub fn restore_snapshot(&mut self, snapshot: ScoreSnapshot) {
        self.endpoints = snapshot.endpoints;
        self.last_decay_at = SystemTime::UNIX_EPOCH + Duration::from_secs(snapshot.last_decay_at_secs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use latch_core::score::{LatencyBreakdown, ObservationError, StreamMetrics, TokenStats};
    use std::time::SystemTime;

    /// A fake clock that returns a controllable time.
    struct FakeClock {
        now: std::sync::Arc<std::sync::Mutex<SystemTime>>,
    }

    impl FakeClock {
        fn new() -> Self {
            Self {
                now: std::sync::Arc::new(std::sync::Mutex::new(SystemTime::now())),
            }
        }
        fn advance(&self, dur: Duration) {
            let mut t = self.now.lock().unwrap();
            *t = *t + dur;
        }
        fn clone_ref(&self) -> Self {
            Self {
                now: self.now.clone(),
            }
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> SystemTime {
            *self.now.lock().unwrap()
        }
    }

    fn default_config() -> ScoreConfig {
        ScoreConfig {
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

    fn make_obs(endpoint_id: &str, pool_id: &str, success: bool) -> RequestObservation {
        RequestObservation {
            endpoint_id: endpoint_id.to_string(),
            pool_id: pool_id.to_string(),
            started_at: SystemTime::now(),
            success,
            error: if success {
                None
            } else {
                Some(ObservationError::Timeout)
            },
            was_retry: false,
            latency: LatencyBreakdown {
                total_ms: 300,
                ttft_ms: Some(200),
            },
            tokens: TokenStats {
                input: 100,
                output: 50,
            },
            stream: None,
        }
    }

    #[test]
    fn new_engine_has_no_scores() {
        let engine = ScoringEngine::new(default_config());
        assert!(engine.get_score("ep-1").is_none());
    }

    #[test]
    fn observe_success_raises_score() {
        let clock = FakeClock::new();
        let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

        let obs = make_obs("ep-1", "pool-1", true);
        engine.observe(obs);

        let score = engine.get_score("ep-1").unwrap();
        assert!(score.score > 50.0, "success should give decent score, got {}", score.score);
    }

    #[test]
    fn observe_failure_lowers_score() {
        let clock = FakeClock::new();
        let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

        let obs = make_obs("ep-1", "pool-1", false);
        engine.observe(obs);

        let score = engine.get_score("ep-1").unwrap();
        assert!(score.score < 50.0, "failure should lower score, got {}", score.score);
    }

    #[test]
    fn score_is_clamped() {
        let clock = FakeClock::new();
        let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

        // Many failures with bad latency
        for _ in 0..10 {
            let mut obs = make_obs("ep-1", "pool-1", false);
            obs.latency = LatencyBreakdown {
                total_ms: 5000,
                ttft_ms: Some(3000),
            };
            engine.observe(obs);
        }

        let score = engine.get_score("ep-1").unwrap();
        assert!(
            score.score >= 0.0 && score.score <= 100.0,
            "score {} out of range",
            score.score
        );
    }

    #[test]
    fn tier_calculation_is_correct() {
        assert_eq!(ScoringEngine::<FakeClock>::score_to_tier(95.0), ScoreTier::Gold);
        assert_eq!(ScoringEngine::<FakeClock>::score_to_tier(80.0), ScoreTier::Silver);
        assert_eq!(ScoringEngine::<FakeClock>::score_to_tier(50.0), ScoreTier::Bronze);
        assert_eq!(ScoringEngine::<FakeClock>::score_to_tier(30.0), ScoreTier::Poor);
    }

    #[test]
    fn decay_does_not_panic() {
        let clock = FakeClock::new();
        let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

        engine.observe(make_obs("ep-1", "pool-1", true));
        clock.advance(Duration::from_secs(600));
        engine.decay(); // should not panic

        let score = engine.get_score("ep-1").unwrap();
        assert!(score.score >= 0.0 && score.score <= 100.0);
    }

    #[test]
    fn window_size_is_respected() {
        let clock = FakeClock::new();
        let mut config = default_config();
        config.window_size = 5;
        let mut engine = ScoringEngine::with_clock(config, clock.clone_ref());

        for i in 0..10 {
            let mut obs = make_obs("ep-1", "pool-1", true);
            obs.latency.ttft_ms = Some(i * 100);
            engine.observe(obs);
        }

        let state = engine.endpoints.get("ep-1").unwrap();
        assert_eq!(state.observations.len(), 5, "window should be capped at 5");
    }

    #[test]
    fn stream_broken_lowers_quality() {
        let clock = FakeClock::new();
        let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

        // First observation: success but stream broken → quality = 0
        let mut obs = make_obs("ep-1", "pool-1", true);
        obs.stream = Some(StreamMetrics {
            ttft_ms: 200,
            tokens_per_second: Some(60.0),
            max_inter_chunk_ms: Some(100),
            chunk_count: 10,
            completed_normally: false,
            stream_broken: true,
        });
        engine.observe(obs);

        let breakdown = engine.get_breakdown("ep-1").unwrap();
        assert!(breakdown.quality < 1.0, "stream_broken should lower quality, got {}", breakdown.quality);

        // Add a failure to bring score down, then verify quality stays low
        let mut obs2 = make_obs("ep-1", "pool-1", false);
        obs2.error = Some(ObservationError::Timeout);
        engine.observe(obs2);

        let score = engine.get_score("ep-1").unwrap();
        assert!(score.score < 90.0, "quality penalty + failure should lower overall score, got {}", score.score);
    }

    #[test]
    fn consecutive_timeouts_excludes_endpoint() {
        let clock = FakeClock::new();
        let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

        // 3 consecutive timeouts → should be excluded
        for _ in 0..3 {
            let mut obs = make_obs("ep-1", "pool-1", false);
            obs.error = Some(ObservationError::Timeout);
            engine.observe(obs);
        }

        let score = engine.get_score("ep-1").unwrap();
        assert!(score.excluded, "endpoint should be excluded after 3 consecutive timeouts");
        assert_eq!(score.exclusion_reason.as_deref(), Some("consecutive timeouts"));
    }

    #[test]
    fn success_resets_consecutive_counters() {
        let clock = FakeClock::new();
        let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

        // 2 timeouts
        for _ in 0..2 {
            let mut obs = make_obs("ep-1", "pool-1", false);
            obs.error = Some(ObservationError::Timeout);
            engine.observe(obs);
        }

        // 1 success → should reset counters
        let obs = make_obs("ep-1", "pool-1", true);
        engine.observe(obs);

        let state = engine.endpoints.get("ep-1").unwrap();
        assert_eq!(state.consecutive_timeouts, 0, "success should reset consecutive_timeouts");
        assert!(!state.last_score.excluded, "endpoint should not be excluded after success");
    }

    #[test]
    fn quality_dimension_affects_score() {
        let clock = FakeClock::new();
        let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

        // observation with good quality (stream completes normally)
        let mut obs = make_obs("ep-1", "pool-1", true);
        obs.stream = Some(StreamMetrics {
            ttft_ms: 200,
            tokens_per_second: Some(80.0), // > good_tps (50.0)
            max_inter_chunk_ms: Some(50),
            chunk_count: 20,
            completed_normally: true,
            stream_broken: false,
        });
        engine.observe(obs);

        let breakdown = engine.get_breakdown("ep-1").unwrap();
        assert!(breakdown.quality > 0.8, "good stream should give high quality, got {}", breakdown.quality);
    }

    #[test]
    fn rank_pool_returns_ranking_with_recommended() {
        let clock = FakeClock::new();
        let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

        // Add two endpoints to pool-1
        let obs1 = make_obs("ep-1", "pool-1", true);
        let mut obs2 = make_obs("ep-2", "pool-1", true);
        obs2.latency.ttft_ms = Some(100); // faster → higher score
        engine.observe(obs1);
        engine.observe(obs2);

        let ranking = engine.rank_pool("pool-1").unwrap();
        assert_eq!(ranking.pool_id, "pool-1");
        assert!(ranking.recommended.is_some(), "should have a recommended endpoint");
        assert!(ranking.ranked_endpoints.len() >= 2);
        // Recommended should be the highest-scoring endpoint
        if let Some(ref rec) = ranking.recommended {
            assert_eq!(rec.endpoint_id, ranking.ranked_endpoints[0].endpoint_id);
        }
    }

    #[test]
    fn rank_pool_excludes_excluded_endpoints() {
        let clock = FakeClock::new();
        let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

        // ep-1: 3 consecutive timeouts → excluded
        for _ in 0..3 {
            let mut obs = make_obs("ep-1", "pool-1", false);
            obs.error = Some(ObservationError::Timeout);
            engine.observe(obs);
        }

        // ep-2: success
        let obs2 = make_obs("ep-2", "pool-1", true);
        engine.observe(obs2);

        let ranking = engine.rank_pool("pool-1").unwrap();
        assert!(!ranking.excluded_endpoints.is_empty(), "should have excluded endpoints");
        assert_eq!(ranking.excluded_endpoints[0].endpoint_id, "ep-1");
        // Recommended should be ep-2 (not excluded)
        if let Some(ref rec) = ranking.recommended {
            assert_eq!(rec.endpoint_id, "ep-2");
        }
    }

    #[test]
    fn rank_all_returns_all_pools() {
        let clock = FakeClock::new();
        let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

        engine.observe(make_obs("ep-1", "pool-1", true));
        engine.observe(make_obs("ep-2", "pool-2", true));

        let all = engine.rank_all();
        assert!(all.len() >= 2);
        let pool_ids: Vec<&str> = all.iter().map(|r| r.pool_id.as_str()).collect();
        assert!(pool_ids.contains(&"pool-1"));
        assert!(pool_ids.contains(&"pool-2"));
    }

    #[test]
    fn get_pool_feedback_returns_scores() {
        let clock = FakeClock::new();
        let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

        engine.observe(make_obs("ep-1", "pool-1", true));
        engine.observe(make_obs("ep-2", "pool-1", true));

        let fb = engine.get_pool_feedback("pool-1");
        assert!(fb.endpoint_scores.contains_key("ep-1"));
        assert!(fb.endpoint_scores.contains_key("ep-2"));
    }

    #[test]
    fn export_and_restore_snapshot() {
        let clock = FakeClock::new();
        let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

        // Observe some data
        engine.observe(make_obs("ep-1", "pool-1", true));
        engine.observe(make_obs("ep-2", "pool-1", false));

        // Export snapshot
        let snapshot = engine.export_snapshot();
        assert!(snapshot.endpoints.len() >= 2);
        assert!(snapshot.last_decay_at_secs > 0);

        // Restore into a new engine (different clock is OK)
        let mut engine2 = ScoringEngine::with_clock(default_config(), clock.clone_ref());
        engine2.restore_snapshot(snapshot);

        // Scores should survive the round-trip exactly
        let s1 = engine.get_score("ep-1").unwrap();
        let s2 = engine2.get_score("ep-1").unwrap();
        assert_eq!(s1.score, s2.score);
    }

    #[test]
    fn restored_engine_can_continue_observing() {
        let clock = FakeClock::new();
        let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

        engine.observe(make_obs("ep-1", "pool-1", true));
        let snapshot = engine.export_snapshot();

        let mut engine2 = ScoringEngine::with_clock(default_config(), clock);
        engine2.restore_snapshot(snapshot);

        // Can still observe after restore
        engine2.observe(make_obs("ep-1", "pool-1", true));
        let score = engine2.get_score("ep-1").unwrap();
        assert!(score.score > 0.0);
    }
}
