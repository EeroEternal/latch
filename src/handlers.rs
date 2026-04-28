use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};
use latch_core::{Message, SessionId};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::router::get_target_node;
use crate::state::AppState;

#[derive(Serialize, Deserialize, Debug)]
pub struct InitRequest {
    pub system_prompt: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct InitResponse {
    pub session_id: SessionId,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DeltaRequest {
    pub session_id: SessionId,
    pub new_msg: Message,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DeltaResponse {
    pub message: Message,
}

#[derive(Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<Message>,
    max_tokens: u32,
}

pub async fn init_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<InitRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<Value>)> {
    let session_id = Uuid::new_v4().to_string();
    let session_id = SessionId::new(session_id);
    let target_node = get_target_node(&payload.system_prompt, &state.backend_nodes);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let session = Arc::new(crate::state::Session {
        session_id: session_id.clone(),
        target_node,
        history: std::sync::Mutex::new(vec![Message {
            role: "system".to_string(),
            content: payload.system_prompt,
        }]),
        last_active: AtomicI64::new(now),
    });

    state.sessions.insert(session_id.clone(), session);

    Ok((StatusCode::OK, Json(InitResponse { session_id })))
}

pub async fn delta_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<DeltaRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<Value>)> {
    let session = state.sessions.get(&payload.session_id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Session not found"})),
        )
    })?;
    let session = session.value().clone();

    // CRITICAL: Lock history, push, clone, and DROP the lock immediately
    let history_clone = {
        let mut guard = session.history.lock().unwrap();
        guard.push(payload.new_msg);
        guard.clone()
    };

    // Update last_active
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    session.last_active.store(now, Ordering::SeqCst);

    let target_url = format!("{}/v1/chat/completions", session.target_node.as_str());
    let request_body = ChatCompletionRequest {
        model: "default".to_string(),
        messages: history_clone,
        max_tokens: 1024,
    };

    let response = state
        .http_client
        .post(&target_url)
        .json(&request_body)
        .send()
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("Backend request failed: {}", e)})),
            )
        })?;

    let response_json: Value = response.json().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("Failed to parse backend response: {}", e)})),
        )
    })?;

    let assistant_content = response_json
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|msg| msg.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();

    let assistant_msg = Message {
        role: "assistant".to_string(),
        content: assistant_content,
    };

    // Briefly lock history again to push assistant's message
    {
        let mut guard = session.history.lock().unwrap();
        guard.push(assistant_msg.clone());
    }

    // Update last_active again
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    session.last_active.store(now, Ordering::SeqCst);

    Ok((
        StatusCode::OK,
        Json(DeltaResponse {
            message: assistant_msg,
        }),
    ))
}
