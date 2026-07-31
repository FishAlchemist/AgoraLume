//! HTTP surface, split into the chat stream and the workspace resources.
//!
//! The route table is described with `utoipa`, so the OpenAPI document is
//! generated from the same handlers that serve traffic — it can never drift
//! from the implementation. This is the production API surface; only the data
//! behind it is provisional (in-memory store, simulated agent replies). Emit
//! `openapi.yml` with the binary's `--dump-openapi` flag.

mod auth;
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
        (name = "llm", description = "Operator: real-model provider configuration"),
        (name = "auth", description = "Login and token refresh, shared by every role")
    )
)]
struct ApiDoc;

/// Assembles the full API as an `OpenApiRouter`, the single definition both the
/// running server and the spec generator draw from.
fn api() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .merge(chat::router())
        .merge(workspace::router())
        .merge(llm::router())
        .merge(auth::router())
}

/// The wire contract's version segment. Every route lives under this one
/// prefix — nothing in `api()` knows its own mount point — so bumping the
/// contract (`/v1beta` to `/v1`) is this one line, mirrored on the frontend by
/// `API_VERSION` in `frontend/src/lib/api/version.ts`.
const API_VERSION: &str = "/v1beta";

/// `api()` nested under [`API_VERSION`], carrying the top-level OpenAPI
/// metadata. `.nest()` prefixes the generated paths to match, so the spec and
/// the router can never disagree about where a route lives.
fn versioned_api() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::with_openapi(ApiDoc::openapi()).nest(API_VERSION, api())
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

    let (router, _) = versioned_api().split_for_parts();
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
    versioned_api().into_openapi()
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
    /// send the follow-up `PATCH`. Unversioned path in, [`API_VERSION`] applied
    /// here — same reason as [`get_json`].
    async fn preflight_allow_origin(router: Router, origin: &str) -> Option<String> {
        let request = Request::builder()
            .method("OPTIONS")
            .uri(format!("{API_VERSION}/llm/settings"))
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
        let operator = Arc::new(crate::state::OperatorState::new(AgentRuntime::mock()));
        Arc::new(AppState::new(operator, None, "test"))
    }

    /// GETs `path` (unversioned, e.g. `/debug/usage` — [`API_VERSION`] is
    /// applied here, the one place a version bump touches this test module)
    /// through the fully assembled router and returns the parsed JSON body —
    /// end-to-end through actual route dispatch, not just the `AppState`
    /// method underneath, so a route that got shadowed or mis-registered (as
    /// opposed to a wrong-answer bug in the method itself) would show up here.
    async fn get_json(router: Router, path: &str) -> (StatusCode, serde_json::Value) {
        let request = Request::builder()
            .uri(format!("{API_VERSION}{path}"))
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        // Some error responses (e.g. an auth rejection) are plain text, not
        // JSON — `Null` there too, since these tests only assert on `status`
        // in that case.
        let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, body)
    }

    #[tokio::test]
    async fn debug_usage_by_persona_resolves_independently_at_both_scopes() {
        use crate::models::{AgentTrace, TokenUsage};

        let state = state();
        state.account_by_id("test").record_trace(
            "lab",
            AgentTrace {
                ts: 0,
                group_id: "lab".to_string(),
                persona_id: "aria".to_string(),
                persona_name: "Aria".to_string(),
                system: String::new(),
                conversation: String::new(),
                action: "read".to_string(),
                message: None,
                mood: None,
                usage: Some(TokenUsage {
                    prompt_tokens: 10,
                    completion_tokens: 0,
                    total_tokens: 10,
                    cached_prompt_tokens: 0,
                }),
                model: None,
                duration_ms: None,
                estimated_cost: None,
            },
        );
        let app = router(state, None);

        // The plain site-wide total — unaffected by the sibling by-persona route.
        let (status, body) = get_json(app.clone(), "/debug/usage").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["requests"], 1);

        // The global by-persona breakdown — a distinct path, not a 404 or a
        // re-dispatch to `/debug/usage` above.
        let (status, body) = get_json(app.clone(), "/debug/usage/by-persona").await;
        assert_eq!(status, StatusCode::OK);
        let list = body.as_array().expect("a JSON array");
        assert!(
            list.iter().any(|p| p["personaId"] == "aria"),
            "aria's global entry should be present: {body}"
        );

        // The group-scoped by-persona breakdown — same last path segment, a
        // different route entirely.
        let (status, body) = get_json(app, "/groups/lab/debug/usage/by-persona").await;
        assert_eq!(status, StatusCode::OK);
        let list = body.as_array().expect("a JSON array");
        assert!(
            list.iter().any(|p| p["personaId"] == "aria"),
            "aria's group-scoped entry should be present: {body}"
        );
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

    // --- Auth: `state()` above is deliberately mock (bypasses login), which
    // is right for every test above but wrong for testing the gate itself —
    // these use a *not*-mock runtime (same rule-based brain, `mock: false`)
    // so `CurrentAccount` actually has to resolve a token. ---

    fn not_mock_state(data_dir: Option<std::path::PathBuf>, auth_disabled: bool) -> Arc<AppState> {
        use crate::agent::mock::RuleBrain;
        use crate::agent::turn::LoopConfig;
        let runtime = AgentRuntime::new(Arc::new(RuleBrain::new()), LoopConfig::default(), false);
        let operator = Arc::new(crate::state::OperatorState::new(runtime));
        let state = AppState::new(operator, data_dir, "test")
            .with_admin_auth(Some(crate::auth::hash_password("admin-pw")), auth_disabled);
        Arc::new(state)
    }

    fn temp_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("agoralume-auth-test-{}", uuid::Uuid::new_v4()))
    }

    /// POSTs a JSON body to `path` (unversioned) and returns the status and
    /// parsed body, mirroring [`get_json`] for the request shapes login and
    /// refresh need.
    async fn post_json(
        router: Router,
        path: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let request = Request::builder()
            .method("POST")
            .uri(format!("{API_VERSION}{path}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, body)
    }

    /// GETs `path` (unversioned) with a bearer token attached.
    async fn get_json_authed(router: Router, path: &str, token: &str) -> StatusCode {
        let request = Request::builder()
            .uri(format!("{API_VERSION}{path}"))
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        router.oneshot(request).await.unwrap().status()
    }

    #[tokio::test]
    async fn protected_route_401s_without_a_token_when_auth_is_enforced() {
        let (status, _) = get_json(router(not_mock_state(None, false), None), "/organizations").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_disabled_bypasses_login_even_when_not_mock() {
        let (status, _) = get_json(router(not_mock_state(None, true), None), "/organizations").await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn wrong_password_is_rejected() {
        let app = router(not_mock_state(None, false), None);
        let (status, _) = post_json(
            app,
            "/auth/login",
            serde_json::json!({ "username": "admin", "password": "wrong" }),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn admin_login_then_refresh_round_trips() {
        let app = router(not_mock_state(None, false), None);
        let (status, tokens) = post_json(
            app.clone(),
            "/auth/login",
            serde_json::json!({ "username": "admin", "password": "admin-pw" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let refresh_token = tokens["refreshToken"].as_str().expect("a refresh token");

        let (status, _) =
            post_json(app, "/auth/refresh", serde_json::json!({ "refreshToken": refresh_token })).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn admin_token_is_rejected_on_a_per_account_route() {
        let app = router(not_mock_state(None, false), None);
        let (_, tokens) = post_json(
            app.clone(),
            "/auth/login",
            serde_json::json!({ "username": "admin", "password": "admin-pw" }),
        )
        .await;
        let access_token = tokens["accessToken"].as_str().expect("an access token");

        // The admin role has no account/workspace of its own to resolve to —
        // see `CurrentAccount`'s docs.
        let status = get_json_authed(app, "/organizations", access_token).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn account_login_reaches_a_protected_route_with_its_access_token() {
        let dir = temp_dir();
        // Seed the account's credentials before the server ever opens it, so
        // login has a password to check against other than an unknowable
        // per-boot random one.
        let account_dir = dir.join("accounts").join("acct-1");
        let creds = crate::auth::AccountCredentials {
            username: "alice".to_string(),
            password_hash: Some(crate::auth::hash_password("alice-pw")),
            allow_admin_readonly: false,
        };
        crate::persist::Persistence::new(&account_dir).save_credentials(&creds);

        let app = router(not_mock_state(Some(dir), false), None);
        let (status, tokens) = post_json(
            app.clone(),
            "/auth/login",
            serde_json::json!({ "username": "alice", "password": "alice-pw" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let access_token = tokens["accessToken"].as_str().expect("an access token");

        let status = get_json_authed(app, "/organizations", access_token).await;
        assert_eq!(status, StatusCode::OK);
    }
}
