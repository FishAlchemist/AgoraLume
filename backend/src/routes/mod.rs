//! HTTP surface, split into the chat stream and the workspace resources.
//!
//! The route table is described with `utoipa`, so the OpenAPI document is
//! generated from the same handlers that serve traffic — it can never drift
//! from the implementation. This is the production API surface; only the data
//! behind it is provisional (in-memory store, simulated agent replies). Emit
//! `openapi.yml` with the binary's `--dump-openapi` flag.

mod chat;
mod llm;
mod workspace;

use std::sync::Arc;

use axum::Router;
use axum::http::{HeaderValue, Method};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa::openapi::OpenApi as OpenApiDoc;
use utoipa_axum::router::OpenApiRouter;

use crate::state::AppState;

/// Top-level API metadata. Paths and most schemas are contributed by the
/// handlers; `ReadReceipt` and `Turn` are added explicitly because they only
/// appear in the SSE stream (as `read` and `turn` events), never as a typed
/// request/response body.
#[derive(OpenApi)]
#[openapi(
    components(schemas(crate::models::ReadReceipt, crate::models::Turn)),
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
        (name = "settings", description = "Client preferences"),
        (name = "llm", description = "Operator: real-model provider configuration")
    )
)]
struct ApiDoc;

/// Assembles the full API as an `OpenApiRouter`, the single definition both the
/// running server and the spec generator draw from.
fn api() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .merge(chat::router())
        .merge(workspace::router())
        .merge(llm::router())
}

/// Builds the runtime router with CORS and request tracing. `cors_allowed_origins`
/// is `Config::cors_allowed_origins` — `None`/empty keeps the default wide open.
pub fn router(state: Arc<AppState>, cors_allowed_origins: Option<&[String]>) -> Router {
    // The frontend is a separate static origin (its dev server, or wherever the
    // SPA is hosted), so allow cross-origin access. By default that's *any*
    // origin: no request needs cookies or other ambient browser credentials, so
    // a wildcard doesn't grant a page access to another user's session the way
    // it would with cookie auth. It does mean the whole API — including
    // `PATCH /llm/settings`, which can update the stored provider key — is
    // reachable, unauthenticated, from any page open in the same browser;
    // that's the same trust model as the rest of this API (no auth anywhere
    // yet), not something new to this route.
    //
    // An operator who *does* know their frontend's origin can set
    // `AGORALUME_CORS_ORIGINS` to lock this down. `PATCH`/`DELETE` and our
    // JSON bodies make every mutating request "non-simple", so the browser
    // always preflights with `OPTIONS` before sending the real request; a
    // browser won't send that real request if the preflight's origin isn't in
    // the allow-list. That stops another tab in the same browser from issuing
    // the request at all — it doesn't require sessions, tokens, or any change
    // to the handlers themselves. It's still not real authentication: it's
    // enforced by the browser, so it does nothing against curl or a
    // same-origin page that's itself compromised.
    let allow_origin = allow_origin_from(cors_allowed_origins);
    let cors = CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ])
        .allow_headers(Any);

    let (router, _) = api().split_for_parts();
    router
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state)
}

