//! `latch-billing` - Pure synchronous token billing core library.
//!
//! This crate defines the core types, traits, and pricing models for
//! token-based billing. It is **purely synchronous** and has **zero I/O**
//! dependencies.
//!
//! # Architecture
//!
//! - **Types**: `UsageObservation`, `MeterSet`, `BillingSubject`, etc.
//! - **Traits**: `PricingSource`, `RatingEngine`, `ObservationStore`,
//!   `RatedRecordStore`, `RatedRecordExporter`, `QuotaAuthorizer`, etc.
//! - **Design principle**: Runtime-agnostic sync library. Async I/O should be
//!   implemented by downstream consumers (e.g., gateway applications).
//!
//! # Quick start
//!
//! ```rust,ignore
//! use latch_billing::*;
//!
//! // 1. Create an observation
//! let mut meter_set = MeterSet::new();
//! meter_set.accumulate(MeterKind::InputTokens, 1000).unwrap();
//!
//! let observation = UsageObservation {
//!     event_id: UsageEventId::from_attempt("req-123", 0, "openai").unwrap(),
//!     subject: BillingSubject::default(),
//!     meter_set,
//!     model_ref: ModelRef { ... },
//!     provider_ref: Some(ProviderRef { provider_id: "openai".to_string() }),
//!     source: UsageSource::ProviderReported,
//!     outcome: UsageOutcome::Success,
//!     timing: UsageTiming { ... },
//!     correlation: CorrelationIds::default(),
//!     attributes: Attributes::new(),
//! };
//!
//! // 2. Get a price snapshot (push mode - caller fethes)
//! let snapshot = ...; // fetched by caller (e.e., from DB)
//!
//! // 3. Create rating context (for tier-based pricing)
//! let context = RatingContext::default();
//!
//! // 4. Rate the observation
//! let engine = DefaultRatingEngine::new();
//! let rated = engine.rate(&observation, &snapshot, &context)?;
//! println!("Cost: {} {}", rated.rating.total_cost, rated.rating.currency.0);
//! ```
//!
//! # Module structure
//!
//! - `observation`: `UsageObservation`, `MeterSet`, `MeterKind`, `UsageSource`, `UsageOutcome`, `Attributes`
//! - `identity`: `BillingSubject`, `UsageEventId`, `UsageEventIdBuilder`, `CorrelationIds`
//! - `pricing`: `ModelRef`, `ProviderRef`, `PriceSnapshot`, `PricingSource` trait, `TierConfig`, `TierBaseline`
//! - `rating`: `RatedUsageRecord`, `RatingResult`, `RatedLineItem`, `RatingEngine` trait, `RatingContext`
//! - `storage`: `ObservationStore` trait, `RatedRecordStore` trait, `StoreResult`
//! - `quota`: `QuotaAuthorizer` trait, `QuotaReservator` trait (Phase 2)
//! - `export`: `RatedRecordExporter` trait

// ============================================================================
// Re-exports
// ============================================================================

// Core types
pub use chrono;
pub use rust_decimal;

// Observation module
pub mod observation;
pub use observation::{
    Attributes, AttributeError, CurrencyCode, CurrencyCodeError, MeterKind,
    MeterSet, MeterSetError, UsageObservation, UsageOutcome, UsageSource,
    UsageTiming,
};

// Identity module
pub mod identity;
pub use identity::{
    BillingSubject, CorrelationIds, UsageEventId, UsageEventIdBuilder,
    UsageEventIdError,
};

// Pricing module
pub mod pricing;
pub use pricing::{
    AccumulationScope, MeterPrice, ModelRef, PriceSnapshot, PricingError,
    PricingSource, ProviderRef, TierBaseline, TierBoundary, TierConfig,
};

// Rating module
pub mod rating;
pub use rating::{
    RatedLineItem, RatedUsageRecord, RatingContext, RatingEngine, RatingError,
    RatingResult,
};

// Storage module
pub mod storage;
pub use storage::{ObservationStore, RatedRecordStore, StoreError, StoreResult};

// Quota module
pub mod quota;
pub use quota::{
    QuotaAuthorizer, QuotaDecision, QuotaError, QuotaRequest, QuotaReservator,
    Reservation, ReservationRequest, UsageAmount,
};

// Export module
pub mod export;
pub use export::{ExportError, RatedRecordExporter};

// ============================================================================
// Conditional modules
// ============================================================================

/// TOML pricing source (requires `toml` feature).
#[cfg(feature = "toml")]
pub mod toml_source;
#[cfg(feature = "toml")]
pub use toml_source::TomlPricingSource;
