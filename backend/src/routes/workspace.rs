//! Workspace CRUD: the backend owns organizations, departments, personas,
//! groups, and settings, so the client reads and mutates them through here
//! instead of holding its own authoritative copy.
//!
//! Conventions: `POST` creates (201 + body, server assigns the id); `PATCH`
//! merges a partial body `{ ...existing, ...patch }` (200 + body); `DELETE`
//! returns 204. Unknown ids give 404; refusing to delete the last user identity
//! gives 409.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::Value;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::models::{
    Department, Group, Organization, Persona, PromptLabel, PromptLabelInput, Settings,
};
use crate::state::AppState;
use crate::workspace::PersonaError;

pub fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(routes!(list_orgs, create_org))
        .routes(routes!(get_org, update_org, delete_org))
        .routes(routes!(list_depts, create_dept))
        .routes(routes!(get_dept, update_dept, delete_dept))
        .routes(routes!(list_personas, create_persona))
        .routes(routes!(get_persona, update_persona, delete_persona))
        .routes(routes!(list_prompt_labels))
        .routes(routes!(set_prompt_label))
        .routes(routes!(list_groups, create_group))
        .routes(routes!(get_group, update_group, delete_group))
        .routes(routes!(get_settings, update_settings))
}

// --- Organizations ----------------------------------------------------------

#[utoipa::path(get, path = "/organizations", tag = "organizations",
    responses((status = 200, body = Vec<Organization>)))]
async fn list_orgs(State(s): State<Arc<AppState>>) -> Json<Vec<Organization>> {
    Json(s.workspace().organizations.clone())
}

#[utoipa::path(get, path = "/organizations/{id}", tag = "organizations",
    params(("id" = String, Path)),
    responses((status = 200, body = Organization), (status = 404)))]
async fn get_org(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Organization>, StatusCode> {
    s.workspace()
        .organizations
        .iter()
        .find(|o| o.id == id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

#[utoipa::path(post, path = "/organizations", tag = "organizations",
    request_body = Organization,
    responses((status = 201, body = Organization)))]
async fn create_org(
    State(s): State<Arc<AppState>>,
    Json(body): Json<Organization>,
) -> (StatusCode, Json<Organization>) {
    let org = s.workspace().create_organization(body);
    s.persist_workspace();
    (StatusCode::CREATED, Json(org))
}

#[utoipa::path(patch, path = "/organizations/{id}", tag = "organizations",
    params(("id" = String, Path)),
    request_body = Organization,
    responses((status = 200, body = Organization), (status = 404)))]
async fn update_org(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(patch): Json<Value>,
) -> Result<Json<Organization>, StatusCode> {
    let updated = s.workspace().update_organization(&id, patch);
    if updated.is_some() {
        s.persist_workspace();
    }
    updated.map(Json).ok_or(StatusCode::NOT_FOUND)
}

#[utoipa::path(delete, path = "/organizations/{id}", tag = "organizations",
    params(("id" = String, Path)),
    responses((status = 204, description = "Deleted (cascades to its departments)"), (status = 404)))]
async fn delete_org(State(s): State<Arc<AppState>>, Path(id): Path<String>) -> StatusCode {
    if s.workspace().delete_organization(&id) {
        s.persist_workspace();
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

// --- Departments ------------------------------------------------------------

#[utoipa::path(get, path = "/departments", tag = "departments",
    responses((status = 200, body = Vec<Department>)))]
async fn list_depts(State(s): State<Arc<AppState>>) -> Json<Vec<Department>> {
    Json(s.workspace().departments.clone())
}

#[utoipa::path(get, path = "/departments/{id}", tag = "departments",
    params(("id" = String, Path)),
    responses((status = 200, body = Department), (status = 404)))]
