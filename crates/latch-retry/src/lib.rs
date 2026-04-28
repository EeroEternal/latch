use latch_core::RetryConfig;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttemptDecision {
    RetryAfter(Duration),
    Fallback { provider: String },
    Stop,
}

#[derive(Clone, Debug, Default)]
pub struct RetryState {
    consecutive_failures: usize,
    circuit_opened_at: Option<Instant>,
    half_open_attempts: usize,
}

impl RetryState {
    pub fn on_success(&mut self) {
        self.consecutive_failures = 0;
        self.circuit_opened_at = None;
        self.half_open_attempts = 0;
    }

    pub fn on_failure(&mut self, cfg: &RetryConfig) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if let Some(cb) = &cfg.circuit_breaker {
            if self.consecutive_failures >= cb.failure_threshold && self.circuit_opened_at.is_none()
            {
                self.circuit_opened_at = Some(Instant::now());
                self.half_open_attempts = 0;
            }
        }
    }
}

pub fn next_decision(state: &mut RetryState, cfg: &RetryConfig, attempt_index: usize) -> AttemptDecision {
    if let Some(cb) = &cfg.circuit_breaker {
        if let Some(opened_at) = state.circuit_opened_at {
            let elapsed = opened_at.elapsed();
            let open_for = Duration::from_millis(cb.open_ms);
            if elapsed < open_for {
                return maybe_fallback_or_stop(cfg);
            }

            if state.half_open_attempts >= cb.half_open_max_attempts {
                return maybe_fallback_or_stop(cfg);
            }
            state.half_open_attempts = state.half_open_attempts.saturating_add(1);
        }
    }

    if attempt_index + 1 >= cfg.max_attempts {
        return maybe_fallback_or_stop(cfg);
    }

    AttemptDecision::RetryAfter(compute_backoff(cfg, attempt_index))
}

pub fn compute_backoff(cfg: &RetryConfig, attempt_index: usize) -> Duration {
    let mut backoff = cfg.backoff_ms.saturating_mul(1u64 << attempt_index.min(20));
    if let Some(max_backoff) = cfg.max_backoff_ms {
        backoff = backoff.min(max_backoff);
    }
    Duration::from_millis(backoff)
}

fn maybe_fallback_or_stop(cfg: &RetryConfig) -> AttemptDecision {
    if let Some(provider) = &cfg.fallback_provider {
        AttemptDecision::Fallback {
            provider: provider.clone(),
        }
    } else {
        AttemptDecision::Stop
    }
}

#[cfg(feature = "tokio")]
pub async fn sleep_for(decision: &AttemptDecision) {
    if let AttemptDecision::RetryAfter(d) = decision {
        tokio::time::sleep(*d).await;
    }
}

#[cfg(test)]
mod tests {
    use super::{next_decision, AttemptDecision, RetryState};
    use latch_core::{config::CircuitBreakerConfig, RetryConfig};
    use std::time::Duration;

    fn base_cfg() -> RetryConfig {
        RetryConfig {
            max_attempts: 3,
            backoff_ms: 100,
            max_backoff_ms: Some(500),
            fallback_provider: None,
            circuit_breaker: None,
        }
    }

    #[test]
    fn retries_with_exponential_backoff_until_limit() {
        let mut state = RetryState::default();
        let cfg = base_cfg();
        assert_eq!(
            next_decision(&mut state, &cfg, 0),
            AttemptDecision::RetryAfter(Duration::from_millis(100))
        );
        assert_eq!(
            next_decision(&mut state, &cfg, 1),
            AttemptDecision::RetryAfter(Duration::from_millis(200))
        );
        assert_eq!(next_decision(&mut state, &cfg, 2), AttemptDecision::Stop);
    }

    #[test]
    fn returns_fallback_when_configured() {
        let mut state = RetryState::default();
        let mut cfg = base_cfg();
        cfg.fallback_provider = Some("strong".to_string());
        assert_eq!(
            next_decision(&mut state, &cfg, 2),
            AttemptDecision::Fallback {
                provider: "strong".to_string()
            }
        );
    }

    #[test]
    fn open_circuit_stops_before_retry_window() {
        let mut state = RetryState::default();
        let mut cfg = base_cfg();
        cfg.circuit_breaker = Some(CircuitBreakerConfig {
            failure_threshold: 2,
            open_ms: 10_000,
            half_open_max_attempts: 1,
        });

        state.on_failure(&cfg);
        state.on_failure(&cfg);
        assert_eq!(next_decision(&mut state, &cfg, 0), AttemptDecision::Stop);
    }
}
