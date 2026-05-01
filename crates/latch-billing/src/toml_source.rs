//! TOML pricing source implementation.
//!
//! `TomlPricingSource` loads price snapshots from a TOML file.
//! This is for Phase 3 (file-based pricing, no DB needed).
//!
//! # TOML format
//!
//! ```toml
//! [[snapshots]]
//! snapshot_id = "snap-1"
//! billable_model = "gpt-4o"
//! currency = "USD"
//! effective_from = "2025-01-01T00:00:00Z"
//!
//! [prices]
//! InputTokens = { unit_price = 0.0003, unit_display = "1M tokens" }
//! OutputTokens = { unit_price = 0.0006, unit_display = "1M tokens" }
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! let source = TomlPricingSource::from_file("prices.toml")?;
//! let snapshot = source.resolve_snapshot(&model_ref, Some(&provider_ref))?;
//! ```

use crate::observation::MeterKind;
use crate::pricing::{MeterPrice, ModelRef, PriceSnapshot, PricingError, PricingSource, ProviderRef};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

// ============================================================================
// TOML pricing source
// ============================================================================

/// Loads price snapshots from a TOML file.
///
/// This is a sync `PricingSource` implementation for
/// file-based pricing (no DB/async needed).
pub struct TomlPricingSource {
    snapshots: HashMap<String, PriceSnapshot>,
}

impl TomlPricingSource {
    /// Load snapshots from a TOML file.
    pub fn from_file(path: impl Into<PathBuf>) -> Result<Self, PricingError> {
        let path = path.into();
        let content = fs::read_to_string(&path)
            .map_err(|e| PricingError::InvalidConfig(format!("Failed to read {}: {}", path.display(), e)))?;

        let config: TomlConfig = toml::from_str(&content)
            .map_err(|e| PricingError::InvalidConfig(format!("Failed to parse TOML: {}", e)))?;

        let mut snapshots = HashMap::new();
        for snap in config.snapshots {
            let price_snap: PriceSnapshot = snap.try_into()
                .map_err(|e| PricingError::InvalidConfig(format!("Failed to convert snapshot: {}", e)))?;
            snapshots.insert(price_snap.model_ref.billable_model.clone(), price_snap);
        }

        Ok(Self { snapshots })
    }
}

impl PricingSource for TomlPricingSource {
    fn resolve_snapshot(
        &self,
        model_ref: &ModelRef,
        _provider_ref: Option<&ProviderRef>,
    ) -> Result<PriceSnapshot, PricingError> {
        self.snapshots
            .get(&model_ref.billable_model)
            .cloned()
            .ok_or_else(|| PricingError::NotFound {
                model: model_ref.billable_model.clone(),
                provider: model_ref.vendor.clone(),
            })
    }
}

// ============================================================================
// TOML deserialization structs
// ============================================================================

#[derive(Debug, Deserialize)]
struct TomlConfig {
    snapshots: Vec<TomlSnapshot>,
}

#[derive(Debug, Deserialize)]
struct TomlSnapshot {
    snapshot_id: String,
    billable_model: String,
    vendor: Option<String>,
    region: Option<String>,
    tier: Option<String>,
    currency: String,
    effective_from: String,
    effective_until: Option<String>,
    #[serde(default)]
    prices: HashMap<String, TomlMeterPrice>,
}

#[derive(Debug, Deserialize)]
struct TomlMeterPrice {
    unit_price: String, // Decimal as string
    unit_display: String,
}

// Need to convert `TomlSnapshot` to `PriceSnapshot`
impl TryFrom<TomlSnapshot> for PriceSnapshot {
    type Error = PricingError;

    fn try_from(toml: TomlSnapshot) -> Result<Self, Self::Error> {
        let currency = toml.currency.parse()
            .map_err(|e| PricingError::InvalidConfig(format!("Invalid currency: {}", e)))?;

        let mut prices = HashMap::new();
        for (key, price) in toml.prices {
            let meter_kind = parse_meter_kind(&key)?;
            let unit_price = price.unit_price.parse()
                .map_err(|e| PricingError::InvalidConfig(format!("Invalid price: {}", e)))?;

            prices.insert(meter_kind, MeterPrice {
                unit_price,
                unit_display: price.unit_display,
            });
        }

        let effective_from = toml.effective_from.parse()
            .map_err(|e| PricingError::InvalidConfig(format!("Invalid date: {}", e)))?;

        let effective_until = if let Some(ref s) = toml.effective_until {
            Some(s.parse().map_err(|e| PricingError::InvalidConfig(format!("Invalid date: {}", e)))?)
        } else {
            None
        };

        Ok(PriceSnapshot {
            snapshot_id: toml.snapshot_id,
            model_ref: ModelRef {
                billable_model: toml.billable_model,
                vendor: toml.vendor,
                region: toml.region,
                tier: toml.tier,
            },
            currency,
            prices,
            tiers: None, // TODO: add tier support
            effective_from,
            effective_until,
        })
    }
}

/// Parse meter kind from string (TOML key).
fn parse_meter_kind(s: &str) -> Result<MeterKind, PricingError> {
    match s {
        "InputTokens" => Ok(MeterKind::InputTokens),
        "OutputTokens" => Ok(MeterKind::OutputTokens),
        "CachedInputTokens" => Ok(MeterKind::CachedInputTokens),
        "CachedWriteTokens" => Ok(MeterKind::CachedWriteTokens),
        "ReasoningTokens" => Ok(MeterKind::ReasoningTokens),
        "AudioInputTokens" => Ok(MeterKind::AudioInputTokens),
        "AudioOutputTokens" => Ok(MeterKind::AudioOutputTokens),
        "ImageCount" => Ok(MeterKind::ImageCount),
        _ => Ok(MeterKind::Custom(s.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn toml_source_loads_from_file() {
        let toml_content = r#"
[[snapshots]]
snapshot_id = "snap-1"
billable_model = "gpt-4o"
currency = "USD"
effective_from = "2025-01-01T00:00:00Z"

[snapshots.prices.InputTokens]
unit_price = "0.0003"
unit_display = "1M tokens"

[snapshots.prices.OutputTokens]
unit_price = "0.0006"
unit_display = "1M tokens"
"#;

        // Write to temp file
        let mut file = tempfile::Builder::new()
            .suffix(".toml")
            .tempfile()
            .unwrap();
        file.write_all(toml_content.as_bytes()).unwrap();
        file.flush().unwrap();

        let source = TomlPricingSource::from_file(file.path()).unwrap();
        assert!(!source.snapshots.is_empty());
    }
}
