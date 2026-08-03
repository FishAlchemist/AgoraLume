//! Chat endpoints: message history, sending, the live SSE stream, and the
//! diagnostics (token usage, agent traces) that hang off a conversation.
//!
//! Every route here except `/health` and `/meta` resolves through
//! [`CurrentAccount`], so a group id in a path only ever addresses a group in
//! the caller's own workspace.

use std::convert::Infallible;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::agent::event::Event as AgentEvent;
use crate::api_error::ApiError;
use crate::models::{
    AgentTrace, Cost, DebugUsage, GroupSuggestions, Message, ModelUsage, PersonaUsage, ServerMeta,
};
use crate::state::{AppState, CurrentAccount, DebugTotals, StreamEvent};

/// The largest history window a single request may ask for.
///
/// The tail of the log was already bounded (`INITIAL_CAP` inside
/// `AppState::list_window`), but the anchored path was not: `before` is a
/// `usize`, so `?anchor=…&before=4294967295` walked back to the first line ever
/// sent and serialized the lot. Treating every request as hostile, that is a
/// cheap way to make the server do expensive work, so the window is clamped
/// rather than trusted. The frontend's own page size is 40.
const MAX_WINDOW: usize = 500;

/// The longest message text accepted from a client.
///
/// Nothing enforced a length before, and a chat line is not merely stored: it
/// is replayed into every agent's prompt for the rest of the conversation, so
/// an oversized one costs tokens on every subsequent turn, not just once.
const MAX_MESSAGE_CHARS: usize = 8_000;

/// The longest environment-event description accepted from a client. Shorter
/// than a message because it is a stage direction ("It starts to rain."), and
/// it reaches the same prompts.
const MAX_EVENT_CHARS: usize = 1_000;

pub fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(routes!(health))
        .routes(routes!(meta))
        .routes(routes!(usage))
        .routes(routes!(usage_by_persona))
        .routes(routes!(list_messages, send_message))
        .routes(routes!(post_event))
        .routes(routes!(retry_turn))
        .routes(routes!(get_suggestions, regenerate_suggestions))
        .routes(routes!(list_traces))
        .routes(routes!(stream))
}

/// Liveness probe, for an orchestrator rather than the app.
//
// The app uses `/meta` instead, since it needs the server's mode as well as its
// reachability. Kept separate on purpose: `/meta` reads runtime and persistence
// state, so it can fail for reasons that have nothing to do with the process
// being alive. Public — a probe that needed a token couldn't report that the
// server is up when auth is what's broken.
#[utoipa::path(get, path = "/health", tag = "service", security(()),
    responses((status = 200, description = "Up; the body is the literal `ok`", body = String)))]
async fn health() -> &'static str {
    "ok"
}

/// The server's mode: mock build (no LLM, in-memory) vs. production.
//
// Public by necessity: `authRequired` is what tells a client whether it needs
// to log in at all, so it cannot itself require a login.
#[utoipa::path(get, path = "/meta", tag = "service", security(()),
    responses((status = 200, description = "Server capabilities and mode", body = ServerMeta)))]
