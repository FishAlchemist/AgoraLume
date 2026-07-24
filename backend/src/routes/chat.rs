//! Chat endpoints: message history, sending, and the live SSE stream.

use std::convert::Infallible;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use serde::Deserialize;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::models::{Message, ServerMeta};
use crate::sim::schedule_turn;
use crate::state::{AppState, StreamEvent};

pub fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(routes!(health))
        .routes(routes!(meta))
        .routes(routes!(list_messages, send_message))
        .routes(routes!(stream))
}

/// Liveness probe — cheap "is the server up" check.
#[utoipa::path(get, path = "/health", tag = "service",
    responses((status = 200, description = "Service is up", body = String)))]
async fn health() -> &'static str {
    "ok"
}

/// The server's mode, so the client can distinguish a mock build (no LLM,
/// in-memory) from a production one — separately from mere reachability.
#[utoipa::path(get, path = "/meta", tag = "service",
    responses((status = 200, description = "Server capabilities", body = ServerMeta)))]
async fn meta() -> Json<ServerMeta> {
    // This build has no LLM and no persistence, so it reports mock mode.
    // Flip `llm`/`persistent` (and `mock`) here as those land.
    Json(ServerMeta {
        mock: true,
        llm: false,
        persistent: false,
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

/// The full message history for a group, oldest first.
#[utoipa::path(get, path = "/groups/{id}/messages", tag = "chat",
    params(("id" = String, Path, description = "Group id")),
    responses((status = 200, description = "Message history", body = Vec<Message>)))]
async fn list_messages(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Json<Vec<Message>> {
    Json(state.list(&id))
}

/// The body of a send request.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct SendBody {
    /// The message text.
    text: String,
    /// The "you" identity to author the message as. When omitted, the group's
    /// stored `selfPersonaId` is used.
    #[serde(default)]
    persona_id: Option<String>,
}

/// Posts a user message and kicks off the agents' turn. The returned line is the
/// stored user message; AI replies, moods, and read receipts arrive on the
/// group's SSE stream.
#[utoipa::path(post, path = "/groups/{id}/messages", tag = "chat",
    params(("id" = String, Path, description = "Group id")),
    request_body = SendBody,
    responses(
        (status = 200, description = "The stored user message", body = Message),
        (status = 404, description = "Unknown group")))]
async fn send_message(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<SendBody>,
) -> Result<Json<Message>, StatusCode> {
    // `turn_members` doubles as an existence check and gives us the group's
    // stored "you" identity to fall back on.
    let Some((self_id, _ai)) = state.workspace().turn_members(&id) else {
        return Err(StatusCode::NOT_FOUND);
    };
    // Author as the caller's active identity when provided (until the workspace
    // is synced, the client is the source of truth for who "you" currently are).
    let author = body.persona_id.clone().unwrap_or(self_id);

    // Store the user's line (seeded with an empty read set) and hand it back.
    // It is not broadcast: the client already renders it from this response.
    let message = Message::conversation(&id, author, body.text.clone(), Some(vec![]));
    state.store(&id, message.clone());

    schedule_turn(state, id, message.id().to_string(), body.text);
    Ok(Json(message))
}

/// Server-Sent Events for a group: default `message` events (AI replies and
/// moods) and named `read` events (read receipts).
#[utoipa::path(get, path = "/groups/{id}/stream", tag = "chat",
    params(("id" = String, Path, description = "Group id")),
    responses((status = 200,
        description = "text/event-stream: `message` frames carry a Message, `read` frames carry a ReadReceipt")))]
async fn stream(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> impl IntoResponse {
    let receiver = state.channel(&id).subscribe();
    let events = BroadcastStream::new(receiver).filter_map(to_sse_event);
    Sse::new(events).keep_alive(KeepAlive::default())
}

/// Renders a broadcast item as an SSE frame, dropping lag errors. Messages use
/// the default `message` event; read receipts use a named `read` event, matching
/// the two listeners in the frontend's `HttpChatApi`.
fn to_sse_event(
    item: Result<StreamEvent, tokio_stream::wrappers::errors::BroadcastStreamRecvError>,
) -> Option<Result<Event, Infallible>> {
    let event = match item.ok()? {
        StreamEvent::Message(message) => Event::default().json_data(message).ok()?,
        StreamEvent::Read(receipt) => Event::default().event("read").json_data(receipt).ok()?,
    };
    Some(Ok(event))
}
