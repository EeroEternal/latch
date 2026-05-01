use crate::EndpointScore;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}

#[derive(Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolRanking {
    pub pool_id: String,
    pub ranked_endpoints: Vec<EndpointScore>,
    pub recommended: Option<EndpointScore>,
    pub recommended_fallback: Option<EndpointScore>,
    pub excluded_endpoints: Vec<EndpointScore>,
}