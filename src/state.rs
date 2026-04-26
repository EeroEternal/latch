use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicI64;
use std::sync::{Arc, Mutex};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Message {
    pub role: String,
    pub content: String,
}

pub struct Session {
    pub session_id: String,
    pub target_node: String,
    pub history: Mutex<Vec<Message>>,
    pub last_active: AtomicI64,
}

pub struct AppState {
    pub sessions: DashMap<String, Arc<Session>>,
    pub backend_nodes: Vec<String>,
    pub http_client: reqwest::Client,
}
