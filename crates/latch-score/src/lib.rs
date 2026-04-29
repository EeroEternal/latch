use latch_core::{
    config::PoolFeedback,
    score::*,
};
use std::collections::HashMap;

/// Scoring engine for calculating endpoint scores
pub struct ScoringEngine {
    config: ScoreConfig,
    scores: HashMap<String, EndpointScore>,
}

impl ScoringEngine {
    /// Create a new ScoringEngine with the given config
    pub fn new(config: ScoreConfig) -> Self {
        ScoringEngine {
            config,
            scores: HashMap::new(),
        }
    }

    /// Update scores based on a request observation
    pub fn update(&mut self, obs: &RequestObservation, feedback: &mut PoolFeedback) {
        let endpoint_id = &obs.endpoint_id;

        // Calculate new score for this endpoint
        let mut score = self.calculate_score(obs);

        // Apply existing score (exponential moving average using decay)
        if let Some(existing) = self.scores.get(endpoint_id) {
            // Use decay_period_secs to weight existing score
            let alpha = 0.3; // Can be made configurable
            score.score = alpha * score.score + (1.0 - alpha) * existing.score;
        }

        // Clamp to valid range [0, 100]
        score.score = score.score.max(0.0).min(100.0);

        // Update tier based on score
        score.tier = Self::score_to_tier(score.score);

        // Update internal scores
        self.scores.insert(endpoint_id.clone(), score.clone());

        // Update feedback (for use by other crates)
        feedback.endpoint_scores.insert(endpoint_id.clone(), score.score / 100.0); // Normalize to [0, 1]
    }

    /// Calculate score breakdown for an observation
    fn calculate_score(&self, obs: &RequestObservation) -> EndpointScore {
        let mut breakdown = ScoreBreakdown {
            availability: 1.0,
            latency: 1.0,
            quality: 1.0,
            cost: 1.0,
            penalty: 0.0,
        };

        // Availability score (based on success)
        if !obs.success {
            breakdown.availability = 0.0;
            breakdown.penalty += 20.0;
        }

        // Latency score
        if let Some(ttft) = obs.latency.ttft_ms {
            if ttft < self.config.good_ttft_ms {
                breakdown.latency = 1.0;
            } else if ttft < self.config.acceptable_ttft_ms {
                breakdown.latency = 0.5;
            } else {
                breakdown.latency = 0.0;
            }
        }

        // Quality score (based on stream quality if available)
        if let Some(stream) = &obs.stream {
            if stream.broken {
                breakdown.quality = 0.0;
                breakdown.penalty += 10.0;
            }
            if stream.tokens_per_second > 0.0 {
                let tps_score = (stream.tokens_per_second / self.config.good_tps).min(1.0);
                breakdown.quality *= tps_score as f32;
            }
        }

        // Calculate overall score (weighted average, normalized to [0, 100])
        let overall = (breakdown.availability * self.config.availability_weight
            + breakdown.latency * self.config.latency_weight
            + breakdown.quality * self.config.quality_weight
            + breakdown.cost * self.config.cost_weight)
            * 100.0
            - breakdown.penalty;

        let tier = Self::score_to_tier(overall);

        EndpointScore {
            endpoint_id: obs.endpoint_id.clone(),
            pool_id: obs.pool_id.clone(),
            score: overall.max(0.0).min(100.0),
            tier,
            observation_count: 1,
            last_updated: obs.started_at,
            breakdown,
        }
    }

    /// Convert numeric score to tier
    fn score_to_tier(score: f32) -> ScoreTier {
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

    /// Get the current score for an endpoint (normalized to [0, 1])
    pub fn get_score(&self, endpoint_id: &str) -> Option<f32> {
        self.scores.get(endpoint_id).map(|s| s.score / 100.0)
    }

    /// Get all endpoint scores
    pub fn get_all_scores(&self) -> &HashMap<String, EndpointScore> {
        &self.scores
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

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
            error: if success { None } else { Some(ObservationError::Other("test error".to_string())) },
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
        assert!(engine.get_all_scores().is_empty());
    }

    #[test]
    fn update_with_successful_observation() {
        let mut engine = ScoringEngine::new(default_config());
        let mut feedback = PoolFeedback::default();

        let obs = make_obs("ep-1", "pool-1", true);
        engine.update(&obs, &mut feedback);

        let score = engine.get_score("ep-1").unwrap();
        assert!(score > 0.5); // Good latency should give decent score
        assert!(feedback.endpoint_scores.contains_key("ep-1"));
    }

    #[test]
    fn update_with_error_observation() {
        let mut engine = ScoringEngine::new(default_config());
        let mut feedback = PoolFeedback::default();

        let obs = make_obs("ep-1", "pool-1", false);
        engine.update(&obs, &mut feedback);

        let score = engine.get_score("ep-1").unwrap();
        assert!(score < 0.5); // Error should lower score
    }

    #[test]
    fn score_is_clamped_to_valid_range() {
        let mut engine = ScoringEngine::new(default_config());
        let mut feedback = PoolFeedback::default();

        // Very bad observation
        let mut obs = make_obs("ep-1", "pool-1", false);
        obs.latency = LatencyBreakdown {
            total_ms: 5000,
            ttft_ms: Some(3000),
        };

        engine.update(&obs, &mut feedback);

        let score = engine.get_score("ep-1").unwrap();
        assert!(score >= 0.0 && score <= 1.0);
    }

    #[test]
    fn tier_calculation_is_correct() {
        assert_eq!(ScoringEngine::score_to_tier(95.0), ScoreTier::Gold);
        assert_eq!(ScoringEngine::score_to_tier(80.0), ScoreTier::Silver);
        assert_eq!(ScoringEngine::score_to_tier(50.0), ScoreTier::Bronze);
        assert_eq!(ScoringEngine::score_to_tier(30.0), ScoreTier::Poor);
    }
}
