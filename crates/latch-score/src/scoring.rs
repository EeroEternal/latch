use crate::state::default_breakdown;
use crate::{ObservationError, RequestObservation, ScoreBreakdown, ScoreConfig};
use std::collections::VecDeque;
use std::time::{Duration, SystemTime};

pub(crate) fn compute_score(
    config: &ScoreConfig,
    now: SystemTime,
    window: &VecDeque<RequestObservation>,
) -> (f64, ScoreBreakdown) {
    if window.is_empty() {
        return (config.baseline_score, default_breakdown());
    }

    let lambda = (2.0f64).ln() / config.decay_period_secs as f64;
    let mut acc = ScoreAccumulator::default();

    for obs in window {
        acc.add_observation(config, now, lambda, obs);
    }

    let availability = normalized(acc.weighted_success, acc.total_weight, 1.0);
    let latency = normalized(acc.weighted_latency, acc.latency_count, 1.0);
    let quality = normalized(acc.weighted_quality, acc.quality_count, 1.0);
    let cost = 1.0;

    let raw_score = availability * config.availability_weight * 100.0
        + latency * config.latency_weight * 100.0
        + quality * config.quality_weight * 100.0
        + cost * config.cost_weight * 100.0;
    let score = (raw_score - acc.penalty).clamp(0.0, 100.0);

    let breakdown = ScoreBreakdown {
        availability,
        latency,
        quality,
        cost,
        penalty: acc.penalty,
    };

    (score, breakdown)
}

#[derive(Default)]
struct ScoreAccumulator {
    total_weight: f64,
    weighted_success: f64,
    weighted_latency: f64,
    latency_count: f64,
    weighted_quality: f64,
    quality_count: f64,
    penalty: f64,
}

impl ScoreAccumulator {
    fn add_observation(
        &mut self,
        config: &ScoreConfig,
        now: SystemTime,
        lambda: f64,
        obs: &RequestObservation,
    ) {
        let weight = observation_weight(now, lambda, obs);
        self.total_weight += weight;

        if obs.success {
            self.weighted_success += weight;
        } else {
            self.penalty += 80.0 * weight;
        }

        if let Some(latency_score) = latency_score(config, obs) {
            self.weighted_latency += latency_score * weight;
            self.latency_count += weight;
        }

        if obs.was_retry {
            self.penalty += 20.0 * weight;
        }

        self.weighted_quality += quality_score(config, obs) * weight;
        self.quality_count += weight;
    }
}

fn observation_weight(now: SystemTime, lambda: f64, obs: &RequestObservation) -> f64 {
    let age = now
        .duration_since(obs.started_at)
        .unwrap_or(Duration::ZERO)
        .as_secs() as f64;
    (-lambda * age).exp()
}

fn latency_score(config: &ScoreConfig, obs: &RequestObservation) -> Option<f64> {
    let ttft = obs.latency.ttft_ms?;

    Some(if ttft <= config.good_ttft_ms {
        1.0
    } else if ttft <= config.acceptable_ttft_ms {
        0.5
    } else {
        0.1
    })
}

fn quality_score(config: &ScoreConfig, obs: &RequestObservation) -> f64 {
    let mut quality = 1.0;

    if matches!(
        obs.error.as_ref(),
        Some(ObservationError::EmptyResponse | ObservationError::TruncatedStream)
    ) {
        quality = 0.0;
    }

    if let Some(stream) = obs.stream.as_ref() {
        if stream.stream_broken {
            quality = 0.0;
        } else {
            if !stream.completed_normally {
                quality *= 0.5;
            }
            if let Some(tps) = stream.tokens_per_second {
                quality *= (tps / config.good_tps).min(1.0);
            }
        }
    }

    quality
}

fn normalized(sum: f64, weight: f64, default: f64) -> f64 {
    if weight > 0.0 {
        (sum / weight).clamp(0.0, 1.0)
    } else {
        default
    }
}