//! Storage module - defines traits for persisting observations and rated records.
//!
//! **Design note**: Both `ObservationStore` and `RatedRecordStore` are
//! **sync** traits. `latch-billing` does not expose async traits.
//!
//! For async storage (e.g., Postgres via sqlx), downstream consumers should
//! implement their own async buffer layer using these sync traits as the foundation.

use crate::observation::UsageObservation;
use crate::rating::RatedUsageRecord;

/// Trait for appending raw usage observations.
///
/// Observations are **immutable facts** - they should never be updated
/// or deleted, only appended. This is the "append-only event stream"
/// pattern.
///
/// # Idempotency
///
/// Same `UsageEventId` repeated append should:
/// - Be treated as success (not an error)
/// - Side effects happen only once (no double counting)
/// - Return `StoreResult::AlreadyExists` for callers to observe
///
/// # Design
///
/// - Sync trait: implementors should use in-memory storage or
///   implement their own async buffer layer in the application.
/// - Fail-open semantics: callers should not fail if storage fails.
///
/// # Example
///
/// ```rust,ignore
/// struct InMemoryStore {
///     observations: Mutex<Vec<UsageObservation>>,
/// }
///
/// impl ObservationStore for InMemoryStore {
///     fn append_observation(&self, obs: UsageObservation) -> Result<StoreResult, StoreError> {
///         self.observations.lock().unwrap().push(obs);
///         Ok(StoreResult::Appended)
///     }
/// }
/// ```
pub trait ObservationStore: Send + Sync {
    /// Append a single observation to the store.
    ///
    /// # Errors
    ///
    /// Returns `StoreError` if the observation cannot be persisted.
    /// Callers should implement their own retry logic at the application layer.
    fn append_observation(&self, observation: UsageObservation) -> Result<StoreResult, StoreError>;
}

/// Trait for appending rated usage records.
///
/// Rated records are the output of the rating engine.
/// They can be stored separately from (or in addition to) raw observations.
///
/// # When to use which pattern
///
/// | Pattern | Write | Use case |
/// |---------|-------|----------|
/// | Observations only | `append_observation` | Offline rating, facts never lost |
/// | Rated records only | `append_rated_record` | Inline rating, no raw data needed |
/// | Both | Both | Audit + replay (recommended for xrouter) |
pub trait RatedRecordStore: Send + Sync {
    /// Append a single rated record to the store.
    fn append_rated_record(&self, record: RatedUsageRecord) -> Result<StoreResult, StoreError>;
}

/// Result of a store operation (for idempotency).
///
/// Same `UsageEventId` repeated append should return `AlreadyExists`
/// (side effects happen only once).
#[derive(Debug, Clone, PartialEq)]
pub enum StoreResult {
    /// First-time write.
    Appended,
    /// Idempotent duplicate: already exists, not re-written.
    AlreadyExists,
}

/// Error type for storage operations.
#[derive(Debug, Clone)]
pub enum StoreError {
    /// Connection/network error (retryable).
    ConnectionError(String),

    /// Data integrity error (not retryable).
    IntegrityError(String),

    /// Serialization/deserialization error.
    SerializationError(String),

    /// Generic error.
    Other(String),
}

// Implement a simple display for StoreError
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::ConnectionError(e) => write!(f, "Connection error: {e}"),
            StoreError::IntegrityError(e) => write!(f, "Integrity error: {e}"),
            StoreError::SerializationError(e) => write!(f, "Serialization error: {e}"),
            StoreError::Other(e) => write!(f, "Store error: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::UsageEventId;
    use crate::observation::{Attributes, MeterSet, UsageObservation, UsageOutcome, UsageSource, UsageTiming};
    use crate::pricing::ModelRef;
    use chrono::Utc;
    use std::sync::{Arc, Mutex};

    /// Simple in-memory store for testing.
    struct InMemoryObservationStore {
        observations: Arc<Mutex<Vec<UsageObservation>>>,
    }

    impl InMemoryObservationStore {
        fn new() -> Self {
            Self {
                observations: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl ObservationStore for InMemoryObservationStore {
        fn append_observation(&self, observation: UsageObservation) -> Result<StoreResult, StoreError> {
            self.observations.lock().unwrap().push(observation);
            Ok(StoreResult::Appended)
        }
    }

    #[test]
    fn in_memory_store_appends() {
        let store = InMemoryObservationStore::new();
        let obs = UsageObservation {
            event_id: UsageEventId::from_raw("test-1"),
            subject: crate::identity::BillingSubject::default(),
            meter_set: MeterSet::new(),
            model_ref: ModelRef {
                billable_model: "test".to_string(),
                vendor: None,
                region: None,
                tier: None,
            },
            provider_ref: None,
            source: UsageSource::Estimated,
            outcome: UsageOutcome::Success,
            timing: UsageTiming {
                observed_at: Utc::now(),
                completed_at: None,
            },
            correlation: crate::identity::CorrelationIds::default(),
            attributes: Attributes::new(),
        };

        let result = store.append_observation(obs).unwrap();
        assert_eq!(result, StoreResult::Appended);
        assert_eq!(store.observations.lock().unwrap().len(), 1);
    }

    #[test]
    fn store_error_display() {
        let err = StoreError::ConnectionError("db down".to_string());
        assert_eq!(err.to_string(), "Connection error: db down");
    }

    #[test]
    fn store_result_equality() {
        assert_eq!(StoreResult::Appended, StoreResult::Appended);
        assert_eq!(StoreResult::AlreadyExists, StoreResult::AlreadyExists);
        assert_ne!(StoreResult::Appended, StoreResult::AlreadyExists);
    }
}
