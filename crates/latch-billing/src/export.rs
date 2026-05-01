//! Export module - defines the `RatedRecordExporter` trait.
//!
//! Exporters consume `RatedUsageRecord`s (not raw observations) and
//! send them to external systems (Stripe, OpenMeter, Kafka, etc.).

use crate::rating::RatedUsageRecord;

/// Trait for exporting rated usage records to external systems.
///
/// **Design note**: Exporters consume *rated* records, not raw observations.
/// This means rating (pricing application) happens before export.
///
/// # When to use
///
/// - **Billing integration**: Export to Stripe, OpenMeter, etc.
/// - **Analytics**: Export to data warehouse
/// - **Real-time dashboards**: Export to Kafka for streaming analytics
///
/// # Implementations (Phase 5)
///
/// - `OpenMeterExporter`: Export to OpenMeter API
/// - `KafkaExporter`: Export to Kafka topic
/// - `StripeExporter`: Export to Stripe for invoicing
///
/// # Example
///
/// ```rust,ignore
/// struct StdoutExporter;
///
/// impl RatedRecordExporter for StdoutExporter {
///     fn export(&self, record: &RatedUsageRecord) -> Result<(), ExportError> {
///         println!("Exported: {} {}", record.rating.total_cost, record.rating.currency);
///         Ok(())
///     }
/// }
/// ```
pub trait RatedRecordExporter: Send + Sync {
    /// Export a rated usage record.
    ///
    /// # Errors
    ///
    /// Returns `ExportError` if the record cannot be exported.
    /// Callers should implement retry logic (the exporter itself
    /// should be idempotent).
    fn export(&self, record: &RatedUsageRecord) -> Result<(), ExportError>;
}

/// Error type for export operations.
#[derive(Debug, Clone)]
pub enum ExportError {
    /// Connection error (retryable).
    ConnectionError(String),
    /// Authentication error (not retryable - config issue).
    AuthError(String),
    /// Invalid data (not retryable - data issue).
    InvalidData(String),
    /// Rate limited (retryable with backoff).
    RateLimited { retry_after_secs: Option<u64> },
    /// Generic error.
    Other(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::ConnectionError(e) => write!(f, "Export connection error: {e}"),
            ExportError::AuthError(e) => write!(f, "Export auth error: {e}"),
            ExportError::InvalidData(e) => write!(f, "Export invalid data: {e}"),
            ExportError::RateLimited {
                retry_after_secs: _,
            } => write!(f, "Export rate limited"),
            ExportError::Other(e) => write!(f, "Export error: {e}"),
        }
    }
}

impl std::error::Error for ExportError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::UsageEventId;
    use crate::observation::{MeterKind, MeterSet, UsageObservation, UsageOutcome, UsageSource, UsageTiming, Attributes};
    use crate::pricing::ModelRef;
    use crate::rating::{RatedLineItem, RatingResult};
    use crate::CurrencyCode;
    use chrono::Utc;
    use rust_decimal_macros::dec;
    use std::sync::{Arc, Mutex};

    /// Simple stdout exporter for testing.
    struct StdoutExporter {
        exported: Arc<Mutex<Vec<String>>>,
    }

    impl StdoutExporter {
        fn new() -> Self {
            Self {
                exported: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl RatedRecordExporter for StdoutExporter {
        fn export(&self, record: &RatedUsageRecord) -> Result<(), ExportError> {
            let msg = format!(
                "{} {}",
                record.rating.total_cost, record.rating.currency.0
            );
            self.exported.lock().unwrap().push(msg);
            Ok(())
        }
    }

    #[test]
    fn stdout_exporter_works() {
        let exporter = StdoutExporter::new();

        // Create a minimal rated record
        let record = RatedUsageRecord {
            rated_record_id: "test-1:v1".to_string(),
            observation: UsageObservation {
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
            },
            rating: RatingResult {
                line_items: vec![RatedLineItem {
                    meter_kind: MeterKind::InputTokens,
                    quantity: 100,
                    unit_price: dec!(0.0003),
                    subtotal: dec!(0.03),
                }],
                total_cost: dec!(0.03),
                currency: CurrencyCode::usd(),
                price_snapshot_id: "snap-1".to_string(),
                rated_at: Utc::now(),
            },
            supersedes: None,
        };

        exporter.export(&record).unwrap();
        assert_eq!(exporter.exported.lock().unwrap().len(), 1);
    }

    #[test]
    fn export_error_display() {
        let err = ExportError::ConnectionError("timeout".to_string());
        assert_eq!(err.to_string(), "Export connection error: timeout");
    }
}
