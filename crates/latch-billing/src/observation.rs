//! Observation module - defines the raw usage observation types.
//!
//! `UsageObservation` is an immutable fact representing a single observation
//! of token usage. It is the core input to the rating engine.

use crate::identity::{CorrelationIds, UsageEventId};
use crate::pricing::{ModelRef, ProviderRef};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Raw usage observation - an immutable fact.
///
/// This is the primary input to the billing system. Once created,
/// observations should never be modified - only rated to produce derived records.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageObservation {
    /// Unique identifier for this observation (idempotency key).
    pub event_id: UsageEventId,

    /// Who is being billed for this usage.
    pub subject: crate::identity::BillingSubject,

    /// The actual meter readings (token counts, image counts, etc.).
    pub meter_set: MeterSet,

    /// Which model/pricing dimension this observation applies to.
    pub model_ref: ModelRef,

    /// Which provider handled this request (if applicable).
    pub provider_ref: Option<ProviderRef>,

    /// Where this observation came from (provider API response, stream accumulation, etc.).
    pub source: UsageSource,

    /// The final state of the request (success/error/timeout).
    pub outcome: UsageOutcome,

    /// Timing information for this observation.
    pub timing: UsageTiming,

    /// Correlation IDs for tracing across systems.
    pub correlation: CorrelationIds,

    /// Extensible attributes: is_fallback, step_type, estimated_reason, etc.
    pub attributes: Attributes,
}

/// A collection of meter readings, keyed by `MeterKind`.
///
/// Uses `HashMap` to guarantee no duplicate `MeterKind` entries.
/// Use `MeterSet::insert()` to add values - it will accumulate
/// quantities for the same key instead of overwriting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeterSet {
    pub meters: HashMap<MeterKind, u64>,
}

impl MeterSet {
    /// Create a new empty `MeterSet`.
    pub fn new() -> Self {
        Self {
            meters: HashMap::new(),
        }
    }

    /// Insert or accumulate a meter reading.
    ///
    /// If the key already exists, the quantity is added to the existing value.
    /// Uses `checked_add` to prevent u64 overflow.
    pub fn accumulate(&mut self, kind: MeterKind, quantity: u64) -> Result<(), MeterSetError> {
        use std::collections::hash_map::Entry;
        match self.meters.entry(kind) {
            Entry::Occupied(mut e) => {
                let new_val = e
                    .get()
                    .checked_add(quantity)
                    .ok_or_else(|| MeterSetError::Overflow(e.key().clone()))?;
                e.insert(new_val);
            }
            Entry::Vacant(e) => {
                e.insert(quantity);
            }
        }
        Ok(())
    }

    /// Get the quantity for a given meter kind.
    /// Returns 0 if the key is not present.
    pub fn get(&self, kind: &MeterKind) -> u64 {
        self.meters.get(kind).copied().unwrap_or(0)
    }
}

impl Default for MeterSet {
    fn default() -> Self {
        Self::new()
    }
}

/// Error type for `MeterSet` operations.
#[derive(Debug, Clone)]
pub enum MeterSetError {
    /// Insert operation would cause u64 overflow.
    Overflow(MeterKind),
}

/// The kind of meter being measured.
///
/// Uses an enum for type safety and extensibility.
/// `Custom(String)` allows for provider-specific meter kinds.
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub enum MeterKind {
    InputTokens,
    OutputTokens,
    CachedInputTokens,
    CachedWriteTokens,
    ReasoningTokens,
    AudioInputTokens,
    AudioOutputTokens,
    ImageCount,
    Custom(String),
}

/// Where a usage observation originated from.
///
/// This is important for auditing - observations from different sources
/// may have different trust levels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UsageSource {
    /// Reported directly by the provider API response.
    ProviderReported,
    /// Accumulated from streaming response chunks.
    StreamAccumulated,
    /// Estimated (e.g., from a request that failed before completion).
    Estimated,
    /// Corrected - must carry `correction_of` attribute pointing to original UsageEventId.
    ///
    /// Correction rules:
    /// - Raw observations are always append-only, never deleted or overwritten
    /// - Correction itself is also an independent observation with its own UsageEventId
    /// - Projection/aggregation layer only counts the latest valid version
    /// - Latest version determined by: primary sort key `observed_at`, secondary sort key `event_id` (lexicographic)
    Corrected { correction_of: UsageEventId },
}

/// The final state of the request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UsageOutcome {
    Success,
    Error { code: String },
    Timeout,
    Unknown,
}

/// Constrained attribute set, newtype wrapper to enforce documentation constraints.
///
/// Key constraints:
/// - Keys cannot start with `sys.` (reserved prefix for library use)
/// - Key max length: 64 characters
/// - Value max length: 256 characters
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Attributes {
    inner: HashMap<String, String>,
}

