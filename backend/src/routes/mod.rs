//! HTTP surface, split into the chat stream and the workspace resources.
//!
//! The route table is described with `utoipa`, so the OpenAPI document is
//! generated from the same handlers that serve traffic — it can never drift
//! from the implementation. This is the production API surface; only the data
//! behind it is provisional (in-memory store, simulated agent replies). Emit
//! `openapi.yml` with the binary's `--dump-openapi` flag.

mod chat;
mod workspace;

use std::sync::Arc;

use axum::Router;
use axum::http::Method;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa::openapi::OpenApi as OpenApiDoc;
use utoipa_axum::router::OpenApiRouter;

use crate::state::AppState;

/// Top-level API metadata. Paths and most schemas are contributed by the
/// handlers; `ReadReceipt` is added explicitly because it only appears in the
/// SSE stream (as a `read` event), never as a typed request/response body.
#[derive(OpenApi)]
#[openapi(
    components(schemas(crate::models::ReadReceipt)),
    info(
        title = "AgoraLume API",
        version = "0.1.0",
        description = "AgoraLume API: user + multi-AI group chat with modular personas. The backend is the single source of truth for the workspace (organizations, departments, personas, groups, settings) and streams chat over SSE. This is the production API contract; the current build backs it with an in-memory store and simulated agent replies until an LLM is connected."
    ),
    tags(
        (name = "service", description = "Liveness and server mode"),
        (name = "chat", description = "Messages and the live SSE stream"),
        (name = "organizations", description = "Top-level persona buckets"),
        (name = "departments", description = "Sub-units within an organization"),
        (name = "personas", description = "User identities and AI agents"),
        (name = "groups", description = "Chat rooms and their membership"),
        (name = "settings", description = "Client preferences")
    )
)]
struct ApiDoc;

/// Assembles the full API as an `OpenApiRouter`, the single definition both the
/// running server and the spec generator draw from.
fn api() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .merge(chat::router())
        .merge(workspace::router())
}

/// Builds the runtime router with CORS and request tracing.
pub fn router(state: Arc<AppState>) -> Router {
    // The frontend is a separate static origin (its dev server, or wherever the
    // SPA is hosted), so allow cross-origin access. These endpoints carry no
    // credentials, so a wildcard origin is safe.
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
        .allow_headers(Any);

    let (router, _) = api().split_for_parts();
    router
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state)
}

/// The generated OpenAPI document, for serving or writing to `openapi.yml`.
pub fn openapi() -> OpenApiDoc {
    api().into_openapi()
}