/// Parses `AGORALUME_CORS_ORIGINS` entries into an `AllowOrigin`, falling back
/// to `Any` when unset, empty, every entry fails to parse as a header value
/// (an origin must not be blank, and can't contain characters like whitespace
/// or control characters — see `HeaderValue::from_str`), or any entry is
/// literally `*` (`AllowOrigin::list` panics on a wildcard; someone who typed
/// `*` meant "allow anything" anyway, which is the default). An invalid entry
/// is logged and skipped individually rather than rejected wholesale, so one
/// typo in a list of several doesn't quietly widen the allow-list back to
/// nothing configured.
fn allow_origin_from(origins: Option<&[String]>) -> AllowOrigin {
    let Some(origins) = origins else {
        return Any.into();
    };
    if origins.iter().any(|o| o.trim() == "*") {
        tracing::warn!(
            "AGORALUME_CORS_ORIGINS contains `*`; allowing any origin (the same as leaving it unset)"
        );
        return Any.into();
    }
    let parsed: Vec<HeaderValue> = origins
        .iter()
        .filter_map(|origin| match HeaderValue::from_str(origin) {
            Ok(value) => Some(value),
            Err(e) => {
                tracing::warn!(origin, error = %e, "invalid AGORALUME_CORS_ORIGINS entry, ignoring");
                None
            }
        })
        .collect();
    if parsed.is_empty() {
        tracing::warn!(
            "AGORALUME_CORS_ORIGINS was set but no entry parsed as a valid origin; falling back to allowing any origin"
        );
        Any.into()
    } else {
        AllowOrigin::list(parsed)
    }
}

/// The generated OpenAPI document, for serving or writing to `openapi.yml`.
pub fn openapi() -> OpenApiDoc {
    api().into_openapi()
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;

    use super::*;
    use crate::agent::turn::AgentRuntime;

    /// Sends a CORS preflight (`OPTIONS` + the two `Access-Control-Request-*`
    /// headers a browser adds automatically) for a `PATCH /llm/settings` from
    /// `origin`, and returns the `Access-Control-Allow-Origin` response header
    /// if one came back — absence is exactly what tells a real browser not to
    /// send the follow-up `PATCH`.
    async fn preflight_allow_origin(router: Router, origin: &str) -> Option<String> {
        let request = Request::builder()
            .method("OPTIONS")
            .uri("/llm/settings")
            .header(header::ORIGIN, origin)
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "PATCH")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .map(|v| v.to_str().unwrap().to_string())
    }

    fn state() -> Arc<AppState> {
        Arc::new(AppState::with_runtime(AgentRuntime::mock()))
    }

    #[tokio::test]
    async fn default_cors_allows_any_origin() {
        let allowed = preflight_allow_origin(router(state(), None), "https://evil.example").await;
        assert_eq!(allowed.as_deref(), Some("*"));
    }

    #[tokio::test]
    async fn configured_origin_is_allowed_and_echoed_back() {
        let origins = vec!["https://good.example".to_string()];
        let allowed =
            preflight_allow_origin(router(state(), Some(&origins)), "https://good.example").await;
        assert_eq!(allowed.as_deref(), Some("https://good.example"));
    }

    #[tokio::test]
    async fn other_origin_is_rejected_once_a_list_is_configured() {
        let origins = vec!["https://good.example".to_string()];
        let allowed =
            preflight_allow_origin(router(state(), Some(&origins)), "https://evil.example").await;
        assert_eq!(allowed, None);
    }

    #[tokio::test]
    async fn all_invalid_entries_falls_back_to_any() {
        // A header value can't contain a control character; this list has no
        // usable entry.
        let origins = vec!["bad\u{0}origin".to_string()];
        let allowed =
            preflight_allow_origin(router(state(), Some(&origins)), "https://evil.example").await;
        assert_eq!(allowed.as_deref(), Some("*"));
    }

    #[tokio::test]
    async fn wildcard_entry_falls_back_to_any_instead_of_panicking() {
        // `AllowOrigin::list` panics if given a literal "*" — someone who
        // wrote AGORALUME_CORS_ORIGINS=* meant "allow anything" anyway.
        let origins = vec!["*".to_string()];
        let allowed =
            preflight_allow_origin(router(state(), Some(&origins)), "https://evil.example").await;
        assert_eq!(allowed.as_deref(), Some("*"));
    }

    #[tokio::test]
    async fn wildcard_mixed_with_a_real_origin_still_allows_any() {
        let origins = vec!["https://good.example".to_string(), "*".to_string()];
        let allowed =
            preflight_allow_origin(router(state(), Some(&origins)), "https://evil.example").await;
        assert_eq!(allowed.as_deref(), Some("*"));
    }
}
