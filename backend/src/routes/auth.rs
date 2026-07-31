//! Login and token refresh — the one flow every role (admin and regular
//! accounts alike) shares, per the account-system design: "所有帳號都要同一組
//! 身分驗證機制，避免驗證的不一致". See [`crate::auth`] for the token scheme
//! itself (opaque, server-tracked, not JWT) and [`crate::state::CurrentAccount`]
//! for how a subsequent request's access token resolves to an account.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::Subject;
use crate::state::AppState;

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

/// Logs in as the admin (username `"admin"`) or a regular account (its own
/// stored username). An account with no fixed password yet accepts this
/// boot's generated one instead — see the server log at startup.
#[utoipa::path(post, path = "/auth/login", tag = "auth",
    request_body = LoginRequest,
    responses(
        (status = 200, body = TokenPair),
        (status = 401, description = "Unknown username or wrong password", body = String),
    ))]
async fn login(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<TokenPair>, (StatusCode, String)> {
    let (issued, subject) = state
        .login(&body.username, &body.password)
        .ok_or((StatusCode::UNAUTHORIZED, "invalid username or password".to_string()))?;
    let role = match subject {
        Subject::Admin => "admin",
        Subject::Account(_) => "account",
    };
    Ok(Json(TokenPair {
        access_token: issued.access_token,
        refresh_token: issued.refresh_token,
        role: role.to_string(),
    }))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct RefreshRequest {
    refresh_token: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct AccessToken {
    access_token: String,
}

/// Mints a fresh access token from a still-valid refresh token, without
/// asking for the password again. The refresh token itself is not rotated —
/// it keeps working until its own (much longer) expiry.
#[utoipa::path(post, path = "/auth/refresh", tag = "auth",
    request_body = RefreshRequest,
    responses(
        (status = 200, body = AccessToken),
        (status = 401, description = "Unknown, expired, or not actually a refresh token", body = String),
    ))]
async fn refresh(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RefreshRequest>,
) -> Result<Json<AccessToken>, (StatusCode, String)> {
    let access_token = state
        .refresh_access_token(&body.refresh_token)
        .ok_or((StatusCode::UNAUTHORIZED, "invalid or expired refresh token".to_string()))?;
    Ok(Json(AccessToken { access_token }))
}
