use crate::{CompressionStrategy, Message};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompressionResult {
    pub messages: Vec<Message>,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub strategy_used: CompressionStrategy,
    /// Savings ratio (0.0-1.0)
    pub savings_ratio: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CompressionAction {
    #[serde(rename = "skip")]
    Skip {
        tokens: usize,
        reason: String,
    },
    #[serde(rename = "applied")]
    Applied(CompressionResult),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RoutingDecision {
    #[serde(rename = "route")]
    Route {
        provider: String,
        reason: String,
        confidence: f32,
    },
    #[serde(rename = "uncertain")]
    Uncertain {
        reason: String,
        candidates: Vec<(String, f32)>,
    },
}

impl Default for RoutingDecision {
    fn default() -> Self {
        RoutingDecision::Uncertain {
            reason: "no routing has been performed".to_string(),
            candidates: vec![],
        }
    }
}