async fn meta(State(state): State<Arc<AppState>>) -> Json<ServerMeta> {
    // Liveness and mode are independent facts. `llm` = a real model drives the
    // agents (else the rule-based mock); `persistent` = state is written to disk.
    // "Mock" is the precise combination of neither: no LLM and no persistence.
    let llm = !state.runtime().is_mock();
    let persistent = state.persistent();
    Json(ServerMeta {
        mock: !llm && !persistent,
        llm,
        persistent,
        auth_required: state.auth_required(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

// --- Diagnostics ------------------------------------------------------------
//
// Usage used to be four routes — {site-wide, per-group} × {totals, by persona}
// — at two different path prefixes and under two different tags, with the
// per-group pair 404ing and the site-wide pair unable to. They are two
// questions, not four: "what did this cost" and "which character spent it".
// Scope is a property of the question, so it is a query parameter, and the two
// routes now answer for one group or for everything through the same code path.

/// Which slice of usage to report on.
#[derive(Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
struct UsageScope {
    /// One group only. Omitted, covers the whole account.
    group_id: Option<String>,
}

impl UsageScope {
    /// Resolves the scope, rejecting a group the workspace doesn't have.
    ///
    /// Checked even though an unknown group would simply have no recorded
    /// usage: silently answering "zero" for a typo'd or guessed id is
    /// indistinguishable from answering for a real but idle group, and every
    /// other group-scoped route in this file 404s.
    fn resolve(&self, state: &CurrentAccount) -> Result<Option<&str>, ApiError> {
        let Some(id) = self.group_id.as_deref() else {
            return Ok(None);
        };
        if state.workspace().turn_members(id).is_none() {
            return Err(ApiError::not_found(format!("no group with id \"{id}\"")));
        }
        Ok(Some(id))
    }
}

/// Cumulative LLM usage: requests, tokens, cache-hit ratio, estimated cost, and
/// the same totals per model. Cost is always an estimate.
//
// With persistence on this survives a restart. Cost is accrued one trace at a
// time at whatever rate was configured when that trace was recorded, so
// changing the configured rates later never reprices history.
#[utoipa::path(get, path = "/usage", tag = "diagnostics",
    params(UsageScope),
    responses(
        (status = 200, description = "Usage for the requested scope", body = DebugUsage),
        (status = 404, description = "Unknown `groupId`")))]
async fn usage(
    state: CurrentAccount,
    Query(scope): Query<UsageScope>,
) -> Result<Json<DebugUsage>, ApiError> {
    let totals = match scope.resolve(&state)? {
        Some(group_id) => state.group_debug_totals(group_id),
        None => state.debug_totals(),
    };
    Ok(Json(debug_usage_view(totals)))
}

/// The same usage broken down by persona, highest total tokens first. Unscoped,
/// it also includes the synthetic `system` bucket (compression, suggestions).
#[utoipa::path(get, path = "/usage/by-persona", tag = "diagnostics",
    params(UsageScope),
    responses(
        (status = 200, description = "One entry per persona", body = Vec<PersonaUsage>),
        (status = 404, description = "Unknown `groupId`")))]
async fn usage_by_persona(
    state: CurrentAccount,
    Query(scope): Query<UsageScope>,
) -> Result<Json<Vec<PersonaUsage>>, ApiError> {
    let per_persona = match scope.resolve(&state)? {
        Some(group_id) => state.persona_debug_totals_all(group_id),
        None => state.global_persona_debug_totals_all(),
    };
    let mut list: Vec<PersonaUsage> = per_persona
        .into_iter()
        .map(|(persona_id, totals)| PersonaUsage { persona_id, usage: debug_usage_view(totals) })
        .collect();
    list.sort_by_key(|p| std::cmp::Reverse(p.usage.total_tokens));
    Ok(Json(list))
}

/// Turns a raw per-model breakdown into the `DebugUsage` wire shape: the grand
/// totals aren't kept separately anywhere — they're the sum of the per-model
/// entries, computed here so there is exactly one place that can drift out of
/// sync with the breakdown: nowhere. Shared by both scopes, which differ only
/// in which [`DebugTotals`] they pass in.
fn debug_usage_view(totals: DebugTotals) -> DebugUsage {
    let mut requests = 0u64;
    let mut prompt_tokens = 0u64;
    let mut completion_tokens = 0u64;
    let mut total_tokens = 0u64;
    let mut cached_prompt_tokens = 0u64;
    let mut estimated_cost: Option<Cost> = None;

    let mut models: Vec<ModelUsage> = totals
        .models
        .into_iter()
        .map(|(model, m)| {
            requests += m.requests;
            prompt_tokens += m.prompt_tokens;
            completion_tokens += m.completion_tokens;
            total_tokens += m.total_tokens;
            cached_prompt_tokens += m.cached_prompt_tokens;
            if let Some(cost) = &m.cost {
                estimated_cost = Some(match estimated_cost.take() {
                    Some(acc) if acc.currency == cost.currency => acc.add(cost.clone()),
                    _ => cost.clone(),
                });
            }
            ModelUsage {
                model,
                requests: m.requests,
                prompt_tokens: m.prompt_tokens,
                completion_tokens: m.completion_tokens,
                total_tokens: m.total_tokens,
                cached_prompt_tokens: m.cached_prompt_tokens,
                estimated_cost: m.cost,
            }
        })
        .collect();
    models.sort_by_key(|m| std::cmp::Reverse(m.total_tokens));

    let cache_hit_ratio =
        if prompt_tokens > 0 { cached_prompt_tokens as f64 / prompt_tokens as f64 } else { 0.0 };

    DebugUsage {
        requests,
        prompt_tokens,
        completion_tokens,
        total_tokens,
        cached_prompt_tokens,
        cache_hit_ratio,
        estimated_cost,
        models,
    }
}