impl Attributes {
    pub const MAX_KEY_LEN: usize = 64;
    pub const MAX_VALUE_LEN: usize = 256;

    /// Create a new empty Attributes set.
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    /// Insert an attribute. Returns error if key starts with "sys." or exceeds length limits.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) -> Result<(), AttributeError> {
        let key = key.into();
        let value = value.into();

        if key.starts_with("sys.") {
            return Err(AttributeError::ReservedPrefix(key));
        }

        if key.len() > Self::MAX_KEY_LEN {
            let len = key.len();
            return Err(AttributeError::KeyTooLong { key, len });
        }

        if value.len() > Self::MAX_VALUE_LEN {
            let len = value.len();
            return Err(AttributeError::ValueTooLong { key, len });
        }

        self.inner.insert(key, value);
        Ok(())
    }

    /// Get an attribute value by key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.inner.get(key).map(|s| s.as_str())
    }

    /// Iterate over all attributes.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.inner.iter()
    }
}

/// Error type for Attributes operations.
#[derive(Debug, Clone)]
pub enum AttributeError {
    ReservedPrefix(String),
    KeyTooLong { key: String, len: usize },
    ValueTooLong { key: String, len: usize },
}

/// Timing information for a usage observation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageTiming {
    /// When this observation was generated.
    pub observed_at: DateTime<Utc>,

    /// When the request completed (filled in when streaming ends).
    pub completed_at: Option<DateTime<Utc>>,
}

/// Currency code, wrapped in newtype for type safety.
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct CurrencyCode(pub String);

impl CurrencyCode {
    /// Helper constructors (not const - use FromStr for parsing).
    pub fn usd() -> Self {
        CurrencyCode("USD".to_string())
    }
    pub fn cny() -> Self {
        CurrencyCode("CNY".to_string())
    }
    pub fn eur() -> Self {
        CurrencyCode("EUR".to_string())
    }
}

impl std::str::FromStr for CurrencyCode {
    type Err = CurrencyCodeError;

    /// Only accepts 3 uppercase ASCII letters (ISO 4217 format).
    /// Does not do full ISO 4217 enum validation to avoid heavy dependencies.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() == 3 && s.chars().all(|c| c.is_ascii_uppercase()) {
            Ok(CurrencyCode(s.to_string()))
        } else {
            Err(CurrencyCodeError::Invalid(s.to_string()))
        }
    }
}

/// Error type for `CurrencyCode` parsing.
#[derive(Debug, Clone)]
pub enum CurrencyCodeError {
    Invalid(String),
}

impl std::fmt::Display for CurrencyCodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CurrencyCodeError::Invalid(s) => write!(f, "Invalid currency code: {s}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_set_accumulate_accumulates() {
        let mut ms = MeterSet::new();
        ms.accumulate(MeterKind::InputTokens, 100).unwrap();
        ms.accumulate(MeterKind::InputTokens, 50).unwrap();
        assert_eq!(ms.get(&MeterKind::InputTokens), 150);
    }

    #[test]
    fn meter_set_overflow_returns_error() {
        let mut ms = MeterSet::new();
        ms.accumulate(MeterKind::InputTokens, u64::MAX).unwrap();
        let result = ms.accumulate(MeterKind::InputTokens, 1);
        assert!(matches!(result, Err(MeterSetError::Overflow(_))));
    }

    #[test]
    fn meter_set_get_missing_returns_zero() {
        let ms = MeterSet::new();
        assert_eq!(ms.get(&MeterKind::OutputTokens), 0);
    }

    #[test]
    fn meter_kind_custom_hash() {
        let mut map = HashMap::new();
        map.insert(MeterKind::Custom("test".to_string()), 1);
        assert_eq!(map.get(&MeterKind::Custom("test".to_string())), Some(&1));
    }

    #[test]
    fn meter_kind_enum_variant_not_confused_with_custom() {
        let mut map = HashMap::new();
        map.insert(MeterKind::InputTokens, 1);
        map.insert(MeterKind::Custom("InputTokens".to_string()), 2);
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn attributes_insert_valid() {
        let mut attrs = Attributes::new();
        assert!(attrs.insert("key1", "value1").is_ok());
        assert_eq!(attrs.get("key1"), Some("value1"));
    }

    #[test]
    fn attributes_rejects_reserved_prefix() {
        let mut attrs = Attributes::new();
        let result = attrs.insert("sys.test", "value");
        assert!(matches!(result, Err(AttributeError::ReservedPrefix(_))));
    }

    #[test]
    fn attributes_rejects_too_long_key() {
        let mut attrs = Attributes::new();
        let long_key = "a".repeat(65);
        let result = attrs.insert(long_key, "value");
        assert!(matches!(result, Err(AttributeError::KeyTooLong { .. })));
    }
}
