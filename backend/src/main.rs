//! AgoraLume backend — the API server.
//!
//! An axum server exposing the production AgoraLume HTTP + SSE API (the same
//! contract the frontend's `HttpChatApi` targets). The API is real; only the
//! data behind it is provisional in this build — an in-memory store and
//! simulated agent replies stand in until real persistence and an LLM are
//! connected. Nothing about the API surface changes when they are.
//!
//! Point the frontend at it with `VITE_API_BASE_URL=http://127.0.0.1:8080`.

mod config;
mod models;
mod routes;
mod sim;
mod state;
mod workspace;

use std::sync::Arc;

use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::state::AppState;

#[tokio::main]
async fn main() {
    // One-shot mode: `--dump-openapi [path]` writes the OpenAPI document (YAML)
    // and exits. The frontend's type generation reads this file. Handled before
    // any server setup so it needs nothing running.
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--dump-openapi") {
        let path = args.next().unwrap_or_else(|| "openapi.yml".to_string());
        let yaml = routes::openapi()
            .to_yaml()
            .expect("serialize OpenAPI document to YAML");
        std::fs::write(&path, yaml).unwrap_or_else(|e| panic!("failed to write {path}: {e}"));
        println!("wrote {path}");
        return;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info")),
        )
        .init();

    let config = Config::from_env();
    let state = Arc::new(AppState::seeded());
    let app = routes::router(state);

    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {}: {e}", config.bind));

    tracing::info!(
        address = %config.bind,
        data_dir = %config.data_dir,
        "AgoraLume backend listening (in-memory store; simulated replies, no LLM yet)"
    );

    axum::serve(listener, app)
        .await
        .expect("server error");
}
