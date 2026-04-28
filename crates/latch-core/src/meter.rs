use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SessionUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub requests: u64,
    pub estimated_cost: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum MeterRejectReason {
    SessionTokenLimitExceeded,
    SessionRequestLimitExceeded,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum MeterVerdict {
    Allow,
    Reject(MeterRejectReason),
}
