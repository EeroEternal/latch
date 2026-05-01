//! Rating module - defines rated usage records and the `RatingEngine` trait.
//!
//! Rating is the process of converting a `UsageObservation` (raw meters)
//! into a `RatedUsageRecord` (meters + cost) using a `PriceSnapshot`.

use crate::CurrencyCode;
use crate::observation::{MeterKind, UsageObservation};
use crate::pricing::{PriceSnapshot, TierConfig};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// A rated usage record - the result of applying rating to an observation.
///
/// This is the "derived" data - it combines the immutable observation
/// with the computed cost (rating result).
///
/// Each record has an independent identity (`rated_record_id`) for audit trails.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RatedUsageRecord {
    /// Unique identifier for this rated record (independent of observation.event_id).
    ///
    /// Format: `"{observation.event_id}:v{revision}"`
    /// - First rating of an observation: `":v1"`
    /// - Correction: `":v2"` (supersedes v1)
    pub rated_record_id: String,

    /// The original observation (immutable fact).
    pub observation: UsageObservation,

    /// The rating result (computed cost).
    pub rating: RatingResult,

    /// If this record supersedes a previous one, this points to the old record's ID.
    ///
    /// Only the latest record in the `supersedes` chain is "active".
    pub supersedes: Option<String>,
}

/// The result of rating an observation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RatingResult {
    /// Line items - one per meter kind that was rated.
    pub line_items: Vec<RatedLineItem>,

    /// Total cost across all line items.
    pub total_cost: Decimal,

    /// Currency for the total cost.
    pub currency: CurrencyCode,

    /// ID of the price snapshot used for rating.
    pub price_snapshot_id: String,

    /// When this rating was performed.
    pub rated_at: DateTime<Utc>,
}

/// Context for rating - carries cumulative usage, billing period, etc.
///
/// This is needed for tier-based pricing (Phase 3).
#[derive(Debug, Clone, Default)]
pub struct RatingContext {
    /// Cumulative usage of the baseline meter (in MTok).
    ///
    /// This is used to determine which tier to apply.
    pub cumulative_baseline_usage_mtok: u64,

    /// Billing period identifier (e.g., "2025-05") for tier reset.
    pub billing_period: Option<String>,

    /// Tenant scope for tenant-level accumulation.
    pub tenant_scope: Option<String>,
}

/// A single rated line item.
///
/// Connects a meter reading to its computed cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RatedLineItem {
    /// Which meter this line item is for.
    pub meter_kind: MeterKind,

    /// The quantity that was rated.
    pub quantity: u64,

    /// Unit price used for this line item.
    pub unit_price: Decimal,

    /// Subtotal for this line item (= quantity * unit_price, adjusted for display units).
    pub subtotal: Decimal,
}

/// Trait for rating engines.
///
/// A RatingEngine takes a `UsageObservation`, a `PriceSnapshot`,
/// and a `RatingContext` and produces a `RatingResult`.
///
/// # Design
///
/// - `PricingSource` resolves/fetches the snapshot.
/// - `RatingEngine` computes the cost from the snapshot and context.
/// This separation allows for easy caching of snapshots and
/// unit testing of the rating logic.
///
/// # Example
///
/// ```rust,ignore
/// let engine = DefaultRatingEngine::new();
/// let snapshot = pricing_source.resolve_snapshot(&model_ref, Some(&provider_ref))?;
/// let context = RatingContext::default();
/// let result = engine.rate(&observation, &snapshot, &context)?;
/// ```
pub trait RatingEngine: Send + Sync {
    /// Rate a usage observation using the given price snapshot and context.
    ///
    /// # Errors
    ///
    /// Returns `RatingError` if:
    /// - No price is found for a meter kind in the observation
    /// - The computed cost overflows Decimal
    /// - Tier configuration is invalid
    fn rate(
        &self,
        observation: &UsageObservation,
        snapshot: &PriceSnapshot,
        context: &RatingContext,
    ) -> Result<RatingResult, RatingError>;
}

/// Error type for rating operations.
#[derive(Debug, Clone)]
pub enum RatingError {
    /// No price found for this meter kind in the snapshot.
    NoPriceForMeter {
        meter_kind: MeterKind,
        snapshot_id: String,
    },
    /// Decimal overflow during cost calculation.
    DecimalOverflow {
        meter_kind: MeterKind,
        quantity: u64,
        unit_price: Decimal,
    },
    /// Invalid tier configuration.
    InvalidTierConfig(String),
    /// No tiers match the given usage.
    NoMatchingTier {
        usage_mtok: u64,
    },
    /// Generic error.
    Other(String),
}

// ============================================================================
// DefaultRatingEngine
// ============================================================================

/// Default implementation of `RatingEngine`.
///
/// Applies pricing by:
/// 1. Iterating over meters in `UsageObservation`
/// 2. Looking up price in `PriceSnapshot`
/// 3. Calculating subtotal (= quantity * unit_price)
/// 4. If tier config exists, applying tier-based multiplier
/// 5. Summing all line items for total cost
pub struct DefaultRatingEngine;

impl DefaultRatingEngine {
    /// Create a new `DefaultRatingEngine`.
    pub fn new() -> Self {
        Self
    }
}