/// Recent agent traces: the prompt each character received and what it decided.
/// Live updates arrive as `debug` SSE frames.
//
// The most revealing read in the API: a trace carries a persona's full system
// prompt and the conversation context it saw. Scoped to the caller's own
// workspace like everything else here, so it exposes only that account's own
// characters to that account.
#[utoipa::path(get, path = "/groups/{groupId}/traces", tag = "diagnostics",
    params(("groupId" = String, Path, description = "The group whose traces to read")),
    responses(
        (status = 200, description = "Traces, oldest first", body = Vec<AgentTrace>),
        (status = 404, description = "Unknown group")))]
async fn list_traces(
    state: CurrentAccount,
    Path(id): Path<String>,
) -> Result<Json<Vec<AgentTrace>>, ApiError> {
    if state.workspace().turn_members(&id).is_none() {
        return Err(ApiError::not_found(format!("no group with id \"{id}\"")));
    }
    Ok(Json(state.debug_traces(&id)))
}

// --- Messages ---------------------------------------------------------------

/// Query for a window of message history — one shape for every navigation. All
/// fields are optional.
#[derive(Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
struct HistoryQuery {
    /// Line id to centre the window on. Omitted, the window ends at the newest line.
    anchor: Option<String>,
    /// Lines before the anchor (or before the tail). Clamped to 500.
    #[param(maximum = 500)]
    before: Option<usize>,
    /// Lines after the anchor. Ignored without an `anchor`. Clamped to 500.
    #[param(maximum = 500)]
    after: Option<usize>,
    /// Read mark (epoch ms). Without an `anchor`, widens the window back to cover
    /// every unread line.
    since: Option<i64>,
}

/// A contiguous window of message history, oldest first. Max 500 lines (160
/// without an `anchor`).
//
// One shape drives every navigation: initial open (`before` + `since`), paging
// earlier (`anchor` + `before`), later (`anchor` + `after`), and jumping to an
// arbitrary line (`anchor` + `before` + `after`).
//
// An unknown group id yields an empty list rather than a 404, unlike the other
// group routes. That's deliberate: clients call this bare to reconcile after an
// SSE reconnect, so a group deleted in another tab must heal to "no messages"
// rather than throw.
#[utoipa::path(get, path = "/groups/{groupId}/messages", tag = "chat",
    params(("groupId" = String, Path, description = "The group whose history to read"), HistoryQuery),
    responses((status = 200, description = "The window, oldest first; empty for an unknown group", body = Vec<Message>)))]
async fn list_messages(
    state: CurrentAccount,
    Path(id): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Json<Vec<Message>> {
    // Clamp rather than reject: a client asking for more than the cap wants "as
    // much as you'll give me", and failing that request would only push it into
    // paging in a loop for the same total volume.
    let before = query.before.map(|n| n.min(MAX_WINDOW));
    let after = query.after.map(|n| n.min(MAX_WINDOW));
    Json(state.list_window(&id, query.anchor.as_deref(), before, after, query.since))
}

/// The body of a send request.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct SendBody {
    /// The message text.
    #[schema(max_length = 8000)]
    text: String,
    /// Which user identity to author as; must be a `user` persona, not an AI
    /// one. Omitted, the group's `selfPersonaId` is used.
    #[serde(default)]
    persona_id: Option<String>,
}

/// Posts a user message and starts the agents' turn. Replies, moods, and read
/// receipts arrive on the SSE stream.
#[utoipa::path(post, path = "/groups/{groupId}/messages", tag = "chat",
    params(("groupId" = String, Path, description = "The group to post into")),
    request_body = SendBody,
    responses(
        (status = 200, description = "The stored message", body = Message),
        (status = 404, description = "Unknown group"),
        (status = 422, description = "Blank or over-long text, or a `personaId` that is not a user identity")))]
