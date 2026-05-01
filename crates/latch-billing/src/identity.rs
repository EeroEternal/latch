//! Identity module - defines billing subjects, correlation IDs, and idempotency keys.
//!
//! The key design decision here is that `UsageEventId` encapsulates
//! the idempotency key generation logic, preventing it from being
//! scattered across adapter layers.

use serde::{Deserialize, Serialize};

/// Billing subject - who is being billed for this usage.
///
/// All fields are optional to support different billing models:
/// - API key-based billing: use `api_key_id`
/// - User-based billing: use `end_user_id`
/// - Organization-based billing: use `org_id` + `tenant_id`
/// - Feature-based billing: use `feature`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BillingSubject {
    /// Top-level tenant (e.g., enterprise customer).
    pub tenant_id: Option<String>,

    /// Organization within the tenant.
    pub org_id: Option<String>,

    /// Project within the organization.
    pub project_id: Option<String>,

    /// API key used for this request.
    pub api_key_id: Option<String>,

    /// End user (for user-level billing).
    pub end_user_id: Option<String>,

    /// Feature being used (for feature-based billing).
    pub feature: Option<String>,
}

/// Idempotency key for usage observations.
///
/// This is the core of our deduplication strategy. The key insight is
/// that a single `request_id` is NOT sufficient - retry attempts and
/// fallback chains can produce multiple observations for what the caller
/// sees as one request.
///
/// The recommended construction is `UsageEventId::from_attempt()` which
/// includes `(request_id, attempt_index, provider_id)` to produce a
/// unique key per observation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageEventId {
    /// The idempotency key string.
    ///
    /// Format (from_attempt): `"{request_id}:{attempt_index}:{provider_id}"`
    /// Format (from_raw): caller-defined
    pub idempotency_key: String,
}

impl UsageEventId {
    /// Construct an idempotency key from request components.
    ///
    /// This is the **recommended** construction method. It ensures the
    /// idempotency key includes all components needed to distinguish:
    /// - Different requests (request_id)
    /// - Retry attempts (attempt_index)
    /// - Fallback providers (provider_id)
    ///
    /// Returns `Err(UsageEventIdError::InvalidAttemptIndex)` if `attempt_index < 0`.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use latch_billing::identity::UsageEventId;
    /// let id = UsageEventId::from_attempt("req-123", 0, "openai").unwrap();
    /// assert_eq!(id.idempotency_key, "req-123:0:openai");
    /// ```
    pub fn from_attempt(
        request_id: &str,
        attempt_index: i32,
        provider_id: &str,
    ) -> Result<Self, UsageEventIdError> {
        if attempt_index < 0 {
            return Err(UsageEventIdError::InvalidAttemptIndex(attempt_index));
        }
        Ok(Self {
            idempotency_key: format!("{request_id}:{attempt_index}:{provider_id}"),
        })
    }

    /// Construct an idempotency key from a raw string.
    ///
    /// Use this for non-provider scenarios (e.g., client-side estimation,
    /// batch processing). The caller is responsible for ensuring uniqueness.
    pub fn from_raw(key: impl Into<String>) -> Self {
        Self {
            idempotency_key: key.into(),
        }
    }
}

/// Builder for more precise idempotency semantics (e.g., step-level billing).
pub struct UsageEventIdBuilder {
    request_id: String,
    attempt_index: i32,
    provider_id: String,
    step_id: Option<String>,
    phase: Option<String>,
}

impl UsageEventIdBuilder {
    /// Create a new builder with required fields.
    pub fn new(request_id: &str, attempt_index: i32, provider_id: &str) -> Self {
        Self {
            request_id: request_id.to_string(),
            attempt_index,
            provider_id: provider_id.to_string(),
            step_id: None,
            phase: None,
        }
    }

    /// Add step ID for step-level billing.
    pub fn step_id(mut self, id: impl Into<String>) -> Self {
        self.step_id = Some(id.into());
        self
    }

    /// Add phase for multi-phase attempts.
    pub fn phase(mut self, p: impl Into<String>) -> Self {
        self.phase = Some(p.into());
        self
    }

    /// Build the UsageEventId.
    pub fn build(self) -> Result<UsageEventId, UsageEventIdError> {
        if self.attempt_index < 0 {
            return Err(UsageEventIdError::InvalidAttemptIndex(self.attempt_index));
        }

        let mut key = format!(
            "{}:{}:{}",
            self.request_id, self.attempt_index, self.provider_id
        );

        if let Some(ref s) = self.step_id {
            key.push(':');
            key.push_str(s);
        }

        if let Some(ref p) = self.phase {
            key.push(':');
            key.push_str(p);
        }

        Ok(UsageEventId { idempotency_key: key })
    }
}

/// Error type for UsageEventId operations.
#[derive(Debug, Clone)]
pub enum UsageEventIdError {
    InvalidAttemptIndex(i32),
}

/// Correlation IDs for tracing requests across systems.
///
/// These are informational - they help with debugging and audit trails
/// but are not used for idempotency.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CorrelationIds {
    /// The request ID (from the gateway or client).
    pub request_id: Option<String>,

    /// Distributed tracing ID (e.g., OpenTelemetry trace ID).
    pub trace_id: Option<String>,

    /// Session ID (for conversational use cases).
    pub session_id: Option<String>,

    /// Turn ID (within a session).
    pub turn_id: Option<String>,

    /// Run ID (for agent/ workflow use cases).
    pub run_id: Option<String>,

    /// Step ID (within a run).
    pub step_id: Option<String>,

    /// Attempt index (0 = first attempt, 1 = first retry, etc.).
    /// Mirrors the field in `UsageEventId::from_attempt()`.
    pub attempt_index: Option<i32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_attempt_constructs_correct_key() {
        let id = UsageEventId::from_attempt("req-123", 0, "openai").unwrap();
        assert_eq!(id.idempotency_key, "req-123:0:openai");
    }

    #[test]
    fn from_attempt_with_retry() {
        let id = UsageEventId::from_attempt("req-123", 1, "anthropic").unwrap();
        assert_eq!(id.idempotency_key, "req-123:1:anthropic");
    }

    #[test]
    fn from_attempt_rejects_negative_attempt() {
        let result = UsageEventId::from_attempt("req-123", -1, "openai");
        assert!(matches!(result, Err(UsageEventIdError::InvalidAttemptIndex(-1))));
    }

    #[test]
    fn from_raw_uses_caller_key() {
        let id = UsageEventId::from_raw("custom-key-123");
        assert_eq!(id.idempotency_key, "custom-key-123");
    }

    #[test]
    fn billing_subject_default_all_none() {
        let sub = BillingSubject::default();
        assert_eq!(sub.tenant_id, None);
        assert_eq!(sub.api_key_id, None);
    }

    #[test]
    fn usage_event_id_builder_basic() {
        let id = UsageEventIdBuilder::new("req-123", 0, "openai")
            .build()
            .unwrap();
        assert_eq!(id.idempotency_key, "req-123:0:openai");
    }

    #[test]
    fn usage_event_id_builder_with_step() {
        let id = UsageEventIdBuilder::new("req-123", 0, "openai")
            .step_id("step-1")
            .build()
            .unwrap();
        assert_eq!(id.idempotency_key, "req-123:0:openai:step-1");
    }

    #[test]
    fn usage_event_id_builder_with_phase() {
        let id = UsageEventIdBuilder::new("req-123", 0, "openai")
            .phase("init")
            .build()
            .unwrap();
        assert_eq!(id.idempotency_key, "req-123:0:openai:init");
    }
}
