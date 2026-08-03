//! Admin-only account management: provisioning a new login for someone to
//! use, and listing the accounts that already exist. Not a workspace route —
//! see [`CurrentAdmin`], the admin role has no workspace of its own; this is
//! the operator managing *other* accounts' logins.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::api_error::ApiError;
use crate::models::{AccountSummary, CreateAccountRequest, UpdateAccountRequest};
use crate::state::{AppState, CurrentAdmin};

pub fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(routes!(list_accounts, create_account))
        .routes(routes!(update_account))
}

/// Every existing account, for the admin dashboard's account list.
#[utoipa::path(get, path = "/accounts", tag = "accounts",
    responses((status = 200, description = "Every account's id and login name; never a password", body = Vec<AccountSummary>)))]
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
        (status = 201, description = "The created account", body = AccountSummary),
        (status = 422, description = "Empty/reserved/taken username, empty password, or no persistent backend")))]
async fn create_account(
    _admin: CurrentAdmin,
    State(s): State<Arc<AppState>>,
    Json(body): Json<CreateAccountRequest>,
) -> Result<(StatusCode, Json<AccountSummary>), ApiError> {
    let (account_id, credentials) =
        s.create_account(&body.username, &body.password).map_err(ApiError::unprocessable)?;
    // 201, like every other creation in this API — it used to answer 200,
    // which said "here's a result" where the rest of the surface says "a new
    // resource now exists".
    Ok((StatusCode::CREATED, Json(AccountSummary { account_id, username: credentials.username })))
}

/// Changes an existing account's username, password, or both. Every live
/// session for that account is revoked: its tokens stop working immediately and
/// the holder must sign in again.
//
// The revocation is the point of the operation when an account is compromised.
// Before it existed, the old tokens kept working — the refresh token for
// another 30 days — so changing the password locked out only the person who
// didn't already have one.
#[utoipa::path(patch, path = "/accounts/{accountId}", tag = "accounts",
    params(("accountId" = String, Path, description = "The account to edit")),
    request_body = UpdateAccountRequest,
    responses(
        (status = 200, description = "The updated account; its sessions are revoked", body = AccountSummary),
        (status = 422, description = "Unknown id, empty/reserved/taken username, empty password, or no persistent backend")))]
async fn update_account(
    _admin: CurrentAdmin,
    State(s): State<Arc<AppState>>,
    Path(account_id): Path<String>,
    Json(body): Json<UpdateAccountRequest>,
) -> Result<Json<AccountSummary>, ApiError> {
    let credentials = s
        .update_account(&account_id, body.username.as_deref(), body.password.as_deref())
        .map_err(ApiError::unprocessable)?;
    Ok(Json(AccountSummary { account_id, username: credentials.username }))
}
