use crate::{EndpointScore, ObservationError, RequestObservation, ScoreBreakdown, ScoreConfig, ScoreTier};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

#[derive(Serialize, Deserialize)]
pub struct ScoreSnapshot {
    pub endpoints: HashMap<String, EndpointState>,
    pub last_decay_at_secs: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EndpointState {
    pub(crate) observations: VecDeque<RequestObservation>,
    pub(crate) last_score: EndpointScore,
    pub(crate) last_breakdown: ScoreBreakdown,
    pub(crate) consecutive_timeouts: u32,
    pub(crate) consecutive_stream_breaks: u32,
}

impl EndpointState {
    pub(crate) fn new(endpoint_id: String, pool_id: String, config: &ScoreConfig) -> Self {
        let baseline = config.baseline_score;
        let breakdown = default_breakdown();

        Self {
            observations: VecDeque::new(),
            last_score: EndpointScore {
                endpoint_id,
                pool_id,
                score: baseline,
                tier: score_to_tier(baseline),
                observation_count: 0,
                breakdown: breakdown.clone(),
                excluded: false,
                exclusion_reason: None,
            },
            last_breakdown: breakdown,
            consecutive_timeouts: 0,
            consecutive_stream_breaks: 0,
        }
    }

    pub(crate) fn record_observation(&mut self, obs: RequestObservation, window_size: usize) {
        self.update_failure_counters(&obs);
        self.observations.push_back(obs);

        while self.observations.len() > window_size {
            self.observations.pop_front();
        }
    }

    pub(crate) fn apply_score(&mut self, score: f64, breakdown: ScoreBreakdown) {
        self.last_score.score = score;
        self.last_score.tier = score_to_tier(score);
        self.last_score.observation_count = self.observations.len();
        self.last_score.breakdown = breakdown.clone();
        self.last_breakdown = breakdown;
        self.update_exclusion();
    }

    fn update_failure_counters(&mut self, obs: &RequestObservation) {
        if obs.success {
            self.consecutive_timeouts = 0;
            self.consecutive_stream_breaks = 0;
            return;
        }

        let is_timeout = matches!(obs.error.as_ref(), Some(ObservationError::Timeout));
        let is_stream_broken = matches!(obs.stream.as_ref(), Some(stream) if stream.stream_broken);

        if is_timeout {
            self.consecutive_timeouts += 1;
            self.consecutive_stream_breaks = 0;
        } else {
            self.consecutive_timeouts = 0;
        }

        if is_stream_broken {
            self.consecutive_stream_breaks += 1;
            self.consecutive_timeouts = 0;
        } else {
            self.consecutive_stream_breaks = 0;
        }
    }

    fn update_exclusion(&mut self) {
        if self.consecutive_timeouts >= 3 {
            self.last_score.excluded = true;
            self.last_score.exclusion_reason = Some("consecutive timeouts".to_string());
        } else if self.consecutive_stream_breaks >= 3 {
            self.last_score.excluded = true;
            self.last_score.exclusion_reason = Some("consecutive stream breaks".to_string());
        } else {
            self.last_score.excluded = false;
            self.last_score.exclusion_reason = None;
        }
    }
}

pub(crate) fn default_breakdown() -> ScoreBreakdown {
    ScoreBreakdown {
        availability: 1.0,
        latency: 1.0,
        quality: 1.0,
        cost: 1.0,
        penalty: 0.0,
    }
}

pub(crate) fn score_to_tier(score: f64) -> ScoreTier {
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