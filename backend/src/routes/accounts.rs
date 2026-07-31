//! Admin-only account management: provisioning a new login for someone to
//! use, and listing the accounts that already exist. Not a workspace route —
//! see [`CurrentAdmin`], the admin role has no workspace of its own; this is
//! the operator managing *other* accounts' logins.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::models::{AccountSummary, CreateAccountRequest};
use crate::state::{AppState, CurrentAdmin};

pub fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new().routes(routes!(list_accounts, create_account))
}

/// Every existing account, for the admin dashboard's account list.
#[utoipa::path(get, path = "/accounts", tag = "accounts",
    responses(
        (status = 200, body = Vec<AccountSummary>),
        (status = 401, description = "Missing/invalid token, or a valid token that isn't the admin role", body = String),
    ))]
async fn list_accounts(_admin: CurrentAdmin, State(s): State<Arc<AppState>>) -> Json<Vec<AccountSummary>> {
    Json(
        s.list_accounts()
            .into_iter()
            .map(|(account_id, username)| AccountSummary { account_id, username })
            .collect(),
    )
}

/// Provisions a brand-new account with an admin-chosen username and
/// password. There's no self-service registration — this is the only way an
/// account gets created.
#[utoipa::path(post, path = "/accounts", tag = "accounts",
    request_body = CreateAccountRequest,
    responses(
        (status = 200, body = AccountSummary),
        (status = 401, description = "Missing/invalid token, or a valid token that isn't the admin role", body = String),
        (status = 422, description = "empty username/password, a reserved or already-taken username, or no persistent data directory configured", body = String),
    ))]
async fn create_account(
    _admin: CurrentAdmin,
    State(s): State<Arc<AppState>>,
    Json(body): Json<CreateAccountRequest>,
) -> Result<Json<AccountSummary>, (StatusCode, String)> {
    let (account_id, credentials) = s
        .create_account(&body.username, &body.password)
        .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e))?;
    Ok(Json(AccountSummary { account_id, username: credentials.username }))
}
