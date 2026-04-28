use crate::{CompressionStrategy, Message};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompressionResult {
    pub messages: Vec<Message>,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub strategy_used: CompressionStrategy,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub provider: String,
    pub reason: String,
    pub confidence: f32,
}