async fn send_message(
    state: CurrentAccount,
    Path(id): Path<String>,
    Json(body): Json<SendBody>,
) -> Result<Json<Message>, ApiError> {
    // `turn_members` doubles as an existence check and gives us the group's
    // stored "you" identity to fall back on.
    let Some((self_id, _ai)) = state.workspace().turn_members(&id) else {
        return Err(ApiError::not_found(format!("no group with id \"{id}\"")));
    };
    let text = body.text.trim();
    if text.is_empty() {
        return Err(ApiError::unprocessable("message text cannot be blank"));
    }
    if text.chars().count() > MAX_MESSAGE_CHARS {
        return Err(ApiError::unprocessable(format!(
            "message text is limited to {MAX_MESSAGE_CHARS} characters"
        )));
    }

    // Author as the caller's active identity when provided. Validated rather
    // than trusted: an unchecked `personaId` let a client attribute a line to
    // any persona in the workspace — including an AI one, which would put words
    // in a character's mouth and feed them back to every agent as context.
    let author = match &body.persona_id {
        Some(persona_id) => {
            let ws = state.workspace();
            let persona = ws.personas.iter().find(|p| &p.id == persona_id).ok_or_else(|| {
                ApiError::unprocessable(format!("no persona with id \"{persona_id}\""))
            })?;
            if persona.kind != crate::models::PersonaKind::User {
                return Err(ApiError::unprocessable(
                    "personaId must name a user identity; a message cannot be authored as an AI persona",
                ));
            }
            persona_id.clone()
        }
        None => self_id,
    };

    // Store the user's line (seeded with an empty read set) and hand it back.
    // It is not broadcast: the client already renders it from this response.
    let message = Message::conversation(&id, author, text.to_string(), Some(vec![]));
    state.store(&id, message.clone());

    // Hand the turn to the group's coordinator; replies stream in over SSE.
    // `dispatch` spawns the coordinator task on first use, which needs the
    // `Arc` itself (to hold a clone across the `tokio::spawn`) rather than
    // just a `&AccountState` — hence `.0` instead of going through `Deref`.
    state.0.dispatch(&id, AgentEvent::User { message_id: message.id().to_string() });
    Ok(Json(message))
}

/// The body of an environment-event request.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct EventBody {
    /// What changed, e.g. "It starts to rain."
    #[schema(max_length = 1000)]
    description: String,
    /// Urgent events preempt the current turn; ordinary ones fold into the
    /// context at the next agent boundary.
    #[serde(default)]
    urgent: bool,
}

/// Posts an environment event — rain, time passing, an emergency — letting the
/// world outside the chat influence the agents. Its effect arrives on the SSE
/// stream.
#[utoipa::path(post, path = "/groups/{groupId}/events", tag = "chat",
    params(("groupId" = String, Path, description = "The group to post the event into")),
    request_body = EventBody,
    responses(
        (status = 202, description = "Queued"),
        (status = 404, description = "Unknown group"),
        (status = 422, description = "Blank or over-long description")))]
async fn post_event(
    state: CurrentAccount,
    Path(id): Path<String>,
    Json(body): Json<EventBody>,
) -> Result<axum::http::StatusCode, ApiError> {
    if state.workspace().turn_members(&id).is_none() {
        return Err(ApiError::not_found(format!("no group with id \"{id}\"")));
    }
    let description = body.description.trim();
    if description.is_empty() {
        return Err(ApiError::unprocessable("event description cannot be blank"));
    }
    if description.chars().count() > MAX_EVENT_CHARS {
        return Err(ApiError::unprocessable(format!(
            "event description is limited to {MAX_EVENT_CHARS} characters"
        )));
    }
    state.0.dispatch(
        &id,
        AgentEvent::Environment { description: description.to_string(), urgent: body.urgent },
    );
    Ok(axum::http::StatusCode::ACCEPTED)
}

/// Resumes a turn suspended by a failed agent inference. A no-op if nothing is
/// suspended. Its effect arrives on the SSE stream.
#[utoipa::path(post, path = "/groups/{groupId}/retry", tag = "chat",
    params(("groupId" = String, Path, description = "The group whose suspended turn to resume")),
    responses(
        (status = 202, description = "Accepted"),
        (status = 404, description = "Unknown group")))]
async fn retry_turn(
    state: CurrentAccount,
    Path(id): Path<String>,
) -> Result<axum::http::StatusCode, ApiError> {
    if state.workspace().turn_members(&id).is_none() {
        return Err(ApiError::not_found(format!("no group with id \"{id}\"")));
    }
    state.0.dispatch(&id, AgentEvent::Retry);
    Ok(axum::http::StatusCode::ACCEPTED)
}

