use crate::scoring::compute_score;
use crate::state::{EndpointState, ScoreSnapshot};
use crate::{
    Clock, EndpointScore, PoolFeedback, PoolRanking, RequestObservation, ScoreBreakdown,
    ScoreConfig, SystemClock,
};
#[cfg(test)]
use crate::state::score_to_tier as classify_tier;
#[cfg(test)]
use crate::ScoreTier;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

pub struct ScoringEngine<C: Clock> {
    config: ScoreConfig,
    clock: C,
    pub(crate) endpoints: HashMap<String, EndpointState>,
    last_decay_at: SystemTime,
}

impl ScoringEngine<SystemClock> {
    pub fn new(config: ScoreConfig) -> Self {
        Self::with_clock(config, SystemClock::default())
    }
}

impl<C: Clock> ScoringEngine<C> {
    pub fn with_clock(config: ScoreConfig, clock: C) -> Self {
        let now = clock.now();

        Self {
            config,
            clock,
            endpoints: HashMap::new(),
            last_decay_at: now,
        }
    }

    pub fn observe(&mut self, obs: RequestObservation) {
        let endpoint_id = obs.endpoint_id.clone();
        let pool_id = obs.pool_id.clone();

        if !self.endpoints.contains_key(&endpoint_id) {
            self.endpoints.insert(
                endpoint_id.clone(),
                EndpointState::new(endpoint_id.clone(), pool_id, &self.config),
            );
        }

        let state = self
            .endpoints
            .get_mut(&endpoint_id)
            .expect("endpoint state should exist after insertion");

        state.record_observation(obs, self.config.window_size);

        let now = self.clock.now();
        let (score, breakdown) = compute_score(&self.config, now, &state.observations);
        state.apply_score(score, breakdown);
    }

    pub fn decay(&mut self) {
        let now = self.clock.now();

        for state in self.endpoints.values_mut() {
            let (score, breakdown) = compute_score(&self.config, now, &state.observations);
            state.apply_score(score, breakdown);
        }

        self.last_decay_at = now;
    }

    pub fn get_score(&self, endpoint_id: &str) -> Option<EndpointScore> {
        self.endpoints.get(endpoint_id).map(|state| state.last_score.clone())
    }

    pub fn get_breakdown(&self, endpoint_id: &str) -> Option<ScoreBreakdown> {
        self.endpoints
            .get(endpoint_id)
            .map(|state| state.last_breakdown.clone())
    }

    #[cfg(test)]
    pub(crate) fn score_to_tier(score: f64) -> ScoreTier {
        classify_tier(score)
    }

    pub fn rank_pool(&self, pool_id: &str) -> Option<PoolRanking> {
        let mut endpoints: Vec<&EndpointScore> = self
            .endpoints
            .values()
            .filter(|state| state.last_score.pool_id == pool_id)
            .map(|state| &state.last_score)
            .collect();

        if endpoints.is_empty() {
            return None;
        }

        endpoints.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let excluded: Vec<EndpointScore> = endpoints
            .iter()
            .filter(|endpoint| endpoint.excluded)
            .map(|endpoint| (*endpoint).clone())
            .collect();
        let ranked: Vec<EndpointScore> = endpoints
            .iter()
            .filter(|endpoint| !endpoint.excluded)
            .map(|endpoint| (*endpoint).clone())
            .collect();

        Some(PoolRanking {
            pool_id: pool_id.to_string(),
            recommended: ranked.first().cloned(),
            recommended_fallback: ranked.get(1).cloned(),
            ranked_endpoints: ranked,
            excluded_endpoints: excluded,
        })
    }

    pub fn rank_all(&self) -> Vec<PoolRanking> {
        let mut seen = HashSet::new();
        let mut results = Vec::new();

        for state in self.endpoints.values() {
            let pool_id = &state.last_score.pool_id;
            if seen.contains(pool_id) {
                continue;
            }

            seen.insert(pool_id.clone());
            if let Some(ranking) = self.rank_pool(pool_id) {
                results.push(ranking);
            }
        }

        results
    }

    pub fn get_pool_feedback(&self, pool_id: &str) -> PoolFeedback {
        let mut feedback = PoolFeedback::default();
        let mut total_obs = 0u32;
        let mut failed_obs = 0u32;

        for (endpoint_id, state) in &self.endpoints {
            if state.last_score.pool_id != pool_id {
                continue;
            }

            feedback
                .endpoint_scores
                .insert(endpoint_id.clone(), state.last_score.score);

            total_obs += state.observations.len() as u32;
            failed_obs += state
                .observations
                .iter()
                .filter(|obs| !obs.success)
                .count() as u32;
        }

        let failure_rate = if total_obs > 0 {
            failed_obs as f64 / total_obs as f64
        } else {
            0.0
        };
        let recent_failures = if failure_rate < 0.05 {
            0
        } else if failure_rate < 0.10 {
            1
        } else if failure_rate < 0.20 {
            2
        } else if failure_rate < 0.35 {
            3
        } else {
            4
        };

        feedback
            .recent_failures
            .insert(pool_id.to_string(), recent_failures);
        feedback
    }

    pub fn export_snapshot(&self) -> ScoreSnapshot {
        ScoreSnapshot {
            endpoints: self.endpoints.clone(),
            last_decay_at_secs: self
                .last_decay_at
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or(Duration::ZERO)
                .as_secs(),
        }
    }

    pub fn restore_snapshot(&mut self, snapshot: ScoreSnapshot) {
        self.endpoints = snapshot.endpoints;
        self.last_decay_at = SystemTime::UNIX_EPOCH + Duration::from_secs(snapshot.last_decay_at_secs);
    }
}