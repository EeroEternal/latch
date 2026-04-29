use latch_core::{MeterConfig, MeterRejectReason, MeterVerdict, SessionUsage};
use std::collections::HashMap;

#[derive(Default)]
pub struct UsageMeter {
    sessions: HashMap<String, SessionUsage>,
}

impl UsageMeter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_session_usage(&self, session_id: &str) -> Option<&SessionUsage> {
        self.sessions.get(session_id)
    }

    pub fn preview_request(
        &self,
        session_id: &str,
        cfg: &MeterConfig,
        predicted_input_tokens: u64,
        predicted_output_tokens: u64,
    ) -> MeterVerdict {
        let current = self.sessions.get(session_id).cloned().unwrap_or_default();
        let next_input = current.input_tokens.saturating_add(predicted_input_tokens);
        let next_output = current
            .output_tokens
            .saturating_add(predicted_output_tokens);
        let next_requests = current.requests.saturating_add(1);

        if let Some(limit) = cfg.session_token_limit {
            if next_input.saturating_add(next_output) > limit {
                return MeterVerdict::Reject(MeterRejectReason::SessionTokenLimitExceeded);
            }
        }

        if let Some(limit) = cfg.session_request_limit {
            if next_requests > limit {
                return MeterVerdict::Reject(MeterRejectReason::SessionRequestLimitExceeded);
            }
        }

        MeterVerdict::Allow
    }

    pub fn record_request(
        &mut self,
        session_id: &str,
        cfg: &MeterConfig,
        input_tokens: u64,
        output_tokens: u64,
    ) -> SessionUsage {
        let entry = self.sessions.entry(session_id.to_string()).or_default();
        entry.input_tokens = entry.input_tokens.saturating_add(input_tokens);
        entry.output_tokens = entry.output_tokens.saturating_add(output_tokens);
        entry.requests = entry.requests.saturating_add(1);
        entry.estimated_cost = estimate_cost(cfg, entry.input_tokens, entry.output_tokens);
        entry.clone()
    }
}

pub fn estimate_cost(cfg: &MeterConfig, input_tokens: u64, output_tokens: u64) -> f64 {
    let input_cost = (input_tokens as f64 / 1000.0) * cfg.price_per_1k_input_tokens;
    let output_cost = (output_tokens as f64 / 1000.0) * cfg.price_per_1k_output_tokens;
    input_cost + output_cost
}

#[cfg(test)]
mod tests {
    use super::{estimate_cost, UsageMeter};
    use latch_core::{MeterConfig, MeterRejectReason, MeterVerdict};

    fn cfg() -> MeterConfig {
        MeterConfig {
            session_token_limit: Some(1000),
            session_request_limit: Some(2),
            price_per_1k_input_tokens: 0.01,
            price_per_1k_output_tokens: 0.03,
            currency: "USD".to_string(),
        }
    }

    #[test]
    fn tracks_usage_and_cost() {
        let mut meter = UsageMeter::new();
        let usage = meter.record_request("s1", &cfg(), 200, 100);
        assert_eq!(usage.input_tokens, 200);
        assert_eq!(usage.output_tokens, 100);
        assert_eq!(usage.requests, 1);
        assert!((usage.estimated_cost - 0.005).abs() < 1e-9);
    }

    #[test]
    fn rejects_when_token_limit_would_be_exceeded() {
        let mut meter = UsageMeter::new();
        meter.record_request("s1", &cfg(), 800, 100);
        let verdict = meter.preview_request("s1", &cfg(), 80, 50);
        assert_eq!(
            verdict,
            MeterVerdict::Reject(MeterRejectReason::SessionTokenLimitExceeded)
        );
    }

    #[test]
    fn rejects_when_request_limit_would_be_exceeded() {
        let mut meter = UsageMeter::new();
        meter.record_request("s1", &cfg(), 10, 10);
        meter.record_request("s1", &cfg(), 10, 10);
        let verdict = meter.preview_request("s1", &cfg(), 1, 1);
        assert_eq!(
            verdict,
            MeterVerdict::Reject(MeterRejectReason::SessionRequestLimitExceeded)
        );
    }

    #[test]
    fn estimate_cost_uses_separate_input_and_output_prices() {
        let c = cfg();
        let cost = estimate_cost(&c, 1000, 500);
        assert!((cost - 0.025).abs() < 1e-9);
    }
}
