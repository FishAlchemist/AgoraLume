//! HTTP surface, split into the chat stream and the workspace resources.
//!
//! The route table is described with `utoipa`, so the OpenAPI document is
//! generated from the same handlers that serve traffic — it can never drift
//! from the implementation. This is the production API surface; only the data
//! behind it is provisional (in-memory store, simulated agent replies). Emit
//! `openapi.yml` with the binary's `--dump-openapi` flag.

mod accounts;
mod auth;
mod chat;
mod contract;
mod llm;
mod workspace;

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderValue, Method};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa::openapi::OpenApi as OpenApiDoc;
use utoipa_axum::router::OpenApiRouter;

use crate::routes::contract::SecurityAddon;
use crate::state::AppState;

/// Top-level API metadata. Paths and most schemas are contributed by the
/// handlers; `ReadReceipt` and `Turn` are added explicitly because they only
/// appear in the SSE stream (as `read` and `turn` events), never as a typed
/// request/response body.
#[derive(OpenApi)]
#[openapi(
    modifiers(&SecurityAddon),
    security(("bearerAuth" = [])),
    servers(
        (url = "/", description = "Same-origin: the bundled build, where the SPA and the API share a host"),
        (url = "/api", description = "Behind an edge/dev proxy (VITE_API_PREFIX=/api)"),
        (url = "http://127.0.0.1:8080", description = "A local backend (pnpm dev:api)")
    ),
    components(schemas(
        crate::models::ReadReceipt,
        crate::models::Turn,
        crate::api_error::ApiError
    )),
    info(
        title = "AgoraLume API",
        version = "0.1.0",
        description = "User + multi-AI group chat with modular personas. The backend owns the workspace and streams chat over SSE.\n\nPOST creates (201), PATCH merges a partial body (200), DELETE returns 204. Failures are RFC 9457 problem documents; `type` is a stable `urn:agoralume:error:…`."
    ),
    tags(
        (name = "service", description = "Liveness and server mode"),
        (name = "chat", description = "Messages and the live SSE stream"),
        (name = "organizations", description = "Top-level persona buckets"),
        (name = "departments", description = "Sub-units within an organization"),
        (name = "personas", description = "User identities and AI agents"),
        (name = "groups", description = "Chat rooms and their membership"),
        (name = "preferences", description = "The signed-in account's own display preferences"),
        (name = "diagnostics", description = "Token usage, estimated cost, and agent traces"),
        (name = "llm", description = "Operator: real-model provider configuration"),
        (name = "auth", description = "Login and token refresh, shared by every role"),
        (name = "accounts", description = "Admin: provisioning and listing accounts")
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
        .merge(accounts::router())
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
    // it would with cookie auth. Every route that needs it (per-account data
    // via `CurrentAccount`, operator config like `PATCH /llm/settings` via
    // `AuthenticatedSubject`) still requires its own bearer token when auth is
    // enforced — a wildcard origin doesn't bypass that, it just means any page
    // that already *has* a valid token for this server can send it cross-origin.
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
        // Nothing this API accepts is large: the biggest legitimate body is a
        // persona carrying a system prompt. axum's own default is 2 MiB, which
        // is generous for a JSON API whose every field is a name, a short text,
        // or a handful of ids — and a request body is read into memory before a
        // handler ever sees it, so the limit is what bounds the cost of a
        // deliberately oversized one.
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state)
}

