//! Chat endpoints: message history, sending, and the live SSE stream.

use std::convert::Infallible;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::agent::event::Event as AgentEvent;
use crate::models::{AgentTrace, DebugUsage, Message, ServerMeta};
use crate::state::{AppState, StreamEvent};

pub fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(routes!(health))
        .routes(routes!(meta))
        .routes(routes!(debug_usage))
        .routes(routes!(list_messages, send_message))
        .routes(routes!(post_event))
        .routes(routes!(retry_turn))
        .routes(routes!(debug_traces))
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
async fn meta(State(state): State<Arc<AppState>>) -> Json<ServerMeta> {
    // Liveness and mode are independent facts. `llm` = a real model drives the
    // agents (else the rule-based mock); `persistent` = state is written to disk.
    // "Mock" is the precise combination of neither: no LLM and no persistence.
    let llm = !state.runtime.mock;
    let persistent = state.persistent();
    Json(ServerMeta {
        mock: !llm && !persistent,
        llm,
        persistent,
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

/// Cumulative LLM usage since startup — the global "total usage" readout:
/// request count, token breakdown, cache-hit ratio, and an estimated cost when
/// pricing is configured (always an estimate, for reference only).
#[utoipa::path(get, path = "/debug/usage", tag = "service",
    responses((status = 200, description = "Cumulative LLM usage", body = DebugUsage)))]
async fn debug_usage(State(state): State<Arc<AppState>>) -> Json<DebugUsage> {
    let totals = state.debug_totals();
    let cache_hit_ratio = if totals.prompt_tokens > 0 {
        totals.cached_prompt_tokens as f64 / totals.prompt_tokens as f64
    } else {
        0.0
    };
    let estimated_cost = state.pricing().map(|pricing| {
        pricing.estimate(totals.prompt_tokens, totals.cached_prompt_tokens, totals.completion_tokens)
    });
    Json(DebugUsage {
        requests: totals.requests,
        prompt_tokens: totals.prompt_tokens,
        completion_tokens: totals.completion_tokens,
        total_tokens: totals.total_tokens,
        cached_prompt_tokens: totals.cached_prompt_tokens,
        cache_hit_ratio,
        estimated_cost,
    })
}

/// Recent agent traces for a group — the exact prompt each character received
/// and what it decided — for hydrating the debug panel. Live updates then arrive
/// as `debug` SSE frames on the group stream.
#[utoipa::path(get, path = "/groups/{id}/debug/traces", tag = "chat",
    params(("id" = String, Path, description = "Group id")),
    responses((status = 200, description = "Recent agent traces, oldest first", body = Vec<AgentTrace>)))]
async fn debug_traces(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Json<Vec<AgentTrace>> {
    Json(state.debug_traces(&id))
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

    // Hand the turn to the group's coordinator; replies stream in over SSE.
    state.dispatch(&id, AgentEvent::User { message_id: message.id().to_string() });
    Ok(Json(message))
}

/// The body of an environment-event request.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct EventBody {
    /// A short description of what changed, e.g. "It starts to rain."
    description: String,
    /// Urgent events preempt the current turn (discarding the in-flight agent);
    /// ordinary ones fold into the context at the next agent boundary.
    #[serde(default)]
    urgent: bool,
}

/// Posts an environment event into a group — rain, time passing, an emergency —
/// letting the world outside the chat influence the agents. Accepted and queued
/// for the group's coordinator; its effect (reactions, moods) arrives on the
/// group's SSE stream.
#[utoipa::path(post, path = "/groups/{id}/events", tag = "chat",
    params(("id" = String, Path, description = "Group id")),
    request_body = EventBody,
    responses(
        (status = 202, description = "Event accepted"),
        (status = 404, description = "Unknown group")))]
async fn post_event(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<EventBody>,
) -> StatusCode {
    if state.workspace().turn_members(&id).is_none() {
        return StatusCode::NOT_FOUND;
    }
    state.dispatch(
        &id,
        AgentEvent::Environment { description: body.description, urgent: body.urgent },
    );
    StatusCode::ACCEPTED
}

/// Resumes a turn that was suspended by a failed agent inference: the agents who
/// have not yet read the pending message respond to the current chat. A no-op if
/// nothing is suspended (e.g. the pending turn was already voided by a newer
/// message). Its effect arrives on the group's SSE stream.
#[utoipa::path(post, path = "/groups/{id}/retry", tag = "chat",
    params(("id" = String, Path, description = "Group id")),
    responses(
        (status = 202, description = "Retry accepted"),
        (status = 404, description = "Unknown group")))]
async fn retry_turn(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> StatusCode {
    if state.workspace().turn_members(&id).is_none() {
        return StatusCode::NOT_FOUND;
    }
    state.dispatch(&id, AgentEvent::Retry);
    StatusCode::ACCEPTED
}

/// A turn-activity SSE frame: `true` while the group's agent loop runs a turn,
/// `false` when it goes idle.
#[derive(Serialize)]
struct ActivityFrame {
    active: bool,
}

/// Server-Sent Events for a group: default `message` events (AI replies and
/// moods), named `read` events (read receipts), and named `activity` events
/// (the agent loop turning busy/idle).
#[utoipa::path(get, path = "/groups/{id}/stream", tag = "chat",
    params(("id" = String, Path, description = "Group id")),
    responses((status = 200,
        description = "text/event-stream: `message` frames carry a Message, `read` frames carry a ReadReceipt, `activity` frames carry `{ active: bool }`, `debug` frames carry an AgentTrace")))]
async fn stream(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> impl IntoResponse {
    // Subscribe before reading the activity flag: any change that races this
    // arrives on `live` afterwards, so the seed can only be stale, never lost.
    let receiver = state.channel(&id).subscribe();
    let live = BroadcastStream::new(receiver).filter_map(to_sse_event);
    // Emit a comment the instant the client subscribes. Without an initial byte
    // the response body stays empty until the first keep-alive (~15s), and a
    // buffering reverse proxy (Vite's dev proxy, nginx, …) holds the response
    // head until then — so EventSource would stall on open. A comment line is
    // ignored by every SSE client, so this only affects flush timing.
    let opened = tokio_stream::once(Ok::<Event, Infallible>(Event::default().comment("open")));
    // Seed the just-connected client with the current turn activity, reusing the
    // same serialization as live frames. A reconnect (common through a tunnel)
    // missed the `activity` frames broadcast while it was down, so without this
    // its composer lock could stay stuck until a manual refresh.
    let seed = tokio_stream::iter(to_sse_event(Ok(StreamEvent::Activity(state.is_active(&id)))));
    let events = opened.chain(seed).chain(live);
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
        StreamEvent::Activity(active) => {
            Event::default().event("activity").json_data(ActivityFrame { active }).ok()?
        }
        StreamEvent::Debug(trace) => Event::default().event("debug").json_data(trace).ok()?,
    };
    Some(Ok(event))
}
