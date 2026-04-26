use axum::{routing::post, Router};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio;

mod ghost;
mod handlers;
mod router;
mod state;

use ghost::spawn_ghost_daemon;
use handlers::{delta_handler, init_handler};
use state::AppState;

#[tokio::main]
async fn main() {
    let http_client = reqwest::Client::builder()
        .pool_max_idle_per_host(100)
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .expect("Failed to build HTTP client");

    let backend_nodes = vec![
        "http://127.0.0.1:30000".to_string(),
        "http://127.0.0.1:30001".to_string(),
    ];

    let app_state = Arc::new(AppState {
        sessions: dashmap::DashMap::new(),
        backend_nodes,
        http_client,
    });

    spawn_ghost_daemon(app_state.clone());

    let app = Router::new()
        .route("/v1/agent/init", post(init_handler))
        .route("/v1/agent/delta", post(delta_handler))
        .with_state(app_state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    println!("latchd listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