/// The largest request body any endpoint accepts, in bytes.
const MAX_BODY_BYTES: usize = 512 * 1024;

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
///
/// The document-wide consistency passes run here rather than as derive-level
/// modifiers because they walk the operations, which only exist once the
/// handler routers have been merged — see [`contract::finalize`].
pub fn openapi() -> OpenApiDoc {
    let mut doc = versioned_api().into_openapi();
    contract::finalize(&mut doc);
    doc
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

    /// GETs `path` (unversioned, e.g. `/usage` — [`API_VERSION`] is
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
        let (status, body) = get_json(app.clone(), "/usage").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["requests"], 1);

        // The global by-persona breakdown — a distinct path, not a 404 or a
        // re-dispatch to `/usage` above.
        let (status, body) = get_json(app.clone(), "/usage/by-persona").await;
        assert_eq!(status, StatusCode::OK);
        let list = body.as_array().expect("a JSON array");
        assert!(
            list.iter().any(|p| p["personaId"] == "aria"),
            "aria's global entry should be present: {body}"
        );

        // The same two questions, scoped to one group by query parameter rather
        // than by a parallel pair of paths.
        let (status, body) = get_json(app.clone(), "/usage?groupId=lab").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["requests"], 1);

        let (status, body) = get_json(app.clone(), "/usage/by-persona?groupId=lab").await;
        assert_eq!(status, StatusCode::OK);
        let list = body.as_array().expect("a JSON array");
        assert!(
            list.iter().any(|p| p["personaId"] == "aria"),
            "aria's group-scoped entry should be present: {body}"
        );

        // A group the workspace doesn't have is a 404, not a silent zero — the
        // scoped and unscoped forms must not be told apart by guessing an id.
        let (status, _) = get_json(app, "/usage?groupId=no-such-group").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
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

    /// [`get_json_authed`], but also returns the parsed body — for routes
    /// whose response shape (not just its status) depends on who's asking,
    /// e.g. `LlmSettingsView.canEdit`.
    async fn get_json_authed_body(
        router: Router,
        path: &str,
        token: &str,
    ) -> (StatusCode, serde_json::Value) {
        let request = Request::builder()
            .uri(format!("{API_VERSION}{path}"))
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, body)
    }

    /// POSTs a JSON body to `path` (unversioned) with a bearer token attached,
    /// mirroring [`post_json`] + [`get_json_authed`] for the account-management
    /// routes, which are both authenticated and take a body.
    async fn post_json_authed(
        router: Router,
        path: &str,
        token: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let request = Request::builder()
            .method("POST")
            .uri(format!("{API_VERSION}{path}"))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, body)
    }

    /// PATCHes a JSON body to `path` (unversioned) with a bearer token
    /// attached, mirroring [`post_json_authed`] for `PATCH /accounts/{id}`.
    async fn patch_json_authed(
        router: Router,
        path: &str,
        token: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let request = Request::builder()
            .method("PATCH")
            .uri(format!("{API_VERSION}{path}"))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, body)
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
        assert_eq!(tokens["role"].as_str(), Some("admin"));
        let refresh_token = tokens["refreshToken"].as_str().expect("a refresh token");

        let (status, _) =
            post_json(app, "/auth/refresh", serde_json::json!({ "refreshToken": refresh_token })).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn llm_settings_401s_without_a_token_when_auth_is_enforced() {
        let (status, _) = get_json(router(not_mock_state(None, false), None), "/llm/settings").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn llm_settings_accepts_an_admin_token_unlike_a_per_account_route() {
        // Unlike `CurrentAccount` routes, `/llm/settings` isn't tied to any one
        // account — it's operator-level config, so (for now — see
        // `AuthenticatedSubject`'s docs) either role's token is enough, not
        // just a regular account's.
        let app = router(not_mock_state(None, false), None);
        let (_, tokens) = post_json(
            app.clone(),
            "/auth/login",
            serde_json::json!({ "username": "admin", "password": "admin-pw" }),
        )
        .await;
        let access_token = tokens["accessToken"].as_str().expect("an access token");

        let status = get_json_authed(app, "/llm/settings", access_token).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn writing_llm_settings_requires_the_admin_role_not_just_any_token() {
        // A regular account can read the shared LLM config (the test above)
        // but must not be able to change it or spend its stored key — see
        // `CurrentAdmin`'s docs on `routes::llm`.
        let dir = temp_dir();
        let account_dir = dir.join("accounts").join("acct-1");
        let creds = crate::auth::AccountCredentials {
            username: "alice".to_string(),
            password_hash: Some(crate::auth::hash_password("alice-pw")),
            allow_admin_readonly: false,
        };
        crate::persist::Persistence::new(&account_dir).save_credentials(&creds);

        let app = router(not_mock_state(Some(dir), false), None);
        let (_, tokens) = post_json(
            app.clone(),
            "/auth/login",
            serde_json::json!({ "username": "alice", "password": "alice-pw" }),
        )
        .await;
        let account_token = tokens["accessToken"].as_str().expect("an access token");

        // 403, not 401: the token is perfectly valid, it just isn't admin.
        // A client that saw 401 here would waste a refresh round-trip trying
        // to fix an authentication problem it doesn't have.
        let (status, _) = patch_json_authed(
            app.clone(),
            "/llm/settings",
            account_token,
            serde_json::json!({ "enabled": false }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, _) = post_json_authed(
            app,
            "/llm/models",
            account_token,
            serde_json::json!({ "baseUrl": "https://api.openai.com/v1" }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn llm_settings_reports_can_edit_matching_the_caller_s_actual_role() {
        // The frontend keys its write controls off `canEdit` instead of
        // re-deriving "am I admin" from its own copy of the session role
        // (see `LlmSettingsView::can_edit`'s docs) — this pins that the value
        // it reads actually matches what `PATCH`/`POST /llm/models` will do.
        let dir = temp_dir();
        let account_dir = dir.join("accounts").join("acct-1");
        let creds = crate::auth::AccountCredentials {
            username: "alice".to_string(),
            password_hash: Some(crate::auth::hash_password("alice-pw")),
            allow_admin_readonly: false,
        };
        crate::persist::Persistence::new(&account_dir).save_credentials(&creds);

        let app = router(not_mock_state(Some(dir), false), None);
        let admin_token = admin_access_token(app.clone()).await;
        let (_, tokens) = post_json(
            app.clone(),
            "/auth/login",
            serde_json::json!({ "username": "alice", "password": "alice-pw" }),
        )
        .await;
        let account_token = tokens["accessToken"].as_str().expect("an access token");

        let (status, body) = get_json_authed_body(app.clone(), "/llm/settings", &admin_token).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["canEdit"].as_bool(), Some(true));

        let (status, body) = get_json_authed_body(app, "/llm/settings", account_token).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["canEdit"].as_bool(), Some(false));
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
        // see `CurrentAccount`'s docs. 403, not 401: the token authenticated
        // fine, it just names a subject this route can't act as.
        let status = get_json_authed(app, "/organizations", access_token).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
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
        assert_eq!(tokens["role"].as_str(), Some("account"));
        let access_token = tokens["accessToken"].as_str().expect("an access token");

        let status = get_json_authed(app, "/organizations", access_token).await;
        assert_eq!(status, StatusCode::OK);
    }

    /// Logs in as the fixed admin account against a persisted `not_mock_state`
    /// and returns its access token — shared setup for the `/accounts` tests.
    async fn admin_access_token(app: Router) -> String {
        let (status, tokens) = post_json(
            app,
            "/auth/login",
            serde_json::json!({ "username": "admin", "password": "admin-pw" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        tokens["accessToken"].as_str().expect("an access token").to_string()
    }

    #[tokio::test]
    async fn admin_can_create_and_list_an_account_that_can_then_log_in() {
        let app = router(not_mock_state(Some(temp_dir()), false), None);
        let admin_token = admin_access_token(app.clone()).await;

        let (status, created) = post_json_authed(
            app.clone(),
            "/accounts",
            &admin_token,
            serde_json::json!({ "username": "bob", "password": "bob-pw" }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(created["username"].as_str(), Some("bob"));
        assert!(created["accountId"].as_str().is_some_and(|id| !id.is_empty()));

        let request = Request::builder()
            .uri(format!("{API_VERSION}/accounts"))
            .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let listed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            listed.as_array().unwrap().iter().any(|a| a["username"] == "bob"),
            "expected the newly created account in the list: {listed}"
        );

        let (status, tokens) = post_json(
            app,
            "/auth/login",
            serde_json::json!({ "username": "bob", "password": "bob-pw" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(tokens["role"].as_str(), Some("account"));
    }

    #[tokio::test]
    async fn creating_an_account_requires_the_admin_role_not_just_any_token() {
        let dir = temp_dir();
        let account_dir = dir.join("accounts").join("acct-1");
        let creds = crate::auth::AccountCredentials {
            username: "alice".to_string(),
            password_hash: Some(crate::auth::hash_password("alice-pw")),
            allow_admin_readonly: false,
        };
        crate::persist::Persistence::new(&account_dir).save_credentials(&creds);

        let app = router(not_mock_state(Some(dir), false), None);
        let (_, tokens) = post_json(
            app.clone(),
            "/auth/login",
            serde_json::json!({ "username": "alice", "password": "alice-pw" }),
        )
        .await;
        let account_token = tokens["accessToken"].as_str().expect("an access token");

        let (status, _) = post_json_authed(
            app,
            "/accounts",
            account_token,
            serde_json::json!({ "username": "carol", "password": "carol-pw" }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn creating_an_account_without_persistence_is_rejected() {
        let app = router(not_mock_state(None, false), None);
        let admin_token = admin_access_token(app.clone()).await;

        let (status, _) = post_json_authed(
            app,
            "/accounts",
            &admin_token,
            serde_json::json!({ "username": "dave", "password": "dave-pw" }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn creating_an_account_with_a_taken_username_is_rejected() {
        let app = router(not_mock_state(Some(temp_dir()), false), None);
        let admin_token = admin_access_token(app.clone()).await;

        let (status, _) = post_json_authed(
            app.clone(),
            "/accounts",
            &admin_token,
            serde_json::json!({ "username": "erin", "password": "erin-pw" }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);

        let (status, _) = post_json_authed(
            app,
            "/accounts",
            &admin_token,
            serde_json::json!({ "username": "erin", "password": "another-pw" }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn admin_can_edit_an_existing_account_username_and_password() {
        let app = router(not_mock_state(Some(temp_dir()), false), None);
        let admin_token = admin_access_token(app.clone()).await;

        let (_, created) = post_json_authed(
            app.clone(),
            "/accounts",
            &admin_token,
            serde_json::json!({ "username": "frank", "password": "frank-pw" }),
        )
        .await;
        let account_id = created["accountId"].as_str().expect("an account id");

        let (status, updated) = patch_json_authed(
            app.clone(),
            &format!("/accounts/{account_id}"),
            &admin_token,
            serde_json::json!({ "username": "franklin", "password": "franklin-pw" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(updated["username"].as_str(), Some("franklin"));
        assert_eq!(updated["accountId"].as_str(), Some(account_id));

        // The old username/password no longer log in; the new pair does.
        let (status, _) = post_json(
            app.clone(),
            "/auth/login",
            serde_json::json!({ "username": "frank", "password": "frank-pw" }),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, tokens) = post_json(
            app,
            "/auth/login",
            serde_json::json!({ "username": "franklin", "password": "franklin-pw" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(tokens["role"].as_str(), Some("account"));
    }

    #[tokio::test]
    async fn editing_an_unknown_account_id_is_rejected() {
        let app = router(not_mock_state(Some(temp_dir()), false), None);
        let admin_token = admin_access_token(app.clone()).await;

        let (status, _) = patch_json_authed(
            app,
            "/accounts/does-not-exist",
            &admin_token,
            serde_json::json!({ "username": "someone-else" }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn editing_an_account_requires_the_admin_role_not_just_any_token() {
        let app = router(not_mock_state(Some(temp_dir()), false), None);
        let admin_token = admin_access_token(app.clone()).await;

        let (_, created) = post_json_authed(
            app.clone(),
            "/accounts",
            &admin_token,
            serde_json::json!({ "username": "gina", "password": "gina-pw" }),
        )
        .await;
        let account_id = created["accountId"].as_str().expect("an account id");

        let (_, tokens) = post_json(
            app.clone(),
            "/auth/login",
            serde_json::json!({ "username": "gina", "password": "gina-pw" }),
        )
        .await;
        let account_token = tokens["accessToken"].as_str().expect("an access token");

        let (status, _) = patch_json_authed(
            app,
            &format!("/accounts/{account_id}"),
            account_token,
            serde_json::json!({ "username": "gina-the-second" }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn stream_route_requires_the_same_bearer_header_as_every_other_route() {
        // The group SSE stream is just another CurrentAccount route — no
        // query-param fallback. EventSource can't set headers, so the
        // frontend reads this stream via `fetch` instead (which can), rather
        // than the backend accepting a token in the URL, where a forwarding
        // proxy could log it.
        let dir = temp_dir();
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

        let status = get_json_authed(app.clone(), "/groups/lounge/stream", access_token).await;
        assert_eq!(status, StatusCode::OK);

        // A token in the query string alone must not work. Still 401 (not the
        // new 403): there is no `Authorization` header at all here, so the
        // server never authenticated anyone.
        let request = Request::builder()
            .uri(format!("{API_VERSION}/groups/lounge/stream?access_token={access_token}"))
            .body(Body::empty())
            .unwrap();
        let status = app.oneshot(request).await.unwrap().status();
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    /// Seeds one account with known credentials and returns the assembled
    /// router — the shared setup for the session-lifetime tests below.
    fn app_with_alice() -> Router {
        let dir = temp_dir();
        let account_dir = dir.join("accounts").join("acct-1");
        let creds = crate::auth::AccountCredentials {
            username: "alice".to_string(),
            password_hash: Some(crate::auth::hash_password("alice-pw")),
            allow_admin_readonly: false,
        };
        crate::persist::Persistence::new(&account_dir).save_credentials(&creds);
        router(not_mock_state(Some(dir), false), None)
    }

    #[tokio::test]
    async fn changing_an_account_password_cuts_off_its_live_sessions() {
        // The point of changing a compromised account's password is to lock out
        // whoever already has a session. Before revocation existed, their
        // access token kept working until it expired and their refresh token
        // for another 30 days — so the operator's one lever did nothing.
        let app = app_with_alice();
        let (_, tokens) = post_json(
            app.clone(),
            "/auth/login",
            serde_json::json!({ "username": "alice", "password": "alice-pw" }),
        )
        .await;
        let access_token = tokens["accessToken"].as_str().expect("an access token").to_string();
        let refresh_token = tokens["refreshToken"].as_str().expect("a refresh token").to_string();
        assert_eq!(get_json_authed(app.clone(), "/organizations", &access_token).await, StatusCode::OK);

        let admin_token = admin_access_token(app.clone()).await;
        let (status, _) = patch_json_authed(
            app.clone(),
            "/accounts/acct-1",
            &admin_token,
            serde_json::json!({ "password": "rotated-pw" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        assert_eq!(
            get_json_authed(app.clone(), "/organizations", &access_token).await,
            StatusCode::UNAUTHORIZED,
            "the old access token must stop working immediately"
        );
        let (status, _) =
            post_json(app, "/auth/refresh", serde_json::json!({ "refreshToken": refresh_token }))
                .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "the refresh token is the one that outlives everything; it must go too"
        );
    }

    #[tokio::test]
    async fn logout_revokes_the_presented_tokens_server_side() {
        let app = app_with_alice();
        let (_, tokens) = post_json(
            app.clone(),
            "/auth/login",
            serde_json::json!({ "username": "alice", "password": "alice-pw" }),
        )
        .await;
        let access_token = tokens["accessToken"].as_str().expect("an access token").to_string();
        let refresh_token = tokens["refreshToken"].as_str().expect("a refresh token").to_string();

        let (status, _) = post_json_authed(
            app.clone(),
            "/auth/logout",
            &access_token,
            serde_json::json!({ "refreshToken": refresh_token }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        assert_eq!(
            get_json_authed(app.clone(), "/organizations", &access_token).await,
            StatusCode::UNAUTHORIZED
        );
        let (status, _) =
            post_json(app, "/auth/refresh", serde_json::json!({ "refreshToken": refresh_token }))
                .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_message_cannot_be_authored_as_an_ai_persona() {
        // `personaId` was taken on trust, so a crafted request could put words
        // in a character's mouth — and those words then entered every other
        // agent's context as something that character had said.
        let app = router(state(), None);
        let (status, groups) = get_json(app.clone(), "/groups").await;
        assert_eq!(status, StatusCode::OK);
        let group_id = groups[0]["id"].as_str().expect("a seeded group").to_string();

        let (_, personas) = get_json(app.clone(), "/personas").await;
        let ai_id = personas
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["kind"] == "ai")
            .and_then(|p| p["id"].as_str())
            .expect("a seeded AI persona")
            .to_string();

        let request = Request::builder()
            .method("POST")
            .uri(format!("{API_VERSION}/groups/{group_id}/messages"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({ "text": "I agree with myself.", "personaId": ai_id })
                    .to_string(),
            ))
            .unwrap();
        let status = app.oneshot(request).await.unwrap().status();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
}
