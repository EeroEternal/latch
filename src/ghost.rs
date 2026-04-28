use latch_core::Message;
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::interval;

use crate::state::AppState;

#[derive(Serialize)]
struct GhostRequest {
    model: String,
    messages: Vec<Message>,
    max_tokens: u32,
    temperature: f32,
}

pub fn spawn_ghost_daemon(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(5));
        loop {
            ticker.tick().await;
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;

            let mut to_remove = Vec::new();

            for entry in state.sessions.iter() {
                let session = entry.value();
                let last_active = session.last_active.load(Ordering::SeqCst);
                let idle = now - last_active;

                // GC Rule: idle > 1 hour
                if idle > 3600 {
                    to_remove.push(entry.key().clone());
                }
                // Ghost Rule: idle > 15s and <= 3600s
                else if idle > 15 {
                    // Instantly update last_active to prevent duplicate pings
                    session.last_active.store(now, Ordering::SeqCst);

                    let history_clone = {
                        let guard = session.history.lock().unwrap();
                        guard.clone()
                    };
                    let target_node = session.target_node.clone();
                    let session_id = session.session_id.clone();
                    let client = state.http_client.clone();

                    tokio::spawn(async move {
                        let payload = GhostRequest {
                            model: "default".to_string(),
                            messages: history_clone,
                            max_tokens: 1,
                            temperature: 0.0,
                        };

                        let url = format!("{}/v1/chat/completions", target_node.as_str());

                        match client.post(&url).json(&payload).send().await {
                            Ok(_) => {
                                println!("[Ghost] Keep-alive sent for session {}", session_id);
                            }
                            Err(e) => {
                                eprintln!(
                                    "[Ghost] Keep-alive failed for session {}: {}",
                                    session_id, e
                                );
                            }
                        }
                    });
                }
            }

            for sid in to_remove {
                state.sessions.remove(&sid);
                println!("[Ghost] Session {} removed due to inactivity (GC)", sid);
            }
        }
    });
}
