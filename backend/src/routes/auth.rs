//! Login and token refresh — the one flow every role (admin and regular
//! accounts alike) shares, per the account-system design: "所有帳號都要同一組
//! 身分驗證機制，避免驗證的不一致". See [`crate::auth`] for the token scheme
//! itself (opaque, server-tracked, not JWT) and [`crate::state::CurrentAccount`]
//! for how a subsequent request's access token resolves to an account.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::api_error::ApiError;
use crate::auth::Subject;
use crate::state::{AppState, LoginError};

pub fn router() -> OpenApiRouter<Arc<AppState>> {
    // Separate `.routes()` calls, not one `routes!(login, refresh)` group:
    // both are POST, and utoipa_axum's macro chains multiple handlers from
    // one group onto a single method router (the way `list_messages,
    // send_message` share one path with GET+POST) rather than registering
    // each at its own declared path — fine for distinct methods, but two
    // POSTs in one group panics at startup ("Overlapping method route").
    OpenApiRouter::new()
        .routes(routes!(login))
        .routes(routes!(refresh))
        .routes(routes!(logout))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct LoginRequest {
    /// The fixed admin login name, or a regular account's own username.
    username: String,
    password: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct TokenPair {
    /// Short-lived; sent as `Authorization: Bearer <token>` on every request.
    access_token: String,
    /// Long-lived; used only against `POST /auth/refresh` to mint a fresh
    /// access token without asking for the password again.
    refresh_token: String,
    /// `"admin"` or `"account"` — which kind of session this token belongs
    /// to, so the frontend can route an admin session to its own dashboard
    /// instead of a workspace it doesn't have. A UX routing hint only, not
    /// itself a permission grant: every route still checks the token's
    /// actual `Subject` server-side regardless of what this field says.
    role: String,
}

/// `"admin"` or `"account"`, straight from a resolved [`Subject`] — the one
/// place that mapping happens, so `login` and `refresh` can't disagree on it.
fn role_of(subject: &Subject) -> &'static str {
    match subject {
        Subject::Admin => "admin",
        Subject::Account(_) => "account",
    }
}

/// Logs in as the admin (username `"admin"`) or a regular account (its own
/// stored username). An account with no fixed password yet accepts this
/// boot's generated one instead — see the server log at startup.
#[utoipa::path(post, path = "/auth/login", tag = "auth", security(()),
    request_body = LoginRequest,
    responses(
        (status = 200, description = "A fresh token pair and the session's role", body = TokenPair),
        (status = 401, description = "Unknown username or wrong password (not distinguished)"),
        (status = 429, description = "Too many recent failed attempts for this username",
            headers(("Retry-After" = u32, description = "Seconds to wait before trying again")))))]
async fn login(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<TokenPair>, ApiError> {
    // One message for both "no such username" and "wrong password": saying
    // which would confirm whether an account exists. The timing is equalized
    // too — see `DUMMY_PASSWORD_HASH`.
    let (issued, subject) = state.login(&body.username, &body.password).map_err(|e| match e {
        LoginError::InvalidCredentials => {
            ApiError::unauthorized("invalid-credentials", "invalid username or password", "Bearer")
        }
        LoginError::TooManyAttempts { retry_after_secs } => ApiError::too_many_requests(
            "too-many-attempts",
            "too many recent failed attempts for this username",
            retry_after_secs,
        ),
    })?;
    Ok(Json(TokenPair {
        access_token: issued.access_token,
        refresh_token: issued.refresh_token,
        role: role_of(&subject).to_string(),
    }))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct RefreshRequest {
    refresh_token: String,
}

/// Mints a fresh access/refresh pair from a still-valid refresh token,
/// without asking for the password again. The presented refresh token is
/// rotated out — it stops working the moment its replacement is issued (a
/// short grace window covers two callers racing on the same one; see
/// `TokenStore::refresh`).
#[utoipa::path(post, path = "/auth/refresh", tag = "auth", security(()),
    request_body = RefreshRequest,
    responses(
        (status = 200, description = "A fresh token pair; the old refresh token stops working", body = TokenPair),
        (status = 401, description = "Unknown, expired, or already-rotated refresh token")))]
async fn refresh(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RefreshRequest>,
) -> Result<Json<TokenPair>, ApiError> {
    let (issued, subject) = state.refresh_access_token(&body.refresh_token).ok_or_else(|| {
        ApiError::unauthorized(
            "invalid-token",
            "invalid or expired refresh token",
            "Bearer error=\"invalid_token\"",
        )
    })?;
    Ok(Json(TokenPair {
        access_token: issued.access_token,
        refresh_token: issued.refresh_token,
        role: role_of(&subject).to_string(),
    }))
}

/// The body of a sign-out request.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct LogoutRequest {
    /// The session's refresh token, so it dies with the access token. Omitting
    /// it leaves a token good for another 30 days alive, which is almost never
    /// what a sign-out means.
    #[serde(default)]
    refresh_token: Option<String>,
}

/// Ends the current session: the presented access token (from the
/// `Authorization` header) and refresh token stop working immediately.
//
// There was no way to do this before — a client could forget its tokens
// locally, but the server kept honouring them until they expired.
//
// Public, deliberately: possessing a token is what entitles you to destroy it,
// and requiring a *valid* access token would mean an already-expired one
// stranded its refresh token with no way to retract it. Always answers 204,
// whether or not the tokens existed, so it can't probe which are live.
#[utoipa::path(post, path = "/auth/logout", tag = "auth", security(()),
    request_body = LogoutRequest,
    responses((status = 204, description = "Signed out (idempotent)")))]
async fn logout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<LogoutRequest>,
) -> StatusCode {
    let access_token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    state.logout(access_token, body.refresh_token.as_deref());
    StatusCode::NO_CONTENT
}