async fn get_dept(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Department>, StatusCode> {
    s.workspace()
        .departments
        .iter()
        .find(|d| d.id == id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

#[utoipa::path(post, path = "/departments", tag = "departments",
    request_body = Department,
    responses((status = 201, body = Department)))]
async fn create_dept(
    State(s): State<Arc<AppState>>,
    Json(body): Json<Department>,
) -> (StatusCode, Json<Department>) {
    let dept = s.workspace().create_department(body);
    s.persist_workspace();
    (StatusCode::CREATED, Json(dept))
}

#[utoipa::path(patch, path = "/departments/{id}", tag = "departments",
    params(("id" = String, Path)),
    request_body = Department,
    responses((status = 200, body = Department), (status = 404)))]
async fn update_dept(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(patch): Json<Value>,
) -> Result<Json<Department>, StatusCode> {
    let updated = s.workspace().update_department(&id, patch);
    if updated.is_some() {
        s.persist_workspace();
    }
    updated.map(Json).ok_or(StatusCode::NOT_FOUND)
}

#[utoipa::path(delete, path = "/departments/{id}", tag = "departments",
    params(("id" = String, Path)),
    responses((status = 204), (status = 404)))]
async fn delete_dept(State(s): State<Arc<AppState>>, Path(id): Path<String>) -> StatusCode {
    if s.workspace().delete_department(&id) {
        s.persist_workspace();
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

// --- Personas ---------------------------------------------------------------

#[utoipa::path(get, path = "/personas", tag = "personas",
    responses((status = 200, body = Vec<Persona>)))]
async fn list_personas(State(s): State<Arc<AppState>>) -> Json<Vec<Persona>> {
    Json(s.workspace().personas.clone())
}

#[utoipa::path(get, path = "/personas/{id}", tag = "personas",
    params(("id" = String, Path)),
    responses((status = 200, body = Persona), (status = 404)))]
async fn get_persona(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Persona>, StatusCode> {
    s.workspace()
        .personas
        .iter()
        .find(|p| p.id == id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/// Creates a persona. Rejects (409) a duplicate name (names are globally unique)
/// or a second user identity (there is only ever one "you").
#[utoipa::path(post, path = "/personas", tag = "personas",
    request_body = Persona,
    responses(
        (status = 201, body = Persona),
        (status = 409, description = "Refused: name already in use, or a second user identity")))]
async fn create_persona(
    State(s): State<Arc<AppState>>,
    Json(body): Json<Persona>,
) -> Result<(StatusCode, Json<Persona>), StatusCode> {
    let created = s.workspace().create_persona(body);
    match created {
        Ok(persona) => {
            s.persist_workspace();
            Ok((StatusCode::CREATED, Json(persona)))
        }
        Err(_) => Err(StatusCode::CONFLICT),
    }
}

/// Updates a persona. 404 for an unknown id; 409 for a duplicate name or a change
/// that would produce a second user identity.
#[utoipa::path(patch, path = "/personas/{id}", tag = "personas",
    params(("id" = String, Path)),
    request_body = Persona,
    responses(
        (status = 200, body = Persona),
        (status = 404),
        (status = 409, description = "Refused: name already in use, or a second user identity")))]
async fn update_persona(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(patch): Json<Value>,
) -> Result<Json<Persona>, StatusCode> {
    let updated = s.workspace().update_persona(&id, patch);
    match updated {
        Ok(persona) => {
            s.persist_workspace();
            Ok(Json(persona))
        }
        Err(PersonaError::NotFound) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::CONFLICT),
    }
}

/// Deletes a persona. Refuses (409) to remove the last remaining user identity,
/// since every group still needs a "you".
#[utoipa::path(delete, path = "/personas/{id}", tag = "personas",
    params(("id" = String, Path)),
    responses(
        (status = 204),
        (status = 404),
        (status = 409, description = "Refused: last remaining user identity")))]
async fn delete_persona(State(s): State<Arc<AppState>>, Path(id): Path<String>) -> StatusCode {
    let mut ws = s.workspace();
    if !ws.personas.iter().any(|p| p.id == id) {
        return StatusCode::NOT_FOUND;
    }
    if ws.delete_persona(&id) {
        drop(ws);
        s.persist_workspace();
        StatusCode::NO_CONTENT
    } else {
        // Found but refused: it's the last remaining user identity.
        StatusCode::CONFLICT
    }
}