/// Cached conversation-starter suggestions, served from the server-side cache.
/// Empty (`generatedAt == 0`) until the first generation completes.
//
// If the cache is stale — the conversation moved on, or the part of day changed
// — a fresh generation starts in the background and arrives on the
// `suggestions` SSE frame. The frontend only fetches and displays.
#[utoipa::path(get, path = "/groups/{groupId}/suggestions", tag = "chat",
    params(("groupId" = String, Path, description = "The group whose suggestions to read")),
    responses(
        (status = 200, description = "The cached suggestions; a refresh may follow on the stream", body = GroupSuggestions),
        (status = 404, description = "Unknown group")))]
async fn get_suggestions(
    state: CurrentAccount,
    Path(id): Path<String>,
) -> Result<Json<GroupSuggestions>, ApiError> {
    if state.workspace().turn_members(&id).is_none() {
        return Err(ApiError::not_found(format!("no group with id \"{id}\"")));
    }
    // Return the cache now; regenerate in the background only if stale.
    state.0.request_suggestions(&id, false);
    Ok(Json(state.suggestions(&id)))
}

/// Creates a fresh set of suggestions (the composer's "give me other ideas").
/// Generated in the background; the result arrives on the `suggestions` SSE
/// frame. Rate-limited server-side — repeated calls coalesce.
//
// A `POST` to the same collection the `GET` reads, rather than the
// `…/suggestions/regenerate` verb this replaced: creating a new set is what the
// request does, and the resource it creates them for is already in the path.
#[utoipa::path(post, path = "/groups/{groupId}/suggestions", tag = "chat",
    params(("groupId" = String, Path, description = "The group to regenerate suggestions for")),
    responses(
        (status = 202, description = "Accepted, or coalesced with a recent one"),
        (status = 404, description = "Unknown group")))]
async fn regenerate_suggestions(
    state: CurrentAccount,
    Path(id): Path<String>,
) -> Result<axum::http::StatusCode, ApiError> {
    if state.workspace().turn_members(&id).is_none() {
        return Err(ApiError::not_found(format!("no group with id \"{id}\"")));
    }
    state.0.request_suggestions(&id, true);
    Ok(axum::http::StatusCode::ACCEPTED)
}

/// A turn-activity SSE frame: `true` while the group's agent loop runs a turn,
/// `false` when it goes idle.
#[derive(Serialize)]
struct ActivityFrame {
    active: bool,
}

/// Server-Sent Events for a group.
///
/// Frames: unnamed = `Message`; `read` = `ReadReceipt`; `activity` =
/// `{ "active": bool }`; `turn` = `Turn`; `debug` = `AgentTrace`;
/// `suggestions` = `GroupSuggestions`. `activity` and `turn` are seeded on connect.
//
// The token goes in the `Authorization` header like every other route — there
// is deliberately no `?access_token=` fallback, since a forwarding proxy logs
// URLs. `EventSource` cannot set headers, so a client reads this with `fetch`.
// OpenAPI can't type per-event-name bodies, so the media type is all the
// document can state; the frame shapes are in the doc comment above.
#[utoipa::path(get, path = "/groups/{groupId}/stream", tag = "chat",
    params(("groupId" = String, Path, description = "The group to stream")),
    responses((status = 200, description = "An open event stream", content_type = "text/event-stream")))]
async fn stream(state: CurrentAccount, Path(id): Path<String>) -> impl IntoResponse {
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
    // Seed the current turn too, so the pinned progress bar shows the group's
    // latest processing state the instant the client connects — independently of
    // how much message history it loads, and even for an event-triggered round
    // that left no user message to reconstruct it from.
    let turn_seed = tokio_stream::iter(
        state.current_turn(&id).and_then(|turn| to_sse_event(Ok(StreamEvent::Turn(turn)))),
    );
    let events = opened.chain(seed).chain(turn_seed).chain(live);
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
        StreamEvent::Suggestions(suggestions) => {
            Event::default().event("suggestions").json_data(suggestions).ok()?
        }
        StreamEvent::Turn(turn) => Event::default().event("turn").json_data(turn).ok()?,
    };
    Some(Ok(event))
}