impl RatingEngine for DefaultRatingEngine {
    fn rate(
        &self,
        observation: &UsageObservation,
        snapshot: &PriceSnapshot,
        context: &RatingContext,
    ) -> Result<RatingResult, RatingError> {
        let mut line_items = Vec::new();
        let mut total_cost: Decimal = 0.into();

        // Iterate over meters in observation
        for (meter_kind, &quantity) in &observation.meter_set.meters {
            // Look up price
            let price = snapshot.prices.get(meter_kind).ok_or_else(|| {
                RatingError::NoPriceForMeter {
                    meter_kind: meter_kind.clone(),
                    snapshot_id: snapshot.snapshot_id.clone(),
                }
            })?;

            // Convert quantity to Decimal
            let quantity_dec: Decimal = quantity.into();
            let unit_price = price.unit_price;

            // Calculate subtotal = unit_price * quantity
            let subtotal = unit_price
                .checked_mul(quantity_dec)
                .ok_or_else(|| RatingError::DecimalOverflow {
                    meter_kind: meter_kind.clone(),
                    quantity,
                    unit_price,
                })?;

            // Apply tier multiplier if tier config exists
            let final_subtotal = if let Some(ref tier_config) = snapshot.tiers {
                let multiplier = calculate_tier_multiplier(tier_config, context)?;
                if let Some(m) = multiplier {
                    subtotal
                        .checked_mul(m)
                        .ok_or_else(|| RatingError::DecimalOverflow {
                            meter_kind: meter_kind.clone(),
                            quantity,
                            unit_price,
                        })?
                } else {
                    subtotal
                }
            } else {
                subtotal
            };

            total_cost = total_cost
                .checked_add(final_subtotal)
                .ok_or_else(|| RatingError::DecimalOverflow {
                    meter_kind: meter_kind.clone(),
                    quantity,
                    unit_price,
                })?;

            line_items.push(RatedLineItem {
                meter_kind: meter_kind.clone(),
                quantity,
                unit_price,
                subtotal: final_subtotal,
            });
        }

        Ok(RatingResult {
            line_items,
            total_cost,
            currency: snapshot.currency.clone(),
            price_snapshot_id: snapshot.snapshot_id.clone(),
            rated_at: Utc::now(),
        })
    }
}

/// Calculate tier multiplier based on cumulative usage.
fn calculate_tier_multiplier(
    tier_config: &TierConfig,
    context: &RatingContext,
) -> Result<Option<Decimal>, RatingError> {
    let cumulative = context.cumulative_baseline_usage_mtok;

    // Find the matching tier (last boundary where cumulative <= up_to_mtok)
    let mut matched_multiplier: Option<Decimal> = None;

    for boundary in &tier_config.boundaries {
        if cumulative <= boundary.up_to_mtok {
            if let Some(mp) = boundary.price_multiplier {
                matched_multiplier = Some(mp);
            }
        }
    }

    Ok(matched_multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::MeterSet;
    use crate::pricing::{MeterPrice, TierBoundary, TierConfig, TierBaseline, AccumulationScope};
    use rust_decimal_macros::dec;

    #[test]
    fn rated_line_item_calculation() {
        // 1000 tokens * $0.0003/token = $0.30
        let item = RatedLineItem {
            meter_kind: MeterKind::InputTokens,
            quantity: 1000,
            unit_price: dec!(0.0003),
            subtotal: dec!(0.30),
        };
        assert_eq!(item.subtotal, dec!(0.30));
    }

    #[test]
    fn rating_result_total_cost() {
        let result = RatingResult {
            line_items: vec![],
            total_cost: dec!(1.50),
            currency: CurrencyCode::usd(),
            price_snapshot_id: "snap-123".to_string(),
            rated_at: Utc::now(),
        };
        assert_eq!(result.total_cost, dec!(1.50));
    }

    #[test]
    fn default_rating_engine_basic() {
        let engine = DefaultRatingEngine::new();

        // Create observation
        let mut meter_set = MeterSet::new();
        meter_set.accumulate(MeterKind::InputTokens, 1000).unwrap();

        let observation = UsageObservation {
            event_id: crate::identity::UsageEventId::from_raw("test-1"),
            subject: crate::identity::BillingSubject::default(),
            meter_set,
            model_ref: crate::pricing::ModelRef {
                billable_model: "test".to_string(),
                vendor: None,
                region: None,
                tier: None,
            },
            provider_ref: None,
            source: crate::observation::UsageSource::Estimated,
            outcome: crate::observation::UsageOutcome::Success,
            timing: crate::observation::UsageTiming {
                observed_at: Utc::now(),
                completed_at: None,
            },
            correlation: crate::identity::CorrelationIds::default(),
            attributes: crate::observation::Attributes::new(),
        };

        // Create snapshot
        let mut prices = std::collections::HashMap::new();
        prices.insert(
            MeterKind::InputTokens,
            MeterPrice {
                unit_price: dec!(0.0003),
                unit_display: "1M tokens".to_string(),
            },
        );

        let snapshot = PriceSnapshot {
            snapshot_id: "test-snap".to_string(),
            model_ref: crate::pricing::ModelRef {
                billable_model: "test".to_string(),
                vendor: None,
                region: None,
                tier: None,
            },
            currency: CurrencyCode::usd(),
            prices,
            tiers: None,
            effective_from: Utc::now(),
            effective_until: None,
        };

        let context = RatingContext::default();

        let result = engine.rate(&observation, &snapshot, &context).unwrap();
        assert_eq!(result.total_cost, dec!(0.30)); // 1000 * 0.0003
    }
}