// --- Prompt identity labels -------------------------------------------------

/// All user-assigned names for persona identity hashes. The memory UI reads this
/// to show a friendly label (e.g. "bar 版") next to a persona's prompt version.
#[utoipa::path(get, path = "/prompt-labels", tag = "personas",
    responses((status = 200, body = Vec<PromptLabel>)))]
async fn list_prompt_labels(State(s): State<Arc<AppState>>) -> Json<Vec<PromptLabel>> {
    Json(s.workspace().prompt_labels())
}

/// Names an identity hash, or clears its name when the label is blank. Idempotent
/// (PUT), keyed by the full hash the client already holds from the persona.
#[utoipa::path(put, path = "/prompt-labels/{hash}", tag = "personas",
    params(("hash" = String, Path)),
    request_body = PromptLabelInput,
    responses((status = 200, body = PromptLabel)))]
async fn set_prompt_label(
    State(s): State<Arc<AppState>>,
    Path(hash): Path<String>,
    Json(body): Json<PromptLabelInput>,
) -> Json<PromptLabel> {
    let label = s.workspace().set_prompt_label(&hash, &body.label);
    s.persist_workspace();
    Json(label)
}

// --- Groups -----------------------------------------------------------------

#[utoipa::path(get, path = "/groups", tag = "groups",
    responses((status = 200, body = Vec<Group>)))]
async fn list_groups(State(s): State<Arc<AppState>>) -> Json<Vec<Group>> {
    Json(s.workspace().groups.clone())
}

#[utoipa::path(get, path = "/groups/{id}", tag = "groups",
    params(("id" = String, Path)),
    responses((status = 200, body = Group), (status = 404)))]
async fn get_group(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Group>, StatusCode> {
    s.workspace()
        .groups
        .iter()
        .find(|g| g.id == id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

#[utoipa::path(post, path = "/groups", tag = "groups",
    request_body = Group,
    responses((status = 201, body = Group)))]
async fn create_group(
    State(s): State<Arc<AppState>>,
    Json(body): Json<Group>,
) -> (StatusCode, Json<Group>) {
    let group = s.workspace().create_group(body);
    s.persist_workspace();
    (StatusCode::CREATED, Json(group))
}

#[utoipa::path(patch, path = "/groups/{id}", tag = "groups",
    params(("id" = String, Path)),
    request_body = Group,
    responses((status = 200, body = Group), (status = 404)))]
async fn update_group(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(patch): Json<Value>,
) -> Result<Json<Group>, StatusCode> {
    let updated = s.workspace().update_group(&id, patch);
    if updated.is_some() {
        s.persist_workspace();
    }
    updated.map(Json).ok_or(StatusCode::NOT_FOUND)
}

#[utoipa::path(delete, path = "/groups/{id}", tag = "groups",
    params(("id" = String, Path)),
    responses((status = 204), (status = 404)))]
async fn delete_group(State(s): State<Arc<AppState>>, Path(id): Path<String>) -> StatusCode {
    if s.workspace().delete_group(&id) {
        s.persist_workspace();
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

// --- Settings ---------------------------------------------------------------

#[utoipa::path(get, path = "/settings", tag = "settings",
    responses((status = 200, body = Settings)))]
async fn get_settings(State(s): State<Arc<AppState>>) -> Json<Settings> {
    Json(s.workspace().settings.clone())
}

#[utoipa::path(patch, path = "/settings", tag = "settings",
    request_body = Settings,
    responses((status = 200, body = Settings), (status = 422)))]
async fn update_settings(
    State(s): State<Arc<AppState>>,
    Json(patch): Json<Value>,
) -> Result<Json<Settings>, StatusCode> {
    let updated = s.workspace().update_settings(patch);
    if updated.is_some() {
        s.persist_workspace();
    }
    updated.map(Json).ok_or(StatusCode::UNPROCESSABLE_ENTITY)
}
