use dashmap::DashMap;
use latch_core::{Message, RouteTarget, SessionId};
use std::sync::atomic::AtomicI64;
use std::sync::{Arc, Mutex};

pub struct Session {
    pub session_id: SessionId,
    pub target_node: RouteTarget,
    pub history: Mutex<Vec<Message>>,
    pub last_active: AtomicI64,
}

pub struct AppState {
    pub sessions: DashMap<SessionId, Arc<Session>>,
    pub backend_nodes: Vec<String>,
    pub http_client: reqwest::Client,
}
