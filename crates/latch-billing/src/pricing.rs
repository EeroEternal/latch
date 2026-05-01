//! Pricing module - defines pricing models and the `PricingSource` trait.
//!
//! The key design decision here is "push mode" for pricing:
//! - `PricingSource` trait is provided for in-memory/file-based pricing (Phase 3).
//! - For DB/remote pricing, the caller (e.g., xrouter adapter) should
//!   asynchronously fetch the `PriceSnapshot` and pass it to `RatingEngine::rate()`.

use crate::observation::MeterKind;
use crate::CurrencyCode;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Reference to a model for pricing purposes.
///
/// Pricing is keyed by `(billable_model, vendor, region, tier)`.
/// The optional fields allow for flexible pricing strategies:
/// - Global pricing: only `billable_model` is set
/// - Vendor-specific: add `vendor`
/// - Regional pricing: add `region`
/// - Tier-based: add `tier` (e.g., "enterprise", "standard")
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRef {
    /// The billable model name (e.g., "gpt-4o", "claude-3-opus").
    pub billable_model: String,

    /// Vendor/provider name (e.g., "openai", "anthropic").
    pub vendor: Option<String>,

    /// Region for regional pricing (e.g., "us", "eu").
    pub region: Option<String>,

    /// Pricing tier (e.g., "standard", "enterprise", "batch").
    pub tier: Option<String>,
}

/// Reference to a provider instance.
///
/// Used to distinguish between different deployments of the same provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderRef {
    /// Provider identifier (e.g., "openai-west", "self-hosted-1").
    pub provider_id: String,
}

/// A snapshot of prices for a model at a point in time.
///
/// This is the key structure passed to `RatingEngine::rate()`.
/// It is immutable once created - price changes create a new snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceSnapshot {
    /// Unique identifier for this snapshot (for audit/traceability).
    pub snapshot_id: String,

    /// Which model this snapshot applies to.
    pub model_ref: ModelRef,

    /// Currency for all prices in this snapshot.
    pub currency: CurrencyCode,

    /// Price per meter kind.
    /// Only meter kinds present in this map will be billed.
    pub prices: HashMap<MeterKind, MeterPrice>,

    /// Optional tier configuration for volume-based pricing.
    pub tiers: Option<TierConfig>,

    /// When this snapshot becomes effective.
    pub effective_from: DateTime<Utc>,

    /// When this snapshot expires (None = no expiration).
    pub effective_until: Option<DateTime<Utc>>,
}

/// Price for a single meter kind.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeterPrice {
    /// Price per 1 unit of meter (e.g., per 1 token, not per 1M).
    pub unit_price: Decimal,

    /// Display string for the unit (e.g., "1M tokens", "per image").
    pub unit_display: String,
}

/// Tier configuration for volume-based pricing.
///
/// Switches prices based on cumulative usage (in MTok) of a baseline meter.
/// The cumulative value is provided by the caller when calling `RatingEngine::rate()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierConfig {
    /// Which meter to accumulate for tier calculation.
    pub baseline_meter: TierBaseline,

    /// Who provides the cumulative value.
    pub accumulation_scope: AccumulationScope,

    /// Sorted list of tier boundaries.
    ///
    /// Each boundary defines a pricing threshold.
    /// The list should be sorted by `up_to_mtok` ascending.
    pub boundaries: Vec<TierBoundary>,
}

/// Which meter to use as baseline for tier calculation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TierBaseline {
    /// Accumulate by a single meter kind.
    Meter(MeterKind),
    /// Accumulate by total tokens (all meter kinds).
    TotalTokens,
}

/// Who provides the cumulative usage value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AccumulationScope {
    /// Caller provides cumulative value via `RatingContext` (only executable path currently).
    CallerProvided,
}

/// A single tier boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierBoundary {
    /// Threshold (inclusive) in MTok (millions of tokens).
    /// `0` means "starting from 0" (first tier).
    pub up_to_mtok: u64,

    /// Price multiplier relative to `base_price`.
    /// If set, final price = `base_price * price_multiplier`.
    pub price_multiplier: Option<Decimal>,

    /// OR: absolute price per MTok.
    /// If set, this overrides `price_multiplier`.
    pub absolute_price_per_mtok: Option<Decimal>,
}

/// Trait for resolving price snapshots.
///
/// **Design note**: This trait is sync and intended for in-memory
/// or file-based pricing sources (e.g., `TomlPricingSource` in Phase 3).
///
/// For DB/remote pricing, use **push mode** instead:
/// 1. Caller (e.g., xrouter adapter) asynchronously fetches pricing data
/// 2. Caller constructs `PriceSnapshot`
/// 3. Caller passes `PriceSnapshot` to `RatingEngine::rate()`
///
/// This keeps `tokenbill-core` sync and avoids forcing async on all users.
pub trait PricingSource: Send + Sync {
    /// Resolve a price snapshot for the given model and provider.
    fn resolve_snapshot(
        &self,
        model_ref: &ModelRef,
        provider_ref: Option<&ProviderRef>,
    ) -> Result<PriceSnapshot, PricingError>;
}

/// Error type for pricing operations.
#[derive(Debug, Clone)]
pub enum PricingError {
    /// No price found for the given model/provider.
    NotFound {
        model: String,
        provider: Option<String>,
    },
    /// Invalid pricing configuration.
    InvalidConfig(String),
    /// Generic error.
    Other(String),
}

impl std::fmt::Display for PricingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PricingError::NotFound { model, provider } => {
                write!(f, "Price not found for model={model}, provider={provider:?}")
            }
            PricingError::InvalidConfig(s) => write!(f, "Invalid pricing config: {s}"),
            PricingError::Other(s) => write!(f, "Pricing error: {s}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_ref_equality() {
        let m1 = ModelRef {
            billable_model: "gpt-4o".to_string(),
            vendor: Some("openai".to_string()),
            region: None,
            tier: None,
        };
        let m2 = ModelRef {
            billable_model: "gpt-4o".to_string(),
            vendor: Some("openai".to_string()),
            region: None,
            tier: None,
        };
        // Note: ModelRef doesn't derive Eq/PartialEq in this design.
        // That's intentional - we compare by snapshot_id or use custom comparison.
        assert_eq!(m1.billable_model, m2.billable_model);
    }

    #[test]
    fn meter_price_creation() {
        let price = MeterPrice {
            unit_price: Decimal::new(30, 2), // 0.30
            unit_display: "1M tokens".to_string(),
        };
        assert_eq!(price.unit_price, Decimal::new(30, 2));
    }
}
