//! Quota module - defines traits for quota enforcement.
//!
//! **Design note**: Quota operations are **fail-closed** (unlike metering
//! which is fail-open). If quota check fails, the request MUST be denied.
//!
//! These are trait seams - the actual implementation is in Phase 2
//! (`tokenbill-tokio::quota_redis`).

use crate::identity::BillingSubject;
use crate::observation::MeterKind;
use std::collections::HashMap;

// ============================================================================
// Placeholder types for Phase 2
// ============================================================================
// These types are defined here as placeholders so that the trait signatures
// compile in Phase 1. The full implementation is in Phase 2.
//
// Phase 2 will expand these with proper fields.

/// Request to check if a quota allows an operation.
///
/// **Phase 2**: Will include subject, requested amount, quota type, etc.
#[derive(Debug, Clone)]
pub struct QuotaRequest {
    /// Who is being checked.
    pub subject: BillingSubject,
    /// What usage amount is being requested.
    pub requested: UsageAmount,
    /// Additional context (Phase 2).
    pub _placeholder: (),
}

/// The decision from a quota check.
#[derive(Debug, Clone)]
pub enum QuotaDecision {
    /// Request is allowed.
    Allowed {
        /// Remaining quota after this request (if known).
        remaining: Option<u64>,
    },
    /// Request is denied.
    Denied {
        /// Reason for denial.
        reason: String,
    },
}

/// Request to reserve quota (prior to consumption).
#[derive(Debug, Clone)]
pub struct ReservationRequest {
    /// Who is reserving.
    pub subject: BillingSubject,
    /// How much to reserve.
    pub amount: UsageAmount,
    /// Additional context (Phase 2).
    pub _placeholder: (),
}

/// An active reservation.
#[derive(Debug, Clone)]
pub struct Reservation {
    /// Unique reservation ID.
    pub id: String,
    /// Reserved amount.
    pub amount: UsageAmount,
    /// Additional context (Phase 2).
    pub _placeholder: (),
}

/// A usage amount for quota purposes.
///
/// Similar to `MeterSet` but used in quota context.
#[derive(Debug, Clone, Default)]
pub struct UsageAmount {
    /// Meter readings.
    pub meters: HashMap<MeterKind, u64>,
}

impl UsageAmount {
    /// Create a new empty `UsageAmount`.
    pub fn new() -> Self {
        Self {
            meters: HashMap::new(),
        }
    }

    /// Add a meter reading.
    pub fn insert(&mut self, kind: MeterKind, quantity: u64) {
        self.meters
            .entry(kind)
            .and_modify(|v| *v += quantity)
            .or_insert(quantity);
    }

    /// Get the quantity for a meter kind.
    pub fn get(&self, kind: &MeterKind) -> u64 {
        self.meters.get(kind).copied().unwrap_or(0)
    }
}

// ============================================================================
// Traits
// ============================================================================

/// Trait for synchronous quota authorization.
///
/// **Fail-closed**: If this returns `Err`, the request MUST be denied.
/// This is the opposite of metering which is fail-open.
///
/// # When to use
///
/// - Pre-request check: "Can this user afford this request?"
/// - Synchronous: must return quickly (< 10ms typically)
///
/// # Phase 2
///
/// Implementation: `RedisQuotaAuthorizer` (in `tokenbill-tokio`)
pub trait QuotaAuthorizer: Send + Sync {
    /// Check if a request is allowed under quota.
    ///
    /// # Errors
    ///
    /// - `QuotaError::ConnectionError`: fail-closed, deny request
    /// - `QuotaError::Exceeded`: quota exceeded, deny request
    /// - `QuotaError::Other`: fail-closed, deny request
    fn authorize(&self, request: &QuotaRequest) -> Result<QuotaDecision, QuotaError>;
}

/// Trait for quota reservation/commit/refund.
///
/// This is the "two-phase" quota pattern:
/// 1. `reserve()`: reserve quota before the request
/// 2. `commit()`: after request completes, commit actual usage
/// 3. `refund()`: if request fails, refund unused reservation
///
/// **Fail-closed**: All operations must succeed or the request is denied.
///
/// # Phase 2
///
/// Implementation: `RedisQuotaReservator` (in `tokenbill-tokio`)
pub trait QuotaReservator: Send + Sync {
    /// Reserve quota for a request.
    fn reserve(
        &self,
        reservation: &ReservationRequest,
    ) -> Result<Reservation, QuotaError>;

    /// Commit actual usage after request completes.
    ///
    /// `amount` is the actual usage (may be less than reserved).
    fn commit(&self, reservation_id: &str, amount: &UsageAmount)
        -> Result<(), QuotaError>;

    /// Refund an unused or partial reservation.
    fn refund(&self, reservation_id: &str, unused: &UsageAmount)
        -> Result<(), QuotaError>;
}

// ============================================================================
// Error type
// ============================================================================

/// Error type for quota operations.
///
/// **All quota errors are fail-closed** - they should result in
/// the request being denied.
#[derive(Debug, Clone)]
pub enum QuotaError {
    /// Quota exceeded.
    Exceeded {
        subject: String,
        limit: u64,
        requested: u64,
    },
    /// Connection error (e.g., Redis down).
    ConnectionError(String),
    /// Reservation not found (invalid/expired reservation ID).
    ReservationNotFound(String),
    /// Invalid reservation state.
    InvalidState(String),
    /// Generic error.
    Other(String),
}

impl std::fmt::Display for QuotaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QuotaError::Exceeded {
                subject,
                limit,
                requested,
            } => write!(
                f,
                "Quota exceeded for {subject}: limit={limit}, requested={requested}"
            ),
            QuotaError::ConnectionError(e) => {
                write!(f, "Quota connection error: {e}")
            }
            QuotaError::ReservationNotFound(id) => {
                write!(f, "Reservation not found: {id}")
            }
            QuotaError::InvalidState(e) => write!(f, "Invalid reservation state: {e}"),
            QuotaError::Other(e) => write!(f, "Quota error: {e}"),
        }
    }
}

impl std::error::Error for QuotaError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_amount_insert_and_get() {
        let mut ua = UsageAmount::new();
        ua.insert(MeterKind::InputTokens, 100);
        ua.insert(MeterKind::InputTokens, 50);
        assert_eq!(ua.get(&MeterKind::InputTokens), 150);
    }

    #[test]
    fn usage_amount_default_empty() {
        let ua = UsageAmount::default();
        assert_eq!(ua.get(&MeterKind::InputTokens), 0);
    }

    #[test]
    fn quota_error_display() {
        let err = QuotaError::Exceeded {
            subject: "user-1".to_string(),
            limit: 1000,
            requested: 1500,
        };
        assert!(err.to_string().contains("exceeded"));
    }
}
